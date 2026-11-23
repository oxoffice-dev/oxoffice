//! Capture namespace planning: the declarations and bindings an original capture needs.

use super::*;

#[derive(Default)]
pub(super) struct CaptureDeclarations {
    prefixes: Vec<String>,
    namespaces: Vec<String>,
    declaration_bytes: Vec<std::result::Result<usize, &'static str>>,
    indices: std::collections::HashMap<u64, Vec<u32>>,
}

impl CaptureDeclarations {
    /// Identical prefix and namespace pairs share one pool entry. Later
    /// physical copies still charge every retained record. Hashes only select
    /// a lookup bucket. Exact pair equality remains mandatory.
    pub(super) fn push(&mut self, prefix: String, namespace: String) -> Result<u32> {
        use std::hash::BuildHasher;
        let hash = self
            .indices
            .hasher()
            .hash_one((prefix.as_str(), namespace.as_str()));
        if let Some(indices) = self.indices.get(&hash)
            && let Some(index) = indices
                .iter()
                .copied()
                .find(|index| self.prefix(*index) == prefix && self.namespace(*index) == namespace)
        {
            return Ok(index);
        }
        let index = u32::try_from(self.prefixes.len())
            .map_err(|_| OxmlError::InvalidValue("original capture declaration overflow".into()))?;
        self.declaration_bytes
            .push(capture_declaration_bytes(&prefix, &namespace));
        self.prefixes.push(prefix);
        self.namespaces.push(namespace);
        self.indices.entry(hash).or_default().push(index);
        Ok(index)
    }

    /// Reusing an original fact never first clones its namespace URI.
    pub(super) fn intern(&mut self, prefix: &str, namespace: &str) -> Result<u32> {
        use std::hash::BuildHasher;
        let hash = self.indices.hasher().hash_one((prefix, namespace));
        if let Some(indices) = self.indices.get(&hash)
            && let Some(index) = indices
                .iter()
                .copied()
                .find(|index| self.prefix(*index) == prefix && self.namespace(*index) == namespace)
        {
            return Ok(index);
        }
        self.push(prefix.to_owned(), namespace.to_owned())
    }

    pub(super) fn prefix(&self, index: u32) -> &str {
        &self.prefixes[index as usize]
    }
    pub(super) fn namespace(&self, index: u32) -> &str {
        &self.namespaces[index as usize]
    }
    pub(super) fn declaration_bytes(&self, index: u32) -> Result<usize> {
        self.declaration_bytes[index as usize]
            .map_err(|message| OxmlError::InvalidValue(message.into()))
    }
}

#[derive(Clone, Copy)]
struct CaptureBinding {
    index: u32,
    depth: usize,
    declared: bool,
    /// Physical namespace attribute bytes already equal their decoded value.
    literal_namespace: bool,
}

/// Operation-local effective bindings. Empty XML scopes add no history entry.
pub(super) struct CaptureNamespaceState {
    histories: std::collections::HashMap<String, Vec<CaptureBinding>>,
    word_aliases: std::collections::HashSet<String>,
}

impl CaptureNamespaceState {
    pub(super) fn retain_part_declarations(
        &mut self,
        declarations: &[u32],
        pool: &CaptureDeclarations,
    ) {
        for &index in declarations {
            let history = self
                .histories
                .entry(pool.prefix(index).to_owned())
                .or_default();
            history.clear();
            history.push(CaptureBinding {
                index,
                depth: 0,
                declared: true,
                literal_namespace: true,
            });
        }
    }

    pub(super) fn new(inherited: &[u32], pool: &CaptureDeclarations, aliases: &[String]) -> Self {
        let mut histories = std::collections::HashMap::<String, Vec<CaptureBinding>>::new();
        for &index in inherited {
            histories
                .entry(pool.prefix(index).to_owned())
                .or_default()
                .push(CaptureBinding {
                    index,
                    depth: 0,
                    declared: false,
                    literal_namespace: true,
                });
        }
        Self {
            histories,
            word_aliases: aliases
                .iter()
                .filter(|alias| !alias.starts_with('\0'))
                .cloned()
                .collect(),
        }
    }

