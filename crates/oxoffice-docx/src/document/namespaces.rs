//! Namespace declarations of Word parts: scopes, root declarations, nested owners and markers.

use super::*;

pub(super) fn document_namespace_scopes(xml: &[u8]) -> Result<DocumentNamespaceScopes> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut scopes: Vec<Vec<(String, String)>> = Vec::new();
    let mut root_declarations = Vec::new();
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(oxoffice_docx_oxml::OxmlError::from)?;
        match event {
            Event::Start(ref element) => {
                let mut scope = scopes.last().cloned().unwrap_or_default();
                let declarations = namespace_declarations(element)?;
                apply_namespace_declarations(element, &mut scope)?;
                if scopes.is_empty() {
                    root_declarations = declarations.clone();
                }
                if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == WORD_NAMESPACE.as_bytes())
                    && matches_local_name(element.name().as_ref(), b"body")
                {
                    return Ok(DocumentNamespaceScopes {
                        root_declarations,
                        body_declarations: declarations,
                        body_bindings: scope,
                    });
                }
                scopes.push(scope);
            }
            Event::Empty(ref element) => {
                let mut scope = scopes.last().cloned().unwrap_or_default();
                let declarations = namespace_declarations(element)?;
                apply_namespace_declarations(element, &mut scope)?;
                if scopes.is_empty() {
                    root_declarations = declarations.clone();
                }
                if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == WORD_NAMESPACE.as_bytes())
                    && matches_local_name(element.name().as_ref(), b"body")
                {
                    return Ok(DocumentNamespaceScopes {
                        root_declarations,
                        body_declarations: declarations,
                        body_bindings: scope,
                    });
                }
            }
            Event::End(_) => {
                scopes.pop();
            }
            Event::Eof => {
                return Ok(DocumentNamespaceScopes {
                    root_declarations,
                    ..DocumentNamespaceScopes::default()
                });
            }
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn namespace_declarations(element: &BytesStart<'_>) -> Result<Vec<(String, String)>> {
    let mut declarations = Vec::new();
    apply_namespace_declarations(element, &mut declarations)?;
    Ok(declarations)
}

/// The declarations on `element` that change a binding `inherited` already
/// holds.
///
/// A declaration that rebinds a prefix to the URI already in scope resolves no
/// name differently from the scope it sits in, so the element owns no
/// namespace and canonical serialization cannot lose one by dropping it. The
/// retained root-attribute record materializes exactly such a redundant
/// binding onto every paragraph and run that keeps a prefixed producer
/// attribute, and treating those as owners makes identical runs
/// indistinguishable to the retained-owner matcher.
fn rebinding_namespace_declarations(
    element: &BytesStart<'_>,
    inherited: &[(String, String)],
) -> Result<Vec<(String, String)>> {
    let mut declarations = namespace_declarations(element)?;
    declarations.retain(|(name, value)| {
        !inherited
            .iter()
            .any(|(candidate, bound)| candidate == name && bound == value)
    });
    Ok(declarations)
}

/// Retain producer root metadata when a style edit requires typed serialization.
/// The serializer owns the canonical `w` and `r` prefixes. Every other root
/// attribute, including compatibility declarations, stays in source order.
pub(super) fn retain_styles_root_attributes(source: &[u8], serialized: Vec<u8>) -> Result<Vec<u8>> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(source);
    let mut buffer = Vec::new();
    let root = loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid styles root: {error}")))?
        {
            Event::Start(element) | Event::Empty(element) => break element.into_owned(),
            Event::Eof => return Err(Error::Other("styles part has no root".to_owned())),
            _ => buffer.clear(),
        }
    };
    let mut retained = Vec::new();
    for attribute in root.attributes() {
        let attribute = attribute.map_err(oxoffice_docx_oxml::OxmlError::from)?;
        let name = std::str::from_utf8(attribute.key.as_ref())
            .map_err(|error| Error::Other(format!("invalid styles attribute name: {error}")))?
            .to_owned();
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, root.decoder())
            .map_err(|error| Error::Other(format!("invalid styles attribute: {error}")))?
            .into_owned();
        if name == "xmlns:w" || name == "xmlns:r" {
            let expected = if name == "xmlns:w" {
                oxoffice_docx_oxml::namespace::W_NS
            } else {
                drawing_ns::R
            };
            if value != expected {
                return Err(Error::Other(format!(
                    "cannot serialize styles with a shadowed `{}` namespace",
                    &name[6..]
                )));
            }
        } else {
            retained.push((name, value));
        }
    }
    let mut start = BytesStart::new("w:styles");
    start.push_attribute(("xmlns:w", oxoffice_docx_oxml::namespace::W_NS));
    start.push_attribute(("xmlns:r", drawing_ns::R));
    for (name, value) in &retained {
        start.push_attribute((name.as_str(), value.as_str()));
    }
    let mut writer = Writer::new(Vec::new());
    writer
        .write_event(Event::Start(start))
        .map_err(|error| Error::Other(format!("invalid styles root: {error}")))?;
    let new_root = writer.into_inner();
    let opening = b"<w:styles";
    let position = serialized
        .windows(opening.len())
        .position(|window| window == opening)
        .ok_or_else(|| Error::Other("serialized styles root is missing".to_owned()))?;
    let end = serialized[position..]
        .iter()
        .position(|byte| *byte == b'>')
        .ok_or_else(|| Error::Other("serialized styles root is incomplete".to_owned()))?
        + position
        + 1;
    let mut output = Vec::with_capacity(serialized.len() + new_root.len());
    output.extend_from_slice(&serialized[..position]);
    output.extend_from_slice(&new_root);
    output.extend_from_slice(&serialized[end..]);
    Ok(output)
}

