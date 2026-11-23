//! Signatures of body content, used to align and to check a result.

use super::*;

type ControlPropertySignature<'a> = Option<(
    Option<oxoffice_docx_oxml::content_control::SdtType>,
    Option<&'a oxoffice_docx_oxml::content_control::CT_DataBinding>,
)>;

/// The content-control properties that name, protect or file a control
/// without changing what it holds, by their `w:sdtPr` element names.
const CONTROL_METADATA: [&str; 5] = ["tag", "alias", "lock", "placeholder", "docPartGallery"];

thread_local! {
    static RELATIONSHIP_CLASSES: std::cell::RefCell<HashMap<String, String>> =
        std::cell::RefCell::default();
}

/// While alive, signatures read a relationship id of `classes` as the first
/// id of its class (`relationship_classes`), so a picture, chart or link
/// reached through another relationship to the same payload is no change.
/// The classes are those of the story owner being compared: the edited
/// version's relationships were remapped to the original's ids before.
pub(super) struct RelationshipClassScope(HashMap<String, String>);

impl RelationshipClassScope {
    pub(super) fn enter(classes: HashMap<String, String>) -> Self {
        Self(RELATIONSHIP_CLASSES.replace(classes))
    }
}

impl Drop for RelationshipClassScope {
    fn drop(&mut self) {
        RELATIONSHIP_CLASSES.set(std::mem::take(&mut self.0));
    }
}

fn classify_relationship(id: &mut String) {
    RELATIONSHIP_CLASSES.with_borrow(|classes| {
        if let Some(class) = classes.get(id.as_str()) {
            id.clone_from(class);
        }
    });
}

/// A copy of `link` that names its relationship by its class.
pub(super) fn classified_hyperlink(
    link: &oxoffice_docx_oxml::text::HyperlinkSpan,
) -> oxoffice_docx_oxml::text::HyperlinkSpan {
    let mut link = link.clone();
    if let Some(id) = &mut link.rel_id {
        classify_relationship(id);
    }
    link
}

pub(super) fn body_signature(content: &BodyContent) -> String {
    match content {
        BodyContent::Paragraph(paragraph) => format!("p:{}", paragraph_signature(paragraph)),
        BodyContent::Table(table) => format!("t:{}", table_signature(table)),
        BodyContent::ContentControl(control) => format!("s:{}", control_signature(control)),
        BodyContent::RawXml(raw) => format!("x:{raw:?}"),
    }
}

fn paragraph_signature(paragraph: &CT_P) -> String {
    let numbering = paragraph_numbering(paragraph);
    let extra_xml = paragraph_raw_children(paragraph);
    format!(
        "{numbering:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}",
        paragraph.runs.iter().map(run_signature).collect::<Vec<_>>(),
        paragraph
            .hyperlinks
            .iter()
            .map(classified_hyperlink)
            .collect::<Vec<_>>(),
        paragraph.comment_ranges,
        paragraph.bookmark_markers,
        extra_xml,
        paragraph
            .content_controls
            .iter()
            .map(|(at, raw_before, markers_before, control)| (
                at,
                raw_before,
                markers_before,
                control_signature(control),
            ))
            .collect::<Vec<_>>(),
    )
}

fn paragraph_numbering(paragraph: &CT_P) -> Option<(Option<u32>, Option<u32>)> {
    paragraph.properties.as_ref().and_then(|properties| {
        (properties.num_id.is_some() || properties.num_ilvl.is_some())
            .then_some((properties.num_id, properties.num_ilvl))
    })
}

pub(super) fn run_signature(run: &CT_R) -> String {
    let raw = semantic_run_raw(run);
    format!(
        "{:?}:{raw:?}",
        run.content
            .iter()
            .map(run_content_signature)
            .collect::<Vec<_>>()
    )
}

pub(super) fn semantic_run_raw(run: &CT_R) -> Vec<(&[u8], usize)> {
    if run.extra_xml.len() != run.extra_xml_positions.len() {
        return run
            .extra_xml
            .iter()
            .enumerate()
            .filter(|(_, raw)| !oxoffice_docx_oxml::text::is_root_attribute_record(raw))
            .map(|(position, raw)| (raw.as_slice(), position))
            .collect();
    }
    run.extra_xml
        .iter()
        .zip(&run.extra_xml_positions)
        .filter(|(_, position)| !CT_R::raw_child_is_root_attributes(**position))
        .map(|(raw, position)| (raw.as_slice(), *position))
        .collect()
}

