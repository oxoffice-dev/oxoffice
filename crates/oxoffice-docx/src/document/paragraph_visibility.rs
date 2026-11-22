//! Which paragraphs a reader sees, from the model and from the main part source.

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ParagraphVisibilityScope {
    FacadeRecursive,
    DirectBody,
}

type ParagraphVisibilityPath = Vec<(&'static [u8], usize)>;
type ParagraphVisibilityOwners<'a> = Vec<(ParagraphVisibilityPath, &'a CT_P)>;

// Pair typed owners with their structural encounter slots. Raw children and
// formatting are not owner slots. Tables directly in body or cells stay opaque.
fn paragraph_visibility_owners(
    document: &Document,
    scope: ParagraphVisibilityScope,
) -> ParagraphVisibilityOwners<'_> {
    fn control<'a>(
        sdt: &'a CT_Sdt,
        owner: &'static [u8],
        path: &[(&'static [u8], usize)],
        out: &mut ParagraphVisibilityOwners<'a>,
    ) {
        let mut slot = 0;
        for child in &sdt.content {
            let name: &'static [u8] = match child {
                SdtContent::Paragraph(_) if matches!(owner, b"body" | b"tc") => b"p",
                SdtContent::Table(_) if matches!(owner, b"body" | b"tc") => b"tbl",
                SdtContent::Row(_) if owner == b"tbl" => b"tr",
                SdtContent::Cell(_) if owner == b"tr" => b"tc",
                SdtContent::ContentControl(_) => b"sdt",
                _ => continue,
            };
            let mut next = path.to_vec();
            next.push((name, slot));
            slot += 1;
            match child {
                SdtContent::Paragraph(p) => out.push((next, p)),
                SdtContent::Table(t) => table(t, &next, out),
                SdtContent::Row(r) => row(r, &next, out),
                SdtContent::Cell(c) => cell(c, &next, out),
                SdtContent::ContentControl(c) => control(c, owner, &next, out),
                _ => {}
            }
        }
    }
    fn table<'a>(
        t: &'a CT_Tbl,
        path: &[(&'static [u8], usize)],
        out: &mut ParagraphVisibilityOwners<'a>,
    ) {
        let mut slot = 0;
        for index in 0..=t.rows.len() {
            for (_, _, c) in t.content_controls.iter().filter(|(at, _, _)| *at == index) {
                let mut next = path.to_vec();
                next.push((b"sdt", slot));
                slot += 1;
                control(c, b"tbl", &next, out);
            }
            if let Some(r) = t.rows.get(index) {
                let mut next = path.to_vec();
                next.push((b"tr", slot));
                slot += 1;
                row(r, &next, out);
            }
        }
    }
    fn row<'a>(
        r: &'a CT_Row,
        path: &[(&'static [u8], usize)],
        out: &mut ParagraphVisibilityOwners<'a>,
    ) {
        let mut slot = 0;
        for index in 0..=r.cells.len() {
            for (_, _, c) in r.content_controls.iter().filter(|(at, _, _)| *at == index) {
                let mut next = path.to_vec();
                next.push((b"sdt", slot));
                slot += 1;
                control(c, b"tr", &next, out);
            }
            if let Some(c) = r.cells.get(index) {
                let mut next = path.to_vec();
                next.push((b"tc", slot));
                slot += 1;
                cell(c, &next, out);
            }
        }
    }
    fn cell<'a>(
        c: &'a CT_Tc,
        path: &[(&'static [u8], usize)],
        out: &mut ParagraphVisibilityOwners<'a>,
    ) {
        for (slot, child) in c.content.iter().enumerate() {
            let name: &'static [u8] = match child {
                CellContent::Paragraph(_) => b"p",
                CellContent::Table(_) => b"tbl",
                CellContent::ContentControl(_) => b"sdt",
            };
            let mut next = path.to_vec();
            next.push((name, slot));
            match child {
                CellContent::Paragraph(p) => out.push((next, p)),
                CellContent::ContentControl(c) => control(c, b"tc", &next, out),
                CellContent::Table(_) => {}
            }
        }
    }
    let mut out = Vec::new();
    let mut slot = 0;
    for child in &document.document.body.content {
        let name: &'static [u8] = match child {
            BodyContent::Paragraph(_) => b"p",
            BodyContent::Table(_) => b"tbl",
            BodyContent::ContentControl(_) => b"sdt",
            BodyContent::RawXml(_) => continue,
        };
        let path = vec![(name, slot)];
        slot += 1;
        match child {
            BodyContent::Paragraph(p) => out.push((path, p.as_ref())),
            BodyContent::ContentControl(c)
                if scope == ParagraphVisibilityScope::FacadeRecursive =>
            {
                control(c, b"body", &path, &mut out)
            }
            _ => {}
        }
    }
    out
}

