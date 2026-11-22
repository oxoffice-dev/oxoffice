//! When a paragraph, table or header may be cached, what an entry retains, and fingerprints.

use super::*;

fn paragraph_source_is_cache_safe(
    paragraph: &CT_P,
    styles: &CT_Styles,
    allow_fields: bool,
    allow_bookmarks: bool,
) -> bool {
    let extra_xml_is_represented = paragraph.extra_xml.is_empty()
        || (allow_bookmarks && paragraph_bookmark_raw_is_exact(paragraph));
    if !paragraph.hyperlinks.is_empty()
        || !paragraph.comment_ranges.is_empty()
        || (!allow_bookmarks && !paragraph.bookmark_markers.is_empty())
        || !extra_xml_is_represented
        || !paragraph.content_controls.is_empty()
        || !paragraph.revisions.is_empty()
    {
        return false;
    }

    let style_id = paragraph
        .properties
        .as_ref()
        .and_then(|properties| properties.style_id.as_deref());
    let resolved = style_resolver::resolve_paragraph_properties(style_id, styles);
    if resolved.num_id.is_some()
        || paragraph.properties.as_ref().is_some_and(|properties| {
            properties.num_id.is_some()
                || properties.sect_pr.is_some()
                || properties.numbering_revision.is_some()
                || !properties.numbering_revision_xml.is_empty()
                || properties.change.is_some()
                || !properties.revision_xml.is_empty()
                || properties.rpr.as_ref().is_some_and(|rpr| {
                    !rpr.revision_markers.is_empty()
                        || rpr.change.is_some()
                        || !rpr.revision_xml.is_empty()
                        || !rpr.revision_xml_positions.is_empty()
                })
        })
    {
        return false;
    }

    paragraph.runs.iter().all(|run| {
        run.alt_drawings.is_empty()
            && run.extra_xml.is_empty()
            && run.extra_xml_positions.is_empty()
            && run.properties.as_ref().is_none_or(|rpr| {
                rpr.revision_markers.is_empty()
                    && rpr.change.is_none()
                    && rpr.revision_xml.is_empty()
                    && rpr.revision_xml_positions.is_empty()
            })
            && run.content.iter().all(|content| match content {
                RunContent::Text(_)
                | RunContent::Tab
                | RunContent::Break(..)
                | RunContent::FootnoteRef { .. }
                | RunContent::EndnoteRef { .. } => true,
                RunContent::Field(_) => allow_fields,
                _ => false,
            })
    })
}

pub(super) fn paragraph_is_restart_record_source_safe(
    paragraph: &CT_P,
    styles: &CT_Styles,
) -> bool {
    paragraph_source_is_cache_safe(paragraph, styles, true, true)
        && paragraph.runs.iter().all(|run| {
            run.properties
                .as_ref()
                .is_none_or(|properties| properties.language.is_none())
        })
}

// Drawing content is excluded by paragraph_source_is_cache_safe, including
// through table_is_cache_safe. Picture owners therefore execute fresh on these
// paths. Only the header/footer VML cache captures and replays picture owners.
pub(super) fn paragraph_is_cache_safe(paragraph: &CT_P, styles: &CT_Styles) -> bool {
    paragraph_source_is_cache_safe(paragraph, styles, false, false)
}

fn paragraph_bookmark_raw_is_exact(paragraph: &CT_P) -> bool {
    paragraph.extra_xml.len() == paragraph.bookmark_markers.len()
        && paragraph
            .extra_xml
            .iter()
            .enumerate()
            .zip(&paragraph.bookmark_markers)
            .all(|((raw_index, (run_index, raw)), marker)| {
                let raw_before = paragraph.extra_xml[..raw_index]
                    .iter()
                    .filter(|(at, _)| at == run_index)
                    .count();
                *run_index == marker.run_index()
                    && raw_before == marker.raw_before()
                    && raw_xml_is_exact_word_bookmark(raw, marker)
            })
}

fn raw_xml_is_exact_word_bookmark(raw: &[u8], marker: &BookmarkMarker) -> bool {
    let Some(start) = raw.iter().position(|byte| !byte.is_ascii_whitespace()) else {
        return false;
    };
    let end = raw
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .expect("non-empty raw XML has a final byte");
    let raw = &raw[start..=end];
    if !raw.ends_with(b"/>") {
        return false;
    }
    let Some((root_name, raw_attributes)) = raw_root_start_tag(raw) else {
        return false;
    };
    let Some(attributes) = parse_raw_attributes(raw_attributes) else {
        return false;
    };
    let expected_local_name = if marker.is_start() {
        b"bookmarkStart".as_slice()
    } else {
        b"bookmarkEnd".as_slice()
    };
    if xml_local_name(root_name) != expected_local_name
        || !raw_name_has_namespace(
            root_name,
            &attributes,
            oxoffice_docx_oxml::namespace::W_NS,
            false,
        )
    {
        return false;
    }
    let word_attribute_count = |local: &[u8]| {
        attributes
            .iter()
            .filter(|(name, _)| {
                xml_local_name(name) == local
                    && raw_name_has_namespace(
                        name,
                        &attributes,
                        oxoffice_docx_oxml::namespace::W_NS,
                        true,
                    )
            })
            .count()
    };
    if word_attribute_count(b"id") != 1
        || word_attribute_count(b"name") != usize::from(marker.is_start())
    {
        return false;
    }

    let mut document_xml = Vec::with_capacity(raw.len() + 160);
    document_xml.extend_from_slice(
        br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p>"#,
    );
    document_xml.extend_from_slice(raw);
    document_xml.extend_from_slice(b"</w:p></w:body></w:document>");
    let Ok(document) = CT_Document::from_xml(&document_xml) else {
        return false;
    };
    let [BodyContent::Paragraph(parsed)] = document.body.content.as_slice() else {
        return false;
    };
    let [(run_index, parsed_raw)] = parsed.extra_xml.as_slice() else {
        return false;
    };
    let [parsed_marker] = parsed.bookmark_markers.as_slice() else {
        return false;
    };
    parsed.properties.is_none()
        && parsed.runs.is_empty()
        && parsed.hyperlinks.is_empty()
        && parsed.comment_ranges.is_empty()
        && parsed.content_controls.is_empty()
        && parsed.revisions.is_empty()
        && *run_index == 0
        && parsed_raw == raw
        && parsed_marker.raw_before() == 0
        && parsed_marker.is_start() == marker.is_start()
        && parsed_marker.id().is_some()
        && parsed_marker.id() == marker.id()
        && parsed_marker.name() == marker.name()
        && if parsed_marker.is_start() {
            parsed_marker.name().is_some()
        } else {
            parsed_marker.name().is_none()
        }
}

pub(super) fn paragraph_has_field(paragraph: &CT_P) -> bool {
    paragraph
        .runs
        .iter()
        .flat_map(|run| &run.content)
        .any(|content| matches!(content, RunContent::Field(_)))
}

fn paragraph_has_note_reference(paragraph: &CT_P) -> bool {
    paragraph.runs.iter().any(|run| {
        run.content.iter().any(|content| {
            !content_note_references(content, RevisionView::Tracked)
                .is_ok_and(|references| references.is_empty())
        })
    })
}

pub(super) fn header_footer_part_is_cache_safe(
    part: &oxoffice_docx_oxml::header_footer::CT_HdrFtr,
    styles: &CT_Styles,
) -> bool {
    if !part.extra_xml.is_empty() {
        return false;
    }
    let raw_watermark_count = part
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.runs)
        .flat_map(|run| &run.extra_xml)
        .filter(|raw| raw_xml_root_is_word_pict(raw, &part.extra_namespaces))
        .count();
    if raw_watermark_count != part.watermarks().len() {
        return false;
    }
    part.paragraphs.iter().all(|paragraph| {
        if !paragraph.extra_xml.is_empty() {
            return false;
        }
        let mut projected = paragraph.clone();
        for run in &mut projected.runs {
            if run.extra_xml.len() != run.extra_xml_positions.len()
                || !run
                    .extra_xml
                    .iter()
                    .all(|raw| raw_xml_root_is_word_pict(raw, &part.extra_namespaces))
            {
                return false;
            }
            run.extra_xml.clear();
            run.extra_xml_positions.clear();
        }
        !paragraph_has_note_reference(&projected) && paragraph_is_cache_safe(&projected, styles)
    })
}

fn raw_xml_root_is_word_pict(raw: &[u8], namespaces: &[(String, String)]) -> bool {
    let raw = raw
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map_or(raw, |start| &raw[start..]);
    let Some((name, attributes)) = raw.strip_prefix(b"<").and_then(|raw| {
        let name_end = raw
            .iter()
            .position(|byte| byte.is_ascii_whitespace() || matches!(byte, b'>' | b'/'))?;
        Some((&raw[..name_end], &raw[name_end..]))
    }) else {
        return false;
    };
    let mut components = name.rsplitn(2, |byte| *byte == b':');
    if components.next() != Some(b"pict".as_slice()) {
        return false;
    }
    let prefix = components.next();
    let declaration = prefix.map_or_else(
        || "xmlns".to_owned(),
        |prefix| format!("xmlns:{}", String::from_utf8_lossy(prefix)),
    );
    if let Some(namespace) = raw_xml_start_attribute(attributes, declaration.as_bytes()) {
        return namespace == oxoffice_docx_oxml::namespace::W_NS.as_bytes();
    }
    let Some(prefix) = prefix.and_then(|prefix| std::str::from_utf8(prefix).ok()) else {
        return false;
    };
    if prefix == "w" {
        return !namespaces.iter().any(|(name, namespace)| {
            name != "xmlns:w" && namespace == oxoffice_docx_oxml::namespace::W_NS
        });
    }
    namespaces.iter().any(|(name, namespace)| {
        name.strip_prefix("xmlns:") == Some(prefix)
            && namespace == oxoffice_docx_oxml::namespace::W_NS
    })
}