pub(super) fn run_content_signature(content: &RunContent) -> String {
    match content {
        RunContent::Field(_) => "field-owner".to_owned(),
        RunContent::Drawing(drawing) => {
            let mut signature = drawing_signature(drawing);
            if let Some(inline) = &mut signature.inline {
                classify_relationship(&mut inline.embed_id);
                inline.link_id.iter_mut().for_each(classify_relationship);
                inline
                    .chart_rel_id
                    .iter_mut()
                    .for_each(classify_relationship);
            }
            if let Some(anchor) = &mut signature.anchor {
                classify_relationship(&mut anchor.embed_id);
                anchor.link_id.iter_mut().for_each(classify_relationship);
                anchor
                    .chart_rel_id
                    .iter_mut()
                    .for_each(classify_relationship);
            }
            format!("Drawing({signature:?})")
        }
        RunContent::Text(text) => format!("Text({:?})", text_signature(text)),
        RunContent::DeletedText(text) => format!("DeletedText({:?})", text_signature(text)),
        content => format!("{content:?}"),
    }
}

/// The text and whether its `xml:space="preserve"` changes how it reads.
///
/// The flag only protects whitespace at an edge of the text. Producers write
/// it on every `w:t` or only where needed, so elsewhere it is serialization.
fn text_signature(text: &CT_Text) -> (&str, bool) {
    (
        &text.text,
        text.preserve_space && has_edge_whitespace(&text.text),
    )
}

/// Whether XML whitespace starts or ends the text.
pub(super) fn has_edge_whitespace(text: &str) -> bool {
    let whitespace = |character: char| matches!(character, ' ' | '\t' | '\n' | '\r');
    text.starts_with(whitespace) || text.ends_with(whitespace)
}

fn table_signature(table: &CT_Tbl) -> String {
    format!(
        "{:?}:{:?}:{:?}:{:?}",
        table.grid,
        table.rows.iter().map(row_signature).collect::<Vec<_>>(),
        table.extra_xml,
        table
            .content_controls
            .iter()
            .map(|(at, raw_before, control)| (at, raw_before, control_signature(control)))
            .collect::<Vec<_>>()
    )
}

fn row_signature(row: &CT_Row) -> String {
    format!(
        "{:?}:{:?}:{:?}",
        row.cells.iter().map(cell_signature).collect::<Vec<_>>(),
        row_raw_children(row),
        row.content_controls
            .iter()
            .map(|(at, raw_before, control)| (at, raw_before, control_signature(control)))
            .collect::<Vec<_>>()
    )
}

fn cell_signature(cell: &oxoffice_docx_oxml::table::CT_Tc) -> String {
    format!(
        "{:?}:{:?}",
        cell.content
            .iter()
            .map(|content| match content {
                CellContent::Paragraph(paragraph) => paragraph_signature(paragraph),
                CellContent::Table(table) => table_signature(table),
                CellContent::ContentControl(control) => control_signature(control),
            })
            .collect::<Vec<_>>(),
        cell.extra_xml
    )
}

fn control_signature(control: &CT_Sdt) -> String {
    let properties = control_property_signature(control);
    format!(
        "{:?}:{:?}",
        properties,
        control
            .content
            .iter()
            .filter_map(|content| match content {
                SdtContent::Paragraph(paragraph) => Some(paragraph_signature(paragraph)),
                SdtContent::Table(table) => Some(table_signature(table)),
                SdtContent::Row(row) => Some(row_signature(row)),
                SdtContent::Cell(cell) => Some(cell_signature(cell)),
                SdtContent::Run(run) => Some(run_signature(run)),
                SdtContent::ContentControl(control) => Some(control_signature(control)),
                SdtContent::RawXml(raw) if raw.iter().all(u8::is_ascii_whitespace) => None,
                SdtContent::RawXml(raw) => Some(format!("{raw:?}")),
            })
            .collect::<Vec<_>>()
    )
}

/// The content-control properties that decide what a control holds, its type
/// and data binding, which alignment, refusal and the accept and reject
/// postconditions compare.
///
/// `w:id` is left out because producers renumber it on save, and the
/// [`CONTROL_METADATA`] because a difference there is reported as a
/// diagnostic. A pair that differs only by those keeps the original's
/// `w:sdtPr`. A `w:sdtPr` with neither property reads like no `w:sdtPr`.
pub(super) fn control_property_signature(control: &CT_Sdt) -> ControlPropertySignature<'_> {
    control
        .properties
        .as_ref()
        .map(|properties| (properties.control_type, properties.data_binding.as_ref()))
        .filter(|signature| !matches!(signature, (None, None)))
}