pub(super) fn root_namespace_declarations(xml: &[u8]) -> Result<Vec<(String, String)>> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid XML root: {error}")))?
        {
            Event::Start(element) | Event::Empty(element) => {
                return namespace_declarations(&element);
            }
            Event::Eof => {
                return Err(Error::Other(
                    "XML part has no root namespace owner".to_owned(),
                ));
            }
            _ => buffer.clear(),
        }
    }
}

fn apply_namespace_declarations(
    element: &BytesStart<'_>,
    scope: &mut Vec<(String, String)>,
) -> Result<()> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(oxoffice_docx_oxml::OxmlError::from)?;
        let name = attribute.key.as_ref();
        if name != b"xmlns" && !name.starts_with(b"xmlns:") {
            continue;
        }
        let name = std::str::from_utf8(name)
            .map_err(oxoffice_docx_oxml::OxmlError::from)?
            .to_owned();
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
            .map_err(oxoffice_docx_oxml::OxmlError::from)?
            .into_owned();
        scope.retain(|(candidate, _)| candidate != &name);
        scope.push((name, value));
    }
    Ok(())
}

/// The prefix whose binding the canonical serializer cannot keep when it
/// writes this main part again: any declaration on the body, or a root
/// declaration of one of the serializer's own prefixes to another
/// namespace. A default namespace is kept as declared (the root writer
/// writes it back), whatever it binds.
pub(super) fn unsafe_serializer_namespace_prefix(
    root_declarations: &[(String, String)],
    body_declarations: &[(String, String)],
) -> Option<String> {
    if let Some((name, _)) = body_declarations.first() {
        return Some(namespace_prefix(name));
    }

    root_declarations.iter().find_map(|(name, value)| {
        let prefix = namespace_prefix(name);
        let expected = canonical_serializer_namespace(&prefix)?;
        (value != expected).then_some(prefix)
    })
}

fn canonical_serializer_namespace(prefix: &str) -> Option<&'static str> {
    match prefix {
        "w" => Some(WORD_NAMESPACE),
        "r" => Some("http://schemas.openxmlformats.org/officeDocument/2006/relationships"),
        "mc" => Some("http://schemas.openxmlformats.org/markup-compatibility/2006"),
        "wp" => Some("http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"),
        "a" => Some("http://schemas.openxmlformats.org/drawingml/2006/main"),
        "pic" => Some("http://schemas.openxmlformats.org/drawingml/2006/picture"),
        "c" => Some("http://schemas.openxmlformats.org/drawingml/2006/chart"),
        "w14" => Some("http://schemas.microsoft.com/office/word/2010/wordml"),
        "w15" => Some("http://schemas.microsoft.com/office/word/2012/wordml"),
        _ => None,
    }
}

pub(super) fn namespace_prefix(declaration_name: &str) -> String {
    declaration_name
        .strip_prefix("xmlns:")
        .unwrap_or("")
        .to_owned()
}

fn modeled_owner_name(local_name: &[u8]) -> Option<&'static str> {
    Some(match local_name {
        b"p" => "p",
        b"tbl" => "tbl",
        b"tc" => "tc",
        b"sdt" => "sdt",
        b"hyperlink" => "hyperlink",
        b"r" => "r",
        _ => return None,
    })
}

fn is_modeled_word_child(parent: Option<&[u8]>, local_name: &[u8]) -> bool {
    match parent {
        Some(b"p") => matches!(
            local_name,
            b"pPr"
                | b"r"
                | b"fldSimple"
                | b"hyperlink"
                | b"sdt"
                | b"bookmarkStart"
                | b"bookmarkEnd"
                | b"commentRangeStart"
                | b"commentRangeEnd"
                | b"ins"
                | b"del"
                | b"moveFrom"
                | b"moveTo"
        ),
        Some(b"r") => matches!(
            local_name,
            b"rPr"
                | b"t"
                | b"delText"
                | b"tab"
                | b"br"
                | b"drawing"
                | b"fldChar"
                | b"instrText"
                | b"commentReference"
                | b"footnoteReference"
                | b"endnoteReference"
        ),
        Some(b"hyperlink") => matches!(
            local_name,
            b"r" | b"fldSimple" | b"ins" | b"del" | b"moveFrom" | b"moveTo"
        ),
        Some(b"sdt") => matches!(local_name, b"sdtPr" | b"sdtEndPr" | b"sdtContent"),
        Some(b"sdtContent") => {
            matches!(local_name, b"p" | b"tbl" | b"tr" | b"tc" | b"r" | b"sdt")
        }
        Some(b"tbl") => matches!(local_name, b"tblPr" | b"tblGrid" | b"tr" | b"sdt"),
        Some(b"tblGrid") => local_name == b"gridCol",
        Some(b"tr") => matches!(local_name, b"trPr" | b"tc" | b"sdt"),
        Some(b"tc") => matches!(local_name, b"tcPr" | b"p" | b"tbl" | b"sdt"),
        _ => false,
    }
}

/// Keep the exact ordered binding vector when this element declares nothing.
/// Declaring elements retain the original decoder and retain-then-append policy.
fn shared_namespace_bindings(
    element: &BytesStart<'_>,
    inherited: &Arc<Vec<(String, String)>>,
) -> Result<Arc<Vec<(String, String)>>> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(oxoffice_docx_oxml::OxmlError::from)?;
        let name = attribute.key.as_ref();
        if name == b"xmlns" || name.starts_with(b"xmlns:") {
            let mut bindings = inherited.as_ref().clone();
            apply_namespace_declarations(element, &mut bindings)?;
            return Ok(Arc::new(bindings));
        }
    }
    Ok(Arc::clone(inherited))
}