fn paragraph_has_typed_visible_content(paragraph: &CT_P) -> bool {
    let mut pending = paragraph.accepted_bookmark_runs();
    while let Some(run) = pending.pop() {
        if !run.alt_drawings.is_empty() {
            return true;
        }
        for content in &run.content {
            match content {
                RunContent::Text(text) if !text.text.trim().is_empty() => return true,
                RunContent::Tab
                | RunContent::Break(..)
                | RunContent::Symbol { .. }
                | RunContent::Drawing(_)
                | RunContent::FootnoteRef { .. }
                | RunContent::EndnoteRef { .. } => return true,
                RunContent::SpecialCharacter(character)
                    if !matches!(character, SpecialCharacter::SoftHyphen) =>
                {
                    return true;
                }
                RunContent::Field(field) => {
                    match field.cached_result_owner_projection() {
                        Ok(Some(owner)) => {
                            if paragraph_has_typed_visible_content(&owner) {
                                return true;
                            }
                            continue;
                        }
                        // Visibility is conservative on an invalid pending edit.
                        // Checked publication separately propagates its error.
                        Err(_) => return true,
                        Ok(None) => {}
                    }
                    if let Some(runs) = field.cached_result_runs() {
                        // A consistent owned result may contain only deleted text.
                        // Its public string cache remains a text extraction projection.
                        pending.extend(runs);
                    } else if !field.cached_result.trim().is_empty() {
                        return true;
                    }
                }
                _ => {}
            }
        }
    }
    false
}

