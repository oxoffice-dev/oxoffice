//! Tables of contents: their bookmarks, sources, entries and rebuild.

use super::*;

impl Document {
    /// Rebuild every supported table of contents already present in the document.
    ///
    /// The operation stages bookmarks, cached entries, and deterministic page
    /// targets on an independent document. Any malformed or ambiguous source
    /// leaves the receiver unchanged.
    ///
    /// The cached entry paragraphs are replaced, so comment and bookmark
    /// markers placed on them are dropped with them. A comment anchored only
    /// there stays in the comments part without an anchor.
    pub fn rebuild_toc(&mut self) -> Result<TocRebuildReport> {
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_package()?;
        let document_xml = candidate
            .package
            .get_part(&candidate.doc_part_name)
            .ok_or(Error::NoDocumentPart)?
            .to_vec();
        let toc_spans = scan_dynamic_table_spans(&document_xml, DynamicOwnerPolicy::Toc)?;
        let mut diagnostics = collect_simple_toc_diagnostics(&document_xml)?;
        if toc_spans.is_empty() {
            return Ok(TocRebuildReport {
                diagnostics: diagnostics
                    .into_iter()
                    .map(|(_, diagnostic)| diagnostic)
                    .collect(),
                ..Default::default()
            });
        }

        let (toc_fields, mut dynamic_diagnostics) =
            parse_dynamic_toc_fields(&candidate, &document_xml, &toc_spans)?;
        diagnostics.append(&mut dynamic_diagnostics);
        diagnostics.sort_by_key(|(offset, _)| *offset);
        let mut diagnostics = diagnostics
            .into_iter()
            .map(|(_, diagnostic)| diagnostic)
            .collect::<Vec<_>>();
        if toc_fields.iter().all(Option::is_none) {
            return Ok(TocRebuildReport {
                diagnostics,
                ..Default::default()
            });
        }
        // Layout and dynamic-instruction parsing prepare their own clones.
        // Source discovery below consumes this actual staged body instead.
        crate::document::prepare_semantic_body_projection(&mut candidate.document.body.content)?;
        let bookmark_state = inspect_toc_bookmarks(&candidate.document.body, &document_xml)?;
        let numbering_layout = candidate.layout_deterministic()?;
        let sources = discover_toc_sources(
            &candidate,
            &toc_spans,
            &toc_fields,
            &bookmark_state,
            &numbering_layout,
        )?;
        diagnostics.extend(toc_duplicate_style_diagnostics(&candidate.styles));
        diagnostics.extend(toc_default_style_diagnostics(&candidate.styles));
        let (toc_entry_styles, retained_style_defects) =
            ensure_toc_entry_styles(&mut candidate, &sources)?;
        let entry_style_ids = toc_entry_style_ids(&candidate.styles, &toc_entry_styles);
        diagnostics.extend(retained_style_defects);
        candidate.flush_to_package()?;
        let rebuilt_toc_spans = toc_spans
            .iter()
            .zip(&toc_fields)
            .filter_map(|(span, field)| field.is_some().then_some(span.clone()))
            .collect::<Vec<_>>();
        let bookmark_repairs =
            toc_crossing_bookmark_repairs(&bookmark_state, &toc_spans, &toc_fields);
        let authored_bookmark_names = candidate.identifiers.authored_bookmark_names().clone();
        let mut allocator = TocBookmarkAllocator::new(bookmark_state, &authored_bookmark_names);
        let mut bookmark_by_paragraph = BTreeMap::<usize, TocBookmark>::new();
        let mut bookmark_name_remap = BTreeMap::<String, String>::new();
        for source in sources.iter().flatten() {
            if !source.needs_bookmark {
                continue;
            }
            if bookmark_by_paragraph.contains_key(&source.paragraph_index) {
                continue;
            }
            let is_partial_boundary_source = rebuilt_toc_spans.iter().any(|span| {
                source.paragraph_index == span.begin_paragraph
                    || source.paragraph_index == span.end_paragraph
            });
            if !is_partial_boundary_source
                && let Some(existing) = allocator.whole_paragraph_name(source.paragraph_index)
            {
                let authored = candidate.identifiers.is_authored_bookmark(existing.0);
                let name = if authored {
                    let name = allocator.allocate_name()?;
                    if name != existing.1 {
                        bookmark_name_remap.insert(existing.1, name.clone());
                    }
                    name
                } else {
                    existing.1
                };
                bookmark_by_paragraph.insert(
                    source.paragraph_index,
                    TocBookmark {
                        id: existing.0,
                        name,
                        insert: false,
                        authored,
                    },
                );
            } else {
                let allocated = allocator.allocate(&mut candidate.identifiers)?;
                bookmark_by_paragraph.insert(
                    source.paragraph_index,
                    TocBookmark {
                        id: allocated.0,
                        name: allocated.1,
                        insert: true,
                        authored: true,
                    },
                );
            }
        }
        if !bookmark_name_remap.is_empty() {
            let remap = BodyIdentityRemap {
                bookmark_names: bookmark_name_remap,
                ..Default::default()
            };
            let renamed_xml = patch_body_identity_attributes(&document_xml, &remap)?;
            CT_Document::from_xml(&renamed_xml)?;
            candidate
                .package
                .set_part(&candidate.doc_part_name, renamed_xml);
            let mut renamed = reopen_staged_document(candidate)?;
            restore_toc_bookmark_provenance(&mut renamed, &bookmark_by_paragraph);
            let report = renamed.rebuild_toc()?;
            self.commit_staged_mutation(renamed);
            return Ok(report);
        }
        let bookmark_count = bookmark_by_paragraph
            .values()
            .filter(|bookmark| bookmark.insert)
            .count();
        let provisional_xml = insert_toc_bookmarks_xml(
            &document_xml,
            &bookmark_by_paragraph,
            &bookmark_repairs,
            &toc_spans,
            &rebuilt_toc_spans,
        )?;
        CT_Document::from_xml(&provisional_xml)?;
        candidate
            .package
            .set_part(&candidate.doc_part_name, provisional_xml.clone());
        candidate = reopen_staged_document(candidate)?;
        let provisional_spans =
            scan_dynamic_table_spans(&provisional_xml, DynamicOwnerPolicy::Toc)?;
        if provisional_spans.len() != toc_fields.len() {
            return Err(Error::Other(
                "table of contents ownership changed while staging bookmarks".to_owned(),
            ));
        }
        let end_bookmark_starts = provisional_spans
            .iter()
            .enumerate()
            .map(|(toc_index, span)| {
                end_boundary_bookmark_starts(
                    toc_index,
                    span,
                    &provisional_spans,
                    &toc_fields,
                    &bookmark_by_paragraph,
                    &bookmark_repairs,
                )
            })
            .collect::<Vec<_>>();
        let provisional_paragraphs = toc_paragraph_insertions(&provisional_xml)?;
        let mut placeholders = Vec::new();
        let mut edits = Vec::new();
        let mut entry_count = 0usize;
        for (toc_index, ((span, toc), toc_sources)) in provisional_spans
            .iter()
            .zip(&toc_fields)
            .zip(&sources)
            .enumerate()
        {
            let Some(toc) = toc else {
                continue;
            };
            let generated = render_toc_entries(
                toc_index,
                toc,
                toc_sources,
                &toc_entry_styles,
                toc_section_text_width(&candidate.document.body, span.begin_paragraph),
                &bookmark_by_paragraph,
                &provisional_xml,
                &mut placeholders,
            )?;
            entry_count += toc_sources.len();
            if let Some(mut merged) = entry_boundary_toc_edits(
                &provisional_xml,
                span,
                &provisional_paragraphs,
                &generated,
                &entry_style_ids,
                &end_bookmark_starts[toc_index],
            )? {
                edits.append(&mut merged);
                continue;
            }
            if span.begin_paragraph == span.end_paragraph {
                return Err(Error::Other(
                    "table of contents result must span paragraph boundaries".to_owned(),
                ));
            }
            edits.push(FieldSourceEdit {
                start: span.result_start,
                end: span.result_end,
                replacement: dynamic_toc_replacement(
                    &provisional_xml,
                    span,
                    &generated,
                    &end_bookmark_starts[toc_index],
                )?,
            });
        }
        let mut staged_xml = provisional_xml;
        for edit in edits.into_iter().rev() {
            staged_xml.splice(edit.start..edit.end, edit.replacement);
        }
        CT_Document::from_xml(&staged_xml)?;
        candidate
            .package
            .set_part(&candidate.doc_part_name, staged_xml);
        let mut provisional = reopen_staged_document(candidate)?;

        let page_values = deterministic_toc_page_values(&provisional)?;
        let current_xml = provisional
            .package
            .get_part(&provisional.doc_part_name)
            .ok_or(Error::NoDocumentPart)?;
        let mut final_xml = current_xml.to_vec();
        let final_spans = scan_dynamic_table_spans(&final_xml, DynamicOwnerPolicy::Toc)?;
        if final_spans.len() != toc_fields.len() {
            return Err(Error::Other(
                "table of contents ownership changed during page substitution".to_owned(),
            ));
        }
        let mut page_edits = Vec::with_capacity(placeholders.len());
        for placeholder in &placeholders {
            let target = page_values.get(&placeholder.bookmark).ok_or_else(|| {
                Error::Other(format!(
                    "table of contents page target {} was not resolved",
                    placeholder.bookmark
                ))
            })?;
            let span = &final_spans[placeholder.toc_index];
            let owned = &final_xml[span.result_start..span.result_end];
            let matches = byte_match_offsets(owned, placeholder.token.as_bytes());
            if matches.len() != 1 {
                return Err(Error::Other(format!(
                    "table of contents page placeholder {} was not uniquely owned",
                    placeholder.token
                )));
            }
            let start = span.result_start + matches[0];
            page_edits.push(FieldSourceEdit {
                start,
                end: start + placeholder.token.len(),
                replacement: target.as_bytes().to_vec(),
            });
        }
        page_edits.sort_by_key(|edit| edit.start);
        for edit in page_edits.into_iter().rev() {
            final_xml.splice(edit.start..edit.end, edit.replacement);
        }
        let final_spans = scan_dynamic_table_spans(&final_xml, DynamicOwnerPolicy::Toc)?;
        final_xml =
            relocate_end_boundary_bookmark_starts(final_xml, &final_spans, &end_bookmark_starts)?;
        CT_Document::from_xml(&final_xml)?;
        provisional
            .package
            .set_part(&provisional.doc_part_name, final_xml);
        let mut completed = reopen_staged_document(provisional)?;
        restore_toc_bookmark_provenance(&mut completed, &bookmark_by_paragraph);
        completed.invalidate_layout();
        self.commit_staged_mutation(completed);

        Ok(TocRebuildReport {
            entry_count,
            bookmark_count,
            diagnostics,
        })
    }
}

fn restore_toc_bookmark_provenance(
    document: &mut Document,
    bookmarks: &BTreeMap<usize, TocBookmark>,
) {
    for bookmark in bookmarks.values().filter(|bookmark| bookmark.authored) {
        document
            .identifiers
            .restore_authored_bookmark(bookmark.id, &bookmark.name);
    }
}

#[derive(Debug, Clone)]
struct TocBookmark {
    id: i32,
    name: String,
    insert: bool,
    authored: bool,
}

#[derive(Debug)]
struct TocBookmarkState {
    max_id: i32,
    names: HashSet<String>,
    named_ranges: HashMap<String, (TocDocumentPosition, TocDocumentPosition)>,
    whole_paragraphs: HashMap<usize, (i32, String)>,
    ranges: Vec<TocBookmarkRange>,
}

#[derive(Debug, Clone)]
struct TocBookmarkRange {
    id: i32,
    name: String,
    start_offset: usize,
    end_offset: usize,
}