    pub(super) fn enter(
        &mut self,
        declarations: &[u32],
        depth: usize,
        pool: &CaptureDeclarations,
        literals: Option<&[bool]>,
    ) {
        for (at, &index) in declarations.iter().enumerate() {
            self.histories
                .entry(pool.prefix(index).to_owned())
                .or_default()
                .push(CaptureBinding {
                    index,
                    depth,
                    declared: true,
                    literal_namespace: literals.is_none_or(|values| values[at]),
                });
        }
    }

    pub(super) fn leave(&mut self, declarations: &[u32], pool: &CaptureDeclarations) {
        for &index in declarations.iter().rev() {
            let history = self
                .histories
                .get_mut(pool.prefix(index))
                .expect("entered namespace prefix");
            let binding = history.pop().expect("entered namespace binding");
            debug_assert_eq!(binding.index, index);
        }
    }
}

fn escaped_capture_len(value: &str) -> Result<usize> {
    value
        .bytes()
        .try_fold(0usize, |sum, byte| {
            sum.checked_add(match byte {
                b'&' => 5,
                b'<' | b'>' => 4,
                b'\'' | b'"' => 6,
                _ => 1,
            })
        })
        .ok_or_else(|| OxmlError::InvalidValue("original capture escaping overflow".into()))
}

fn capture_declaration_bytes(
    prefix: &str,
    namespace: &str,
) -> std::result::Result<usize, &'static str> {
    let mut bytes = 9usize;
    if !prefix.is_empty() {
        bytes = bytes
            .checked_add(prefix.len())
            .and_then(|bytes| bytes.checked_add(1))
            .ok_or("original capture declaration overflow")?;
    }
    let escaped =
        escaped_capture_len(namespace).map_err(|_| "original capture escaping overflow")?;
    bytes
        .checked_add(escaped)
        .ok_or("original capture declaration overflow")
}

pub(super) fn capture_index_push(indices: &mut Vec<u32>, index: u32) {
    if !indices.contains(&index) {
        indices.push(index);
    }
}

pub(super) fn classify_original_capture(
    empty: bool,
    word: bool,
    local: &[u8],
    modeled: bool,
    scope: &[OriginalCaptureScope],
    owners: &[OriginalCaptureOwner],
) -> OriginalCaptureClass {
    let parent_raw = scope.last().is_some_and(|parent| parent.raw);
    let parent_typed = scope.last().is_some_and(|parent| parent.typed_drawing);
    let parent_drawing_root = scope.last().is_some_and(|parent| parent.drawing_root);
    let parent_dropped = scope.last().is_some_and(|parent| parent.dropped);
    let direct_owner_child = owners
        .last()
        .is_some_and(|owner| scope.len() == owner.depth + 1);
    // Only a nonempty direct drawing under the run is inspected by the drawing
    // parser. Empty drawings stay verbatim raw children.
    let drawing_root = !empty
        && !parent_raw
        && !parent_typed
        && !parent_dropped
        && !parent_drawing_root
        && direct_owner_child
        && owners.last().is_some_and(|owner| owner.run)
        && word
        && local == b"drawing";
    // The drawing parser installs only a Start inline or anchor, matched by
    // local name. Empty inline and anchor elements are discarded.
    let typed_root = parent_drawing_root
        && crate::drawing::DrawingContainerKind::for_name(local, empty).is_some();
    let inside_typed = parent_typed || typed_root;
    let dropped = parent_dropped || (parent_drawing_root && !typed_root);
    let parent = scope.last();
    // Paragraph properties, annotations and other retained children cannot
    // introduce run owners merely by containing qualified-looking names.
    // These two wrappers are different: accepted reads expand their content
    // through the same paragraph parser.
    let opaque_paragraph_child = parent.is_some_and(|parent| parent.paragraph)
        && !(word
            && (admitted_word_child(b"p", local) || matches!(local, b"smartTag" | b"customXml")));
    let constructed = if scope.is_empty() {
        true
    } else {
        parent.is_some_and(|parent| match local {
            b"r" => parent.constructs_runs,
            b"hyperlink" => parent.constructs_hyperlinks,
            _ => false,
        })
    };
    // A run or hyperlink is an owner only when the enclosing constructor
    // parses that child. Property leaves, opaque extensions, and rejected
    // ruby annotations keep the bytes without manufacturing a nested owner.
    let owner = word
        && matches!(local, b"r" | b"hyperlink")
        && constructed
        && !parent_raw
        && !parent_dropped
        && !dropped
        && !parent.is_some_and(|parent| {
            parent.ruby_opaque || parent.unconstructed || parent.inline_drawing || parent.text_span
        });
    // Namespace membership is not a modeled-child test. Unknown Word-local
    // children such as contentPart stay raw. Descendants of an opaque or
    // discarded payload are not new owners.
    let raw = !inside_typed
        && !dropped
        && !drawing_root
        && !owner
        && (parent_raw || opaque_paragraph_child || (direct_owner_child && !modeled));
    OriginalCaptureClass {
        drawing_root,
        typed_root,
        inside_typed,
        dropped,
        raw,
        owner,
    }
}

