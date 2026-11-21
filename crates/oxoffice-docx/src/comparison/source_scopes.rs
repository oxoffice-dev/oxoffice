//! Source fragments, their parent scopes and the canonical form of an owned story.

use super::*;

/// Make a changed source owner self-contained without rewriting its payload.
/// Unchanged revision-bearing owners bypass this analysis/publication path.
pub(super) fn source_fragment(xml: &str, bindings: &[(String, String)]) -> Result<String> {
    let mut scope = BTreeMap::from([("w".to_owned(), W_NS.to_owned())]);
    for (name, value) in bindings {
        let prefix = name.strip_prefix("xmlns:").unwrap_or("");
        scope.insert(prefix.to_owned(), value.clone());
    }
    String::from_utf8(crate::document::close_content_fragment_namespaces(
        xml.as_bytes(),
        &scope,
    )?)
    .map_err(utf8_error)
}

pub(super) fn publication_parent_at(parent: &str, at: usize) -> Result<NamespaceBindings> {
    publication_parents_at(parent, &[at])
        .pop()
        .expect("one result per position")
}

/// The scope of the element that holds each position of `parent`, as
/// [`publication_parent_at`] gives it for that position alone.
///
/// One read of `parent` answers every position, so a parent of many children
/// is read once, not once per child. Each result, an error included, is the
/// one a read for that position alone gives, and is only met when a caller
/// takes it.
pub(super) fn publication_parents_at(
    parent: &str,
    positions: &[usize],
) -> Vec<Result<NamespaceBindings>> {
    if positions.is_empty() {
        return Vec::new();
    }
    let opening = format!(r#"<scope xmlns:w="{W_NS}">"#);
    let wrapped = format!("{opening}{parent}</scope>");
    let mut reader = NsReader::from_str(&wrapped);
    let mut scopes = Vec::<NamespaceBindings>::new();
    let mut results = positions.iter().map(|_| None).collect::<Vec<_>>();
    // The positions still to reach, the nearest last.
    let mut pending = (0..positions.len()).collect::<Vec<_>>();
    pending.sort_by_key(|index| std::cmp::Reverse(positions[*index]));
    // A position the read passed holds no event. Its read goes on to the end.
    let mut passed = Vec::new();
    let failure = loop {
        if pending.is_empty() && passed.is_empty() {
            break None;
        }
        let before = reader.buffer_position() as usize;
        let event = match reader.read_resolved_event() {
            Ok((_, event)) => event,
            Err(error) => break Some(error.to_string()),
        };
        while let Some(&index) = pending.last()
            && opening.len() + positions[index] <= before
        {
            pending.pop();
            if opening.len() + positions[index] == before {
                results[index] = Some(scopes.last().cloned().ok_or_else(|| {
                    Error::Other("publication boundary has no actual parent".into())
                }));
            } else {
                passed.push(index);
            }
        }
        match event {
            Event::Start(_) => match resolved_source_bindings(&reader) {
                Ok(bindings) => scopes.push(bindings),
                Err(error) => break Some(error.to_string()),
            },
            Event::End(_) => {
                scopes.pop();
            }
            Event::Eof => {
                break Some("publication boundary is absent from its actual shell".into());
            }
            _ => {}
        }
    };
    if let Some(message) = failure {
        for index in pending.into_iter().chain(passed) {
            results[index] = Some(Err(Error::Other(message.clone())));
        }
    }
    results
        .into_iter()
        .map(|result| result.expect("every position is answered"))
        .collect()
}

/// The scopes [`publication_parent_at`] gives for the positions of one
/// parent that a caller may ask, read in one pass on the first question.
/// A position outside that set is read alone.
pub(super) struct ParentScopes<'a> {
    parent: &'a str,
    positions: Vec<usize>,
    scopes: std::cell::OnceCell<HashMap<usize, Result<NamespaceBindings>>>,
}

impl<'a> ParentScopes<'a> {
    pub(super) fn new(parent: &'a str, positions: Vec<usize>) -> Self {
        Self {
            parent,
            positions,
            scopes: std::cell::OnceCell::new(),
        }
    }

