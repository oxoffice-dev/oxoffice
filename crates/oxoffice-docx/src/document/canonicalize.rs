//! What a save does with identifiers: it keeps them. An edit allocates each
//! id when it inserts (decision 3 of the edit-cost spec); the save orders the
//! relationships edits added and writes every id as the document holds it.
//! Debug builds check, on every save and staged commit, that the save
//! changed no id and that the saved package reads them back.

use super::*;
#[cfg(debug_assertions)]
use std::collections::BTreeSet;

/// The number of an `rIdN` relationship id, others last, for ordering the
/// relationships edits added.
fn relationship_number(relationship: &oxoffice_opc::relationship::Relationship) -> u32 {
    relationship
        .id
        .strip_prefix("rId")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(u32::MAX)
}

impl Document {
    /// Give each picture a handle added from a relationship id, the ones
    /// marked [`CT_Inline::doc_pr_id_unallocated`], its drawing id, in
    /// document order, by the rule an insert uses. Every other id is kept,
    /// whatever its value.
    pub(super) fn allocate_handle_drawing_ids(&mut self) -> Result<()> {
        let mut pending = false;
        super::walkers::visit_all_drawings(&self.document.body.content, &mut |drawing| {
            pending |= drawing
                .inline
                .as_ref()
                .is_some_and(|inline| inline.doc_pr_id_unallocated);
        });
        // Reaching the body mutably drops the proof that it is the opened
        // model, so a body without such a picture is left untouched.
        if !pending {
            return Ok(());
        }
        let mut result = Ok(());
        let identifiers = &mut self.identifiers;
        visit_authored_drawings_mut(&mut self.document.body.content, &mut |drawing| {
            if let Some(inline) = &mut drawing.inline
                && inline.doc_pr_id_unallocated
                && result.is_ok()
            {
                match identifiers.reserve_drawing_id() {
                    Ok(allocated) => {
                        inline.doc_pr_id = allocated;
                        inline.doc_pr_id_unallocated = false;
                    }
                    Err(error) => result = Err(error),
                }
            }
        });
        result
    }

    /// Order the relationships edits added, as the save always wrote them:
    /// in the main part after the ones the document was read with, and in a
    /// header, footer, notes or comments part in their own slots, each by
    /// number. Every id keeps its value.
    pub(super) fn order_authored_relationships(&mut self) {
        let main = self.doc_part_name.clone();
        let preserved = self
            .identifiers
            .preserved_relationship_ids
            .get(&relationship_owner_identity(&main))
            .cloned()
            .unwrap_or_default();
        if let Some(relationships) = self.package.get_part_rels_mut(&main) {
            let mut authored = Vec::new();
            relationships.items.retain(|relationship| {
                if preserved.contains(&relationship.id) {
                    true
                } else {
                    authored.push(relationship.clone());
                    false
                }
            });
            authored.sort_by_key(relationship_number);
            relationships.items.extend(authored);
        }
        for owner in self.related_story_parts() {
            let identity = relationship_owner_identity(&owner);
            let fully_authored = self.identifiers.authored_story_parts.contains(&owner);
            let preserved = self
                .identifiers
                .preserved_relationship_ids
                .get(&identity)
                .cloned()
                .unwrap_or_default();
            let partially_authored = self
                .identifiers
                .authored_story_relationship_ids
                .get(&identity)
                .cloned()
                .unwrap_or_default();
            if !fully_authored && partially_authored.is_empty() {
                continue;
            }
            let Some(relationships) = self.package.get_part_rels_mut(&owner) else {
                continue;
            };
            let is_authored = |relationship: &oxoffice_opc::relationship::Relationship| {
                if fully_authored {
                    !preserved.contains(&relationship.id)
                } else {
                    partially_authored.contains(&relationship.id)
                }
            };
            let mut authored = relationships
                .items
                .iter()
                .filter(|relationship| is_authored(relationship))
                .cloned()
                .collect::<Vec<_>>();
            authored.sort_by_key(relationship_number);
            let mut authored = authored.into_iter();
            for relationship in &mut relationships.items {
                if is_authored(relationship) {
                    *relationship = authored
                        .next()
                        .expect("each authored relationship slot has a value");
                }
            }
        }
    }

