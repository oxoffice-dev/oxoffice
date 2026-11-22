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
        let result = edit(&mut Paragraph {
            inner: &mut paragraph,
        })?;
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
    fn hyperlink_character_style_ids(&self) -> HashSet<String> {
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

    /// Insert one owned fragment at a checked direct-child boundary.
    ///
    /// A location at a block inserts before the bookmark starts written just
    /// before it, which belong to that block (Google Docs writes a heading's
    /// bookmark so); a location at such a bookmark start inserts right there.
    /// The other insertions at a location ([`Self::insert_html_fragment`],
    /// [`Self::clone_content`], [`Self::move_content`],
    /// [`Self::import_fragment`]) place their content the same way.
    pub fn insert_content(
        &mut self,
        destination: &ContentLocation,
        fragment: ContentFragment,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, owner) = candidate.story_source_and_owner(&destination.story)?;
        let part_name = source.part_name.clone();
        let source_xml = source.xml.into_owned();
        let (boundary, _, _) = validated_insertion_boundary(&source_xml, &owner, destination)?;
        reject_section_owning_content_fragment(&fragment)?;
        let fragment_xml =
            content_fragment_for_insertion(&candidate, &destination.story, &fragment)?;
        // The same fragment can be inserted twice: its paragraph and row ids
        // (w14:paraId, w14:textId) go, as clone_content drops them, and Word
        // gives new ones.
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
        insert_story_fragment(&mut updated, &owner, boundary, fragment_xml)?;
        set_story_source_xml(&mut candidate, &part_name, updated)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Insert one bounded HTML fragment at a checked direct-child boundary.
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
                "HTML fragments support body, table-cell, header, and footer stories".to_owned(),
            ));
        }
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, owner) = candidate.story_source_and_owner(&destination.story)?;
        let part_name = source.part_name.clone();
        let mut source_xml = source.xml.into_owned();
        let (boundary, direct_index, _) =
            validated_insertion_boundary(&source_xml, &owner, destination)?;
        let modeled_cell_route = (destination.story.kind == StoryKind::TableCell
            && part_name == candidate.doc_part_name)
            .then(|| modeled_main_cell_route(&source_xml, &owner))
            .transpose()?;
        let projected =
            crate::html::project_html_fragment(&mut candidate, &destination.story, html, images)?;
        let inserted_count = projected.content.len();
        if destination.story.kind == StoryKind::Body && part_name == candidate.doc_part_name {
            candidate
                .document
                .body
                .content
                .splice(direct_index..direct_index, projected.content);
        } else if destination.story.kind == StoryKind::TableCell
            && part_name == candidate.doc_part_name
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
            insert_html_content_into_cell(cell, direct_index, projected.content)?;
        } else {
            let mut fragment_xml = Vec::new();
            for content in projected.content {
                fragment_xml.extend_from_slice(&serialize_content_fragment(content)?);
            }
            insert_story_fragment(&mut source_xml, &owner, boundary, fragment_xml)?;
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

    /// Import an owned cross-document fragment at a checked story boundary.
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
        let (mut owner, mut boundary, _, _) =
            fragment_insertion_boundary(&source_xml, &owner, destination)?;
        if fragment.include_final_section_properties
            && (destination.story.kind != StoryKind::Body
                || (!destination.is_end && destination.index_path.len() != 1))
        {
            return Err(Error::Other(
                "section-inclusive fragments require the main body destination".to_owned(),
            ));
        }
        let imported_xml = fragment.import_content_staged(
            &mut candidate,
            &part_name,
            policy,
            false,
            &mut Vec::new(),
        )?;
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
            let (updated_owner, updated_boundary, _, _) =
                fragment_insertion_boundary(&source_xml, &updated_owner, destination)?;
            owner = updated_owner;
            boundary = updated_boundary;
        }
        insert_story_fragment(&mut source_xml, &owner, boundary, imported_xml)?;
        set_story_source_xml(&mut candidate, &part_name, source_xml)?;
        let reopened = candidate.reopen_prepared_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Remove one checked direct child and return it as an owned fragment.
    ///
    /// The bookmark starts written just before the child whose bookmark ends
    /// inside it go too, and are not part of the fragment: the fragment keeps
    /// the bookmark's end without its start.
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
        let item = validated_direct_content_item(&source_xml, &owner, location)?;
        let namespace_scope = story_namespace_scope_at(&source_xml, item.full.start)?;
        let closed =
            close_content_fragment_namespaces(&source_xml[item.full.clone()], &namespace_scope)?;
        Self::refuse_commented_fragment(&closed)?;
        if owner.kind == StoryKind::TableCell
            && item.kind == StoryItemKind::Paragraph
            && direct_story_content_items(&source_xml, &owner)?
                .iter()
                .filter(|candidate| candidate.kind == StoryItemKind::Paragraph)
                .count()
                == 1
        {
            return Err(Error::Other(
                "cannot remove the final required table-cell paragraph".to_owned(),
            ));
        }
        let mut fragment = ContentFragment {
            kind: item.kind,
            xml: source_xml[item.full.clone()].to_vec(),
            source_part_name: Some(part_name.clone()),
            source_story_kind: Some(location.story.kind),
            source_owner_index: Some(location.story.owner_index),
            namespace_scope,
            relationships: Vec::new(),
        };
        reject_section_owning_content_fragment(&fragment)?;
        validate_fragment_relationships(&candidate, &location.story, &fragment)?;
        fragment.relationships = referenced_relationships(&candidate, &location.story, &fragment)?;
        // The bookmark starts that lead the item and end inside it go too.
        let direct_items = direct_story_content_items(&source_xml, &owner)?;
        let mut removed = match direct_items
            .iter()
            .position(|direct| direct.full == item.full)
        {
            Some(index) => owned_leading_bookmark_start_items(
                &source_xml,
                &direct_items,
                index,
                index..index + 1,
            )?
            .into_iter()
            .map(|start| direct_items[start].full.clone())
            .collect(),
            None => Vec::new(),
        };
        removed.push(item.full);
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
        if !freed.is_empty() {
            let referenced = xml_attribute_values(&updated)?;
            for id in freed {
                let entry = (part_name.clone(), id.to_owned());
                if !referenced.contains(id) && !candidate.freed_relationships.contains(&entry) {
                    candidate.freed_relationships.push(entry);
                }
            }
        }
        set_story_source_xml(&mut candidate, &part_name, updated)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(fragment)
    }

    /// Clone one checked direct child into a checked insertion boundary.
    ///
    /// Document-wide identities of the copy are made unique, and its
    /// paragraphs and table rows drop `w14:paraId` and `w14:textId`.
    pub fn clone_content(
        &mut self,
        source_location: &ContentLocation,
        destination: &ContentLocation,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, source_owner) = candidate.story_source_and_owner(&source_location.story)?;
        let source_xml = source.xml.into_owned();
        let source_item =
            validated_direct_content_item(&source_xml, &source_owner, source_location)?;
        let mut fragment = ContentFragment {
            kind: source_item.kind,
            xml: source_xml[source_item.full.clone()].to_vec(),
            source_part_name: Some(source_location.story.part_name.clone()),
            source_story_kind: Some(source_location.story.kind),
            source_owner_index: Some(source_location.story.owner_index),
            namespace_scope: story_namespace_scope_at(&source_xml, source_item.full.start)?,
            relationships: Vec::new(),
        };
        reject_section_owning_content_fragment(&fragment)?;
        fragment.relationships =
            referenced_relationships(&candidate, &source_location.story, &fragment)?;

        let (destination_source, destination_owner) =
            candidate.story_source_and_owner(&destination.story)?;
        let destination_part_name = destination_source.part_name.clone();
        let destination_xml = destination_source.xml.into_owned();
        let (boundary, _, _) =
            validated_insertion_boundary(&destination_xml, &destination_owner, destination)?;
        fragment.xml = freshen_content_fragment_identities(&mut candidate, &fragment)?;
        let fragment_xml =
            content_fragment_for_insertion(&candidate, &destination.story, &fragment)?;
        let mut updated = destination_xml;
        insert_story_fragment(&mut updated, &destination_owner, boundary, fragment_xml)?;
        set_story_source_xml(&mut candidate, &destination_part_name, updated)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Move one checked direct child within its current story owner.
    ///
    /// The bookmark starts written just before the child whose bookmark ends
    /// inside it move with it; one that ends further on stays. A destination
    /// at the child, at its leading starts or just after it moves nothing.
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
        let source_item = validated_direct_content_item(&source_xml, &owner, source_location)?;

        let (destination_source, destination_owner) =
            candidate.story_source_and_owner(&destination.story)?;
        let destination_xml = destination_source.xml.into_owned();
        let (boundary, destination_index, _) =
            validated_insertion_boundary(&destination_xml, &destination_owner, destination)?;
        if source_location.story != destination.story {
            return Err(Error::Other(
                "content can move only within one unchanged story owner".to_owned(),
            ));
        }
        let direct_items = direct_story_content_items(&source_xml, &owner)?;
        let source_index = direct_items
            .iter()
            .position(|item| item.full == source_item.full)
            .ok_or_else(|| Error::Other("direct content source was not found".to_owned()))?;
        let fragment = ContentFragment {
            kind: source_item.kind,
            xml: source_xml[source_item.full.clone()].to_vec(),
            source_part_name: Some(part_name.clone()),
            source_story_kind: Some(source_location.story.kind),
            source_owner_index: Some(source_location.story.owner_index),
            namespace_scope: story_namespace_scope_at(&source_xml, source_item.full.start)?,
            relationships: Vec::new(),
        };
        reject_section_owning_content_fragment(&fragment)?;
        validate_fragment_relationships(&candidate, &destination.story, &fragment)?;
        // The item moves with the bookmark starts that lead it and end
        // inside it; a destination at the item or just after it changes
        // nothing.
        let leading_first = leading_bookmark_start_item(&source_xml, &direct_items, source_index)?;
        if (leading_first..=source_index + 1).contains(&destination_index) {
            return Ok(());
        }
        let mut removed = owned_leading_bookmark_start_items(
            &source_xml,
            &direct_items,
            source_index,
            source_index..source_index + 1,
        )?
        .into_iter()
        .map(|start| direct_items[start].full.clone())
        .collect::<Vec<_>>();
        removed.push(source_item.full);
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
        let mut updated = source_xml;
        for range in removed.into_iter().rev() {
            updated.drain(range);
        }
        updated.splice(adjusted_boundary..adjusted_boundary, moved);
        set_story_source_xml(&mut candidate, &part_name, updated)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }
}
