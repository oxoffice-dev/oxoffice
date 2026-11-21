//! Section geometry: page size, columns, borders, line numbers, and the raw attribute reading it needs.

use super::*;

/// Word's line-number gap when `w:lnNumType` leaves `w:distance` out.
const AUTOMATIC_LINE_NUMBER_DISTANCE: f64 = 18.0;

/// Bytes the modeled `w:sectPr` children own outside `CT_SectPr` itself.
///
/// The note, paper-source, page-border and line-number members are boxed, so
/// the struct's own size does not account for them, and each of them owns
/// retained attribute and raw-child storage of its own.
pub(super) fn section_child_retained_bytes(section: &CT_SectPr) -> usize {
    let retained_attributes = |attributes: &Vec<(String, String)>| {
        attributes
            .capacity()
            .saturating_mul(std::mem::size_of::<(String, String)>())
            .saturating_add(
                attributes
                    .iter()
                    .map(|(name, value)| name.capacity().saturating_add(value.capacity()))
                    .fold(0usize, usize::saturating_add),
            )
    };
    let note_bytes = |notes: &oxoffice_docx_oxml::document::CT_NoteProperties| {
        std::mem::size_of::<oxoffice_docx_oxml::document::CT_NoteProperties>()
            .saturating_add(
                [&notes.pos, &notes.num_fmt, &notes.num_restart]
                    .into_iter()
                    .map(|value| value.as_ref().map_or(0, String::capacity))
                    .fold(0usize, usize::saturating_add),
            )
            .saturating_add(
                notes
                    .extra_xml
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(usize, Vec<u8>)>()),
            )
            .saturating_add(
                notes
                    .extra_xml
                    .iter()
                    .map(|(_, raw)| raw.capacity())
                    .fold(0usize, usize::saturating_add),
            )
    };
    let edge_bytes = |edge: &Option<oxoffice_docx_oxml::borders::CT_BorderEdge>| {
        edge.as_ref().map_or(0, |edge| {
            edge.color
                .as_ref()
                .map_or(0, String::capacity)
                .saturating_add(retained_attributes(&edge.extra_attributes))
        })
    };

    let mut bytes = section.text_direction.as_ref().map_or(0, String::capacity);
    if let Some(notes) = section.footnote_pr.as_deref() {
        bytes = bytes.saturating_add(note_bytes(notes));
    }
    if let Some(notes) = section.endnote_pr.as_deref() {
        bytes = bytes.saturating_add(note_bytes(notes));
    }
    if let Some(source) = section.paper_source.as_deref() {
        bytes = bytes
            .saturating_add(std::mem::size_of::<
                oxoffice_docx_oxml::document::CT_PaperSource,
            >())
            .saturating_add(retained_attributes(&source.extra_attributes));
    }
    if let Some(numbering) = section.line_numbers.as_deref() {
        bytes = bytes
            .saturating_add(std::mem::size_of::<
                oxoffice_docx_oxml::document::CT_LineNumber,
            >())
            .saturating_add(retained_attributes(&numbering.extra_attributes));
    }
    if let Some(borders) = section.page_borders.as_deref() {
        bytes = bytes
            .saturating_add(std::mem::size_of::<
                oxoffice_docx_oxml::document::CT_PageBorders,
            >())
            .saturating_add(retained_attributes(&borders.extra_attributes))
            .saturating_add(
                borders
                    .extra_xml
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Vec<u8>>()),
            )
            .saturating_add(
                borders
                    .extra_xml
                    .iter()
                    .map(Vec::capacity)
                    .fold(0usize, usize::saturating_add),
            )
            .saturating_add(edge_bytes(&borders.top))
            .saturating_add(edge_bytes(&borders.left))
            .saturating_add(edge_bytes(&borders.bottom))
            .saturating_add(edge_bytes(&borders.right));
    }
    bytes
}