fn raw_xml_start_attribute<'a>(mut input: &'a [u8], expected: &[u8]) -> Option<&'a [u8]> {
    loop {
        input = input
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
            .map_or(input, |start| &input[start..]);
        if input.first().is_none_or(|byte| matches!(byte, b'>' | b'/')) {
            return None;
        }
        let name_end = input
            .iter()
            .position(|byte| byte.is_ascii_whitespace() || matches!(byte, b'=' | b'>' | b'/'))?;
        let name = &input[..name_end];
        input = &input[name_end..];
        input = input
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
            .map_or(input, |start| &input[start..]);
        if input.first() != Some(&b'=') {
            return None;
        }
        input = &input[1..];
        input = input
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
            .map_or(input, |start| &input[start..]);
        let quote = *input.first()?;
        if !matches!(quote, b'\'' | b'"') {
            return None;
        }
        input = &input[1..];
        let value_end = input.iter().position(|byte| *byte == quote)?;
        let value = &input[..value_end];
        input = &input[value_end + 1..];
        if name == expected {
            return Some(value);
        }
    }
}

pub(super) fn header_footer_section_is_cache_safe(section: &CT_SectPr) -> bool {
    section.change.is_none()
        && section.extra_xml.len() == section.extra_xml_positions.len()
        && section
            .extra_xml
            .iter()
            .zip(&section.extra_xml_positions)
            .all(|(raw, position)| CT_SectPr::raw_position_is_root_attributes(position, raw))
}

pub(super) fn table_is_cache_safe(table: &CT_Tbl, styles: &CT_Styles) -> bool {
    table.extra_xml.is_empty()
        && table.content_controls.is_empty()
        && table.properties.as_ref().is_none_or(|properties| {
            properties.change.is_none() && properties.revision_xml.is_empty()
        })
        && table.rows.iter().all(|row| {
            row.extra_xml
                .iter()
                .all(|(position, raw)| CT_Row::raw_is_root_attributes(*position, raw))
                && row.content_controls.is_empty()
                // A row the accepted view leaves out has no block row to
                // pair with on a cache hit.
                && !row.accepted_view_removes()
                && row.properties.as_ref().is_none_or(|properties| {
                    properties.revision_markers.is_empty() && properties.revision_xml.is_empty()
                })
                && row.cells.iter().all(|cell| {
                    cell.extra_xml.is_empty()
                        && cell
                            .properties
                            .as_ref()
                            .is_none_or(|properties| properties.extra_xml.is_empty())
                        && cell.content.iter().all(|content| match content {
                            CellContent::Paragraph(paragraph) => {
                                !paragraph_has_note_reference(paragraph)
                                    && paragraph_is_cache_safe(paragraph, styles)
                            }
                            CellContent::Table(table) => table_is_cache_safe(table, styles),
                            CellContent::ContentControl(_) => false,
                        })
                })
        })
}

/// Whether a paragraph of the table takes a bookmark marker written between
/// blocks: one before the table belongs to its first paragraph, one after it
/// to its last, which a retained table block would not record.
pub(super) fn table_takes_block_markers(table: &CT_Tbl, block_bookmarks: &BlockBookmarks) -> bool {
    !block_bookmarks.is_empty()
        && table.rows.iter().flat_map(|row| &row.cells).any(|cell| {
            cell.content.iter().any(|content| match content {
                CellContent::Paragraph(paragraph) => {
                    let (before, after) = block_bookmarks.markers(paragraph);
                    !before.is_empty() || !after.is_empty()
                }
                CellContent::Table(table) => table_takes_block_markers(table, block_bookmarks),
                // A control already makes the table unsafe to retain.
                CellContent::ContentControl(_) => false,
            })
        })
}

pub(super) fn table_semantics(
    table: &CT_Tbl,
    block: &table::TableBlock,
    retained: &TableSemantics,
    sources: Option<&SourceRegistry>,
    story: &WordStory,
    table_path: &[usize],
) -> TableSemantics {
    let rows = table
        .rows
        .iter()
        .zip(&block.rows)
        .zip(&retained.rows)
        .enumerate()
        .map(|(row_index, ((row, block_row), retained_row))| {
            let cells = row
                .cells
                .iter()
                .zip(&block_row.cells)
                .zip(&retained_row.cells)
                .enumerate()
                .map(|(cell_index, ((cell, block_cell), retained_cell))| {
                    let blocks = cell
                        .content
                        .iter()
                        .zip(&block_cell.blocks)
                        .zip(&retained_cell.blocks)
                        .enumerate()
                        .map(|(content_index, ((content, block_item), retained_item))| {
                            let mut source_path = table_path.to_vec();
                            source_path.extend([row_index, cell_index, content_index]);
                            match (content, block_item, retained_item) {
                                (
                                    CellContent::Paragraph(_),
                                    table::CellBlock::Paragraph(_),
                                    CellBlockSemantics::Paragraph(retained),
                                ) => CellBlockSemantics::Paragraph(ParagraphSemantics {
                                    paint: Arc::clone(&retained.paint),
                                    tb_rl_exact_baseline: None,
                                    border_group_before: retained.border_group_before,
                                    source_node: sources
                                        .and_then(|sources| sources.id(story, &source_path)),
                                    structure_id: None,
                                    reflow_direction: retained.reflow_direction,
                                }),
                                (
                                    CellContent::Table(table),
                                    table::CellBlock::Table(block),
                                    CellBlockSemantics::Table(retained),
                                ) => CellBlockSemantics::Table(table_semantics(
                                    table,
                                    block,
                                    retained,
                                    sources,
                                    story,
                                    &source_path,
                                )),
                                _ => unreachable!("cache-safe table topology stays aligned"),
                            }
                        })
                        .collect();
                    block::CellSemantics { blocks }
                })
                .collect();
            block::RowSemantics { cells }
        })
        .collect();
    TableSemantics { rows }
}

pub(super) fn canonicalize_table_sources(block: &mut table::TableBlock) -> Result<()> {
    for row in &mut block.rows {
        for cell in &mut row.cells {
            for block in &mut cell.blocks {
                match block {
                    table::CellBlock::Paragraph(paragraph) => {
                        rebind_paragraph_source(paragraph, Some(CACHE_SOURCE_NODE))?;
                    }
                    table::CellBlock::Table(table) => canonicalize_table_sources(table)?,
                }
            }
        }
    }
    Ok(())
}

pub(super) fn font_contribution_trace_bytes(trace: &Vec<FontTraceEvent>) -> usize {
    trace.iter().fold(
        trace
            .capacity()
            .saturating_mul(std::mem::size_of::<FontTraceEvent>()),
        |total, event| {
            total.saturating_add(match event {
                FontTraceEvent::Resolution(_) => 0,
                FontTraceEvent::Contribution(contribution) => contribution
                    .requested_family
                    .as_ref()
                    .map_or(0, String::capacity),
            })
        },
    )
}

pub(super) fn table_cache_entry_bytes(
    key: &TableCacheKey,
    block: &table::TableBlock,
    semantics: &TableSemantics,
    diagnostics: &[Diagnostic],
    font_trace_bytes: usize,
) -> usize {
    let diagnostic_bytes = diagnostics
        .len()
        .saturating_mul(std::mem::size_of::<Diagnostic>())
        .saturating_add(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.capacity())
                .fold(0usize, usize::saturating_add),
        );
    std::mem::size_of::<TableCacheEntry>()
        .saturating_add(table_key_retained_bytes(&key.table))
        .saturating_add(table_block_retained_bytes(block, Some(semantics)))
        .saturating_add(table_semantics_retained_bytes(semantics))
        .saturating_add(2 * std::mem::size_of::<usize>())
        .saturating_add(font_trace_bytes)
        .saturating_add(diagnostic_bytes)
}

fn table_semantics_retained_bytes(semantics: &TableSemantics) -> usize {
    let mut bytes = semantics
        .rows
        .capacity()
        .saturating_mul(std::mem::size_of::<block::RowSemantics>());
    for row in &semantics.rows {
        bytes = bytes.saturating_add(
            row.cells
                .capacity()
                .saturating_mul(std::mem::size_of::<block::CellSemantics>()),
        );
        for cell in &row.cells {
            bytes = bytes.saturating_add(
                cell.blocks
                    .capacity()
                    .saturating_mul(std::mem::size_of::<CellBlockSemantics>()),
            );
            for block in &cell.blocks {
                match block {
                    CellBlockSemantics::Table(table) => {
                        bytes = bytes.saturating_add(table_semantics_retained_bytes(table))
                    }
                    CellBlockSemantics::Paragraph(paragraph) => {
                        bytes = bytes.saturating_add(word_paint_arc_bytes(&paragraph.paint))
                    }
                }
            }
        }
    }
    bytes
}

/// Heap bytes held by a retained ordered attribute vector.
fn attribute_pairs_bytes(values: &[(String, String)]) -> usize {
    values
        .len()
        .saturating_mul(std::mem::size_of::<(String, String)>())
        .saturating_add(
            values
                .iter()
                .map(|(name, value)| name.capacity().saturating_add(value.capacity()))
                .fold(0usize, usize::saturating_add),
        )
}