/// Report each [`CONTROL_METADATA`] property that differs between the two
/// controls. The redline keeps the original `w:sdtPr`.
pub(super) fn control_metadata_diagnostics(
    original: &CT_Sdt,
    edited: &CT_Sdt,
    location: &str,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<()> {
    if original.properties == edited.properties {
        return Ok(());
    }
    let edited_values = control_metadata(edited)?;
    for ((name, original_value), edited_value) in CONTROL_METADATA
        .iter()
        .zip(control_metadata(original)?)
        .zip(edited_values)
    {
        if original_value != edited_value {
            diagnostics.push(ComparisonDiagnostic {
                location: location.to_owned(),
                message: format!(
                    "content-control {name} differs and the original {name} was retained"
                ),
            });
        }
    }
    Ok(())
}

/// The [`CONTROL_METADATA`] values of a control, in that order.
///
/// The lock, placeholder and gallery are kept as raw `w:sdtPr` children, so
/// they are read from the serialized properties by local name.
fn control_metadata(control: &CT_Sdt) -> Result<[Option<String>; 5]> {
    let Some(properties) = &control.properties else {
        return Ok(Default::default());
    };
    let mut values = [
        properties.tag.clone(),
        properties.alias.clone(),
        None,
        None,
        None,
    ];
    let mut shell = control.clone();
    shell.content.clear();
    let xml = control_xml(&shell)?;
    let mut reader = Reader::from_str(&xml);
    let mut path = Vec::<Vec<u8>>::new();
    let mut buffer = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!("comparison content-control scan failed: {error}"))
        })?;
        let (element, empty) = match event {
            Event::Start(element) => (element, false),
            Event::Empty(element) => (element, true),
            Event::End(_) => {
                path.pop();
                buffer.clear();
                continue;
            }
            Event::Eof => break,
            _ => {
                buffer.clear();
                continue;
            }
        };
        let local = element.local_name().as_ref().to_vec();
        let parents = path.iter().map(Vec::as_slice).collect::<Vec<_>>();
        let slot = match (parents.as_slice(), local.as_slice()) {
            ([b"sdt", b"sdtPr"], b"lock") => Some(2),
            ([b"sdt", b"sdtPr", b"placeholder"], b"docPart") => Some(3),
            ([b"sdt", b"sdtPr", b"docPartObj" | b"docPartList"], b"docPartGallery") => Some(4),
            _ => None,
        };
        if let Some(slot) = slot {
            let mut value = String::new();
            for attribute in element.attributes() {
                let attribute = attribute.map_err(|error| {
                    Error::Other(format!(
                        "comparison content-control attribute failed: {error}"
                    ))
                })?;
                if attribute.key.local_name().as_ref() == b"val" {
                    value = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                        .map_err(|error| {
                            Error::Other(format!(
                                "comparison content-control value failed: {error}"
                            ))
                        })?
                        .into_owned();
                }
            }
            values[slot] = Some(value);
        }
        if !empty {
            path.push(local);
        }
        buffer.clear();
    }
    Ok(values)
}

pub(super) fn modeled_control_content(control: &CT_Sdt) -> Vec<&SdtContent> {
    control
        .content
        .iter()
        .filter(|content| {
            !matches!(content, SdtContent::RawXml(raw) if raw.iter().all(u8::is_ascii_whitespace))
        })
        .collect()
}

pub(super) fn control_whitespace_slots(control: &CT_Sdt) -> Result<Vec<String>> {
    let modeled_count = modeled_control_content(control).len();
    let mut slots = vec![String::new(); modeled_count + 1];
    let mut modeled_before = 0usize;
    for content in &control.content {
        match content {
            SdtContent::RawXml(raw) if raw.iter().all(u8::is_ascii_whitespace) => {
                slots[modeled_before].push_str(std::str::from_utf8(raw).map_err(utf8_error)?);
            }
            _ => modeled_before += 1,
        }
    }
    Ok(slots)
}

fn normalized_body(document: &CT_Document) -> Vec<String> {
    document.body.content.iter().map(body_signature).collect()
}

