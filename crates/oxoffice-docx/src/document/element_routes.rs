//! Raw XML of single elements: reading and replacing a paragraph, run, table, cell or section properties.

use super::*;

fn direct_body_paragraph_slot(location: &ContentLocation) -> Option<usize> {
    if location.story().kind() != StoryKind::Body || location.index_path().len() != 1 {
        return None;
    }
    location.index_path().first().copied()
}

impl Document {
    /// Replace the visible text of one checked story item atomically.
    /// Replace one checked story item in its actual package owner.
    ///
    /// The item kind, physical story and surrounding bytes remain unchanged.
    /// Relationship-bearing content must already belong to this story part.
    /// The footnotes and endnotes only the replaced content referenced go too.
    pub fn replace_story_content(
        &mut self,
        location: &ContentLocation,
        fragment: ContentFragment,
    ) -> Result<()> {
        if location.is_end
            || location.item_kind != fragment.kind
            || location.item_kind == StoryItemKind::PreservedNode
        {
            return Err(Error::Other(
                "story replacement requires the same modeled item kind".to_owned(),
            ));
        }
        reject_section_owning_content_fragment(&fragment)?;
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let owner = if location.item_kind == StoryItemKind::Paragraph {
            let (_, owner) = candidate.story_source_and_owner(&location.story)?;
            Some(owner.clone())
        } else {
            None
        };
        let (source, item) = if location.item_kind == StoryItemKind::Paragraph {
            candidate.story_range_paragraph_source(location)?
        } else {
            candidate.story_item_source(location)?
        };
        if item.kind != location.item_kind {
            return Err(StoryError::KindMismatch {
                expected: location.item_kind,
                actual: item.kind,
            }
            .into());
        }
        let part_name = source.part_name.clone();
        let item_range = item.full.clone();
        let source_xml = source.xml.clone().into_owned();
        drop(source);
        let paired = match &owner {
            Some(owner)
                if source_xml
                    .windows(b"AlternateContent".len())
                    .any(|window| window == b"AlternateContent") =>
            {
                paired_scoped_ranges(&source_xml, owner, &item_range, None)?
            }
            _ => None,
        };
        let replacement = content_fragment_for_insertion(&candidate, &location.story, &fragment)?;
        // Replaced main-part content takes the notes only it referenced.
        let before = (part_name == candidate.doc_part_name).then(|| source_xml.clone());
        let mut xml = source_xml;
        if let Some(ranges) = paired {
            let mut edits = Vec::with_capacity(ranges.len());
            for range in ranges {
                let scope = story_namespace_scope_at(&xml, range.start)?;
                edits.push((
                    range,
                    close_content_fragment_namespaces(&replacement, &scope)?,
                ));
            }
            for (range, bytes) in edits.into_iter().rev() {
                xml.splice(range, bytes);
            }
        } else {
            let scope = story_namespace_scope_at(&xml, item_range.start)?;
            let replacement = close_content_fragment_namespaces(&replacement, &scope)?;
            xml.splice(item_range, replacement);
        }
        let after = before.as_ref().map(|_| xml.clone());
        set_story_source_xml(&mut candidate, &part_name, xml)?;
        if let (Some(before), Some(after)) = (before, after) {
            candidate.remove_unreferenced_notes(&before, &after)?;
        }
        candidate.reconcile_comment_removal(self)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Apply checked element content only when paragraph-owned section profiles
    /// match the selected original. Generic content insertion keeps its stricter
    /// section-owner refusal contract.
    fn replace_checked_element_content(
        &mut self,
        location: &ContentLocation,
        fragment: ContentFragment,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();

        candidate.flush_to_package()?;

        let (source, item) = if location.item_kind == StoryItemKind::Paragraph {
            candidate.story_range_paragraph_source(location)?
        } else {
            candidate.story_item_source(location)?
        };
        if location.is_end || item.kind != fragment.kind || item.kind != location.item_kind {
            return Err(Error::Other(
                "element XML requires the same existing physical owner".into(),
            ));
        }
        let part_name = source.part_name.clone();
        let scope = story_namespace_scope_at(&source.xml, item.full.start)?;
        let original = close_content_fragment_namespaces(&source.xml[item.full.clone()], &scope)?;
        let incoming = close_content_fragment_namespaces(&fragment.xml, &fragment.namespace_scope)?;
        let profiles = |xml: &[u8]| -> Result<Vec<CT_SectPr>> {
            let mut wrapped =
                format!(r#"<w:document xmlns:w="{WORD_NAMESPACE}"><w:body>"#).into_bytes();
            wrapped.extend_from_slice(xml);
            wrapped.extend_from_slice(b"</w:body></w:document>");
            // Checked paragraph XML writes canonical `w14:paraId` without a
            // local declaration. The wrapper root has to bind that prefix
            // before the section-profile parse, or the attribute is unbound.
            declare_w14_on_part_root(&mut wrapped)?;
            let document = CT_Document::from_xml(&wrapped)?;
            let mut profiles = Vec::new();
            visit_body_paragraphs(&document.body.content, false, &mut |paragraph| {
                if let Some(section) = paragraph
                    .properties
                    .as_ref()
                    .and_then(|properties| properties.sect_pr.as_ref())
                {
                    profiles.push(section.clone());
                }
            });
            Ok(profiles)
        };
        if profiles(&original)? != profiles(&incoming)? {
            return Err(Error::Other("element XML changed or introduced paragraph-owned section properties are unsupported, use the checked section owner".into()));
        }
        let mut xml = source.xml.into_owned();
        let replacement = content_fragment_for_insertion(&candidate, &location.story, &fragment)?;

        let replacement = close_content_fragment_namespaces(&replacement, &scope)?;

        // Replaced main-part content takes the notes only it referenced.
        let before = (part_name == candidate.doc_part_name).then(|| xml.clone());
        xml.splice(item.full, replacement);
        let after = before.as_ref().map(|_| xml.clone());
        set_story_source_xml(&mut candidate, &part_name, xml)?;
        if let (Some(before), Some(after)) = (before, after) {
            candidate.remove_unreferenced_notes(&before, &after)?;
        }

        candidate.reconcile_comment_removal(self)?;

        let reopened = candidate.prepare_and_reopen_staged()?;

        self.commit_staged_mutation(reopened);

        Ok(())
    }

    /// Namespace-complete XML for the held physical paragraph owner.
    #[doc(hidden)]
    pub fn paragraph_element_xml(&self, location: &ContentLocation) -> Result<Vec<u8>> {
        Ok(self
            .story_range_paragraph_snapshot(location)?
            .xml()
            .to_vec())
    }

    /// Namespace-complete XML for an accepted run in a held paragraph owner.
    #[doc(hidden)]
    pub fn run_element_xml(
        &self,
        location: &ContentLocation,
        path: &oxoffice_docx_oxml::text::AcceptedRunPath,
    ) -> Result<Vec<u8>> {
        let snapshot = self.story_range_paragraph_snapshot(location)?;
        let paragraph = CT_P::from_xml_fragment(snapshot.xml())?;
        let run = paragraph
            .accepted_run(path)
            .ok_or_else(|| Error::Other("element XML run owner is absent".into()))?;
        let mut bytes = Vec::new();
        run.to_xml(&mut Writer::new(&mut bytes))?;
        let scope = BTreeMap::from([
            ("w".to_owned(), WORD_NAMESPACE.to_owned()),
            (
                "r".to_owned(),
                oxoffice_docx_oxml::namespace::R_NS.to_owned(),
            ),
        ]);
        close_content_fragment_namespaces(&bytes, &scope)
    }

    /// The main part bytes for `item` alone in this body, and the item's
    /// namespace-complete XML there, closed as [`Self::close_direct_fragment`]
    /// closes it.
    pub(super) fn standalone_body_item(&self, item: &BodyContent) -> Result<(Vec<u8>, Vec<u8>)> {
        let part = self.document.standalone_body_item_xml(item)?;
        let (start, end) = standalone_body_item_span(&part)?;
        let xml = self.close_direct_fragment(&part, start..end)?;
        Ok((part, xml))
    }

    /// Close the fragment at `span` of a standalone main part over the
    /// declarations in scope there, as the whole part would close it, except
    /// for `w14`.
    ///
    /// The whole part declares `w14` on its root when any of its bytes use it,
    /// so a whole-part read carries that declaration whenever any body item
    /// uses it. A direct read declares it when the original root does or the
    /// fragment itself uses it, by name or in a markup compatibility list.
    fn close_direct_fragment(&self, part: &[u8], span: Range<usize>) -> Result<Vec<u8>> {
        let mut scope = story_namespace_scope_at(part, span.start)?;
        let fragment = &part[span];
        let root_declares = self
            .document
            .extra_namespaces
            .iter()
            .any(|(name, _)| name == "xmlns:w14");
        if !root_declares {
            if fragment_uses_w14(fragment)? {
                scope
                    .entry("w14".to_owned())
                    .or_insert_with(|| W14_NAMESPACE.to_owned());
            } else {
                scope.remove("w14");
            }
        }
        close_content_fragment_namespaces(fragment, &scope)
    }

    /// Namespace-complete XML for the direct body paragraph at `slot`, read
    /// without serializing, scanning or admitting the rest of the body.
    ///
    /// The XML is [`Self::paragraph_element_xml`] for that paragraph up to
    /// the `w14` declaration, which [`Self::close_direct_fragment`] decides
    /// from the root and the paragraph alone. `None` when the child is not a
    /// paragraph.
    #[doc(hidden)]
    pub fn direct_body_paragraph_xml(&self, slot: usize) -> Result<Option<Vec<u8>>> {
        let Some(item @ BodyContent::Paragraph(_)) = self.document.body.content.get(slot) else {
            return Ok(None);
        };
        let (_, xml) = self.standalone_body_item(item)?;
        CT_P::from_xml_fragment(&xml)?;
        Ok(Some(xml))
    }

    /// Namespace-complete XML for an accepted run of the direct body
    /// paragraph at `slot`, the bytes [`Self::run_element_xml`] returns for
    /// it, read without serializing or scanning the rest of the body.
    #[doc(hidden)]
    pub fn direct_body_run_xml(
        &self,
        slot: usize,
        path: &oxoffice_docx_oxml::text::AcceptedRunPath,
    ) -> Result<Option<Vec<u8>>> {
        let Some(item @ BodyContent::Paragraph(_)) = self.document.body.content.get(slot) else {
            return Ok(None);
        };
        // The run closes over a fixed scope, so the paragraph's own closure
        // only has to parse.
        let (_, xml) = self.standalone_body_item(item)?;
        let paragraph = CT_P::from_xml_fragment(&xml)?;
        let run = paragraph
            .accepted_run(path)
            .ok_or_else(|| Error::Other("element XML run owner is absent".into()))?;
        let mut bytes = Vec::new();
        run.to_xml(&mut Writer::new(&mut bytes))?;
        let scope = BTreeMap::from([
            ("w".to_owned(), WORD_NAMESPACE.to_owned()),
            (
                "r".to_owned(),
                oxoffice_docx_oxml::namespace::R_NS.to_owned(),
            ),
        ]);
        let closed = close_content_fragment_namespaces(&bytes, &scope)?;
        Ok(Some(closed))
    }

    /// Namespace-complete XML for the direct body table at `slot`, read as
    /// [`Self::direct_body_paragraph_xml`] reads a paragraph, the XML of
    /// [`Self::table_element_xml`] up to the `w14` declaration.
    #[doc(hidden)]
    pub fn direct_body_table_xml(&self, slot: usize) -> Result<Option<Vec<u8>>> {
        let Some(item @ BodyContent::Table(_)) = self.document.body.content.get(slot) else {
            return Ok(None);
        };
        Ok(Some(self.standalone_body_item(item)?.1))
    }

    /// Namespace-complete XML for a physical cell of the direct body table at
    /// `slot`, read as [`Self::direct_body_table_xml`] reads its table, the
    /// XML of [`Self::cell_element_xml`] up to the `w14` declaration, which
    /// the root and the cell alone decide.
    #[doc(hidden)]
    pub fn direct_body_cell_xml(
        &self,
        slot: usize,
        row: usize,
        cell: usize,
    ) -> Result<Option<Vec<u8>>> {
        let Some(item @ BodyContent::Table(_)) = self.document.body.content.get(slot) else {
            return Ok(None);
        };
        let (part, _) = self.standalone_body_item(item)?;
        let body = scan_story_owners(&part, StoryKind::Body)?
            .into_iter()
            .find(|owner| owner.kind == StoryKind::Body)
            .ok_or_else(|| Error::Other("standalone body item has no body owner".into()))?;
        let table = scan_story_items(&part, &body, None)?
            .into_iter()
            .find(|item| item.direct_owner_child)
            .ok_or_else(|| Error::Other("standalone body table has no item".into()))?;
        let owner = physical_table_cell_owner(&part, StoryKind::Body, &table, row, cell)?;
        let xml = self.close_direct_fragment(&part, owner.full)?;
        Ok(Some(xml))
    }

    /// The paragraph [`Self::insert_paragraph`] and [`Self::add_paragraph`]
    /// place for `text`, built before it is placed.
    #[doc(hidden)]
    pub fn detached_text_paragraph(text: &str) -> CT_P {
        let mut p = CT_P::new();
        if !text.is_empty() {
            p.add_run(text);
        }
        p
    }

    /// The accepted text and run paths that a checked owner inventory reads
    /// for `paragraph` placed as a direct body paragraph of this document.
    ///
    /// `None` unless the paragraph is plain and owns no section, so that it
    /// is one body story item and owns no other story or route.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // The two read facts of one paragraph owner.
    pub fn direct_body_paragraph_facts(
        &self,
        paragraph: &CT_P,
    ) -> Result<Option<(String, Vec<oxoffice_docx_oxml::text::AcceptedRunPath>)>> {
        if !paragraph_is_plain(paragraph)
            || paragraph
                .properties
                .as_ref()
                .is_some_and(|properties| properties.sect_pr.is_some())
        {
            return Ok(None);
        }
        paragraph.validate_resource_limits()?;
        // The undecided `w14` declaration names no prefix the paragraph uses.
        let (_, xml) =
            self.standalone_body_item(&BodyContent::Paragraph(Box::new(paragraph.clone())))?;
        let parsed = CT_P::from_xml_fragment(&xml)?;
        Ok(Some((parsed.accepted_text(), parsed.accepted_run_paths())))
    }

    /// Read plain opened body paragraphs under one exact opening proof.
    ///
    /// This finite reader-invariance proof returns facts only, never Source
    /// input or retained authority. Every miss leaves the full writer available.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // Existing paragraph text and run-path facts, in physical slot order.
    pub fn opened_plain_body_paragraph_facts(
        &self,
    ) -> Result<
        Option<
            Vec<(
                Option<Arc<BodyContent>>,
                String,
                Vec<oxoffice_docx_oxml::text::AcceptedRunPath>,
            )>,
        >,
    > {
        let Some(kept) = self.kept_main_part() else {
            return Ok(None);
        };
        if self.plain_local_edit_blocked
            || kept.serialization_proof.is_none()
            || kept.root_namespace_declarations != self.root_namespace_declarations
            || kept.body_namespace_declarations != self.body_namespace_declarations
            || kept.body_namespace_bindings != self.body_namespace_bindings
        {
            return Ok(None);
        }
        let Some(opened) = kept.main_document() else {
            return Ok(None);
        };
        let CT_Document {
            body:
                oxoffice_docx_oxml::document::CT_Body {
                    content: current,
                    sect_pr: current_section,
                },
            extra_namespaces: current_namespaces,
            background_xml: current_background,
            background_extra_xml: current_background_extra,
            root_attributes: current_attributes,
        } = &*self.document;
        let CT_Document {
            body:
                oxoffice_docx_oxml::document::CT_Body {
                    content: opened_content,
                    sect_pr: opened_section,
                },
            extra_namespaces: opened_namespaces,
            background_xml: opened_background,
            background_extra_xml: opened_background_extra,
            root_attributes: opened_attributes,
        } = opened;
        if (
            current_section,
            current_namespaces,
            current_background,
            current_background_extra,
            current_attributes,
        ) != (
            opened_section,
            opened_namespaces,
            opened_background,
            opened_background_extra,
            opened_attributes,
        ) || current.len() != opened_content.len()
        {
            return Ok(None);
        }
        let same_opening = kept
            .main_document
            .get()
            .and_then(Option::as_ref)
            .is_some_and(|opened| self.document.same_opening_clone(opened));
        // Compare and qualify each physical item while its values are in use.
        // Every opening item must match before any finite result or fallback.
        let facts = {
            let scans = self
                .main_body_scans
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut models = offered_canonical_blocks(
                scans.serialized.as_ref().or(self.main_part_blocks.as_ref()),
            )
            .filter(|record| record.blocks.len() == current.len());
            let staged_origin = same_opening
                && models.is_some_and(|record| {
                    kept.initial_writer_blocks
                        .as_ref()
                        .and_then(std::sync::Weak::upgrade)
                        .is_some_and(|opening| Arc::ptr_eq(&opening, &record.blocks))
                });
            let mut qualified_properties: Option<&oxoffice_docx_oxml::properties::CT_RPr> = None;
            let mut facts = Some(Vec::with_capacity(current.len()));
            for (slot, (item, opened_item)) in current.iter().zip(opened_content).enumerate() {
                let mut qualify = || {
                    let BodyContent::Paragraph(paragraph) = item else {
                        return None;
                    };
                    if !paragraph_has_finite_plain_properties(paragraph, &mut qualified_properties)
                    {
                        return None;
                    }
                    // These are the same finite writer-invariant facts.
                    let length = paragraph
                        .runs
                        .iter()
                        .flat_map(|run| &run.content)
                        .try_fold(0usize, |length, content| match content {
                            RunContent::Text(text) => length.checked_add(text.text.len()),
                            _ => None,
                        })
                        .filter(|length| *length <= isize::MAX as usize);
                    let text = length
                        .map(|length| {
                            let mut text = String::with_capacity(length);
                            for run in &paragraph.runs {
                                for content in &run.content {
                                    let RunContent::Text(content) = content else {
                                        return paragraph.accepted_text();
                                    };
                                    text.push_str(&content.text);
                                }
                            }
                            text
                        })
                        .unwrap_or_else(|| paragraph.accepted_text());
                    Some((text, paragraph.accepted_run_paths()))
                };
                // Only proved plain values may omit equality. Unsupported
                // equality can itself read XML and retains its original order.
                let early = (same_opening && facts.is_some()).then(&mut qualify);
                let finite_origin = early.as_ref().is_some_and(Option::is_some);
                if !finite_origin && item != opened_item {
                    return Ok(None);
                }
                if let Some(record) = models
                    && !(finite_origin && staged_origin)
                    && record.blocks[slot].staged.as_deref() != Some(item)
                {
                    models = None;
                }
                let Some(values) = facts.as_mut() else {
                    continue;
                };
                let finite = early.unwrap_or_else(qualify);
                match finite {
                    Some((text, runs)) => values.push((None, text, runs)),
                    None => facts = None,
                }
            }
            // Only a complete record alignment may lend any model value.
            if let (Some(values), Some(record)) = (facts.as_mut(), models) {
                for ((model, _, _), block) in values.iter_mut().zip(record.blocks.iter()) {
                    *model = block.staged.clone();
                }
            }
            facts
        };
        // The finite graph has exact resource maxima (0, 0). Unsupported
        // payloads retain the full guard, after the complete opening check
        // and after releasing the temporary writer-record lock.
        if facts.is_none() {
            self.document.validate_resource_limits()?;
        }
        Ok(facts)
    }

    /// The number of body story items that the direct body child at `slot`
    /// contributes, itself included, without scanning the rest of the body.
    #[doc(hidden)]
    pub fn direct_body_story_item_count(&self, slot: usize) -> Result<Option<usize>> {
        let Some(item) = self.document.body.content.get(slot) else {
            return Ok(None);
        };
        let (part, _) = self.standalone_body_item(item)?;
        let owner = scan_story_owners(&part, StoryKind::Body)?
            .into_iter()
            .find(|owner| owner.kind == StoryKind::Body)
            .ok_or_else(|| Error::Other("standalone body item has no body owner".into()))?;
        Ok(Some(scan_story_items(&part, &owner, None)?.len()))
    }

    /// The number of paragraphs [`Self::paragraphs`] counts before body child
    /// `slot`, without reading the body's XML.
    #[doc(hidden)]
    pub fn paragraph_count_before_content(&self, slot: usize) -> usize {
        self.document.body.paragraph_count_before(slot)
    }

    /// Whether removal takes the direct body child at `slot` alone.
    ///
    /// A plain paragraph with no section qualifies. Retained `w:sdt`
    /// namespace owners whose current replay still saves keep that removal
    /// local. Any other unsavable body stays on the checked staged path.
    #[doc(hidden)]
    pub fn plain_removable_paragraph(&self, slot: usize) -> bool {
        let plain = matches!(
            self.document.body.content.get(slot),
            Some(BodyContent::Paragraph(paragraph))
                if paragraph_is_plain(paragraph)
                    && paragraph
                        .properties
                        .as_ref()
                        .is_none_or(|properties| properties.sect_pr.is_none())
        );
        if !plain || self.unsavable_body_edit().is_some() {
            return false;
        }
        if !self.plain_local_edit_blocked {
            return true;
        }
        let Some(xml) = self.package.get_part(&self.doc_part_name) else {
            return false;
        };
        self.kept_main_part()
            .and_then(|kept| kept.nested_namespace_owners().map(<[_]>::to_vec))
            .or_else(|| self.main_part_blocks.as_ref()?.retained_nested_owners(xml))
            .or_else(|| nested_modeled_namespace_owners(xml).ok())
            .is_some_and(|owners| {
                !owners.is_empty() && owners.iter().all(|owner| owner.local_name == "sdt")
            })
    }

    /// Namespace-complete XML for an existing physical table owner.
    #[doc(hidden)]
    pub fn table_element_xml(&self, location: &ContentLocation) -> Result<Vec<u8>> {
        let (source, item) = self.story_item_source(location)?;
        if item.kind != StoryItemKind::Table || location.is_end {
            return Err(Error::Other(
                "element XML needs an existing physical table owner".into(),
            ));
        }
        let scope = story_namespace_scope_at(&source.xml, item.full.start)?;
        close_content_fragment_namespaces(&source.xml[item.full], &scope)
    }

    /// Namespace-complete XML for a physical cell, including an empty cell.
    #[doc(hidden)]
    pub fn cell_element_xml(
        &self,
        table: &ContentLocation,
        row: usize,
        cell: usize,
    ) -> Result<Vec<u8>> {
        let owner = self.story_table_cell(table, row, cell)?;
        let (source, span) = self.story_source_and_owner(&owner)?;
        let scope = story_namespace_scope_at(&source.xml, span.full.start)?;
        close_content_fragment_namespaces(&source.xml[span.full], &scope)
    }

    /// Namespace-complete XML for an existing section properties owner.
    #[doc(hidden)]
    pub fn section_properties_xml(&self, index: usize) -> Result<Vec<u8>> {
        let section = self
            .section(index)
            .ok_or_else(|| Error::Other("element XML section owner is absent".into()))?;
        let mut bytes = Vec::new();
        section.properties().to_xml(&mut Writer::new(&mut bytes))?;
        let scope = BTreeMap::from([
            ("w".to_owned(), WORD_NAMESPACE.to_owned()),
            (
                "r".to_owned(),
                oxoffice_docx_oxml::namespace::R_NS.to_owned(),
            ),
        ]);
        close_content_fragment_namespaces(&bytes, &scope)
    }

    /// Replace the checked existing paragraph owner atomically.
    #[doc(hidden)]
    pub fn replace_paragraph_xml(&mut self, location: &ContentLocation, xml: &str) -> Result<()> {
        self.validate_model_resource_limits()?;
        if let Some(slot) = direct_body_paragraph_slot(location) {
            match self.try_replace_plain_body_paragraph(slot, xml)? {
                None => {}
                Some(_) => return Ok(()),
            }
        }
        let selected = self.story_range_paragraph_snapshot(location)?;
        if selected.xml() == xml.as_bytes() {
            return Ok(());
        }
        let paragraph = CT_P::from_xml_fragment_checked(xml.as_bytes())?;
        let (source, item) = self.story_range_paragraph_source(location)?;
        let context = edited_paragraph_ancestor_context(&source.xml, item.full.start)?;
        admit_edited_paragraph(selected.xml(), xml.as_bytes(), &context)?;
        validate_element_owner_relationships(
            self,
            &location.story,
            WordXmlOwner::Paragraph(&paragraph),
        )?;
        validate_element_xml_identity_change(
            self,
            &location.story,
            selected.xml(),
            xml.as_bytes(),
        )?;
        let fragment = ContentFragment {
            kind: StoryItemKind::Paragraph,
            xml: xml.as_bytes().to_vec(),
            namespace_scope: BTreeMap::new(),
            source_part_name: Some(location.story.part_name.clone()),
            source_story_kind: Some(location.story.kind),
            source_owner_index: Some(location.story.owner_index),
            relationships: Vec::new(),
        };
        self.replace_checked_element_content(location, fragment)
    }

    #[doc(hidden)]
    /// Replace one existing accepted regular run through its physical
    /// paragraph owner, never by inserting a run at an old numeric slot.
    pub fn replace_run_xml(&mut self, position: &StoryRunPosition, xml: &str) -> Result<()> {
        self.validate_model_resource_limits()?;
        let snapshot = self.story_range_paragraph_snapshot(&position.location)?;
        let mut paragraph = CT_P::from_xml_fragment(snapshot.xml())?;
        let path = paragraph
            .accepted_run_paths()
            .get(position.run_index)
            .cloned()
            .ok_or_else(|| Error::Other("element XML run index is out of range".into()))?;
        let current = paragraph
            .accepted_run(&path)
            .ok_or_else(|| Error::Other("element XML run owner is absent".into()))?;
        if current
            .content
            .iter()
            .any(|content| matches!(content, RunContent::Field(_)))
        {
            return Err(Error::Other(
                "element XML regular run replacement cannot replace a field owner".into(),
            ));
        }
        let mut current_xml = Vec::new();
        current.to_xml(&mut Writer::new(&mut current_xml))?;
        let bindings = BTreeMap::from([
            ("w".to_owned(), WORD_NAMESPACE.to_owned()),
            (
                "r".to_owned(),
                oxoffice_docx_oxml::namespace::R_NS.to_owned(),
            ),
        ]);
        let current_xml = close_content_fragment_namespaces(&current_xml, &bindings)?;
        if current_xml == xml.as_bytes() {
            return Ok(());
        }
        let replacement = CT_R::from_xml_fragment_checked(xml.as_bytes())?;
        validate_element_owner_relationships(
            self,
            &position.location.story,
            WordXmlOwner::Run(&replacement),
        )?;
        validate_element_xml_identity_change(
            self,
            &position.location.story,
            &current_xml,
            xml.as_bytes(),
        )?;
        if !paragraph.replace_accepted_run(&path, replacement)? {
            return Err(Error::Other("element XML run owner disappeared".into()));
        }
        let mut fragment = ContentFragment::paragraph(paragraph)?;
        fragment.source_part_name = Some(position.location.story.part_name.clone());
        fragment.source_story_kind = Some(position.location.story.kind);
        fragment.source_owner_index = Some(position.location.story.owner_index);
        self.replace_checked_element_content(&position.location, fragment)
    }

    /// Replace one checked physical table atomically.
    #[doc(hidden)]
    pub fn replace_table_xml(&mut self, location: &ContentLocation, xml: &str) -> Result<()> {
        self.validate_model_resource_limits()?;

        let (source, item) = self.story_item_source(location)?;

        if item.kind != StoryItemKind::Table || location.is_end {
            return Err(Error::Other(
                "element XML needs an existing physical table owner".into(),
            ));
        }
        let scope = story_namespace_scope_at(&source.xml, item.full.start)?;
        let current = close_content_fragment_namespaces(&source.xml[item.full], &scope)?;
        if current == xml.as_bytes() {
            return Ok(());
        }
        let table = CT_Tbl::from_xml_fragment_checked(xml.as_bytes())?;

        validate_element_owner_relationships(self, &location.story, WordXmlOwner::Table(&table))?;

        validate_element_xml_identity_change(self, &location.story, &current, xml.as_bytes())?;

        let fragment = ContentFragment {
            kind: StoryItemKind::Table,
            xml: xml.as_bytes().to_vec(),
            namespace_scope: BTreeMap::new(),
            source_part_name: Some(location.story.part_name.clone()),
            source_story_kind: Some(location.story.kind),
            source_owner_index: Some(location.story.owner_index),
            relationships: Vec::new(),
        };
        self.replace_checked_element_content(location, fragment)
    }

    /// Replace exactly the source-qualified cell resolved by the held table.
    #[doc(hidden)]
    pub fn replace_cell_xml(
        &mut self,
        table: &ContentLocation,
        row: usize,
        cell: usize,
        xml: &str,
    ) -> Result<()> {
        self.validate_model_resource_limits()?;
        let owner = self.story_table_cell(table, row, cell)?;
        let (source, span) = self.story_source_and_owner(&owner)?;
        let scope = story_namespace_scope_at(&source.xml, span.full.start)?;
        let current = close_content_fragment_namespaces(&source.xml[span.full.clone()], &scope)?;
        if current == xml.as_bytes() {
            return Ok(());
        }
        let replacement = CT_Tc::from_xml_fragment_checked(xml.as_bytes())?;
        if replacement.content.is_empty() {
            return Err(Error::Other(
                "element XML cell requires modeled block content".into(),
            ));
        }
        validate_element_owner_relationships(self, &owner, WordXmlOwner::Cell(&replacement))?;
        validate_element_xml_identity_change(self, &owner, &current, xml.as_bytes())?;
        let original_table = self.story_table_model(table)?;
        let original_cell = original_table
            .rows
            .get(row)
            .and_then(|row| row.cells.get(cell))
            .ok_or_else(|| Error::Other("element XML original physical cell disappeared".into()))?;
        let profiles = |cell: &CT_Tc| {
            let mut profiles = Vec::new();
            visit_cell(cell, false, &mut |paragraph| {
                if let Some(section) = paragraph
                    .properties
                    .as_ref()
                    .and_then(|properties| properties.sect_pr.as_ref())
                {
                    profiles.push(section.clone());
                }
            });
            profiles
        };
        if profiles(original_cell) != profiles(&replacement) {
            return Err(Error::Other("element XML changed or introduced paragraph-owned section properties are unsupported, use the checked section owner".into()));
        }
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, span) = candidate.story_source_and_owner(&owner)?;
        let scope = story_namespace_scope_at(&source.xml, span.full.start)?;
        let part = source.part_name.clone();
        let mut bytes = source.xml.into_owned();
        bytes.splice(
            span.full,
            close_content_fragment_namespaces(xml.as_bytes(), &scope)?,
        );
        set_story_source_xml(&mut candidate, &part, bytes)?;
        // Only the closed candidate changed: replacing the qualified cell
        // cannot move its enclosing table or preceding owner boundary.
        // Rebind that candidate's story fingerprint before its topology check.
        let mut enclosing = table.clone();
        enclosing.story = candidate
            .stories()?
            .into_iter()
            .find(|story| {
                story.kind == table.story.kind
                    && story.part_name == table.story.part_name
                    && story.owner_index == table.story.owner_index
            })
            .ok_or_else(|| Error::Other("element XML enclosing table story disappeared".into()))?;
        validate_table_topology(&candidate.story_table_model(&enclosing)?)?;
        candidate.reconcile_comment_removal(self)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Replace an existing physical section owner through a closed candidate.
    #[doc(hidden)]
    pub fn replace_section_properties_xml(&mut self, index: usize, xml: &str) -> Result<()> {
        self.validate_model_resource_limits()?;
        let current = self
            .section(index)
            .ok_or_else(|| Error::Other("element XML section owner is absent".into()))?;
        let replacement = CT_SectPr::from_xml_fragment_checked(xml.as_bytes())?;
        for (references, kind) in [
            (&replacement.header_refs, HeaderFooterKind::Header),
            (&replacement.footer_refs, HeaderFooterKind::Footer),
        ] {
            let mut types = Vec::new();
            for reference in references {
                if types.contains(&reference.hdr_ftr_type) {
                    return Err(Error::Other(
                        "element XML section has duplicate header/footer types".into(),
                    ));
                }
                types.push(reference.hdr_ftr_type);
                let part = self
                    .header_footer_part_name(&reference.rel_id, kind == HeaderFooterKind::Header)
                    .ok_or_else(|| {
                        Error::Other(
                            "element XML section relationship kind or mode is invalid".into(),
                        )
                    })?;
                if self.package.content_types.content_type_for(&part) != Some(kind.content_type())
                    || self
                        .load_header_footer(&reference.rel_id, kind == HeaderFooterKind::Header)
                        .is_none()
                {
                    return Err(Error::Other(
                        "element XML section relationship target is invalid".into(),
                    ));
                }
            }
        }
        let mut bytes = Vec::new();
        current.properties().to_xml(&mut Writer::new(&mut bytes))?;
        let scope = BTreeMap::from([
            ("w".to_owned(), WORD_NAMESPACE.to_owned()),
            (
                "r".to_owned(),
                oxoffice_docx_oxml::namespace::R_NS.to_owned(),
            ),
        ]);
        if close_content_fragment_namespaces(&bytes, &scope)? == xml.as_bytes() {
            return Ok(());
        }
        let mut candidate = self.clone_for_staging();
        *candidate
            .section_mut(index)
            .ok_or_else(|| Error::Other("element XML section owner disappeared".into()))?
            .inner = replacement;
        if candidate
            .validate_sections()
            .findings
            .iter()
            .any(|finding| {
                finding.severity == ValidationSeverity::Error
                    && finding.message == format!("section {index} has nonpositive page size")
            })
        {
            return Err(Error::Other(
                "element XML section geometry has nonpositive page size".into(),
            ));
        }
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }
}