    pub(super) fn at(&self, position: usize) -> Result<NamespaceBindings> {
        let scopes = self.scopes.get_or_init(|| {
            self.positions
                .iter()
                .copied()
                .zip(publication_parents_at(self.parent, &self.positions))
                .collect()
        });
        match scopes.get(&position) {
            Some(Ok(scope)) => Ok(scope.clone()),
            // Every error of that read is `Error::Other`, so this is that error.
            Some(Err(error)) => Err(Error::Other(error.to_string())),
            None => publication_parent_at(self.parent, position),
        }
    }
}

pub(super) fn source_child(parent: &str, span: Range<usize>) -> Result<String> {
    source_children(parent, &[span])
        .pop()
        .expect("one result per span")
}

/// The child of `parent` at each span with the namespaces in scope declared
/// on its root, as [`source_child`] gives it for that span alone.
///
/// One read of `parent` answers every span. Each result, an error included,
/// is the one a read for that span alone gives.
pub(super) fn source_children(parent: &str, spans: &[Range<usize>]) -> Vec<Result<String>> {
    if spans.is_empty() {
        return Vec::new();
    }
    let opening = format!(r#"<scope xmlns:w="{W_NS}">"#);
    let wrapped = format!("{opening}{parent}</scope>");
    let mut reader = NsReader::from_str(&wrapped);
    let mut results = spans.iter().map(|_| None).collect::<Vec<_>>();
    // The spans still to reach, the nearest last.
    let mut pending = (0..spans.len()).collect::<Vec<_>>();
    pending.sort_by_key(|index| std::cmp::Reverse(spans[*index].start));
    // A span the read passed opens no element. Its read goes on to the end.
    let mut passed = Vec::new();
    let failure = loop {
        if pending.is_empty() && passed.is_empty() {
            break None;
        }
        let before = reader.buffer_position() as usize;
        let event = match reader.read_resolved_event() {
            Ok((_, event)) => event,
            Err(error) => break Some(format!("source namespace scan failed: {error}")),
        };
        let opens = matches!(event, Event::Start(_) | Event::Empty(_));
        while let Some(&index) = pending.last() {
            let start = opening.len() + spans[index].start;
            if start < before {
                pending.pop();
                passed.push(index);
            } else if start == before && opens {
                pending.pop();
                results[index] = Some(source_child_at(&reader, parent, spans[index].clone()));
            } else {
                break;
            }
        }
        if matches!(event, Event::Eof) {
            break Some("source child has no opening at its span".into());
        }
    };
    if let Some(message) = failure {
        for index in pending.into_iter().chain(passed) {
            results[index] = Some(Err(Error::Other(message.clone())));
        }
    }
    results
        .into_iter()
        .map(|result| result.expect("every span is answered"))
        .collect()
}

/// The parent scope and the scoped source of the child of `parent` at each
/// span, from one read of `parent` for each, in the order of `spans`.
pub(super) fn child_scopes(
    parent: &str,
    spans: &[Range<usize>],
) -> std::iter::Zip<std::vec::IntoIter<Result<NamespaceBindings>>, std::vec::IntoIter<Result<String>>>
{
    let starts = spans.iter().map(|span| span.start).collect::<Vec<_>>();
    publication_parents_at(parent, &starts)
        .into_iter()
        .zip(source_children(parent, spans))
}

/// The child at `span` with the bindings `reader` holds at its opening.
fn source_child_at(reader: &NsReader<&[u8]>, parent: &str, span: Range<usize>) -> Result<String> {
    let bindings = reader
        .resolver()
        .bindings()
        .map(|(prefix, uri)| {
            let prefix = match prefix {
                quick_xml::name::PrefixDeclaration::Default => String::new(),
                quick_xml::name::PrefixDeclaration::Named(prefix) => {
                    String::from_utf8(prefix.to_vec()).map_err(utf8_error)?
                }
            };
            let uri = std::str::from_utf8(uri.as_ref()).map_err(utf8_error)?;
            Ok((
                if prefix.is_empty() {
                    "xmlns".to_owned()
                } else {
                    format!("xmlns:{prefix}")
                },
                quick_xml::escape::unescape(uri)
                    .map_err(|error| Error::Other(error.to_string()))?
                    .into_owned(),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    source_fragment(&parent[span], &bindings)
}

pub(super) fn root_namespace_bindings(
    xml: &str,
    expected_local: &str,
    inherited: &[(String, String)],
) -> Result<NamespaceBindings> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison namespace parse failed: {error}")))?
        {
            Event::Start(element) | Event::Empty(element) => {
                if element.local_name().as_ref() != expected_local.as_bytes() {
                    return Err(Error::Other(format!(
                        "comparison namespace scope expected {expected_local} root"
                    )));
                }
                let mut bindings = inherited.to_vec();
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| {
                        Error::Other(format!("comparison namespace attribute failed: {error}"))
                    })?;
                    let key = attribute.key.as_ref();
                    if key != b"xmlns" && !key.starts_with(b"xmlns:") {
                        continue;
                    }
                    let name = std::str::from_utf8(key).map_err(utf8_error)?.to_owned();
                    let value = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                        .map_err(|error| {
                            Error::Other(format!(
                                "comparison namespace value decode failed: {error}"
                            ))
                        })?
                        .into_owned();
                    bindings.retain(|(existing, _)| existing != &name);
                    bindings.push((name, value));
                }
                return Ok(bindings);
            }
            Event::Eof => {
                return Err(Error::Other(format!(
                    "comparison {expected_local} namespace scope is missing"
                )));
            }
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn root_inner_range(xml: &str, expected_local: &str) -> Result<Range<usize>> {
    let mut reader = NsReader::from_reader(xml.as_bytes());
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison XML scan failed: {error}")))?;
        let is_word =
            matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes());
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                if depth == 0 {
                    if element.local_name().as_ref() != expected_local.as_bytes() || !is_word {
                        return Err(Error::Other(format!(
                            "comparison expected a Word {expected_local} root"
                        )));
                    }
                    start = Some(after);
                }
                depth += 1;
            }
            Event::Empty(element) if depth == 0 => {
                if element.local_name().as_ref() == expected_local.as_bytes() && is_word {
                    return Ok(after..after);
                }
                return Err(Error::Other(format!(
                    "comparison expected a Word {expected_local} root"
                )));
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok(start.unwrap_or(before)..before);
                }
            }
            Event::Eof => {
                return Err(Error::Other(format!(
                    "comparison XML has no closed {expected_local} root"
                )));
            }
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn story_skeleton(xml: &str, spans: &[Range<usize>]) -> String {
    let mut skeleton = xml.to_owned();
    for span in spans.iter().rev() {
        skeleton.replace_range(span.clone(), "<owner/>");
    }
    skeleton
}

/// Whether owner `left` of the original and owner `right` of the edited
/// source are one tree, as [`canonical_owner_tokens`] reads them.
pub(super) fn same_owner_tree(
    original: Option<OriginalBodySource<'_>>,
    left: usize,
    edited: OriginalBodySource<'_>,
    right: usize,
) -> Result<bool> {
    let Some((source, spans, bindings)) = original else {
        return Ok(false);
    };
    let (edited_source, edited_spans, edited_bindings) = edited;
    let left = source_fragment(&source[spans[left].clone()], bindings)?;
    let right = source_fragment(&edited_source[edited_spans[right].clone()], edited_bindings)?;
    Ok(canonical_owner_tokens(&left)? == canonical_owner_tokens(&right)?)
}

/// Whether an owner holding an earlier revision is the same in both
/// versions: the same typed owner with the same source residual; or one tree
/// another save wrote another way (references, attribute and declaration
/// order); or, with the same residual, an owner the model writes back the
/// same (a save rewrites values it reads alike, such as `0` as `false`, and
/// leaves empty containers out). The residual keeps what the reader drops.
pub(super) fn same_held_owner(
    typed_equal: bool,
    same_residual: bool,
    same_tree: impl FnOnce() -> Result<bool>,
    same_written: impl FnOnce() -> Result<bool>,
) -> Result<bool> {
    Ok(typed_equal && same_residual || same_tree()? || same_residual && same_written()?)
}

/// Whether two owners are written as one tree by the model's own writer.
pub(super) fn same_written_owner(
    original: &BodyContent,
    edited: &BodyContent,
    original_bindings: &[(String, String)],
    edited_bindings: &[(String, String)],
) -> Result<bool> {
    let left = source_fragment(&body_content_xml(original)?, original_bindings)?;
    let right = source_fragment(&body_content_xml(edited)?, edited_bindings)?;
    Ok(canonical_owner_tokens(&left)? == canonical_owner_tokens(&right)?)
}

/// One owner's XML read so that two serializations of one tree compare
/// equal, as [`canonical_owned_story`] reads a story, and further: character
/// and entity references read as the characters they stand for, and
/// adjacent text reads as one token. Blank text counts everywhere: it is
/// content in Word's and math's text elements, and two writings that differ
/// in it are not taken for one tree.
fn canonical_owner_tokens(xml: &str) -> Result<Vec<String>> {
    let scan_error = |error: &dyn std::fmt::Display| {
        Error::Other(format!("comparison owner scan failed: {error}"))
    };
    let mut reader = NsReader::from_reader(xml.as_bytes());
    reader.config_mut().trim_text(false);
    let mut tokens = Vec::new();
    let mut text = String::new();
    let flush = |tokens: &mut Vec<String>, text: &mut String| {
        if !text.is_empty() {
            tokens.push(format!("text {:?}", std::mem::take(text)));
        }
    };
    let mut buffer = Vec::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| scan_error(&error))?;
        let (element, empty) = match event {
            Event::Start(element) => (element, false),
            Event::Empty(element) => (element, true),
            Event::End(_) => {
                flush(&mut tokens, &mut text);
                tokens.push("end".to_owned());
                buffer.clear();
                continue;
            }
            Event::Text(content) => {
                text.push_str(
                    &content
                        .xml10_content()
                        .map_err(|error| scan_error(&error))?,
                );
                buffer.clear();
                continue;
            }
            Event::CData(content) => {
                text.push_str(&content.decode().map_err(|error| scan_error(&error))?);
                buffer.clear();
                continue;
            }
            Event::GeneralRef(reference) => {
                if let Some(character) = reference
                    .resolve_char_ref()
                    .map_err(|error| scan_error(&error))?
                {
                    text.push(character);
                } else {
                    let name = reference.decode().map_err(|error| scan_error(&error))?;
                    let value = quick_xml::escape::resolve_predefined_entity(&name)
                        .ok_or_else(|| scan_error(&format!("unknown entity &{name};")))?;
                    text.push_str(value);
                }
                buffer.clear();
                continue;
            }
            Event::Eof => break,
            _ => {
                buffer.clear();
                continue;
            }
        };
        flush(&mut tokens, &mut text);
        let resolver = reader.resolver();
        let (namespace, local) = resolver.resolve_element(element.name());
        let mut attributes = Vec::new();
        for attribute in element.attributes() {
            let attribute = attribute.map_err(|error| scan_error(&error))?;
            let key = attribute.key.as_ref();
            if key == b"xmlns" || key.starts_with(b"xmlns:") {
                continue;
            }
            let (attribute_namespace, attribute_local) = resolver.resolve_attribute(attribute.key);
            if matches!(
                attribute_namespace,
                ResolveResult::Bound(Namespace(uri)) if uri == oxoffice_core::xml::MC_NS.as_bytes()
            ) {
                continue;
            }
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .map_err(|error| scan_error(&error))?;
            attributes.push(format!(
                "{attribute_namespace:?} {:?} {value:?}",
                String::from_utf8_lossy(attribute_local.as_ref())
            ));
        }
        attributes.sort();
        tokens.push(format!(
            "start {namespace:?} {:?} {attributes:?}",
            String::from_utf8_lossy(local.as_ref())
        ));
        if empty {
            tokens.push("end".to_owned());
        }
        buffer.clear();
    }
    flush(&mut tokens, &mut text);
    Ok(tokens)
}