/// Match the facade's body paragraph inventory, including nested block controls,
/// and mark visible nontext children without entering direct body tables or opaque wrappers.
pub(super) fn paragraph_visibility_from_main_source(
    document: &Document,
    xml: &[u8],
    scope: ParagraphVisibilityScope,
) -> Result<Vec<bool>> {
    let owners = paragraph_visibility_owners(document, scope);
    let mut owner_paths = Vec::new();
    let mut slot_stack: Vec<(ParagraphVisibilityPath, usize)> = Vec::new();
    let mut reader = NsReader::from_reader(xml);
    // Qualified Word name, admitted path, paragraph boundary, typed content slot.
    let mut open: Vec<(Vec<u8>, bool, bool, bool)> = Vec::new();
    let mut in_paragraph = false;
    let mut visible = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let event_start = usize::try_from(reader.buffer_position())
            .map_err(|error| Error::Other(error.to_string()))?;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(oxoffice_docx_oxml::OxmlError::from)?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let local = element.local_name();
                let word = matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes());
                let math = matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == M_NS.as_bytes());
                let compatibility = matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == MC_NS.as_bytes());
                let parent = open.last().map(|(name, _, _, _)| name.as_slice());
                let admitted_parent = open.last().is_some_and(|(_, admitted, _, _)| *admitted);
                // A control inherits its physical container's admitted child kinds.
                let container = if parent == Some(b"sdtContent") {
                    open.iter().rev().find_map(|(name, admitted, _, _)| {
                        (*admitted && matches!(name.as_slice(), b"body" | b"tbl" | b"tr" | b"tc"))
                            .then_some(name.as_slice())
                    })
                } else {
                    parent
                };
                let typed_control = if word
                    && local.as_ref() == b"sdt"
                    && admitted_parent
                    && matches!(event, Event::Start(_))
                {
                    let prefixes = reader.resolver().bindings().try_fold(
                        Vec::new(),
                        |mut scope, (declaration, namespace)| {
                            let prefix = match declaration {
                                PrefixDeclaration::Default => "",
                                PrefixDeclaration::Named(prefix) => std::str::from_utf8(prefix)
                                    .map_err(oxoffice_docx_oxml::OxmlError::from)?,
                            };
                            // Resolver bindings retain raw namespace attribute bytes.
                            // The SDT scope carrier and serializer use semantic URIs.
                            let namespace = quick_xml::events::attributes::Attribute {
                                key: quick_xml::name::QName(b"xmlns"),
                                value: Cow::Borrowed(namespace.as_ref()),
                            }
                            .decoded_and_normalized_value(
                                XmlVersion::Implicit1_0,
                                element.decoder(),
                            )
                            .map_err(oxoffice_docx_oxml::OxmlError::from)?;
                            scope.push(format!("\0{prefix}\0{namespace}"));
                            if namespace == W_NS {
                                scope.push(prefix.to_owned());
                            }
                            Ok::<_, Error>(scope)
                        },
                    )?;
                    let owner = if in_paragraph {
                        StorySdtOwner::Inline
                    } else {
                        match container {
                            Some(b"tbl") => StorySdtOwner::Table,
                            Some(b"tr") => StorySdtOwner::Row,
                            _ => StorySdtOwner::Block,
                        }
                    };
                    let mut probe = reader.clone();
                    probe
                        .read_to_end(element.name())
                        .map_err(oxoffice_docx_oxml::OxmlError::from)?;
                    let end = usize::try_from(probe.buffer_position())
                        .map_err(|error| Error::Other(error.to_string()))?;
                    let source = xml.get(event_start..end).ok_or_else(|| {
                        Error::Other("content-control source span is invalid".to_owned())
                    })?;
                    CT_Sdt::story_raw_is_typed(source, &prefixes, owner)
                } else {
                    false
                };
                let first_content = word
                    && local.as_ref() == b"sdtContent"
                    && admitted_parent
                    && parent == Some(b"sdt")
                    && matches!(event, Event::Start(_))
                    && open.last().is_some_and(|(_, _, _, seen)| !seen);
                let mut paragraph = false;
                let admitted = if in_paragraph {
                    if admitted_parent
                        && ((word
                            && parent == Some(b"r")
                            && matches!(
                                local.as_ref(),
                                b"drawing"
                                    | b"pict"
                                    | b"object"
                                    | b"sym"
                                    | b"tab"
                                    | b"br"
                                    | b"cr"
                                    | b"footnoteReference"
                                    | b"endnoteReference"
                            ))
                            || (math && matches!(local.as_ref(), b"oMath" | b"oMathPara")))
                        && let Some(flag) = visible.last_mut()
                    {
                        *flag = true;
                    }
                    admitted_parent
                        && ((word
                            && matches!(
                                local.as_ref(),
                                b"t" | b"r"
                                    | b"hyperlink"
                                    | b"ins"
                                    | b"moveTo"
                                    | b"smartTag"
                                    | b"customXml"
                                    | b"sdt"
                                    | b"sdtContent"
                                    | b"dir"
                                    | b"bdo"
                            )
                            && match local.as_ref() {
                                b"sdt" => typed_control,
                                b"sdtContent" => first_content,
                                _ => true,
                            })
                            || (compatibility
                                && matches!(
                                    local.as_ref(),
                                    b"AlternateContent" | b"Choice" | b"Fallback"
                                )))
                } else if word {
                    match local.as_ref() {
                        b"document" => open.is_empty(),
                        b"body" => admitted_parent && parent == Some(b"document"),
                        b"sdt" => {
                            scope == ParagraphVisibilityScope::FacadeRecursive
                                && typed_control
                                && matches!(container, Some(b"body" | b"tbl" | b"tr" | b"tc"))
                        }
                        b"sdtContent" => first_content,
                        b"tbl" => {
                            admitted_parent
                                && parent == Some(b"sdtContent")
                                && matches!(container, Some(b"body" | b"tc"))
                        }
                        b"tr" => admitted_parent && container == Some(b"tbl"),
                        b"tc" => admitted_parent && container == Some(b"tr"),
                        b"p" if admitted_parent && matches!(container, Some(b"body" | b"tc")) => {
                            visible.push(false);
                            paragraph = true;
                            if matches!(event, Event::Start(_)) {
                                in_paragraph = true;
                            }
                            true
                        }
                        _ => false,
                    }
                } else {
                    false
                };
                let structural: Option<&'static [u8]> = if !in_paragraph || paragraph {
                    match local.as_ref() {
                        b"p" if paragraph => Some(b"p"),
                        b"sdt" if word && typed_control && admitted_parent => Some(b"sdt"),
                        b"tbl"
                            if word
                                && admitted_parent
                                && matches!(container, Some(b"body" | b"tc")) =>
                        {
                            Some(b"tbl")
                        }
                        b"tr" if word && admitted_parent && container == Some(b"tbl") => {
                            Some(b"tr")
                        }
                        b"tc" if word && admitted_parent && container == Some(b"tr") => Some(b"tc"),
                        _ => None,
                    }
                } else {
                    None
                };
                let mut path = slot_stack
                    .last()
                    .map(|(path, _)| path.clone())
                    .unwrap_or_default();
                if let Some(name) = structural {
                    if let Some((_, slot)) = slot_stack.last_mut() {
                        path.push((name, *slot));
                        *slot += 1;
                    }
                    if paragraph {
                        owner_paths.push(path.clone());
                    }
                }
                if matches!(event, Event::Start(_)) {
                    // sdtContent shares its control's child scope. Other structural
                    // containers own a fresh set of direct structural slots.
                    let slot = if first_content {
                        slot_stack.last().map_or(0, |(_, slot)| *slot)
                    } else {
                        0
                    };
                    slot_stack.push((path, slot));
                }
                if first_content && let Some((_, _, _, seen)) = open.last_mut() {
                    *seen = true;
                }
                if matches!(event, Event::Start(_)) {
                    open.push((
                        if word {
                            local.as_ref().to_vec()
                        } else {
                            Vec::new()
                        },
                        admitted,
                        paragraph,
                        false,
                    ));
                }
            }
            Event::End(_) => {
                slot_stack.pop();
                if open.pop().is_some_and(|(_, _, paragraph, _)| paragraph) {
                    in_paragraph = false;
                }
            }
            Event::Text(text)
                if open
                    .last()
                    .is_some_and(|(name, admitted, _, _)| *admitted && name == b"t") =>
            {
                let text = text
                    .decode()
                    .map_err(|error| Error::Other(error.to_string()))?;
                let text = quick_xml::escape::unescape(&text)
                    .map_err(|error| Error::Other(error.to_string()))?;
                if !text.trim().is_empty()
                    && let Some(flag) = visible.last_mut()
                {
                    *flag = true;
                }
            }
            Event::CData(text)
                if open
                    .last()
                    .is_some_and(|(name, admitted, _, _)| *admitted && name == b"t") =>
            {
                let text = text
                    .decode()
                    .map_err(|error| Error::Other(error.to_string()))?;
                if !text.trim().is_empty()
                    && let Some(flag) = visible.last_mut()
                {
                    *flag = true;
                }
            }
            Event::GeneralRef(reference)
                if open
                    .last()
                    .is_some_and(|(name, admitted, _, _)| *admitted && name == b"t") =>
            {
                let name = reference
                    .decode()
                    .map_err(|error| Error::Other(error.to_string()))?;
                let encoded = format!("&{name};");
                let text = quick_xml::escape::unescape(&encoded)
                    .map_err(|error| Error::Other(error.to_string()))?;
                if !text.trim().is_empty()
                    && let Some(flag) = visible.last_mut()
                {
                    *flag = true;
                }
            }
            Event::Eof if !open.is_empty() => {
                return Err(Error::Other(
                    "main document ends inside an element".to_owned(),
                ));
            }
            Event::Eof => {
                if owner_paths.len() != owners.len()
                    || owner_paths
                        .iter()
                        .zip(&owners)
                        .any(|(source, (model, _))| source != model)
                {
                    return Err(Error::Other(
                        "main document paragraph owners do not match validation scope".to_owned(),
                    ));
                }
                return Ok(owners
                    .iter()
                    .zip(visible)
                    .map(|((_, p), nontext)| paragraph_has_typed_visible_content(p) || nontext)
                    .collect());
            }
            _ => {}
        }
        buffer.clear();
    }
}