#[derive(Debug)]
struct TocBookmarkRepair {
    id: i32,
    name: String,
    toc_index: usize,
    replace_start: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum TocRawOrder {
    BeforeRaw,
    Raw(usize),
    AfterRaw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct TocDocumentPosition {
    paragraph: usize,
    accepted_run_index: usize,
}

fn inspect_toc_bookmarks(body: &CT_Body, xml: &[u8]) -> Result<TocBookmarkState> {
    let whole_bookmarks = whole_paragraph_bookmark_ids(xml)?;
    let marker_offsets = toc_bookmark_marker_offsets(xml)?;
    let mut paragraphs = Vec::new();
    collect_body_paragraphs(body, &mut paragraphs, None);
    let mut starts = HashMap::<i32, (String, usize, usize)>::new();
    let mut start_order = Vec::new();
    let mut ends = HashMap::<i32, (usize, usize)>::new();
    let mut names = HashSet::new();
    let mut max_id = 0i32;
    for (paragraph_index, paragraph) in paragraphs.iter().enumerate() {
        for marker in paragraph.current_bookmark_projection() {
            let Some(id) = marker.id() else {
                return Err(Error::Other(
                    "table of contents source has a bookmark without an ID".to_owned(),
                ));
            };
            max_id = max_id.max(id);
            if marker.is_start() {
                let name = marker.name().ok_or_else(|| {
                    Error::Other(
                        "table of contents source has a bookmark without a name".to_owned(),
                    )
                })?;
                if !names.insert(name.to_owned()) || starts.contains_key(&id) {
                    return Err(Error::Other(
                        "table of contents source has ambiguous duplicate bookmarks".to_owned(),
                    ));
                }
                starts.insert(
                    id,
                    (
                        name.to_owned(),
                        paragraph_index,
                        marker.projected_run_index(),
                    ),
                );
                start_order.push(id);
            } else if ends
                .insert(id, (paragraph_index, marker.projected_run_index()))
                .is_some()
            {
                return Err(Error::Other(
                    "table of contents source has ambiguous duplicate bookmarks".to_owned(),
                ));
            }
        }
    }
    if starts.len() != ends.len() || starts.keys().any(|id| !ends.contains_key(id)) {
        return Err(Error::Other(
            "table of contents source has an unmatched bookmark".to_owned(),
        ));
    }
    let mut named_ranges = HashMap::new();
    let mut whole_paragraphs = HashMap::new();
    let mut ranges = Vec::new();
    for id in start_order {
        let (name, start_paragraph, start_run) = starts
            .remove(&id)
            .expect("bookmark start order contains each validated start once");
        let (end_paragraph, end_run) = ends[&id];
        let start_offset = marker_offsets.get(&(id, true)).copied().ok_or_else(|| {
            Error::Other("table of contents bookmark start offset was not retained".to_owned())
        })?;
        let end_offset = marker_offsets.get(&(id, false)).copied().ok_or_else(|| {
            Error::Other("table of contents bookmark end offset was not retained".to_owned())
        })?;
        if start_offset > end_offset {
            return Err(Error::Other(
                "table of contents source has a reversed bookmark".to_owned(),
            ));
        }
        named_ranges.insert(
            name.clone(),
            (
                TocDocumentPosition {
                    paragraph: start_paragraph,
                    accepted_run_index: start_run,
                },
                TocDocumentPosition {
                    paragraph: end_paragraph,
                    accepted_run_index: end_run,
                },
            ),
        );
        ranges.push(TocBookmarkRange {
            id,
            name: name.clone(),
            start_offset,
            end_offset,
        });
        if start_paragraph == end_paragraph && whole_bookmarks.contains(&id) {
            whole_paragraphs
                .entry(start_paragraph)
                .or_insert((id, name));
        }
    }
    Ok(TocBookmarkState {
        max_id,
        names,
        named_ranges,
        whole_paragraphs,
        ranges,
    })
}

fn toc_crossing_bookmark_repairs(
    state: &TocBookmarkState,
    spans: &[DynamicTocSpan],
    fields: &[Option<TocField>],
) -> Vec<TocBookmarkRepair> {
    state
        .ranges
        .iter()
        .filter_map(|range| {
            let containing_span = |offset: usize| {
                spans
                    .iter()
                    .zip(fields)
                    .enumerate()
                    .find(|(_, (span, field))| {
                        field.is_some() && offset >= span.result_start && offset < span.result_end
                    })
                    .map(|(index, _)| index)
            };
            let start_span = containing_span(range.start_offset);
            let end_span = containing_span(range.end_offset);
            match (start_span, end_span) {
                (Some(toc_index), None) => Some(TocBookmarkRepair {
                    id: range.id,
                    name: range.name.clone(),
                    toc_index,
                    replace_start: true,
                }),
                (None, Some(toc_index)) => Some(TocBookmarkRepair {
                    id: range.id,
                    name: range.name.clone(),
                    toc_index,
                    replace_start: false,
                }),
                _ => None,
            }
        })
        .collect()
}

#[derive(Clone, Copy)]
enum TocParagraphToken {
    Content,
    BookmarkStart(i32),
    BookmarkEnd(i32),
}

fn toc_bookmark_marker_offsets(xml: &[u8]) -> Result<HashMap<(i32, bool), usize>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut elements = Vec::<DynamicXmlElement>::new();
    let mut offsets = HashMap::new();
    let mut paragraph_count = 0usize;
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid table of contents XML: {error}")))?;
        let word = namespace_is_word(&namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let local = local_name(element.name().as_ref()).to_vec();
                invalidate_overdeep_revision_owner(word, &local, &mut elements);
                let (namespace_bindings, inherited_namespaces) =
                    dynamic_namespace_bindings(&element, &elements)?;
                let mut typed_block_owner = classify_typed_block_owner(word, &local, &elements);
                let is_typed_paragraph = typed_block_owner == Some(TypedBlockOwner::Paragraph);
                let paragraph = if is_typed_paragraph {
                    let paragraph = paragraph_count;
                    paragraph_count += 1;
                    Some(paragraph)
                } else {
                    elements.last().and_then(|element| element.paragraph)
                };
                let mut is_typed_inline_owner = typed_inline_owner(
                    &element,
                    reader.resolver(),
                    word,
                    &local,
                    &elements,
                    paragraph,
                )?;
                validate_dynamic_content_control(
                    xml,
                    before,
                    word,
                    &local,
                    &namespace_bindings,
                    &mut typed_block_owner,
                    &mut is_typed_inline_owner,
                )?;
                if word
                    && matches!(local.as_slice(), b"bookmarkStart" | b"bookmarkEnd")
                    && accepted_bookmark_parent(&elements, paragraph)
                    && let Some((_, id)) = resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"id",
                        AttributeNamespace::Word,
                    )?
                    && let Ok(id) = id.parse::<i32>()
                    && offsets
                        .insert((id, local == b"bookmarkStart"), before)
                        .is_some()
                {
                    return Err(Error::Other(
                        "table of contents source has ambiguous duplicate bookmarks".to_owned(),
                    ));
                }
                let revision_depth = if is_typed_inline_owner {
                    elements.last().map_or(0, |parent| {
                        parent.revision_depth
                            + usize::from(matches!(local.as_slice(), b"ins" | b"moveTo"))
                    })
                } else {
                    0
                };
                mark_typed_sdt_content(
                    &mut elements,
                    &local,
                    typed_block_owner,
                    is_typed_inline_owner,
                );
                elements.push(DynamicXmlElement {
                    local_name: local,
                    qualified_name: String::from_utf8_lossy(element.name().as_ref()).into_owned(),
                    typed_block_owner,
                    is_word: word,
                    is_typed_paragraph,
                    is_typed_inline_owner,
                    revision_depth,
                    sdt_content_seen: false,
                    namespace_bindings,
                    inherited_namespaces,
                    run_position: None,
                    hyperlink_plan: None,
                    start: before,
                    start_tag_end: after,
                    paragraph,
                });
            }
            Event::Empty(element) => {
                let local = local_name(element.name().as_ref()).to_vec();
                // Empty content is a retained raw slot, not the typed Start slot.
                let word = word && &local[..] != b"sdtContent";
                invalidate_overdeep_revision_owner(word, &local, &mut elements);
                let paragraph = elements.last().and_then(|element| element.paragraph);
                let typed_block_owner = classify_typed_block_owner(word, &local, &elements);
                let is_typed_inline_owner = typed_inline_owner(
                    &element,
                    reader.resolver(),
                    word,
                    &local,
                    &elements,
                    paragraph,
                )?;
                if word
                    && matches!(local.as_slice(), b"bookmarkStart" | b"bookmarkEnd")
                    && accepted_bookmark_parent(&elements, paragraph)
                    && let Some((_, id)) = resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"id",
                        AttributeNamespace::Word,
                    )?
                    && let Ok(id) = id.parse::<i32>()
                    && offsets
                        .insert((id, local == b"bookmarkStart"), before)
                        .is_some()
                {
                    return Err(Error::Other(
                        "table of contents source has ambiguous duplicate bookmarks".to_owned(),
                    ));
                }
                mark_typed_sdt_content(
                    &mut elements,
                    &local,
                    typed_block_owner,
                    is_typed_inline_owner,
                );
            }
            Event::End(_) => {
                elements.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(offsets)
}

fn whole_paragraph_bookmark_ids(xml: &[u8]) -> Result<HashSet<i32>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut elements = Vec::<DynamicXmlElement>::new();
    let mut paragraphs = Vec::<Vec<TocParagraphToken>>::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid table of contents XML: {error}")))?;
        let word = namespace_is_word(&namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let local = local_name(element.name().as_ref()).to_vec();
                let (namespace_bindings, inherited_namespaces) =
                    dynamic_namespace_bindings(&element, &elements)?;
                let mut typed_block_owner = classify_typed_block_owner(word, &local, &elements);
                let mut is_typed_inline_owner = false;
                validate_dynamic_content_control(
                    xml,
                    before,
                    word,
                    &local,
                    &namespace_bindings,
                    &mut typed_block_owner,
                    &mut is_typed_inline_owner,
                )?;
                let is_typed_paragraph = typed_block_owner == Some(TypedBlockOwner::Paragraph);
                let paragraph = if is_typed_paragraph {
                    paragraphs.push(Vec::new());
                    Some(paragraphs.len() - 1)
                } else {
                    elements.last().and_then(|element| element.paragraph)
                };
                if let Some(index) = paragraph
                    && elements
                        .last()
                        .is_some_and(|parent| parent.is_typed_paragraph)
                    && local != b"pPr"
                {
                    paragraphs[index].push(paragraph_token(
                        &element,
                        reader.resolver(),
                        word,
                        &local,
                    )?);
                }
                mark_typed_sdt_content(&mut elements, &local, typed_block_owner, false);
                elements.push(DynamicXmlElement {
                    local_name: local,
                    qualified_name: String::from_utf8_lossy(element.name().as_ref()).into_owned(),
                    typed_block_owner,
                    is_word: word,
                    is_typed_paragraph,
                    is_typed_inline_owner: false,
                    revision_depth: 0,
                    sdt_content_seen: false,
                    namespace_bindings,
                    inherited_namespaces,
                    run_position: None,
                    hyperlink_plan: None,
                    start: before,
                    start_tag_end: after,
                    paragraph,
                });
            }
            Event::Empty(element) => {
                let name = element.name();
                let local = local_name(name.as_ref());
                // Empty content is a retained raw slot, not the typed Start slot.
                let word = word && local != b"sdtContent";
                let typed_block_owner = classify_typed_block_owner(word, local, &elements);
                mark_typed_sdt_content(&mut elements, local, typed_block_owner, false);
                if let Some(parent) = elements.last()
                    && parent.is_typed_paragraph
                    && local != b"pPr"
                    && let Some(index) = parent.paragraph
                {
                    paragraphs[index].push(paragraph_token(
                        &element,
                        reader.resolver(),
                        word,
                        local,
                    )?);
                }
            }
            Event::End(_) => {
                elements.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    let mut whole = HashSet::new();
    for tokens in paragraphs {
        let content = tokens
            .iter()
            .enumerate()
            .filter_map(|(index, token)| {
                matches!(token, TocParagraphToken::Content).then_some(index)
            })
            .collect::<Vec<_>>();
        let mut starts = HashMap::new();
        let mut ends = HashMap::new();
        for (index, token) in tokens.iter().enumerate() {
            match token {
                TocParagraphToken::BookmarkStart(id) => {
                    starts.insert(*id, index);
                }
                TocParagraphToken::BookmarkEnd(id) => {
                    ends.insert(*id, index);
                }
                TocParagraphToken::Content => {}
            }
        }
        for (id, start) in starts {
            let Some(end) = ends.get(&id).copied() else {
                continue;
            };
            if content
                .iter()
                .all(|position| *position > start && *position < end)
            {
                whole.insert(id);
            }
        }
    }
    Ok(whole)
}

fn paragraph_token(
    element: &BytesStart<'_>,
    resolver: &NamespaceResolver,
    word: bool,
    local: &[u8],
) -> Result<TocParagraphToken> {
    if word && matches!(local, b"bookmarkStart" | b"bookmarkEnd") {
        let id = resolved_element_attribute(element, resolver, b"id", AttributeNamespace::Word)?
            .and_then(|(_, id)| id.parse::<i32>().ok());
        if let Some(id) = id {
            return Ok(if local == b"bookmarkStart" {
                TocParagraphToken::BookmarkStart(id)
            } else {
                TocParagraphToken::BookmarkEnd(id)
            });
        }
    }
    Ok(TocParagraphToken::Content)
}

