//! Editing story paragraphs, hyperlinks and content: insert, import, remove, clone and move.

use super::*;

impl Document {
    /// Read owned facts from one source-qualified rich paragraph.
    #[doc(hidden)]
    pub fn read_story_paragraph<T>(
        &self,
        location: &ContentLocation,
        read: impl FnOnce(ParagraphRef<'_>) -> T,
    ) -> Result<T> {
        let snapshot = self.story_range_paragraph_snapshot(location)?;
        let paragraph = CT_P::from_xml_fragment(snapshot.xml())?;
        Ok(read(ParagraphRef { inner: &paragraph }))
    }

    /// Edit a direct cell paragraph without a whole-document transaction when local admission is sufficient.
    /// Controlled paragraphs and candidates needing package reconciliation keep the checked story route.
    #[doc(hidden)]
    pub fn edit_cell_paragraph<T>(
        &mut self,
        table: usize,
        row: usize,
        cell: usize,
        paragraph: usize,
        edit: impl FnOnce(&mut Paragraph<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut table_index = table;
        let selected = nth_table_in_body_ref(&self.document.body.content, &mut table_index)
            .and_then(|table| crate::table::table_cell(table, row, cell))
            .ok_or_else(|| Error::Other("cell index is out of range".to_owned()))?;
        let readers = selected.paragraphs();
        let direct_count = selected
            .content
            .iter()
            .filter(|child| matches!(child, CellContent::Paragraph(_)))
            .count();
        if readers.len() != direct_count || paragraph >= direct_count {
            let location = self.cell_paragraph_story_location(table, row, cell, paragraph)?;
            return self.edit_story_paragraph(&location, edit);
        }
        let original = readers[paragraph].clone();
        let mut candidate = original.clone();
        let result = edit(&mut Paragraph {
            inner: &mut candidate,
        })?;
        self.localize_run_styles(&mut candidate);
        let candidate = self.redline_edited_paragraph(&original, candidate)?;
        let mut fragment = ContentFragment::paragraph(candidate.clone())?;
        reject_section_owning_content_fragment(&fragment)?;
        let qualified = (|| -> Option<bool> {
            if self.plain_local_edit_blocked
                || oxoffice_core::xml::validate_strict_xml_1_0(&fragment.xml).is_err()
            {
                return Some(false);
            }
            let inherited = fragment
                .namespace_scope
                .iter()
                .map(|(prefix, namespace)| {
                    (
                        if prefix.is_empty() {
                            "xmlns".to_owned()
                        } else {
                            format!("xmlns:{prefix}")
                        },
                        namespace.clone(),
                    )
                })
                .collect::<Vec<_>>();
            if !xml_relationship_ids_in_order_with_bindings(&fragment.xml, &inherited)
                .ok()?
                .is_empty()
            {
                return Some(false);
            }
            let before = ContentFragment::paragraph(original.clone()).ok()?;
            let count = |xml: &[u8]| -> Option<BTreeMap<(i32, usize), usize>> {
                let mut counts = BTreeMap::new();
                for marker in crate::comments::comment_source_markers(xml).ok()? {
                    let entry = counts.entry((marker.id, marker.family)).or_insert(0usize);
                    *entry = entry.checked_add(1)?;
                }
                Some(counts)
            };
            let before = count(&before.xml)?;
            let after = count(&fragment.xml)?;
            Some(
                before
                    .iter()
                    .all(|(marker, before)| after.get(marker).copied().unwrap_or(0) >= *before),
            )
        })()
        .unwrap_or(false);
        if !qualified {
            let location = self.cell_paragraph_story_location(table, row, cell, paragraph)?;
            fragment.source_part_name = Some(location.story.part_name.clone());
            fragment.source_story_kind = Some(location.story.kind);
            fragment.source_owner_index = Some(location.story.owner_index);
            self.replace_story_content(&location, fragment)?;
            return Ok(result);
        }
        let mut table_index = table;
        let selected = nth_table_in_body(&mut self.document.body.content, &mut table_index)
            .and_then(|table| crate::table::table_cell_mut(table, row, cell))
            .ok_or_else(|| Error::Other("cell index is out of range".to_owned()))?;
        if selected
            .content
            .iter()
            .filter(|child| matches!(child, CellContent::Paragraph(_)))
            .count()
            != direct_count
        {
            return Err(Error::Other(
                "cell paragraph changed during local edit".to_owned(),
            ));
        }
        let selected = selected
            .paragraphs_mut()
            .into_iter()
            .nth(paragraph)
            .ok_or_else(|| Error::Other("cell paragraph index is out of range".to_owned()))?;
        if *selected != original {
            return Err(Error::Other(
                "cell paragraph changed during local edit".to_owned(),
            ));
        }
        *selected = candidate;
        self.invalidate_layout();
        Ok(result)
    }

    /// Edit one source-qualified rich paragraph and publish it atomically.
    #[doc(hidden)]
    pub fn edit_story_paragraph<T>(
        &mut self,
        location: &ContentLocation,
        edit: impl FnOnce(&mut Paragraph<'_>) -> Result<T>,
    ) -> Result<T> {
        let snapshot = self.story_range_paragraph_snapshot(location)?;
        let mut paragraph = CT_P::from_xml_fragment(snapshot.xml())?;
        let before = self.tracking.is_some().then(|| paragraph.clone());
        let result = edit(&mut Paragraph {
            inner: &mut paragraph,
        })?;
        // `Paragraph`'s adders name built-in styles by their English id; a
        // localized file names them otherwise.
        self.localize_run_styles(&mut paragraph);
        if let Some(before) = before {
            paragraph = self.redline_edited_paragraph(&before, paragraph)?;
        }
        let mut fragment = ContentFragment::paragraph(paragraph)?;
        // This fragment is rebuilt from the checked paragraph of this same owner.
        // Keep relationship admission scoped to that owner, including newly
        // allocated relationships already validated in its package scope.
        fragment.source_part_name = Some(location.story.part_name.clone());
        fragment.source_story_kind = Some(location.story.kind);
        fragment.source_owner_index = Some(location.story.owner_index);
        self.replace_story_content(location, fragment)?;
        Ok(result)
    }

    pub fn set_story_text(&mut self, location: &ContentLocation, value: &str) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, owner) = candidate.story_source_and_owner(&location.story)?;
        if location.index_path.len() != 1 {
            return Err(StoryError::InvalidPath {
                path: location.index_path.clone(),
            }
            .into());
        }
        let items = scan_story_items(&source.xml, &owner, None)?;
        let index = location.index_path[0];
        let item = items.get(index).ok_or(StoryError::OutOfBounds {
            index,
            len: items.len(),
        })?;
        if item.kind != location.item_kind {
            return Err(StoryError::KindMismatch {
                expected: location.item_kind,
                actual: item.kind,
            }
            .into());
        }
        if item.kind == StoryItemKind::PreservedNode {
            return Err(StoryError::NotTextBearing { kind: item.kind }.into());
        }
        let original_len = source.xml.len();
        let item_start = item.full.start;
        let item_end = item.full.end;
        let updated = replace_story_item_text(&source.xml, item, value)?;
        let new_item_end = if updated.len() >= original_len {
            item_end + (updated.len() - original_len)
        } else {
            item_end
                .checked_sub(original_len - updated.len())
                .ok_or_else(|| {
                    Error::Other("story text replacement underflowed its item range".to_owned())
                })?
        };
        let updated = remove_newly_empty_hyperlinks(
            &source.xml,
            item_start..item_end,
            &updated,
            item_start..new_item_end,
        )?;
        if source.part_name == candidate.doc_part_name {
            candidate.document =
                crate::document::DocumentModel::unproved(CT_Document::from_xml(&updated)?);
            candidate.flush_to_package()?;
        } else {
            candidate.package.set_part(&source.part_name, updated);
        }
        let reopened = candidate.reopen_prepared_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// [`Self::set_story_text`] on an item of the main part's body, its table
    /// cells or text boxes, written in the direct body block that holds it
    /// (`block`, found by the caller) and swapped in place under a
    /// `BodyBlocks` unit: `Some(changed)`. `None`, changing nothing, when the
    /// block does not hold that item, a save of the body is known to fail,
    /// or the edited block does not read back as it writes: the caller then
    /// takes [`Self::set_story_text`].
    #[doc(hidden)]
    pub fn set_story_text_in_block(
        &mut self,
        location: &ContentLocation,
        block: crate::ControlBlock,
        value: &str,
    ) -> Result<Option<bool>> {
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let result = self.set_story_text_in_block_unchecked(location, block, value);
        #[cfg(debug_assertions)]
        match &result {
            Ok(None) => assert!(
                self.clone_for_staging().to_bytes().ok()
                    == before.clone_for_staging().to_bytes().ok(),
                "a story text edit left to the staged route changed the document"
            ),
            _ => self.assert_in_place_matches_staged(
                before,
                &result
                    .as_ref()
                    .map(|_| ())
                    .map_err(|error| Error::Other(error.to_string())),
                |staged| {
                    let location = staged.fingerprinted(location);
                    staged.set_story_text(&location, value)
                },
            ),
        }
        result
    }

    fn set_story_text_in_block_unchecked(
        &mut self,
        location: &ContentLocation,
        block: crate::ControlBlock,
        value: &str,
    ) -> Result<Option<bool>> {
        let story = &location.story;
        let placed = match story.kind {
            StoryKind::Body => story.owner_index == 0 && block.owner_index == 0,
            StoryKind::TableCell | StoryKind::TextBox => true,
            _ => false,
        };
        if !placed
            || location.index_path.len() != 1
            || location.item_kind == StoryItemKind::PreservedNode
            || story.part_name != self.doc_part_name
        {
            return Ok(None);
        }
        // A character XML cannot carry fails the staged route's save, with
        // its own error.
        if oxoffice_core::xml::reject_non_xml_characters("story text", value).is_err() {
            return Ok(None);
        }
        let Some(source) = self.body_block_item(
            block.slot,
            (story.kind, block.owner_index),
            block.item_index,
            location.item_kind,
        )?
        else {
            return Ok(None);
        };
        let item = &source.span;
        let updated = replace_story_item_text(&source.xml, item, value)?;
        let original_len = source.xml.len();
        let new_item_end = (item.full.end + updated.len())
            .checked_sub(original_len)
            .ok_or_else(|| {
                Error::Other("story text replacement underflowed its item range".to_owned())
            })?;
        let updated = remove_newly_empty_hyperlinks(
            &source.xml,
            item.full.clone(),
            &updated,
            item.full.start..new_item_end,
        )?;
        // The head and the tail are written before the block and are not
        // the item's: an edit that changed them is not one of this block.
        if updated.len() < source.head.len() + source.tail
            || updated[..source.head.len()] != source.head[..]
            || updated[updated.len() - source.tail..] != *source.tail_bytes()
        {
            return Ok(None);
        }
        let Some(parsed) =
            Self::reparse_body_block(&source.head, source.tail_bytes(), source.block_of(&updated))
        else {
            return Ok(None);
        };
        // `body_block_item` answered that the body saves after a typed edit;
        // a text that declares no other namespace keeps that answer, which
        // would otherwise be asked again over the whole main part.
        let same_declarations = crate::content_control::declares_only_word_extensions(
            source.block_of(&source.xml),
            source.block_of(&updated),
        );
        let changed = self.replace_body_block_in_place(block.slot, parsed)?;
        if changed && same_declarations {
            self.keep_body_savable_after_swap();
        }
        Ok(Some(changed))
    }

    /// Point one story hyperlink at an external URL.
    ///
    /// `link_index` indexes [`Self::story_links`] for `story`. An anchor
    /// hyperlink becomes external and loses its anchor. The link's
    /// relationship is retargeted in place when nothing else references it.
    /// Otherwise the link gets a new relationship, so another link that shared
    /// the old one keeps its target. A relationship left unreferenced is removed.
    ///
    /// Only links that [`Self::story_links`] lists can be addressed. An empty
    /// `w:hyperlink`, a link inside `w:fldSimple`, a link in a text box inside
    /// `mc:AlternateContent`, and a HYPERLINK field are out of reach.
    pub fn set_hyperlink_url(
        &mut self,
        story: &StoryId,
        link_index: usize,
        url: &str,
    ) -> Result<()> {
        self.edit_story_hyperlink(story, link_index, Some(url))
    }

    /// [`Self::set_hyperlink_url`] on the link `link` of the story item at
    /// `location`, an item of the main part's body held by the direct body
    /// block `block` (found by the caller), when its relationship is an
    /// external hyperlink that no other element names and the link has no
    /// anchor: the relationship is retargeted in place under
    /// `EditUnit::Parts`, and no XML changes. The link is read from its
    /// block written alone, and whether another element names its
    /// relationship from a walk of what the main part writes, so nothing is
    /// serialized beyond the block. `story_link` is the link's index among
    /// its story's ([`Self::story_links`]), for the debug oracle. `None`,
    /// changing nothing, for every other case: the caller takes
    /// [`Self::set_hyperlink_url`].
    #[doc(hidden)]
    pub fn set_hyperlink_url_in_block(
        &mut self,
        location: &ContentLocation,
        block: crate::ControlBlock,
        link: usize,
        story_link: usize,
        url: &str,
    ) -> Result<Option<()>> {
        // Read by the debug oracle alone.
        #[cfg(not(debug_assertions))]
        let _ = story_link;
        let story = &location.story;
        if story.kind != StoryKind::Body
            || story.owner_index != 0
            || block.owner_index != 0
            || story.part_name != self.doc_part_name
            || location.index_path.len() != 1
            // A target the relationships part cannot carry fails the staged
            // route's save, with its own error.
            || oxoffice_core::xml::reject_non_xml_characters("hyperlink URL", url).is_err()
        {
            return Ok(None);
        }
        let Some(item) = self.framed_body_block_item(
            block.slot,
            (StoryKind::Body, 0),
            block.item_index,
            location.item_kind,
        )?
        else {
            return Ok(None);
        };
        // The links of the block's items, each paired with the smallest
        // item that holds it, as the story's links are.
        let owners = scan_story_owners(&item.xml, StoryKind::Body)?;
        let Some(owner) = owners
            .iter()
            .find(|owner| owner.kind == StoryKind::Body && owner.owner_index == 0)
        else {
            return Ok(None);
        };
        let items = scan_story_items(&item.xml, owner, None)?;
        let scopes =
            story_namespace_scopes_at(&item.xml, items.iter().map(|item| item.scan.start))?;
        let Some((_, found)) = ordered_story_link_spans(&item.xml, story, items, &scopes)?
            .into_iter()
            .filter(|(holder, _)| holder.index_path == [block.item_index])
            .nth(link)
        else {
            return Ok(None);
        };
        let Some(relationship_id) = found.rel_id.filter(|_| found.anchor.is_none()) else {
            return Ok(None);
        };
        let external_hyperlink = self
            .package
            .get_part_rels(&self.doc_part_name)
            .and_then(|relationships| relationships.get_by_id(&relationship_id))
            .is_some_and(|relationship| {
                relationship.rel_type == rel_types::HYPERLINK
                    && relationship.target_mode.as_deref() == Some("External")
            });
        if !external_hyperlink || self.main_part_relationship_references(&relationship_id) != 1 {
            return Ok(None);
        }
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let part = self.doc_part_name.clone();
        let result = self.edit_in_place(&[EditUnit::Parts], |document| {
            document
                .package
                .set_relationship_target(&part, &relationship_id, url);
            Ok(())
        });
        // The staged route runs on the document as a save reopens it: a
        // copy of the live document keeps relationships its save prunes (a
        // link an earlier in-place edit removed), which the staged route
        // writes back, while this edit leaves them to the save. It writes
        // the main part again from the story's XML, whose elements declare
        // the namespaces they use again: those declarations are set aside.
        #[cfg(debug_assertions)]
        self.assert_in_place_matches_staged_declarations_aside(before, &result, |staged| {
            *staged = Document::from_bytes(&staged.to_bytes()?)?;
            let story = staged.fingerprinted(location).story;
            staged.set_hyperlink_url(&story, story_link, url)
        });
        result.map(Some)
    }

    /// Remove one story hyperlink and keep its content in place.
    ///
    /// `link_index` indexes [`Self::story_links`] for `story`. The runs keep
    /// their formatting, except that the built-in Hyperlink and
    /// FollowedHyperlink character styles are cleared, as Word's Remove
    /// Hyperlink does. A relationship left unreferenced is removed. The links
    /// out of reach are those [`Self::set_hyperlink_url`] names.
    pub fn remove_hyperlink(&mut self, story: &StoryId, link_index: usize) -> Result<()> {
        self.edit_story_hyperlink(story, link_index, None)
    }

    fn edit_story_hyperlink(
        &mut self,
        story: &StoryId,
        link_index: usize,
        url: Option<&str>,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, owner) = candidate.story_source_and_owner(story)?;
        let part_name = source.part_name.clone();
        let xml = source.xml.into_owned();
        let mut links = story_link_spans(&xml, &owner, story)?;
        let len = links.len();
        if link_index >= len {
            return Err(StoryError::OutOfBounds {
                index: link_index,
                len,
            }
            .into());
        }
        let (_, link) = links.swap_remove(link_index);
        let scope = story_namespace_scope_at(&xml, link.full.start)?;
        let updated = match url {
            Some(url) => {
                let (start, _) = story_hyperlink_tags(&xml, &link.full)?;
                let with_relationship = |relationship_id: &str| -> Result<Vec<u8>> {
                    let tag = external_story_hyperlink_start(
                        &xml[start.clone()],
                        &scope,
                        relationship_id,
                    )?;
                    let mut updated = xml.clone();
                    updated.splice(start.clone(), tag);
                    Ok(updated)
                };
                let shared = match link.rel_id.as_deref() {
                    Some(old) => xml_relationship_ids_in_order(&with_relationship("")?)?
                        .iter()
                        .any(|id| id == old),
                    None => false,
                };
                let in_place = link.rel_id.as_deref().filter(|_| !shared).and_then(|old| {
                    candidate
                        .package
                        .get_part_rels_mut(&part_name)?
                        .items
                        .iter_mut()
                        .find(|relationship| {
                            relationship.id == old
                                && relationship.rel_type == rel_types::HYPERLINK
                                && relationship.target_mode.as_deref() == Some("External")
                        })
                });
                match in_place {
                    Some(relationship) => {
                        relationship.target = url.to_owned();
                        let relationship_id = relationship.id.clone();
                        if link.anchor.is_some() {
                            with_relationship(&relationship_id)?
                        } else {
                            xml.clone()
                        }
                    }
                    None => {
                        let relationship_id = candidate.add_external_relationship_checked(
                            &part_name,
                            rel_types::HYPERLINK,
                            url,
                        )?;
                        if part_name == candidate.doc_part_name {
                            candidate
                                .identifiers
                                .register_nested_story_relationship(story, relationship_id.clone());
                        }
                        with_relationship(&relationship_id)?
                    }
                }
            }
            None => {
                let styles = candidate.hyperlink_character_style_ids();
                unwrap_story_hyperlink(&xml, &link.full, &scope, &styles)?
            }
        };
        if let Some(old) = link.rel_id.as_deref()
            && !xml_relationship_ids_in_order(&updated)?
                .iter()
                .any(|id| id == old)
            && let Some(relationships) = candidate.package.get_part_rels_mut(&part_name)
        {
            let before = relationships.items.len();
            relationships.items.retain(|relationship| {
                relationship.id != old || relationship.rel_type != rel_types::HYPERLINK
            });
            if relationships.items.len() != before {
                if relationships.items.is_empty() {
                    candidate.package.remove_part_rels(&part_name);
                }
                candidate
                    .identifiers
                    .retire_authored_story_relationships(&part_name, [old.to_owned()]);
            }
        }
        set_story_source_xml(&mut candidate, &part_name, updated)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// The character style IDs that mark hyperlink text: the built-in
    /// Hyperlink and FollowedHyperlink styles, by ID and by name.
    pub(crate) fn hyperlink_character_style_ids(&self) -> HashSet<String> {
        const NAMES: [&str; 2] = ["Hyperlink", "FollowedHyperlink"];
        let mut ids = NAMES.map(str::to_owned).into_iter().collect::<HashSet<_>>();
        ids.extend(
            self.styles
                .styles
                .iter()
                .filter(|style| {
                    style.style_type == StyleType::Character
                        && style.name.as_deref().is_some_and(|name| {
                            NAMES.iter().any(|known| known.eq_ignore_ascii_case(name))
                        })
                })
                .map(|style| style.style_id.clone()),
        );
        ids
    }

    /// Insert one owned fragment at a checked block location: before the
    /// block a location names, or at an end, in a story or inside a block
    /// content control ([`ContentLocation::control_end`]).
    ///
    /// A location at a block inserts before the bookmark starts written just
    /// before it, which belong to that block (Google Docs writes a heading's
    /// bookmark so); a location at such a bookmark start inserts right there.
    /// The other insertions at a location ([`Self::insert_html_fragment`],
    /// [`Self::clone_content`], [`Self::move_content`],
    /// [`Self::import_fragment`]) place their content the same way.
    ///
    /// The bookmark markers the fragment carries go around it. An identity
    /// the document already holds takes a fresh value (a bookmark id or
    /// name, which the fragment's REF fields follow; a control or drawing
    /// id), and a note reference whose note another reference shows gets a
    /// copy of the note; the rest is kept, so a cut and paste changes nothing
    /// and the same fragment can be inserted twice. A lone bookmark start,
    /// whose end the fragment does not hold, keeps its id: inserted where
    /// that bookmark is whole, it is refused and changes nothing.
    pub fn insert_content(
        &mut self,
        destination: &ContentLocation,
        fragment: ContentFragment,
    ) -> Result<()> {
        if self.inserts_in_note_part(destination, &fragment)? {
            #[cfg(debug_assertions)]
            let before = self.clone_for_staging();
            #[cfg(debug_assertions)]
            let staged_fragment = fragment.clone();
            let result = self.edit_in_place(
                // The flush of pending typed notes may write pending comments.
                &[
                    crate::EditUnit::Footnotes,
                    crate::EditUnit::Endnotes,
                    crate::EditUnit::Comments,
                ],
                |document| document.insert_related_story_content(destination, fragment, true),
            );
            #[cfg(debug_assertions)]
            self.assert_in_place_matches_staged(before, &result, |staged| {
                staged.insert_content_staged(destination, staged_fragment)
            });
            return result;
        }
        if self.inserts_in_header_footer_part(destination, &fragment)? {
            #[cfg(debug_assertions)]
            let before = self.clone_for_staging();
            #[cfg(debug_assertions)]
            let staged_fragment = fragment.clone();
            // The part journal undoes the header or footer part's bytes and
            // answers the no-op.
            let result = self.edit_in_place(&[crate::EditUnit::Parts], |document| {
                document.insert_related_story_content(destination, fragment, false)?;
                // The staged route's reopen refuses a character XML 1.0
                // cannot carry: the part written is checked as it reads it.
                let part = destination.story.part_name();
                if let Some(xml) = document.package.get_part(part) {
                    OpcPackage::reject_part_xml_characters(part, xml)?;
                }
                Ok(())
            });
            #[cfg(debug_assertions)]
            self.assert_in_place_matches_staged(before, &result, |staged| {
                staged.insert_content_staged(destination, staged_fragment)
            });
            return result;
        }
        self.insert_content_staged(destination, fragment)
    }

    /// Whether [`Self::insert_content`] edits a header or footer part in
    /// place: the destination is in a part the main part relates as a header
    /// or footer, and the fragment holds no note reference (a shown note is
    /// copied through the main part).
    fn inserts_in_header_footer_part(
        &self,
        destination: &ContentLocation,
        fragment: &ContentFragment,
    ) -> Result<bool> {
        let part = destination.story.part_name();
        if part == self.doc_part_name || !self.is_header_footer_part(part) {
            return Ok(false);
        }
        Ok(!fragment_holds_note_references(fragment)?)
    }

    /// Whether [`Self::insert_content`] edits in place: the destination is
    /// in the footnotes or endnotes part, and the fragment holds no note
    /// reference (a shown note is copied through the main part).
    fn inserts_in_note_part(
        &self,
        destination: &ContentLocation,
        fragment: &ContentFragment,
    ) -> Result<bool> {
        let part = destination.story.part_name();
        if part == self.doc_part_name {
            return Ok(false);
        }
        // An endnotes relationship the staged route would refuse keeps it.
        let in_notes = self.footnotes_part_name.as_deref() == Some(part)
            || self
                .note_part_name(StoryKind::Endnote)
                .ok()
                .flatten()
                .is_some_and(|name| name == part);
        Ok(in_notes && !fragment_holds_note_references(fragment)?)
    }

    /// [`Self::insert_content`] in a note, header or footer part, on the
    /// live document: that part is rewritten, the body is neither serialized
    /// nor reopened. `flush_notes` puts the typed notes not yet in their
    /// part there first, which a note part needs.
    pub(super) fn insert_related_story_content(
        &mut self,
        destination: &ContentLocation,
        fragment: ContentFragment,
        flush_notes: bool,
    ) -> Result<()> {
        // Typed footnotes not yet in the part go there first, as the staged
        // route's flush puts them.
        if flush_notes {
            self.flush_dirty_related_story_models()?;
        }
        let (source, owner) = self.story_source_and_owner(&destination.story)?;
        let part_name = source.part_name.clone();
        let source_xml = source.xml.into_owned();
        let slot = resolve_block_location(&source_xml, &owner, destination)?;
        let content = close_content_fragment_namespaces(&fragment.xml, &fragment.namespace_scope)?;
        let (boundary, _) = slot.insertion(&source_xml, &content)?;
        reject_section_owning_content_fragment(&fragment)?;
        let mut fragment = fragment;
        freshen_taken_content_fragment_identities(self, &mut fragment)?;
        let fragment_xml = content_fragment_for_insertion(self, &destination.story, &fragment)?;
        let mut fragment_xml = without_w14_identities(&fragment_xml, &fragment.namespace_scope);
        let edits = self
            .identifiers
            .fresh_drawing_id_edits(&fragment_xml, &fragment.namespace_scope)?;
        for (start, end, replacement) in edits.into_iter().rev() {
            fragment_xml.splice(start..end, replacement);
        }
        let mut updated = source_xml;
        insert_story_fragment(&mut updated, &slot.owner, boundary, fragment_xml)?;
        set_story_source_xml(self, &part_name, updated)?;
        self.invalidate_layout();
        Ok(())
    }

    /// The staged route of [`Self::insert_content`]: a copy is edited, saved
    /// and reopened. It is the debug oracle of the in-place route.
    pub(super) fn insert_content_staged(
        &mut self,
        destination: &ContentLocation,
        fragment: ContentFragment,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, owner) = candidate.story_source_and_owner(&destination.story)?;
        let part_name = source.part_name.clone();
        let source_xml = source.xml.into_owned();
        let slot = resolve_block_location(&source_xml, &owner, destination)?;
        let content = close_content_fragment_namespaces(&fragment.xml, &fragment.namespace_scope)?;
        let (boundary, _) = slot.insertion(&source_xml, &content)?;
        reject_section_owning_content_fragment(&fragment)?;
        // The same fragment can be inserted twice: what the document already
        // holds takes a fresh identity (bookmark ids and names, control and
        // drawing ids), a note another reference shows is copied, and the
        // paragraph and row ids (w14:paraId, w14:textId) go, as
        // clone_content drops them, and Word gives new ones.
        // The notes go first: the fragment's fresh names then avoid theirs.
        let mut fragment = fragment;
        copy_shown_notes(&mut candidate, &mut fragment)?;
        freshen_taken_content_fragment_identities(&mut candidate, &mut fragment)?;
        let fragment_xml =
            content_fragment_for_insertion(&candidate, &destination.story, &fragment)?;
        let mut fragment_xml = without_w14_identities(&fragment_xml, &fragment.namespace_scope);
        // A header, footer, note or comment part keeps the drawing ids it
        // holds at a save, so the drawings it receives take theirs now.
        if part_name != candidate.doc_part_name {
            let edits = candidate
                .identifiers
                .fresh_drawing_id_edits(&fragment_xml, &fragment.namespace_scope)?;
            for (start, end, replacement) in edits.into_iter().rev() {
                fragment_xml.splice(start..end, replacement);
            }
        }
        let mut updated = source_xml;
        insert_story_fragment(&mut updated, &slot.owner, boundary, fragment_xml)?;
        set_story_source_xml(&mut candidate, &part_name, updated)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Insert one bounded HTML fragment at a checked block location.
    ///
    /// Images are resolved only from data URIs or the explicit resource slice.
    /// The document remains unchanged if parsing, projection, package
    /// reconciliation, insertion, or reopen validation fails.
    pub fn insert_html_fragment(
        &mut self,
        destination: &ContentLocation,
        html: &str,
        images: &[crate::HtmlImageResource<'_>],
    ) -> Result<crate::HtmlFragmentInsertResult> {
        if !matches!(
            destination.story.kind,
            StoryKind::Body | StoryKind::TableCell | StoryKind::Header | StoryKind::Footer
        ) {
            return Err(Error::Other(
                "HTML fragments support body, table-cell, header, and footer stories: for another \
                 story, read the HTML into a document (from_html), take its body with \
                 from_range and import_fragment it"
                    .to_owned(),
            ));
        }
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, owner) = candidate.story_source_and_owner(&destination.story)?;
        let part_name = source.part_name.clone();
        let mut source_xml = source.xml.into_owned();
        let slot = resolve_block_location(&source_xml, &owner, destination)?;
        // HTML projects paragraphs and tables, never a repeating section item.
        let (boundary, direct_index) = slot.insertion(&source_xml, b"<w:p/>")?;
        // Inside a block control the fragment is written as XML: the
        // modeled routes splice the story owner's own children.
        let in_control = slot.control.is_some();
        let modeled_cell_route = (destination.story.kind == StoryKind::TableCell
            && part_name == candidate.doc_part_name
            && !in_control)
            .then(|| modeled_main_cell_route(&source_xml, &owner))
            .transpose()?;
        let projected =
            crate::html::project_html_fragment(&mut candidate, &destination.story, html, images)?;
        let inserted_count = projected.content.len();
        if destination.story.kind == StoryKind::Body
            && part_name == candidate.doc_part_name
            && !in_control
        {
            candidate
                .document
                .body
                .content
                .splice(direct_index..direct_index, projected.content);
        } else if destination.story.kind == StoryKind::TableCell
            && part_name == candidate.doc_part_name
            && !in_control
        {
            let (content_index, mut cell_index) = modeled_cell_route.ok_or_else(|| {
                Error::Other("modeled table-cell route disappeared before insertion".to_owned())
            })?;
            let content = candidate
                .document
                .body
                .content
                .get_mut(content_index)
                .ok_or_else(|| StoryError::OwnerNotFound {
                    story: destination.story.clone(),
                })?;
            let cell = match content {
                BodyContent::Table(table) => nth_cell_in_table(table, &mut cell_index),
                BodyContent::ContentControl(control) => {
                    nth_cell_in_control(control, &mut cell_index)
                }
                BodyContent::Paragraph(_) | BodyContent::RawXml(_) => None,
            }
            .ok_or_else(|| StoryError::OwnerNotFound {
                story: destination.story.clone(),
            })?;
            // The modeled cell counts its children without its properties.
            let properties = slot.items[..direct_index]
                .iter()
                .filter(|item| {
                    item.kind == StoryItemKind::PreservedNode
                        && root_local_name(&source_xml[item.full.clone()]).as_deref()
                            == Some(b"tcPr".as_slice())
                })
                .count();
            insert_html_content_into_cell(cell, direct_index - properties, projected.content)?;
        } else {
            let mut fragment_xml = Vec::new();
            for content in projected.content {
                fragment_xml.extend_from_slice(&serialize_content_fragment(content)?);
            }
            insert_story_fragment(&mut source_xml, &slot.owner, boundary, fragment_xml)?;
            set_story_source_xml(&mut candidate, &part_name, source_xml)?;
        }
        let reopened = candidate.prepare_and_reopen_staged()?;
        let result_story = reopened
            .stories()?
            .into_iter()
            .find(|story| {
                story.kind == destination.story.kind
                    && story.part_name == destination.story.part_name
                    && story.owner_index == destination.story.owner_index
            })
            .ok_or_else(|| StoryError::OwnerNotFound {
                story: destination.story.clone(),
            })?;
        let range_end = direct_index
            .checked_add(inserted_count)
            .ok_or_else(|| Error::Other("HTML fragment range overflowed".to_owned()))?;
        self.commit_staged_mutation(reopened);
        Ok(crate::HtmlFragmentInsertResult {
            story: result_story,
            direct_range: direct_index..range_end,
            diagnostics: projected.diagnostics,
        })
    }

    /// Import an owned cross-document fragment at a checked block location.
    ///
    /// The complete supported dependency closure is allocated on a staged
    /// candidate. Unreconcilable dependencies return an error before the
    /// destination changes.
    pub fn import_fragment(
        &mut self,
        destination: &ContentLocation,
        fragment: &DocumentFragment,
        policy: FragmentConflictPolicy,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, owner, _) =
            candidate.fragment_story_source_owner_and_owners(&destination.story)?;
        let part_name = source.part_name.clone();
        let mut source_xml = source.xml.into_owned();
        let slot = resolve_block_location(&source_xml, &owner, destination)?;
        let (mut boundary, _) = slot.leading_boundary(&source_xml)?;
        let mut owner = slot.owner.clone();
        if fragment.include_final_section_properties
            && (destination.story.kind != StoryKind::Body || slot.control.is_some())
        {
            return Err(Error::Other(
                "section-inclusive fragments require the main body destination: import it at a \
                 direct body location or at end(body)"
                    .to_owned(),
            ));
        }
        let imported_xml = fragment.import_content_staged(
            &mut candidate,
            &part_name,
            policy,
            false,
            &mut Vec::new(),
        )?;
        slot.check_takes(&imported_xml)?;
        // Companion closure may append notes or comment threads to the same
        // physical part that owns the destination. Refresh that part before
        // inserting, so publishing the selected owner cannot discard them.
        candidate.prepare_staged_package()?;
        let companion_owner = candidate.comments_part_name.as_deref() == Some(&part_name)
            || candidate.note_part_name(StoryKind::Footnote)?.as_deref() == Some(&part_name)
            || candidate.note_part_name(StoryKind::Endnote)?.as_deref() == Some(&part_name);
        if companion_owner {
            let (updated_source, updated_owner, _) =
                candidate.fragment_story_source_owner_and_owners(&destination.story)?;
            source_xml = updated_source.xml.into_owned();
            let slot = resolve_block_location(&source_xml, &updated_owner, destination)?;
            boundary = slot.leading_boundary(&source_xml)?.0;
            owner = slot.owner;
        }
        insert_story_fragment(&mut source_xml, &owner, boundary, imported_xml)?;
        set_story_source_xml(&mut candidate, &part_name, source_xml)?;
        let reopened = candidate.reopen_prepared_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Remove the block a checked location names and return it as an owned
    /// fragment. The only block of a block content control, header, footer,
    /// note, comment or text box stays: remove that container instead.
    ///
    /// The bookmark markers the child owns go with it, in the fragment: the
    /// bookmark starts written just before it whose bookmark ends inside it
    /// or in the bookmark ends written just after it, those ends, and the
    /// ends written just after it that close a bookmark started inside it.
    /// [`Self::insert_content`] writes them back around it.
    ///
    /// A picture, chart, embedded object or link the removal leaves
    /// unreferenced stays in the package until the next write or signature,
    /// which drops it if nothing names it then:
    /// until a signature the fragment can be inserted again (cut and paste);
    /// a write drops it from the bytes written only.
    pub fn remove_content_at(&mut self, location: &ContentLocation) -> Result<ContentFragment> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, owner) = candidate.story_source_and_owner(&location.story)?;
        let part_name = source.part_name.clone();
        let source_xml = source.xml.into_owned();
        let slot = resolve_block_location(&source_xml, &owner, location)?;
        let item = slot.block()?.clone();
        slot.check_leaves_a_block(&source_xml)?;
        let namespace_scope = story_namespace_scope_at(&source_xml, item.full.start)?;
        let closed =
            close_content_fragment_namespaces(&source_xml[item.full.clone()], &namespace_scope)?;
        Self::refuse_commented_fragment(&closed)?;
        let mut fragment = ContentFragment {
            kind: item.kind,
            xml: source_xml[item.full.clone()].to_vec(),
            source_part_name: Some(part_name.clone()),
            source_story_kind: Some(location.story.kind),
            source_owner_index: Some(location.story.owner_index),
            namespace_scope,
            relationships: Vec::new(),
            markers: FragmentMarkers::default(),
        };
        reject_section_owning_content_fragment(&fragment)?;
        validate_fragment_relationships(&candidate, &location.story, &fragment)?;
        fragment.relationships = referenced_relationships(&candidate, &location.story, &fragment)?;
        // The bookmark markers the item owns go too, in the fragment.
        let direct_items = &slot.items;
        let index = slot.index;
        let owned =
            owned_bookmark_marker_items(&source_xml, direct_items, index, index..index + 1)?;
        let marker = |index: &usize| source_xml[direct_items[*index].full.clone()].to_vec();
        fragment.markers = FragmentMarkers {
            before: owned.before.iter().map(marker).collect(),
            after: owned.after.iter().map(marker).collect(),
        };
        let mut removed = owned
            .before
            .iter()
            .chain(&owned.after)
            .map(|index| direct_items[*index].full.clone())
            .collect::<Vec<_>>();
        removed.push(item.full);
        removed.sort_by_key(|range| range.start);
        let mut updated = source_xml;
        for range in removed.into_iter().rev() {
            updated.drain(range);
        }
        // The fragment's pictures, charts, embedded objects and links go at
        // the next write if nothing names them then (it may be inserted
        // again before).
        let freed: Vec<&str> = fragment
            .relationships
            .iter()
            .filter(|(relationship, _)| is_content_relationship(&relationship.rel_type))
            .map(|(relationship, _)| relationship.id.as_str())
            .collect();
        // Every one is recorded, named elsewhere or not: another edit may
        // remove the other occurrence before the fragment comes back.
        for id in freed {
            let entry = (part_name.clone(), id.to_owned());
            if !candidate.freed_relationships.contains(&entry) {
                candidate.freed_relationships.push(entry);
            }
        }
        set_story_source_xml(&mut candidate, &part_name, updated)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(fragment)
    }

