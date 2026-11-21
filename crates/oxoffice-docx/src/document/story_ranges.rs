//! Paragraphs and ranges of a story: editing them, comment ranges and their markers.

use super::*;

impl Document {
    pub(crate) fn story_paragraph_mut(&mut self, location: &ContentLocation) -> Result<&mut CT_P> {
        let (part_name, paragraph_slot, ordinal, cell_route) = {
            let (source, owner) = self.story_source_and_owner(&location.story)?;
            let part_name = source.part_name.clone();
            let source_xml = source.xml.into_owned();
            // A paragraph inside a block content control of the body is the
            // control item followed by the paragraph's position among the
            // paragraphs of that control.
            let (item_index, ordinal) = match location.index_path.as_slice() {
                [item_index] => (*item_index, None),
                [item_index, ordinal] if location.story.kind == StoryKind::Body => {
                    (*item_index, Some(*ordinal))
                }
                _ => {
                    return Err(StoryError::InvalidPath {
                        path: location.index_path.clone(),
                    }
                    .into());
                }
            };
            let items = scan_story_items(&source_xml, &owner, None)?;
            let item = items.get(item_index).ok_or(StoryError::OutOfBounds {
                index: item_index,
                len: items.len(),
            })?;
            if location.item_kind != StoryItemKind::Paragraph {
                return Err(Error::Other(
                    "comment positions must identify paragraphs".to_owned(),
                ));
            }
            if ordinal.is_some()
                && (item.kind != StoryItemKind::ContentControl || !item.direct_owner_child)
            {
                return Err(Error::Other(
                    "a two-segment comment position must start with a block content control"
                        .to_owned(),
                ));
            }
            if ordinal.is_none() && item.kind != location.item_kind {
                return Err(StoryError::KindMismatch {
                    expected: location.item_kind,
                    actual: item.kind,
                }
                .into());
            }
            // A paragraph item is a direct child of its owner. In the body it
            // resolves to its direct body slot, the count of direct items
            // before it, and in a cell to its position among the cell's
            // direct paragraphs. Neither counts the paragraphs inside a block
            // content control. The body section properties are serialized
            // last, so they never precede a paragraph.
            let preceding = items[..item_index].iter();
            let paragraph_slot = if location.story.kind == StoryKind::Body {
                preceding.filter(|item| item.direct_owner_child).count()
            } else {
                preceding
                    .filter(|item| item.kind == StoryItemKind::Paragraph)
                    .count()
            };
            let cell_route = (location.story.kind == StoryKind::TableCell
                && part_name == self.doc_part_name)
                .then(|| modeled_main_cell_route(&source_xml, &owner))
                .transpose()?;
            (part_name, paragraph_slot, ordinal, cell_route)
        };
        if part_name != self.doc_part_name {
            return Err(Error::Other(
                "path-aware comments currently support the main document story".to_owned(),
            ));
        }
        match location.story.kind {
            StoryKind::Body => {
                match (self.document.body.content.get_mut(paragraph_slot), ordinal) {
                    (Some(BodyContent::Paragraph(paragraph)), None) => Ok(paragraph),
                    (Some(BodyContent::ContentControl(control)), Some(ordinal)) => control
                        .content
                        .iter_mut()
                        .filter_map(|child| match child {
                            SdtContent::Paragraph(paragraph) => Some(paragraph),
                            _ => None,
                        })
                        .nth(ordinal)
                        .ok_or_else(|| {
                            Error::Other(format!(
                                "comment content control has no paragraph {ordinal}"
                            ))
                        }),
                    _ => Err(Error::Other("comment body paragraph is missing".to_owned())),
                }
            }
            StoryKind::TableCell => {
                let (content_index, mut cell_index) = cell_route.ok_or_else(|| {
                    Error::Other("comment position has no modeled table-cell route".to_owned())
                })?;
                let content = self
                    .document
                    .body
                    .content
                    .get_mut(content_index)
                    .ok_or_else(|| Error::Other("comment table container is missing".to_owned()))?;
                let cell = match content {
                    BodyContent::Table(table) => nth_cell_in_table(table, &mut cell_index),
                    BodyContent::ContentControl(control) => {
                        nth_cell_in_control(control, &mut cell_index)
                    }
                    BodyContent::Paragraph(_) | BodyContent::RawXml(_) => None,
                }
                .ok_or_else(|| Error::Other("comment table cell is missing".to_owned()))?;
                cell.content
                    .iter_mut()
                    .filter_map(|child| match child {
                        CellContent::Paragraph(paragraph) => Some(paragraph),
                        CellContent::Table(_) | CellContent::ContentControl(_) => None,
                    })
                    .nth(paragraph_slot)
                    .ok_or_else(|| Error::Other("comment cell paragraph is missing".to_owned()))
            }
            _ => Err(Error::Other(
                "path-aware comments support body and table-cell paragraphs".to_owned(),
            )),
        }
    }

    /// Insert a checked run through its physical story paragraph owner.
    pub(crate) fn insert_story_run_staged(
        &mut self,
        position: &crate::StoryRunPosition,
        run: CT_R,
    ) -> Result<()> {
        let (source, item) = self.story_range_paragraph_source(&position.location)?;
        if item.kind != StoryItemKind::Paragraph {
            return Err(Error::Other(
                "field insertion needs a paragraph location".into(),
            ));
        }
        let part_name = source.part_name.clone();
        let mut xml = source.xml.into_owned();
        let scope = story_namespace_scope_at(&xml, item.full.start)?;
        let closed = close_content_fragment_namespaces(&xml[item.full.clone()], &scope)?;
        let mut paragraph = CT_P::from_xml_fragment(&closed)?;
        paragraph
            .insert_accepted_run(position.run_index, run)
            .map_err(|error| Error::Other(format!("field insertion boundary: {error}")))?;
        let replacement = serialize_content_fragment(BodyContent::Paragraph(Box::new(paragraph)))?;
        let replacement = close_content_fragment_namespaces(&replacement, &scope)?;
        xml.splice(item.full, replacement);
        set_story_source_xml(self, &part_name, xml)
    }

