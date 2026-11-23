//! Paragraph and section properties in a comparison, and deleted or inserted paragraphs.

use super::*;

pub(super) fn paragraph_properties_xml(
    original: &CT_P,
    edited: &CT_P,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let source = metadata
        .paragraph_source(original)
        .map(|xml| direct_property(xml, "pPr"))
        .transpose()?
        .flatten();
    let generated =
        paragraph_properties_xml_impl(original, edited, location, metadata, diagnostics)?;
    let before = original
        .properties
        .as_ref()
        .map(property_xml)
        .transpose()?
        .unwrap_or_default();
    let output = patch_property_source(&before, &generated, source.as_deref(), &metadata.ids)?;
    let output = if let Some(source) = source {
        preserve_prior_property_sources(&output, &source)?
    } else {
        metadata.ids.generated(output)?
    };
    if let Some(paragraph) = metadata.paragraph_source(original)
        && let Some(span) = direct_word_element_spans(paragraph, "pPr")?.first()
    {
        let context = root_namespace_bindings(paragraph, "p", &[])?;
        restore_current_property_roots(&output, &paragraph[span.clone()], &context)
    } else {
        Ok(output)
    }
}

fn paragraph_properties_xml_impl(
    original: &CT_P,
    edited: &CT_P,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let original_section = original
        .properties
        .as_ref()
        .and_then(|properties| properties.sect_pr.as_ref());
    let edited_section = edited
        .properties
        .as_ref()
        .and_then(|properties| properties.sect_pr.as_ref());
    let tracked_section = section_properties_xml(
        original_section,
        edited_section,
        &format!("{location}/section"),
        metadata,
        diagnostics,
    )?;
    if metadata.options.ignore_formatting {
        return original
            .properties
            .as_ref()
            .map(property_xml)
            .transpose()
            .map(Option::unwrap_or_default);
    }

    let original_modeled = modeled_paragraph_properties(original.properties.as_ref());
    let edited_modeled = modeled_paragraph_properties(edited.properties.as_ref());
    let changed = compared_paragraph_properties(original_modeled.clone())
        != compared_paragraph_properties(edited_modeled.clone());
    let same_base = compared_paragraph_properties(split_paragraph_mark(original_modeled.clone()).0)
        == compared_paragraph_properties(split_paragraph_mark(edited_modeled.clone()).0);
    let mut current = if changed && !same_base {
        edited.properties.clone().unwrap_or_default()
    } else {
        // A change of the mark alone keeps the original's own base, so that
        // the level a `w:numId` without `w:ilvl` shows stays unwritten.
        let mut current = original.properties.clone().unwrap_or_default();
        if changed {
            current.rpr = edited
                .properties
                .as_ref()
                .and_then(|edited| edited.rpr.clone());
        }
        current
    };
    current.sect_pr = None;
    current.change = None;
    let (
        original_numbering_xml,
        original_numbering_positions,
        original_numbering_position,
        original_revision_xml,
        original_revision_positions,
    ) = original
        .properties
        .as_ref()
        .map(|properties| {
            (
                properties.numbering_revision_xml.clone(),
                properties.numbering_revision_xml_positions.clone(),
                properties.numbering_revision_position,
                properties.revision_xml.clone(),
                properties.revision_xml_positions.clone(),
            )
        })
        .unwrap_or_default();
    let edited_properties = edited.properties.as_ref().cloned().unwrap_or_default();
    if edited_properties.numbering_revision_xml != original_numbering_xml
        || edited_properties.numbering_revision_xml_positions != original_numbering_positions
        || edited_properties.numbering_revision_position != original_numbering_position
        || edited_properties.revision_xml != original_revision_xml
        || edited_properties.revision_xml_positions != original_revision_positions
    {
        formatting_diagnostic(diagnostics, location.to_owned());
    }
    current.numbering_revision_xml = original_numbering_xml;
    current.numbering_revision_xml_positions = original_numbering_positions;
    current.numbering_revision_position = original_numbering_position;
    current.revision_xml = original_revision_xml;
    current.revision_xml_positions = original_revision_positions;
    let needs_owner = changed || !tracked_section.is_empty() || original.properties.is_some();
    if !needs_owner {
        return Ok(String::new());
    }
    let mut current_xml = property_xml(&current)?;
    if current_xml.is_empty() {
        current_xml.push_str("<w:pPr></w:pPr>");
    }
    // `w:pPrChange` holds only the base properties, so the mark records its
    // own change in a `w:rPrChange`, as Word writes it.
    let (original_base, original_mark) = split_paragraph_mark(original_modeled);
    let (edited_base, edited_mark) = split_paragraph_mark(edited_modeled);
    if original_mark != edited_mark {
        let previous = run_property_xml(original_mark.as_ref())?;
        let change =
            metadata
                .ids
                .revision("rPrChange", metadata.author, metadata.timestamp, &previous)?;
        if let Some(span) = direct_word_element_spans(&current_xml, "rPr")?
            .into_iter()
            .next()
        {
            let updated = append_word_child(&current_xml[span.clone()], "rPr", &change)?;
            current_xml.replace_range(span, &updated);
        } else {
            current_xml = inject_before_close(
                &current_xml,
                "</w:pPr>",
                &format!("<w:rPr>{change}</w:rPr>"),
            )?;
        }
    }
    if !tracked_section.is_empty() {
        current_xml = inject_before_close(&current_xml, "</w:pPr>", &tracked_section)?;
    }
    if compared_paragraph_properties(original_base.clone())
        != compared_paragraph_properties(edited_base.clone())
    {
        let previous = original_base
            .as_ref()
            .map(property_xml)
            .transpose()?
            .filter(|xml| !xml.is_empty())
            .unwrap_or_else(|| "<w:pPr/>".to_owned());
        let change =
            metadata
                .ids
                .revision("pPrChange", metadata.author, metadata.timestamp, &previous)?;
        current_xml = inject_before_close(&current_xml, "</w:pPr>", &change)?;
    }
    metadata.ids.generated(current_xml)
}