/// An owned story read so that two serializations of one tree compare equal:
/// the part with each owner as one placeholder, then each owner, whose first
/// token is its start tag.
///
/// Names resolve to their namespaces, attributes compare as a sorted set and
/// an empty element reads as a start and an end. The XML declaration,
/// comments, processing instructions, whitespace-only text, namespace
/// declarations and Markup Compatibility attributes are left out. They say
/// how the part is written, not what it holds, and the redline keeps the
/// original bytes.
pub(super) fn canonical_owned_story(
    xml: &str,
    owner_local: &str,
) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    let scan_error = |error: &dyn std::fmt::Display| {
        Error::Other(format!("comparison story shell scan failed: {error}"))
    };
    let mut reader = NsReader::from_reader(xml.as_bytes());
    reader.config_mut().trim_text(false);
    let mut skeleton = Vec::new();
    let mut owners = Vec::<Vec<String>>::new();
    let mut in_owner = false;
    let mut depth = 0usize;
    let mut buffer = Vec::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| scan_error(&error))?;
        let tokens = match (in_owner, owners.last_mut()) {
            (true, Some(owner)) => owner,
            _ => &mut skeleton,
        };
        let (element, empty) = match event {
            Event::Start(element) => (element, false),
            Event::Empty(element) => (element, true),
            Event::End(_) => {
                tokens.push("end".to_owned());
                depth = depth.saturating_sub(1);
                in_owner &= depth > 1;
                buffer.clear();
                continue;
            }
            Event::Text(text) if !text.iter().all(u8::is_ascii_whitespace) => {
                tokens.push(format!("text {:?}", String::from_utf8_lossy(&text)));
                buffer.clear();
                continue;
            }
            Event::CData(text) => {
                tokens.push(format!("text {:?}", String::from_utf8_lossy(&text)));
                buffer.clear();
                continue;
            }
            Event::GeneralRef(reference) => {
                tokens.push(format!(
                    "reference {:?}",
                    String::from_utf8_lossy(&reference)
                ));
                buffer.clear();
                continue;
            }
            Event::Eof => break,
            _ => {
                buffer.clear();
                continue;
            }
        };
        let resolver = reader.resolver();
        let (namespace, local) = resolver.resolve_element(element.name());
        let mut attributes = Vec::new();
        for attribute in element.attributes() {
            let attribute = attribute.map_err(|error| scan_error(&error))?;
            let key = attribute.key.as_ref();
            if key == b"xmlns" || key.starts_with(b"xmlns:") {
                continue;
            }
            let (attribute_namespace, attribute_local) = resolver.resolve_attribute(attribute.key);
            if matches!(
                attribute_namespace,
                ResolveResult::Bound(Namespace(uri)) if uri == oxoffice_core::xml::MC_NS.as_bytes()
            ) {
                continue;
            }
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .map_err(|error| scan_error(&error))?;
            attributes.push(format!(
                "{attribute_namespace:?} {:?} {value:?}",
                String::from_utf8_lossy(attribute_local.as_ref())
            ));
        }
        attributes.sort();
        let start = format!(
            "start {namespace:?} {:?} {attributes:?}",
            String::from_utf8_lossy(local.as_ref())
        );
        let is_owner = depth == 1
            && local.as_ref() == owner_local.as_bytes()
            && matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes());
        let tokens = if is_owner {
            skeleton.push("owner".to_owned());
            in_owner = !empty;
            owners.push(Vec::new());
            owners.last_mut().expect("owner was just pushed")
        } else {
            tokens
        };
        tokens.push(start);
        if empty {
            tokens.push("end".to_owned());
        } else {
            depth += 1;
        }
        buffer.clear();
    }
    Ok((skeleton, owners))
}