    pub(crate) fn anchor_story_range(
        &mut self,
        range: &crate::comments::StoryRunRange,
        anchor: RangeAnchor<'_>,
        label: &str,
        endpoint_probe: bool,
    ) -> Result<()> {
        let start = &range.start.location;
        let end = &range.end.location;
        let start_run = range.start.run_index;
        let end_run = range.end.run_index;
        if start.story != end.story {
            return Err(Error::Other(format!(
                "{label} range endpoints must be ordered in one story owner"
            )));
        }
        let nested_body_control = [start, end].iter().any(|location| {
            location.index_path.len() == 2
                && self
                    .story_item_source(&ContentLocation::new(
                        location.story.clone(),
                        StoryItemKind::ContentControl,
                        vec![location.index_path[0]],
                    ))
                    .is_ok_and(|(_, control)| !control.direct_owner_child)
        });
        // Multiple paragraphs must be qualified before either endpoint changes
        // the owner's fingerprint. The raw path below publishes both spans once.
        if start == end
            && start.story.kind() == StoryKind::Body
            && (start.index_path.len() == 2 || end.index_path.len() == 2)
            && !nested_body_control
        {
            if start.index_path > end.index_path {
                return Err(Error::Other(format!(
                    "{label} range endpoints must be ordered in one story owner"
                )));
            }
            let mut first = self.story_paragraph_mut(start)?.clone();
            let mut last = if start == end {
                first.clone()
            } else {
                self.story_paragraph_mut(end)?.clone()
            };
            if start == end {
                first
                    .anchor_accepted_range(Some(start_run), Some(end_run), anchor)
                    .map_err(|error| {
                        Error::Other(format!("{label} range cannot be anchored: {error}"))
                    })?;
            } else {
                first
                    .anchor_accepted_range(Some(start_run), None, anchor)
                    .map_err(|error| {
                        Error::Other(format!("{label} range cannot be anchored: {error}"))
                    })?;
                last.anchor_accepted_range(None, Some(end_run), anchor)
                    .map_err(|error| {
                        Error::Other(format!("{label} range cannot be anchored: {error}"))
                    })?;
            }
            *self.story_paragraph_mut(start)? = first;
            if start != end {
                *self.story_paragraph_mut(end)? = last;
            }
            return Ok(());
        }
        let (source, start_item) = self.story_range_paragraph_source(start)?;
        let part_name = source.part_name.clone();
        let source_xml = source.xml.into_owned();
        let same_endpoint = start == end;
        let end_item = if same_endpoint {
            start_item.clone()
        } else {
            self.story_range_paragraph_source(end)?.1
        };
        if start_item.full.start > end_item.full.start {
            return Err(Error::Other(format!(
                "{label} range endpoints must be ordered in one story owner"
            )));
        }
        let read_paragraph = |item: &StoryItemSpan| -> Result<CT_P> {
            if item.kind != StoryItemKind::Paragraph {
                return Err(Error::Other(
                    "comment positions must identify paragraphs".to_owned(),
                ));
            }
            let scope = story_namespace_scope_at(&source_xml, item.full.start)?;
            let closed = close_content_fragment_namespaces(&source_xml[item.full.clone()], &scope)?;
            Ok(CT_P::from_xml_fragment(&closed)?)
        };
        let mut first = read_paragraph(&start_item)?;
        let mut last = if start_item.full == end_item.full {
            None
        } else {
            Some(read_paragraph(&end_item)?)
        };
        for (boundary, index, paragraph) in [
            ("start", start_run, &first),
            ("end", end_run, last.as_ref().unwrap_or(&first)),
        ] {
            let count = paragraph.accepted_run_paths().len();
            if index > count {
                return Err(Error::Other(format!(
                    "{label} range {boundary} run index {index} exceeds paragraph run count {count}"
                )));
            }
        }
        if start_item.full == end_item.full {
            if start_run > end_run {
                return Err(Error::Other(format!(
                    "{label} range start must not follow its end"
                )));
            }
            first
                .anchor_accepted_range(Some(start_run), Some(end_run), anchor)
                .map_err(|error| {
                    Error::Other(format!("{label} range cannot be anchored: {error}"))
                })?;
        } else {
            first
                .anchor_accepted_range(Some(start_run), None, anchor)
                .map_err(|error| {
                    Error::Other(format!("{label} range cannot be anchored: {error}"))
                })?;
            last.as_mut()
                .expect("distinct endpoint was parsed")
                .anchor_accepted_range(None, Some(end_run), anchor)
                .map_err(|error| {
                    Error::Other(format!("{label} range cannot be anchored: {error}"))
                })?;
        }
        let mut edits = vec![(start_item.full, first)];
        if let Some(last) = last {
            edits.push((end_item.full, last));
        }
        edits.sort_by_key(|item| std::cmp::Reverse(item.0.start));
        let mut updated = source_xml;
        for (span, paragraph) in edits {
            let scope = story_namespace_scope_at(&updated, span.start)?;
            let xml = serialize_content_fragment(BodyContent::Paragraph(Box::new(paragraph)))?;
            let xml = close_content_fragment_namespaces(&xml, &scope)?;
            updated.splice(span, xml);
        }
        let updated = match anchor {
            RangeAnchor::Comment(id) if !endpoint_probe => {
                self.comment_move_source_with_namespaces(&part_name, updated, id, false)?
            }
            _ => updated,
        };
        set_story_source_xml(self, &part_name, updated)
    }