    /// Clone the block a checked location names into a checked insertion
    /// boundary. A block holding only part of a complex field is refused.
    ///
    /// The bookmark markers the child owns (see [`Self::remove_content_at`])
    /// are copied with it. Document-wide identities of the copy are made
    /// unique, a note it references is copied for it, and its paragraphs and
    /// table rows drop `w14:paraId` and `w14:textId`.
    pub fn clone_content(
        &mut self,
        source_location: &ContentLocation,
        destination: &ContentLocation,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, source_owner) = candidate.story_source_and_owner(&source_location.story)?;
        let source_xml = source.xml.into_owned();
        let source_slot = resolve_block_location(&source_xml, &source_owner, source_location)?;
        let source_item = source_slot.block()?.clone();
        refuse_partial_complex_field(
            &source_xml,
            std::slice::from_ref(&source_item.full),
            &format!("the block at {:?}", source_location.index_path),
        )?;
        let mut fragment = ContentFragment {
            kind: source_item.kind,
            xml: source_xml[source_item.full.clone()].to_vec(),
            source_part_name: Some(source_location.story.part_name.clone()),
            source_story_kind: Some(source_location.story.kind),
            source_owner_index: Some(source_location.story.owner_index),
            namespace_scope: story_namespace_scope_at(&source_xml, source_item.full.start)?,
            relationships: Vec::new(),
            markers: FragmentMarkers::default(),
        };
        reject_section_owning_content_fragment(&fragment)?;
        fragment.relationships =
            referenced_relationships(&candidate, &source_location.story, &fragment)?;
        let source_items = &source_slot.items;
        let index = source_slot.index;
        let owned =
            owned_bookmark_marker_items(&source_xml, source_items, index, index..index + 1)?;
        let marker = |index: &usize| source_xml[source_items[*index].full.clone()].to_vec();
        fragment.markers = FragmentMarkers {
            before: owned.before.iter().map(marker).collect(),
            after: owned.after.iter().map(marker).collect(),
        };