fn inherited_capture_binding(value: &str) -> (&str, &str) {
    value
        .strip_prefix('\0')
        .and_then(|rest| rest.split_once('\0'))
        .unwrap_or((value, W_NS))
}

pub(super) fn reduce_inherited_bindings(prefixes: &[String]) -> Vec<(&str, &str)> {
    let mut last = std::collections::HashMap::new();
    for (position, value) in prefixes.iter().enumerate() {
        last.insert(inherited_capture_binding(value).0, position);
    }
    prefixes
        .iter()
        .enumerate()
        .filter_map(|(position, value)| {
            let (prefix, namespace) = inherited_capture_binding(value);
            (last.get(prefix) == Some(&position)).then_some((prefix, namespace))
        })
        .collect()
}

pub(super) fn original_capture_binding(
    prefix: &str,
    namespace_state: &CaptureNamespaceState,
) -> Option<(u32, usize)> {
    namespace_state
        .histories
        .get(prefix)
        .and_then(|history| history.last())
        .map(|binding| (binding.index, binding.depth))
}

pub(super) fn original_capture_root_attributes(
    start: &BytesStart<'_>,
    namespace_state: &CaptureNamespaceState,
    pool: &CaptureDeclarations,
) -> WordSourceResult<(usize, usize)> {
    let mut attribute_bytes = 0usize;
    let mut attributes = 0usize;
    let mut used = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        let name = attribute.key.as_ref();
        if name == b"xmlns" || name.starts_with(b"xmlns:") {
            continue;
        }
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
            .map_err(OxmlError::from)?
            .into_owned();
        let (prefix, local) = match name.iter().position(|byte| *byte == b':') {
            Some(separator) => (
                std::str::from_utf8(&name[..separator]).map_err(OxmlError::from)?,
                &name[separator + 1..],
            ),
            None => ("", name),
        };
        let namespace = if prefix == "xml" {
            Some("http://www.w3.org/XML/1998/namespace")
        } else {
            original_capture_binding(prefix, namespace_state)
                .map(|(index, _)| pool.namespace(index))
        };
        for value_prefix in crate::text::root_attribute_value_prefixes(namespace, local, &value) {
            if value_prefix == "xml"
                || used
                    .iter()
                    .any(|candidate: &String| candidate == value_prefix)
            {
                continue;
            }
            used.push(value_prefix.to_owned());
        }
        if prefix != "xml" && !used.iter().any(|candidate| candidate == prefix) {
            used.push(prefix.to_owned());
        }
        attribute_bytes = attribute_bytes
            .checked_add(name.len())
            .and_then(|bytes| bytes.checked_add(escaped_capture_len(&value).ok()?))
            .and_then(|bytes| bytes.checked_add(4))
            .ok_or_else(|| {
                WordSourceFailure::resource("original capture attribute overflow".into())
            })?;
        attributes += 1;
    }
    if attributes == 0 {
        return Ok((0, 0));
    }
    let mut record = attribute_bytes
        .checked_add(22)
        .ok_or_else(|| WordSourceFailure::resource("original capture record overflow".into()))?;
    for prefix in used {
        let Some((index, _)) = original_capture_binding(&prefix, namespace_state) else {
            continue;
        };
        record = record
            .checked_add(
                pool.declaration_bytes(index)
                    .map_err(WordSourceFailure::Resource)?,
            )
            .ok_or_else(|| {
                WordSourceFailure::resource("original capture record overflow".into())
            })?;
    }
    Ok((record, attribute_bytes))
}