    /// Stage a comment move using physical source edits, then bind the surviving
    /// destination paragraphs and accepted run boundaries to their new owners.
    pub(crate) fn move_comment_story_range(
        &mut self,
        entry: &crate::comments::StoryRangeRef,
        target: &StoryRunRange,
        id: i32,
    ) -> Result<()> {
        // Validate both endpoints before any source is removed. The probe never
        // publishes its synthesized reference or changes the requested axis.
        let mut probe = self.clone_for_staging();
        probe.anchor_story_range(target, RangeAnchor::Comment(id), "comment", true)?;
        let endpoints = [&target.start, &target.end]
            .map(|position| {
                let (source, paragraph) = self.story_range_paragraph_source(&position.location)?;
                Ok((source.part_name.clone(), paragraph.full.start))
            })
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        let (part_name, xml, selected) = self.story_range_marker_spans(entry, true)?;
        let (edits, reference, reference_span) = comment_move_source_edits(&xml, &selected)?;
        let mut reference_axis = None;
        let (_, paragraphs) = self.story_range_paragraph_sources()?;
        for (_, _, span, location, raw) in paragraphs {
            if location.story != entry.range().start.location.story {
                continue;
            }
            // This inventory already owns the checked physical span and
            // closed XML. Resolving every location again repeats owner scans.
            if !span.contains(&reference_span.start) {
                continue;
            }
            let paragraph_model = CT_P::from_xml_fragment(&raw)?;
            let indices = paragraph_model
                .accepted_run_paths()
                .iter()
                .enumerate()
                .filter_map(|(index, path)| {
                    paragraph_model
                        .accepted_run(path)
                        .filter(|run| {
                            run.content.iter().any(|content| {
                                matches!(content,
                        RunContent::CommentReference { id: found, .. } if *found == id)
                            })
                        })
                        .map(|_| index)
                })
                .collect::<Vec<_>>();
            if indices.len() != 1 || reference_axis.is_some() {
                return Err(Error::Other(
                    "comment reference has no unique accepted run boundary".into(),
                ));
            }
            reference_axis = Some((span.start, indices[0]));
        }
        let (reference_paragraph, reference_index) = reference_axis.ok_or_else(|| {
            Error::Other("comment reference is outside the supported accepted run axis".into())
        })?;
        let map_offset = |offset: usize| -> Result<usize> {
            if edits.iter().any(|span| span.contains(&offset)) {
                return Err(Error::Other(
                    "comment move destination is inside a removed marker-only owner".into(),
                ));
            }
            Ok(offset
                - edits
                    .iter()
                    .filter(|span| span.end <= offset)
                    .map(|span| span.len())
                    .sum::<usize>())
        };
        let mut updated = xml;
        for span in edits.iter().rev() {
            updated.drain(span.clone());
        }
        let updated = self.comment_move_source_with_namespaces(&part_name, updated, id, true)?;
        set_story_source_xml(self, &part_name, updated)?;
        let stories = self.stories()?;
        let rebase = |position: &StoryRunPosition,
                      endpoint: &(String, usize)|
         -> Result<StoryRunPosition> {
            let original = position.location.story();
            let story = stories
                .iter()
                .find(|story| {
                    story.kind == original.kind
                        && story.part_name == original.part_name
                        && story.owner_index == original.owner_index
                })
                .ok_or_else(|| Error::Other("comment move destination story disappeared".into()))?;
            let expected = if endpoint.0 == part_name {
                map_offset(endpoint.1)?
            } else {
                endpoint.1
            };
            let (source, owner) = self.story_source_and_owner(story)?;
            let qualified =
                story_range_paragraph_spans(source.xml.as_ref(), &[(story.clone(), owner.clone())])
                    .ok()
                    .filter(|spans| spans.len() == 1)
                    .and_then(|mut spans| spans.pop());
            let mut found = None;
            if let Some(paragraphs) = qualified {
                for (location, span) in paragraphs {
                    if span.start == expected {
                        if found.is_some() {
                            return Err(Error::Other("ambiguous comment move destination".into()));
                        }
                        found = Some(location.index_path);
                    }
                }
            } else {
                // Preserve the original lookup and its early error ordering.
                for (index, item) in scan_story_items(source.xml.as_ref(), &owner, None)?
                    .iter()
                    .enumerate()
                {
                    let paths = match item.kind {
                        StoryItemKind::Paragraph => vec![(vec![index], item.full.start)],
                        StoryItemKind::ContentControl => {
                            scan_story_control_paragraphs(source.xml.as_ref(), item)?
                                .iter()
                                .enumerate()
                                .map(|(child, span)| (vec![index, child], span.start))
                                .collect()
                        }
                        _ => Vec::new(),
                    };
                    for (path, offset) in paths {
                        if offset == expected {
                            if found.is_some() {
                                return Err(Error::Other(
                                    "ambiguous comment move destination".into(),
                                ));
                            }
                            found = Some(path);
                        }
                    }
                }
            }
            let path = found.ok_or_else(|| {
                Error::Other("comment move destination paragraph disappeared".into())
            })?;
            let removed_before_boundary = endpoint.0 == part_name
                && endpoint.1 == reference_paragraph
                && position.run_index > reference_index
                && edits.iter().any(|span| {
                    span.start <= reference_span.start && reference_span.end <= span.end
                });
            Ok(StoryRunPosition {
                location: ContentLocation::new(story.clone(), StoryItemKind::Paragraph, path),
                run_index: position.run_index - usize::from(removed_before_boundary),
            })
        };
        let rebound = StoryRunRange {
            start: rebase(&target.start, &endpoints[0])?,
            end: rebase(&target.end, &endpoints[1])?,
        };
        self.anchor_story_range(&rebound, RangeAnchor::Comment(id), "comment", false)?;
        // Replace only the uniquely synthesized selected reference run. The
        // original run carries its namespace closure, formatting and raw attrs.
        self.restore_comment_reference_run(id, reference)
    }

    pub(crate) fn remove_comment_source_for_move(
        &mut self,
        entry: &crate::comments::StoryRangeRef,
    ) -> Result<Vec<u8>> {
        let (part, xml, selected) = self.story_range_marker_spans(entry, true)?;
        let (edits, reference, _) = comment_move_source_edits(&xml, &selected)?;
        let mut updated = xml;
        for span in edits.iter().rev() {
            updated.drain(span.clone());
        }
        let updated = self.comment_move_source_with_namespaces(
            &part,
            updated,
            match entry.kind() {
                crate::comments::StoryRangeKind::Comment { id } => *id,
                _ => return Err(Error::Other("expected comment move source".into())),
            },
            true,
        )?;
        set_story_source_xml(self, &part, updated)?;
        Ok(reference)
    }

    pub(crate) fn restore_comment_reference_run(
        &mut self,
        id: i32,
        reference: Vec<u8>,
    ) -> Result<()> {
        let selected = self.checked_movable_comment(id)?;
        self.splice_comment_reference_run(selected.range().end.location.story(), id, reference)
    }

    /// Replace the reference run that anchoring synthesized for comment `id`
    /// in `story` with the original one. A reply moving with its thread names
    /// the story its reply authoring wrote, without another selection.
    pub(crate) fn splice_comment_reference_run(
        &mut self,
        story: &crate::StoryId,
        id: i32,
        reference: Vec<u8>,
    ) -> Result<()> {
        let (source, owner) = self.story_source_and_owner(story)?;
        let part = source.part_name.clone();
        let span = comment_reference_run_span(source.xml.as_ref(), &owner.full, id)?;
        let mut xml = source.xml.into_owned();
        xml.splice(span, reference);
        let xml = self.comment_move_source_with_namespaces(&part, xml, id, false)?;
        set_story_source_xml(self, &part, xml)
    }