struct TocBookmarkAllocator {
    next_id: Option<i32>,
    next_name: Option<u32>,
    names: HashSet<String>,
    whole_paragraphs: HashMap<usize, (i32, String)>,
}

impl TocBookmarkAllocator {
    fn new(state: TocBookmarkState, authored_names: &HashSet<String>) -> Self {
        let mut names = state.names;
        names.retain(|name| !authored_names.contains(name));
        Self {
            next_id: state.max_id.checked_add(1),
            next_name: Some(1),
            names,
            whole_paragraphs: state.whole_paragraphs,
        }
    }

    fn whole_paragraph_name(&self, paragraph: usize) -> Option<(i32, String)> {
        self.whole_paragraphs.get(&paragraph).cloned()
    }

    fn allocate(&mut self, identifiers: &mut DocumentIdentifiers) -> Result<(i32, String)> {
        let preferred = self.next_id.ok_or_else(|| {
            Error::Other("table of contents exhausted the bookmark ID range".to_owned())
        })?;
        let id = identifiers.reserve_preferred_bookmark_id(preferred)?;
        let name = self.allocate_name()?;
        self.next_id = id.checked_add(1);
        Ok((id, name))
    }

    fn allocate_name(&mut self) -> Result<String> {
        loop {
            let suffix = self.next_name.ok_or_else(|| {
                Error::Other("table of contents exhausted the bookmark name range".to_owned())
            })?;
            let name = format!("_Toc{suffix}");
            self.next_name = suffix.checked_add(1);
            if self.names.insert(name.clone()) {
                return Ok(name);
            }
        }
    }
}

#[derive(Debug, Clone)]
struct TocSource {
    paragraph_index: usize,
    level: u8,
    title: String,
    omit_page_number: bool,
    needs_bookmark: bool,
    sequence_prefix: Option<String>,
    numbering_prefix: Option<TocNumberingPrefix>,
}

#[derive(Debug, Clone)]
struct TocNumberingPrefix {
    marker: String,
    suffix: ST_LvlSuffix,
}

#[derive(Debug, Clone)]
struct TocEntryStyle {
    style_id: String,
    has_right_tab: bool,
}

fn discover_toc_sources(
    document: &Document,
    spans: &[DynamicTocSpan],
    fields: &[Option<TocField>],
    bookmarks: &TocBookmarkState,
    numbering_layout: &Arc<oxoffice_docx_layout::WordLayoutResult>,
) -> Result<Vec<Vec<TocSource>>> {
    let mut paragraphs = Vec::new();
    collect_body_paragraphs(&document.document.body, &mut paragraphs, None);
    // Direct body paragraphs whose deleted mark joins them to the next one.
    let body = &document.document.body;
    let joining = (0..body.content.len())
        .filter(|index| body.accepted_paragraph_joins_next(*index))
        .filter_map(|index| match &body.content[index] {
            BodyContent::Paragraph(paragraph) => {
                Some(std::ptr::from_ref(paragraph.as_ref()).addr())
            }
            _ => None,
        })
        .collect::<HashSet<_>>();
    let context = FieldEvaluationContext::default();
    let sequence_snapshot = Arc::new(
        oxoffice_docx_layout::engine::evaluate_sequence_fields(&document.build_layout_input()?)
            .map_err(|error| {
                Error::Other(format!("TOC sequence source evaluation failed: {error}"))
            })?,
    );
    let mut sequence_nodes = Vec::new();
    for index in 1u32.. {
        let Some(node) = oxoffice_docx_layout::SourceNodeId::new(index) else {
            break;
        };
        let Some(path) = sequence_snapshot.source_node(node) else {
            break;
        };
        if matches!(path.story, oxoffice_docx_layout::WordStory::Document) {
            sequence_nodes.push(node);
        }
    }
    if sequence_nodes.len() != paragraphs.len() {
        return Err(Error::Other(
            "TOC sequence paragraph source inventory is incomplete".into(),
        ));
    }
    let mut all_sources = Vec::with_capacity(fields.len());
    for toc in fields {
        let Some(toc) = toc else {
            all_sources.push(Vec::new());
            continue;
        };
        let bookmark_range = toc
            .bookmark
            .as_ref()
            .map(|name| {
                bookmarks.named_ranges.get(name).copied().ok_or_else(|| {
                    Error::Other(format!(
                        "table of contents source bookmark {name} was not found"
                    ))
                })
            })
            .transpose()?;
        let mut sources = Vec::new();
        let mut evaluator = Evaluator::new(document, &context);
        evaluator.bind_numbering_layout(Arc::clone(numbering_layout));
        evaluator.sequence_snapshot = Some(Arc::clone(&sequence_snapshot));
        let mut sequence_value = None;
        // Accepting a deleted paragraph mark joins the paragraph to the next
        // one, which keeps the next paragraph's properties: the joining text
        // opens that paragraph's entry.
        let mut joined_title = String::new();
        for (paragraph_index, paragraph) in paragraphs.iter().enumerate() {
            let joins_next = joining.contains(&std::ptr::from_ref(*paragraph).addr());
            let joined_prefix = if joins_next {
                String::new()
            } else {
                std::mem::take(&mut joined_title)
            };
            let paragraph_fully_owned = spans.iter().any(|span| {
                paragraph_index > span.begin_paragraph && paragraph_index < span.end_paragraph
            });
            if paragraph_fully_owned {
                continue;
            }
            if evaluator
                .numbering_source(sequence_nodes[paragraph_index])
                .and_then(|node| numbering_layout.paragraph_omitted_by_table(node))
                == Some(true)
            {
                continue;
            }
            evaluator.register_paragraph_fields(paragraph, Some(sequence_nodes[paragraph_index]));
            let accepted_runs = accepted_toc_runs(paragraph);
            for (accepted_run_index, run) in accepted_runs.iter().enumerate() {
                let position = TocDocumentPosition {
                    paragraph: paragraph_index,
                    accepted_run_index,
                };
                let in_bookmark =
                    bookmark_range.is_none_or(|(start, end)| position >= start && position < end);
                for content in &run.run.content {
                    let RunContent::Field(field) = content else {
                        continue;
                    };
                    let owned_position = TocOwnedPosition {
                        paragraph: paragraph_index,
                        run: TocRunPosition {
                            run_boundary: run.run_boundary,
                            raw_order: run.raw_order,
                            nested_order: run.nested_order,
                        },
                    };
                    if toc_source_position_is_owned(spans, owned_position) {
                        continue;
                    }
                    let outcome =
                        evaluator.evaluate_field(field, "main", &paragraphs, paragraph_index, None);
                    if field.instruction.name == "SEQ"
                        && let Some(identifier) = field.instruction.arguments.first()
                        && field_argument_text(identifier).is_some_and(|name| {
                            toc.sequence_identifier
                                .as_deref()
                                .is_some_and(|selected| selected.eq_ignore_ascii_case(name))
                        })
                        && let FieldOutcome::Resolved(ref value) = outcome
                    {
                        sequence_value = Some(value.clone());
                    }
                    if !in_bookmark {
                        continue;
                    }
                    let FieldOutcome::TableOfContentsEntry(tc) = outcome else {
                        continue;
                    };
                    if toc_accepts_tc(toc, &tc) {
                        let omit_page_number = tc.omit_page_number
                            || toc
                                .omit_page_number_levels
                                .is_some_and(|(start, end)| (start..=end).contains(&tc.level));
                        sources.push(TocSource {
                            paragraph_index,
                            level: tc.level,
                            title: tc.entry,
                            omit_page_number,
                            needs_bookmark: toc.hyperlink || !omit_page_number,
                            sequence_prefix: sequence_value.clone(),
                            numbering_prefix: toc_numbering_prefix(
                                numbering_layout,
                                paragraph_index,
                            ),
                        });
                    }
                }
            }
            let paragraph_in_bookmark = bookmark_range.is_none_or(|(start, end)| {
                accepted_runs
                    .iter()
                    .enumerate()
                    .any(|(accepted_run_index, run)| {
                        let position = TocDocumentPosition {
                            paragraph: paragraph_index,
                            accepted_run_index,
                        };
                        let owned_position = TocOwnedPosition {
                            paragraph: paragraph_index,
                            run: TocRunPosition {
                                run_boundary: run.run_boundary,
                                raw_order: run.raw_order,
                                nested_order: run.nested_order,
                            },
                        };
                        !toc_source_position_is_owned(spans, owned_position)
                            && position >= start
                            && position < end
                    })
            });
            if !paragraph_in_bookmark {
                continue;
            }
            let kept = accepted_runs
                .iter()
                .filter(|run| {
                    !toc_source_position_is_owned(
                        spans,
                        TocOwnedPosition {
                            paragraph: paragraph_index,
                            run: TocRunPosition {
                                run_boundary: run.run_boundary,
                                raw_order: run.raw_order,
                                nested_order: run.nested_order,
                            },
                        },
                    )
                })
                .collect::<Vec<_>>();
            // A heading the table does not own in part reads as a reader
            // sees it, simple field results included; the run axis has no
            // run for a simple field kept as written.
            let title = if kept.len() == accepted_runs.len() {
                paragraph.accepted_text()
            } else {
                kept.iter()
                    .map(|run| run.run.accepted_text())
                    .collect::<String>()
            };
            if joins_next {
                joined_title.push_str(&title);
                continue;
            }
            let title = joined_prefix + &title;
            if title.is_empty() {
                continue;
            }
            let effective_heading = evaluator
                .numbering_source(sequence_nodes[paragraph_index])
                .and_then(|node| numbering_layout.paragraph_navigation_source(node))
                .map(|(_, level, ..)| level);
            let Some(level) = toc_paragraph_level(document, toc, paragraph, effective_heading)?
            else {
                continue;
            };
            let omit_page_number = toc
                .omit_page_number_levels
                .is_some_and(|(start, end)| (start..=end).contains(&level));
            sources.push(TocSource {
                paragraph_index,
                level,
                title,
                omit_page_number,
                needs_bookmark: toc.hyperlink || !omit_page_number,
                sequence_prefix: sequence_value.clone(),
                numbering_prefix: toc_numbering_prefix(numbering_layout, paragraph_index),
            });
        }
        sources.sort_by_key(|source| source.paragraph_index);
        all_sources.push(sources);
    }
    Ok(all_sources)
}

fn toc_numbering_prefix(
    layout: &oxoffice_docx_layout::WordLayoutResult,
    paragraph_index: usize,
) -> Option<TocNumberingPrefix> {
    let numbering = layout.document_paragraph_numbering(paragraph_index)?;
    if numbering.marker_text.is_empty() {
        return None;
    }
    Some(TocNumberingPrefix {
        marker: numbering.marker_text.clone(),
        suffix: numbering.suffix,
    })
}

fn ensure_toc_entry_styles(
    document: &mut Document,
    sources: &[Vec<TocSource>],
) -> Result<(BTreeMap<u8, TocEntryStyle>, Vec<String>)> {
    let source_styles = toc_style_view(&document.styles);
    let mut levels = sources
        .iter()
        .flatten()
        .map(|source| source.level)
        .collect::<Vec<_>>();
    levels.sort_unstable();
    levels.dedup();
    let mut resolved = BTreeMap::new();
    for level in levels {
        let built_in_name = format!("toc {level}");
        let canonical_id = format!("TOC{level}");
        let style_id = document
            .styles
            .styles
            .iter()
            .find(|style| {
                // Entries reference their style by identifier, so a producer
                // style without one cannot be the entry style.
                style.style_type == StyleType::Paragraph
                    && !style.style_id.is_empty()
                    && style
                        .name
                        .as_deref()
                        .is_some_and(|name| name.trim().eq_ignore_ascii_case(&built_in_name))
            })
            .map(|style| style.style_id.clone())
            .or_else(|| {
                document
                    .styles
                    .get_by_id(&canonical_id)
                    .map(|style| style.style_id.clone())
            })
            .unwrap_or_else(|| {
                let (style, _, _) =
                    style::StyleBuilder::paragraph(&canonical_id, &format!("TOC {level}")).build();
                std::sync::Arc::make_mut(&mut document.styles)
                    .styles
                    .push(style);
                canonical_id
            });
        let effective = style::resolve_paragraph_properties(Some(&style_id), &document.styles);
        let has_right_tab = effective
            .tabs
            .as_ref()
            .is_some_and(|tabs| tabs.tabs.iter().any(|tab| tab.val == ST_TabJc::Right));
        resolved.insert(
            level,
            TocEntryStyle {
                style_id,
                has_right_tab,
            },
        );
    }
    // Producer defects the read surface accepts stay in the package and are
    // reported. Only a defect the staged entry styles introduce rejects.
    let retained =
        style::validate_style_graph_change(&source_styles, &toc_style_view(&document.styles))?
            .into_iter()
            .map(|defect| format!("{defect}, retained while rebuilding TOC"))
            .collect();
    Ok((resolved, retained))
}