pub(super) fn original_capture_event_prefixes(
    start: &BytesStart<'_>,
    namespace_state: &CaptureNamespaceState,
    pool: &CaptureDeclarations,
) -> Result<Vec<String>> {
    let name = start.name();
    let name = name.as_ref();
    let mut used = Vec::new();
    // An unprefixed element uses the default namespace. An unprefixed
    // attribute does not, matching record_external_bindings_used.
    let prefix = match name.iter().position(|byte| *byte == b':') {
        Some(separator) => std::str::from_utf8(&name[..separator])?,
        None => "",
    };
    if prefix != "xml" {
        used.push(prefix.to_owned());
    }
    for attribute in start.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        if key == b"xmlns" || key.starts_with(b"xmlns:") {
            continue;
        }
        let Some(separator) = key.iter().position(|byte| *byte == b':') else {
            continue;
        };
        let prefix = std::str::from_utf8(&key[..separator])?;
        if prefix != "xml" && !used.iter().any(|candidate| candidate == prefix) {
            used.push(prefix.to_owned());
        }
        let Some((index, _)) = original_capture_binding(prefix, namespace_state) else {
            continue;
        };
        let local = &key[separator + 1..];
        // Opaque payload values stay verbatim. Only a decodable namespace-
        // valued attribute contributes dependency prefixes. An undecodable
        // raw value is not a reason to refuse the whole input here.
        let Ok(value) =
            attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
        else {
            continue;
        };
        for value_prefix in
            crate::text::root_attribute_value_prefixes(Some(pool.namespace(index)), local, &value)
        {
            if value_prefix == "xml"
                || used
                    .iter()
                    .any(|candidate: &String| candidate == value_prefix)
            {
                continue;
            }
            used.push(value_prefix.to_owned());
        }
    }
    Ok(used)
}

pub(super) fn capture_non_namespace_attribute(start: &BytesStart<'_>) -> bool {
    start.attributes().flatten().any(|attribute| {
        let key = attribute.key.as_ref();
        key != b"xmlns" && !key.starts_with(b"xmlns:")
    })
}

pub(super) fn original_capture_word_name(
    name: &[u8],
    attribute: bool,
    namespace_state: &CaptureNamespaceState,
    pool: &CaptureDeclarations,
) -> bool {
    let prefix = match name.iter().position(|byte| *byte == b':') {
        Some(separator) => std::str::from_utf8(&name[..separator]).ok(),
        None if !attribute => Some(""),
        None => None,
    };
    let Some(prefix) = prefix else {
        return false;
    };
    namespace_state
        .histories
        .get(prefix)
        .and_then(|history| history.last())
        .is_some_and(|binding| {
            (binding.declared || namespace_state.word_aliases.contains(prefix))
                && pool.namespace(binding.index) == W_NS
        })
}

pub(super) fn constructor_namespace_name(
    name: &[u8],
    local: &[u8],
    namespace: &str,
    state: &CaptureNamespaceState,
    pool: &CaptureDeclarations,
    physical_since: usize,
) -> bool {
    if !matches_local_name(name, local) {
        return false;
    }
    let prefix = name
        .iter()
        .position(|byte| *byte == b':')
        .map_or(Ok(""), |at| std::str::from_utf8(&name[..at]));
    // These actual drawing consumers compare canonical ASCII namespace URIs.
    // NsReader preserves physical declaration bytes inside the captured
    // container. External bindings are decoded and emitted by its real closure.
    prefix
        .ok()
        .and_then(|prefix| state.histories.get(prefix)?.last())
        .is_some_and(|binding| {
            pool.namespace(binding.index) == namespace
                && (binding.depth < physical_since || binding.literal_namespace)
        })
}