    // Source discovery retains the established canonical fingerprint axis.
    // Before a move replaces that source, replay its original qualified nested
    // declarations so the replacement does not erase preserved raw aliases.
    fn comment_move_source_with_namespaces(
        &self,
        part: &str,
        xml: Vec<u8>,
        id: i32,
        source_removal: bool,
    ) -> Result<Vec<u8>> {
        if part != self.doc_part_name {
            return Ok(xml);
        }
        let original = self
            .package
            .get_part(part)
            .ok_or_else(|| Error::Other("comment move main source is missing".into()))?;
        // Apply the same proved source edits to the original namespace authority.
        // Whole transported runs disappear, while mixed source runs keep their
        // declarations and every non-reference child in their original order.
        let mut remaining_source = original.to_vec();
        if source_removal {
            let selected = crate::comments::comment_source_markers(original)?
                .into_iter()
                .filter(|marker| marker.id == id)
                .map(|marker| marker.span)
                .collect::<Vec<_>>();
            if selected.len() != 3 {
                return Err(Error::Other(
                    "comment move original source lacks one qualified pair and reference".into(),
                ));
            }
            let (edits, _, _) = comment_move_source_edits(original, &selected)?;
            for span in edits.into_iter().rev() {
                remaining_source.drain(span);
            }
        }
        let original = remaining_source.as_slice();
        let mut owners = nested_modeled_namespace_owners(original)?;
        let original_spans = modeled_owner_spans(original)?;
        let spans = modeled_owner_spans(&xml)?;
        for owner in &mut owners {
            let has_pair = |source: &[u8], span: &ModeledOwnerSpan| -> Result<bool> {
                let mut scope = span
                    .bindings
                    .iter()
                    .map(|(name, uri)| (namespace_prefix(name), uri.clone()))
                    .collect::<BTreeMap<_, _>>();
                for (name, uri) in &owner.declarations {
                    let prefix = namespace_prefix(name);
                    scope.insert(prefix, uri.clone());
                }
                let closed =
                    close_content_fragment_namespaces(&source[span.start..span.end], &scope)?;
                Ok(CT_P::comment_range_text_from_source(&closed, id)?.is_some())
            };
            let mut original_selected = false;
            for span in original_spans
                .iter()
                .filter(|span| span.local_name == owner.local_name)
            {
                let markers = candidate_namespace_owner_markers(
                    &original[span.start..span.end],
                    &owner.declarations,
                    &span.bindings,
                    &owner.markers,
                )?;
                if marker_multiset_matches(&owner.markers, &markers, true)
                    && has_pair(original, span)?
                {
                    original_selected = true;
                }
            }
            let mut matching = Vec::new();
            for span in spans
                .iter()
                .filter(|span| span.local_name == owner.local_name)
            {
                let markers = candidate_namespace_owner_markers(
                    &xml[span.start..span.end],
                    &owner.declarations,
                    &span.bindings,
                    &owner.markers,
                )?;
                if marker_multiset_matches(&owner.markers, &markers, true) {
                    matching.push(span);
                }
            }
            let selected = matching
                .iter()
                .map(|span| has_pair(&xml, span))
                .collect::<Result<Vec<_>>>()?;
            if original_selected || selected.iter().any(|selected| *selected) {
                let [span] = matching.as_slice() else {
                    return Err(Error::Other(
                        "comment move has ambiguous retained namespace owner".into(),
                    ));
                };
                owner.snapshot = logical_owner_snapshot(
                    &xml[span.start..span.end],
                    &span.bindings,
                    &owner.declarations,
                )?;
            }
        }
        replay_nested_namespace_declarations(&xml, &owners)
    }