        let (destination_source, destination_owner) =
            candidate.story_source_and_owner(&destination.story)?;
        let destination_part_name = destination_source.part_name.clone();
        let destination_xml = destination_source.xml.into_owned();
        let destination_slot =
            resolve_block_location(&destination_xml, &destination_owner, destination)?;
        let content = close_content_fragment_namespaces(&fragment.xml, &fragment.namespace_scope)?;
        let (boundary, _) = destination_slot.insertion(&destination_xml, &content)?;
        copy_shown_notes(&mut candidate, &mut fragment)?;
        freshen_content_fragment_identities(&mut candidate, &mut fragment)?;
        let fragment_xml =
            content_fragment_for_insertion(&candidate, &destination.story, &fragment)?;
        let mut updated = destination_xml;
        insert_story_fragment(
            &mut updated,
            &destination_slot.owner,
            boundary,
            fragment_xml,
        )?;
        set_story_source_xml(&mut candidate, &destination_part_name, updated)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Move the block a checked location names within its story, into or
    /// out of a block content control too. A block holding only part of a
    /// complex field is refused.
    ///
    /// The bookmark markers the child owns (see [`Self::remove_content_at`])
    /// move with it; a start whose bookmark ends further on stays. A
    /// destination at the child, at its markers or just after them moves
    /// nothing.
    pub fn move_content(
        &mut self,
        source_location: &ContentLocation,
        destination: &ContentLocation,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, owner) = candidate.story_source_and_owner(&source_location.story)?;
        let part_name = source.part_name.clone();
        let source_xml = source.xml.into_owned();
        let source_slot = resolve_block_location(&source_xml, &owner, source_location)?;
        let source_item = source_slot.block()?.clone();

