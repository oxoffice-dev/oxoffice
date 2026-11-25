//! The element XML replacements of a direct body paragraph, of one of its
//! runs and of a section's properties, and the scoped text replacement in a
//! direct body paragraph, edited in place.
//!
//! See `specs/2026-11-24-staged-edits-in-place.md`. Each route writes the
//! one direct body block it changes alone, between the head and the tail of
//! the main part (`body_block_item`), reads it back and swaps it in under a
//! `BodyBlocks { slot, count: 1 }` unit; the body's last section properties
//! go under `BodySection`. Nothing is staged, saved or reopened. A location
//! the route does not take answers `Ok(None)` and changes nothing: the caller
//! keeps today's staged route, which debug builds also run as the oracle of
//! every other answer.

use super::element_routes::paragraph_owned_section_profiles;
use super::fragment_checks::{
    WordXmlOwner, content_fragment_for_insertion,
    validate_element_owner_relationships_in_checked_story,
};
use super::story_text::bookmark_marker_spans;
use super::*;

/// Whether `incoming`, the XML that replaces the paragraph XML `previous`,
/// names the same comment and bookmark markers in the same order, every
/// note `previous` references, and no id the registry does not hold for
/// `previous` (drawing, bookmark, content control and permission ids, each
/// at most as often). Then the replacement removes no note, comment or
/// bookmark another part of the body could still name, and reserves no id:
/// nothing outside the paragraph needs reading. `false` for anything else,
/// and for XML these scans do not read, which the staged route answers.
fn same_paragraph_identities(previous: &[u8], incoming: &[u8]) -> bool {
    let comments = |xml: &[u8]| {
        crate::comments::comment_source_markers(xml).map(|markers| {
            markers
                .iter()
                .map(|marker| (marker.id, marker.family))
                .collect::<Vec<_>>()
        })
    };
    let bookmarks = |xml: &[u8]| {
        bookmark_marker_spans(xml).map(|markers| {
            markers
                .into_iter()
                .map(|marker| (marker.start, marker.id))
                .collect::<Vec<_>>()
        })
    };
    let definitions = |xml: &[u8]| {
        scan_identifier_definitions(xml, &STORY_CATEGORIES.into_iter().collect()).map(
            |definitions| {
                let mut counts = HashMap::<IdentifierDefinition, usize>::new();
                for definition in definitions {
                    *counts.entry(definition).or_default() += 1;
                }
                counts
            },
        )
    };
    let same = || -> Result<bool> {
        if comments(previous)? != comments(incoming)?
            || bookmarks(previous)? != bookmarks(incoming)?
        {
            return Ok(false);
        }
        let (before, after) = (
            content::note_reference_ids_in(previous)?,
            content::note_reference_ids_in(incoming)?,
        );
        if !before.footnotes.is_subset(&after.footnotes)
            || !before.endnotes.is_subset(&after.endnotes)
        {
            return Ok(false);
        }
        let (Some(held), Some(written)) = (definitions(previous), definitions(incoming)) else {
            return Ok(false);
        };
        Ok(written
            .iter()
            .all(|(definition, count)| held.get(definition).is_some_and(|held| held >= count)))
    };
    same().unwrap_or(false)
}

/// `block` without the namespace declarations of its first start tag that
/// bind a prefix to the namespace `scope`, the bindings in force around the
/// block, already binds it to: the same element in the same namespaces.
/// `None` when the block does not start with a start tag.
fn without_declarations_in_scope(
    block: &[u8],
    scope: &BTreeMap<String, String>,
) -> Option<Vec<u8>> {
    let mut reader = quick_xml::Reader::from_reader(block);
    reader.config_mut().trim_text(false);
    let (element, empty) = match reader.read_event().ok()? {
        Event::Start(element) => (element, false),
        Event::Empty(element) => (element, true),
        _ => return None,
    };
    let end = reader.buffer_position() as usize;
    let mut kept = BytesStart::new(String::from_utf8(element.name().as_ref().to_vec()).ok()?);
    let mut removed = false;
    for attribute in element.attributes() {
        let attribute = attribute.ok()?;
        let key = attribute.key.as_ref();
        let prefix = match key {
            b"xmlns" => Some(""),
            _ => key
                .strip_prefix(b"xmlns:")
                .and_then(|p| std::str::from_utf8(p).ok()),
        };
        if let Some(prefix) = prefix
            && scope.get(prefix).map(String::as_bytes) == Some(attribute.value.as_ref())
        {
            removed = true;
            continue;
        }
        kept.push_attribute(attribute);
    }
    if !removed {
        return Some(block.to_vec());
    }
    let mut out = Vec::with_capacity(block.len());
    let mut writer = Writer::new(&mut out);
    let event = if empty {
        Event::Empty(kept)
    } else {
        Event::Start(kept)
    };
    writer.write_event(event).ok()?;
    out.extend_from_slice(&block[end..]);
    Some(out)
}