    pub(super) fn opaque_relationship_ids_from_serialized_story(
        &self,
        modeled_relationships: &[String],
    ) -> Result<Vec<String>> {
        // Without modeled candidates there is nothing to mask. An unchanged
        // typed story can inventory its exact retained source, including opaque
        // property-root attributes that canonical serialization cannot replay.
        // Dirty stories and modeled relationships still need the checked mask.
        // The serialization shortcut still requires the opened model. Namespace
        // owners follow the opened part bytes after that model changes.
        let source_kept = self.kept_main_part();
        let model_kept = source_kept
            .clone()
            .filter(|_| self.opened_main_still_serializes());
        if modeled_relationships.is_empty()
            && let Some(source) = self.package.get_part(&self.doc_part_name)
            && {
                let matches = if model_kept.is_some() {
                    true
                } else if let Some(parsed) = source_kept.as_deref().and_then(Parsed::main_document)
                {
                    // The live model may differ, but these retained source
                    // bytes still have the exact parse from their opening.
                    parsed == &*self.document
                } else if let Some(proved) = self
                    .main_part_blocks
                    .as_ref()
                    .and_then(|blocks| blocks.proved_document_matches(source, &self.document))
                {
                    #[cfg(debug_assertions)]
                    {
                        let fresh = CT_Document::from_xml(source).ok();
                        assert_eq!(
                            fresh.as_ref() == Some(&*self.document),
                            proved,
                            "the proved main part disagrees with a fresh parse"
                        );
                    }
                    proved
                } else {
                    CT_Document::from_xml(source).ok().as_ref() == Some(&*self.document)
                };
                #[cfg(debug_assertions)]
                if let Some(kept) = model_kept.as_deref() {
                    {
                        assert_kept_main_document_without_initializing(
                            Some(kept),
                            Some(source),
                            &self.document,
                        );
                    }
                }
                matches
            }
        {
            return relationship_ids_by_blocks(source, self.main_part_blocks.as_ref(), &[]);
        }
        let scanned;
        let nested_namespace_owners = match source_kept
            .as_deref()
            .and_then(Parsed::nested_namespace_owners)
        {
            Some(owners) => {
                #[cfg(debug_assertions)]
                {
                    let source = self.package.get_part(&self.doc_part_name);
                    {
                        assert_eq!(
                            format!("{owners:?}"),
                            format!(
                                "{:?}",
                                source
                                    .map(nested_modeled_namespace_owners)
                                    .transpose()
                                    .ok()
                                    .flatten()
                                    .unwrap_or_default()
                            ),
                            "the kept nested namespace owners differ from a scan"
                        );
                    }
                }
                owners
            }
            None => {
                let nested_source = self.package.get_part(&self.doc_part_name);
                let proved_owners = nested_source.and_then(|xml| {
                    self.main_part_blocks
                        .as_ref()
                        .and_then(|blocks| blocks.retained_nested_owners(xml))
                });
                if let Some(owners) = proved_owners {
                    #[cfg(debug_assertions)]
                    {
                        let fresh = nested_source
                            .map(nested_modeled_namespace_owners)
                            .transpose()
                            .ok()
                            .flatten()
                            .unwrap_or_default();
                        assert_eq!(
                            format!("{owners:?}"),
                            format!("{fresh:?}"),
                            "the proved nested owners differ from a scan"
                        );
                    }
                    scanned = owners;
                    &scanned
                } else {
                    scanned = nested_source
                        .map(nested_modeled_namespace_owners)
                        .transpose()?
                        .unwrap_or_default();
                    &scanned
                }
            }
        };
        let serialize = |document: &CT_Document,
                         blocked: Result<(Vec<u8>, Option<MainPartBlocks>)>,
                         query_source: Option<&MainPartBlocks>|
         -> Result<(Vec<u8>, Option<MainPartBlocks>)> {
            match blocked {
                Ok((xml, record)) => {
                    let spans = record.as_ref().and_then(|record| {
                        query_source
                            .and_then(|source| record.masked_query_namespace_spans(&xml, source))
                            .or_else(|| record.proved_namespace_spans(&xml))
                    });
                    let replayed = replay_nested_namespace_declarations_with_spans(
                        &xml,
                        nested_namespace_owners,
                        spans.as_deref(),
                    )?;
                    Ok(relationship_xml_from_replayed(replayed, Ok((xml, record))))
                }
                Err(_) => {
                    // A failed optional writer proof does not replace the
                    // authoritative full writer or its original error.
                    let xml = document.to_xml()?;
                    let spans = self
                        .main_part_blocks
                        .as_ref()
                        .and_then(|blocks| blocks.proved_namespace_spans(&xml));
                    Ok((
                        replay_nested_namespace_declarations_with_spans(
                            &xml,
                            nested_namespace_owners,
                            spans.as_deref(),
                        )?,
                        None,
                    ))
                }
            }
        };
        let owners_empty = nested_namespace_owners.is_empty();
        if modeled_relationships.is_empty() {
            if owners_empty {
                let (current_xml, current_blocks) = self.canonical_source()?;
                return relationship_ids_by_blocks(&current_xml, current_blocks.as_ref(), &[]);
            }
            let (current_xml, current_blocks) =
                serialize(&self.document, self.canonical_source(), None)?;
            return relationship_ids_by_blocks(&current_xml, current_blocks.as_ref(), &[]);
        }

        let (current_xml, current_blocks) = if owners_empty {
            self.canonical_source()?
        } else {
            serialize(&self.document, self.canonical_source(), None)?
        };
        let current_relationships =
            relationship_ids_by_blocks(&current_xml, current_blocks.as_ref(), &[])?;
        let mut masked = self.document.clone();
        let mut masked_values = HashSet::new();
        let mut remap = HashMap::new();
        for (index, relationship_id) in modeled_relationships.iter().enumerate() {
            let mut ordinal = index;
            let sentinel = loop {
                let candidate = format!("rdocx-modeled-relationship-{ordinal}");
                if !current_relationships.contains(&candidate)
                    && masked_values.insert(candidate.clone())
                {
                    break candidate;
                }
                ordinal = ordinal
                    .checked_add(modeled_relationships.len().max(1))
                    .ok_or_else(|| {
                        Error::Other("modeled relationship mask range is exhausted".to_owned())
                    })?;
            };
            remap.insert(relationship_id.clone(), sentinel);
        }

        visit_authored_drawings_mut(&mut masked.body.content, &mut |drawing| {
            if let Some(mut drawing) = drawing
                .inline
                .as_mut()
                .map(DrawingMut::Inline)
                .or_else(|| drawing.anchor.as_mut().map(DrawingMut::Anchor))
            {
                drawing.remap_relationship(&remap);
            }
        });
        visit_body_paragraphs_mut(&mut masked.body.content, &mut |paragraph| {
            for hyperlink in &mut paragraph.hyperlinks {
                if let Some(id) = &mut hyperlink.rel_id
                    && let Some(updated) = remap.get(id)
                {
                    id.clone_from(updated);
                }
            }
            if let Some(section) = paragraph
                .properties
                .as_mut()
                .and_then(|properties| properties.sect_pr.as_mut())
            {
                for reference in section
                    .header_refs
                    .iter_mut()
                    .chain(&mut section.footer_refs)
                {
                    if let Some(updated) = remap.get(&reference.rel_id) {
                        reference.rel_id.clone_from(updated);
                    }
                }
            }
        });
        if let Some(section) = &mut masked.body.sect_pr {
            for reference in section
                .header_refs
                .iter_mut()
                .chain(&mut section.footer_refs)
            {
                if let Some(updated) = remap.get(&reference.rel_id) {
                    reference.rel_id.clone_from(updated);
                }
            }
        }

        let (masked_xml, masked_blocks) = if owners_empty {
            Self::canonical_source_from(&masked, current_blocks.as_ref())?
        } else {
            serialize(
                &masked,
                Self::canonical_source_from(&masked, current_blocks.as_ref()),
                current_blocks.as_ref(),
            )?
        };
        let masked_relationships = masked_blocks.as_ref().and_then(|masked| {
            current_blocks
                .as_ref()
                .and_then(|current| masked.masked_query_relationship_ids(&masked_xml, current))
        });
        let masked_relationships = match masked_relationships {
            Some(ids) => ids,
            None => relationship_ids_by_blocks(&masked_xml, masked_blocks.as_ref(), &[])?,
        };
        Ok(masked_relationships
            .into_iter()
            .filter(|id| !masked_values.contains(id) && current_relationships.contains(id))
            .collect())
    }