/// The accept and reject postcondition projection of a story: each item's
/// policy signature followed by the modeled properties of every paragraph it
/// owns. The signature leaves formatting out because alignment matches changed
/// paragraphs through it, so without the properties a resolution that loses
/// a paragraph's alignment or mark formatting would pass.
pub(super) fn coalesced_postcondition_body(
    document: &CT_Document,
    options: &ComparisonOptions,
) -> Vec<String> {
    let scalar = ComparisonOptions {
        granularity: ComparisonGranularity::Character,
        ..options.clone()
    };
    postcondition_body(document, &scalar)
}

pub(super) fn postcondition_body(
    document: &CT_Document,
    options: &ComparisonOptions,
) -> Vec<String> {
    let mut items = normalized_body_with_options(document, options);
    if options.ignore_formatting {
        return items;
    }
    for (item, content) in items.iter_mut().zip(&document.body.content) {
        let mut properties = Vec::new();
        match content {
            BodyContent::Paragraph(paragraph) => {
                properties.push(modeled_paragraph_properties(paragraph.properties.as_ref()));
            }
            BodyContent::Table(table) => table_paragraph_properties(table, &mut properties),
            BodyContent::ContentControl(control) => {
                control_paragraph_properties(control, &mut properties);
            }
            BodyContent::RawXml(_) => {}
        }
        item.push_str(&format!(":{properties:?}"));
    }
    items
}

fn table_paragraph_properties(table: &CT_Tbl, properties: &mut Vec<Option<CT_PPr>>) {
    for cell in table.rows.iter().flat_map(|row| &row.cells) {
        cell_paragraph_properties(cell, properties);
    }
}

fn cell_paragraph_properties(cell: &CT_Tc, properties: &mut Vec<Option<CT_PPr>>) {
    for content in &cell.content {
        match content {
            CellContent::Paragraph(paragraph) => {
                properties.push(modeled_paragraph_properties(paragraph.properties.as_ref()));
            }
            CellContent::Table(table) => table_paragraph_properties(table, properties),
            CellContent::ContentControl(control) => {
                control_paragraph_properties(control, properties);
            }
        }
    }
}

fn control_paragraph_properties(control: &CT_Sdt, properties: &mut Vec<Option<CT_PPr>>) {
    for content in &control.content {
        match content {
            SdtContent::Paragraph(paragraph) => {
                properties.push(modeled_paragraph_properties(paragraph.properties.as_ref()));
            }
            SdtContent::Table(table) => table_paragraph_properties(table, properties),
            SdtContent::Row(row) => {
                for cell in &row.cells {
                    cell_paragraph_properties(cell, properties);
                }
            }
            SdtContent::Cell(cell) => cell_paragraph_properties(cell, properties),
            SdtContent::ContentControl(control) => {
                control_paragraph_properties(control, properties);
            }
            SdtContent::Run(_) | SdtContent::RawXml(_) => {}
        }
    }
}

pub(super) fn normalized_body_with_options(
    document: &CT_Document,
    options: &ComparisonOptions,
) -> Vec<String> {
    if options == &ComparisonOptions::default() {
        return normalized_body(document);
    }
    document
        .body
        .content
        .iter()
        .map(|content| body_signature_with_options(content, options))
        .collect()
}

pub(super) fn body_signature_with_options(
    content: &BodyContent,
    options: &ComparisonOptions,
) -> String {
    match content {
        BodyContent::Paragraph(paragraph) => {
            format!("p:{}", paragraph_signature_with_options(paragraph, options))
        }
        BodyContent::Table(table) => {
            format!("t:{}", table_signature_with_options(table, options))
        }
        BodyContent::ContentControl(control) => {
            format!("s:{}", control_signature_with_options(control, options))
        }
        BodyContent::RawXml(raw) => format!("x:{raw:?}"),
    }
}

pub(super) fn paragraph_signature_with_options(
    paragraph: &CT_P,
    options: &ComparisonOptions,
) -> String {
    let numbering = (!options.ignore_formatting)
        .then(|| paragraph_numbering(paragraph))
        .flatten();
    let hyperlinks = paragraph
        .hyperlinks
        .iter()
        .map(|link| {
            (
                classified_hyperlink(link).rel_id,
                &link.anchor,
                &link.tooltip,
                &link.doc_location,
                &link.extra_attributes,
                hyperlink_raw_boundaries(paragraph, link, options),
                link.preserved_raw_before,
                policy_run_boundary(paragraph, link.run_start, options),
                policy_run_boundary(paragraph, link.run_end, options),
            )
        })
        .collect::<Vec<_>>();
    format!(
        "{numbering:?}:{:?}:{hyperlinks:?}",
        paragraph_tokens(paragraph, options)
    )
}