/// Modeled paragraph properties without the paragraph mark, and the mark.
fn split_paragraph_mark(
    properties: Option<CT_PPr>,
) -> (
    Option<CT_PPr>,
    Option<oxoffice_docx_oxml::properties::CT_RPr>,
) {
    match properties {
        Some(mut properties) => {
            let mark = properties.rpr.take();
            (nonempty_paragraph_properties(properties), mark)
        }
        None => (None, None),
    }
}

/// Whether `source` is one element without children or attributes, namespace
/// declarations aside, such as LibreOffice's `<w:rPr></w:rPr>`.
fn bare_empty_element(source: &str) -> bool {
    let bare = |element: &BytesStart<'_>| {
        element.attributes().flatten().all(|attribute| {
            attribute.key.as_ref() == b"xmlns" || attribute.key.as_ref().starts_with(b"xmlns:")
        })
    };
    let mut reader = Reader::from_str(source);
    reader.config_mut().trim_text(true);
    match reader.read_event() {
        Ok(Event::Empty(element)) => bare(&element),
        Ok(Event::Start(element)) if bare(&element) => {
            matches!(reader.read_event(), Ok(Event::End(_)))
        }
        _ => false,
    }
}

/// Paragraph properties without an empty mark `w:rPr`, which LibreOffice
/// writes `<w:rPr></w:rPr>` and the model writes back as nothing, and none
/// when nothing else is left.
fn without_empty_mark(xml: Option<&str>) -> Result<Option<String>> {
    let Some(xml) = xml else {
        return Ok(None);
    };
    let mut stripped = xml.to_owned();
    for span in direct_word_element_spans(xml, "rPr")?.into_iter().rev() {
        if bare_empty_element(&xml[span.clone()]) {
            stripped.replace_range(span, "");
        }
    }
    Ok((!bare_empty_element(&stripped)).then_some(stripped))
}