/// Convert section properties to page geometry.
pub(crate) fn sect_pr_to_geometry(sect_pr: &CT_SectPr) -> PageGeometry {
    let geometry = PageGeometry {
        page_width: sect_pr.page_width.map(|t| t.to_pt()).unwrap_or(612.0),
        page_height: sect_pr.page_height.map(|t| t.to_pt()).unwrap_or(792.0),
        margin_top: sect_pr.margin_top.map(|t| t.to_pt()).unwrap_or(72.0),
        margin_right: sect_pr.margin_right.map(|t| t.to_pt()).unwrap_or(72.0),
        margin_bottom: sect_pr.margin_bottom.map(|t| t.to_pt()).unwrap_or(72.0),
        margin_left: sect_pr.margin_left.map(|t| t.to_pt()).unwrap_or(72.0),
        header_distance: sect_pr.header_distance.map(|t| t.to_pt()).unwrap_or(36.0),
        footer_distance: sect_pr.footer_distance.map(|t| t.to_pt()).unwrap_or(36.0),
        columns: Vec::new(),
        column_separator: false,
        page_borders: sect_pr_page_borders(sect_pr),
        line_numbers: sect_pr_line_numbers(sect_pr),
        vertical_alignment: sect_pr.vertical_alignment,
        mirror_margins: false,
        do_not_use_html_paragraph_auto_spacing: false,
        // `w:sectPr/w:textDirection` is authored and preserved. This is the
        // render projection over it, and it writes nothing back.
        body_rotation: sect_pr
            .text_direction
            .as_deref()
            .and_then(crate::table::text_direction_rotation),
    };
    resolve_column_tracks(sect_pr, geometry)
}

/// Resolve a section's column tracks onto its geometry.
///
/// A section that resolves to one column is returned untouched, so its content
/// width stays the exact expression it evaluated before columns existed. A
/// neutral one-track form would reassociate the arithmetic and move every
/// recorded baseline in the workspace, which is why this bypasses rather than
/// generalises.
fn resolve_column_tracks(sect_pr: &CT_SectPr, geometry: PageGeometry) -> PageGeometry {
    let Some(columns) = sect_pr.columns.as_ref() else {
        return geometry;
    };
    let explicit = columns.equal_width == Some(false) || !columns.columns.is_empty();
    let widths: Vec<(f64, f64)> = if explicit
        && columns.columns.len() > 1
        && columns.columns.iter().all(|column| column.width.is_some())
    {
        columns
            .columns
            .iter()
            .map(|column| {
                (
                    column.width.map_or(0.0, |value| value.to_pt()),
                    column.space.map_or(0.0, |value| value.to_pt()),
                )
            })
            .collect()
    } else {
        let count = columns.num.unwrap_or(1);
        if count < 2 {
            return geometry;
        }
        let space = columns.space.map_or(0.0, |value| value.to_pt());
        let measure = geometry.text_measure();
        let width = (measure - space * f64::from(count - 1)) / f64::from(count);
        (0..count).map(|_| (width, space)).collect()
    };

    if widths.iter().any(|(width, _)| *width <= 0.0) {
        return geometry;
    }

    let mut tracks = Vec::with_capacity(widths.len());
    let mut x = geometry.margin_left;
    for (width, space) in widths {
        tracks.push(ColumnTrack { x, width });
        x += width + space;
    }
    PageGeometry {
        column_separator: columns.sep.unwrap_or(false),
        columns: tracks,
        ..geometry
    }
}

/// Resolve `w:pgBorders` into the frame the paginator draws.
fn sect_pr_page_borders(sect_pr: &CT_SectPr) -> Option<PageBorderFrame> {
    let borders = sect_pr.page_borders.as_deref()?;
    if borders.top.is_none()
        && borders.left.is_none()
        && borders.bottom.is_none()
        && borders.right.is_none()
    {
        return None;
    }
    Some(PageBorderFrame {
        display: borders.display.unwrap_or(ST_PageBorderDisplay::AllPages),
        offset_from: borders.offset_from.unwrap_or(ST_PageBorderOffset::Text),
        in_front: borders.z_order != Some(ST_PageBorderZOrder::Back),
        edges: CT_PBdr {
            top: borders.top.clone(),
            left: borders.left.clone(),
            bottom: borders.bottom.clone(),
            right: borders.right.clone(),
            between: None,
            bar: None,
        },
    })
}

/// Resolve `w:lnNumType` into the numbering the paginator draws.
fn sect_pr_line_numbers(sect_pr: &CT_SectPr) -> Option<LineNumbering> {
    let numbering = sect_pr.line_numbers.as_deref()?;
    Some(LineNumbering {
        count_by: numbering.count_by?.max(1),
        start: numbering.start.unwrap_or(0),
        distance: numbering
            .distance
            .map_or(AUTOMATIC_LINE_NUMBER_DISTANCE, |value| value.to_pt()),
        restart: numbering.restart.unwrap_or(ST_LineNumberRestart::NewPage),
        font_id: None,
    })
}