/// The style graph a TOC rebuild resolves. It keeps the first definition of
/// each style ID and, for each style type, the default that
/// `CT_Styles::get_default` gives layout. The package keeps every definition.
fn toc_style_view(styles: &CT_Styles) -> CT_Styles {
    let mut view = styles.clone();
    let mut seen = HashSet::new();
    view.styles
        .retain(|style| seen.insert(style.style_id.clone()));
    for style in &mut view.styles {
        style.is_default = styles
            .get_default(style.style_type)
            .is_some_and(|default| default.style_id == style.style_id);
    }
    view
}

fn toc_default_style_diagnostics(styles: &CT_Styles) -> Vec<String> {
    [
        StyleType::Paragraph,
        StyleType::Character,
        StyleType::Table,
        StyleType::Numbering,
    ]
    .into_iter()
    .filter_map(|style_type| {
        let first = styles.get_default(style_type)?;
        styles
            .styles
            .iter()
            .any(|style| {
                style.style_type == style_type
                    && style.is_default
                    && style.style_id != first.style_id
            })
            .then(|| {
                format!(
                    "multiple default {} styles used first default '{}' while rebuilding TOC",
                    style_type.to_str(),
                    first.style_id
                )
            })
    })
    .collect()
}

fn toc_duplicate_style_diagnostics(styles: &CT_Styles) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut reported = HashSet::new();
    styles
        .styles
        .iter()
        .filter_map(|style| {
            if seen.insert(style.style_id.as_str()) || !reported.insert(style.style_id.as_str()) {
                return None;
            }
            Some(format!(
                "duplicate style ID '{}' used first definition while rebuilding TOC",
                style.style_id
            ))
        })
        .collect()
}

pub(super) fn toc_section_text_width(body: &CT_Body, paragraph_index: usize) -> i32 {
    const DEFAULT_PAGE_WIDTH: i32 = 12_240;
    const DEFAULT_MARGIN: i32 = 1_440;
    const DEFAULT_TEXT_WIDTH: i32 = DEFAULT_PAGE_WIDTH - 2 * DEFAULT_MARGIN;

    let mut paragraphs = Vec::new();
    collect_body_paragraphs(body, &mut paragraphs, None);
    let section = paragraphs
        .iter()
        .skip(paragraph_index)
        .find_map(|paragraph| {
            paragraph
                .properties
                .as_ref()
                .and_then(|properties| properties.sect_pr.as_ref())
        })
        .or(body.sect_pr.as_ref());
    let Some(section) = section else {
        return DEFAULT_TEXT_WIDTH;
    };
    let page_width = section
        .page_width
        .map(|width| width.0)
        .unwrap_or(DEFAULT_PAGE_WIDTH);
    let left = section
        .margin_left
        .map(|margin| margin.0)
        .unwrap_or(DEFAULT_MARGIN);
    let right = section
        .margin_right
        .map(|margin| margin.0)
        .unwrap_or(DEFAULT_MARGIN);
    page_width
        .checked_sub(left)
        .and_then(|width| width.checked_sub(right))
        .filter(|width| *width > 0)
        .unwrap_or(DEFAULT_TEXT_WIDTH)
}

pub(super) fn toc_source_position_is_owned(
    spans: &[DynamicTocSpan],
    position: TocOwnedPosition,
) -> bool {
    spans.iter().any(|span| {
        if span.begin_paragraph == span.end_paragraph && position.paragraph == span.begin_paragraph
        {
            return position.run >= span.result_start_position
                && position.run < span.result_end_position;
        }
        if position.paragraph > span.begin_paragraph && position.paragraph < span.end_paragraph {
            return true;
        }
        if position.paragraph == span.begin_paragraph {
            return position.run >= span.result_start_position;
        }
        if position.paragraph == span.end_paragraph {
            return position.run < span.result_end_position;
        }
        false
    })
}

pub(super) fn accepted_toc_runs(paragraph: &CT_P) -> Vec<AcceptedTocRun<'_>> {
    let mut runs = Vec::new();
    append_accepted_paragraph_runs(paragraph, None, &mut runs);
    let mut nested_run_orders = HashMap::<(usize, TocRawOrder), usize>::new();
    for run in &mut runs {
        let next = nested_run_orders
            .entry((run.run_boundary, run.raw_order))
            .or_default();
        run.nested_order = *next;
        *next += 1;
    }
    runs
}

pub(super) fn accepted_paragraph_owners(
    paragraph: &CT_P,
    boundary: usize,
) -> Vec<(TocRawOrder, AcceptedParagraphOwner<'_>)> {
    let mut owners = paragraph
        .content_controls
        .iter()
        .filter(|(at, _, _, _)| *at == boundary)
        .map(|(_, raw_before, _, control)| {
            (
                TocRawOrder::Raw(*raw_before),
                0u8,
                AcceptedParagraphOwner::Control(control),
            )
        })
        .chain(
            paragraph
                .revisions
                .iter()
                .filter(|(at, _, _)| *at == boundary)
                .map(|(_, slot, revision)| {
                    (
                        accepted_revision_raw_order(paragraph, boundary, *slot),
                        1u8,
                        AcceptedParagraphOwner::Revision(revision),
                    )
                }),
        )
        .collect::<Vec<_>>();
    owners.sort_by_key(|(raw_order, kind, _)| (*raw_order, *kind));
    owners
        .into_iter()
        .map(|(order, _, owner)| (order, owner))
        .collect()
}

fn append_accepted_paragraph_runs<'a>(
    paragraph: &'a CT_P,
    inherited_position: Option<(usize, TocRawOrder)>,
    runs: &mut Vec<AcceptedTocRun<'a>>,
) {
    for boundary in 0..=paragraph.runs.len() {
        for (raw_order, owner) in accepted_paragraph_owners(paragraph, boundary) {
            let position = inherited_position.unwrap_or((boundary, raw_order));
            match owner {
                AcceptedParagraphOwner::Control(control) => {
                    append_accepted_control_runs(control, position, runs);
                }
                AcceptedParagraphOwner::Revision(revision) => {
                    append_accepted_revision_runs(revision, position, runs);
                }
            }
        }
        if let Some(run) = paragraph.runs.get(boundary) {
            let (run_boundary, raw_order) =
                inherited_position.unwrap_or((boundary, TocRawOrder::AfterRaw));
            runs.push(AcceptedTocRun {
                run,
                run_boundary,
                raw_order,
                nested_order: 0,
            });
        }
    }
}

fn accepted_revision_raw_order(paragraph: &CT_P, boundary: usize, slot: usize) -> TocRawOrder {
    let Some(index) = hyperlink_revision_index(slot) else {
        return TocRawOrder::Raw(slot);
    };
    if let Some(raw_before) = paragraph
        .hyperlinks
        .get(index)
        .and_then(|hyperlink| hyperlink.preserved_raw_before)
    {
        TocRawOrder::Raw(raw_before)
    } else if paragraph
        .hyperlinks
        .get(index)
        .is_some_and(|hyperlink| boundary == hyperlink.run_end)
    {
        TocRawOrder::BeforeRaw
    } else {
        TocRawOrder::AfterRaw
    }
}

pub(super) enum AcceptedParagraphOwner<'a> {
    Control(&'a CT_Sdt),
    Revision(&'a CT_Revision),
}

fn append_accepted_control_runs<'a>(
    control: &'a CT_Sdt,
    position: (usize, TocRawOrder),
    runs: &mut Vec<AcceptedTocRun<'a>>,
) {
    for boundary in 0..=control.content.len() {
        for (_, revision) in control.revisions().iter().filter(|(at, _)| *at == boundary) {
            append_accepted_revision_runs(revision, position, runs);
        }
        if let Some(content) = control.content.get(boundary) {
            match content {
                SdtContent::Run(run) => runs.push(AcceptedTocRun {
                    run,
                    run_boundary: position.0,
                    raw_order: position.1,
                    nested_order: 0,
                }),
                SdtContent::ContentControl(control) => {
                    append_accepted_control_runs(control, position, runs)
                }
                SdtContent::Paragraph(paragraph) => {
                    append_accepted_paragraph_runs(paragraph, Some(position), runs)
                }
                SdtContent::Table(table) => append_accepted_table_runs(table, position, runs),
                SdtContent::Row(row) => append_accepted_row_runs(row, position, runs),
                SdtContent::Cell(cell) => append_accepted_cell_runs(cell, position, runs),
                SdtContent::RawXml(_) => {}
            }
        }
    }
}

fn append_accepted_revision_runs<'a>(
    revision: &'a CT_Revision,
    position: (usize, TocRawOrder),
    runs: &mut Vec<AcceptedTocRun<'a>>,
) {
    match revision.kind() {
        RevisionKind::Insertion | RevisionKind::MoveTo => {
            if let Some(paragraph) = revision.content_paragraph() {
                append_accepted_paragraph_runs(paragraph, Some(position), runs);
                return;
            }
            let RevisionContent::Runs(direct) = revision.content() else {
                return;
            };
            for boundary in 0..=direct.len() {
                for (_, nested) in revision
                    .nested_revisions()
                    .iter()
                    .filter(|(at, _)| *at == boundary)
                {
                    append_accepted_revision_runs(nested, position, runs);
                }
                if let Some(run) = direct.get(boundary) {
                    runs.push(AcceptedTocRun {
                        run,
                        run_boundary: position.0,
                        raw_order: position.1,
                        nested_order: 0,
                    });
                }
            }
        }
        RevisionKind::Deletion
        | RevisionKind::MoveFrom
        | RevisionKind::RunPropertyChange
        | RevisionKind::ParagraphPropertyChange
        | RevisionKind::TablePropertyChange
        | RevisionKind::SectionPropertyChange => {}
    }
}

fn append_accepted_table_runs<'a>(
    table: &'a CT_Tbl,
    position: (usize, TocRawOrder),
    runs: &mut Vec<AcceptedTocRun<'a>>,
) {
    for boundary in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == boundary)
        {
            append_accepted_control_runs(control, position, runs);
        }
        if let Some(row) = table.rows.get(boundary) {
            append_accepted_row_runs(row, position, runs);
        }
    }
}

fn append_accepted_row_runs<'a>(
    row: &'a CT_Row,
    position: (usize, TocRawOrder),
    runs: &mut Vec<AcceptedTocRun<'a>>,
) {
    for boundary in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == boundary)
        {
            append_accepted_control_runs(control, position, runs);
        }
        if let Some(cell) = row.cells.get(boundary) {
            append_accepted_cell_runs(cell, position, runs);
        }
    }
}

fn append_accepted_cell_runs<'a>(
    cell: &'a CT_Tc,
    position: (usize, TocRawOrder),
    runs: &mut Vec<AcceptedTocRun<'a>>,
) {
    for content in &cell.content {
        match content {
            CellContent::Paragraph(paragraph) => {
                append_accepted_paragraph_runs(paragraph, Some(position), runs)
            }
            CellContent::Table(table) => append_accepted_table_runs(table, position, runs),
            CellContent::ContentControl(control) => {
                append_accepted_control_runs(control, position, runs)
            }
        }
    }
}

fn field_argument_text(argument: &FieldArgument) -> Option<&str> {
    match argument {
        FieldArgument::Text(value) => Some(value),
        FieldArgument::Nested(_) => None,
    }
}

fn toc_accepts_tc(toc: &TocField, tc: &TcField) -> bool {
    match &toc.entries {
        TocEntrySelection::None => false,
        TocEntrySelection::All => true,
        TocEntrySelection::Identifier(identifier) => tc
            .table_identifier
            .as_deref()
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(identifier)),
    }
}