/// One token of a paragraph signature.
#[derive(PartialEq, Eq)]
enum ParagraphToken {
    /// A compared unit, or a whole run on the whole-run path.
    Unit(String),
    /// Consecutive plain characters at character granularity. One token
    /// holds the whole stretch, so the signature costs one token per stretch
    /// of text and still reads the same however the runs split it.
    Scalars(String),
    /// A unit the options ignore, such as whitespace, a field or a comment
    /// reference.
    Ignored(String),
    /// A preserved raw child, comment marker, inline control or hyperlink edge.
    Boundary(String),
}

/// Paragraph content as tokens in document order.
///
/// Raw children, comment markers and inline controls come out at each run
/// boundary in the order the serializer writes them, and each run follows
/// as its compared units. Splitting or merging runs therefore cannot move
/// a marker, and a run without content cannot reorder its neighbours.
/// Ignored units are kept only where they touch a boundary token, so a
/// marker on either side of ignored content still differs.
fn paragraph_tokens(paragraph: &CT_P, options: &ComparisonOptions) -> Vec<String> {
    let attributed = uses_attributed_run_path(options);
    let mut tokens = Vec::new();
    let mut open_hyperlink = None;
    for boundary in 0..=paragraph.runs.len() {
        let inside = paragraph
            .hyperlinks
            .iter()
            .rposition(|link| link.run_start <= boundary && boundary < link.run_end);
        if let Some(index) = open_hyperlink
            && open_hyperlink != inside
        {
            tokens.push(ParagraphToken::Boundary(format!("hyperlink-end:{index}")));
            open_hyperlink = None;
        }
        push_boundary_tokens(paragraph, boundary, options, &mut tokens);
        for (index, _) in paragraph.hyperlinks.iter().enumerate().filter(|(_, link)| {
            link.run_start == boundary
                && link.run_end == boundary
                && link.preserved_raw_before.is_none()
        }) {
            tokens.push(ParagraphToken::Boundary(format!("hyperlink-empty:{index}")));
        }
        let Some(run) = paragraph.runs.get(boundary) else {
            break;
        };
        if let Some(index) = inside
            && open_hyperlink != inside
        {
            tokens.push(ParagraphToken::Boundary(format!("hyperlink-start:{index}")));
            open_hyperlink = inside;
        }
        if !attributed {
            let signature = run_signature_with_options(run, options);
            if !signature.is_empty() {
                tokens.push(ParagraphToken::Unit(signature));
            }
            continue;
        }
        for unit in signature_run_units(run, options) {
            if unit_is_empty(&unit) {
                continue;
            }
            if let Some(text) = signature_scalars(&unit, options) {
                if let Some(ParagraphToken::Scalars(stretch)) = tokens.last_mut() {
                    stretch.push_str(text);
                } else {
                    tokens.push(ParagraphToken::Scalars(text.to_owned()));
                }
                continue;
            }
            let signature = attributed_unit_signature(&unit);
            tokens.push(if unit_is_ignorable(&unit) {
                ParagraphToken::Ignored(signature)
            } else {
                ParagraphToken::Unit(signature)
            });
        }
    }
    tokens
        .dedup_by(|next, previous| matches!(next, ParagraphToken::Ignored(_)) && next == previous);
    let ignored = |index: usize| matches!(tokens.get(index), Some(ParagraphToken::Ignored(_)));
    let boundary = |index: usize| matches!(tokens.get(index), Some(ParagraphToken::Boundary(_)));
    let mut keep = vec![true; tokens.len()];
    let mut start = 0;
    while start < tokens.len() {
        if !ignored(start) {
            start += 1;
            continue;
        }
        let end = (start..tokens.len())
            .find(|index| !ignored(*index))
            .unwrap_or(tokens.len());
        let touches_boundary = (start > 0 && boundary(start - 1)) || boundary(end);
        keep[start..end].fill(touches_boundary);
        start = end;
    }
    let mut kept = Vec::<ParagraphToken>::new();
    for (token, _) in tokens.into_iter().zip(keep).filter(|(_, keep)| *keep) {
        // A stretch continues over ignored units that left no token.
        if let (ParagraphToken::Scalars(text), Some(ParagraphToken::Scalars(stretch))) =
            (&token, kept.last_mut())
        {
            stretch.push_str(text);
        } else {
            kept.push(token);
        }
    }
    kept.into_iter()
        .map(|token| match token {
            ParagraphToken::Unit(signature) => format!("unit:{signature}"),
            ParagraphToken::Scalars(text) => format!("scalars:{text:?}"),
            ParagraphToken::Ignored(signature) | ParagraphToken::Boundary(signature) => signature,
        })
        .collect()
}