impl Document {
    /// The direct body paragraph a location names, written alone in its
    /// block ([`Self::body_block_item`]), when `location` is a paragraph of
    /// the main part's body that is itself the direct body block `block`
    /// names. `None` for any other location, and for a body that cannot be
    /// saved after a typed edit.
    fn direct_paragraph_block_item(
        &self,
        location: &ContentLocation,
        block: crate::ControlBlock,
    ) -> Result<Option<BodyBlockItem>> {
        let story = &location.story;
        if story.kind != StoryKind::Body
            || story.owner_index != 0
            || block.owner_index != 0
            || block.item_index != 0
            || location.is_end
            || location.index_path.len() != 1
            || location.item_kind != StoryItemKind::Paragraph
            || story.part_name != self.doc_part_name
            || !matches!(
                self.document.body.content.get(block.slot),
                Some(BodyContent::Paragraph(_))
            )
        {
            return Ok(None);
        }
        self.body_block_item(
            block.slot,
            (StoryKind::Body, 0),
            0,
            StoryItemKind::Paragraph,
        )
    }

    /// Swap in the paragraph `fragment` holds for the direct body paragraph
    /// of `source` at `slot`, whose namespace-complete XML is `previous`, as
    /// the staged element replacement splices it: `Some(changed)`. `None`,
    /// changing nothing, when the replacement changes a marker or an id
    /// ([`same_paragraph_identities`]) or does not read back as it writes.
    fn swap_checked_paragraph_in_block(
        &mut self,
        location: &ContentLocation,
        slot: usize,
        source: &BodyBlockItem,
        previous: &[u8],
        fragment: ContentFragment,
    ) -> Result<Option<bool>> {
        if !same_paragraph_identities(previous, &fragment.xml) {
            return Ok(None);
        }
        let scope = story_namespace_scope_at(&source.xml, source.item.start)?;
        let original = close_content_fragment_namespaces(&source.xml[source.item.clone()], &scope)?;
        let incoming = close_content_fragment_namespaces(&fragment.xml, &fragment.namespace_scope)?;
        if paragraph_owned_section_profiles(&original)?
            != paragraph_owned_section_profiles(&incoming)?
        {
            return Err(Error::Other("element XML changed or introduced paragraph-owned section properties are unsupported, use the checked section owner".into()));
        }
        let replacement = content_fragment_for_insertion(self, &location.story, &fragment)?;
        let replacement = close_content_fragment_namespaces(&replacement, &scope)?;
        let mut updated = source.xml.clone();
        updated.splice(source.item.clone(), replacement);
        self.swap_block_xml(slot, source, &updated)
    }

    /// Swap in the block `updated`, an edit of `source`'s XML, for the
    /// direct body block at `slot`: `Some(changed)`. `None`, changing
    /// nothing, when the edit changed the head or the tail of the part or the
    /// block does not read back as it writes.
    fn swap_block_xml(
        &mut self,
        slot: usize,
        source: &BodyBlockItem,
        updated: &[u8],
    ) -> Result<Option<bool>> {
        if updated.len() < source.head.len() + source.tail
            || updated[..source.head.len()] != source.head[..]
            || updated[updated.len() - source.tail..] != *source.tail_bytes()
        {
            return Ok(None);
        }
        // The staged splice declares the part's namespaces again on the
        // block; the model writes them once, on the root.
        let scope = story_namespace_scope_at(&source.xml, source.item.start)?;
        let Some(block) = without_declarations_in_scope(source.block_of(updated), &scope) else {
            return Ok(None);
        };
        let Some(parsed) = Self::reparse_body_block(&source.head, source.tail_bytes(), &block)
        else {
            return Ok(None);
        };
        // `body_block_item` answered that the body saves after a typed edit;
        // a block that declares no other namespace keeps that answer, which
        // would otherwise be asked again over the whole main part.
        let same_declarations = crate::content_control::declares_only_word_extensions(
            source.block_of(&source.xml),
            &block,
        );
        let changed = self.replace_body_block_in_place(slot, parsed)?;
        if changed && same_declarations {
            self.keep_body_savable_after_swap();
        }
        Ok(Some(changed))
    }

    /// [`Self::replace_paragraph_xml`] on a direct body paragraph, the block
    /// `block` names (found by the caller), edited in place under a
    /// `BodyBlocks { slot, count: 1 }` unit: `Some(changed)`, with the
    /// staged route's checks and errors; a refusal changes nothing. `None`,
    /// changing nothing, for any other location, a body that cannot be saved
    /// after a typed edit, a replacement that changes a comment or bookmark
    /// marker, drops a note reference or writes an id the paragraph did not
    /// hold, and XML that does not read back as it writes: the caller then
    /// takes [`Self::replace_paragraph_xml`].
    #[doc(hidden)]
    pub fn replace_paragraph_xml_in_block(
        &mut self,
        location: &ContentLocation,
        block: crate::ControlBlock,
        xml: &str,
    ) -> Result<Option<bool>> {
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let result = self.replace_paragraph_xml_in_block_unchecked(location, block, xml);
        #[cfg(debug_assertions)]
        self.assert_block_route_matches_staged(
            before,
            &result,
            true,
            |_| (),
            |staged| {
                let location = staged.fingerprinted(location);
                staged.replace_paragraph_xml(&location, xml)
            },
        );
        result
    }

