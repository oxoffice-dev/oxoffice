//! Checking a Word owner fragment, and the checked disposal of paragraphs.

use super::*;

struct CheckedTextOwner {
    name: Option<Vec<u8>>,
    content_seen: bool,
    properties_seen: bool,
    last_property_slot: Option<u8>,
    property_names: HashSet<Vec<u8>>,
    property_leaf: bool,
    sdt_owner: Option<SdtOwner>,
}

/// Admit a complete fragment before any typed recursive construction.
/// The preservation reader may retain unmodelled children. Checked owner
/// replacement additionally requires qualified roots and modeled p/r order.
pub(crate) fn validate_word_owner_fragment(xml: &[u8], root: &[u8], checked: bool) -> Result<()> {
    if checked {
        oxoffice_core::xml::validate_strict_xml_1_0(xml)
            .map_err(|error| OxmlError::InvalidValue(format!("element XML: {error:?}")))?;
    }
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut stack = Vec::<CheckedTextOwner>::new();
    let mut root_seen = false;
    let mut buffer = Vec::new();
    loop {
        let (namespace, event) = reader.read_resolved_event_into(&mut buffer)?;
        let word = matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == crate::namespace::W_NS.as_bytes()
            || std::str::from_utf8(uri).ok().and_then(|uri| quick_xml::escape::unescape(uri).ok()).is_some_and(|uri| uri == crate::namespace::W_NS));
        let mut modeled = word && stack.last().is_none_or(|frame| frame.name.is_some());
        let mut property_leaf = false;
        let mut sdt_owner = stack.last().and_then(|frame| frame.sdt_owner);
        match event {
            Event::DocType(_) => {
                return Err(OxmlError::InvalidValue(
                    "element XML cannot contain a DTD or entity declaration".into(),
                ));
            }
            Event::Start(ref element) | Event::Empty(ref element) => {
                let name = element.local_name();
                let local = name.as_ref();
                if checked && stack.last().is_some_and(|frame| frame.property_leaf) {
                    return Err(OxmlError::InvalidValue(
                        "element XML known property leaf cannot contain child elements".into(),
                    ));
                }
                if stack.is_empty() {
                    if root_seen || local != root || (checked && !word) {
                        return Err(OxmlError::InvalidValue(format!(
                            "element XML requires exactly one qualified Word {} root",
                            String::from_utf8_lossy(root)
                        )));
                    }
                    root_seen = true;
                }
                if checked && modeled {
                    if let Some(frame) = stack.last_mut()
                        && let Some(parent) = frame.name.as_deref()
                    {
                        let content_seen = &mut frame.content_seen;
                        let properties_seen = &mut frame.properties_seen;
                        if let Some((slot, repeatable)) =
                            crate::table::checked_owner_child_slot(parent, local)?
                        {
                            if frame.last_property_slot.is_some_and(|last| last > slot)
                                || (!repeatable && !frame.property_names.insert(local.to_vec()))
                            {
                                return Err(OxmlError::InvalidValue("element XML table owner has duplicate or out-of-order known children".into()));
                            }
                            frame.last_property_slot = Some(slot);
                        }
                        if parent == b"sectPr"
                            && let Some((slot, repeatable)) =
                                crate::document::checked_section_child_slot(local)
                        {
                            if frame.last_property_slot.is_some_and(|last| last > slot)
                                || (!repeatable && !frame.property_names.insert(local.to_vec()))
                            {
                                return Err(OxmlError::InvalidValue("element XML section has duplicate or out-of-order known children".into()));
                            }
                            frame.last_property_slot = Some(slot);
                        }
                        let properties = match parent {
                            b"p" => Some(b"pPr".as_slice()),
                            b"r" => Some(b"rPr".as_slice()),
                            _ => None,
                        };
                        if let Some(properties) = properties {
                            if local == properties {
                                if *content_seen || *properties_seen {
                                    return Err(OxmlError::InvalidValue(format!(
                                        "element XML {} must occur once before content",
                                        String::from_utf8_lossy(properties)
                                    )));
                                }
                                *properties_seen = true;
                            } else {
                                *content_seen = true;
                                if matches!(
                                    local,
                                    b"p" | b"tbl" | b"tr" | b"tc" | b"sectPr" | b"pPr" | b"rPr"
                                ) || (parent == b"r"
                                    && matches!(local, b"r" | b"hyperlink" | b"fldSimple" | b"sdt"))
                                {
                                    return Err(OxmlError::InvalidValue(format!(
                                        "element XML {} is not a valid direct child of {}",
                                        String::from_utf8_lossy(local),
                                        String::from_utf8_lossy(parent)
                                    )));
                                }
                            }
                        }
                    }
                } else if checked
                    && let Some(frame) = stack.last_mut()
                    && matches!(frame.name.as_deref(), Some(b"p" | b"r"))
                {
                    frame.content_seen = true;
                }
                if checked && modeled {
                    let mut prefixes = Vec::new();
                    for (prefix, namespace) in reader.resolver().bindings() {
                        if namespace.as_ref() == crate::namespace::W_NS.as_bytes()
                            || std::str::from_utf8(namespace.as_ref())
                                .ok()
                                .and_then(|uri| quick_xml::escape::unescape(uri).ok())
                                .is_some_and(|uri| uri == crate::namespace::W_NS)
                        {
                            let prefix = match prefix {
                                quick_xml::name::PrefixDeclaration::Default => "",
                                quick_xml::name::PrefixDeclaration::Named(prefix) => {
                                    std::str::from_utf8(prefix)?
                                }
                            };
                            prefixes.push(prefix.to_owned());
                        }
                    }
                    if stack
                        .last()
                        .is_some_and(|frame| frame.name.as_deref() == Some(b"sectPr"))
                    {
                        for (prefix, namespace) in reader.resolver().bindings() {
                            let prefix = match prefix {
                                quick_xml::name::PrefixDeclaration::Default => "",
                                quick_xml::name::PrefixDeclaration::Named(prefix) => {
                                    std::str::from_utf8(prefix)?
                                }
                            };
                            prefixes.push(format!(
                                "\0{prefix}\0{}",
                                std::str::from_utf8(namespace.as_ref())?
                            ));
                        }
                        property_leaf =
                            crate::document::validate_checked_section_child(element, &prefixes)?;
                    }
                    if let Some(frame) = stack.last_mut() {
                        if frame.name.as_deref() == Some(b"numPr")
                            && matches!(local, b"ilvl" | b"numId")
                        {
                            let slot = u8::from(local == b"numId");
                            if !frame.property_names.insert(local.to_vec())
                                || frame.last_property_slot.is_some_and(|last| last > slot)
                            {
                                return Err(OxmlError::InvalidValue("element XML numbering has duplicate or out-of-order properties".into()));
                            }
                            frame.last_property_slot = Some(slot);
                            crate::paragraph_properties::validate_checked_property_child(
                                element, &prefixes,
                            )?;
                            property_leaf = true;
                        }
                        if let Some(parent) = frame.name.as_deref()
                            && matches!(parent, b"tblPr" | b"tblPrEx" | b"tcPr" | b"trPr")
                        {
                            property_leaf = crate::table::validate_checked_property_child(
                                element, &prefixes, parent,
                            )?;
                        }
                        let slot = match frame.name.as_deref() {
                            Some(b"pPr") => crate::paragraph_properties::ppr_schema_slot(
                                element.name().as_ref(),
                                &prefixes,
                            ),
                            Some(b"rPr") => crate::run_properties::rpr_schema_slot(
                                element.name().as_ref(),
                                &prefixes,
                            ),
                            _ => None,
                        };
                        if matches!(frame.name.as_deref(), Some(b"pPr" | b"rPr")) {
                            if let Some(slot) = slot {
                                let paragraph_properties = frame.name.as_deref() == Some(b"pPr");
                                if (paragraph_properties
                                    && (!frame.property_names.insert(local.to_vec())
                                        || frame
                                            .last_property_slot
                                            .is_some_and(|last| last > slot)))
                                    || (!paragraph_properties
                                        && frame.property_names.contains(b"rPrChange".as_slice()))
                                {
                                    return Err(OxmlError::InvalidValue(format!(
                                        "element XML {} has duplicate or out-of-order known properties",
                                        String::from_utf8_lossy(
                                            frame.name.as_deref().unwrap_or_default()
                                        )
                                    )));
                                }
                                frame.last_property_slot = Some(slot);
                                frame.property_names.insert(local.to_vec());
                                property_leaf = if paragraph_properties {
                                    !matches!(
                                        local,
                                        b"numPr"
                                            | b"pBdr"
                                            | b"tabs"
                                            | b"rPr"
                                            | b"sectPr"
                                            | b"pPrChange"
                                    )
                                } else {
                                    local != b"rPrChange"
                                };
                                if frame.name.as_deref() == Some(b"pPr") {
                                    crate::paragraph_properties::validate_checked_property_child(
                                        element, &prefixes,
                                    )?;
                                } else {
                                    crate::run_properties::validate_checked_property_child(
                                        element, &prefixes,
                                    )?;
                                }
                            } else {
                                modeled = false;
                            }
                        }
                    }
                }
                if checked
                    && modeled
                    && let Some(parent) = stack.last().and_then(|frame| frame.name.as_deref())
                {
                    if local == b"sdt" {
                        sdt_owner = match parent {
                            b"p" => Some(SdtOwner::Inline),
                            b"tbl" => Some(SdtOwner::Table),
                            b"tr" => Some(SdtOwner::Row),
                            b"tc" => Some(SdtOwner::Cell),
                            _ => sdt_owner,
                        };
                    }
                    modeled = match parent {
                        b"p" => {
                            local == b"pPr" || crate::document::admitted_word_child(parent, local)
                        }
                        b"r" => {
                            local == b"rPr" || crate::document::admitted_word_child(parent, local)
                        }
                        b"tbl" | b"tr" | b"tc" => {
                            crate::table::checked_owner_child_slot(parent, local)?.is_some()
                        }
                        b"sectPr" => crate::document::checked_section_child_slot(local).is_some(),
                        b"sdt" => local == b"sdtContent",
                        b"sdtContent" => {
                            local == b"sdt" || sdt_owner.is_some_and(|owner| owner.owns(local))
                        }
                        b"hyperlink" | b"fldSimple" | b"ruby" | b"rt" | b"rubyBase" | b"ins"
                        | b"del" | b"moveFrom" | b"moveTo" => {
                            crate::document::admitted_word_child(parent, local)
                        }
                        _ => modeled,
                    };
                }
                if matches!(event, Event::Start(_)) {
                    stack.push(CheckedTextOwner {
                        name: modeled.then(|| local.to_vec()),
                        content_seen: false,
                        properties_seen: false,
                        last_property_slot: None,
                        property_names: HashSet::new(),
                        property_leaf,
                        sdt_owner,
                    });
                }
            }
            Event::End(_) => {
                stack.pop().ok_or_else(|| {
                    OxmlError::InvalidValue("element XML has an unmatched end".into())
                })?;
            }
            Event::Text(ref text)
                if stack.is_empty()
                    || (checked
                        && stack.last().is_some_and(|frame| {
                            frame.property_leaf
                                || frame
                                    .name
                                    .as_deref()
                                    .is_some_and(|name| matches!(name, b"p" | b"r"))
                        })) =>
            {
                if text.as_ref().iter().any(|byte| !byte.is_ascii_whitespace()) {
                    return Err(OxmlError::InvalidValue(
                        "element XML has text outside an allowed text owner".into(),
                    ));
                }
            }
            Event::CData(_) | Event::GeneralRef(_)
                if stack.is_empty()
                    || (checked
                        && stack.last().is_some_and(|frame| {
                            frame.property_leaf
                                || frame
                                    .name
                                    .as_deref()
                                    .is_some_and(|name| matches!(name, b"p" | b"r"))
                        })) =>
            {
                return Err(OxmlError::InvalidValue(
                    "element XML has content outside an allowed text owner".into(),
                ));
            }
            Event::Eof => {
                if !root_seen || !stack.is_empty() {
                    return Err(OxmlError::InvalidValue(
                        "element XML requires one complete root".into(),
                    ));
                }
                return Ok(());
            }
            _ => {}
        }
        buffer.clear();
    }
}