pub(super) fn prepare_word_line_numbers(
    input: &LayoutInput,
    fonts: &mut FontManager,
) -> (Option<paginator::WordLineNumberStyle>, Vec<Diagnostic>) {
    if !document_sections(&input.document)
        .into_iter()
        .any(|section| sect_pr_line_numbers(section).is_some())
    {
        return (None, Vec::new());
    }
    let mut diagnostics = Vec::new();
    let properties = style_resolver::line_number_properties(&input.styles, &mut diagnostics);
    let size = properties.sz.map_or(11.0, |value| value.to_pt());
    let bold = properties.bold.unwrap_or(false);
    let italic = properties.italic.unwrap_or(false);
    let family = resolve_font_family(&properties, input.theme.as_ref(), WordFontSlot::Ascii);
    let font_id = if size.is_finite() && size > 0.0 {
        match fonts.resolve_font(family.as_deref(), bold, italic) {
            Ok(font) => Some(font),
            Err(error) => {
                diagnostics.push(Diagnostic {
                    message: format!("line-number font could not be resolved: {error}"),
                });
                None
            }
        }
    } else {
        diagnostics.push(Diagnostic {
            message: "line-number nominal font size must be finite and positive".into(),
        });
        None
    };
    (
        Some(paginator::WordLineNumberStyle {
            font_id,
            size,
            bold,
            italic,
            kerning: style_resolver::word_kerning_enabled(&properties),
        }),
        diagnostics,
    )
}

/// Page geometry for one whole section, with the state `sect_pr_to_geometry`
/// cannot reach on its own.
///
/// The settings part owns mirrored margins, and the line-number font must be
/// resolved before pagination because the paginator borrows the font manager
/// immutably. Both are read only where a section is finished, so a document
/// that uses neither takes exactly the path it took before.
pub(super) fn section_page_geometry(
    sect_pr: &CT_SectPr,
    input: &LayoutInput,
    line_number_style: Option<paginator::WordLineNumberStyle>,
    diagnostics: &mut Vec<Diagnostic>,
) -> PageGeometry {
    let mut geometry = sect_pr_to_geometry(sect_pr);
    // A vertical section fills one transposed band. Composing it with column
    // tracks would transpose each track about its own centre while the page
    // rotates about one, so the tracks are dropped and the fact is recorded
    // rather than painted wrong.
    if geometry.body_rotation.is_some() && !geometry.columns.is_empty() {
        push_unique_diagnostic(
            diagnostics,
            "vertical section text is laid out in one column track".to_owned(),
        );
        geometry.columns = Vec::new();
        geometry.column_separator = false;
    }
    if geometry.vertical_alignment == Some(ST_VerticalJc::Both) {
        diagnostics.push(Diagnostic {
            message: "section vertical alignment both is laid out as top, vertical distribution is not implemented"
                .to_owned(),
        });
    }
    if input.mirror_margins {
        geometry.mirror_margins = true;
        let gutter = sect_pr.gutter.map_or(0.0, |value| value.to_pt());
        if input.gutter_at_top {
            geometry.margin_top += gutter;
        } else {
            geometry.margin_left += gutter;
        }
        // The gutter moved the text measure, so the tracks resolved against
        // the ungutttered page are stale. Clearing them first keeps a bypass
        // in the second pass from leaving the first pass's answer behind.
        geometry.columns = Vec::new();
        geometry.column_separator = false;
        geometry = resolve_column_tracks(sect_pr, geometry);
    }
    geometry.do_not_use_html_paragraph_auto_spacing = input.do_not_use_html_paragraph_auto_spacing;
    if let Some(line_numbers) = geometry.line_numbers.as_mut() {
        line_numbers.font_id = line_number_style.and_then(|style| style.font_id);
    }
    geometry
}

pub(super) fn section_page_number_start(sect_pr: &CT_SectPr) -> Option<usize> {
    effective_section_page_number(sect_pr).start
}

pub(super) fn xml_local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