    fn story_range_marker_spans(
        &self,
        entry: &crate::comments::StoryRangeRef,
        move_comment_reference: bool,
    ) -> Result<(String, Vec<u8>, Vec<Range<usize>>)> {
        use crate::comments::StoryRangeKind;
        let (source, owner) = self.story_source_and_owner(entry.range().start.location.story())?;
        let part_name = source.part_name.clone();
        let xml = source.xml.as_ref();
        let qualified = story_range_paragraph_spans(
            xml,
            &[(entry.range().start.location.story().clone(), owner.clone())],
        )
        .ok()
        .filter(|spans| spans.len() == 1)
        .and_then(|mut spans| spans.pop());
        let paragraphs = if let Some(paragraphs) = qualified {
            paragraphs
                .into_iter()
                .map(|(_, span)| span)
                .collect::<Vec<_>>()
        } else {
            let mut paragraphs = Vec::new();
            for item in scan_story_items(xml, &owner, None)? {
                match item.kind {
                    StoryItemKind::Paragraph => paragraphs.push(item.full),
                    StoryItemKind::ContentControl => {
                        paragraphs.extend(scan_story_control_paragraphs(xml, &item)?);
                    }
                    _ => {}
                }
            }
            paragraphs
        };
        let (start_tag, end_tag, identity) = match entry.kind() {
            StoryRangeKind::Bookmark { id, .. } => (
                b"bookmarkStart".as_slice(),
                b"bookmarkEnd".as_slice(),
                id.to_string(),
            ),
            StoryRangeKind::Comment { id } => (
                b"commentRangeStart".as_slice(),
                b"commentRangeEnd".as_slice(),
                id.to_string(),
            ),
            StoryRangeKind::Permission { id, .. } => (
                b"permStart".as_slice(),
                b"permEnd".as_slice(),
                id.to_string(),
            ),
            StoryRangeKind::Proofing { kind } => {
                (b"proofErr".as_slice(), b"proofErr".as_slice(), kind.clone())
            }
        };
        // Source-order disjoint ranges admit one forward membership cursor.
        // Unqualified inventories keep the original containment predicate.
        let ordered = paragraphs
            .iter()
            .all(|span| span.start <= span.end && span.end <= xml.len())
            && paragraphs
                .windows(2)
                .all(|pair| pair[0].end <= pair[1].start);
        let mut paragraph_cursor = 0usize;
        let mut paragraph_contains = |offset: usize| {
            if !ordered {
                return paragraphs
                    .iter()
                    .any(|paragraph| paragraph.contains(&offset));
            }
            while let Some(paragraph) = paragraphs.get(paragraph_cursor) {
                if offset < paragraph.end {
                    return paragraph.start <= offset;
                }
                paragraph_cursor += 1;
            }
            false
        };
        let mut reader = NsReader::from_reader(xml);
        reader.config_mut().trim_text(false);
        let mut ranges = Vec::new();
        let mut ordinal = 0usize;
        let mut stack = Vec::<(bool, Vec<u8>, usize)>::new();
        let mut buffer = Vec::new();
        loop {
            let before = reader.buffer_position() as usize;
            let (namespace, event) = match reader.read_resolved_event_into(&mut buffer) {
                Ok(event) => event,
                Err(error) => {
                    return Err(Error::Other(format!("range removal scan failed: {error}")));
                }
            };
            let is_word = word_element(&namespace);
            drop(namespace);
            let after = reader.buffer_position() as usize;
            let hidden = stack.iter().any(|(word, name, start)| {
                *word
                    && matches!(
                        name.as_slice(),
                        b"del" | b"moveFrom" | b"smartTag" | b"customXml" | b"fldSimple"
                    )
                    || *word && name == b"txbxContent" && *start > owner.full.start
            });
            if let Event::End(_) = &event {
                stack.pop();
            }
            if let Event::Start(element) = &event {
                stack.push((is_word, element.local_name().as_ref().to_vec(), before));
            }
            let element = match &event {
                Event::Start(element) | Event::Empty(element)
                    if !hidden
                        && is_word
                        && matches!(
                            element.local_name().as_ref(),
                            b"bookmarkStart"
                                | b"bookmarkEnd"
                                | b"commentRangeStart"
                                | b"commentRangeEnd"
                                | b"permStart"
                                | b"permEnd"
                                | b"proofErr"
                                | b"commentReference"
                        )
                        && paragraph_contains(before) =>
                {
                    element
                }
                Event::Eof => break,
                _ => {
                    buffer.clear();
                    continue;
                }
            };
            let mut id_value = None;
            let mut type_value = None;
            for attribute in element.attributes() {
                let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
                let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                if word_element(&namespace) && matches!(local.as_ref(), b"id" | b"type") {
                    let value = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                        .map_err(|error| Error::Other(error.to_string()))?;
                    if local.as_ref() == b"id" {
                        id_value = Some(value.into_owned());
                    } else {
                        type_value = Some(value.into_owned());
                    }
                }
            }
            let name = element.local_name();
            let name = name.as_ref();
            let paired = matches!(
                name,
                b"bookmarkStart"
                    | b"bookmarkEnd"
                    | b"commentRangeStart"
                    | b"commentRangeEnd"
                    | b"permStart"
                    | b"permEnd"
            ) || name == b"proofErr"
                && matches!(
                    type_value.as_deref(),
                    Some("spellStart" | "spellEnd" | "gramStart" | "gramEnd")
                );
            let this_ordinal = paired.then(|| {
                let current = ordinal;
                ordinal += 1;
                current
            });
            let matched = if name == b"commentReference" {
                move_comment_reference
                    && matches!(entry.kind(), StoryRangeKind::Comment { .. })
                    && id_value.as_deref() == Some(identity.as_str())
            } else {
                (name == start_tag || name == end_tag)
                    && this_ordinal.is_some_and(|index| {
                        index == entry.start_ordinal || index == entry.end_ordinal
                    })
                    && if matches!(entry.kind(), StoryRangeKind::Proofing { .. }) {
                        type_value.as_deref() == Some(format!("{identity}Start").as_str())
                            || type_value.as_deref() == Some(format!("{identity}End").as_str())
                    } else {
                        id_value.as_deref() == Some(identity.as_str())
                    }
            };
            if matched {
                let end = if matches!(event, Event::Empty(_)) {
                    after
                } else {
                    story_element_end(xml, before)?
                };
                ranges.push(before..end);
            }
            buffer.clear();
        }
        let expected =
            if move_comment_reference && matches!(entry.kind(), StoryRangeKind::Comment { .. }) {
                3
            } else {
                2
            };
        if ranges.len() != expected {
            return Err(Error::Other(format!(
                "range marker identity {} has {} selected elements, expected {expected}",
                identity,
                ranges.len()
            )));
        }
        Ok((part_name, xml.to_vec(), ranges))
    }

    pub(crate) fn remove_story_range_markers(
        &mut self,
        entry: &crate::comments::StoryRangeRef,
        move_comment_reference: bool,
    ) -> Result<()> {
        let (part_name, mut updated, ranges) =
            self.story_range_marker_spans(entry, move_comment_reference)?;
        for range in ranges.into_iter().rev() {
            updated.splice(range, []);
        }
        set_story_source_xml(self, &part_name, updated)
    }
}

impl Document {
    pub(crate) fn location_is_end(location: &ContentLocation) -> bool {
        location.is_end
    }

    pub(crate) fn owned_story_parts(&self) -> Result<Vec<Vec<u8>>> {
        Ok(self
            .story_sources()?
            .into_iter()
            .map(|source| source.xml.into_owned())
            .collect())
    }

    pub(crate) fn copy_indexed_story_item(
        &self,
        location: &ContentLocation,
        index_path: &[usize],
    ) -> Result<(String, Vec<u8>, std::ops::Range<usize>, bool)> {
        let mut located = location.clone();
        located.index_path = index_path.to_vec();
        located.is_end = false;
        let (source, item) = if located.item_kind == StoryItemKind::Paragraph {
            self.story_range_paragraph_source(&located)?
        } else {
            self.story_item_source(&located)?
        };
        Ok((
            source.part_name.clone(),
            source.xml.into_owned(),
            item.full,
            item.direct_owner_child,
        ))
    }

    pub(crate) fn write_story_part(&mut self, part_name: &str, xml: Vec<u8>) -> Result<()> {
        set_story_source_xml(self, part_name, xml)
    }

    pub(crate) fn story_owner_span(
        &self,
        story: &StoryId,
    ) -> Result<(Vec<u8>, std::ops::Range<usize>)> {
        let (source, owner) = self.story_source_and_owner(story)?;
        Ok((source.xml.into_owned(), owner.full))
    }

    /// Refresh an owner by its complete kind/part/index selector after a
    /// mutation, discovering every story before qualifying the selected span.
    pub(crate) fn refreshed_story_owner_span(
        &self,
        previous: &StoryId,
    ) -> Result<Option<(Vec<u8>, Range<usize>)>> {
        let (sources, content, children) = self.spanned_story_sources()?;
        let main = self
            .main_story_layout(&sources[0].xml, &content, &children, false)
            .ok();
        let borrowed = sources
            .iter()
            .map(|source| StorySource {
                root_kind: source.root_kind,
                part_name: source.part_name.clone(),
                xml: Cow::Borrowed(source.xml.as_ref()),
            })
            .collect();
        let Some(refreshed) =
            Self::stories_in_sources(borrowed, main)?
                .into_iter()
                .find(|candidate| {
                    candidate.kind == previous.kind
                        && candidate.part_name == previous.part_name
                        && candidate.owner_index == previous.owner_index
                })
        else {
            return Ok(None);
        };
        let (source, owner, _) =
            self.story_source_owner_and_layout_in(&refreshed, false, sources, &content, &children)?;
        Ok(Some((source.xml.into_owned(), owner.full)))
    }