// These concrete owned frames are used only when a checked replacement
// discards its candidate or displaces its old paragraph. They do not change
// public Drop or move semantics. Boxes keep the work frontier compact.
enum CheckedDiscard {
    Paragraph(Box<CT_P>),
    Run(Box<CT_R>),
    Field(Box<Field>),
    Body(Box<crate::document::CT_Body>),
    Control(Box<CT_Sdt>),
    Table(Box<crate::table::CT_Tbl>),
    Row(Box<crate::table::CT_Row>),
    Cell(Box<crate::table::CT_Tc>),
    Revision(Box<CT_Revision>),
    Drawing(Box<CT_Drawing>),
}

fn discard_instruction(instruction: FieldInstruction, pending: &mut Vec<CheckedDiscard>) {
    for argument in instruction.arguments.into_iter().chain(
        instruction
            .switches
            .into_iter()
            .filter_map(|switch| switch.argument),
    ) {
        match argument {
            FieldArgument::Text(_) => {}
            FieldArgument::Nested(field) => pending.push(CheckedDiscard::Field(field)),
            FieldArgument::Joined(pieces) => {
                for piece in pieces {
                    if let FieldArgument::Nested(field) = piece {
                        pending.push(CheckedDiscard::Field(field));
                    }
                }
            }
        }
    }
}
fn discard_run_properties(
    properties: crate::properties::CT_RPr,
    pending: &mut Vec<CheckedDiscard>,
) {
    pending.extend(
        properties
            .revision_markers
            .into_iter()
            .map(|revision| CheckedDiscard::Revision(Box::new(revision))),
    );
    if let Some(revision) = properties.change {
        pending.push(CheckedDiscard::Revision(revision));
    }
}
fn discard_paragraph_properties(
    properties: crate::paragraph_properties::CT_PPr,
    pending: &mut Vec<CheckedDiscard>,
) {
    if let Some(properties) = properties.rpr {
        discard_run_properties(properties, pending);
    }
    for revision in [properties.numbering_revision, properties.change]
        .into_iter()
        .flatten()
    {
        pending.push(CheckedDiscard::Revision(Box::new(revision)));
    }
    if let Some(properties) = properties.sect_pr
        && let Some(revision) = properties.change
    {
        pending.push(CheckedDiscard::Revision(Box::new(revision)));
    }
}
fn discard_segments(segments: Vec<CachedDisplaySegment>, pending: &mut Vec<CheckedDiscard>) {
    for segment in segments {
        if let Some(properties) = segment.properties {
            discard_run_properties(properties, pending);
        }
    }
}