/// The `w:pPr` of a [`final_paragraph`]: the `own` properties when both
/// resolutions agree, otherwise the `accepted` ones with the section break of
/// `own`, a `w:pPrChange` holding the rejected paragraph properties, and a
/// mark `w:rPrChange` holding the rejected mark formatting.
///
/// Rejecting a `w:pPrChange` keeps a mark `w:rPr` only when it holds a
/// revision, so a mark with formatting also carries the `w:rPrChange` when
/// only the paragraph properties differ.
pub(super) fn final_paragraph_properties_xml(
    own: &PropertySide<'_>,
    accepted: &PropertySide<'_>,
    rejected: &PropertySide<'_>,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    let accepted_modeled =
        compared_paragraph_properties(modeled_paragraph_properties(accepted.typed));
    let rejected_modeled =
        compared_paragraph_properties(modeled_paragraph_properties(rejected.typed));
    // Not skipped with `ignore_formatting`: the two sides are the properties
    // of two paragraphs that a resolution merges, not a formatting edit, and
    // Word's merged paragraph keeps this mark's unless the change says so.
    if accepted_modeled == rejected_modeled
        && without_empty_mark(accepted.xml.as_deref())?
            == without_empty_mark(rejected.xml.as_deref())?
    {
        return Ok(own.xml.clone().unwrap_or_default());
    }
    let mut current = accepted
        .xml
        .clone()
        .unwrap_or(metadata.ids.generated("<w:pPr/>".to_owned())?);
    let own_section = own
        .xml
        .as_deref()
        .map(|xml| direct_property(xml, "sectPr"))
        .transpose()?
        .flatten()
        .unwrap_or_default();
    current = replace_property_slot(&current, "sectPr", &own_section)?;
    // An empty mark carries no format, written either way.
    let mark = |side: &PropertySide<'_>| -> Result<Option<String>> {
        Ok(side
            .xml
            .as_deref()
            .map(|xml| direct_property(xml, "rPr"))
            .transpose()?
            .flatten()
            .filter(|mark| !bare_empty_element(mark)))
    };
    let accepted_mark = mark(accepted)?;
    let rejected_mark = mark(rejected)?;
    if accepted_mark != rejected_mark {
        let previous = rejected_mark
            .clone()
            .unwrap_or(metadata.ids.generated("<w:rPr/>".to_owned())?);
        let change =
            metadata
                .ids
                .revision("rPrChange", metadata.author, metadata.timestamp, &previous)?;
        let mark = accepted_mark
            .clone()
            .unwrap_or(metadata.ids.generated("<w:rPr/>".to_owned())?);
        let updated = append_word_child(&mark, "rPr", &change)?;
        current = replace_property_slot(&current, "rPr", &updated)?;
    }
    let base = |side: &PropertySide<'_>| -> Result<Option<String>> {
        side.xml
            .as_deref()
            .map(|xml| without_property_children(xml, &["rPr", "sectPr", "pPrChange"]))
            .transpose()
    };
    let accepted_base = base(accepted)?;
    let rejected_base = base(rejected)?;
    if accepted_base != rejected_base {
        let previous = rejected_base.unwrap_or(metadata.ids.generated("<w:pPr/>".to_owned())?);
        let change =
            metadata
                .ids
                .revision("pPrChange", metadata.author, metadata.timestamp, &previous)?;
        current = append_word_child(&current, "pPr", &change)?;
    }
    Ok(current)
}

pub(super) fn modeled_paragraph_properties(properties: Option<&CT_PPr>) -> Option<CT_PPr> {
    properties.cloned().and_then(|mut properties| {
        properties.sect_pr = None;
        properties.num_ilvl_raw = None;
        properties.num_id_raw = None;
        properties.numbering_revision = None;
        properties.numbering_revision_xml.clear();
        properties.numbering_revision_xml_positions.clear();
        properties.numbering_revision_position = None;
        properties.change = None;
        properties.revision_xml.clear();
        properties.revision_xml_positions.clear();
        nonempty_paragraph_properties(properties)
    })
}