    pub(super) fn opened_main_still_serializes(&self) -> bool {
        let Some(kept) = self.kept_main_part() else {
            return false;
        };
        let Some(expected) = kept.serialization_proof.as_deref() else {
            return false;
        };
        if kept.root_namespace_declarations != self.root_namespace_declarations
            || kept.body_namespace_declarations != self.body_namespace_declarations
            || kept.body_namespace_bindings != self.body_namespace_bindings
            || kept.main_document() != Some(&*self.document)
        {
            return false;
        }
        self.serialized_main_part_matches(expected).unwrap_or(false)
    }

    fn active_header_footer_parts(&self) -> Vec<String> {
        let mut parts = Vec::new();
        for (relationship_id, is_header) in self.header_footer_rel_ids() {
            if let Some(part) = self.header_footer_part_name(&relationship_id, is_header)
                && !parts.contains(&part)
            {
                parts.push(part);
            }
        }
        parts
    }

    pub(super) fn related_story_parts(&self) -> Vec<String> {
        let mut parts = self.active_header_footer_parts();
        if let Some(relationships) = self.package.get_part_rels(&self.doc_part_name) {
            for relationship in &relationships.items {
                if matches!(
                    relationship.rel_type.as_str(),
                    rel_types::FOOTNOTES | rel_types::ENDNOTES | rel_types::COMMENTS
                ) && relationship_is_internal(relationship)
                {
                    let part =
                        OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target);
                    if self.package.contains_part(&part) && !parts.contains(&part) {
                        parts.push(part);
                    }
                }
            }
        }
        parts
    }

    pub(super) fn main_story_relationship_ids(&self) -> Vec<String> {
        let mut modeled = Vec::new();
        collect_relationship_ids(&self.document.body.content, &mut modeled);
        if let Some(section) = &self.document.body.sect_pr {
            for reference in section.header_refs.iter().chain(&section.footer_refs) {
                if !modeled.contains(&reference.rel_id) {
                    modeled.push(reference.rel_id.clone());
                }
            }
        }
        let owner = relationship_owner_identity(&self.doc_part_name);
        let nested = self
            .identifiers
            .authored_nested_story_relationship_ids
            .get(&owner)
            .cloned()
            .unwrap_or_default();
        let semantic = modeled
            .iter()
            .cloned()
            .chain(nested.iter().cloned())
            .collect::<HashSet<_>>();
        if let Ok((xml, blocks)) = self.canonical_source()
            && let Some(ordered) = {
                if let Some(all) = blocks
                    .as_ref()
                    .and_then(|blocks| blocks.proved_relationship_ids(&xml, &[]))
                {
                    let mut seen = HashSet::new();
                    let ordered = all
                        .into_iter()
                        .filter(|id| semantic.contains(id) && seen.insert(id.clone()))
                        .collect::<Vec<_>>();
                    (ordered.len() == semantic.len()).then_some(ordered)
                } else {
                    authored_relationship_ids_in_source_order(&xml, &semantic)
                }
            }
        {
            return ordered;
        }
        let mut nested = nested.into_iter().collect::<Vec<_>>();
        nested.sort();
        for id in nested {
            if !modeled.contains(&id) {
                modeled.push(id);
            }
        }
        modeled
    }
}