/// Dispose only a checked replacement's private candidate/displaced owner.
/// Shared Arcs are released without mutating another live owner. Unique Arc
/// models join the same frontier, including retired source/cache alternatives.
pub(crate) fn discard_checked_paragraph(paragraph: CT_P) {
    let mut pending = vec![CheckedDiscard::Paragraph(Box::new(paragraph))];
    while let Some(owner) = pending.pop() {
        match owner {
            CheckedDiscard::Paragraph(paragraph) => {
                if let Some(properties) = paragraph.properties {
                    discard_paragraph_properties(properties, &mut pending);
                }
                pending.extend(
                    paragraph
                        .runs
                        .into_iter()
                        .map(|run| CheckedDiscard::Run(Box::new(run))),
                );
                pending.extend(
                    paragraph
                        .content_controls
                        .into_iter()
                        .map(|(_, _, _, control)| CheckedDiscard::Control(Box::new(control))),
                );
                pending.extend(
                    paragraph
                        .revisions
                        .into_iter()
                        .map(|(_, _, revision)| CheckedDiscard::Revision(Box::new(revision))),
                );
                for ruby in paragraph.rubies {
                    pending.extend(
                        ruby.ruby_text
                            .into_iter()
                            .map(|run| CheckedDiscard::Run(Box::new(run))),
                    );
                }
            }
            CheckedDiscard::Run(run) => {
                if let Some(properties) = run.properties {
                    discard_run_properties(properties, &mut pending);
                }
                for content in run.content {
                    match content {
                        RunContent::Field(field) => {
                            pending.push(CheckedDiscard::Field(Box::new(field)))
                        }
                        RunContent::Drawing(drawing) => {
                            pending.push(CheckedDiscard::Drawing(Box::new(drawing)))
                        }
                        _ => {}
                    }
                }
                pending.extend(
                    run.alt_drawings
                        .into_iter()
                        .map(|drawing| CheckedDiscard::Drawing(Box::new(drawing))),
                );
            }
            CheckedDiscard::Field(field) => {
                discard_instruction(field.instruction, &mut pending);
                pending.extend(
                    field
                        .cached_fields
                        .into_iter()
                        .map(|(_, field)| CheckedDiscard::Field(Box::new(field))),
                );
                for runs in [
                    field.simple_cached_runs,
                    field.typed_cached_runs,
                    field.parsed_cached_runs,
                ]
                .into_iter()
                .flatten()
                {
                    pending.extend(
                        runs.into_iter()
                            .map(|run| CheckedDiscard::Run(Box::new(run))),
                    );
                }
                if let Some(ordered) = field.ordered_result
                    && let Some(paragraph) = Arc::into_inner(ordered.paragraph)
                {
                    pending.push(CheckedDiscard::Paragraph(Box::new(paragraph)));
                }
                if let Some(span) = field.span
                    && let Some(span) = Arc::into_inner(span)
                {
                    pending.extend(
                        span.runs
                            .into_iter()
                            .flatten()
                            .map(|run| CheckedDiscard::Run(Box::new(run))),
                    );
                }
                if let Some(source) = Arc::into_inner(field.source) {
                    match source {
                        FieldSource::New {
                            original_instruction,
                            cached_runs,
                            cached_segments,
                            ..
                        } => {
                            discard_instruction(original_instruction, &mut pending);
                            if let Some(runs) = cached_runs {
                                pending.extend(
                                    runs.into_iter()
                                        .map(|run| CheckedDiscard::Run(Box::new(run))),
                                );
                            }
                            discard_segments(cached_segments, &mut pending);
                        }
                        FieldSource::Parsed {
                            original_instruction,
                            cached_segments,
                            ..
                        } => {
                            discard_instruction(original_instruction, &mut pending);
                            discard_segments(cached_segments, &mut pending);
                        }
                    }
                }
            }
            CheckedDiscard::Body(body) => {
                if let Some(properties) = body.sect_pr
                    && let Some(revision) = properties.change
                {
                    pending.push(CheckedDiscard::Revision(Box::new(revision)));
                }
                for content in body.content {
                    match content {
                        crate::document::BodyContent::Paragraph(paragraph) => {
                            pending.push(CheckedDiscard::Paragraph(paragraph))
                        }
                        crate::document::BodyContent::Table(table) => {
                            pending.push(CheckedDiscard::Table(table))
                        }
                        crate::document::BodyContent::ContentControl(control) => {
                            pending.push(CheckedDiscard::Control(control))
                        }
                        crate::document::BodyContent::RawXml(_) => {}
                    }
                }
            }
            CheckedDiscard::Control(control) => {
                let (content, revisions, originals) = control.into_discard_parts();
                pending.extend(
                    revisions
                        .into_iter()
                        .map(|revision| CheckedDiscard::Revision(Box::new(revision))),
                );
                pending.extend(
                    originals
                        .into_iter()
                        .map(|run| CheckedDiscard::Run(Box::new(run))),
                );
                for content in content {
                    match content {
                        crate::content_control::SdtContent::Paragraph(paragraph) => {
                            pending.push(CheckedDiscard::Paragraph(Box::new(paragraph)))
                        }
                        crate::content_control::SdtContent::Run(run) => {
                            pending.push(CheckedDiscard::Run(Box::new(run)))
                        }
                        crate::content_control::SdtContent::Table(table) => {
                            pending.push(CheckedDiscard::Table(Box::new(table)))
                        }
                        crate::content_control::SdtContent::Row(row) => {
                            pending.push(CheckedDiscard::Row(Box::new(row)))
                        }
                        crate::content_control::SdtContent::Cell(cell) => {
                            pending.push(CheckedDiscard::Cell(Box::new(cell)))
                        }
                        crate::content_control::SdtContent::ContentControl(control) => {
                            pending.push(CheckedDiscard::Control(Box::new(control)))
                        }
                        crate::content_control::SdtContent::RawXml(_) => {}
                    }
                }
            }
            CheckedDiscard::Table(table) => {
                if let Some(properties) = table.properties
                    && let Some(revision) = properties.change
                {
                    pending.push(CheckedDiscard::Revision(Box::new(revision)));
                }
                pending.extend(
                    table
                        .rows
                        .into_iter()
                        .map(|row| CheckedDiscard::Row(Box::new(row))),
                );
                pending.extend(
                    table
                        .content_controls
                        .into_iter()
                        .map(|(_, _, control)| CheckedDiscard::Control(Box::new(control))),
                );
            }
            CheckedDiscard::Row(row) => {
                if let Some(properties) = row.properties {
                    pending.extend(
                        properties
                            .revision_markers
                            .into_iter()
                            .map(|revision| CheckedDiscard::Revision(Box::new(revision))),
                    );
                }
                pending.extend(
                    row.cells
                        .into_iter()
                        .map(|cell| CheckedDiscard::Cell(Box::new(cell))),
                );
                pending.extend(
                    row.content_controls
                        .into_iter()
                        .map(|(_, _, control)| CheckedDiscard::Control(Box::new(control))),
                );
            }
            CheckedDiscard::Cell(cell) => {
                for content in cell.content {
                    match content {
                        crate::table::CellContent::Paragraph(paragraph) => {
                            pending.push(CheckedDiscard::Paragraph(Box::new(paragraph)))
                        }
                        crate::table::CellContent::Table(table) => {
                            pending.push(CheckedDiscard::Table(Box::new(table)))
                        }
                        crate::table::CellContent::ContentControl(control) => {
                            pending.push(CheckedDiscard::Control(Box::new(control)))
                        }
                    }
                }
            }
            CheckedDiscard::Revision(revision) => {
                let (content, paragraph, nested) = revision.into_discard_parts();
                if let Some(paragraph) = paragraph
                    && let Some(paragraph) = Arc::into_inner(paragraph)
                {
                    pending.push(CheckedDiscard::Paragraph(Box::new(paragraph)));
                }
                pending.extend(
                    nested
                        .into_iter()
                        .map(|revision| CheckedDiscard::Revision(Box::new(revision))),
                );
                match content {
                    crate::revision::RevisionContent::Runs(runs) => pending.extend(
                        runs.into_iter()
                            .map(|run| CheckedDiscard::Run(Box::new(run))),
                    ),
                    crate::revision::RevisionContent::PriorRunProperties(properties) => {
                        discard_run_properties(*properties, &mut pending)
                    }
                    crate::revision::RevisionContent::PriorParagraphProperties(properties) => {
                        discard_paragraph_properties(*properties, &mut pending)
                    }
                    crate::revision::RevisionContent::PriorTableProperties(properties) => {
                        if let Some(revision) = properties.change {
                            pending.push(CheckedDiscard::Revision(Box::new(revision)));
                        }
                    }
                    crate::revision::RevisionContent::PriorSectionProperties(properties) => {
                        if let Some(revision) = properties.change {
                            pending.push(CheckedDiscard::Revision(Box::new(revision)));
                        }
                    }
                    crate::revision::RevisionContent::Marker => {}
                }
            }
            CheckedDiscard::Drawing(drawing) => {
                if let Some(anchor) = drawing.anchor
                    && let Some(shape) = anchor.shape
                {
                    if let Some(body) = shape.text_body {
                        pending.push(CheckedDiscard::Body(Box::new(body)));
                    }
                    pending.extend(
                        shape
                            .text
                            .into_iter()
                            .map(|paragraph| CheckedDiscard::Paragraph(Box::new(paragraph))),
                    );
                }
            }
        }
    }
}