/// The units a signature reads `run` as.
///
/// At character granularity a run of plain content is read one stretch at a
/// time, which gives the tokens its characters would give once
/// [`signature_scalars`] joins them. A raw child and deleted text belong to
/// single characters, so a run that holds one is read character by character.
fn signature_run_units(run: &CT_R, options: &ComparisonOptions) -> Vec<AttributedRunUnit> {
    let stretches = options.granularity == ComparisonGranularity::Character
        && run.extra_xml.is_empty()
        && run.alt_drawings.is_empty()
        && !run
            .content
            .iter()
            .any(|content| matches!(content, RunContent::DeletedText(_)));
    if stretches {
        attributed_run_units(
            std::slice::from_ref(run),
            &ComparisonOptions {
                granularity: ComparisonGranularity::Run,
                ..options.clone()
            },
        )
    } else {
        attributed_run_units(std::slice::from_ref(run), options)
    }
}

/// The plain characters of a unit, which a character-granularity signature
/// holds with those before and after them as one token.
fn signature_scalars<'a>(
    unit: &'a AttributedRunUnit,
    options: &ComparisonOptions,
) -> Option<&'a str> {
    (options.granularity == ComparisonGranularity::Character)
        .then(|| unit_plain_text(unit))
        .flatten()
        .filter(|text| !text.is_empty())
}

/// Push the raw children, comment markers and inline controls at one run
/// boundary in the order `CT_P` serialization writes them.
fn push_boundary_tokens(
    paragraph: &CT_P,
    boundary: usize,
    options: &ComparisonOptions,
    tokens: &mut Vec<ParagraphToken>,
) {
    let extras = paragraph
        .extra_xml
        .iter()
        .filter(|(position, raw)| {
            *position == boundary && !CT_P::raw_is_root_attributes(*position, raw)
        })
        .map(|(_, raw)| raw)
        .collect::<Vec<_>>();
    for raw_index in 0..=extras.len() {
        let markers = paragraph
            .comment_ranges
            .iter()
            .filter_map(|marker| {
                let (start, id, run_index, raw_before, has_child_content) = match *marker {
                    CommentRangeMarker::Start {
                        id,
                        run_index,
                        raw_before,
                        has_child_content,
                    } => (true, id, run_index, raw_before, has_child_content),
                    CommentRangeMarker::End {
                        id,
                        run_index,
                        raw_before,
                        has_child_content,
                    } => (false, id, run_index, raw_before, has_child_content),
                };
                (run_index == boundary && raw_before.min(extras.len()) == raw_index)
                    .then(|| format!("comment-range:{start}:{id}:{has_child_content}"))
            })
            .collect::<Vec<_>>();
        for marker_index in 0..=markers.len() {
            for (_, _, _, control) in
                paragraph
                    .content_controls
                    .iter()
                    .filter(|(at, raw_before, markers_before, _)| {
                        *at == boundary
                            && (*raw_before).min(extras.len()) == raw_index
                            && (*markers_before).min(markers.len()) == marker_index
                    })
            {
                tokens.push(ParagraphToken::Boundary(format!(
                    "control:{}",
                    control_signature_with_options(control, options)
                )));
            }
            if let Some(marker) = markers.get(marker_index)
                && !options.ignore_comments
            {
                tokens.push(ParagraphToken::Boundary(marker.clone()));
            }
        }
        if let Some(raw) = extras.get(raw_index) {
            tokens.push(ParagraphToken::Boundary(format!(
                "raw:{:?}",
                raw_child_identity(paragraph, boundary, raw_index, raw)
            )));
        }
    }
}

/// The raw children of `paragraph` with their run boundaries, each as
/// [`raw_child_identity`] reads it; the record of its root attributes is not
/// one.
pub(super) fn paragraph_raw_children(paragraph: &CT_P) -> Vec<(usize, std::borrow::Cow<'_, [u8]>)> {
    let mut slots = HashMap::<usize, usize>::new();
    paragraph
        .extra_xml
        .iter()
        .filter(|(position, raw)| !CT_P::raw_is_root_attributes(*position, raw))
        .map(|(position, raw)| {
            let slot = slots.entry(*position).or_default();
            let identity = raw_child_identity(paragraph, *position, *slot, raw);
            *slot += 1;
            (*position, identity)
        })
        .collect()
}