/// The identifiers reads show and a save writes, for the debug oracle of
/// decision 3: the ids of the main part (bookmarks, drawings, relationship
/// references), the numbering each body paragraph names, the numbering,
/// comment and footnote definitions, every relationship by owner, and the
/// media part names.
/// The relationships of one owner: id to type, target and target mode.
#[cfg(debug_assertions)]
type RelationshipTargets = BTreeMap<String, (String, String, Option<String>)>;

#[cfg(debug_assertions)]
#[derive(Debug, PartialEq, Eq)]
pub(super) struct IdentifierSummary {
    /// The bookmark and drawing ids of the main part, its relationship
    /// references and the numbering instances it names, in document order;
    /// none when the model does not serialize (the save then fails, and says
    /// why).
    main: Option<(Vec<IdentifierDefinition>, Vec<String>, Vec<String>)>,
    numbering: BTreeSet<(u32, u32)>,
    abstract_numbering: BTreeSet<u32>,
    relationships: BTreeMap<String, RelationshipTargets>,
    media: BTreeSet<String>,
}

#[cfg(debug_assertions)]
impl IdentifierSummary {
    /// The summary of what `document`'s models hold.
    pub(super) fn of(document: &Document) -> Self {
        Self::with_main(document, document.document.to_xml().ok())
    }