    /// Accepted content controls, non-preserved carriers, and text-box owners.
    ///
    /// Ranges come from the story-item scan. Controls are already deduplicated
    /// by part and byte span. Text-box ranges identify legacy fields that
    /// belong after the non-textbox inventory.
    #[allow(clippy::type_complexity)]
    pub(crate) fn accepted_form_regions(
        &self,
    ) -> Result<(
        Vec<(String, std::ops::Range<usize>)>,
        Vec<(String, std::ops::Range<usize>)>,
        Vec<(String, std::ops::Range<usize>)>,
    )> {
        let mut controls = Vec::new();
        let mut carriers = Vec::new();
        let mut text_boxes = Vec::new();
        let mut seen_controls = HashSet::new();
        // Story discovery still names the accepted parts. Offsets come from the
        // package bytes those parts will be patched in. The body model serializes
        // separately and its offsets are not the physical file.
        for source in self.story_sources()? {
            let Some(xml) = self.package.get_part(&source.part_name) else {
                continue;
            };
            let part = source.part_name.clone();
            for owner in scan_story_owners(xml, source.root_kind)? {
                if owner.kind == StoryKind::TextBox {
                    text_boxes.push((part.clone(), owner.full.clone()));
                }
                let items = scan_story_items(xml, &owner, None)?;
                for item in items {
                    if item.kind == StoryItemKind::ContentControl
                        && seen_controls.insert((part.clone(), item.full.start, item.full.end))
                    {
                        controls.push((part.clone(), item.full.clone()));
                    }
                    if owner.kind != StoryKind::TextBox && item.kind != StoryItemKind::PreservedNode
                    {
                        carriers.push((part.clone(), item.full.clone()));
                    }
                }
            }
        }
        Ok((controls, carriers, text_boxes))
    }

    pub(super) fn story_range_paragraph_source<'a>(
        &'a self,
        location: &ContentLocation,
    ) -> Result<(StorySource<'a>, StoryItemSpan)> {
        if location.index_path.len() == 1 {
            return self.story_item_source(location);
        }
        if location.index_path.len() != 2 || location.item_kind != StoryItemKind::Paragraph {
            return Err(StoryError::InvalidPath {
                path: location.index_path.clone(),
            }
            .into());
        }
        let (source, owner, layout) = self.story_source_owner_and_layout(&location.story, true)?;
        let items = story_owner_items(source.xml.as_ref(), &owner, layout.as_ref())?;
        let control_index = location.index_path[0];
        let control = items.get(control_index).ok_or(StoryError::OutOfBounds {
            index: control_index,
            len: items.len(),
        })?;
        if control.kind != StoryItemKind::ContentControl {
            return Err(StoryError::KindMismatch {
                expected: StoryItemKind::ContentControl,
                actual: control.kind,
            }
            .into());
        }
        let paragraphs = story_control_paragraphs(source.xml.as_ref(), control, layout.as_ref())?;
        let paragraph_index = location.index_path[1];
        let full = paragraphs
            .get(paragraph_index)
            .cloned()
            .ok_or(StoryError::OutOfBounds {
                index: paragraph_index,
                len: paragraphs.len(),
            })?;
        Ok((
            source,
            StoryItemSpan {
                kind: StoryItemKind::Paragraph,
                scan: full.clone(),
                full,
                direct_owner_child: false,
                complex_field: false,
                complex_ancestors: Vec::new(),
                sdt_context: None,
            },
        ))
    }
}

impl Document {
    pub(crate) fn patch_story_paragraph_field_sources(
        &mut self,
        location: &ContentLocation,
        replacements: &[(Vec<u8>, Vec<u8>)],
    ) -> Result<()> {
        // Earlier writes may have changed the owning story's fingerprint.
        // Refresh only that fingerprint, keeping its physical part and ordinal.
        let mut location = location.clone();
        location.story = self
            .stories()?
            .into_iter()
            .find(|story| {
                story.kind == location.story.kind
                    && story.part_name == location.story.part_name
                    && story.owner_index == location.story.owner_index
            })
            .ok_or_else(|| {
                Error::Other("field cache owner disappeared during staging".to_owned())
            })?;
        let (source, paragraph) = self.story_range_paragraph_source(&location)?;
        let part_name = source.part_name.clone();
        let mut xml = source.xml.into_owned();
        let mut edits = Vec::new();
        let mut cursor = 0;
        for (original, replacement) in replacements {
            if original.is_empty() {
                return Err(Error::Other("empty field source identity".to_owned()));
            }
            let offset = crate::field::find_typed_field_source(
                &xml,
                paragraph.full.start,
                paragraph.full.end,
                original,
                cursor,
            )?
            .ok_or_else(|| {
                Error::Other("field source no longer belongs to its checked paragraph".to_owned())
            })?;
            let start = paragraph.full.start + offset;
            cursor = offset + original.len();
            edits.push((start..(start + original.len()), replacement));
        }
        for (range, replacement) in edits.into_iter().rev() {
            xml.splice(range, replacement.iter().copied());
        }
        set_story_source_xml(self, &part_name, xml)?;
        self.invalidate_layout();
        Ok(())
    }

    pub(crate) fn text_box_cache_paragraphs(&self) -> Result<Vec<TextBoxCacheParagraph>> {
        let (sources, paragraphs) = self.story_range_paragraph_sources()?;
        let text_boxes = sources
            .iter()
            .map(|source| {
                Ok(scan_story_owners(source.xml.as_ref(), source.root_kind)?
                    .into_iter()
                    .filter(|owner| owner.kind == StoryKind::TextBox)
                    .collect::<Vec<_>>())
            })
            .collect::<Result<Vec<_>>>()?;
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for (source_index, _, paragraph, location, xml) in paragraphs {
            if !matches!(
                location.story.kind,
                StoryKind::TextBox | StoryKind::TableCell
            ) {
                continue;
            }
            let source = &sources[source_index];
            let text_box = text_boxes[source_index]
                .iter()
                .filter(|owner| {
                    owner.full.start <= paragraph.start && owner.full.end >= paragraph.end
                })
                .min_by_key(|owner| owner.full.len());
            if let Some(text_box) = text_box
                && seen.insert((source.part_name.clone(), paragraph.start, paragraph.end))
            {
                result.push(TextBoxCacheParagraph {
                    location,
                    xml,
                    owner_index: text_box.owner_index,
                    paragraph_index: paragraph.start,
                });
            }
        }
        result.sort_by(|left, right| {
            (
                left.location.story.part_name.as_str(),
                left.owner_index,
                left.paragraph_index,
            )
                .cmp(&(
                    right.location.story.part_name.as_str(),
                    right.owner_index,
                    right.paragraph_index,
                ))
        });
        let mut ordinals = HashMap::<(String, usize), usize>::new();
        for paragraph in &mut result {
            let ordinal = ordinals
                .entry((
                    paragraph.location.story.part_name.clone(),
                    paragraph.owner_index,
                ))
                .or_default();
            paragraph.paragraph_index = *ordinal;
            *ordinal += 1;
        }
        Ok(result)
    }