/// Keep the raw children of `paragraph` that `keep` accepts (it may rewrite
/// them), moving each equation that follows a removed one at its boundary so
/// that it still pairs with its raw child, as [`raw_child_identity`] reads
/// it. An equation whose raw child goes, goes with it.
pub(super) fn retain_raw_children(
    paragraph: &mut CT_P,
    mut keep: impl FnMut(&mut Vec<u8>) -> bool,
) {
    let mut slots = HashMap::<usize, usize>::new();
    let mut removed = Vec::new();
    paragraph.extra_xml.retain_mut(|(position, raw)| {
        let slot = slots.entry(*position).or_default();
        let current = *slot;
        *slot += 1;
        let kept = keep(raw);
        if !kept {
            removed.push((*position, current));
        }
        kept
    });
    if removed.is_empty() {
        return;
    }
    paragraph.equations.retain_mut(|(at, slot, _)| {
        if removed.contains(&(*at, *slot)) {
            return false;
        }
        *slot -= removed
            .iter()
            .filter(|(position, before)| position == at && before < slot)
            .count();
        true
    });
}

/// The raw child of `paragraph` at `slot` among those at run boundary
/// `position`, as the comparison reads it. An equation reads as its model
/// writes it, in one canonical writing (`OfficeMath::canonical_xml`): the
/// same equation a save wrote again, with the root's namespaces declared on
/// it or a default its source left out (a fraction's bar), reads the same.
/// Any other raw child reads as its bytes.
pub(super) fn raw_child_identity<'a>(
    paragraph: &CT_P,
    position: usize,
    slot: usize,
    raw: &'a [u8],
) -> std::borrow::Cow<'a, [u8]> {
    paragraph
        .equations
        .iter()
        .find(|(at, equation_slot, _)| *at == position && *equation_slot == slot)
        .and_then(|(_, _, equation)| equation.canonical_xml().ok())
        .map_or(std::borrow::Cow::Borrowed(raw), std::borrow::Cow::Owned)
}

pub(super) fn run_signature_with_options(run: &CT_R, options: &ComparisonOptions) -> String {
    if options == &ComparisonOptions::default() {
        return run_signature(run);
    }
    let content = run
        .content
        .iter()
        .filter_map(|content| {
            if options.ignore_comments && matches!(content, RunContent::CommentReference { .. }) {
                return None;
            }
            if options.ignore_fields && matches!(content, RunContent::Field(_)) {
                return Some("ignored:field".to_owned());
            }
            match content {
                RunContent::Text(text) | RunContent::DeletedText(text)
                    if options.ignore_whitespace =>
                {
                    let visible = text
                        .text
                        .chars()
                        .filter(|character| !character.is_whitespace())
                        .collect::<String>();
                    (!visible.is_empty()).then_some(format!("text:{visible:?}"))
                }
                _ => Some(run_content_signature(content)),
            }
        })
        .collect::<Vec<_>>();
    let raw = semantic_run_raw(run);
    if content.is_empty() && raw.is_empty() {
        String::new()
    } else {
        format!("{content:?}:{raw:?}")
    }
}

pub(super) fn table_signature_with_options(table: &CT_Tbl, options: &ComparisonOptions) -> String {
    format!(
        "{:?}:{:?}:{:?}:{:?}",
        table.grid,
        table
            .rows
            .iter()
            .map(|row| row_signature_with_options(row, options))
            .collect::<Vec<_>>(),
        table.extra_xml,
        table
            .content_controls
            .iter()
            .map(|(at, raw_before, control)| (
                at,
                raw_before,
                control_signature_with_options(control, options),
            ))
            .collect::<Vec<_>>()
    )
}

pub(super) fn row_signature_with_options(row: &CT_Row, options: &ComparisonOptions) -> String {
    format!(
        "{:?}:{:?}:{:?}",
        row.cells
            .iter()
            .map(|cell| cell_signature_with_options(cell, options))
            .collect::<Vec<_>>(),
        row_raw_children(row),
        row.content_controls
            .iter()
            .map(|(at, raw_before, control)| (
                at,
                raw_before,
                control_signature_with_options(control, options),
            ))
            .collect::<Vec<_>>()
    )
}