fn paragraph_key_retained_bytes(paragraph: &CT_P) -> usize {
    fn option_string_bytes(value: &Option<String>) -> usize {
        value.as_ref().map_or(0, String::capacity)
    }
    fn raw_vectors_bytes(values: &[Vec<u8>]) -> usize {
        values
            .len()
            .saturating_mul(std::mem::size_of::<Vec<u8>>())
            .saturating_add(
                values
                    .iter()
                    .map(Vec::capacity)
                    .fold(0usize, usize::saturating_add),
            )
    }
    fn run_properties_bytes(properties: &CT_RPr) -> usize {
        [
            &properties.style_id,
            &properties.font_ascii,
            &properties.font_hansi,
            &properties.font_east_asia,
            &properties.font_cs,
            &properties.font_ascii_theme,
            &properties.font_hansi_theme,
            &properties.font_east_asia_theme,
            &properties.font_cs_theme,
            &properties.font_hint,
            &properties.color,
            &properties.color_theme,
            &properties.vert_align,
        ]
        .into_iter()
        .map(option_string_bytes)
        .fold(0usize, usize::saturating_add)
        .saturating_add(attribute_pairs_bytes(&properties.font_extra_attributes))
        .saturating_add(attribute_pairs_bytes(&properties.color_extra_attributes))
        .saturating_add(
            properties
                .shading
                .as_ref()
                .map_or(0, |shading| shading_bytes(shading)),
        )
        .saturating_add(
            properties
                .revision_markers
                .capacity()
                .saturating_mul(std::mem::size_of::<CT_Revision>()),
        )
        .saturating_add(raw_vectors_bytes(&properties.revision_xml))
        .saturating_add(
            properties
                .revision_xml_positions
                .capacity()
                .saturating_mul(std::mem::size_of::<(u8, usize)>()),
        )
    }
    fn shading_bytes(shading: &CT_Shd) -> usize {
        shading
            .val
            .capacity()
            .saturating_add(option_string_bytes(&shading.color))
            .saturating_add(option_string_bytes(&shading.fill))
            .saturating_add(option_string_bytes(&shading.theme_color))
            .saturating_add(option_string_bytes(&shading.theme_fill))
            .saturating_add(attribute_pairs_bytes(&shading.extra_attributes))
    }
    fn paragraph_border_bytes(borders: &CT_PBdr) -> usize {
        [
            &borders.top,
            &borders.bottom,
            &borders.left,
            &borders.right,
            &borders.between,
            &borders.bar,
        ]
        .into_iter()
        .filter_map(Option::as_ref)
        .map(|edge| option_string_bytes(&edge.color))
        .fold(0usize, usize::saturating_add)
    }
    fn paragraph_properties_bytes(properties: &CT_PPr) -> usize {
        option_string_bytes(&properties.style_id)
            .saturating_add(option_string_bytes(&properties.line_rule))
            .saturating_add(
                properties
                    .borders
                    .as_ref()
                    .map_or(0, |borders| paragraph_border_bytes(borders)),
            )
            .saturating_add(properties.tabs.as_ref().map_or(0, |tabs| {
                tabs.tabs
                    .capacity()
                    .saturating_mul(std::mem::size_of::<CT_TabStop>())
            }))
            .saturating_add(
                properties
                    .shading
                    .as_ref()
                    .map_or(0, |shading| shading_bytes(shading)),
            )
            .saturating_add(properties.rpr.as_ref().map_or(0, run_properties_bytes))
            .saturating_add(raw_vectors_bytes(&properties.numbering_revision_xml))
            .saturating_add(raw_vectors_bytes(&properties.revision_xml))
    }

    let run_bytes = paragraph
        .runs
        .capacity()
        .saturating_mul(std::mem::size_of::<CT_R>())
        .saturating_add(
            paragraph
                .runs
                .iter()
                .map(|run| {
                    run.content
                        .capacity()
                        .saturating_mul(std::mem::size_of::<RunContent>())
                        .saturating_add(
                            run.content
                                .iter()
                                .map(|content| match content {
                                    RunContent::Text(text) | RunContent::DeletedText(text) => {
                                        text.text.capacity()
                                    }
                                    RunContent::Symbol { font, .. } => font.capacity(),
                                    RunContent::Tab
                                    | RunContent::Break(..)
                                    | RunContent::Drawing(_)
                                    | RunContent::Field(_)
                                    | RunContent::FootnoteRef { .. }
                                    | RunContent::EndnoteRef { .. }
                                    | RunContent::CommentReference { .. }
                                    | RunContent::SpecialCharacter(_) => 0,
                                })
                                .fold(0usize, usize::saturating_add),
                        )
                        .saturating_add(run.properties.as_ref().map_or(0, run_properties_bytes))
                        .saturating_add(raw_vectors_bytes(&run.extra_xml))
                        .saturating_add(
                            run.extra_xml_positions
                                .capacity()
                                .saturating_mul(std::mem::size_of::<usize>()),
                        )
                })
                .fold(0usize, usize::saturating_add),
        );
    let paragraph_vectors =
        paragraph
            .hyperlinks
            .capacity()
            .saturating_mul(std::mem::size_of::<oxoffice_docx_oxml::text::HyperlinkSpan>())
            .saturating_add(paragraph.comment_ranges.capacity().saturating_mul(
                std::mem::size_of::<oxoffice_docx_oxml::text::CommentRangeMarker>(),
            ))
            .saturating_add(
                paragraph
                    .extra_xml
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(usize, Vec<u8>)>()),
            )
            .saturating_add(
                paragraph
                    .extra_xml
                    .iter()
                    .map(|(_, raw)| raw.capacity())
                    .fold(0usize, usize::saturating_add),
            );
    run_bytes.saturating_add(paragraph_vectors).saturating_add(
        paragraph
            .properties
            .as_ref()
            .map_or(0, paragraph_properties_bytes),
    )
}

fn table_key_retained_bytes(table: &CT_Tbl) -> usize {
    fn option_string_bytes(value: &Option<String>) -> usize {
        value.as_ref().map_or(0, String::capacity)
    }
    fn raw_entries_bytes(values: &[(usize, Vec<u8>)], capacity: usize) -> usize {
        capacity
            .saturating_mul(std::mem::size_of::<(usize, Vec<u8>)>())
            .saturating_add(
                values
                    .iter()
                    .map(|(_, raw)| raw.capacity())
                    .fold(0usize, usize::saturating_add),
            )
    }
    fn raw_vectors_bytes(values: &[Vec<u8>]) -> usize {
        values
            .len()
            .saturating_mul(std::mem::size_of::<Vec<u8>>())
            .saturating_add(
                values
                    .iter()
                    .map(Vec::capacity)
                    .fold(0usize, usize::saturating_add),
            )
    }
    fn shading_bytes(shading: &CT_Shd) -> usize {
        shading
            .val
            .capacity()
            .saturating_add(option_string_bytes(&shading.color))
            .saturating_add(option_string_bytes(&shading.fill))
            .saturating_add(option_string_bytes(&shading.theme_color))
            .saturating_add(option_string_bytes(&shading.theme_fill))
            .saturating_add(attribute_pairs_bytes(&shading.extra_attributes))
    }
    fn table_border_bytes(borders: &oxoffice_docx_oxml::table::CT_TblBorders) -> usize {
        [
            &borders.top,
            &borders.bottom,
            &borders.left,
            &borders.right,
            &borders.inside_h,
            &borders.inside_v,
        ]
        .into_iter()
        .filter_map(Option::as_ref)
        .map(|edge| option_string_bytes(&edge.color))
        .fold(0usize, usize::saturating_add)
    }
    fn width_bytes(width: &oxoffice_docx_oxml::table::CT_TblWidth) -> usize {
        width.width_type.capacity()
    }
    fn table_properties_bytes(properties: &oxoffice_docx_oxml::table::CT_TblPr) -> usize {
        option_string_bytes(&properties.style_id)
            .saturating_add(option_string_bytes(&properties.layout))
            .saturating_add(properties.width.as_ref().map_or(0, width_bytes))
            .saturating_add(properties.indent.as_ref().map_or(0, width_bytes))
            .saturating_add(properties.borders.as_ref().map_or(0, table_border_bytes))
            .saturating_add(properties.shading.as_ref().map_or(0, shading_bytes))
            .saturating_add(
                properties
                    .look
                    .as_ref()
                    .map_or(0, |look| option_string_bytes(&look.val)),
            )
            .saturating_add(raw_vectors_bytes(&properties.revision_xml))
    }
    fn row_properties_bytes(properties: &oxoffice_docx_oxml::table::CT_TrPr) -> usize {
        option_string_bytes(&properties.height_rule)
            .saturating_add(option_string_bytes(&properties.cnf_style))
            .saturating_add(
                properties
                    .revision_markers
                    .capacity()
                    .saturating_mul(std::mem::size_of::<CT_Revision>()),
            )
            .saturating_add(raw_vectors_bytes(&properties.revision_xml))
    }
    fn cell_properties_bytes(properties: &oxoffice_docx_oxml::table::CT_TcPr) -> usize {
        properties
            .width
            .as_ref()
            .map_or(0, width_bytes)
            .saturating_add(properties.borders.as_ref().map_or(0, table_border_bytes))
            .saturating_add(properties.shading.as_ref().map_or(0, shading_bytes))
            .saturating_add(option_string_bytes(&properties.text_direction))
            .saturating_add(option_string_bytes(&properties.cnf_style))
            .saturating_add(raw_entries_bytes(
                &properties.extra_xml,
                properties.extra_xml.capacity(),
            ))
    }

    let grid_bytes = table.grid.as_ref().map_or(0, |grid| {
        grid.columns
            .capacity()
            .saturating_mul(std::mem::size_of::<oxoffice_docx_oxml::table::CT_TblGridCol>())
    });
    table
        .rows
        .capacity()
        .saturating_mul(std::mem::size_of::<CT_Row>())
        .saturating_add(grid_bytes)
        .saturating_add(
            table
                .rows
                .iter()
                .map(|row| {
                    row.properties
                        .as_ref()
                        .map_or(0, row_properties_bytes)
                        .saturating_add(raw_entries_bytes(&row.extra_xml, row.extra_xml.capacity()))
                        .saturating_add(
                            row.content_controls
                                .capacity()
                                .saturating_mul(std::mem::size_of::<(usize, usize, CT_Sdt)>()),
                        )
                        .saturating_add(
                            row.cells
                                .capacity()
                                .saturating_mul(std::mem::size_of::<CT_Tc>())
                                .saturating_add(
                                    row.cells
                                        .iter()
                                        .map(|cell| {
                                            cell.properties
                                        .as_ref()
                                        .map_or(0, cell_properties_bytes)
                                        .saturating_add(raw_entries_bytes(
                                            &cell.extra_xml,
                                            cell.extra_xml.capacity(),
                                        ))
                                        .saturating_add(cell.content
                                        .capacity()
                                        .saturating_mul(std::mem::size_of::<CellContent>())
                                        .saturating_add(
                                            cell.content
                                                .iter()
                                                .map(|content| match content {
                                                    CellContent::Paragraph(paragraph) => {
                                                        paragraph_key_retained_bytes(paragraph)
                                                    }
                                                    CellContent::Table(table) => {
                                                        table_key_retained_bytes(table)
                                                    }
                                                    CellContent::ContentControl(_) => usize::MAX,
                                                })
                                                .fold(0usize, usize::saturating_add),
                                        ))
                                        })
                                        .fold(0usize, usize::saturating_add),
                                ),
                        )
                })
                .fold(0usize, usize::saturating_add),
        )
        .saturating_add(table.properties.as_ref().map_or(0, table_properties_bytes))
        .saturating_add(raw_entries_bytes(
            &table.extra_xml,
            table.extra_xml.capacity(),
        ))
        .saturating_add(
            table
                .content_controls
                .capacity()
                .saturating_mul(std::mem::size_of::<(usize, usize, CT_Sdt)>()),
        )
}