pub(super) fn drawing_metadata_after_capture(
    (input, length): (WordXmlInput<'_>, usize),
) -> Result<u32> {
    let mut reader = Reader::from_reader(std::io::Read::take(input, length as u64));
    let mut buffer = Vec::new();
    match reader.read_event_into(&mut buffer)? {
        Event::Start(start) | Event::Empty(start) => first_drawing_metadata_id(&start),
        _ => Err(OxmlError::MissingElement("wp:docPr".into())),
    }
}

fn first_drawing_metadata_id(start: &BytesStart<'_>) -> Result<u32> {
    for attribute in start.attributes() {
        let attribute = attribute?;
        if let Some(value) = crate::drawing::drawing_id_value(
            &attribute,
            attribute.key.as_ref() == b"id",
            start.decoder(),
        )? {
            return Ok(value);
        }
    }
    Err(OxmlError::InvalidValue(
        "wp:docPr requires an unqualified id attribute".into(),
    ))
}

/// Only aliases physically used by this element enter the scalar view.
/// The actual scalar decoders retain their attribute-selection rules.
pub(super) fn constructor_attribute_prefixes(
    start: &BytesStart<'_>,
    namespace_state: &CaptureNamespaceState,
    pool: &CaptureDeclarations,
) -> Result<Vec<String>> {
    let mut prefixes = Vec::new();
    let name = start.name();
    if original_capture_word_name(name.as_ref(), false, namespace_state, pool) {
        let prefix = name
            .as_ref()
            .iter()
            .position(|byte| *byte == b':')
            .map_or("", |at| {
                std::str::from_utf8(&name.as_ref()[..at]).unwrap_or("")
            });
        prefixes.push(prefix.to_owned());
    }
    for attribute in start.attributes() {
        let attribute = attribute?;
        let name = attribute.key.as_ref();
        if original_capture_word_name(name, true, namespace_state, pool) {
            let at = name
                .iter()
                .position(|byte| *byte == b':')
                .expect("qualified attribute");
            let prefix = std::str::from_utf8(&name[..at])?;
            if !prefixes.iter().any(|known| known == prefix) {
                prefixes.push(prefix.to_owned());
            }
        }
    }
    Ok(prefixes)
}

/// Only lexical attribute prefixes can make this decoder fail resolution.
/// Namespace-valued payload dependencies are charged independently by capture.
pub(super) fn root_attribute_plan_failed(
    start: &BytesStart<'_>,
    namespaces: &CaptureNamespaceState,
    pool: &CaptureDeclarations,
) -> Result<bool> {
    let mut bindings = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute?;
        let Some(at) = attribute.key.as_ref().iter().position(|byte| *byte == b':') else {
            continue;
        };
        let Ok(prefix) = std::str::from_utf8(&attribute.key.as_ref()[..at]) else {
            return Ok(true);
        };
        if prefix == "xmlns"
            || prefix == "xml"
            || bindings.iter().any(|(known, _)| *known == prefix)
        {
            continue;
        }
        if let Some((index, _)) = original_capture_binding(prefix, namespaces) {
            bindings.push((pool.prefix(index), pool.namespace(index)));
        } else if let Some(alias) = namespaces.word_aliases.get(prefix) {
            bindings.push((alias.as_str(), W_NS));
        }
    }
    Ok(crate::text::RootAttributePlan::read(start, &bindings).is_err())
}

pub(super) fn simple_field_header_projection_stable(
    start: &BytesStart<'_>,
    namespace_state: &CaptureNamespaceState,
    pool: &CaptureDeclarations,
) -> Result<bool> {
    for attribute in start.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        let Some(at) = key.iter().position(|byte| *byte == b':') else {
            continue;
        };
        if &key[at + 1..] != b"instr" {
            continue;
        }
        let prefix = std::str::from_utf8(&key[..at])?;
        if original_capture_binding(prefix, namespace_state)
            .is_some_and(|(index, _)| pool.namespace(index) == W_NS)
        {
            return Ok(original_capture_word_name(key, true, namespace_state, pool));
        }
    }
    Ok(true)
}