    fn replace_paragraph_xml_in_block_unchecked(
        &mut self,
        location: &ContentLocation,
        block: crate::ControlBlock,
        xml: &str,
    ) -> Result<Option<bool>> {
        let Some(source) = self.direct_paragraph_block_item(location, block)? else {
            return Ok(None);
        };
        let Some(selected) = self.direct_body_paragraph_xml(block.slot)? else {
            return Ok(None);
        };
        if selected == xml.as_bytes() {
            return Ok(Some(false));
        }
        let paragraph = CT_P::from_xml_fragment_checked(xml.as_bytes())?;
        let context = edited_paragraph_ancestor_context(&source.xml, source.item.start)?;
        admit_edited_paragraph(&selected, xml.as_bytes(), &context)?;
        validate_element_owner_relationships_in_checked_story(
            self,
            &location.story,
            WordXmlOwner::Paragraph(&paragraph),
        )?;
        let fragment = ContentFragment {
            kind: StoryItemKind::Paragraph,
            xml: xml.as_bytes().to_vec(),
            namespace_scope: BTreeMap::new(),
            source_part_name: Some(location.story.part_name.clone()),
            source_story_kind: Some(location.story.kind),
            source_owner_index: Some(location.story.owner_index),
            relationships: Vec::new(),
            markers: FragmentMarkers::default(),
        };
        self.swap_checked_paragraph_in_block(location, block.slot, &source, &selected, fragment)
    }

    /// [`Self::replace_run_xml`] on a run of a direct body paragraph, the
    /// block `block` names, edited in place as
    /// [`Self::replace_paragraph_xml_in_block`] edits the paragraph, with the
    /// same `None` cases.
    #[doc(hidden)]
    pub fn replace_run_xml_in_block(
        &mut self,
        position: &StoryRunPosition,
        block: crate::ControlBlock,
        xml: &str,
    ) -> Result<Option<bool>> {
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let result = self.replace_run_xml_in_block_unchecked(position, block, xml);
        #[cfg(debug_assertions)]
        self.assert_block_route_matches_staged(
            before,
            &result,
            true,
            |_| (),
            |staged| {
                let position = StoryRunPosition {
                    location: staged.fingerprinted(&position.location),
                    run_index: position.run_index,
                };
                staged.replace_run_xml(&position, xml)
            },
        );
        result
    }