fn table_block_retained_bytes(
    block: &table::TableBlock,
    semantics: Option<&TableSemantics>,
) -> usize {
    fn border_bytes(borders: &oxoffice_docx_oxml::table::CT_TblBorders) -> usize {
        [
            &borders.top,
            &borders.bottom,
            &borders.left,
            &borders.right,
            &borders.inside_h,
            &borders.inside_v,
        ]
        .into_iter()
        .map(|edge| {
            edge.as_ref()
                .and_then(|edge| edge.color.as_ref())
                .map_or(0, String::capacity)
        })
        .fold(0usize, usize::saturating_add)
    }

    let rows = block
        .rows
        .iter()
        .enumerate()
        .map(|(row_index, row)| {
            row.cells
                .capacity()
                .saturating_mul(std::mem::size_of::<table::TableCell>())
                .saturating_add(
                    row.cells
                        .iter()
                        .enumerate()
                        .map(|(cell_index, cell)| {
                            cell.blocks
                                .capacity()
                                .saturating_mul(std::mem::size_of::<table::CellBlock>())
                                .saturating_add(
                                    cell.blocks
                                        .iter()
                                        .enumerate()
                                        .map(|(block_index, block)| {
                                            let companion = semantics
                                                .and_then(|semantics| semantics.rows.get(row_index))
                                                .and_then(|row| row.cells.get(cell_index))
                                                .and_then(|cell| cell.blocks.get(block_index));
                                            match block {
                                                table::CellBlock::Paragraph(paragraph) => {
                                                    paragraph_cache_entry_bytes(
                                                        &CT_P::new(),
                                                        paragraph,
                                                        match companion {
                                                            Some(
                                                                CellBlockSemantics::Paragraph(
                                                                    semantics,
                                                                ),
                                                            ) => Some(semantics.paint.as_ref()),
                                                            _ => None,
                                                        },
                                                        &[],
                                                        0,
                                                    )
                                                }
                                                table::CellBlock::Table(table) => {
                                                    table_block_retained_bytes(
                                                        table,
                                                        match companion {
                                                            Some(CellBlockSemantics::Table(
                                                                semantics,
                                                            )) => Some(semantics),
                                                            _ => None,
                                                        },
                                                    )
                                                }
                                            }
                                        })
                                        .fold(0usize, usize::saturating_add),
                                )
                                .saturating_add(cell.borders.as_ref().map_or(0, border_bytes))
                        })
                        .fold(0usize, usize::saturating_add),
                )
        })
        .fold(0usize, usize::saturating_add);
    std::mem::size_of::<table::TableBlock>()
        .saturating_add(
            block
                .col_widths
                .capacity()
                .saturating_mul(std::mem::size_of::<f64>()),
        )
        .saturating_add(
            block
                .rows
                .capacity()
                .saturating_mul(std::mem::size_of::<table::TableRow>()),
        )
        .saturating_add(
            block
                .header_row_indices
                .capacity()
                .saturating_mul(std::mem::size_of::<usize>()),
        )
        .saturating_add(rows)
        .saturating_add(block.borders.as_ref().map_or(0, border_bytes))
}

pub(super) fn canonicalize_layout_fonts(
    pages: &mut [Arc<PageFrame>],
    font_manager: &FontManager,
    current_fonts: &[FontId],
) -> Result<Vec<oxoffice_layout::FontData>> {
    fn collect(
        elements: &[PositionedElement],
        remap: &mut HashMap<FontId, FontId>,
        order: &mut Vec<FontId>,
    ) {
        for element in elements {
            match element {
                PositionedElement::Text(run) => {
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        remap.entry(run.font_id)
                    {
                        let local = FontId(order.len() as u32);
                        entry.insert(local);
                        order.push(run.font_id);
                    }
                }
                PositionedElement::MultilingualText(run) => {
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        remap.entry(run.font_id)
                    {
                        let local = FontId(order.len() as u32);
                        entry.insert(local);
                        order.push(run.font_id);
                    }
                }
                PositionedElement::Group(group) => collect(&group.children, remap, order),
                PositionedElement::MarkedContent { children, .. } => {
                    collect(children, remap, order)
                }
                _ => {}
            }
        }
    }

    fn rewrite(elements: &mut [PositionedElement], remap: &HashMap<FontId, FontId>) {
        for element in elements {
            match element {
                PositionedElement::Text(run) => {
                    run.font_id = remap[&run.font_id];
                }
                PositionedElement::MultilingualText(run) => {
                    run.font_id = remap[&run.font_id];
                }
                PositionedElement::Group(group) => rewrite(&mut group.children, remap),
                PositionedElement::MarkedContent { children, .. } => rewrite(children, remap),
                _ => {}
            }
        }
    }

    let mut remap = HashMap::new();
    let mut order = Vec::with_capacity(current_fonts.len());
    for &font_id in current_fonts {
        if let std::collections::hash_map::Entry::Vacant(entry) = remap.entry(font_id) {
            let local = FontId(order.len() as u32);
            entry.insert(local);
            order.push(font_id);
        }
    }
    for page in pages.iter() {
        collect(&page.elements, &mut remap, &mut order);
    }
    let mut fonts = Vec::with_capacity(order.len());
    for persistent_id in order {
        let mut font = font_manager.font_data(persistent_id)?;
        font.id = remap[&persistent_id];
        fonts.push(font);
    }
    for page in pages {
        rewrite(&mut Arc::make_mut(page).elements, &remap);
    }
    Ok(fonts)
}

pub(super) fn restart_block_is_safe<B: LayoutBlockLike>(block: &B) -> bool {
    if let Some(paragraph) = block.paragraph() {
        restart_record_block_is_safe(block)
            && paragraph.lines.iter().all(|line| {
                line.items.iter().all(|item| match item {
                    LineItem::Text(text) | LineItem::Marker(text) => {
                        text.field_kind.is_none() && text.field_source.is_none()
                    }
                    LineItem::MultilingualText(text) => {
                        text.base().field_kind.is_none() && text.base().field_source.is_none()
                    }
                    LineItem::Tab {
                        leader: Some(text), ..
                    } => text.field_kind.is_none() && text.field_source.is_none(),
                    _ => true,
                })
            })
    } else {
        restart_record_block_is_safe(block)
    }
}

pub(super) fn restart_record_block_is_safe<B: LayoutBlockLike>(block: &B) -> bool {
    if let Some(paragraph) = block.paragraph() {
        paragraph.anchored.is_empty()
            && paragraph.lines.iter().all(|line| {
                line.items.iter().all(|item| match item {
                    LineItem::Text(_) | LineItem::Marker(_) => true,
                    LineItem::MultilingualText(span) => paragraph
                        .paint
                        .or_else(|| {
                            paragraph
                                .semantics
                                .map(|semantics| semantics.paint.as_ref())
                        })
                        .and_then(|paint| paint.legacy_cache.as_ref())
                        .is_some_and(|proof| {
                            multilingual_cache_bytes(span, Some(proof)) != usize::MAX
                        }),
                    LineItem::Tab {
                        leader: Some(_), ..
                    } => true,
                    LineItem::Tab { leader: None, .. } => true,
                    LineItem::Image { .. } | LineItem::Group { .. } => false,
                    _ => false,
                })
            })
    } else {
        block.table().is_some_and(|table| {
            table_block_retained_bytes(table.block, table.semantics) != usize::MAX
        })
    }
}