    /// Snapshot one checked range paragraph, including a paragraph within a block control.
    /// This does not expand the ordinary story-item inventory.
    #[doc(hidden)]
    pub fn story_range_paragraph_snapshot(
        &self,
        location: &ContentLocation,
    ) -> Result<StoryItemSnapshot> {
        let (source, item) = self.story_range_paragraph_source(location)?;
        let scope = story_namespace_scope_at(source.xml.as_ref(), item.full.start)?;
        let xml = close_content_fragment_namespaces(&source.xml.as_ref()[item.full], &scope)?;
        let text = CT_P::from_xml_fragment(&xml)?.accepted_text();
        let containing = if let [control, _] = location.index_path() {
            ContentLocation::new(
                location.story().clone(),
                StoryItemKind::ContentControl,
                vec![*control],
            )
        } else {
            location.clone()
        };
        let direct_body_index = StoryItemRef {
            document: self,
            location: containing,
        }
        .direct_body_index()?;
        Ok(StoryItemSnapshot {
            location: location.clone(),
            direct_body_index,
            direct_child: location.index_path().len() == 1,
            text: Some(text),
            xml,
        })
    }

    // Guard the unique newly placed pair against the actual main source after
    // existing namespace replay. Only this unpublished candidate's document part
    // is flushed, leaving pending comment models and identifier state untouched.
    pub(crate) fn comment_literal_range_text(&mut self, id: i32) -> Result<Option<String>> {
        let xml = self.comment_move_source_with_namespaces(
            &self.doc_part_name,
            self.document.to_xml()?,
            id,
            false,
        )?;
        let part = self.doc_part_name.clone();
        set_story_source_xml(self, &part, xml)?;
        self.flush_document_to_package()?;
        let source = self.package.get_part(&self.doc_part_name).ok_or_else(|| {
            Error::Other("comment literal guard has no main document source".into())
        })?;
        Ok(CT_P::comment_range_text_from_source(source, id)?)
    }

    // Keep canonical locations and fingerprints while restoring only the proved
    // namespace context used by rich comment projection. Physical event offsets
    // qualify every owner before a restored declaration can reinterpret a name.
    pub(crate) fn namespace_restored_main_source(&self) -> Result<Vec<u8>> {
        let canonical = self.main_part_with_body_spans()?.0;
        let original = self
            .package
            .get_part(&self.doc_part_name)
            .ok_or_else(|| Error::Other("rich comment projection has no main source".into()))?;
        let mut namespace_owners = nested_modeled_namespace_owners(original)?;
        if namespace_owners.is_empty() {
            return Ok(canonical);
        }
        let canonical_spans = modeled_owner_spans(&canonical)?;
        let mut needed = Vec::new();
        for owner in namespace_owners.drain(..) {
            let original_scope = story_namespace_scope_at(original, owner.source.start)?;
            let original_owner = close_content_fragment_namespaces(
                &original[owner.source.clone()],
                &original_scope,
            )?;
            let mut redundant = 0;
            for target in canonical_spans.iter().filter(|span| {
                span.local_name == owner.local_name
                    && owner
                        .declarations
                        .iter()
                        .all(|declaration| span.declarations.contains(declaration))
            }) {
                let scope = story_namespace_scope_at(&canonical, target.start)?;
                let target_owner = close_content_fragment_namespaces(
                    &canonical[target.start..target.end],
                    &scope,
                )?;
                // Full owner payload, names and attributes prove the unique
                // source counterpart. Only redundant namespace declarations may
                // differ, with every resolved namespace fact still identical.
                if let Some((_, changed)) =
                    comment_namespace_source_boundaries(&original_owner, &target_owner)?
                    && changed.is_empty()
                {
                    redundant += 1;
                }
            }
            if redundant != 1 {
                needed.push(owner);
            }
        }
        let restored = replay_nested_namespace_declarations(&canonical, &needed)?;
        if restored == canonical {
            return Ok(canonical);
        }
        let (boundaries, _) = comment_namespace_source_boundaries(&canonical, &restored)?
            .ok_or_else(|| {
                Error::Other("rich comment namespace replay changed physical source".into())
            })?;
        let owners = scan_story_owners(&canonical, StoryKind::Body)?;
        let restored_owners = scan_story_owners(&restored, StoryKind::Body)?;
        if owners.len() != restored_owners.len()
            || owners.iter().any(|owner| {
                !restored_owners.iter().any(|other| {
                    other.kind == owner.kind
                        && other.owner_index == owner.owner_index
                        && boundaries.get(&owner.full.start) == Some(&other.full.start)
                        && boundaries.get(&owner.full.end) == Some(&other.full.end)
                })
            })
        {
            return Err(Error::Other(
                "rich comment namespace replay changes story ownership".into(),
            ));
        }
        Ok(restored)
    }