/// `properties` as two paragraphs' properties are compared, never written: a
/// `w:numId` without `w:ilvl` takes the level both inputs show.
pub(super) fn compared_paragraph_properties(properties: Option<CT_PPr>) -> Option<CT_PPr> {
    properties.map(|mut properties| {
        super::numbering::normalize_numbering(&mut properties);
        properties
    })
}

pub(super) fn nonempty_paragraph_properties(mut properties: CT_PPr) -> Option<CT_PPr> {
    if properties
        .rpr
        .as_ref()
        .is_some_and(|mark| *mark == oxoffice_docx_oxml::properties::CT_RPr::default())
    {
        properties.rpr = None;
    }
    (properties != CT_PPr::default()).then_some(properties)
}

pub(super) fn paragraph_properties_differ(
    original: Option<&CT_PPr>,
    edited: Option<&CT_PPr>,
) -> bool {
    let modeled = |properties: Option<&CT_PPr>| {
        properties.cloned().and_then(|mut properties| {
            if let Some(section) = properties.sect_pr.as_mut() {
                clear_default_orientation(section);
            }
            super::numbering::normalize_numbering(&mut properties);
            nonempty_paragraph_properties(properties)
        })
    };
    modeled(original) != modeled(edited)
}

/// Portrait is the schema default, which the section writer omits.
pub(super) fn clear_default_orientation(section: &mut oxoffice_docx_oxml::document::CT_SectPr) {
    if section.orientation == Some(ST_PageOrientation::Portrait) {
        section.orientation = None;
    }
}

pub(super) fn section_properties_xml(
    original: Option<&oxoffice_docx_oxml::document::CT_SectPr>,
    edited: Option<&oxoffice_docx_oxml::document::CT_SectPr>,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    let (Some(original), Some(edited)) = (original, edited) else {
        return match (original, edited) {
            (None, None) => Ok(String::new()),
            _ => Err(Error::Other(format!(
                "comparison cannot create or remove a section shell at {location}"
            ))),
        };
    };
    if original.header_refs != edited.header_refs || original.footer_refs != edited.footer_refs {
        return Err(Error::Other(format!(
            "comparison cannot change related-story references at {location}"
        )));
    }
    if metadata.options.ignore_formatting {
        return section_property_xml(original);
    }
    let mut original_modeled = original.clone();
    original_modeled.change = None;
    let mut edited_modeled = edited.clone();
    edited_modeled.change = None;
    clear_default_orientation(&mut original_modeled);
    clear_default_orientation(&mut edited_modeled);
    // The prefixes in scope where a page-number restart was read, which a
    // save declares differently, are not the section's.
    let read_context_free = |section: &oxoffice_docx_oxml::document::CT_SectPr| {
        let mut section = section.clone();
        if let Some(page_number) = &mut section.page_number {
            page_number.reserved_insertion_prefixes.clear();
        }
        section
    };
    if original_modeled == edited_modeled
        || read_context_free(&original_modeled) == read_context_free(&edited_modeled)
    {
        return section_property_xml(original);
    }
    if original.extra_xml != edited.extra_xml {
        formatting_diagnostic(diagnostics, location.to_owned());
        edited_modeled.extra_xml = original.extra_xml.clone();
    }
    let previous = section_property_xml(&original_modeled)?;
    let change = metadata.ids.revision(
        "sectPrChange",
        metadata.author,
        metadata.timestamp,
        &previous,
    )?;
    let current = section_property_xml(&edited_modeled)?;
    metadata
        .ids
        .generated(inject_before_close(&current, "</w:sectPr>", &change)?)
}