pub(super) fn page_has_substitution_state(page: &PageFrame) -> bool {
    let mut found = false;
    oxoffice_layout::walk(&page.elements, &mut |element, _| {
        found |= match element {
            PositionedElement::Text(run) => run.field_kind.is_some() || run.field_source.is_some(),
            PositionedElement::MultilingualText(run) => {
                run.field_kind.is_some() || run.field_source.is_some()
            }
            _ => false,
        };
    });
    found
}

pub(super) fn restart_cache_entries(cache: &RestartCache) -> usize {
    cache.raw_pages.len().max(cache.checkpoints.len())
}

pub(super) fn restart_cache_bytes(cache: &RestartCache) -> usize {
    let vector_bytes = cache
        .body
        .capacity()
        .saturating_mul(std::mem::size_of::<RestartBodyEntry>())
        .saturating_add(
            cache
                .body_fragments
                .capacity()
                .saturating_mul(std::mem::size_of::<Vec<WordBodyLayoutFragment>>()),
        )
        .saturating_add(
            cache
                .body_fragments
                .iter()
                .map(|fragments| {
                    fragments
                        .capacity()
                        .saturating_mul(std::mem::size_of::<WordBodyLayoutFragment>())
                })
                .fold(0usize, usize::saturating_add),
        )
        .saturating_add(
            cache
                .raw_pages
                .capacity()
                .saturating_add(cache.pages.capacity())
                .saturating_mul(std::mem::size_of::<Arc<PageFrame>>()),
        )
        .saturating_add(
            cache
                .substitution_inputs
                .capacity()
                .saturating_mul(std::mem::size_of::<Option<FieldSubstitutionInputs>>()),
        )
        .saturating_add(
            cache
                .outlines
                .capacity()
                .saturating_mul(std::mem::size_of::<oxoffice_layout::OutlineEntry>()),
        )
        .saturating_add(
            cache
                .checkpoints
                .capacity()
                .saturating_mul(std::mem::size_of::<paginator::PaginationCheckpoint>()),
        )
        .saturating_add(
            cache
                .font_trace
                .capacity()
                .saturating_mul(std::mem::size_of::<FontId>()),
        );
    let paint_bytes = cache
        .paint
        .capacity()
        .saturating_mul(std::mem::size_of::<RestartPaint>())
        .saturating_add(
            cache
                .paint
                .iter()
                .map(|paint| match paint {
                    RestartPaint::Paragraph(paint) => word_paint_arc_bytes(paint),
                    RestartPaint::Table(semantics) => table_semantics_retained_bytes(semantics),
                })
                .fold(0usize, usize::saturating_add),
        );
    let policy_bytes = [&cache.raw_policies, &cache.policies]
        .iter()
        .map(|maps| {
            maps.capacity()
                .saturating_mul(std::mem::size_of::<PaintPolicies>())
                .saturating_add(
                    maps.iter()
                        .map(|map| {
                            map.retained_bytes()
                                .and_then(|bytes| {
                                    bytes.checked_sub(std::mem::size_of::<PaintPolicies>())
                                })
                                .unwrap_or(usize::MAX)
                        })
                        .fold(0usize, usize::saturating_add),
                )
        })
        .fold(0usize, usize::saturating_add);
    let body_bytes = paint_bytes.saturating_add(policy_bytes).saturating_add(
        cache
            .body
            .iter()
            .map(RestartBodyEntry::bytes)
            .fold(0usize, usize::saturating_add),
    );
    debug_assert_eq!(cache.raw_pages.len(), cache.pages.len());
    let page_bytes = cache
        .raw_pages
        .iter()
        .zip(&cache.pages)
        .map(|(pristine, substituted)| {
            let substituted_bytes = if Arc::ptr_eq(pristine, substituted) {
                0
            } else {
                page_frame_retained_bytes(substituted)
            };
            page_frame_retained_bytes(pristine)
                .saturating_add(2 * std::mem::size_of::<usize>())
                .saturating_add(substituted_bytes)
                .saturating_add(if Arc::ptr_eq(pristine, substituted) {
                    0
                } else {
                    2 * std::mem::size_of::<usize>()
                })
        })
        .fold(0usize, usize::saturating_add);
    let outline_bytes = cache
        .outlines
        .iter()
        .map(|outline| outline.title.capacity())
        .fold(0usize, usize::saturating_add);
    let substitution_bytes = cache
        .substitution_inputs
        .iter()
        .filter_map(Option::as_ref)
        .map(|inputs| {
            inputs
                .bookmark_pages
                .capacity()
                .saturating_mul(std::mem::size_of::<(usize, usize)>())
                .saturating_add(
                    inputs
                        .font_identity
                        .capacity()
                        .saturating_mul(std::mem::size_of::<FontId>()),
                )
                .saturating_add(inputs.page_format_context.capacity())
        })
        .fold(0usize, usize::saturating_add);
    std::mem::size_of::<RestartCache>()
        .saturating_add(vector_bytes)
        .saturating_add(body_bytes)
        .saturating_add(page_bytes)
        .saturating_add(substitution_bytes)
        .saturating_add(outline_bytes)
}

fn page_frame_retained_bytes(page: &PageFrame) -> usize {
    fn glyph_bytes(run: &GlyphRun) -> usize {
        run.text
            .capacity()
            .saturating_add(
                run.glyph_ids
                    .capacity()
                    .saturating_mul(std::mem::size_of::<u16>()),
            )
            .saturating_add(
                run.advances
                    .capacity()
                    .saturating_mul(std::mem::size_of::<f64>()),
            )
    }

    fn multilingual_glyph_bytes(run: &oxoffice_layout::MultilingualGlyphRun) -> usize {
        run.logical_text
            .capacity()
            .saturating_add(run.language.as_ref().map_or(0, String::capacity))
            .saturating_add(
                run.glyph_ids
                    .capacity()
                    .saturating_mul(std::mem::size_of::<u16>()),
            )
            .saturating_add(
                [
                    run.x_advances.capacity(),
                    run.y_advances.capacity(),
                    run.x_offsets.capacity(),
                    run.y_offsets.capacity(),
                ]
                .into_iter()
                .sum::<usize>()
                .saturating_mul(std::mem::size_of::<f64>()),
            )
            .saturating_add(
                run.clusters
                    .capacity()
                    .saturating_mul(std::mem::size_of::<oxoffice_layout::GlyphCluster>()),
            )
    }

    fn element_bytes(element: &PositionedElement) -> usize {
        match element {
            PositionedElement::Text(run) => glyph_bytes(run),
            PositionedElement::MultilingualText(run) => multilingual_glyph_bytes(run),
            PositionedElement::Image {
                data, content_type, ..
            } => data.capacity().saturating_add(content_type.capacity()),
            PositionedElement::LinkAnnotation { url, .. } => url.capacity(),
            PositionedElement::Destination { name, .. } => name.capacity(),
            PositionedElement::Group(group) => group
                .children
                .capacity()
                .saturating_mul(std::mem::size_of::<PositionedElement>())
                .saturating_add(
                    group
                        .children
                        .iter()
                        .map(element_bytes)
                        .fold(0usize, usize::saturating_add),
                )
                .saturating_add(format!("{:?}", group.effects).len()),
            PositionedElement::MarkedContent { children, .. } => children
                .capacity()
                .saturating_mul(std::mem::size_of::<PositionedElement>())
                .saturating_add(
                    children
                        .iter()
                        .map(element_bytes)
                        .fold(0usize, usize::saturating_add),
                ),
            PositionedElement::Path(path) => format!("{path:?}").len(),
            _ => 0,
        }
    }

    std::mem::size_of::<PageFrame>()
        .saturating_add(
            page.elements
                .capacity()
                .saturating_mul(std::mem::size_of::<PositionedElement>()),
        )
        .saturating_add(
            page.elements
                .iter()
                .map(element_bytes)
                .fold(0usize, usize::saturating_add),
        )
        .saturating_add(format!("{:?}", page.background).len())
}

fn rebind_text_source(text: &mut TextSegment, source_node: Option<SourceNodeId>) {
    match (text.source.as_mut(), source_node) {
        (Some(source), Some(node)) => source.node = node,
        (Some(_), None) => text.source = None,
        (None, _) => {}
    }
    match (text.note_reference_source.as_mut(), source_node) {
        (Some(source), Some(node)) => source.node = node,
        (Some(_), None) => text.note_reference_source = None,
        (None, _) => {}
    }
    match (text.field_source.as_mut(), source_node) {
        (Some(field_source), Some(node)) => field_source.node = node,
        (Some(_), None) => text.field_source = None,
        (None, _) => {}
    }
}

fn rebind_multilingual_source(
    text: &mut oxoffice_layout::MultilingualTextSegment,
    source_node: Option<SourceNodeId>,
) -> Result<()> {
    let mut base = text.base().clone();
    rebind_text_source(&mut base, source_node);
    *text = text
        .clone()
        .with_base_and_advances(base, text.x_advances().to_vec())?;
    Ok(())
}