pub(super) fn modeled_owner_spans(xml: &[u8]) -> Result<Vec<ModeledOwnerSpan>> {
    modeled_owner_piece_spans(xml, None)?
        .ok_or_else(|| Error::Other("modeled namespace owner piece was not eligible".to_owned()))
}

/// The modeled owner spans of `xml`.
///
/// `child` is a body-child slice of a head-child-tail wrapper. `None` means
/// the slice is ineligible: a word body starts or ends inside it, or the
/// namespace scope and `inside_body` flag do not return to the values they
/// had when the slice began. That is the old parser state. A story-owner
/// scan can still see one editable body around an opaque inner body, so this
/// check is stricter than that scan. `Err` is a parser failure. The caller
/// keeps the whole scanner and does not publish an empty span list.
pub(super) fn modeled_owner_piece_spans(
    xml: &[u8],
    child: Option<std::ops::Range<usize>>,
) -> Result<Option<Vec<ModeledOwnerSpan>>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut inside_body = false;
    let mut depth = 0usize;
    let mut owner_stack = Vec::new();
    let empty_scope = Arc::new(Vec::new());
    let mut scope_stack: Vec<Arc<Vec<(String, String)>>> = Vec::new();
    let mut spans: Vec<ModeledOwnerSpan> = Vec::new();
    let mut saw_entry = child.is_none();
    let mut saw_exit = child.is_none();
    let mut entry_depth = 0usize;
    let mut entry_scopes = 0usize;
    let mut entry_inside = false;
    let mut ineligible = false;
    loop {
        let start = reader.buffer_position() as usize;
        if let Some(child) = &child {
            if !saw_entry && start >= child.start {
                saw_entry = true;
                entry_depth = depth;
                entry_scopes = scope_stack.len();
                entry_inside = inside_body;
                if !inside_body {
                    ineligible = true;
                }
            }
            if saw_entry && !saw_exit && start >= child.end {
                saw_exit = true;
                if depth != entry_depth
                    || scope_stack.len() != entry_scopes
                    || inside_body != entry_inside
                {
                    ineligible = true;
                }
            }
        }
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid nested namespace scope: {error}")))?;
        let is_word = matches!(
            namespace,
            ResolveResult::Bound(Namespace(uri)) if uri == WORD_NAMESPACE.as_bytes()
        );
        let end = reader.buffer_position() as usize;
        let in_child = child
            .as_ref()
            .is_some_and(|child| start >= child.start && start < child.end);
        match event {
            Event::Start(element) => {
                let local_name = element.local_name();
                if in_child && is_word && local_name.as_ref() == b"body" {
                    ineligible = true;
                }
                let inherited = scope_stack.last().unwrap_or(&empty_scope);
                let bindings = shared_namespace_bindings(&element, inherited)?;
                if inside_body
                    && is_word
                    && let Some(local_name) = modeled_owner_name(local_name.as_ref())
                {
                    let index = spans.len();
                    spans.push(ModeledOwnerSpan {
                        local_name,
                        start,
                        end: 0,
                        declarations: Arc::new(rebinding_namespace_declarations(
                            &element, inherited,
                        )?),
                        bindings: Arc::clone(&bindings),
                    });
                    owner_stack.push((depth, index));
                }
                if is_word && local_name.as_ref() == b"body" {
                    inside_body = true;
                }
                scope_stack.push(bindings);
                depth += 1;
            }
            Event::Empty(element) => {
                let local_name = element.local_name();
                if in_child && is_word && local_name.as_ref() == b"body" {
                    ineligible = true;
                }
                let inherited = scope_stack.last().unwrap_or(&empty_scope);
                let bindings = shared_namespace_bindings(&element, inherited)?;
                if inside_body
                    && is_word
                    && let Some(local_name) = modeled_owner_name(local_name.as_ref())
                {
                    spans.push(ModeledOwnerSpan {
                        local_name,
                        start,
                        end,
                        declarations: Arc::new(rebinding_namespace_declarations(
                            &element, inherited,
                        )?),
                        bindings,
                    });
                }
            }
            Event::End(element) => {
                if in_child && is_word && matches_local_name(element.name().as_ref(), b"body") {
                    ineligible = true;
                }
                depth = depth.saturating_sub(1);
                scope_stack.pop();
                if owner_stack
                    .last()
                    .is_some_and(|(owner_depth, _)| *owner_depth == depth)
                {
                    let (_, index) = owner_stack.pop().expect("checked owner stack");
                    spans[index].end = end;
                }
                if inside_body && is_word && matches_local_name(element.name().as_ref(), b"body") {
                    inside_body = false;
                }
            }
            Event::Eof => {
                if spans.iter().any(|span| span.end == 0) {
                    return Err(Error::Other(
                        "unterminated modeled namespace owner".to_owned(),
                    ));
                }
                if child.is_some() && (!saw_entry || !saw_exit || ineligible) {
                    return Ok(None);
                }
                return Ok(Some(spans));
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn element_declared_prefixes(
    element: &BytesStart<'_>,
    declarations: &[(String, String)],
    bindings: &[(String, String)],
    parent_local_name: Option<&[u8]>,
    shadowed_owner_prefixes: &HashSet<String>,
    allow_redundant_local_bindings: bool,
) -> Vec<String> {
    let prefixes = declarations
        .iter()
        .map(|(name, value)| (namespace_prefix(name), value.as_str()))
        .collect::<Vec<_>>();
    let local_declarations = local_namespace_prefixes(element);
    let mut used = Vec::new();
    let element_is_modeled_word =
        is_modeled_word_child(parent_local_name, element.local_name().as_ref());
    let qualified_name = element.name();
    let element_prefix = std::str::from_utf8(qualified_name.as_ref())
        .ok()
        .map(|name| name.split_once(':').map_or("", |(prefix, _)| prefix));
    if element_prefix.is_some_and(|prefix| {
        prefixes.iter().any(|(candidate, value)| {
            candidate == prefix
                && !shadowed_owner_prefixes.contains(prefix)
                && inherited_namespace(bindings, prefix) == Some(*value)
                && !(*value == WORD_NAMESPACE && element_is_modeled_word)
        }) && (!local_declarations.contains(element_prefix.unwrap_or_default())
            || allow_redundant_local_bindings)
    }) {
        used.push(element_prefix.unwrap_or_default().to_owned());
    }
    for attribute in element.attributes().flatten() {
        let Some(name) = std::str::from_utf8(attribute.key.as_ref()).ok() else {
            continue;
        };
        let Some((prefix, _)) = name.split_once(':') else {
            continue;
        };
        if prefix != "xmlns"
            && prefixes.iter().any(|(candidate, value)| {
                candidate == prefix
                    && !shadowed_owner_prefixes.contains(prefix)
                    && inherited_namespace(bindings, prefix) == Some(*value)
                    && !(*value == WORD_NAMESPACE && element_is_modeled_word)
            })
            && (!local_declarations.contains(prefix) || allow_redundant_local_bindings)
            && !used.iter().any(|candidate| candidate == prefix)
        {
            used.push(prefix.to_owned());
        }
    }
    used
}

fn local_namespace_prefixes(element: &BytesStart<'_>) -> HashSet<String> {
    element
        .attributes()
        .flatten()
        .filter_map(|attribute| {
            let name = std::str::from_utf8(attribute.key.as_ref()).ok()?;
            (name == "xmlns" || name.starts_with("xmlns:")).then(|| namespace_prefix(name))
        })
        .collect()
}

fn resolved_snapshot_name(
    qualified_name: &[u8],
    bindings: &[(String, String)],
    default_namespace_applies: bool,
) -> Result<String> {
    let qualified_name = std::str::from_utf8(qualified_name)
        .map_err(|error| Error::Other(format!("invalid logical owner name: {error}")))?;
    let (prefix, local_name) = qualified_name
        .split_once(':')
        .map_or(("", qualified_name), |(prefix, local)| (prefix, local));
    let namespace = if prefix == "xml" {
        Some(XML_NAMESPACE)
    } else if prefix.is_empty() && !default_namespace_applies {
        None
    } else {
        inherited_namespace(bindings, prefix)
    };
    let namespace = match namespace {
        Some(WORD_NAMESPACE) => WORD_NAMESPACE.to_owned(),
        Some(XML_NAMESPACE) => XML_NAMESPACE.to_owned(),
        _ if !prefix.is_empty() => format!("#prefix:{prefix}"),
        Some(namespace) => namespace.to_owned(),
        None => String::new(),
    };
    Ok(format!("{{{namespace}}}{local_name}"))
}

pub(super) fn logical_owner_snapshot(
    owner_xml: &[u8],
    inherited_bindings: &[(String, String)],
    prospective_declarations: &[(String, String)],
) -> Result<LogicalOwnerSnapshot> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(owner_xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut owner_bindings = inherited_bindings.to_vec();
    owner_bindings.extend(prospective_declarations.iter().cloned());
    let mut scope_stack = vec![owner_bindings];
    let mut structure = Vec::new();
    let mut semantic = Vec::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid logical owner XML: {error}")))?;
        match event {
            event @ (Event::Start(_) | Event::Empty(_)) => {
                let is_start = matches!(&event, Event::Start(_));
                let element = match event {
                    Event::Start(element) | Event::Empty(element) => element,
                    _ => unreachable!(),
                };
                let mut bindings = scope_stack.last().cloned().unwrap_or_default();
                apply_namespace_declarations(&element, &mut bindings)?;
                let name = resolved_snapshot_name(element.name().as_ref(), &bindings, true)?;
                let mut attribute_names = Vec::new();
                let mut attribute_values = Vec::new();
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| {
                        Error::Other(format!("invalid logical owner attribute: {error}"))
                    })?;
                    if attribute.key.as_ref() == b"xmlns"
                        || attribute.key.as_ref().starts_with(b"xmlns:")
                    {
                        continue;
                    }
                    let attribute_name =
                        resolved_snapshot_name(attribute.key.as_ref(), &bindings, false)?;
                    let value = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                        .map_err(|error| {
                            Error::Other(format!("invalid logical owner attribute: {error}"))
                        })?
                        .into_owned();
                    attribute_names.push(attribute_name.clone());
                    attribute_values.push(format!("{attribute_name}={value}"));
                }
                attribute_names.sort();
                attribute_values.sort();
                structure.push(format!("start:{name}:[{}]", attribute_names.join(",")));
                semantic.push(format!("start:{name}:[{}]", attribute_values.join(",")));
                if is_start {
                    scope_stack.push(bindings);
                } else {
                    structure.push(format!("end:{name}"));
                    semantic.push(format!("end:{name}"));
                }
            }
            Event::End(element) => {
                let bindings = scope_stack.last().cloned().unwrap_or_default();
                let name = resolved_snapshot_name(element.name().as_ref(), &bindings, true)?;
                structure.push(format!("end:{name}"));
                semantic.push(format!("end:{name}"));
                scope_stack.pop();
            }
            Event::Text(text) => {
                if xml_bytes_are_whitespace(text.as_ref()) {
                    buffer.clear();
                    continue;
                }
                structure.push("text".to_owned());
                semantic.push(format!("text:{}", String::from_utf8_lossy(text.as_ref())));
            }
            Event::CData(text) => {
                structure.push("cdata".to_owned());
                semantic.push(format!("cdata:{}", String::from_utf8_lossy(text.as_ref())));
            }
            Event::GeneralRef(reference) => {
                structure.push("reference".to_owned());
                semantic.push(format!(
                    "reference:{}",
                    String::from_utf8_lossy(reference.as_ref())
                ));
            }
            Event::Comment(comment) => {
                structure.push("comment".to_owned());
                semantic.push(format!(
                    "comment:{}",
                    String::from_utf8_lossy(comment.as_ref())
                ));
            }
            Event::PI(instruction) => {
                structure.push("processing-instruction".to_owned());
                semantic.push(format!(
                    "processing-instruction:{}",
                    String::from_utf8_lossy(instruction.as_ref())
                ));
            }
            Event::Eof => {
                return Ok(LogicalOwnerSnapshot {
                    structure,
                    semantic,
                });
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn namespace_marker_without_owner_declarations(
    raw: &[u8],
    declarations: &[(String, String)],
    inherited_bindings: &[(String, String)],
    direct_drawing_child: bool,
) -> Result<NamespaceMarker> {
    // The drawing reader closes namespaces lost with its drawing wrapper on
    // the retained inline/anchor root. Admit only that concrete relocation.
    // Every other declaration and every nonnamespace byte stays significant.
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut depth = 0usize;
    let mut root_bindings = inherited_bindings.to_vec();
    let mut child = None;
    let mut relocatable = Vec::new();
    let mut closure_facts = Vec::new();
    let mut root_seen = false;
    let mut qualified = true;
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event()
            .map_err(|error| Error::Other(format!("invalid namespace marker XML: {error}")))?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                if depth == 0 {
                    if direct_drawing_child
                        && matches!(element.local_name().as_ref(), b"inline" | b"anchor")
                    {
                        apply_namespace_declarations(&element, &mut root_bindings)?;
                        let name = element.name();
                        let name = std::str::from_utf8(name.as_ref()).map_err(|error| {
                            Error::Other(format!("invalid namespace marker QName: {error}"))
                        })?;
                        let prefix = name.split_once(':').map_or("", |(prefix, _)| prefix);
                        if inherited_namespace(&root_bindings, prefix)
                            == Some(
                                "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
                            )
                        {
                            let mut removable = declarations.to_vec();
                            for (prefix, uri) in [
                                ("a", "http://schemas.openxmlformats.org/drawingml/2006/main"),
                                (
                                    "pic",
                                    "http://schemas.openxmlformats.org/drawingml/2006/picture",
                                ),
                            ] {
                                closure_facts.push((
                                    prefix.to_owned(),
                                    inherited_namespace(&root_bindings, prefix).map(str::to_owned),
                                ));
                                if inherited_namespace(&root_bindings, prefix) == Some(uri) {
                                    removable.push((format!("xmlns:{prefix}"), uri.to_owned()));
                                }
                            }
                            return Ok(NamespaceMarker {
                                raw: as_captured(strip_marker_root_namespace_declarations(
                                    raw, &removable,
                                )?),
                                namespace_facts: closure_facts,
                            });
                        }
                    }
                    if root_seen || element.local_name().as_ref() != b"drawing" {
                        qualified = false;
                        break;
                    }
                    root_seen = true;
                    apply_namespace_declarations(&element, &mut root_bindings)?;
                    let name = std::str::from_utf8(element.name().as_ref())
                        .map_err(|error| {
                            Error::Other(format!("invalid namespace marker QName: {error}"))
                        })?
                        .to_owned();
                    let prefix = name.split_once(':').map_or("", |(prefix, _)| prefix);
                    qualified &=
                        inherited_namespace(&root_bindings, prefix) == Some(WORD_NAMESPACE);
                    qualified &= element.attributes().all(|attribute| {
                        attribute.is_ok_and(|attribute| {
                            let key = attribute.key.as_ref();
                            key == b"xmlns" || key.starts_with(b"xmlns:")
                        })
                    });
                } else if depth == 1 {
                    if child.is_some() {
                        qualified = false;
                        break;
                    }
                    let mut bindings = root_bindings.clone();
                    apply_namespace_declarations(&element, &mut bindings)?;
                    let name = std::str::from_utf8(element.name().as_ref())
                        .map_err(|error| {
                            Error::Other(format!("invalid namespace marker QName: {error}"))
                        })?
                        .to_owned();
                    let prefix = name.split_once(':').map_or("", |(prefix, _)| prefix);
                    qualified &= matches!(element.local_name().as_ref(), b"inline" | b"anchor")
                        && inherited_namespace(&bindings, prefix)
                            == Some(
                                "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
                            );
                    for (prefix, uri) in [
                        ("a", "http://schemas.openxmlformats.org/drawingml/2006/main"),
                        (
                            "pic",
                            "http://schemas.openxmlformats.org/drawingml/2006/picture",
                        ),
                    ] {
                        closure_facts.push((
                            prefix.to_owned(),
                            inherited_namespace(&bindings, prefix).map(str::to_owned),
                        ));
                        if inherited_namespace(&bindings, prefix) == Some(uri) {
                            relocatable.push((format!("xmlns:{prefix}"), uri.to_owned()));
                        }
                    }
                    child = Some((before, empty.then_some(reader.buffer_position() as usize)));
                }
                if !empty {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 1
                    && let Some((_, end)) = &mut child
                {
                    *end = Some(reader.buffer_position() as usize);
                }
            }
            Event::Text(text) if depth <= 1 => {
                qualified &= text.iter().all(u8::is_ascii_whitespace);
            }
            Event::Comment(_) | Event::PI(_) | Event::CData(_) | Event::GeneralRef(_)
                if depth <= 1 =>
            {
                qualified = false;
            }
            Event::Decl(_) | Event::DocType(_) => {
                qualified = false;
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if qualified && let Some((start, Some(end))) = child {
        let mut root_declarations = declarations.to_vec();
        root_declarations.extend(relocatable.iter().cloned());
        let mut normalized =
            strip_marker_root_namespace_declarations(&raw[..start], &root_declarations)?;
        normalized.extend_from_slice(&strip_marker_root_namespace_declarations(
            &raw[start..end],
            &relocatable,
        )?);
        normalized.extend_from_slice(&raw[end..]);
        return Ok(NamespaceMarker {
            raw: as_captured(normalized),
            namespace_facts: closure_facts,
        });
    }
    Ok(NamespaceMarker {
        raw: as_captured(strip_marker_root_namespace_declarations(raw, declarations)?),
        namespace_facts: Vec::new(),
    })
}

/// `raw`, retained XML, as verbatim capture writes it back
/// (`oxoffice_core::raw_xml::capture_element`): each event written again, so
/// that an end tag spelled `</a:graphic\n>` in the source reads `</a:graphic>`
/// as it does in the serialized part a marker is looked for in. Start tags
/// keep their spelling. XML the reader refuses is kept as it is.
fn as_captured(raw: Vec<u8>) -> Vec<u8> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(raw.as_slice());
    reader.config_mut().trim_text(false);
    let mut writer = quick_xml::Writer::new(Vec::with_capacity(raw.len()));
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => return writer.into_inner(),
            Ok(event) => {
                if writer.write_event(event).is_err() {
                    return raw;
                }
            }
            Err(_) => return raw,
        }
    }
}

fn strip_marker_root_namespace_declarations(
    raw: &[u8],
    declarations: &[(String, String)],
) -> Result<Vec<u8>> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(raw);
    let mut buffer = Vec::new();
    let element = match reader
        .read_event_into(&mut buffer)
        .map_err(|error| Error::Other(format!("invalid namespace marker XML: {error}")))?
    {
        Event::Start(element) | Event::Empty(element) => element,
        _ => return Ok(raw.to_vec()),
    };
    let removable = namespace_declarations(&element)?
        .into_iter()
        .filter(|candidate| declarations.iter().any(|expected| expected == candidate))
        .map(|(name, _)| name.into_bytes())
        .collect::<HashSet<_>>();
    if removable.is_empty() {
        return Ok(raw.to_vec());
    }

    let mut cursor = 1usize;
    while cursor < raw.len()
        && !raw[cursor].is_ascii_whitespace()
        && !matches!(raw[cursor], b'/' | b'>')
    {
        cursor += 1;
    }
    let mut removals = Vec::new();
    loop {
        let whitespace_start = cursor;
        while cursor < raw.len() && raw[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= raw.len() || matches!(raw[cursor], b'/' | b'>') {
            break;
        }
        let name_start = cursor;
        while cursor < raw.len()
            && !raw[cursor].is_ascii_whitespace()
            && !matches!(raw[cursor], b'=' | b'/' | b'>')
        {
            cursor += 1;
        }
        let name_end = cursor;
        while cursor < raw.len() && raw[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= raw.len() || raw[cursor] != b'=' {
            return Ok(raw.to_vec());
        }
        cursor += 1;
        while cursor < raw.len() && raw[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= raw.len() || !matches!(raw[cursor], b'\'' | b'"') {
            return Ok(raw.to_vec());
        }
        let quote = raw[cursor];
        cursor += 1;
        while cursor < raw.len() && raw[cursor] != quote {
            cursor += 1;
        }
        if cursor >= raw.len() {
            return Ok(raw.to_vec());
        }
        cursor += 1;
        if removable.contains(&raw[name_start..name_end]) {
            removals.push((whitespace_start, cursor));
        }
    }
    if removals.is_empty() {
        return Ok(raw.to_vec());
    }
    let mut normalized = Vec::with_capacity(raw.len());
    let mut copied = 0usize;
    for (start, end) in removals {
        normalized.extend_from_slice(&raw[copied..start]);
        copied = end;
    }
    normalized.extend_from_slice(&raw[copied..]);
    Ok(normalized)
}

fn namespace_owner_markers(
    owner_xml: &[u8],
    declarations: &[(String, String)],
    inherited_bindings: &[(String, String)],
    allow_redundant_local_bindings: bool,
) -> Result<Vec<NamespaceMarker>> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(owner_xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut stack = Vec::new();
    let mut element_stack: Vec<(Vec<u8>, String, bool)> = Vec::new();
    let mut empty_markers = Vec::new();
    let mut expanded_markers = Vec::new();
    // Candidate owners are inspected before their retained declarations have
    // been replayed. Seed those prospective bindings at the owner boundary so
    // descendant uses can still be matched, while declarations encountered
    // inside the raw subtree continue to shadow them normally.
    let mut owner_bindings = inherited_bindings.to_vec();
    owner_bindings.extend(declarations.iter().cloned());
    let mut scope_stack = vec![owner_bindings];
    let mut shadow_stack = vec![HashSet::<String>::new()];
    let mut depth = 0usize;
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid namespace owner marker: {error}")))?;
        let end = reader.buffer_position() as usize;
        let direct_drawing_child =
            element_stack
                .last()
                .is_some_and(|(local, prefix, closure_qualified)| {
                    *closure_qualified
                        && local == b"drawing"
                        && inherited_namespace(
                            scope_stack.last().map(Vec::as_slice).unwrap_or_default(),
                            prefix,
                        ) == Some(WORD_NAMESPACE)
                });
        match event {
            Event::Start(element) => {
                let mut bindings = scope_stack.last().cloned().unwrap_or_default();
                apply_namespace_declarations(&element, &mut bindings)?;
                let mut shadowed = shadow_stack.last().cloned().unwrap_or_default();
                if depth > 0 {
                    shadowed.extend(local_namespace_prefixes(&element));
                    if allow_redundant_local_bindings {
                        for (name, value) in namespace_declarations(&element)? {
                            let prefix = namespace_prefix(&name);
                            if declarations.iter().any(|(candidate, owner_value)| {
                                namespace_prefix(candidate) == prefix && value == *owner_value
                            }) {
                                shadowed.remove(&prefix);
                            }
                        }
                    }
                }
                let used_prefixes = if depth > 0 {
                    element_declared_prefixes(
                        &element,
                        declarations,
                        &bindings,
                        element_stack.last().map(|element| element.0.as_slice()),
                        &shadowed,
                        allow_redundant_local_bindings,
                    )
                } else {
                    Vec::new()
                };
                let facts = used_prefixes
                    .into_iter()
                    .map(|prefix| {
                        let namespace = inherited_namespace(&bindings, &prefix).map(str::to_owned);
                        (prefix, namespace)
                    })
                    .collect::<Vec<_>>();
                stack.push((
                    start,
                    facts,
                    scope_stack.last().cloned().unwrap_or_default(),
                    direct_drawing_child,
                ));
                let name = element.name();
                let name = std::str::from_utf8(name.as_ref()).map_err(|error| {
                    Error::Other(format!("invalid namespace marker QName: {error}"))
                })?;
                let prefix = name.split_once(':').map_or("", |(prefix, _)| prefix);
                let closure_qualified = if element.local_name().as_ref() == b"drawing"
                    && inherited_namespace(&bindings, prefix) == Some(WORD_NAMESPACE)
                {
                    let end = story_element_end(owner_xml, start)?;
                    // Child markers may normalize only inside the same actual
                    // minimal drawing route. A QName alone cannot authorize
                    // relocation past opaque wrapper attributes or siblings.
                    !namespace_marker_without_owner_declarations(
                        &owner_xml[start..end],
                        declarations,
                        scope_stack.last().map(Vec::as_slice).unwrap_or_default(),
                        false,
                    )?
                    .namespace_facts
                    .is_empty()
                } else {
                    false
                };
                element_stack.push((
                    element.local_name().as_ref().to_vec(),
                    prefix.to_owned(),
                    closure_qualified,
                ));
                scope_stack.push(bindings);
                shadow_stack.push(shadowed);
                depth += 1;
            }
            Event::Empty(element) => {
                let mut bindings = scope_stack.last().cloned().unwrap_or_default();
                apply_namespace_declarations(&element, &mut bindings)?;
                let mut shadowed = shadow_stack.last().cloned().unwrap_or_default();
                if depth > 0 {
                    shadowed.extend(local_namespace_prefixes(&element));
                    if allow_redundant_local_bindings {
                        for (name, value) in namespace_declarations(&element)? {
                            let prefix = namespace_prefix(&name);
                            if declarations.iter().any(|(candidate, owner_value)| {
                                namespace_prefix(candidate) == prefix && value == *owner_value
                            }) {
                                shadowed.remove(&prefix);
                            }
                        }
                    }
                }
                let used_prefixes = if depth > 0 {
                    element_declared_prefixes(
                        &element,
                        declarations,
                        &bindings,
                        element_stack.last().map(|element| element.0.as_slice()),
                        &shadowed,
                        allow_redundant_local_bindings,
                    )
                } else {
                    Vec::new()
                };
                if !used_prefixes.is_empty() {
                    let mut namespace_facts = used_prefixes
                        .into_iter()
                        .map(|prefix| {
                            let namespace =
                                inherited_namespace(&bindings, &prefix).map(str::to_owned);
                            (prefix, namespace)
                        })
                        .collect::<Vec<_>>();
                    let marker = namespace_marker_without_owner_declarations(
                        &owner_xml[start..end],
                        declarations,
                        scope_stack.last().map(Vec::as_slice).unwrap_or_default(),
                        direct_drawing_child,
                    )?;
                    namespace_facts.extend(marker.namespace_facts);
                    empty_markers.push(NamespaceMarker {
                        raw: marker.raw,
                        namespace_facts,
                    });
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                scope_stack.pop();
                shadow_stack.pop();
                element_stack.pop();
                if let Some((
                    marker_start,
                    mut namespace_facts,
                    inherited_bindings,
                    direct_drawing_child,
                )) = stack.pop()
                    && !namespace_facts.is_empty()
                {
                    let marker = namespace_marker_without_owner_declarations(
                        &owner_xml[marker_start..end],
                        declarations,
                        &inherited_bindings,
                        direct_drawing_child,
                    )?;
                    namespace_facts.extend(marker.namespace_facts);
                    expanded_markers.push(NamespaceMarker {
                        raw: marker.raw,
                        namespace_facts,
                    });
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    empty_markers.extend(expanded_markers);
    Ok(empty_markers)
}

pub(super) fn candidate_namespace_owner_markers(
    owner_xml: &[u8],
    declarations: &[(String, String)],
    inherited_bindings: &[(String, String)],
    expected: &[NamespaceMarker],
) -> Result<Vec<NamespaceMarker>> {
    let candidates = namespace_owner_markers(owner_xml, declarations, inherited_bindings, true)?;
    let mut matched = vec![false; expected.len()];
    Ok(candidates
        .into_iter()
        .filter(|candidate| {
            let expected_index = expected
                .iter()
                .enumerate()
                .position(|(index, marker)| !matched[index] && marker.raw == candidate.raw);
            if let Some(index) = expected_index {
                matched[index] = true;
                true
            } else {
                false
            }
        })
        .collect())
}

pub(super) fn marker_multiset_matches(
    expected: &[NamespaceMarker],
    actual: &[NamespaceMarker],
    include_namespace_facts: bool,
) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    let mut matched = vec![false; actual.len()];
    expected.iter().all(|marker| {
        let candidate = actual.iter().enumerate().position(|(index, candidate)| {
            !matched[index]
                && candidate.raw == marker.raw
                && (!include_namespace_facts || candidate.namespace_facts == marker.namespace_facts)
        });
        if let Some(index) = candidate {
            matched[index] = true;
            true
        } else {
            false
        }
    })
}

pub(super) fn nested_modeled_namespace_owners(xml: &[u8]) -> Result<Vec<NestedNamespaceOwner>> {
    let spans = modeled_owner_spans(xml)?;
    namespace_owners_from_spans(xml, &spans)
}

/// Owner rows for spans already read from `xml`. A whole part uses
/// [`nested_modeled_namespace_owners`].
pub(super) fn namespace_owners_from_spans(
    xml: &[u8],
    spans: &[ModeledOwnerSpan],
) -> Result<Vec<NestedNamespaceOwner>> {
    let mut owners = Vec::new();
    for span in spans {
        let mut declarations = span.declarations.as_ref().clone();
        if declarations.is_empty() {
            continue;
        }
        let markers = namespace_owner_markers(
            &xml[span.start..span.end],
            &declarations,
            &span.bindings,
            false,
        )?;
        if markers.is_empty() {
            continue;
        }
        let used_prefixes = markers
            .iter()
            .flat_map(|marker| marker.namespace_facts.iter())
            .map(|(prefix, _)| prefix.as_str())
            .collect::<HashSet<_>>();
        declarations.retain(|(name, _)| used_prefixes.contains(namespace_prefix(name).as_str()));
        let snapshot =
            logical_owner_snapshot(&xml[span.start..span.end], &span.bindings, &declarations)?;
        let peers = spans
            .iter()
            .filter(|candidate| candidate.local_name == span.local_name);
        let ordinal = peers
            .clone()
            .filter(|candidate| candidate.start < span.start)
            .count();
        let peers = peers.count();
        let ambiguous_without_namespace = spans.iter().any(|candidate| {
            if candidate.start == span.start || candidate.local_name != span.local_name {
                return false;
            }
            let Ok(candidate_snapshot) = logical_owner_snapshot(
                &xml[candidate.start..candidate.end],
                &candidate.bindings,
                &declarations,
            ) else {
                return true;
            };
            if candidate_snapshot != snapshot {
                return false;
            }
            let Ok(candidate_markers) = candidate_namespace_owner_markers(
                &xml[candidate.start..candidate.end],
                &declarations,
                &candidate.bindings,
                &markers,
            ) else {
                return true;
            };
            marker_multiset_matches(&markers, &candidate_markers, false)
        });
        let same_marker_structural_alternate = spans.iter().any(|candidate| {
            if candidate.start == span.start || candidate.local_name != span.local_name {
                return false;
            }
            let Ok(candidate_snapshot) = logical_owner_snapshot(
                &xml[candidate.start..candidate.end],
                &candidate.bindings,
                &declarations,
            ) else {
                return true;
            };
            if candidate_snapshot.structure != snapshot.structure {
                return false;
            }
            let Ok(candidate_markers) = candidate_namespace_owner_markers(
                &xml[candidate.start..candidate.end],
                &declarations,
                &candidate.bindings,
                &markers,
            ) else {
                return true;
            };
            marker_multiset_matches(&markers, &candidate_markers, false)
        });
        let same_namespace_structural_alternate = spans.iter().any(|candidate| {
            if candidate.start == span.start || candidate.local_name != span.local_name {
                return false;
            }
            let Ok(candidate_snapshot) = logical_owner_snapshot(
                &xml[candidate.start..candidate.end],
                &candidate.bindings,
                &declarations,
            ) else {
                return true;
            };
            if candidate_snapshot.structure != snapshot.structure {
                return false;
            }
            let Ok(candidate_markers) = candidate_namespace_owner_markers(
                &xml[candidate.start..candidate.end],
                &declarations,
                &candidate.bindings,
                &markers,
            ) else {
                return true;
            };
            marker_multiset_matches(&markers, &candidate_markers, true)
        });
        owners.push(NestedNamespaceOwner {
            source: span.start..span.end,
            local_name: span.local_name.to_owned(),
            declarations,
            snapshot,
            markers,
            ambiguous_without_namespace,
            same_marker_structural_alternate,
            same_namespace_structural_alternate,
            ordinal,
            peers,
        });
    }
    Ok(owners)
}

pub(super) fn unsafe_nested_namespace_prefix(owners: &[NestedNamespaceOwner]) -> Option<String> {
    owners.iter().find_map(|owner| {
        owner.declarations.iter().find_map(|(name, value)| {
            let prefix = namespace_prefix(name);
            if owner.local_name == "hyperlink" && prefix == "r" {
                return None;
            }
            canonical_serializer_namespace(&prefix)
                .is_some_and(|expected| value != expected)
                .then_some(prefix)
        })
    })
}

/// The refusal of a save that cannot keep the binding of `prefix`. The
/// empty prefix is a default namespace the body declares (a root's is kept),
/// which no known producer writes.
pub(super) fn shadowed_namespace_diagnostic(prefix: &str) -> String {
    if prefix.is_empty() {
        return format!(
            "cannot serialize a modified document whose body declares a default namespace: {DEFAULT_NAMESPACE_BELOW_ROOT}"
        );
    }
    format!("cannot serialize a modified document with a shadowed `{prefix}` namespace")
}

/// What an edit refused for a default namespace declared below the root
/// says. No known producer writes one there, so the refusal stays, with the
/// way out.
pub(super) const DEFAULT_NAMESPACE_BELOW_ROOT: &str = "a document that declares a default namespace below its root (on the body, a table or a block content control) cannot be edited; open and save it in Word or LibreOffice, which write the `w:` prefix, then edit the saved copy";