/// A paragraph deleted whole. Unless it is `carried` (deleted inside a complex
/// field replaced with it, which owns its simple fields), its simple fields
/// are written as whole complex fields (see [`paragraph_children_xml`]).
pub(super) fn deleted_paragraph(
    paragraph: &CT_P,
    carried: bool,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    let (mut output, closing) = paragraph_shell(metadata.paragraph_source(paragraph))?;
    output.push_str(&paragraph_mark_properties(paragraph, "del", metadata)?);
    let (mut children, mut published) =
        paragraph_children_xml(paragraph, true, !carried, metadata)?;
    place_shared_bookmarks(&mut children, &mut published, true, metadata)?;
    output.push_str(&crate::revision::tracked_paragraph_children(
        &children,
        true,
        false,
        &published,
        &mut |content| {
            metadata
                .ids
                .revision("del", metadata.author, metadata.timestamp, content)
        },
    )?);
    output.push_str(&closing);
    metadata.ids.generated(output)
}

/// A paragraph inserted whole: [`deleted_paragraph`] for an insertion.
pub(super) fn inserted_paragraph(
    paragraph: &CT_P,
    carried: bool,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    let (mut output, closing) = paragraph_shell(metadata.paragraph_source(paragraph))?;
    output.push_str(&paragraph_mark_properties(paragraph, "ins", metadata)?);
    let (mut children, mut published) =
        paragraph_children_xml(paragraph, false, !carried, metadata)?;
    place_shared_bookmarks(&mut children, &mut published, false, metadata)?;
    output.push_str(&crate::revision::tracked_paragraph_children(
        &children,
        false,
        false,
        &published,
        &mut |content| {
            metadata
                .ids
                .revision("ins", metadata.author, metadata.timestamp, content)
        },
    )?);
    output.push_str(&closing);
    metadata.ids.generated(output)
}

/// Write each bookmark both inputs hold once, outside the revision marks, in
/// the whole deleted or inserted paragraphs that hold it.
///
/// No tracked change records a bookmark, and Word's Reject All removes a
/// bookmark whose start and end both sit in an inserted paragraph. So a whole
/// inserted paragraph keeps such a bookmark's start where the edited input
/// has it, and a whole deleted one its end where the original has it; the
/// insertion comes first (see `replace_changed_paragraph_runs`). Rejecting
/// leaves the start at the restored paragraph's start, accepting leaves the
/// end just after the inserted paragraph's mark, both over the same text. A
/// bookmark the edited input holds as a point stays whole in the insertion.
/// The bookmarks written that way are recorded for the postconditions.
pub(super) fn place_shared_bookmarks(
    children: &mut String,
    published: &mut Vec<Range<usize>>,
    deleted: bool,
    metadata: &mut Metadata<'_>,
) -> Result<()> {
    for (range, id, start) in shared_bookmark_markers(children, &metadata.shared_bookmarks)?
        .into_iter()
        .rev()
    {
        metadata.relocated_bookmarks.insert(id);
        let point = metadata.point_bookmarks.contains(&id);
        // The insertion writes the starts and the points, the deletion the
        // other ends.
        if deleted == (start || point) {
            for field in published.iter_mut() {
                if field.start >= range.end {
                    *field = field.start - range.len()..field.end - range.len();
                }
            }
            children.replace_range(range, "");
        } else {
            published.push(range);
        }
    }
    Ok(())
}

/// The direct `w:bookmarkStart` and `w:bookmarkEnd` children of a paragraph's
/// XML whose bookmark both inputs hold, as byte ranges, ids and whether each
/// starts.
pub(super) fn shared_bookmark_markers(
    xml: &str,
    shared: &HashSet<i32>,
) -> Result<Vec<(Range<usize>, i32, bool)>> {
    if shared.is_empty() {
        return Ok(Vec::new());
    }
    let opening = "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>";
    let input = format!("{opening}{xml}</w:body></w:document>");
    let tree = crate::revision::XmlTree::parse(input.as_bytes())?;
    let Some(paragraph) = tree
        .elements
        .iter()
        .position(|element| element.word && element.local == "p")
    else {
        return Ok(Vec::new());
    };
    let mut markers = Vec::new();
    for &child in &tree.elements[paragraph].children {
        let element = &tree.elements[child];
        let start = match element.local.as_str() {
            "bookmarkStart" => true,
            "bookmarkEnd" => false,
            _ => continue,
        };
        if !element.word {
            continue;
        }
        if let Some(id) = word_attributes(&tree, input.as_bytes(), child)?
            .get("id")
            .and_then(|id| id.parse::<i32>().ok())
            .filter(|id| shared.contains(id))
        {
            markers.push((
                element.start - opening.len()..element.end - opening.len(),
                id,
                start,
            ));
        }
    }
    Ok(markers)
}