fn rebind_positioned_sources(elements: &mut [PositionedElement], node: Option<SourceNodeId>) {
    for element in elements {
        match element {
            PositionedElement::Text(run) => {
                if let Some(source) = &mut run.source {
                    if let Some(node) = node {
                        source.node = node;
                    } else {
                        run.source = None;
                    }
                }
                if let Some(source) = &mut run.field_source {
                    if let Some(node) = node {
                        source.node = node;
                    } else {
                        run.field_source = None;
                    }
                }
                if let Some(source) = &mut run.note_reference_source {
                    if let Some(node) = node {
                        source.node = node;
                    } else {
                        run.note_reference_source = None;
                    }
                }
            }
            PositionedElement::MultilingualText(run) => {
                if let Some(source) = &mut run.source {
                    if let Some(node) = node {
                        source.node = node;
                    } else {
                        run.source = None;
                    }
                }
                if let Some(source) = &mut run.field_source {
                    if let Some(node) = node {
                        source.node = node;
                    } else {
                        run.field_source = None;
                    }
                }
                if let Some(source) = &mut run.note_reference_source {
                    if let Some(node) = node {
                        source.node = node;
                    } else {
                        run.note_reference_source = None;
                    }
                }
            }
            PositionedElement::Group(group) => rebind_positioned_sources(&mut group.children, node),
            PositionedElement::MarkedContent { children, .. } => {
                rebind_positioned_sources(children, node)
            }
            _ => {}
        }
    }
}

pub(super) fn rebind_paragraph_source(
    block: &mut ParagraphBlock,
    source_node: Option<SourceNodeId>,
) -> Result<()> {
    for line in &mut block.lines {
        for item in &mut line.items {
            match item {
                LineItem::Text(text) | LineItem::Marker(text) => {
                    rebind_text_source(text, source_node)
                }
                LineItem::MultilingualText(text) => rebind_multilingual_source(text, source_node)?,
                LineItem::Tab {
                    leader: Some(leader),
                    ..
                } => rebind_text_source(leader, source_node),
                LineItem::Group { group, .. } => {
                    rebind_positioned_sources(&mut group.children, source_node)
                }
                _ => {}
            }
        }
    }
    if let Some(reflow) = block.reflow.as_mut() {
        for item in &mut reflow.items {
            match item {
                InlineItem::Text(text) | InlineItem::Marker(text) => {
                    rebind_text_source(text, source_node)
                }
                InlineItem::MultilingualText(text) => {
                    rebind_multilingual_source(text, source_node)?
                }
                InlineItem::HyphenatedText { segment, .. } => {
                    rebind_text_source(segment, source_node)
                }
                InlineItem::Group { group, .. } => {
                    rebind_positioned_sources(&mut group.children, source_node)
                }
                _ => {}
            }
        }
    }
    Ok(())
}