fn toc_paragraph_level(
    document: &Document,
    toc: &TocField,
    paragraph: &CT_P,
    effective_heading: Option<Option<u32>>,
) -> Result<Option<u8>> {
    let style_id = paragraph
        .properties
        .as_ref()
        .and_then(|properties| properties.style_id.as_deref());
    if let Some(style_id) = style_id {
        for (name, level) in &toc.custom_styles {
            let matches_id = style_id.eq_ignore_ascii_case(name);
            let matches_name = document
                .styles
                .get_by_id(style_id)
                .and_then(|style| style.name.as_deref())
                .is_some_and(|style_name| style_name.eq_ignore_ascii_case(name));
            if matches_id || matches_name {
                return Ok(Some(*level));
            }
        }
    }
    let outline = if toc.use_outline_levels {
        match effective_heading {
            Some(Some(level)) => u8::try_from(level)
                .ok()
                .filter(|level| (1..=9).contains(level)),
            Some(None) => return Ok(None),
            None => {
                return Err(Error::Other(
                    "TOC outline source has no qualified current layout facts".into(),
                ));
            }
        }
    } else {
        None
    };
    if let Some(style_id) = style_id
        && let Some((start, end)) = toc.heading_levels
        && let Some(level) = heading_style_level(document, style_id)
        && (start..=end).contains(&level)
    {
        return Ok(Some(level));
    }
    // Apply an explicit heading range to outline-derived entries as well.
    Ok(outline.filter(|level| {
        toc.heading_levels
            .is_none_or(|(start, end)| (start..=end).contains(level))
    }))
}

fn heading_style_level(document: &Document, style_id: &str) -> Option<u8> {
    let candidates = std::iter::once(style_id).chain(
        document
            .styles
            .get_by_id(style_id)
            .and_then(|style| style.name.as_deref()),
    );
    for candidate in candidates {
        let normalized = candidate
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        if let Some(suffix) = normalized.strip_prefix("heading")
            && let Ok(level) = suffix.parse::<u8>()
            && (1..=9).contains(&level)
        {
            return Some(level);
        }
    }
    None
}

fn insert_toc_bookmarks_xml(
    xml: &[u8],
    bookmarks: &BTreeMap<usize, TocBookmark>,
    repairs: &[TocBookmarkRepair],
    all_toc_spans: &[DynamicTocSpan],
    toc_spans: &[DynamicTocSpan],
) -> Result<Vec<u8>> {
    let paragraphs = toc_paragraph_insertions(xml)?;
    if bookmarks.keys().any(|index| *index >= paragraphs.len()) {
        return Err(Error::Other(
            "table of contents source paragraph was not found in package XML".to_owned(),
        ));
    }
    let mut insertions = BTreeMap::<usize, Vec<u8>>::new();
    for (paragraph_index, paragraph) in paragraphs.iter().enumerate() {
        let bookmark = bookmarks
            .get(&paragraph_index)
            .filter(|bookmark| bookmark.insert);
        if bookmark.is_none() {
            continue;
        }
        let end_boundary = toc_spans
            .iter()
            .any(|span| span.end_paragraph == paragraph_index);
        let fragment_start = toc_spans
            .iter()
            .filter(|span| span.end_paragraph == paragraph_index)
            .map(|span| span.end_run_end)
            .max()
            .unwrap_or(paragraph.content_start);
        let content_end = toc_spans
            .iter()
            .filter(|span| span.begin_paragraph == paragraph_index)
            .map(|span| span.begin_run_start)
            .min()
            .unwrap_or(paragraph.content_end);
        if fragment_start > content_end {
            return Err(Error::Other(
                "table of contents source bookmark has no unowned paragraph range".to_owned(),
            ));
        }
        if !end_boundary && let Some(bookmark) = bookmark {
            let start_insertion = insertions.entry(fragment_start).or_default();
            start_insertion.extend_from_slice(
                format!(
                    "<w:bookmarkStart w:id=\"{}\" w:name=\"{}\" xmlns:w=\"{W_NS}\"/>",
                    bookmark.id,
                    xml_escape_attribute(&bookmark.name)
                )
                .as_bytes(),
            );
        }

        let end_insertion = insertions.entry(content_end).or_default();
        if let Some(bookmark) = bookmark {
            end_insertion.extend_from_slice(
                format!(
                    "<w:bookmarkEnd w:id=\"{}\" xmlns:w=\"{W_NS}\"/>",
                    bookmark.id
                )
                .as_bytes(),
            );
        }
    }
    let mut repair_ends = BTreeMap::<usize, Vec<i32>>::new();
    for repair in repairs.iter().filter(|repair| !repair.replace_start) {
        let span = all_toc_spans.get(repair.toc_index).ok_or_else(|| {
            Error::Other("table of contents bookmark repair owner was not found".to_owned())
        })?;
        repair_ends
            .entry(span.begin_run_start)
            .or_default()
            .push(repair.id);
    }
    for (position, ids) in repair_ends {
        let insertion = insertions.entry(position).or_default();
        for id in ids.into_iter().rev() {
            insertion.extend_from_slice(
                format!("<w:bookmarkEnd w:id=\"{id}\" xmlns:w=\"{W_NS}\"/>").as_bytes(),
            );
        }
    }
    let mut output = xml.to_vec();
    for (position, replacement) in insertions.into_iter().rev() {
        output.splice(position..position, replacement);
    }
    Ok(output)
}