fn cell_signature_with_options(cell: &CT_Tc, options: &ComparisonOptions) -> String {
    format!(
        "{:?}:{:?}",
        cell.content
            .iter()
            .map(|content| match content {
                CellContent::Paragraph(paragraph) => {
                    paragraph_signature_with_options(paragraph, options)
                }
                CellContent::Table(table) => table_signature_with_options(table, options),
                CellContent::ContentControl(control) => {
                    control_signature_with_options(control, options)
                }
            })
            .collect::<Vec<_>>(),
        cell.extra_xml
    )
}

pub(super) fn control_signature_with_options(
    control: &CT_Sdt,
    options: &ComparisonOptions,
) -> String {
    let mut content = Vec::new();
    // The plain characters since the last token, written as one token.
    let mut scalars = String::new();
    let close = |scalars: &mut String, content: &mut Vec<String>| {
        if !scalars.is_empty() {
            content.push(format!("u:scalars:{:?}", std::mem::take(scalars)));
        }
    };
    for child in &control.content {
        match child {
            SdtContent::RawXml(raw) if raw.iter().all(u8::is_ascii_whitespace) => {}
            SdtContent::Run(run) if uses_attributed_run_path(options) => {
                for unit in signature_run_units(run, options)
                    .into_iter()
                    .filter(|unit| !unit_is_ignorable(unit) && !unit_is_empty(unit))
                {
                    if let Some(text) = signature_scalars(&unit, options) {
                        scalars.push_str(text);
                    } else {
                        close(&mut scalars, &mut content);
                        content.push(format!("u:{}", attributed_unit_signature(&unit)));
                    }
                }
            }
            child => {
                close(&mut scalars, &mut content);
                content.push(control_content_signature_with_options(child, options));
            }
        }
    }
    close(&mut scalars, &mut content);
    format!("{:?}:{:?}", control_property_signature(control), content)
}

pub(super) fn control_content_signature_with_options(
    content: &SdtContent,
    options: &ComparisonOptions,
) -> String {
    match content {
        SdtContent::Paragraph(paragraph) => {
            format!("p:{}", paragraph_signature_with_options(paragraph, options))
        }
        SdtContent::Table(table) => {
            format!("t:{}", table_signature_with_options(table, options))
        }
        SdtContent::Row(row) => format!("r:{}", row_signature_with_options(row, options)),
        SdtContent::Cell(cell) => format!("c:{}", cell_signature_with_options(cell, options)),
        SdtContent::Run(run) => format!("u:{}", run_signature_with_options(run, options)),
        SdtContent::ContentControl(control) => {
            format!("s:{}", control_signature_with_options(control, options))
        }
        SdtContent::RawXml(raw) => format!("x:{raw:?}"),
    }
}

pub(super) fn paragraph_formatting(paragraph: &CT_P) -> Option<CT_PPr> {
    paragraph.properties.clone().and_then(|mut properties| {
        properties.num_id = None;
        properties.num_ilvl = None;
        properties.num_id_raw = None;
        properties.num_ilvl_raw = None;
        properties.numbering_revision = None;
        properties.numbering_revision_xml.clear();
        properties.numbering_revision_xml_positions.clear();
        properties.numbering_revision_position = None;
        properties.change = None;
        properties.revision_xml.clear();
        if let Some(section) = properties.sect_pr.as_mut() {
            clear_default_orientation(section);
        }
        nonempty_paragraph_properties(properties)
    })
}

/// Row properties as the model writes them, without their revision records.
/// An empty container, such as the `<w:trPr></w:trPr>` LibreOffice writes,
/// reads as no properties.
pub(super) fn row_formatting(row: &CT_Row) -> Result<Vec<u8>> {
    let mut writer = Writer::new(Vec::new());
    if let Some(mut properties) = row.properties.clone() {
        properties.revision_markers.clear();
        properties.revision_xml.clear();
        properties.from_source = false;
        properties.to_xml(&mut writer)?;
    }
    Ok(writer.into_inner())
}

/// Cell properties as the model writes them. Empty containers, such as
/// LibreOffice's `<w:tcBorders></w:tcBorders>` or the `<w:tcPr/>` Google
/// Docs writes, read as no properties, although a save keeps them.
pub(super) fn cell_formatting(cell: &CT_Tc) -> Result<Vec<u8>> {
    let mut writer = Writer::new(Vec::new());
    if let Some(mut properties) = cell.properties.clone() {
        properties.from_source = false;
        if properties
            .borders
            .as_ref()
            .is_some_and(oxoffice_docx_oxml::table::CT_TblBorders::is_empty)
        {
            properties.borders = None;
        }
        properties.to_xml(&mut writer)?;
    }
    Ok(writer.into_inner())
}