pub(super) fn remap_text_box_sources(
    block: &mut ParagraphBlock,
    nodes: &HashMap<SourceNodeId, SourceNodeId>,
) -> Result<()> {
    fn node(
        node: SourceNodeId,
        nodes: &HashMap<SourceNodeId, SourceNodeId>,
    ) -> Result<SourceNodeId> {
        nodes.get(&node).copied().ok_or_else(|| {
            LayoutError::Layout("rich text-box source identity escaped its physical body".into())
        })
    }
    fn text(text: &mut TextSegment, nodes: &HashMap<SourceNodeId, SourceNodeId>) -> Result<()> {
        if let Some(source) = &mut text.source {
            source.node = node(source.node, nodes)?;
        }
        if let Some(source) = &mut text.note_reference_source {
            source.node = node(source.node, nodes)?;
        }
        if let Some(source) = &mut text.field_source {
            source.node = node(source.node, nodes)?;
        }
        Ok(())
    }
    fn multilingual(
        value: &mut oxoffice_layout::MultilingualTextSegment,
        nodes: &HashMap<SourceNodeId, SourceNodeId>,
    ) -> Result<()> {
        let mut base = value.base().clone();
        text(&mut base, nodes)?;
        *value = value
            .clone()
            .with_base_and_advances(base, value.x_advances().to_vec())?;
        Ok(())
    }
    fn remap_elements(
        elements: &mut [PositionedElement],
        nodes: &HashMap<SourceNodeId, SourceNodeId>,
    ) -> Result<()> {
        for element in elements {
            match element {
                PositionedElement::Text(run) => {
                    if let Some(source) = &mut run.source {
                        source.node = node(source.node, nodes)?;
                    }
                    if let Some(source) = &mut run.note_reference_source {
                        source.node = node(source.node, nodes)?;
                    }
                    if let Some(source) = &mut run.field_source {
                        source.node = node(source.node, nodes)?;
                    }
                }
                PositionedElement::MultilingualText(run) => {
                    if let Some(source) = &mut run.source {
                        source.node = node(source.node, nodes)?;
                    }
                    if let Some(source) = &mut run.note_reference_source {
                        source.node = node(source.node, nodes)?;
                    }
                    if let Some(source) = &mut run.field_source {
                        source.node = node(source.node, nodes)?;
                    }
                }
                PositionedElement::Group(group) => remap_elements(&mut group.children, nodes)?,
                PositionedElement::MarkedContent { children, .. } => {
                    remap_elements(children, nodes)?
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn line(item: &mut LineItem, nodes: &HashMap<SourceNodeId, SourceNodeId>) -> Result<()> {
        match item {
            LineItem::Text(value) | LineItem::Marker(value) => text(value, nodes)?,
            LineItem::MultilingualText(value) => multilingual(value, nodes)?,
            LineItem::Tab {
                leader: Some(value),
                ..
            } => text(value, nodes)?,
            LineItem::Group { group, .. } => remap_elements(&mut group.children, nodes)?,
            LineItem::Figure { item, .. } => line(item, nodes)?,
            _ => {}
        }
        Ok(())
    }
    fn inline(item: &mut InlineItem, nodes: &HashMap<SourceNodeId, SourceNodeId>) -> Result<()> {
        match item {
            InlineItem::Text(value)
            | InlineItem::Marker(value)
            | InlineItem::HyphenatedText { segment: value, .. } => text(value, nodes)?,
            InlineItem::MultilingualText(value) => multilingual(value, nodes)?,
            InlineItem::Group { group, .. } => remap_elements(&mut group.children, nodes)?,
            InlineItem::Figure { item, .. } => inline(item, nodes)?,
            _ => {}
        }
        Ok(())
    }
    for value in &mut block.lines {
        for item in &mut value.items {
            line(item, nodes)?;
        }
    }
    if let Some(reflow) = &mut block.reflow {
        for item in &mut reflow.items {
            inline(item, nodes)?;
        }
    }
    for drawing in &mut block.anchored {
        match &mut drawing.content {
            block::AnchoredContent::Shape { text, .. } => {
                for paragraph in text {
                    remap_text_box_sources(paragraph, nodes)?;
                }
            }
            block::AnchoredContent::Group(group) => remap_elements(&mut group.children, nodes)?,
            block::AnchoredContent::Image { .. } => {}
        }
    }
    Ok(())
}

pub(super) fn rebind_header_footer_sources(
    story_kind: HeaderFooterStoryKind,
    relationship_id: &str,
    part: &oxoffice_docx_oxml::header_footer::CT_HdrFtr,
    blocks: &mut [ParagraphBlock],
    facts: &mut [paginator::HeaderFooterBlockSemantics],
    sources: Option<&SourceRegistry>,
) -> Result<()> {
    let story = match story_kind {
        HeaderFooterStoryKind::Header => WordStory::Header {
            relationship_id: relationship_id.to_owned(),
        },
        HeaderFooterStoryKind::Footer => WordStory::Footer {
            relationship_id: relationship_id.to_owned(),
        },
    };
    if blocks.len() != facts.len() {
        return Err(LayoutError::Layout(
            "furniture source paint count mismatch".into(),
        ));
    }
    for (paragraph_index, (block, facts)) in blocks.iter_mut().zip(facts).enumerate() {
        debug_assert!(paragraph_index < part.paragraphs.len());
        let source = sources.and_then(|sources| sources.id(&story, &[paragraph_index]));
        rebind_paragraph_source(block, source)?;
        let paint = Arc::make_mut(&mut facts.paint);
        paint.rebind_source(source)?;
        if let Some(registry) = sources {
            registry.bind_text_boxes(block, paint, source)?;
        }
    }
    Ok(())
}

pub(super) fn picture_draw_owner_bytes(owners: &Vec<(String, Option<String>)>) -> usize {
    owners
        .capacity()
        .saturating_mul(std::mem::size_of::<(String, Option<String>)>())
        .saturating_add(
            owners
                .iter()
                .map(|(id, story)| {
                    id.capacity()
                        .saturating_add(story.as_ref().map_or(0, String::capacity))
                })
                .fold(0, usize::saturating_add),
        )
}

pub(super) fn header_footer_cache_entry_bytes(
    key: &HeaderFooterCacheKey,
    content: &HeaderFooterVariantContent,
    diagnostics: &Vec<Diagnostic>,
    font_trace: &Vec<FontTraceEvent>,
) -> usize {
    let block_bytes = key
        .part
        .paragraphs
        .iter()
        .zip(&content.blocks)
        .zip(&content.directions)
        .map(|((paragraph, block), facts)| {
            paragraph_cache_entry_bytes(paragraph, block, Some(&facts.paint), &[], 0)
        })
        .fold(0usize, usize::saturating_add);
    let direction_bytes = content
        .directions
        .iter()
        .map(paginator::HeaderFooterBlockSemantics::retained_bytes)
        .fold(0usize, usize::saturating_add)
        .saturating_add(
            content
                .directions
                .capacity()
                .saturating_sub(content.directions.len())
                .saturating_mul(std::mem::size_of::<paginator::HeaderFooterBlockSemantics>()),
        );
    let diagnostic_bytes = diagnostics
        .capacity()
        .saturating_mul(std::mem::size_of::<Diagnostic>())
        .saturating_add(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.capacity())
                .fold(0usize, usize::saturating_add),
        );
    let watermark_bytes = content.watermark.as_ref().map_or(0, |watermark| {
        page_frame_retained_bytes(&PageFrame::new(
            1,
            0.0,
            0.0,
            vec![PositionedElement::Group(watermark.clone())],
        ))
    });
    let section_capacity = key
        .section
        .header_refs
        .capacity()
        .saturating_mul(std::mem::size_of::<
            oxoffice_docx_oxml::header_footer::HdrFtrRef,
        >())
        .saturating_add(
            key.section
                .footer_refs
                .capacity()
                .saturating_mul(std::mem::size_of::<
                    oxoffice_docx_oxml::header_footer::HdrFtrRef,
                >()),
        )
        .saturating_add(key.section.columns.as_ref().map_or(0, |columns| {
            columns
                .columns
                .capacity()
                .saturating_mul(std::mem::size_of::<oxoffice_docx_oxml::document::CT_Column>())
        }))
        .saturating_add(
            key.section
                .extra_xml
                .capacity()
                .saturating_mul(std::mem::size_of::<Vec<u8>>()),
        )
        .saturating_add(
            key.section
                .header_refs
                .iter()
                .chain(&key.section.footer_refs)
                .map(|reference| reference.rel_id.capacity())
                .fold(0usize, usize::saturating_add),
        )
        .saturating_add(
            key.section
                .extra_xml
                .iter()
                .map(Vec::capacity)
                .fold(0usize, usize::saturating_add),
        )
        .saturating_add(section_child_retained_bytes(&key.section));
    let paragraph_raw_capacity = key
        .part
        .paragraphs
        .iter()
        .map(|paragraph| {
            paragraph
                .extra_xml
                .capacity()
                .saturating_mul(std::mem::size_of::<(usize, Vec<u8>)>())
                .saturating_add(
                    paragraph
                        .extra_xml
                        .iter()
                        .map(|(_, raw)| raw.capacity())
                        .fold(0usize, usize::saturating_add),
                )
                .saturating_add(
                    paragraph
                        .runs
                        .iter()
                        .map(|run| {
                            run.extra_xml
                                .capacity()
                                .saturating_mul(std::mem::size_of::<Vec<u8>>())
                                .saturating_add(
                                    run.extra_xml
                                        .iter()
                                        .map(Vec::capacity)
                                        .fold(0usize, usize::saturating_add),
                                )
                                .saturating_add(
                                    run.extra_xml_positions
                                        .capacity()
                                        .saturating_mul(std::mem::size_of::<usize>()),
                                )
                        })
                        .fold(0usize, usize::saturating_add),
                )
        })
        .fold(0usize, usize::saturating_add);
    let watermark_capacity = key
        .part
        .watermarks()
        .iter()
        .map(|watermark| {
            std::mem::size_of::<VmlWatermark>().saturating_add(match watermark {
                VmlWatermark::Text {
                    text,
                    color,
                    font_family,
                    ..
                } => text
                    .capacity()
                    .saturating_add(color.capacity())
                    .saturating_add(font_family.as_ref().map_or(0, String::capacity)),
                VmlWatermark::Image {
                    relationship_id, ..
                } => relationship_id.capacity(),
            })
        })
        .fold(0usize, usize::saturating_add);
    let part_capacity = key
        .part
        .paragraphs
        .capacity()
        .saturating_mul(std::mem::size_of::<CT_P>())
        .saturating_add(
            content
                .blocks
                .capacity()
                .saturating_mul(std::mem::size_of::<ParagraphBlock>()),
        )
        .saturating_add(
            key.part
                .extra_namespaces
                .capacity()
                .saturating_mul(std::mem::size_of::<(String, String)>()),
        )
        .saturating_add(
            key.part
                .extra_xml
                .capacity()
                .saturating_mul(std::mem::size_of::<Vec<u8>>()),
        )
        .saturating_add(
            key.part
                .extra_namespaces
                .iter()
                .map(|(prefix, namespace)| prefix.capacity().saturating_add(namespace.capacity()))
                .fold(0usize, usize::saturating_add),
        )
        .saturating_add(
            key.part
                .extra_xml
                .iter()
                .map(Vec::capacity)
                .fold(0usize, usize::saturating_add),
        );
    std::mem::size_of::<HeaderFooterCacheEntry>()
        .saturating_add(key.relationship_id.capacity())
        .saturating_add(key.resolved_part_bytes.capacity())
        .saturating_add(format!("{:?}", key.section).len())
        .saturating_add(format!("{:?}", key.part).len())
        .saturating_add(key.rich_body.as_ref().map_or(0, |body| {
            format!("{body:?}")
                .len()
                .saturating_add(
                    body.content
                        .capacity()
                        .saturating_mul(std::mem::size_of::<BodyContent>()),
                )
                .saturating_add(
                    body.content
                        .iter()
                        .map(|content| match content {
                            BodyContent::Paragraph(paragraph) => {
                                paragraph_key_retained_bytes(paragraph)
                            }
                            _ => 0,
                        })
                        .fold(0usize, usize::saturating_add),
                )
        }))
        .saturating_add(key.source_part_name.as_ref().map_or(0, String::capacity))
        .saturating_add(section_capacity)
        .saturating_add(part_capacity)
        .saturating_add(paragraph_raw_capacity)
        .saturating_add(watermark_capacity)
        .saturating_add(block_bytes)
        .saturating_add(direction_bytes)
        .saturating_add(watermark_bytes)
        .saturating_add(diagnostic_bytes)
        .saturating_add(font_contribution_trace_bytes(font_trace))
}

fn word_paint_arc_bytes(paint: &WordParagraphPaint) -> usize {
    paint
        .retained_bytes()
        .and_then(|bytes| bytes.checked_add(2 * std::mem::size_of::<usize>()))
        .unwrap_or(usize::MAX)
}

fn multilingual_cache_bytes(
    span: &oxoffice_layout::MultilingualTextSegment,
    proof: Option<&WordLegacyCache>,
) -> usize {
    let Some(proof) = proof else {
        return usize::MAX;
    };
    if !span.uses_legacy_shaping() || span.kerning_override().is_none() {
        return usize::MAX;
    }
    let base = span.base();
    let bytes = (|| {
        let mut bytes = base
            .text
            .capacity()
            .checked_add(
                base.glyph_ids
                    .capacity()
                    .checked_mul(std::mem::size_of::<u16>())?,
            )?
            .checked_add(
                base.advances
                    .capacity()
                    .checked_mul(std::mem::size_of::<f64>())?,
            )?
            .checked_add(base.hyperlink_url.as_ref().map_or(0, String::capacity))?;
        for values in [
            span.x_advances(),
            span.y_advances(),
            span.x_offsets(),
            span.y_offsets(),
        ] {
            bytes = bytes.checked_add(values.len().checked_mul(std::mem::size_of::<f64>())?)?;
        }
        bytes
            .checked_add(
                span.clusters()
                    .len()
                    .checked_mul(std::mem::size_of::<oxoffice_layout::GlyphCluster>())?,
            )?
            .checked_add(span.language().map_or(0, str::len))?
            .checked_add(proof.language_bytes_per_span)
    })();
    bytes.unwrap_or(usize::MAX)
}

pub(super) fn paragraph_cache_entry_bytes(
    paragraph: &CT_P,
    block: &ParagraphBlock,
    paint: Option<&WordParagraphPaint>,
    diagnostics: &[Diagnostic],
    font_trace_bytes: usize,
) -> usize {
    fn option_string_bytes(value: &Option<String>) -> usize {
        value.as_ref().map_or(0, String::capacity)
    }
    fn shading_bytes(shading: &CT_Shd) -> usize {
        shading
            .val
            .capacity()
            .saturating_add(option_string_bytes(&shading.color))
            .saturating_add(option_string_bytes(&shading.fill))
            .saturating_add(option_string_bytes(&shading.theme_color))
            .saturating_add(option_string_bytes(&shading.theme_fill))
            .saturating_add(attribute_pairs_bytes(&shading.extra_attributes))
    }
    fn run_properties_bytes(properties: &CT_RPr) -> usize {
        [
            &properties.style_id,
            &properties.font_ascii,
            &properties.font_hansi,
            &properties.font_east_asia,
            &properties.font_cs,
            &properties.font_ascii_theme,
            &properties.font_hansi_theme,
            &properties.font_east_asia_theme,
            &properties.font_cs_theme,
            &properties.font_hint,
            &properties.color,
            &properties.color_theme,
            &properties.vert_align,
        ]
        .into_iter()
        .map(option_string_bytes)
        .fold(0usize, usize::saturating_add)
        .saturating_add(attribute_pairs_bytes(&properties.font_extra_attributes))
        .saturating_add(attribute_pairs_bytes(&properties.color_extra_attributes))
        .saturating_add(
            properties
                .shading
                .as_ref()
                .map_or(0, |shading| shading_bytes(shading)),
        )
    }
    fn border_bytes(borders: &CT_PBdr) -> usize {
        [
            &borders.top,
            &borders.bottom,
            &borders.left,
            &borders.right,
            &borders.between,
            &borders.bar,
        ]
        .into_iter()
        .map(|edge| {
            edge.as_ref()
                .and_then(|edge| edge.color.as_ref())
                .map_or(0, String::capacity)
        })
        .fold(0usize, usize::saturating_add)
    }
    fn paragraph_properties_bytes(properties: &CT_PPr) -> usize {
        option_string_bytes(&properties.style_id)
            .saturating_add(option_string_bytes(&properties.line_rule))
            .saturating_add(
                properties
                    .borders
                    .as_ref()
                    .map_or(0, |borders| border_bytes(borders)),
            )
            .saturating_add(properties.tabs.as_ref().map_or(0, |tabs| {
                tabs.tabs
                    .capacity()
                    .saturating_mul(std::mem::size_of::<CT_TabStop>())
            }))
            .saturating_add(
                properties
                    .shading
                    .as_ref()
                    .map_or(0, |shading| shading_bytes(shading)),
            )
            .saturating_add(properties.rpr.as_ref().map_or(0, run_properties_bytes))
    }
    fn paragraph_key_bytes(paragraph: &CT_P) -> usize {
        paragraph
            .runs
            .capacity()
            .saturating_mul(std::mem::size_of::<CT_R>())
            .saturating_add(
                paragraph
                    .runs
                    .iter()
                    .map(|run| {
                        run.content
                            .capacity()
                            .saturating_mul(std::mem::size_of::<RunContent>())
                            .saturating_add(
                                run.content
                                    .iter()
                                    .map(|content| match content {
                                        RunContent::Text(text) => text.text.capacity(),
                                        _ => 0,
                                    })
                                    .fold(0usize, usize::saturating_add),
                            )
                            .saturating_add(run.properties.as_ref().map_or(0, run_properties_bytes))
                    })
                    .fold(0usize, usize::saturating_add),
            )
            .saturating_add(
                paragraph
                    .properties
                    .as_ref()
                    .map_or(0, paragraph_properties_bytes),
            )
    }
    fn text_bytes(text: &TextSegment) -> usize {
        text.text.capacity()
            + text.glyph_ids.capacity() * std::mem::size_of::<u16>()
            + text.advances.capacity() * std::mem::size_of::<f64>()
            + text.hyperlink_url.as_ref().map_or(0, String::capacity)
    }
    fn inline_bytes(item: &InlineItem, proof: Option<&WordLegacyCache>) -> usize {
        match item {
            InlineItem::Text(text) | InlineItem::Marker(text) => text_bytes(text),
            InlineItem::MultilingualText(span) => multilingual_cache_bytes(span, proof),
            InlineItem::Group { .. } => usize::MAX,
            _ => 0,
        }
    }
    fn line_item_bytes(item: &LineItem, proof: Option<&WordLegacyCache>) -> usize {
        match item {
            LineItem::Text(text) | LineItem::Marker(text) => text_bytes(text),
            LineItem::MultilingualText(span) => multilingual_cache_bytes(span, proof),
            LineItem::Tab { leader, .. } => leader.as_ref().map_or(0, text_bytes),
            LineItem::Group { .. } => usize::MAX,
            _ => 0,
        }
    }

    let proof = paint.and_then(|paint| paint.legacy_cache.as_ref());
    let paragraph_bytes = paragraph_key_bytes(paragraph);
    let line_bytes = block
        .lines
        .capacity()
        .saturating_mul(std::mem::size_of::<oxoffice_layout::LayoutLine>())
        .saturating_add(
            block
                .lines
                .iter()
                .map(|line| {
                    line.items
                        .capacity()
                        .saturating_mul(std::mem::size_of::<LineItem>())
                        .saturating_add(
                            line.items
                                .iter()
                                .map(|item| line_item_bytes(item, proof))
                                .fold(0usize, usize::saturating_add),
                        )
                })
                .fold(0usize, usize::saturating_add),
        );
    let reflow_bytes = block.reflow.as_ref().map_or(0, |reflow| {
        std::mem::size_of_val(reflow.as_ref())
            .saturating_add(
                reflow
                    .items
                    .capacity()
                    .saturating_mul(std::mem::size_of::<InlineItem>()),
            )
            .saturating_add(
                reflow
                    .items
                    .iter()
                    .map(|item| inline_bytes(item, proof))
                    .fold(0usize, usize::saturating_add),
            )
            .saturating_add(
                reflow
                    .params
                    .tab_stops
                    .capacity()
                    .saturating_mul(std::mem::size_of::<oxoffice_layout::TabStop>()),
            )
            .saturating_add(
                reflow
                    .params
                    .line_prefix_widths
                    .capacity()
                    .saturating_mul(std::mem::size_of::<f64>()),
            )
            .saturating_add(
                reflow
                    .params
                    .line_suffix_widths
                    .capacity()
                    .saturating_mul(std::mem::size_of::<f64>()),
            )
    });
    let diagnostic_bytes = diagnostics
        .len()
        .saturating_mul(std::mem::size_of::<Diagnostic>())
        .saturating_add(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.capacity())
                .fold(0usize, usize::saturating_add),
        );
    std::mem::size_of::<ParagraphCacheEntry>()
        .saturating_add(arc_allocation_bytes::<ParagraphBlock>())
        .saturating_add(paint.map_or(0, word_paint_arc_bytes))
        .saturating_add(paragraph_bytes)
        .saturating_add(line_bytes)
        .saturating_add(reflow_bytes)
        .saturating_add(if block.anchored.is_empty() {
            0
        } else {
            usize::MAX
        })
        .saturating_add(block.heading_text.as_ref().map_or(0, String::capacity))
        .saturating_add(block.borders.as_ref().map_or(0, border_bytes))
        .saturating_add(font_trace_bytes)
        .saturating_add(diagnostic_bytes)
}

struct StableFingerprint(u64);

impl StableFingerprint {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write_bytes(&mut self, value: &[u8]) {
        for byte in value {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn write_usize(&mut self, value: usize) {
        self.write_bytes(&value.to_le_bytes());
    }

    fn write_tag(&mut self, value: u8) {
        self.write_bytes(&[value]);
    }
}

pub(super) fn paragraph_fingerprint(paragraph: &CT_P) -> u64 {
    let mut fingerprint = StableFingerprint::new();
    fingerprint.write_usize(paragraph.runs.len());
    fingerprint.write_tag(u8::from(paragraph.properties.is_some()));
    // A ruby annotation changes the painted page without changing any run,
    // so a paragraph that carries one must not collide with the same runs
    // unannotated. Nothing is written when there is none, which keeps every
    // existing fingerprint where it was.
    for ruby in &paragraph.rubies {
        fingerprint.write_tag(7);
        fingerprint.write_usize(ruby.base_start);
        fingerprint.write_usize(ruby.base_end);
        for run in &ruby.ruby_text {
            fingerprint.write_bytes(run.text().as_bytes());
        }
    }
    for run in &paragraph.runs {
        fingerprint.write_tag(u8::from(run.properties.is_some()));
        fingerprint.write_usize(run.content.len());
        for content in &run.content {
            match content {
                RunContent::Text(text) => {
                    fingerprint.write_tag(0);
                    fingerprint.write_bytes(text.text.as_bytes());
                    fingerprint.write_tag(u8::from(text.preserve_space));
                }
                RunContent::DeletedText(text) => {
                    fingerprint.write_tag(1);
                    fingerprint.write_bytes(text.text.as_bytes());
                    fingerprint.write_tag(u8::from(text.preserve_space));
                }
                RunContent::Tab => fingerprint.write_tag(2),
                RunContent::Break(kind, clear) => {
                    fingerprint.write_tag(3);
                    fingerprint.write_tag(match kind {
                        BreakType::Line => 0,
                        BreakType::Page => 1,
                        BreakType::Column => 2,
                    });
                    fingerprint.write_tag(match clear {
                        BreakClear::None => 0,
                        BreakClear::Left => 1,
                        BreakClear::Right => 2,
                        BreakClear::All => 3,
                    });
                }
                RunContent::Symbol { font, char_code } => {
                    fingerprint.write_tag(5);
                    fingerprint.write_bytes(font.as_bytes());
                    fingerprint.write_usize(usize::from(*char_code));
                }
                RunContent::SpecialCharacter(character) => {
                    fingerprint.write_tag(6);
                    fingerprint.write_tag(match character {
                        SpecialCharacter::CarriageReturn => 0,
                        SpecialCharacter::NoBreakHyphen => 1,
                        SpecialCharacter::SoftHyphen => 2,
                        SpecialCharacter::PositionalTab { .. } => 3,
                    });
                }
                RunContent::Drawing(_)
                | RunContent::Field(_)
                | RunContent::FootnoteRef { .. }
                | RunContent::EndnoteRef { .. }
                | RunContent::CommentReference { .. } => fingerprint.write_tag(4),
            }
        }
    }
    fingerprint.0
}

pub(super) fn table_fingerprint(table: &CT_Tbl) -> u64 {
    fn write_table(table: &CT_Tbl, fingerprint: &mut StableFingerprint) {
        fingerprint.write_tag(u8::from(table.properties.is_some()));
        fingerprint.write_usize(table.grid.as_ref().map_or(0, |grid| grid.columns.len()));
        fingerprint.write_usize(table.rows.len());
        for row in &table.rows {
            fingerprint.write_tag(u8::from(row.properties.is_some()));
            fingerprint.write_usize(row.cells.len());
            for cell in &row.cells {
                fingerprint.write_tag(u8::from(cell.properties.is_some()));
                fingerprint.write_usize(cell.content.len());
                for content in &cell.content {
                    match content {
                        CellContent::Paragraph(paragraph) => {
                            fingerprint.write_tag(0);
                            fingerprint
                                .write_bytes(&paragraph_fingerprint(paragraph).to_le_bytes());
                        }
                        CellContent::Table(table) => {
                            fingerprint.write_tag(1);
                            write_table(table, fingerprint);
                        }
                        CellContent::ContentControl(_) => fingerprint.write_tag(2),
                    }
                }
            }
        }
    }

    let mut fingerprint = StableFingerprint::new();
    write_table(table, &mut fingerprint);
    fingerprint.0
}