pub(super) fn raw_root_start_tag(raw: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut cursor = 0usize;
    while cursor < raw.len() && raw[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    if raw.get(cursor) != Some(&b'<') {
        return None;
    }
    cursor += 1;
    if matches!(raw.get(cursor), Some(b'!' | b'?' | b'/')) {
        return None;
    }
    let name_start = cursor;
    while cursor < raw.len()
        && !raw[cursor].is_ascii_whitespace()
        && !matches!(raw[cursor], b'>' | b'/')
    {
        cursor += 1;
    }
    if cursor == name_start {
        return None;
    }
    let name_end = cursor;
    let attributes_start = cursor;
    let mut quote = None;
    while cursor < raw.len() {
        match (quote, raw[cursor]) {
            (None, b'\'' | b'"') => quote = Some(raw[cursor]),
            (Some(expected), found) if expected == found => quote = None,
            (None, b'>') => {
                return Some((&raw[name_start..name_end], &raw[attributes_start..cursor]));
            }
            _ => {}
        }
        cursor += 1;
    }
    None
}

pub(super) fn parse_raw_attributes(attributes: &[u8]) -> Option<Vec<(&[u8], &[u8])>> {
    let mut parsed = Vec::new();
    let mut cursor = 0usize;
    while cursor < attributes.len() {
        while cursor < attributes.len() && attributes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor == attributes.len() || attributes[cursor] == b'/' {
            break;
        }
        let name_start = cursor;
        while cursor < attributes.len()
            && !attributes[cursor].is_ascii_whitespace()
            && attributes[cursor] != b'='
        {
            cursor += 1;
        }
        let name_end = cursor;
        while cursor < attributes.len() && attributes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if attributes.get(cursor) != Some(&b'=') {
            cursor = cursor.saturating_add(1);
            continue;
        }
        cursor += 1;
        while cursor < attributes.len() && attributes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let quote = *attributes.get(cursor)?;
        if !matches!(quote, b'\'' | b'"') {
            return None;
        }
        cursor += 1;
        let value_start = cursor;
        while cursor < attributes.len() && attributes[cursor] != quote {
            cursor += 1;
        }
        let value_end = cursor;
        cursor += 1;
        parsed.push((
            &attributes[name_start..name_end],
            &attributes[value_start..value_end],
        ));
    }
    Some(parsed)
}

pub(super) fn raw_name_has_namespace(
    name: &[u8],
    attributes: &[(&[u8], &[u8])],
    expected: &str,
    is_attribute: bool,
) -> bool {
    let prefix = name
        .iter()
        .rposition(|byte| *byte == b':')
        .map(|separator| &name[..separator]);
    let namespace = match prefix {
        Some(prefix) => attributes.iter().find_map(|(attribute_name, value)| {
            attribute_name
                .strip_prefix(b"xmlns:")
                .is_some_and(|declared| declared == prefix)
                .then_some(*value)
        }),
        None if !is_attribute => attributes
            .iter()
            .find_map(|(attribute_name, value)| (*attribute_name == b"xmlns").then_some(*value)),
        None => None,
    };
    match namespace {
        Some(namespace) => decode_xml_attribute(namespace).is_some_and(|value| value == expected),
        None => prefix == Some(b"w".as_slice()) && expected == oxoffice_docx_oxml::namespace::W_NS,
    }
}

fn decode_xml_attribute(value: &[u8]) -> Option<String> {
    let value = std::str::from_utf8(value).ok()?;
    let mut decoded = String::with_capacity(value.len());
    let mut cursor = 0usize;
    while let Some(relative_start) = value[cursor..].find('&') {
        let entity_start = cursor + relative_start;
        decoded.push_str(&value[cursor..entity_start]);
        let entity_end = entity_start + value[entity_start..].find(';')?;
        let entity = &value[entity_start + 1..entity_end];
        match entity {
            "amp" => decoded.push('&'),
            "apos" => decoded.push('\''),
            "gt" => decoded.push('>'),
            "lt" => decoded.push('<'),
            "quot" => decoded.push('"'),
            numeric if numeric.starts_with("#x") => {
                decoded.push(char::from_u32(
                    u32::from_str_radix(&numeric[2..], 16).ok()?,
                )?);
            }
            numeric if numeric.starts_with('#') => {
                decoded.push(char::from_u32(numeric[1..].parse().ok()?)?);
            }
            _ => return None,
        }
        cursor = entity_end + 1;
    }
    decoded.push_str(&value[cursor..]);
    Some(decoded)
}