    /// The summary of `document` with the main part as its package holds it,
    /// which a reopen reads.
    pub(super) fn of_package(document: &Document) -> Self {
        let main = document
            .package
            .get_part(&document.doc_part_name)
            .map(<[u8]>::to_vec);
        Self::with_main(document, main)
    }

    fn with_main(document: &Document, main: Option<Vec<u8>>) -> Self {
        let categories = [
            IdentifierXmlCategory::Bookmark,
            IdentifierXmlCategory::Drawing,
        ]
        .into_iter()
        .collect();
        let (numbering, abstract_numbering) =
            document
                .numbering
                .as_ref()
                .map_or_else(Default::default, |numbering| {
                    (
                        numbering
                            .nums
                            .iter()
                            .map(|instance| (instance.num_id, instance.abstract_num_id))
                            .collect(),
                        numbering
                            .abstract_nums
                            .iter()
                            .map(|definition| definition.abstract_num_id)
                            .collect(),
                    )
                });
        let owners = std::iter::once(("/".to_owned(), &document.package.package_rels)).chain(
            document
                .package
                .part_rels
                .iter()
                .map(|(owner, relationships)| (part_name_identity(owner), relationships)),
        );
        Self {
            main: main.map(|xml| {
                (
                    scan_identifier_definitions(&xml, &categories).unwrap_or_default(),
                    xml_relationship_ids_in_order(&xml).unwrap_or_default(),
                    numbering_references(&xml),
                )
            }),
            numbering,
            abstract_numbering,
            relationships: owners
                .map(|(owner, relationships)| {
                    (
                        owner,
                        relationships
                            .items
                            .iter()
                            .map(|relationship| {
                                (
                                    relationship.id.clone(),
                                    (
                                        relationship.rel_type.clone(),
                                        relationship.target.clone(),
                                        relationship.target_mode.clone(),
                                    ),
                                )
                            })
                            .collect(),
                    )
                })
                .collect(),
            media: document
                .package
                .parts
                .keys()
                .filter(|part| part_name_identity(part).starts_with("/word/media/"))
                .cloned()
                .collect(),
        }
    }

    /// Panic unless `self`, the summary of a reopened package, reads the ids
    /// of `written`, the summary of the package as it was written. The
    /// models serialize some references in their own order (a section's
    /// header and footer references), so the main part's ids compare as
    /// multisets here; the save itself is checked in order by
    /// [`Self::assert_kept_from`].
    pub(super) fn assert_reads_back(&self, written: &Self) {
        let sorted = |summary: &Self| {
            summary
                .main
                .clone()
                .map(|(mut ids, mut references, mut numbering)| {
                    ids.sort_by_key(|id| format!("{id:?}"));
                    references.sort();
                    numbering.sort();
                    (ids, references, numbering)
                })
        };
        assert_eq!(
            (
                sorted(self),
                &self.numbering,
                &self.abstract_numbering,
                &self.relationships,
                &self.media
            ),
            (
                sorted(written),
                &written.numbering,
                &written.abstract_numbering,
                &written.relationships,
                &written.media
            ),
            "the saved package reads other identifiers than it holds"
        );
    }