pub(super) fn capture_revision_opens_runs(
    start: &BytesStart<'_>,
    parent_paragraph: bool,
    namespace_state: &CaptureNamespaceState,
    pool: &CaptureDeclarations,
) -> Result<bool> {
    let prefixes = constructor_attribute_prefixes(start, namespace_state, pool)?;
    Ok(crate::revision::RevisionHeader::read(start, &prefixes, !parent_paragraph).is_ok())
}

fn effective_run_frame_binding(
    prefix: &str,
    owner_depth: usize,
    namespace_state: &CaptureNamespaceState,
) -> Option<(u32, usize)> {
    let history = namespace_state.histories.get(prefix)?;
    let end = history.partition_point(|binding| binding.depth <= owner_depth);
    history
        .get(end.checked_sub(1)?)
        .map(|binding| (binding.index, binding.depth))
}

pub(super) fn charge_original_capture_binding(
    owner: &mut OriginalCaptureOwner,
    index: u32,
    depth: usize,
    pool: &CaptureDeclarations,
    inside_typed: bool,
    namespace_state: &CaptureNamespaceState,
) {
    let canonical_w = pool.prefix(index) == "w" && pool.namespace(index) == W_NS;
    let in_frame = depth <= owner.depth;
    let drawing_scope = depth == owner.depth + 1;
    if inside_typed && drawing_scope && !in_frame {
        // lost_bindings injects a drawing declaration only when the run frame
        // does not already carry the same prefix and URI. A repeated pair is
        // the ancestor binding on the shared record, and on the local record
        // when that ancestor declaration sits on the run itself.
        if let Some((frame_index, frame_depth)) =
            effective_run_frame_binding(pool.prefix(index), owner.depth, namespace_state)
            && pool.prefix(frame_index) == pool.prefix(index)
            && pool.namespace(frame_index) == pool.namespace(index)
        {
            capture_index_push(&mut owner.shared, index);
            if frame_depth == owner.depth && !canonical_w {
                capture_index_push(&mut owner.local, index);
            }
        } else if let Some(copy) = owner.open_raw.last_mut() {
            capture_index_push(copy, index);
        }
    } else if inside_typed && in_frame {
        capture_index_push(&mut owner.shared, index);
        // A run-root declaration used by a drawing is stored on the shared
        // record and again on the lexical local record.
        if depth == owner.depth && !canonical_w {
            capture_index_push(&mut owner.local, index);
        }
    } else if in_frame && !canonical_w {
        capture_index_push(&mut owner.shared, index);
    }
}

fn add_capture_declaration(
    total_needed: &mut usize,
    facts: &mut Vec<u32>,
    index: u32,
    pool: &CaptureDeclarations,
) -> Result<()> {
    *total_needed = total_needed
        .checked_add(pool.declaration_bytes(index)?)
        .ok_or_else(|| OxmlError::InvalidValue("original capture work overflow".into()))?;
    capture_index_push(facts, index);
    Ok(())
}

fn add_capture_record(
    indices: &[u32],
    total_needed: &mut usize,
    facts: &mut Vec<u32>,
    pool: &CaptureDeclarations,
) -> Result<()> {
    if indices.is_empty() {
        return Ok(());
    }
    *total_needed = total_needed
        .checked_add(22)
        .ok_or_else(|| OxmlError::InvalidValue("original capture work overflow".into()))?;
    for index in indices {
        add_capture_declaration(total_needed, facts, *index, pool)?;
    }
    Ok(())
}

pub(super) fn finish_original_capture_owner(
    owner: OriginalCaptureOwner,
    total_needed: &mut usize,
    facts: &mut Vec<u32>,
    pool: &CaptureDeclarations,
) -> Result<()> {
    add_capture_record(&owner.shared, total_needed, facts, pool)?;
    add_capture_record(&owner.local, total_needed, facts, pool)?;
    for copy in &owner.raw_copies {
        for index in copy {
            add_capture_declaration(total_needed, facts, *index, pool)?;
        }
    }
    for copy in &owner.open_raw {
        for index in copy {
            add_capture_declaration(total_needed, facts, *index, pool)?;
        }
    }
    if let Some((_, copy)) = &owner.open_comment {
        for index in copy {
            add_capture_declaration(total_needed, facts, *index, pool)?;
        }
    }
    Ok(())
}