pub(super) fn toc_paragraph_insertions(xml: &[u8]) -> Result<Vec<TocParagraphInsertion>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut elements = Vec::<DynamicXmlElement>::new();
    let mut paragraphs = Vec::<TocParagraphInsertion>::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid table of contents XML: {error}")))?;
        let word = namespace_is_word(&namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let local = local_name(element.name().as_ref()).to_vec();
                let (namespace_bindings, inherited_namespaces) =
                    dynamic_namespace_bindings(&element, &elements)?;
                let mut typed_block_owner = classify_typed_block_owner(word, &local, &elements);
                let mut is_typed_inline_owner = false;
                validate_dynamic_content_control(
                    xml,
                    before,
                    word,
                    &local,
                    &namespace_bindings,
                    &mut typed_block_owner,
                    &mut is_typed_inline_owner,
                )?;
                let is_typed_paragraph = typed_block_owner == Some(TypedBlockOwner::Paragraph);
                let paragraph = if is_typed_paragraph {
                    let index = paragraphs.len();
                    paragraphs.push(TocParagraphInsertion {
                        start: before,
                        end: after,
                        name: String::from_utf8_lossy(element.name().as_ref()).into_owned(),
                        namespaces: namespace_bindings.clone(),
                        has_non_namespace_attributes: element.attributes().any(|attribute| {
                            attribute.is_ok_and(|attribute| {
                                attribute.key.as_ref() != b"xmlns"
                                    && !attribute.key.as_ref().starts_with(b"xmlns:")
                            })
                        }),
                        start_tag_end: after,
                        content_start: after,
                        content_end: after,
                    });
                    Some(index)
                } else {
                    elements.last().and_then(|element| element.paragraph)
                };
                mark_typed_sdt_content(&mut elements, &local, typed_block_owner, false);
                elements.push(DynamicXmlElement {
                    local_name: local,
                    qualified_name: String::from_utf8_lossy(element.name().as_ref()).into_owned(),
                    typed_block_owner,
                    is_word: word,
                    is_typed_paragraph,
                    is_typed_inline_owner: false,
                    revision_depth: 0,
                    sdt_content_seen: false,
                    namespace_bindings,
                    inherited_namespaces,
                    run_position: None,
                    hyperlink_plan: None,
                    start: before,
                    start_tag_end: after,
                    paragraph,
                });
            }
            Event::Empty(element) => {
                let name = element.name();
                let local = local_name(name.as_ref());
                // Empty content is a retained raw slot, not the typed Start slot.
                let word = word && local != b"sdtContent";
                let typed_block_owner = classify_typed_block_owner(word, local, &elements);
                if typed_block_owner == Some(TypedBlockOwner::Paragraph) {
                    paragraphs.push(TocParagraphInsertion {
                        start: before,
                        end: after,
                        name: String::from_utf8_lossy(element.name().as_ref()).into_owned(),
                        namespaces: BTreeMap::new(),
                        has_non_namespace_attributes: false,
                        start_tag_end: before,
                        content_start: before,
                        content_end: before,
                    });
                }
                mark_typed_sdt_content(&mut elements, local, typed_block_owner, false);
                if word
                    && local == b"pPr"
                    && let Some(parent) = elements.last()
                    && parent.is_typed_paragraph
                    && let Some(paragraph) = parent.paragraph
                {
                    paragraphs[paragraph].content_start = after;
                }
            }
            Event::End(element) => {
                let Some(closed) = elements.pop() else {
                    return Err(Error::Other(
                        "table of contents XML has an unmatched end element".to_owned(),
                    ));
                };
                if word
                    && matches_local_name(element.name().as_ref(), b"pPr")
                    && let Some(paragraph) = closed.paragraph
                    && elements
                        .last()
                        .is_some_and(|parent| parent.is_word && parent.local_name == b"p")
                {
                    paragraphs[paragraph].content_start = after;
                }
                if word
                    && matches_local_name(element.name().as_ref(), b"p")
                    && let Some(paragraph) = closed.paragraph
                {
                    paragraphs[paragraph].content_end = before;
                    paragraphs[paragraph].end = after;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(paragraphs)
}

#[derive(Debug)]
struct TocPagePlaceholder {
    toc_index: usize,
    token: String,
    bookmark: String,
}

fn render_toc_entries(
    toc_index: usize,
    toc: &TocField,
    sources: &[TocSource],
    entry_styles: &BTreeMap<u8, TocEntryStyle>,
    fallback_right_tab: i32,
    bookmarks: &BTreeMap<usize, TocBookmark>,
    source_xml: &[u8],
    placeholders: &mut Vec<TocPagePlaceholder>,
) -> Result<Vec<u8>> {
    let mut output = String::new();
    for source in sources {
        let bookmark = bookmarks.get(&source.paragraph_index);
        if source.needs_bookmark && bookmark.is_none() {
            return Err(Error::Other(
                "table of contents source bookmark was not allocated".to_owned(),
            ));
        }
        let entry_style = entry_styles.get(&source.level).ok_or_else(|| {
            Error::Other(format!(
                "table of contents level {} has no entry style",
                source.level
            ))
        })?;
        output.push_str("<w:p><w:pPr><w:pStyle w:val=\"");
        output.push_str(&xml_escape_attribute(&entry_style.style_id));
        output.push_str("\"/>");
        // Word gives the entry of a heading numbered with a tab its own left
        // stop after the number, so the title does not go to the page number
        // stop.
        let number_stop = match source.numbering_prefix.as_ref() {
            Some(prefix) if prefix.suffix == ST_LvlSuffix::Tab => {
                Some(toc_number_tab_stop(&prefix.marker, source.level))
            }
            _ => None,
        };
        let fallback_right = !source.omit_page_number
            && toc.page_number_separator.is_none()
            && !entry_style.has_right_tab;
        if number_stop.is_some() || fallback_right {
            output.push_str("<w:tabs>");
            if let Some(stop) = number_stop {
                output.push_str("<w:tab w:val=\"left\" w:pos=\"");
                output.push_str(&stop.to_string());
                output.push_str("\"/>");
            }
            if fallback_right {
                output.push_str("<w:tab w:val=\"right\" w:leader=\"dot\" w:pos=\"");
                output.push_str(&fallback_right_tab.to_string());
                output.push_str("\"/>");
            }
            output.push_str("</w:tabs>");
        }
        output.push_str("</w:pPr>");
        if toc.hyperlink {
            output.push_str("<w:hyperlink w:anchor=\"");
            output.push_str(&xml_escape_attribute(
                &bookmark.expect("required above").name,
            ));
            output.push_str("\">");
        }
        if let Some(prefix) = source.numbering_prefix.as_ref() {
            output.push_str("<w:r><w:t>");
            output.push_str(&xml_escape_text(&prefix.marker));
            output.push_str("</w:t></w:r>");
            match prefix.suffix {
                ST_LvlSuffix::Tab => output.push_str("<w:r><w:tab/></w:r>"),
                ST_LvlSuffix::Space => {
                    output.push_str("<w:r><w:t xml:space=\"preserve\"> </w:t></w:r>")
                }
                ST_LvlSuffix::Nothing => {}
            }
        }
        output.push_str("<w:r><w:t>");
        output.push_str(&xml_escape_text(&source.title));
        output.push_str("</w:t></w:r>");
        if toc.hyperlink {
            output.push_str("</w:hyperlink>");
        }
        if !source.omit_page_number {
            if let Some(separator) = toc.page_number_separator.as_deref() {
                output.push_str("<w:r><w:t xml:space=\"preserve\">");
                output.push_str(&xml_escape_text(separator));
                output.push_str("</w:t></w:r>");
            } else {
                output.push_str("<w:r><w:tab/></w:r>");
            }
            if let Some(sequence) = source.sequence_prefix.as_deref() {
                output.push_str("<w:r><w:t>");
                output.push_str(&xml_escape_text(sequence));
                output.push_str("</w:t></w:r><w:r><w:t>");
                output.push_str(&xml_escape_text(
                    toc.entry_page_separator.as_deref().unwrap_or("-"),
                ));
                output.push_str("</w:t></w:r>");
            }
            let bookmark = &bookmark.expect("required above").name;
            let token = collision_safe_toc_page_token(source_xml, output.as_bytes(), placeholders);
            output.push_str("<w:fldSimple w:instr=\" PAGEREF ");
            output.push_str(&xml_escape_attribute(bookmark));
            output.push_str(" \\h \" w:dirty=\"0\"><w:r><w:t>");
            output.push_str(&token);
            output.push_str("</w:t></w:r></w:fldSimple>");
            placeholders.push(TocPagePlaceholder {
                toc_index,
                token,
                bookmark: bookmark.clone(),
            });
        }
        output.push_str("</w:p>");
    }
    Ok(output.into_bytes())
}

/// Advance widths of Aptos, Word's default font, for the printable ASCII
/// characters from space to `~`, in its 2048 units per em.
///
/// Word measures a TOC entry's number in this font, so these values are
/// facts about Word's output rather than about the document. They are the
/// font's own advances: Word writes 2509 twips for "WWWWWWWWWW1", which is
/// exactly their sum at 12 points plus 240.
const APTOS_ADVANCES: [u16; 95] = [
    416, 600, 757, 1100, 1094, 1692, 1317, 431, 600, 600, 936, 1094, 585, 697, 585, 695, 1094,
    1094, 1094, 1094, 1094, 1094, 1094, 1094, 1094, 1094, 585, 585, 1094, 1094, 1094, 1027, 1832,
    1207, 1237, 1418, 1405, 1139, 1074, 1451, 1448, 533, 678, 1164, 1023, 1618, 1446, 1500, 1182,
    1500, 1241, 1159, 980, 1395, 1199, 1827, 1132, 1105, 1056, 603, 695, 603, 1094, 942, 1130,
    1088, 1149, 1075, 1148, 1079, 617, 992, 1129, 489, 489, 997, 533, 1747, 1129, 1130, 1149, 1148,
    685, 995, 662, 1144, 926, 1476, 906, 926, 897, 603, 553, 603, 1094,
];

/// The left stop Word writes after the number of a TOC entry, in twips.
///
/// Word measures the number in its own default font, 12 pt Aptos in current
/// releases, whatever the document's fonts, and puts the stop on the first
/// 12 pt boundary at least 12 pt past the number, counting 12 pt of indent
/// for each level below the first. A character outside the table counts as
/// one em.
fn toc_number_tab_stop(marker: &str, level: u8) -> i32 {
    const STEP: u64 = 240;
    let units = marker.chars().fold(0u64, |sum, ch| {
        let advance = u32::from(ch)
            .checked_sub(0x20)
            .and_then(|index| APTOS_ADVANCES.get(index as usize))
            .map_or(2048, |advance| u32::from(*advance));
        sum.saturating_add(u64::from(advance))
    });
    // The indent is `level - 1` steps and the stop is one step past the
    // number, so the two cancel to `level` plus the rounded-up width.
    let steps = u64::from(level)
        .saturating_add(units.saturating_add(2047) / 2048)
        .min((i32::MAX as u64) / STEP);
    (steps as i32) * STEP as i32
}

fn collision_safe_toc_page_token(
    source_xml: &[u8],
    generated: &[u8],
    placeholders: &[TocPagePlaceholder],
) -> String {
    let index = placeholders.len();
    for nonce in 0u64.. {
        let token = format!("__RDOCX_TOC_PAGE_{index}_{nonce}__");
        if find_bytes(source_xml, token.as_bytes()).is_none()
            && find_bytes(generated, token.as_bytes()).is_none()
            && placeholders
                .iter()
                .all(|placeholder| placeholder.token != token)
        {
            return token;
        }
    }
    unreachable!("u64 placeholder nonce space is finite but non-empty")
}

pub(super) fn xml_escape_text(value: &str) -> String {
    quick_xml::escape::escape(value).into_owned()
}

pub(super) fn xml_escape_attribute(value: &str) -> String {
    quick_xml::escape::escape(value).into_owned()
}

fn end_boundary_bookmark_starts(
    toc_index: usize,
    span: &DynamicTocSpan,
    spans: &[DynamicTocSpan],
    fields: &[Option<TocField>],
    bookmarks: &BTreeMap<usize, TocBookmark>,
    repairs: &[TocBookmarkRepair],
) -> Vec<u8> {
    let mut output = Vec::new();
    for repair in repairs
        .iter()
        .filter(|repair| repair.toc_index == toc_index && repair.replace_start)
    {
        output.extend_from_slice(
            format!(
                "<w:bookmarkStart w:id=\"{}\" w:name=\"{}\" xmlns:w=\"{W_NS}\"/>",
                repair.id,
                xml_escape_attribute(&repair.name)
            )
            .as_bytes(),
        );
    }
    let owns_last_end_boundary = fields.get(toc_index).is_some_and(Option::is_some)
        && spans
            .iter()
            .zip(fields)
            .filter(|(candidate, field)| {
                field.is_some() && candidate.end_paragraph == span.end_paragraph
            })
            .all(|(candidate, _)| candidate.end_run_end <= span.end_run_end);
    if owns_last_end_boundary
        && let Some(bookmark) = bookmarks
            .get(&span.end_paragraph)
            .filter(|bookmark| bookmark.insert)
    {
        output.extend_from_slice(
            format!(
                "<w:bookmarkStart w:id=\"{}\" w:name=\"{}\" xmlns:w=\"{W_NS}\"/>",
                bookmark.id,
                xml_escape_attribute(&bookmark.name)
            )
            .as_bytes(),
        );
    }
    output
}

pub(super) fn dynamic_toc_replacement(
    xml: &[u8],
    span: &DynamicTocSpan,
    generated: &[u8],
    end_boundary_bookmark_starts: &[u8],
) -> Result<Vec<u8>> {
    if span.result_start > span.end_paragraph_start
        || span.end_paragraph_start > span.end_paragraph_content_start
        || span.end_paragraph_content_start > span.result_end
        || span.result_end > xml.len()
    {
        return Err(Error::Other(
            "table of contents result paragraph boundaries are invalid".to_owned(),
        ));
    }
    let mut prior_wrapper_end = span.end_paragraph_content_start;
    for &(start, end) in &span.end_wrapper_prefixes {
        if start < prior_wrapper_end || start >= end || end > span.result_end {
            return Err(Error::Other(
                "table of contents end-marker wrappers are invalid".to_owned(),
            ));
        }
        prior_wrapper_end = end;
    }
    let mut replacement = Vec::new();
    append_toc_wrapper_closures(&mut replacement, &span.separator_wrapper_names);
    replacement.extend_from_slice(b"</");
    replacement.extend_from_slice(span.start_paragraph_name.as_bytes());
    replacement.push(b'>');
    replacement.extend_from_slice(generated);
    replacement.extend_from_slice(&xml[span.end_paragraph_start..span.end_paragraph_content_start]);
    replacement.extend_from_slice(end_boundary_bookmark_starts);
    for &(start, end) in &span.end_wrapper_prefixes {
        replacement.extend_from_slice(&xml[start..end]);
    }
    Ok(replacement)
}

/// Reuse eligible entry paragraph roots while replacing only owned entry facts.
fn entry_boundary_toc_edits(
    xml: &[u8],
    span: &DynamicTocSpan,
    paragraphs: &[TocParagraphInsertion],
    generated: &[u8],
    entry_style_ids: &BTreeSet<String>,
    end_boundary_bookmark_starts: &[u8],
) -> Result<Option<Vec<FieldSourceEdit>>> {
    if !span.separator_wrapper_names.is_empty()
        || !span.end_wrapper_prefixes.is_empty()
        || !end_boundary_bookmark_starts.is_empty()
    {
        return Ok(None);
    }
    let begin = &paragraphs[span.begin_paragraph];
    let end = &paragraphs[span.end_paragraph];
    let single = span.begin_paragraph == span.end_paragraph;
    let begin_properties = &xml[begin.start_tag_end..begin.content_start];
    let end_properties = &xml[end.start_tag_end..end.content_start];
    let begin_scan = toc_fragment_scan(begin_properties, &begin.namespaces)?;
    let end_scan = toc_fragment_scan(end_properties, &end.namespaces)?;
    let begin_content = toc_fragment_scan(
        &xml[span.result_start..if single {
            span.result_end
        } else {
            begin.content_end
        }],
        &begin.namespaces,
    )?;
    let after_end = toc_fragment_scan(&xml[span.end_run_end..end.content_end], &end.namespaces)?;
    let is_entry = |scan: &TocFragmentScan, has_run: bool| {
        !scan.blocks_replacement
            && has_run
            && scan
                .style
                .as_ref()
                .is_some_and(|(id, _)| entry_style_ids.contains(id))
    };
    let merge_begin = begin.content_start == span.begin_run_start
        && (is_entry(&begin_scan, begin_content.has_run)
            || (single && !begin_content.has_run && !begin_scan.blocks_replacement));
    let merge_end = !after_end.has_run
        && is_entry(
            &end_scan,
            toc_fragment_scan(&xml[end.content_start..span.result_end], &end.namespaces)?.has_run,
        );
    let mut entries = Vec::new();
    let mut offset = 0;
    for close in byte_match_offsets(generated, b"</w:p>") {
        let next = close + b"</w:p>".len();
        entries.push(&generated[offset..next]);
        offset = next;
    }
    if offset != generated.len() {
        return Ok(None);
    }
    if single && (!merge_begin || after_end.has_run) {
        return Ok(None);
    }
    if !single && !merge_begin && !merge_end {
        return Ok(None);
    }
    if entries.is_empty() {
        let mut edits = Vec::new();
        if merge_begin {
            edits.push(FieldSourceEdit {
                start: begin.start_tag_end,
                end: begin.content_start,
                replacement: toc_overlay_entry_properties(begin_properties, &begin_scan, None)?,
            });
        }
        let replacement = if single {
            Vec::new()
        } else {
            // Zero entries retain the balanced two-root topology. Remove only
            // the entry style on an eligible owner, preserving its other facts.
            let mut replacement = format!("</{}>", begin.name).into_bytes();
            replacement.extend_from_slice(&xml[end.start..end.start_tag_end]);
            if merge_end {
                replacement.extend_from_slice(&toc_overlay_entry_properties(
                    end_properties,
                    &end_scan,
                    None,
                )?);
            } else {
                replacement.extend_from_slice(end_properties);
            }
            replacement
        };
        edits.push(FieldSourceEdit {
            start: span.result_start,
            end: span.result_end,
            replacement,
        });
        return Ok(Some(edits));
    }
    // Combining two roots cannot silently discard identity or foreign attributes
    // on the removed end root. Keep that root in the generic topology instead.
    let keep_end_root = entries.len() == 1
        && merge_begin
        && merge_end
        && !single
        && (end.has_non_namespace_attributes || end_scan.has_opaque_properties);
    let merge_end = merge_end && !keep_end_root;
    let generated_bindings = BTreeMap::from([("w".to_owned(), W_NS.to_owned())]);
    let mut edits = Vec::new();
    let mut replacement = Vec::new();
    if !merge_begin {
        replacement.extend_from_slice(format!("</{}>", begin.name).as_bytes());
    }
    for (index, entry) in entries.iter().enumerate() {
        let properties_end = find_bytes(entry, b"</w:pPr>").ok_or_else(|| {
            Error::Other("generated TOC entry has no paragraph properties".to_owned())
        })? + b"</w:pPr>".len();
        let generated_properties = &entry[b"<w:p>".len()..properties_end];
        let first_reused = index == 0 && merge_begin;
        let last_reused = !single && index + 1 == entries.len() && merge_end;
        if first_reused {
            edits.push(FieldSourceEdit {
                start: begin.start_tag_end,
                end: begin.content_start,
                replacement: toc_overlay_entry_properties(
                    begin_properties,
                    &begin_scan,
                    Some(generated_properties),
                )?,
            });
        } else if last_reused {
            replacement.extend_from_slice(&xml[end.start..end.start_tag_end]);
            replacement.extend_from_slice(&toc_overlay_entry_properties(
                end_properties,
                &end_scan,
                Some(generated_properties),
            )?);
        } else {
            replacement.extend_from_slice(format!("<w:p xmlns:w=\"{W_NS}\">").as_bytes());
            replacement.extend_from_slice(generated_properties);
        }
        replacement.extend_from_slice(&toc_close_fragment_namespaces(
            &entry[properties_end..entry.len() - b"</w:p>".len()],
            &generated_bindings,
        )?);
        if index + 1 < entries.len() || (!single && !last_reused) {
            replacement.extend_from_slice(
                if first_reused {
                    format!("</{}>", begin.name).into_bytes()
                } else {
                    b"</w:p>".to_vec()
                }
                .as_slice(),
            );
        }
    }
    if !single && !merge_end {
        replacement.extend_from_slice(&xml[end.start..end.content_start]);
    }
    edits.push(FieldSourceEdit {
        start: span.result_start,
        end: span.result_end,
        replacement,
    });
    if single && entries.len() > 1 {
        // The final generated entry no longer inherits the original paragraph
        // scope. Retained end content must carry that scope on every moved root.
        edits.push(FieldSourceEdit {
            start: span.result_end,
            end: end.content_end,
            replacement: toc_close_fragment_namespaces(
                &xml[span.result_end..end.content_end],
                &end.namespaces,
            )?,
        });
        edits.push(FieldSourceEdit {
            start: end.content_end,
            end: end.end,
            replacement: b"</w:p>".to_vec(),
        });
    }
    if !single && merge_begin && merge_end && entries.len() == 1 {
        // The end run and every trailing sibling move out of their original
        // namespace scope. Close each retained root, without rewriting tokens.
        edits.push(FieldSourceEdit {
            start: span.result_end,
            end: end.content_end,
            replacement: toc_close_fragment_namespaces(
                &xml[span.result_end..end.content_end],
                &end.namespaces,
            )?,
        });
        edits.push(FieldSourceEdit {
            start: end.content_end,
            end: end.end,
            replacement: format!("</{}>", begin.name).into_bytes(),
        });
    }
    edits.sort_by_key(|edit| edit.start);
    Ok(Some(edits))
}

/// Resolve a fragment in its actual paragraph scope, retaining exact byte ranges.
struct TocFragmentScan {
    namespaces: BTreeMap<String, String>,
    style: Option<(String, std::ops::Range<usize>)>,
    has_run: bool,
    blocks_replacement: bool,
    has_opaque_properties: bool,
    property_root: Option<(usize, usize, usize, usize, String)>,
    children: Vec<(Option<String>, std::ops::Range<usize>)>,
}

fn toc_fragment_scan(
    fragment: &[u8],
    bindings: &BTreeMap<String, String>,
) -> Result<TocFragmentScan> {
    let mut wrapper = b"<toc-fragment".to_vec();
    for (prefix, uri) in bindings {
        if prefix == "xml" {
            continue;
        }
        wrapper.extend_from_slice(
            if prefix.is_empty() {
                " xmlns".to_owned()
            } else {
                format!(" xmlns:{prefix}")
            }
            .as_bytes(),
        );
        wrapper.extend_from_slice(format!("=\"{}\"", xml_escape_attribute(uri)).as_bytes());
    }
    wrapper.push(b'>');
    let origin = wrapper.len();
    wrapper.extend_from_slice(fragment);
    wrapper.extend_from_slice(b"</toc-fragment>");
    let mut reader = NsReader::from_reader(wrapper.as_slice());
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut scan = TocFragmentScan {
        namespaces: bindings.clone(),
        style: None,
        has_run: false,
        blocks_replacement: false,
        has_opaque_properties: false,
        property_root: None,
        children: Vec::new(),
    };
    let mut child: Option<(Option<String>, usize)> = None;
    let mut owned = HashSet::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid TOC boundary XML: {error}")))?;
        let word = namespace_is_word(&namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let empty = matches!(event, Event::Empty(_));
                let name = element.name();
                let local = local_name(name.as_ref());
                if depth >= 1 && word && local == b"r" {
                    scan.has_run = true;
                }
                if depth == 1 && word && matches!(local, b"pPr" | b"tabs") {
                    if scan.property_root.is_some() {
                        return Err(Error::Other(
                            "duplicate owned TOC paragraph properties".to_owned(),
                        ));
                    }
                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(|error| {
                            Error::Other(format!("invalid TOC boundary namespace: {error}"))
                        })?;
                        let key = attribute.key.as_ref();
                        let prefix = if key == b"xmlns" {
                            Some(String::new())
                        } else {
                            key.strip_prefix(b"xmlns:")
                                .map(|prefix| String::from_utf8_lossy(prefix).into_owned())
                        };
                        if let Some(prefix) = prefix {
                            let value = attribute
                                .decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    element.decoder(),
                                )
                                .map_err(|error| {
                                    Error::Other(format!("invalid TOC boundary namespace: {error}"))
                                })?
                                .into_owned();
                            if value.is_empty() {
                                scan.namespaces.remove(&prefix);
                            } else {
                                scan.namespaces.insert(prefix, value);
                            }
                        }
                    }
                    let start = before - origin;
                    let opening = after - origin;
                    scan.property_root = Some((
                        start,
                        opening,
                        opening,
                        opening,
                        String::from_utf8_lossy(name.as_ref()).into_owned(),
                    ));
                    if empty {
                        scan.property_root.as_mut().unwrap().2 = start;
                    }
                }
                // A generated stop cannot erase opaque facts attached to an
                // existing stop. Leave this boundary in the generic topology.
                let inside_tab_stop = depth >= 3
                    && child
                        .as_ref()
                        .is_some_and(|(key, _)| key.as_deref() == Some("tabs"));
                if inside_tab_stop && (depth > 3 || !word || local != b"tab") {
                    scan.blocks_replacement = true;
                }
                let inside_style = child
                    .as_ref()
                    .is_some_and(|(key, _)| key.as_deref() == Some("pStyle"));
                if inside_style && depth >= 3 {
                    scan.blocks_replacement = true;
                }
                if scan.property_root.is_some() && depth >= 1 {
                    if depth >= 2 && !word {
                        scan.has_opaque_properties = true;
                    }
                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(|error| {
                            Error::Other(format!("invalid TOC property attribute: {error}"))
                        })?;
                        let key = attribute.key.as_ref();
                        let declaration = key == b"xmlns" || key.starts_with(b"xmlns:");
                        // Namespace closure on a generated Word fact is not an
                        // opaque producer declaration. Its exact binding is
                        // already required by the qualified owner itself.
                        if key == b"xmlns:w"
                            && word
                            && ((depth == 2 && name.as_ref() == b"w:pStyle")
                                || (inside_tab_stop && depth == 3 && name.as_ref() == b"w:tab"))
                            && attribute
                                .decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    element.decoder(),
                                )
                                .map_err(|error| {
                                    Error::Other(format!("invalid TOC property namespace: {error}"))
                                })?
                                == W_NS
                        {
                            continue;
                        }
                        let (attribute_namespace, attribute_local) =
                            reader.resolver().resolve_attribute(attribute.key);
                        // Only these attributes are generated entry facts. A
                        // recognized property name does not own its other data.
                        let owned_fact = !declaration
                            && word
                            && namespace_is_word(&attribute_namespace)
                            && ((depth == 2
                                && local == b"pStyle"
                                && attribute_local.as_ref() == b"val")
                                || (inside_tab_stop
                                    && depth == 3
                                    && local == b"tab"
                                    && matches!(
                                        attribute_local.as_ref(),
                                        b"val" | b"leader" | b"pos"
                                    )));
                        if !owned_fact {
                            scan.has_opaque_properties = true;
                            if inside_tab_stop || (depth == 2 && word && local == b"pStyle") {
                                // These owners are replaced or removed, including
                                // zero-entry style clearing. Keep opaque owners.
                                scan.blocks_replacement = true;
                            }
                        }
                    }
                }
                if depth == 2 && scan.property_root.is_some() {
                    let key = word.then(|| String::from_utf8_lossy(local).into_owned());
                    if word
                        && scan
                            .property_root
                            .as_ref()
                            .is_some_and(|root| !root.4.ends_with(":tabs") && root.4 != "tabs")
                        && !matches!(local, b"pStyle" | b"tabs")
                    {
                        scan.has_opaque_properties = true;
                    }
                    if let Some(ref key) = key {
                        if key != "tab" && !owned.insert(key.clone()) {
                            return Err(Error::Other(format!(
                                "duplicate owned TOC paragraph property {key}"
                            )));
                        }
                        if matches!(local, b"sectPr" | b"numPr" | b"pPrChange") {
                            scan.blocks_replacement = true;
                        }
                    }
                    let range = before - origin..after - origin;
                    if word && local == b"pStyle" {
                        let mut value = None;
                        for attribute in element.attributes() {
                            let attribute = attribute.map_err(|error| {
                                Error::Other(format!("invalid TOC property attribute: {error}"))
                            })?;
                            let (namespace, local) =
                                reader.resolver().resolve_attribute(attribute.key);
                            if namespace_is_word(&namespace) && local.as_ref() == b"val" {
                                if value.is_some() {
                                    return Err(Error::Other(
                                        "duplicate owned TOC style value".to_owned(),
                                    ));
                                }
                                value = Some(
                                    attribute
                                        .decoded_and_normalized_value(
                                            XmlVersion::Implicit1_0,
                                            element.decoder(),
                                        )
                                        .map_err(|error| {
                                            Error::Other(format!(
                                                "invalid TOC style value: {error}"
                                            ))
                                        })?
                                        .into_owned(),
                                );
                            }
                        }
                        let value = value.ok_or_else(|| {
                            Error::Other("TOC entry style has no owned value".to_owned())
                        })?;
                        scan.style = Some((value, range.clone()));
                    }
                    if empty {
                        scan.children.push((key, range));
                    } else {
                        child = Some((key, range.start));
                    }
                }
                if word && depth > 2 && matches!(local, b"ins" | b"del" | b"moveFrom" | b"moveTo") {
                    scan.blocks_replacement = true;
                }
                if !empty {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| Error::Other("unbalanced TOC boundary XML".to_owned()))?;
                if depth == 2
                    && let Some((key, start)) = child.take()
                {
                    let range = start..after - origin;
                    if key.as_deref() == Some("pStyle")
                        && let Some((_, old)) = scan.style.as_mut()
                    {
                        *old = range.clone();
                    }
                    scan.children.push((key, range));
                }
                if depth == 1
                    && let Some(root) = scan.property_root.as_mut()
                {
                    // Only the pPr root is active at depth two.
                    if root.3 == root.1 {
                        root.2 = before - origin;
                        root.3 = after - origin;
                    }
                }
            }
            Event::Comment(_)
            | Event::PI(_)
            | Event::CData(_)
            | Event::Text(_)
            | Event::GeneralRef(_) => {
                if depth >= 2
                    && scan
                        .property_root
                        .as_ref()
                        .is_some_and(|root| root.3 == root.1)
                {
                    // Ordinary XML indentation does not protect an otherwise
                    // mergeable root. All other body tokens are imported facts.
                    let opaque = match &event {
                        Event::Text(text) => !text
                            .iter()
                            .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n')),
                        _ => true,
                    };
                    scan.has_opaque_properties |= opaque;
                }
                if (depth >= 4
                    && child
                        .as_ref()
                        .is_some_and(|(key, _)| key.as_deref() == Some("tabs")))
                    || (depth >= 3
                        && child
                            .as_ref()
                            .is_some_and(|(key, _)| key.as_deref() == Some("pStyle")))
                {
                    // These tokens belong to the imported stop, not the generated
                    // entry facts. Replacing the stop would discard their bytes.
                    scan.blocks_replacement = true;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(scan)
}

/// Overlay generated style and tab facts. Retain all other source particles.
fn toc_overlay_entry_properties(
    original: &[u8],
    scan: &TocFragmentScan,
    generated: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let canonical = BTreeMap::from([("w".to_owned(), W_NS.to_owned())]);
    let mut replacements = Vec::new();
    if let Some(generated) = generated {
        let new_scan = toc_fragment_scan(generated, &canonical)?;
        for (name, range) in new_scan.children {
            let name = name.unwrap();
            let replacement = if name == "pStyle"
                && let Some((old_id, old_range)) = &scan.style
            {
                let generated_scan = toc_fragment_scan(generated, &canonical)?;
                let new_id = &generated_scan
                    .style
                    .as_ref()
                    .ok_or_else(|| Error::Other("generated TOC style is absent".to_owned()))?
                    .0;
                if old_id == new_id {
                    original[old_range.clone()].to_vec()
                } else {
                    toc_retarget_style(&original[old_range.clone()], new_id, &scan.namespaces)?
                }
            } else {
                toc_close_fragment_namespaces(&generated[range], &canonical)?
            };
            replacements.push((name, replacement));
        }
    }
    let Some((start, opening_end, closing_start, end, ref name)) = scan.property_root else {
        return generated.map_or_else(
            || Ok(original.to_vec()),
            |properties| toc_close_fragment_namespaces(properties, &canonical),
        );
    };
    let mut output = original[..start].to_vec();
    let empty = opening_end == end && closing_start == start;
    if empty {
        output.extend_from_slice(&original[start..opening_end - 2]);
        output.push(b'>');
    } else {
        output.extend_from_slice(&original[start..opening_end]);
    }
    // pStyle comes first in CT_PPr. Insert it before retaining source particles.
    if let Some((_, replacement)) = replacements.iter().find(|(key, _)| key == "pStyle") {
        output.extend_from_slice(replacement);
    }
    let mut cursor = opening_end;
    let mut tabs_written = false;
    for (key, range) in &scan.children {
        output.extend_from_slice(&original[cursor..range.start]);
        if key.as_deref() == Some("pStyle") {
            // Remove the complete old style, including a started element's end.
        } else if key.as_deref() == Some("tabs") {
            if let Some((_, replacement)) = replacements.iter().find(|(key, _)| key == "tabs") {
                output.extend_from_slice(&toc_overlay_tab_particles(
                    &original[range.clone()],
                    replacement,
                    &scan.namespaces,
                )?);
            } else {
                output.extend_from_slice(&original[range.clone()]);
            }
            tabs_written = true;
        } else {
            // Tabs precede spacing and later CT_PPr properties. Unknown particles
            // keep their original order and exact source bytes.
            if !tabs_written
                && matches!(
                    key.as_deref(),
                    Some(
                        "suppressAutoHyphens"
                            | "kinsoku"
                            | "wordWrap"
                            | "overflowPunct"
                            | "topLinePunct"
                            | "autoSpaceDE"
                            | "autoSpaceDN"
                            | "bidi"
                            | "adjustRightInd"
                            | "snapToGrid"
                            | "spacing"
                            | "ind"
                            | "contextualSpacing"
                            | "mirrorIndents"
                            | "suppressOverlap"
                            | "jc"
                            | "textDirection"
                            | "textAlignment"
                            | "textboxTightWrap"
                            | "outlineLvl"
                            | "divId"
                            | "cnfStyle"
                            | "rPr"
                            | "sectPr"
                            | "pPrChange"
                    )
                )
            {
                if let Some((_, replacement)) = replacements.iter().find(|(key, _)| key == "tabs") {
                    output.extend_from_slice(replacement);
                }
                tabs_written = true;
            }
            output.extend_from_slice(&original[range.clone()]);
        }
        cursor = range.end;
    }
    if !tabs_written
        && let Some((_, replacement)) = replacements.iter().find(|(key, _)| key == "tabs")
    {
        output.extend_from_slice(replacement);
    }
    if !empty {
        output.extend_from_slice(&original[cursor..closing_start]);
    }
    if empty {
        output.extend_from_slice(format!("</{name}>").as_bytes());
    } else {
        output.extend_from_slice(&original[closing_start..end]);
    }
    output.extend_from_slice(&original[end..]);
    Ok(output)
}

/// Change only the qualified style value, retaining opaque attributes and body.
fn toc_retarget_style(
    original: &[u8],
    value: &str,
    bindings: &BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let closed = xml_fragment_with_namespaces(original, bindings, "TOC style")?;
    let mut reader = NsReader::from_reader(closed.as_slice());
    let mut buffer = Vec::new();
    let (_, event) = reader
        .read_resolved_event_into(&mut buffer)
        .map_err(|error| Error::Other(format!("invalid TOC style: {error}")))?;
    let element = match event {
        Event::Start(element) | Event::Empty(element) => element,
        _ => return Err(Error::Other("TOC style has no root".to_owned())),
    };
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("invalid TOC style attribute: {error}")))?;
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if namespace_is_word(&namespace) && local.as_ref() == b"val" {
            let (_, start, end) = attribute_source_span(original, attribute.key.as_ref())
                .ok_or_else(|| Error::Other("TOC style source value is unavailable".to_owned()))?;
            let mut output = original[..start].to_vec();
            output.extend_from_slice(xml_escape_attribute(value).as_bytes());
            output.extend_from_slice(&original[end..]);
            return Ok(output);
        }
    }
    Err(Error::Other("TOC style has no qualified value".to_owned()))
}