/// Stage a whole paragraph without its properties, retaining actual run source
/// and destination namespace custody before the shared revision publisher wraps
/// eligible payloads. Field and hyperlink shells remain that publisher's owners.
///
/// With `whole_fields`, each simple field of the paragraph is written as one
/// complex field, the whole of it in an `ins` (or, `deleted`, a `del`) of its
/// own under one new revision id: a `w:fldSimple` cannot carry a mark, and
/// resolving the paragraph would leave its shell. That is the shape Word
/// writes for a field deleted with its paragraph. The byte ranges of those
/// wrappers are returned for the publisher to keep as they are.
pub(super) fn paragraph_children_xml(
    paragraph: &CT_P,
    deleted: bool,
    whole_fields: bool,
    metadata: &mut Metadata<'_>,
) -> Result<(String, Vec<Range<usize>>)> {
    let mut children = paragraph.clone();
    children.properties = None;
    if deleted {
        children.runs = children.runs.iter().map(deleted_run).collect();
    }
    let mut xml = paragraph_xml(&children)?;
    let spans = modeled_paragraph_run_spans(&children, &xml)?;
    // One read of the paragraph answers the scope of every run.
    let parents = ParentScopes::new(&xml, spans.iter().map(|span| span.start).collect());
    let mut replacements = Vec::new();
    let mut fields = Vec::new();
    let mut index = 0;
    while index < children.runs.len() {
        if let Some((coverage, _)) = children.field_source_run_fragments_at(index)? {
            index += coverage;
            continue;
        }
        let run = &paragraph.runs[index];
        if whole_fields && let Some(field) = run_field(run).filter(|field| !field.is_complex()) {
            fields.push(replacements.len());
            replacements.push((
                spans[index].clone(),
                whole_simple_field(field, deleted, metadata)?,
            ));
            index += 1;
            continue;
        }
        if let Some(source) = metadata.run_source(run) {
            let span = spans[index].clone();
            replacements.push((span.clone(), {
                let (raw, typed) = complete_run_selection(&paragraph.runs[index]);
                preserve_run_source(
                    &xml[span.clone()],
                    &run_property_xml(run.properties.as_ref())?,
                    [source, source],
                    &children.runs[index],
                    &raw,
                    &typed,
                    &metadata.ids.revision_parent_scope(&parents.at(span.start)?),
                    &metadata.ids,
                )
            }?));
        }
        index += 1;
    }
    let mut published = Vec::with_capacity(fields.len());
    let mut shift = 0isize;
    for (position, (span, replacement)) in replacements.iter().enumerate() {
        let start = span.start.checked_add_signed(shift).ok_or_else(|| {
            Error::Other("whole-paragraph field publication offset overflow".into())
        })?;
        if fields.contains(&position) {
            published.push(start..start + replacement.len());
        }
        shift += replacement.len() as isize - span.len() as isize;
    }
    for (span, replacement) in replacements.into_iter().rev() {
        xml.replace_range(span, &replacement);
    }
    Ok((xml, published))
}

fn paragraph_mark_properties(
    paragraph: &CT_P,
    kind: &str,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    let marker = metadata
        .ids
        .marker(kind, metadata.author, metadata.timestamp)?;
    let source = PropertySide::of(paragraph, metadata)?;
    let properties = source
        .xml
        .unwrap_or(metadata.ids.generated("<w:pPr/>".to_owned())?);
    marked_paragraph_properties_xml(&properties, &marker, &metadata.ids)
}