    fn replace_run_xml_in_block_unchecked(
        &mut self,
        position: &StoryRunPosition,
        block: crate::ControlBlock,
        xml: &str,
    ) -> Result<Option<bool>> {
        let location = &position.location;
        let Some(source) = self.direct_paragraph_block_item(location, block)? else {
            return Ok(None);
        };
        let Some(snapshot) = self.direct_body_paragraph_xml(block.slot)? else {
            return Ok(None);
        };
        let mut paragraph = CT_P::from_xml_fragment(&snapshot)?;
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
            return Ok(Some(false));
        }
        let replacement = CT_R::from_xml_fragment_checked(xml.as_bytes())?;
        validate_element_owner_relationships_in_checked_story(
            self,
            &location.story,
            WordXmlOwner::Run(&replacement),
        )?;
        // The run's markers and ids are those of the paragraph around it:
        // the paragraph check below answers for them.
        if !same_paragraph_identities(&current_xml, xml.as_bytes()) {
            return Ok(None);
        }
        if !paragraph.replace_accepted_run(&path, replacement)? {
            return Err(Error::Other("element XML run owner disappeared".into()));
        }
        let mut fragment = ContentFragment::paragraph(paragraph)?;
        fragment.source_part_name = Some(location.story.part_name.clone());
        fragment.source_story_kind = Some(location.story.kind);
        fragment.source_owner_index = Some(location.story.owner_index);
        self.swap_checked_paragraph_in_block(location, block.slot, &source, &snapshot, fragment)
    }

    /// The scoped text replacement of [`Self::try_replace_text_at`] in a
    /// direct body paragraph, the block `block` names, edited in place under
    /// a `BodyBlocks { slot, count: 1 }` unit: the scoped operation runs on
    /// that paragraph written alone, which is read back and swapped in.
    /// `Some` holds what the staged route answers (the count, `Ok(0)` when
    /// nothing matched, or the count mismatch, which changes nothing). `None`,
    /// changing nothing, for a table, a content control or any location
    /// [`Self::replace_paragraph_xml_in_block`] does not take: the caller
    /// then takes [`Self::try_replace_text_at_owned`].
    #[doc(hidden)]
    pub fn replace_text_in_block(
        &mut self,
        location: &ContentLocation,
        block: crate::ControlBlock,
        placeholder: &str,
        replacement: &str,
        expected: Option<usize>,
    ) -> Result<Option<std::result::Result<usize, ReplacementCountMismatch>>> {
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let result = self.replace_text_in_block_unchecked(
            location,
            block,
            placeholder,
            replacement,
            expected,
        );
        #[cfg(debug_assertions)]
        self.assert_block_route_matches_staged(before, &result, true, Clone::clone, |staged| {
            let location = staged.fingerprinted(location);
            staged.try_replace_text_at(&location, placeholder, replacement, expected)
        });
        result
    }

    fn replace_text_in_block_unchecked(
        &mut self,
        location: &ContentLocation,
        block: crate::ControlBlock,
        placeholder: &str,
        replacement: &str,
        expected: Option<usize>,
    ) -> Result<Option<std::result::Result<usize, ReplacementCountMismatch>>> {
        oxoffice_core::xml::reject_non_xml_characters("replacement text", replacement)?;
        let Some(source) = self.direct_paragraph_block_item(location, block)? else {
            return Ok(None);
        };
        // The paragraph is a direct child of the body owner, which no
        // `mc:AlternateContent` text box holds: the scoped route pairs no
        // other copy with it (`paired_scoped_edits` selects a text box copy
        // around the owner).
        let span = source.item.clone();
        let scope = story_namespace_scope_at(&source.xml, span.start)?;
        let literal_input = Cell::new(false);
        let mut operation = oxoffice_docx_oxml::document::ScopedWordOperation::new(&source.xml);
        let (raw, count) = operation.replace_block_with_literal_input(
            span.clone(),
            &scope,
            None,
            placeholder,
            replacement,
            &literal_input,
        )?;
        if let Some(expected) = expected
            && count != expected
        {
            return Ok(Some(Err(ReplacementCountMismatch {
                index: 0,
                placeholder: placeholder.into(),
                expected,
                found: count,
            })));
        }
        if count == 0 {
            return Ok(Some(Ok(0)));
        }
        let mut updated = source.xml.clone();
        updated.splice(span, raw);
        Ok(self
            .swap_block_xml(block.slot, &source, &updated)?
            .map(|_| Ok(count)))
    }

    /// Replace the properties of section `index`, as
    /// [`Self::replace_section_properties_xml`] does: the paragraph that owns
    /// them is swapped in place under a `BodyBlocks { slot, count: 1 }`
    /// unit, the body's last section properties under `BodySection`.
    /// `Some(changed)`; a refusal changes nothing. Finding the owner walks
    /// the typed body blocks, which `section` does too; nothing is written
    /// or read as XML but the section.
    #[doc(hidden)]
    pub fn replace_section_properties_xml_in_place(
        &mut self,
        index: usize,
        xml: &str,
    ) -> Result<Option<bool>> {
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let result = self.replace_section_properties_xml_in_place_unchecked(index, xml);
        #[cfg(debug_assertions)]
        self.assert_block_route_matches_staged(
            before,
            &result,
            false,
            |_| (),
            |staged| staged.replace_section_properties_xml_staged(index, xml),
        );
        result
    }

    fn replace_section_properties_xml_in_place_unchecked(
        &mut self,
        index: usize,
        xml: &str,
    ) -> Result<Option<bool>> {
        // The paragraph slot that owns section `index`, or `None` for the
        // body's last section properties.
        let mut remaining = index;
        let mut owner = None;
        for (slot, content) in self.document.body.content.iter().enumerate() {
            if let BodyContent::Paragraph(paragraph) = content
                && paragraph
                    .properties
                    .as_ref()
                    .is_some_and(|properties| properties.sect_pr.is_some())
            {
                if remaining == 0 {
                    owner = Some(Some(slot));
                    break;
                }
                remaining -= 1;
            }
        }
        if owner.is_none() && remaining == 0 && self.document.body.sect_pr.is_some() {
            owner = Some(None);
        }
        let current = match owner {
            Some(Some(slot)) => match &self.document.body.content[slot] {
                BodyContent::Paragraph(paragraph) => paragraph
                    .properties
                    .as_ref()
                    .and_then(|properties| properties.sect_pr.as_ref()),
                _ => None,
            },
            Some(None) => self.document.body.sect_pr.as_ref(),
            None => None,
        }
        .ok_or_else(|| Error::Other("element XML section owner is absent".into()))?;
        let replacement = CT_SectPr::from_xml_fragment_checked(xml.as_bytes())?;
        self.validate_section_replacement_references(&replacement)?;
        let mut bytes = Vec::new();
        current.to_xml(&mut Writer::new(&mut bytes))?;
        let scope = BTreeMap::from([
            ("w".to_owned(), WORD_NAMESPACE.to_owned()),
            (
                "r".to_owned(),
                oxoffice_docx_oxml::namespace::R_NS.to_owned(),
            ),
        ]);
        if close_content_fragment_namespaces(&bytes, &scope)? == xml.as_bytes() {
            return Ok(Some(false));
        }
        // The staged route's one refusal among the section findings.
        if replacement.page_width.is_some_and(|size| size.0 <= 0)
            || replacement.page_height.is_some_and(|size| size.0 <= 0)
        {
            return Err(Error::Other(
                "element XML section geometry has nonpositive page size".into(),
            ));
        }
        match owner {
            Some(Some(slot)) => {
                let BodyContent::Paragraph(paragraph) = &self.document.body.content[slot] else {
                    unreachable!("the section owner is a paragraph");
                };
                let mut paragraph = paragraph.as_ref().clone();
                if let Some(properties) = paragraph.properties.as_mut() {
                    properties.sect_pr = Some(replacement);
                }
                self.replace_body_block_in_place(slot, BodyContent::Paragraph(Box::new(paragraph)))
                    .map(Some)
            }
            _ => {
                self.edit_in_place(&[EditUnit::BodySection], |document| {
                    document.document.body.sect_pr = Some(replacement);
                    document.invalidate_layout();
                    Ok(())
                })?;
                Ok(Some(self.take_last_edit_changed()))
            }
        }
    }

    /// Debug proof of a block route whose answer was `result`: `Ok(None)`
    /// left the document as it was; any other answer is the one `staged`,
    /// the staged route run on `before` (a copy taken before the edit),
    /// gives, read through `value`, with the same saved bytes, or the same
    /// parts once namespace declarations are set aside when
    /// `declarations_aside` (the staged splice declares the part's
    /// namespaces again on the block it writes).
    #[cfg(debug_assertions)]
    fn assert_block_route_matches_staged<U, T: PartialEq + std::fmt::Debug>(
        &self,
        before: Self,
        result: &Result<Option<U>>,
        declarations_aside: bool,
        value: impl FnOnce(&U) -> T,
        staged: impl FnOnce(&mut Self) -> Result<T>,
    ) {
        let answer = match result {
            Ok(None) => {
                assert!(
                    self.clone_for_staging().to_bytes().ok()
                        == before.clone_for_staging().to_bytes().ok(),
                    "an edit left to the staged route changed the document"
                );
                return;
            }
            Ok(Some(answer)) => Ok(value(answer)),
            Err(error) => Err(Error::Other(error.to_string())),
        };
        if declarations_aside {
            self.assert_in_place_matches_staged_declarations_aside(before, &answer, staged);
        } else {
            self.assert_in_place_matches_staged(before, &answer, staged);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three body paragraphs, "Alpha", "Hello NAME" and "Gamma".
    fn three_paragraphs() -> Document {
        let mut document = Document::new();
        for text in ["Alpha", "Hello NAME", "Gamma"] {
            document.add_paragraph(text);
        }
        document
    }

    /// The location of the direct body block at `slot` and the block itself.
    fn block_at(document: &Document, slot: usize) -> (ContentLocation, crate::ControlBlock) {
        let location = document
            .direct_body_item_location(slot)
            .unwrap()
            .expect("the block has a location");
        (
            location,
            crate::ControlBlock {
                slot,
                owner_index: 0,
                item_index: 0,
            },
        )
    }

    fn saved(document: &Document) -> Vec<u8> {
        document.clone_for_staging().to_bytes().unwrap()
    }

    /// The parts a save writes, without their namespace declarations: the
    /// staged splice declares the part's namespaces again on the paragraph
    /// it writes, the model on the root alone.
    fn saved_parts(document: &Document) -> Vec<(String, String)> {
        let declaration = regex::Regex::new(r#" xmlns(:[A-Za-z_][\w.-]*)?="[^"]*""#).unwrap();
        let package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(saved(document)))
            .expect("a saved package reopens");
        let mut parts = package
            .parts
            .iter()
            .map(|(name, bytes)| {
                let text = String::from_utf8_lossy(bytes);
                (
                    name.clone(),
                    declaration.replace_all(&text, "").into_owned(),
                )
            })
            .collect::<Vec<_>>();
        parts.sort();
        parts
    }

    /// The paragraph XML at `slot` with `from` replaced by `to`.
    fn paragraph_xml_with(document: &Document, slot: usize, from: &str, to: &str) -> String {
        let xml = document.direct_body_paragraph_xml(slot).unwrap().unwrap();
        let xml = String::from_utf8(xml).unwrap();
        assert!(xml.contains(from), "{xml} holds {from}");
        xml.replace(from, to)
    }

    /// Run `edit` holding its undo, and return its answer and the report of
    /// what it changed in place, if anything.
    fn held<T>(
        document: &mut Document,
        edit: impl FnOnce(&mut Document) -> Result<T>,
    ) -> (Result<T>, Option<EditReport>) {
        document.hold_edit_undo();
        let result = edit(document);
        let report = document.take_edit_undo().map(|undo| undo.report().clone());
        (result, report)
    }

    fn one_paragraph_changed(report: Option<EditReport>, slot: usize) {
        let report = report.expect("the edit changed the document in place");
        assert_eq!(
            report.units,
            [(EditUnit::BodyBlocks { slot, count: 1 }, true)]
        );
        assert_eq!(
            report.body,
            Some(BodyBlocksChange {
                slot,
                removed: 1,
                inserted: 1
            })
        );
    }

    #[test]
    fn a_paragraph_xml_replacement_edits_its_paragraph_in_place() {
        let mut document = three_paragraphs();
        let mut staged = document.clone_for_staging();
        let (location, block) = block_at(&document, 1);
        let xml = paragraph_xml_with(&document, 1, "Hello NAME", "Hello there");
        let (result, report) = held(&mut document, |document| {
            document.replace_paragraph_xml_in_block(&location, block, &xml)
        });
        assert_eq!(result.unwrap(), Some(true));
        one_paragraph_changed(report, 1);
        staged.replace_paragraph_xml(&location, &xml).unwrap();
        assert_eq!(saved_parts(&document), saved_parts(&staged));
        assert_eq!(document.paragraphs()[1].text(), "Hello there");
    }

    #[test]
    fn a_paragraph_that_keeps_its_footnote_reference_is_replaced_in_place() {
        let mut document = three_paragraphs();
        let id = document.add_footnote("A note");
        document.paragraph_mut(1).unwrap().add_footnote_ref(id);
        let mut staged = document.clone_for_staging();
        let (location, block) = block_at(&document, 1);
        let xml = paragraph_xml_with(&document, 1, "Hello NAME", "Hello there");
        let (result, report) = held(&mut document, |document| {
            document.replace_paragraph_xml_in_block(&location, block, &xml)
        });
        assert_eq!(result.unwrap(), Some(true));
        one_paragraph_changed(report, 1);
        staged.replace_paragraph_xml(&location, &xml).unwrap();
        assert_eq!(saved_parts(&document), saved_parts(&staged));
    }

    #[test]
    fn a_paragraph_replacement_that_drops_a_note_reference_keeps_the_staged_route() {
        let mut document = three_paragraphs();
        let id = document.add_footnote("A note");
        document.paragraph_mut(1).unwrap().add_footnote_ref(id);
        let before = saved(&document);
        let (location, block) = block_at(&document, 1);
        let xml = format!(r#"<w:p xmlns:w="{WORD_NAMESPACE}"><w:r><w:t>Plain</w:t></w:r></w:p>"#);
        let (result, report) = held(&mut document, |document| {
            document.replace_paragraph_xml_in_block(&location, block, &xml)
        });
        assert_eq!(result.unwrap(), None);
        assert!(report.is_none());
        assert_eq!(saved(&document), before);
    }

    #[test]
    fn a_refused_paragraph_replacement_leaves_the_document_unchanged() {
        let mut document = three_paragraphs();
        let before = saved(&document);
        let (location, block) = block_at(&document, 1);
        let xml = paragraph_xml_with(
            &document,
            1,
            "<w:r>",
            r#"<w:pPr><w:sectPr><w:pgSz w:w="12240" w:h="15840"/></w:sectPr></w:pPr><w:r>"#,
        );
        let (result, report) = held(&mut document, |document| {
            document.replace_paragraph_xml_in_block(&location, block, &xml)
        });
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("section properties"),
            "a paragraph may not gain a section"
        );
        assert!(report.is_none());
        assert_eq!(saved(&document), before);
    }

    #[test]
    fn the_same_paragraph_xml_reports_unchanged() {
        let mut document = three_paragraphs();
        let before = saved(&document);
        let (location, block) = block_at(&document, 1);
        let xml = paragraph_xml_with(&document, 1, "NAME", "NAME");
        let (result, report) = held(&mut document, |document| {
            document.replace_paragraph_xml_in_block(&location, block, &xml)
        });
        assert_eq!(result.unwrap(), Some(false));
        assert!(report.is_none());
        assert_eq!(saved(&document), before);
    }

    #[test]
    fn a_paragraph_of_a_table_cell_keeps_the_staged_route() {
        let mut document = three_paragraphs();
        document.add_table(1, 1);
        let location = document
            .stories()
            .unwrap()
            .into_iter()
            .find(|story| story.kind() == StoryKind::TableCell)
            .map(|story| ContentLocation::new(story, StoryItemKind::Paragraph, vec![0]))
            .expect("the cell has a story");
        // Even a block that names a body paragraph does not take a cell's
        // paragraph to the body route.
        let block = crate::ControlBlock {
            slot: 1,
            owner_index: 0,
            item_index: 0,
        };
        let before = saved(&document);
        let xml = format!(r#"<w:p xmlns:w="{WORD_NAMESPACE}"><w:r><w:t>Cell</w:t></w:r></w:p>"#);
        let result = document.replace_paragraph_xml_in_block(&location, block, &xml);
        assert_eq!(result.unwrap(), None);
        assert_eq!(saved(&document), before);
    }

    #[test]
    fn a_run_xml_replacement_edits_its_paragraph_in_place() {
        let mut document = three_paragraphs();
        let mut staged = document.clone_for_staging();
        let (location, block) = block_at(&document, 1);
        let position = StoryRunPosition {
            location,
            run_index: 0,
        };
        let xml = format!(
            r#"<w:r xmlns:w="{WORD_NAMESPACE}"><w:rPr><w:b/></w:rPr><w:t>Bold</w:t></w:r>"#
        );
        let (result, report) = held(&mut document, |document| {
            document.replace_run_xml_in_block(&position, block, &xml)
        });
        assert_eq!(result.unwrap(), Some(true));
        one_paragraph_changed(report, 1);
        staged.replace_run_xml(&position, &xml).unwrap();
        assert_eq!(saved_parts(&document), saved_parts(&staged));
        assert_eq!(document.paragraphs()[1].text(), "Bold");
    }

    #[test]
    fn a_refused_run_replacement_leaves_the_document_unchanged() {
        let mut document = three_paragraphs();
        let before = saved(&document);
        let (location, block) = block_at(&document, 1);
        let position = StoryRunPosition {
            location,
            run_index: 7,
        };
        let xml = format!(r#"<w:r xmlns:w="{WORD_NAMESPACE}"><w:t>x</w:t></w:r>"#);
        let (result, report) = held(&mut document, |document| {
            document.replace_run_xml_in_block(&position, block, &xml)
        });
        assert!(result.unwrap_err().to_string().contains("out of range"));
        assert!(report.is_none());
        assert_eq!(saved(&document), before);
    }

    #[test]
    fn a_scoped_text_replacement_edits_its_paragraph_in_place() {
        let mut document = three_paragraphs();
        let mut staged = document.clone_for_staging();
        let (location, block) = block_at(&document, 1);
        let (result, report) = held(&mut document, |document| {
            document.replace_text_in_block(&location, block, "NAME", "World", Some(1))
        });
        assert_eq!(result.unwrap(), Some(Ok(1)));
        one_paragraph_changed(report, 1);
        assert_eq!(
            staged
                .try_replace_text_at(&location, "NAME", "World", Some(1))
                .unwrap(),
            Ok(1)
        );
        assert_eq!(saved_parts(&document), saved_parts(&staged));
        assert_eq!(document.paragraphs()[1].text(), "Hello World");
    }

    #[test]
    fn a_scoped_text_count_mismatch_leaves_the_document_unchanged() {
        let mut document = three_paragraphs();
        let before = saved(&document);
        let (location, block) = block_at(&document, 1);
        let (result, report) = held(&mut document, |document| {
            document.replace_text_in_block(&location, block, "NAME", "World", Some(2))
        });
        let mismatch = result.unwrap().unwrap().unwrap_err();
        assert_eq!((mismatch.expected, mismatch.found), (2, 1));
        assert!(report.is_none());
        assert_eq!(saved(&document), before);
    }

    #[test]
    fn a_scoped_text_replacement_without_a_match_reports_unchanged() {
        let mut document = three_paragraphs();
        let before = saved(&document);
        let (location, block) = block_at(&document, 0);
        let (result, report) = held(&mut document, |document| {
            document.replace_text_in_block(&location, block, "NAME", "World", None)
        });
        assert_eq!(result.unwrap(), Some(Ok(0)));
        assert!(report.is_none());
        assert_eq!(saved(&document), before);
    }

    /// The section XML of section `index` with its page width set to `width`.
    fn section_xml_with_width(document: &Document, index: usize, width: &str) -> String {
        let xml = String::from_utf8(document.section_properties_xml(index).unwrap()).unwrap();
        let start = xml
            .find("<w:pgSz w:w=\"")
            .expect("the section has a page size")
            + 13;
        let end = start + xml[start..].find('"').unwrap();
        format!("{}{width}{}", &xml[..start], &xml[end..])
    }

    #[test]
    fn the_last_section_properties_are_replaced_in_place() {
        let mut document = three_paragraphs();
        let mut staged = document.clone_for_staging();
        let xml = section_xml_with_width(&document, 0, "11000");
        let (result, report) = held(&mut document, |document| {
            document.replace_section_properties_xml_in_place(0, &xml)
        });
        assert_eq!(result.unwrap(), Some(true));
        let report = report.expect("the section changed in place");
        assert_eq!(report.units, [(EditUnit::BodySection, true)]);
        staged
            .replace_section_properties_xml_staged(0, &xml)
            .unwrap();
        assert_eq!(saved_parts(&document), saved_parts(&staged));
    }

    #[test]
    fn a_paragraph_owned_section_is_replaced_in_its_paragraph_in_place() {
        let mut document = three_paragraphs();
        document.insert_section(0).unwrap();
        let slot = document
            .document
            .body
            .content
            .iter()
            .position(|block| {
                matches!(block, BodyContent::Paragraph(paragraph)
                    if paragraph.properties.as_ref().is_some_and(|p| p.sect_pr.is_some()))
            })
            .expect("a paragraph owns the first section");
        let mut staged = document.clone_for_staging();
        let xml = section_xml_with_width(&document, 0, "11000");
        let (result, report) = held(&mut document, |document| {
            document.replace_section_properties_xml_in_place(0, &xml)
        });
        assert_eq!(result.unwrap(), Some(true));
        one_paragraph_changed(report, slot);
        staged
            .replace_section_properties_xml_staged(0, &xml)
            .unwrap();
        assert_eq!(saved_parts(&document), saved_parts(&staged));
    }

    #[test]
    fn a_refused_section_replacement_leaves_the_document_unchanged() {
        let mut document = three_paragraphs();
        let before = saved(&document);
        let xml = section_xml_with_width(&document, 0, "0");
        let (result, report) = held(&mut document, |document| {
            document.replace_section_properties_xml_in_place(0, &xml)
        });
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("nonpositive page size")
        );
        assert!(report.is_none());
        assert_eq!(saved(&document), before);
    }

    #[test]
    fn the_same_section_xml_reports_unchanged() {
        let mut document = three_paragraphs();
        let before = saved(&document);
        let xml = String::from_utf8(document.section_properties_xml(0).unwrap()).unwrap();
        let (result, report) = held(&mut document, |document| {
            document.replace_section_properties_xml_in_place(0, &xml)
        });
        assert_eq!(result.unwrap(), Some(false));
        assert!(report.is_none());
        assert_eq!(saved(&document), before);
    }

    /// [`three_paragraphs`] saved and opened again with the second
    /// paragraph written as a producer writes it: an rsid, a preserved space
    /// and a bookmark around its text.
    fn opened_with_producer_paragraph() -> Document {
        let bytes = saved(&three_paragraphs());
        let mut package =
            oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).unwrap();
        let name = package.main_document_part().unwrap();
        let xml = String::from_utf8(package.get_part(&name).unwrap().to_vec()).unwrap();
        let producer = xml.replacen(
            "<w:p><w:r><w:t>Hello NAME</w:t></w:r></w:p>",
            r#"<w:p w:rsidR="00AB12CD"><w:bookmarkStart w:id="7" w:name="here"/><w:r><w:t xml:space="preserve">Hello NAME </w:t></w:r><w:bookmarkEnd w:id="7"/></w:p>"#,
            1,
        );
        assert_ne!(producer, xml, "the paragraph is written as expected");
        package.set_part(&name, producer.into_bytes());
        let mut out = std::io::Cursor::new(Vec::new());
        package.write_to(&mut out).unwrap();
        Document::from_bytes(&out.into_inner()).unwrap()
    }

    #[test]
    fn a_producer_paragraph_takes_a_scoped_text_replacement_in_place() {
        let mut document = opened_with_producer_paragraph();
        let mut staged = document.clone_for_staging();
        let (location, block) = block_at(&document, 1);
        let (result, report) = held(&mut document, |document| {
            document.replace_text_in_block(&location, block, "NAME", "World", None)
        });
        assert_eq!(result.unwrap(), Some(Ok(1)));
        one_paragraph_changed(report, 1);
        assert_eq!(
            staged
                .try_replace_text_at(&location, "NAME", "World", None)
                .unwrap(),
            Ok(1)
        );
        assert_eq!(saved_parts(&document), saved_parts(&staged));
    }

    #[test]
    fn a_producer_paragraph_takes_a_paragraph_xml_replacement_in_place() {
        let mut document = opened_with_producer_paragraph();
        let mut staged = document.clone_for_staging();
        let (location, block) = block_at(&document, 1);
        let xml = paragraph_xml_with(&document, 1, "Hello NAME", "Hello there");
        let (result, report) = held(&mut document, |document| {
            document.replace_paragraph_xml_in_block(&location, block, &xml)
        });
        assert_eq!(result.unwrap(), Some(true));
        one_paragraph_changed(report, 1);
        staged.replace_paragraph_xml(&location, &xml).unwrap();
        assert_eq!(saved_parts(&document), saved_parts(&staged));
    }

    #[test]
    fn a_paragraph_with_a_hyperlink_is_replaced_in_place() {
        let mut document = three_paragraphs();
        let id = document.add_hyperlink_relationship("https://example.com");
        document
            .paragraph_mut(1)
            .unwrap()
            .add_hyperlink(" link", &id);
        let mut staged = document.clone_for_staging();
        let (location, block) = block_at(&document, 1);
        let xml = paragraph_xml_with(&document, 1, "Hello NAME", "Hello there");
        let (result, report) = held(&mut document, |document| {
            document.replace_paragraph_xml_in_block(&location, block, &xml)
        });
        assert_eq!(result.unwrap(), Some(true));
        one_paragraph_changed(report, 1);
        staged.replace_paragraph_xml(&location, &xml).unwrap();
        assert_eq!(saved_parts(&document), saved_parts(&staged));
        let missing = paragraph_xml_with(&document, 1, &format!(r#""{id}""#), r#""rIdMissing""#);
        let before = saved(&document);
        assert!(
            document
                .replace_paragraph_xml_in_block(&location, block, &missing)
                .is_err()
        );
        assert_eq!(saved(&document), before);
    }
}