/// Replace owned tab stops while keeping the source tabs root and opaque particles.
fn toc_overlay_tab_particles(
    original: &[u8],
    generated: &[u8],
    bindings: &BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let old = toc_fragment_scan(original, bindings)?;
    let canonical = BTreeMap::from([("w".to_owned(), W_NS.to_owned())]);
    let new = toc_fragment_scan(generated, &canonical)?;
    let (start, opening, closing, end, name) = old
        .property_root
        .as_ref()
        .ok_or_else(|| Error::Other("TOC tabs have no owned root".to_owned()))?;
    let (new_start, new_opening, new_closing, _, _) = new
        .property_root
        .as_ref()
        .ok_or_else(|| Error::Other("generated TOC tabs have no owned root".to_owned()))?;
    let _ = (start, new_start);
    let replacement =
        toc_close_fragment_namespaces(&generated[*new_opening..*new_closing], &canonical)?;
    let empty = *opening == *end && *closing == *start;
    let mut output = Vec::new();
    if empty {
        output.extend_from_slice(&original[..*opening - 2]);
        output.push(b'>');
    } else {
        output.extend_from_slice(&original[..*opening]);
    }
    let mut cursor = *opening;
    let mut written = false;
    for (key, range) in old.children {
        output.extend_from_slice(&original[cursor..range.start]);
        if key.as_deref() == Some("tab") {
            if !written {
                output.extend_from_slice(&replacement);
                written = true;
            }
        } else {
            output.extend_from_slice(&original[range.clone()]);
        }
        cursor = range.end;
    }
    if !written {
        output.extend_from_slice(&replacement);
    }
    if !empty {
        output.extend_from_slice(&original[cursor..*closing]);
        output.extend_from_slice(&original[*closing..*end]);
    } else {
        output.extend_from_slice(format!("</{name}>").as_bytes());
    }
    output.extend_from_slice(&original[*end..]);
    Ok(output)
}