        let (destination_source, destination_owner) =
            candidate.story_source_and_owner(&destination.story)?;
        let destination_xml = destination_source.xml.into_owned();
        let destination_slot =
            resolve_block_location(&destination_xml, &destination_owner, destination)?;
        let (boundary, destination_index) = destination_slot.leading_boundary(&destination_xml)?;
        if source_location.story != destination.story {
            return Err(Error::Other(
                "content can move only within one unchanged story owner: to another story, \
                 clone_content then remove_content_at"
                    .to_owned(),
            ));
        }
        let direct_items = &source_slot.items;
        let source_index = source_slot.index;
        let same_container = source_slot.owner.full == destination_slot.owner.full;
        let fragment = ContentFragment {
            kind: source_item.kind,
            xml: source_xml[source_item.full.clone()].to_vec(),
            source_part_name: Some(part_name.clone()),
            source_story_kind: Some(source_location.story.kind),
            source_owner_index: Some(source_location.story.owner_index),
            namespace_scope: story_namespace_scope_at(&source_xml, source_item.full.start)?,
            relationships: Vec::new(),
            markers: FragmentMarkers::default(),
        };
        reject_section_owning_content_fragment(&fragment)?;
        validate_fragment_relationships(&candidate, &destination.story, &fragment)?;
        // The item moves with the bookmark markers it owns; a destination
        // at the item, at its markers or just after them changes nothing.
        let owned = owned_bookmark_marker_items(
            &source_xml,
            direct_items,
            source_index,
            source_index..source_index + 1,
        )?;
        let leading_first = leading_bookmark_start_item(&source_xml, direct_items, source_index)?;
        let last = owned.after.last().copied().unwrap_or(source_index);
        if same_container && (leading_first..=last + 1).contains(&destination_index) {
            return Ok(());
        }
        if !same_container {
            if source_item.full.start < boundary && boundary < source_item.full.end {
                return Err(Error::Other(
                    "a block cannot move into itself: the destination is inside the content \
                     control being moved"
                        .to_owned(),
                ));
            }
            source_slot.check_leaves_a_block(&source_xml)?;
            destination_slot.check_takes(&close_content_fragment_namespaces(
                &fragment.xml,
                &fragment.namespace_scope,
            )?)?;
        }
        refuse_partial_complex_field(
            &source_xml,
            std::slice::from_ref(&source_item.full),
            &format!("the block at {:?}", source_location.index_path),
        )?;
        let mut removed = owned
            .before
            .iter()
            .chain(&owned.after)
            .map(|index| direct_items[*index].full.clone())
            .collect::<Vec<_>>();
        removed.push(source_item.full);
        removed.sort_by_key(|range| range.start);
        let mut moved = Vec::new();
        for range in &removed {
            moved.extend_from_slice(&source_xml[range.clone()]);
        }
        let removed_before = removed
            .iter()
            .filter(|range| range.end <= boundary)
            .map(|range| range.len())
            .sum::<usize>();
        let adjusted_boundary = boundary.checked_sub(removed_before).ok_or_else(|| {
            Error::Other("content move destination underflowed after removal".to_owned())
        })?;
        // The destination container, where it lies once the moved bytes are
        // out: an empty `w:sdtContent` is written open around them.
        let shifted = |at: usize| {
            at - removed
                .iter()
                .filter(|range| range.end <= at)
                .map(|range| range.len())
                .sum::<usize>()
        };
        let container = StoryOwnerSpan {
            full: shifted(destination_slot.owner.full.start)
                ..shifted(destination_slot.owner.full.end),
            ..destination_slot.owner.clone()
        };
        let mut updated = source_xml;
        for range in removed.into_iter().rev() {
            updated.drain(range);
        }
        insert_story_fragment(&mut updated, &container, adjusted_boundary, moved)?;
        set_story_source_xml(&mut candidate, &part_name, updated)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }
}

/// The local name of the root element of `xml`.
fn root_local_name(xml: &[u8]) -> Option<Vec<u8>> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer).ok()? {
            Event::Start(element) | Event::Empty(element) => {
                return Some(element.local_name().as_ref().to_vec());
            }
            Event::Eof => return None,
            _ => buffer.clear(),
        }
    }
}