    /// Snapshot advisory paragraph values once after checked source traversal.
    pub(crate) fn comment_paragraph_read(&self) -> CommentParagraphRead<'_> {
        let scans = self
            .main_body_scans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let blocks =
            offered_canonical_blocks(scans.serialized.as_ref().or(self.main_part_blocks.as_ref()))
                .map_or(&[][..], |record| record.blocks.as_slice());
        CommentParagraphRead::from_blocks(&self.doc_part_name, blocks)
    }

    /// The checked range paragraphs in story order, each with its flow, as
    /// the source index and flow that `story_range_paragraph_sources` gives,
    /// and its offset in that source, which orders the paragraphs of a flow.
    #[allow(clippy::type_complexity)] // Located paragraphs with their flow and source order.
    pub(crate) fn comment_story_range_paragraphs(
        &self,
    ) -> Result<Vec<((usize, usize), usize, ContentLocation, Vec<u8>)>> {
        let (_, paragraphs) = self.story_range_paragraph_sources()?;
        let mut paragraphs = paragraphs
            .into_iter()
            .map(|(source, flow, span, location, xml)| ((source, flow), span.start, location, xml))
            .collect::<Vec<_>>();
        let canonical = self.document.to_xml()?;
        let restored = self.namespace_restored_main_source()?;
        if restored == canonical {
            return Ok(paragraphs);
        }
        let (boundaries, _) = comment_namespace_source_boundaries(&canonical, &restored)?
            .ok_or_else(|| {
                Error::Other("rich comment namespace replay changed physical source".into())
            })?;
        let owners = scan_story_owners(&canonical, StoryKind::Body)?
            .into_iter()
            .map(|owner| {
                let story = StoryId {
                    kind: owner.kind,
                    part_name: self.doc_part_name.clone(),
                    owner_index: owner.owner_index,
                    fingerprint: owner.fingerprint,
                };
                (story, owner)
            })
            .collect::<Vec<_>>();
        let mut paragraph_spans = HashMap::new();
        for (location, span) in story_range_paragraph_spans(&canonical, &owners)?
            .into_iter()
            .flatten()
        {
            if paragraph_spans.insert(location, span).is_some() {
                return Err(Error::Other(
                    "rich comment has duplicate physical paragraph path".into(),
                ));
            }
        }
        let scopes = story_namespace_scopes_at(
            &restored,
            paragraph_spans
                .values()
                .filter_map(|span| boundaries.get(&span.start).copied()),
        )?;
        for (_, _, location, xml) in &mut paragraphs {
            if location.story.part_name != self.doc_part_name {
                continue;
            }
            let span = paragraph_spans.get(location).ok_or_else(|| {
                Error::Other("rich comment paragraph has no checked physical path".into())
            })?;
            let start = *boundaries.get(&span.start).ok_or_else(|| {
                Error::Other("rich comment paragraph has no physical namespace boundary".into())
            })?;
            let end = *boundaries.get(&span.end).ok_or_else(|| {
                Error::Other("rich comment paragraph has no physical namespace boundary".into())
            })?;
            let scope = scopes.get(&start).ok_or_else(|| {
                Error::Other("rich comment paragraph namespace scope was not inventoried".into())
            })?;
            *xml = close_content_fragment_namespaces(&restored[start..end], scope)?;
        }
        Ok(paragraphs)
    }

    #[doc(hidden)]
    pub fn story_range_paragraphs(&self) -> Result<Vec<(ContentLocation, Vec<u8>)>> {
        Ok(self
            .story_range_located_paragraphs()?
            .into_iter()
            .map(|(_, _, _, location, xml)| (location, xml))
            .collect())
    }

    /// Owned paragraph projection from one source inventory.
    ///
    /// The private source list stays inside this module. Callers receive the
    /// source index, flow, raw span, location and namespace-closed paragraph.
    #[allow(clippy::type_complexity)]
    pub(crate) fn story_range_located_paragraphs(
        &self,
    ) -> Result<Vec<(usize, usize, Range<usize>, ContentLocation, Vec<u8>)>> {
        let (_sources, paragraphs) = self.story_range_paragraph_sources()?;
        Ok(paragraphs)
    }

    /// List every checked range paragraph in story order, with the index of
    /// its source, its flow and its span there, after building each package
    /// source once.
    ///
    /// The flow is the start offset of the paragraph's owner, or for a table
    /// cell that of the innermost other owner holding it, so the paragraphs of
    /// one flow are one story with its table cells, in source order.
    ///
    /// Stories keep the identities and order of [`Self::stories`]. Each source
    /// is scanned for its owners once and its paragraph scopes are inventoried
    /// in one pass.
    #[allow(clippy::type_complexity)] // Sources and their located paragraph spans form one inventory.
    fn story_range_paragraph_sources(
        &self,
    ) -> Result<(
        Vec<StorySource<'_>>,
        Vec<(usize, usize, Range<usize>, ContentLocation, Vec<u8>)>,
    )> {
        let sources = self.story_sources()?;
        let mut paragraphs = Vec::new();
        let mut seen = HashSet::new();
        let mut story_index = 0usize;
        for (source_index, source) in sources.iter().enumerate() {
            let source_xml = source.xml.as_ref();
            let mut owners = Vec::new();
            let mut flows = Vec::new();
            // Owners come in start order, so the open non-cell owners nest.
            let mut holders = Vec::<Range<usize>>::new();
            for owner in scan_story_owners(source_xml, source.root_kind)? {
                while holders
                    .last()
                    .is_some_and(|holder| holder.end <= owner.full.start)
                {
                    holders.pop();
                }
                let flow = match holders.last() {
                    Some(holder) if owner.kind == StoryKind::TableCell => holder.start,
                    _ => owner.full.start,
                };
                if owner.kind != StoryKind::TableCell {
                    holders.push(owner.full.clone());
                }
                let identity = (owner.kind, source.part_name.clone(), owner.owner_index);
                if !seen.insert(identity) {
                    continue;
                }
                let story = StoryId {
                    kind: owner.kind,
                    part_name: source.part_name.clone(),
                    owner_index: owner.owner_index,
                    fingerprint: owner.fingerprint,
                };
                owners.push((story, owner));
                flows.push(flow);
            }
            let spans = story_range_paragraph_spans(source_xml, &owners)?;
            let scopes = story_namespace_scopes_at(
                source_xml,
                spans.iter().flatten().map(|(_, span)| span.start),
            )?;
            for (story_spans, flow) in spans.into_iter().zip(flows) {
                for (location, span) in story_spans {
                    let scope = scopes.get(&span.start).ok_or_else(|| {
                        Error::Other("story paragraph namespace scope was not inventoried".into())
                    })?;
                    let xml = close_content_fragment_namespaces(&source_xml[span.clone()], scope)?;
                    paragraphs.push((story_index, source_index, flow, span, location, xml));
                }
                story_index += 1;
            }
        }
        paragraphs.sort_by_key(|(story_index, _, _, span, _, _)| (*story_index, span.start));
        Ok((
            sources,
            paragraphs
                .into_iter()
                .map(|(_, source_index, flow, span, location, xml)| {
                    (source_index, flow, span, location, xml)
                })
                .collect(),
        ))
    }
}