    /// Panic unless the save that turned `before` into `self` kept every id.
    /// The save may add relationships and parts (a part a model needs) and
    /// drop some (a relationship nothing references any more), never give an
    /// id another value: an added relationship takes an id the document
    /// did not hold and no target a dropped one had.
    pub(super) fn assert_kept_from(&self, before: &Self, registry: &DocumentIdentifiers) {
        if let (Some(main), Some(before)) = (&self.main, &before.main) {
            assert_eq!(main, before, "the save renumbered an id of the main part");
        }
        assert_eq!(
            (&self.numbering, &self.abstract_numbering),
            (&before.numbering, &before.abstract_numbering),
            "the save renumbered a list"
        );
        for (owner, relationships) in &self.relationships {
            let kept = before.relationships.get(owner);
            for (id, target) in relationships {
                match kept.and_then(|kept| kept.get(id)) {
                    Some(previous) => assert_eq!(
                        target, previous,
                        "the save gave relationship {id} of {owner} another target"
                    ),
                    None => {
                        assert!(
                            !registry
                                .relationship_ids
                                .get(owner)
                                .is_some_and(|ids| ids.contains(id)),
                            "the save added relationship {id} of {owner}, an id the document held"
                        );
                        assert!(
                            !kept.is_some_and(|kept| kept
                                .iter()
                                .any(|(dropped, previous)| previous == target
                                    && !relationships.contains_key(dropped))),
                            "the save renamed a relationship of {owner} to {id}"
                        );
                    }
                }
            }
        }
        for added in self.media.difference(&before.media) {
            assert!(
                !registry.part_names.contains(&part_name_identity(added)),
                "the save added media {added}, a name the document held"
            );
        }
    }
}

/// The numbering instance each `w:numId` of `xml` names, in document order.
#[cfg(debug_assertions)]
fn numbering_references(xml: &[u8]) -> Vec<String> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut values = Vec::new();
    loop {
        match reader.read_resolved_event_into(&mut buffer) {
            Ok((
                ResolveResult::Bound(namespace),
                Event::Start(element) | Event::Empty(element),
            )) if namespace.as_ref() == WORD_NAMESPACE.as_bytes()
                && element.local_name().as_ref() == b"numId" =>
            {
                values.extend(
                    element
                        .attributes()
                        .flatten()
                        .filter(|attribute| attribute.key.local_name().as_ref() == b"val")
                        .map(|attribute| String::from_utf8_lossy(&attribute.value).into_owned()),
                );
            }
            Ok((_, Event::Eof)) | Err(_) => return values,
            _ => {}
        }
        buffer.clear();
    }
}

#[cfg(debug_assertions)]
impl DocumentIdentifiers {
    /// Panic unless every id `reopened`, a scan of the saved package, holds
    /// is reserved here, so that no later insert can take it again.
    pub(super) fn assert_covers(&self, reopened: &Self) {
        for (owner, ids) in &reopened.relationship_ids {
            let reserved = self.relationship_ids.get(owner);
            for id in ids {
                assert!(
                    reserved.is_some_and(|reserved| reserved.contains(id)),
                    "relationship {id} of {owner} is saved but not reserved"
                );
            }
        }
        let unreserved = |kind: &str, saved: Vec<String>| {
            assert!(saved.is_empty(), "{kind} saved but not reserved: {saved:?}");
        };
        unreserved(
            "bookmark ids",
            reopened
                .bookmark_ids
                .difference(&self.bookmark_ids)
                .map(ToString::to_string)
                .collect(),
        );
        unreserved(
            "comment ids",
            reopened
                .comment_ids
                .difference(&self.comment_ids)
                .map(ToString::to_string)
                .collect(),
        );
        unreserved(
            "drawing ids",
            reopened
                .drawing_ids
                .difference(&self.drawing_ids)
                .map(ToString::to_string)
                .collect(),
        );
        unreserved(
            "abstract numbering ids",
            reopened
                .abstract_numbering_ids
                .difference(&self.abstract_numbering_ids)
                .map(ToString::to_string)
                .collect(),
        );
        unreserved(
            "numbering instance ids",
            reopened
                .numbering_instance_ids
                .difference(&self.numbering_instance_ids)
                .map(ToString::to_string)
                .collect(),
        );
        unreserved(
            "part names",
            reopened
                .part_names
                .difference(&self.part_names)
                .cloned()
                .collect(),
        );
    }
}