/// Close every top-level fragment root using its original inherited namespace map.
fn toc_close_fragment_namespaces(
    fragment: &[u8],
    bindings: &BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let mut reader = quick_xml::Reader::from_reader(fragment);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut cursor = 0usize;
    let mut root = 0usize;
    let mut output = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid retained TOC fragment: {error}")))?
        {
            Event::Start(_) => {
                if depth == 0 {
                    root = before;
                }
                depth += 1;
            }
            Event::Empty(_) if depth == 0 => {
                let end = reader.buffer_position() as usize;
                output.extend_from_slice(&fragment[cursor..before]);
                output.extend_from_slice(&xml_fragment_with_namespaces(
                    &fragment[before..end],
                    bindings,
                    "TOC fragment",
                )?);
                cursor = end;
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| Error::Other("unbalanced retained TOC fragment".to_owned()))?;
                if depth == 0 {
                    let end = reader.buffer_position() as usize;
                    output.extend_from_slice(&fragment[cursor..root]);
                    output.extend_from_slice(&xml_fragment_with_namespaces(
                        &fragment[root..end],
                        bindings,
                        "TOC fragment",
                    )?);
                    cursor = end;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if depth != 0 {
        return Err(Error::Other("unclosed retained TOC fragment".to_owned()));
    }
    output.extend_from_slice(&fragment[cursor..]);
    Ok(output)
}

/// Every style identifier a TOC entry of any level may carry: the producer
/// styles named `toc 1` to `toc 9`, the canonical `TOC1` to `TOC9`, and the
/// styles this rebuild resolved.
fn toc_entry_style_ids(
    styles: &CT_Styles,
    resolved: &BTreeMap<u8, TocEntryStyle>,
) -> BTreeSet<String> {
    let mut ids = (1..=9)
        .map(|level| format!("TOC{level}"))
        .collect::<BTreeSet<_>>();
    for style in &styles.styles {
        let is_entry_name = style.name.as_deref().is_some_and(|name| {
            (1..=9).any(|level| name.trim().eq_ignore_ascii_case(&format!("toc {level}")))
        });
        if style.style_type == StyleType::Paragraph && !style.style_id.is_empty() && is_entry_name {
            ids.insert(style.style_id.clone());
        }
    }
    ids.extend(resolved.values().map(|style| style.style_id.clone()));
    ids
}

fn relocate_end_boundary_bookmark_starts(
    mut xml: Vec<u8>,
    spans: &[DynamicTocSpan],
    marker_starts: &[Vec<u8>],
) -> Result<Vec<u8>> {
    let mut edits = Vec::new();
    for (toc_index, span) in spans.iter().enumerate() {
        let markers = marker_starts.get(toc_index).ok_or_else(|| {
            Error::Other("table of contents end bookmark owner was not retained".to_owned())
        })?;
        if markers.is_empty() {
            continue;
        }
        let search_end = span
            .end_wrapper_prefixes
            .first()
            .map(|(start, _)| *start)
            .unwrap_or(span.result_end);
        if span.end_paragraph_content_start > search_end || search_end > span.end_run_end {
            return Err(Error::Other(
                "table of contents end bookmark boundaries are invalid".to_owned(),
            ));
        }
        let matches =
            byte_match_offsets(&xml[span.end_paragraph_content_start..search_end], markers);
        if matches.len() != 1 {
            return Err(Error::Other(
                "table of contents end bookmark start was not uniquely staged".to_owned(),
            ));
        }
        let start = span.end_paragraph_content_start + matches[0];
        let end = start + markers.len();
        if end > span.end_run_end {
            return Err(Error::Other(
                "table of contents end bookmark start crosses its field boundary".to_owned(),
            ));
        }
        let mut replacement = xml[end..span.end_run_end].to_vec();
        replacement.extend_from_slice(markers);
        edits.push(FieldSourceEdit {
            start,
            end: span.end_run_end,
            replacement,
        });
    }
    edits.sort_by_key(|edit| edit.start);
    if edits.windows(2).any(|pair| pair[0].end > pair[1].start) {
        return Err(Error::Other(
            "table of contents end bookmark repairs overlap".to_owned(),
        ));
    }
    for edit in edits.into_iter().rev() {
        xml.splice(edit.start..edit.end, edit.replacement);
    }
    Ok(xml)
}

pub(super) fn append_toc_wrapper_closures(output: &mut Vec<u8>, wrappers: &[String]) {
    for name in wrappers.iter().rev() {
        output.extend_from_slice(b"</");
        output.extend_from_slice(name.as_bytes());
        output.push(b'>');
    }
}

pub(super) fn reopen_staged_document(document: Document) -> Result<Document> {
    document.reopen_prepared_staged()
}

fn deterministic_toc_page_values(document: &Document) -> Result<HashMap<String, String>> {
    let layout = document.layout_deterministic()?;
    let section_formats = section_page_formats(&document.document);
    let input = document.build_layout_input()?;
    let snapshot = oxoffice_docx_layout::engine::evaluate_sequence_fields(&input)?;
    let mut output = HashMap::new();
    let mut invalid_target = None;
    for page in &layout.layout.pages {
        oxoffice_layout::walk(&page.elements, &mut |element, _| {
            if let oxoffice_layout::PositionedElement::Text(run) = element
                && let Some(oxoffice_layout::FieldKind::TargetPage(target)) = run.field_kind
                && let Some(name) = layout.page_reference_name(target)
            {
                let owner = layout.bookmark_page_section(name);
                let instruction = run
                    .field_source
                    .and_then(|source| {
                        layout
                            .source_node(source.node)
                            .and_then(|path| snapshot.source_id(path))
                            .map(|node| oxoffice_layout::FieldSource {
                                node,
                                index: source.index,
                            })
                    })
                    .and_then(|source| snapshot.source_field_context(source))
                    .map(|(raw, _, _)| Field::new(raw, "").effective_instruction())
                    .filter(|instruction| instruction.name == "PAGEREF");
                let valid_page = owner
                    .filter(|owner| (1..=layout.layout.pages.len()).contains(&owner.physical_page))
                    .zip(instruction.as_ref())
                    .is_some_and(|(owner, instruction)| {
                        oxoffice_docx_layout::engine::format_page_field_result(
                            instruction,
                            owner.displayed_page,
                            section_formats
                                .get(owner.section_index)
                                .and_then(Option::as_deref),
                        )
                        .is_ok_and(|expected| expected == run.text)
                    });
                if !valid_page
                    || output
                        .get(name)
                        .is_some_and(|existing| existing != &run.text)
                {
                    invalid_target.get_or_insert_with(|| name.to_owned());
                } else {
                    output
                        .entry(name.to_owned())
                        .or_insert_with(|| run.text.clone());
                }
            }
        });
    }
    if let Some(target) = invalid_target {
        return Err(Error::Other(format!(
            "table of contents page target {target} was not resolved"
        )));
    }
    Ok(output)
}
