//! Namespace bindings and root attributes: scopes while writing, generated names, w14, retained roots and external bindings.

use super::*;

pub(super) const ROOT_ATTRIBUTES_ELEMENT: &[u8] = b"rdocxRootAttributes";
pub(crate) const ROOT_ATTRIBUTES_POSITION: usize = usize::MAX;
pub(super) const W14_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordml";
pub(super) const MC_NS: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
pub(super) const WP_NS: &str =
    "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
pub(crate) const ROOT_R_BINDING: u8 = 1;
pub(crate) const ROOT_MC_BINDING: u8 = 2;
pub(crate) const ROOT_WP_BINDING: u8 = 4;

thread_local! {
    pub(super) static ROOT_BINDINGS: Cell<u8> = const { Cell::new(0) };
    static EMITTED_BINDINGS: RefCell<Option<Rc<EmittedBindings>>> = const { RefCell::new(None) };
}

/// Keep part-root guarantees in scope only while that part is serialized.
/// Nested serializations restore the caller's guarantees, including on unwind.
struct EmittedBindings {
    parent: Option<Rc<EmittedBindings>>,
    local: Vec<(String, String)>,
    omission_policy: u8,
    late_w14: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum XmlWriteRole {
    Source,
    Publication,
}

pub(crate) struct RootBindingScope(u8, Option<Rc<EmittedBindings>>);

pub(crate) fn root_binding_scope(bindings: u8) -> RootBindingScope {
    RootBindingScope(
        ROOT_BINDINGS.with(|current| current.replace(bindings)),
        EMITTED_BINDINGS.with(|current| current.borrow_mut().take()),
    )
}

/// Only declarations on the actual emitted start establish destination facts.
/// A part replaces an unrelated caller, while a child overlays its parent.
pub(crate) fn emitted_binding_scope(
    start: &BytesStart<'_>,
    replace: bool,
    late_w14: bool,
) -> Result<RootBindingScope> {
    let policy = EMITTED_BINDINGS.with(|current| {
        current.borrow().as_ref().map_or_else(
            || ROOT_BINDINGS.with(Cell::get),
            |frame| frame.omission_policy,
        )
    });
    emitted_scope(start, replace, late_w14, policy)
}

pub(crate) fn emitted_root_binding_scope(
    start: &BytesStart<'_>,
    policy: u8,
    late_w14: bool,
) -> Result<RootBindingScope> {
    emitted_scope(start, true, late_w14, policy)
}

fn emitted_scope(
    start: &BytesStart<'_>,
    replace: bool,
    late_w14: bool,
    policy: u8,
) -> Result<RootBindingScope> {
    let mut local = raw_namespace_declarations(start)?;
    if replace && !local.iter().any(|(prefix, _)| prefix.is_empty()) {
        local.push((String::new(), String::new()));
    }
    // Existing document/header policy withheld WP for a foreign root binding.
    // A later canonical child must not enable an optimization that part never had.
    let policy = if replace
        && local
            .iter()
            .any(|(prefix, namespace)| prefix == "wp" && namespace != WP_NS)
    {
        policy & !ROOT_WP_BINDING
    } else {
        policy
    };
    install_emitted_scope(local, replace, late_w14, policy)
}

pub(crate) fn emitted_ancestor_scope(local: &[(String, String)]) -> Result<RootBindingScope> {
    // Admission precedes owned guard copies. These are actual local frames,
    // not a flattened source scope or the finalizer's later document root.
    retained_binding_work(0, local, 0)?;
    let policy = EMITTED_BINDINGS.with(|current| {
        current
            .borrow()
            .as_ref()
            .map_or(0, |frame| frame.omission_policy)
    });
    install_emitted_scope(local.to_vec(), false, false, policy)
}

fn install_emitted_scope(
    local: Vec<(String, String)>,
    replace: bool,
    late_w14: bool,
    policy: u8,
) -> Result<RootBindingScope> {
    let previous = EMITTED_BINDINGS.with(|current| current.borrow().clone());
    let frame = Rc::new(EmittedBindings {
        parent: if replace { None } else { previous.clone() },
        local,
        omission_policy: policy,
        late_w14,
    });
    let guard = RootBindingScope(ROOT_BINDINGS.with(Cell::get), previous);
    EMITTED_BINDINGS.with(|current| *current.borrow_mut() = Some(frame));
    let snapshot = emitted_bindings();
    let scope = snapshot.bindings();
    let guarantees = {
        [
            ("r", crate::namespace::R_NS, ROOT_R_BINDING),
            ("mc", MC_NS, ROOT_MC_BINDING),
            ("wp", WP_NS, ROOT_WP_BINDING),
        ]
        .into_iter()
        .filter(|(prefix, namespace, _)| scope.contains(&(*prefix, *namespace)))
        .fold(0, |bits, (_, _, bit)| bits | bit)
            & policy
    };
    ROOT_BINDINGS.with(|current| current.set(guarantees));
    Ok(guard)
}

pub(crate) struct EmittedBindingSnapshot(Option<Rc<EmittedBindings>>);

pub(crate) fn emitted_bindings() -> EmittedBindingSnapshot {
    EmittedBindingSnapshot(EMITTED_BINDINGS.with(|current| current.borrow().clone()))
}

impl EmittedBindingSnapshot {
    pub(crate) fn has_foreign_word_binding(&self) -> bool {
        let mut next = self.0.as_deref();
        while let Some(frame) = next {
            if let Some((_, namespace)) = frame.local.iter().rev().find(|(prefix, _)| prefix == "w")
            {
                return namespace != crate::namespace::W_NS;
            }
            next = frame.parent.as_deref();
        }
        false
    }

    fn guarantees_w14_name(&self) -> bool {
        let scope = self.bindings();
        if let Some((_, namespace)) = scope.iter().find(|(prefix, _)| *prefix == "w14") {
            return *namespace == W14_NS;
        }
        let mut next = self.0.as_deref();
        while let Some(frame) = next {
            if frame.late_w14 {
                return true;
            }
            next = frame.parent.as_deref();
        }
        false
    }

    pub(crate) fn bindings_for(&self, raw: &[u8]) -> Vec<(&str, &str)> {
        let mut bindings = self.bindings();
        if !bindings.iter().any(|(prefix, _)| *prefix == "w14")
            && self.guarantees_w14_name()
            && uses_w14_qualified_name(raw)
        {
            bindings.push(("w14", W14_NS));
        }
        bindings
    }

    pub(crate) fn bindings(&self) -> Vec<(&str, &str)> {
        let frame = &self.0;
        let mut scope = Vec::new();
        let mut next = frame.as_deref();
        while let Some(frame) = next {
            for (prefix, namespace) in frame.local.iter().rev() {
                if !scope.iter().any(|(known, _)| *known == prefix.as_str()) {
                    scope.push((prefix.as_str(), namespace.as_str()));
                }
            }
            next = frame.parent.as_deref();
        }
        if !scope.iter().any(|(prefix, _)| *prefix == "w") {
            scope.push(("w", crate::namespace::W_NS));
        }
        scope
    }
}

impl Drop for RootBindingScope {
    fn drop(&mut self) {
        ROOT_BINDINGS.with(|current| current.set(self.0));
        EMITTED_BINDINGS.with(|current| *current.borrow_mut() = self.1.take());
    }
}

pub(crate) fn raw_root_binding_scope(raw: &[u8], replace: bool) -> Result<RootBindingScope> {
    let mut reader = Reader::from_reader(raw);
    loop {
        match reader.read_event()? {
            Event::Start(start) | Event::Empty(start) => {
                return if replace {
                    emitted_root_binding_scope(&start, 0, false)
                } else {
                    emitted_binding_scope(&start, false, false)
                };
            }
            Event::Eof => return Err(OxmlError::MissingElement("serialization root".into())),
            _ => {}
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum GeneratedWordPrefix<'a> {
    Existing(&'a str),
    Fresh(&'a str, usize),
}

impl<'a> GeneratedWordName<'a> {
    pub(crate) fn fixed(prefix: &'a str) -> Self {
        Self {
            prefix: GeneratedWordPrefix::Existing(prefix),
            declaration: false,
        }
    }

    pub(super) fn prefix_len(&self) -> usize {
        match self.prefix {
            GeneratedWordPrefix::Existing(prefix) => prefix.len(),
            GeneratedWordPrefix::Fresh(seed, 0) => seed.len(),
            GeneratedWordPrefix::Fresh(seed, index) => {
                seed.len() + itoa::Buffer::new().format(index).len()
            }
        }
    }

    pub(super) fn render(&self) -> String {
        match self.prefix {
            GeneratedWordPrefix::Existing(prefix) => prefix.to_owned(),
            GeneratedWordPrefix::Fresh(seed, 0) => seed.to_owned(),
            GeneratedWordPrefix::Fresh(seed, index) => format!("{seed}{index}"),
        }
    }
}

pub(crate) fn generated_word_name<'a>(
    bindings: &[(&'a str, &str)],
) -> Result<GeneratedWordName<'a>> {
    let word = crate::namespace::W_NS;
    if bindings
        .iter()
        .any(|(prefix, namespace)| *prefix == "w" && *namespace != word)
    {
        if let Some((prefix, _)) = bindings
            .iter()
            .find(|(prefix, namespace)| !prefix.is_empty() && *namespace == word)
        {
            return Ok(GeneratedWordName {
                prefix: GeneratedWordPrefix::Existing(prefix),
                declaration: false,
            });
        }
        let mut index = 0usize;
        loop {
            let mut digits = itoa::Buffer::new();
            let suffix = if index == 0 { "" } else { digits.format(index) };
            if !bindings
                .iter()
                .any(|(known, _)| known.strip_prefix("rdocxW") == Some(suffix))
            {
                return Ok(GeneratedWordName {
                    prefix: GeneratedWordPrefix::Fresh("rdocxW", index),
                    declaration: true,
                });
            }
            index = index
                .checked_add(1)
                .ok_or_else(|| OxmlError::InvalidValue("generated Word prefix overflow".into()))?;
        }
    }
    Ok(GeneratedWordName {
        prefix: GeneratedWordPrefix::Existing("w"),
        declaration: !bindings.contains(&("w", word)),
    })
}

pub(crate) fn generated_word_start<'name>(
    start: BytesStart<'name>,
    original: &[String],
    role: XmlWriteRole,
    source_context: Option<&[(&str, &str)]>,
) -> Result<(BytesStart<'name>, String)> {
    generated_word_start_in(start, original, role, source_context, None)
}

pub(crate) fn generated_word_start_in<'name>(
    mut start: BytesStart<'name>,
    original: &[String],
    role: XmlWriteRole,
    source_context: Option<&[(&str, &str)]>,
    emission: Option<&FieldXmlEmission>,
) -> Result<(BytesStart<'name>, String)> {
    let snapshot = emitted_bindings();
    let mut bindings = if role == XmlWriteRole::Source {
        source_context.map_or_else(|| field_scope_bindings(original), |scope| scope.to_vec())
    } else {
        snapshot.bindings()
    };
    let declarations = raw_namespace_declarations(&start)?;
    for (prefix, namespace) in &declarations {
        bindings.retain(|(known, _)| *known != prefix);
        bindings.push((prefix, namespace));
    }
    let word = crate::namespace::W_NS;
    let plan = generated_word_name(&bindings)?;
    let local_name = start.local_name();
    let local = std::str::from_utf8(local_name.as_ref())?;
    let name_len = plan
        .prefix_len()
        .checked_add(1)
        .and_then(|n| n.checked_add(local.len()))
        .ok_or_else(|| OxmlError::InvalidValue("normalized event QName length overflow".into()))?;
    if let Some(emission) = emission {
        // Owned event input means set_name grows this same buffer. It performs
        // no distinct whole-event to_owned/clone before this growth guard.
        emission.reserve_inner_xml(name_len.saturating_sub(start.name().as_ref().len()))?;
        if plan.declaration {
            let bytes = 6usize
                .checked_add(plan.prefix_len())
                .and_then(|n| n.checked_add(4))
                .and_then(|n| n.checked_add(word.len()))
                .ok_or_else(|| {
                    OxmlError::InvalidValue("normalized namespace event length overflow".into())
                })?;
            emission.reserve_inner_xml(bytes)?;
        }
    }
    let prefix = plan.render();
    start.set_name(format!("{prefix}:{local}").as_bytes());
    if plan.declaration {
        let declaration = format!("xmlns:{prefix}");
        start.push_attribute((declaration.as_str(), word));
    }
    Ok((start, prefix))
}

pub(crate) fn restore_generated_word_binding(start: &mut BytesStart<'_>) -> Result<()> {
    if needs_generated_word_binding(start)? {
        start.push_attribute(("xmlns:w", crate::namespace::W_NS));
    }
    Ok(())
}

pub(super) fn needs_generated_word_binding(start: &BytesStart<'_>) -> Result<bool> {
    let local = raw_namespace_declarations(start)?;
    let snapshot = emitted_bindings();
    Ok(!local.iter().any(|(prefix, _)| prefix == "w") && snapshot.has_foreign_word_binding())
}

pub(super) fn namespace_declaration(name: &[u8]) -> bool {
    name == b"xmlns" || name.starts_with(b"xmlns:")
}

pub(crate) fn root_attribute_namespace<'a>(
    name: &[u8],
    bindings: &[(&str, &'a str)],
) -> Result<Option<&'a str>> {
    let Some(separator) = name.iter().position(|byte| *byte == b':') else {
        return Ok(None);
    };
    let prefix = &name[..separator];
    if prefix == b"xml" {
        return Ok(Some("http://www.w3.org/XML/1998/namespace"));
    }
    bindings
        .iter()
        .find(|(candidate, _)| candidate.as_bytes() == prefix)
        .map(|(_, namespace)| Some(*namespace))
        .ok_or_else(|| {
            OxmlError::InvalidValue(format!(
                "root attribute prefix `{}` is unbound",
                String::from_utf8_lossy(prefix)
            ))
        })
}

/// Prefix dependencies of the known namespace-valued root attributes.
/// Attribute names and opaque literal values use their normal preservation path.
pub(crate) fn root_attribute_value_prefixes<'a>(
    namespace: Option<&str>,
    local: &[u8],
    value: &'a str,
) -> Vec<&'a str> {
    if namespace == Some(crate::namespace::MC_NS) {
        match local {
            b"Ignorable" | b"MustUnderstand" => return value.split_ascii_whitespace().collect(),
            b"ProcessContent" | b"PreserveAttributes" | b"PreserveElements" => {
                return value
                    .split_ascii_whitespace()
                    .filter_map(|token| token.split_once(':').map(|(prefix, _)| prefix))
                    .collect();
            }
            _ => {}
        }
    } else if namespace == Some("http://www.w3.org/2001/XMLSchema-instance") && local == b"type" {
        return value
            .split_ascii_whitespace()
            .map(|token| token.split_once(':').map_or("", |(prefix, _)| prefix))
            .collect();
    }
    Vec::new()
}
impl RootAttributePlan {
    pub(crate) fn read(start: &BytesStart<'_>, bindings: &[(&str, &str)]) -> Result<Self> {
        let mut attributes = Vec::new();
        let mut expanded = HashSet::new();
        let mut used_prefixes = Vec::new();
        for attribute in start.attributes() {
            let attribute = attribute?;
            let name = attribute.key.as_ref();
            let value =
                attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())?;
            // A namespace declaration is not retained for its own sake. The loop
            // below re-declares exactly the prefixes these attributes use, and the
            // alias machinery declares the rest on the elements that use them, so
            // recording a declaration here would emit it twice.
            if namespace_declaration(name) {
                continue;
            }
            let value = value.into_owned();
            let namespace = root_attribute_namespace(name, bindings)?;
            let local = name.rsplit(|byte| *byte == b':').next().unwrap_or(name);
            for prefix in root_attribute_value_prefixes(namespace, local, &value) {
                if prefix == "xml" || !bindings.iter().any(|(bound, _)| *bound == prefix) {
                    continue;
                }
                if !used_prefixes
                    .iter()
                    .any(|candidate| candidate == prefix.as_bytes())
                {
                    used_prefixes.push(prefix.as_bytes().to_vec());
                }
            }
            if !expanded.insert((namespace, local.to_vec())) {
                return Err(OxmlError::InvalidValue(format!(
                    "duplicate expanded root attribute `{}`",
                    String::from_utf8_lossy(local)
                )));
            }
            if let Some(separator) = name.iter().position(|byte| *byte == b':')
                && let prefix = &name[..separator]
                && prefix != b"xml"
                && !used_prefixes.iter().any(|candidate| candidate == prefix)
            {
                used_prefixes.push(prefix.to_vec());
            }
            attributes.push((std::str::from_utf8(name)?.to_owned(), value));
        }
        Ok(Self {
            attributes,
            used_prefixes,
        })
    }
    fn emit_in(
        self,
        bindings: &[(&str, &str)],
        emission: Option<&FieldXmlEmission>,
    ) -> Result<Option<Vec<u8>>> {
        if self.attributes.is_empty() {
            return Ok(None);
        }
        let output = if let Some(emission) = emission {
            let (output, _outer) = emission.new_source_record_output();
            let mut writer = Writer::new(output);
            self.write_record(&mut writer, bindings, Some(emission))?;
            writer.into_inner().into_mapped_parts().0
        } else {
            let mut writer = Writer::new(Vec::new());
            self.write_record(&mut writer, bindings, None)?;
            writer.into_inner()
        };
        Ok(Some(output))
    }

    fn write_record<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        bindings: &[(&str, &str)],
        emission: Option<&FieldXmlEmission>,
    ) -> Result<()> {
        let mut record =
            FieldXmlStart::new(std::str::from_utf8(ROOT_ATTRIBUTES_ELEMENT)?, emission)?;
        for (name, value) in &self.attributes {
            record.push(name, value)?;
        }
        for prefix in &self.used_prefixes {
            let namespace = bindings
                .iter()
                .find(|(candidate, _)| candidate.as_bytes() == prefix)
                .map(|(_, namespace)| *namespace)
                .ok_or_else(|| {
                    OxmlError::InvalidValue(format!(
                        "root attribute prefix `{}` is unbound",
                        String::from_utf8_lossy(prefix)
                    ))
                })?;
            if prefix.is_empty() {
                record.push("xmlns", namespace)?;
            } else {
                record.push_word("xmlns", std::str::from_utf8(prefix)?, namespace)?;
            }
        }
        record.write(writer, true)
    }
}

pub(crate) fn capture_root_attribute_record(
    start: &BytesStart<'_>,
    prefixes: &[String],
) -> Result<Option<Vec<u8>>> {
    capture_root_attribute_record_in(start, prefixes, None)
}

fn capture_root_attribute_record_in(
    start: &BytesStart<'_>,
    prefixes: &[String],
    emission: Option<&FieldXmlEmission>,
) -> Result<Option<Vec<u8>>> {
    let mut bindings: Vec<(&str, &str)> = prefixes
        .iter()
        .filter_map(|value| value.strip_prefix('\0')?.split_once('\0'))
        .collect();
    // Explicit bindings win over the conventional bare Word aliases.
    for prefix in prefixes {
        if !prefix.starts_with('\0') && !bindings.iter().any(|(bound, _)| *bound == prefix) {
            bindings.push((prefix, crate::namespace::W_NS));
        }
    }
    RootAttributePlan::read(start, &bindings)?.emit_in(&bindings, emission)
}

/// Preserve an actual Word alias declaration on a paragraph root that keeps a
/// local foreign `w` binding. The alias is the physical declaration the source
/// used for its own Word names, so a Source emission can keep both facts.
pub(super) fn retain_foreign_word_alias_declaration(
    record: Vec<u8>,
    root: &BytesStart<'_>,
    word_prefixes: &[String],
) -> Result<Vec<u8>> {
    let declarations = raw_namespace_declarations(root)?;
    if !declarations
        .iter()
        .any(|(prefix, namespace)| prefix == "w" && namespace != crate::namespace::W_NS)
    {
        return Ok(record);
    }
    let Some(word_prefix) = word_prefixes
        .iter()
        .find(|prefix| !prefix.is_empty() && prefix.as_str() != "w" && !prefix.starts_with('\0'))
    else {
        return Ok(record);
    };
    let declaration = format!("xmlns:{word_prefix}");
    let mut reader = Reader::from_reader(record.as_slice());
    let Event::Empty(element) = reader.read_event()? else {
        return Ok(record);
    };
    if element.attributes().any(|attribute| {
        attribute.is_ok_and(|attribute| attribute.key.as_ref() == declaration.as_bytes())
    }) {
        return Ok(record);
    }
    let mut updated = BytesStart::new(std::str::from_utf8(ROOT_ATTRIBUTES_ELEMENT)?);
    for attribute in element.attributes() {
        updated.push_attribute(attribute?);
    }
    updated.push_attribute((declaration.as_str(), crate::namespace::W_NS));
    let mut writer = Writer::new(Vec::new());
    writer.write_event(Event::Empty(updated))?;
    Ok(writer.into_inner())
}

#[doc(hidden)]
pub fn is_root_attribute_record(raw: &[u8]) -> bool {
    raw.starts_with(b"<rdocxRootAttributes")
}

pub(crate) fn push_root_attribute_record(
    target: &mut BytesStart<'_>,
    raw: &[u8],
    replaced: Option<(&str, &str)>,
    role: XmlWriteRole,
) -> Result<()> {
    push_root_attribute_record_in(target, raw, replaced, role, None)
}

#[derive(Clone)]
enum RootMergeAttribute<'a> {
    Encoded(quick_xml::events::attributes::Attribute<'a>),
    Decoded { name: &'a [u8], value: &'a str },
}
impl RootMergeAttribute<'_> {
    fn name(&self) -> &[u8] {
        match self {
            Self::Encoded(attribute) => attribute.key.as_ref(),
            Self::Decoded { name, .. } => name,
        }
    }
    fn value_is(&self, ascii_uri: &[u8]) -> bool {
        // Callers compare only canonical URI constants with no escaping.
        match self {
            Self::Encoded(attribute) => attribute.value.as_ref() == ascii_uri,
            Self::Decoded { value, .. } => value.as_bytes() == ascii_uri,
        }
    }
    fn decoded(&self, decoder: quick_xml::encoding::Decoder) -> Result<Cow<'_, str>> {
        match self {
            Self::Encoded(attribute) => {
                Ok(attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)?)
            }
            Self::Decoded { value, .. } => {
                // The removed metadata writer emitted referenced whitespace as
                // literal whitespace. Publication decoded that canonical record
                // once more, while Source copied it verbatim. Use the same
                // normalizer with only literal whitespace selected: decoded
                // ampersands are plain text, not new entity references.
                Ok(quick_xml::escape::normalize_attribute_value(
                    value,
                    1,
                    |byte| matches!(*byte, b'\t' | b'\r' | b'\n'),
                    |output, text, index, replacement| {
                        output.push(replacement);
                        index
                            + if text.as_bytes()[index] == b'\r'
                                && text.as_bytes().get(index + 1) == Some(&b'\n')
                            {
                                2
                            } else {
                                1
                            }
                    },
                    |_| None,
                )
                .map_err(quick_xml::Error::from)?)
            }
        }
    }
    fn push_source(
        &self,
        target: &mut BytesStart<'_>,
        emission: Option<&FieldXmlEmission>,
    ) -> Result<()> {
        let value_len = match self {
            Self::Encoded(attribute) => attribute.value.len(),
            Self::Decoded { value, .. } => escaped_source_len(value)?,
        };
        let length = self
            .name()
            .len()
            .checked_add(4)
            .and_then(|n| n.checked_add(value_len))
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalized retained root attribute length overflow".into())
            })?;
        if let Some(emission) = emission {
            emission.reserve_inner_xml(length)?;
        }
        match self {
            Self::Encoded(attribute) => target.push_attribute(attribute.clone()),
            Self::Decoded { name, value } => {
                target.push_attribute((std::str::from_utf8(name)?, *value))
            }
        }
        Ok(())
    }
}

enum RootMergeInput<'a> {
    Record {
        element: &'a BytesStart<'a>,
        resolver: &'a quick_xml::name::NamespaceResolver,
        raw: &'a [u8],
    },
    Plan {
        attributes: Vec<RootMergeAttribute<'a>>,
        resolver: quick_xml::name::NamespaceResolver,
        uses_w14: bool,
        decoder: quick_xml::encoding::Decoder,
    },
}

enum RootMergeIter<'a> {
    Record(quick_xml::events::attributes::Attributes<'a>),
    Plan(std::slice::Iter<'a, RootMergeAttribute<'a>>),
}
impl<'a> Iterator for RootMergeIter<'a> {
    type Item = Result<RootMergeAttribute<'a>>;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Record(attributes) => attributes.next().map(|attribute| {
                attribute
                    .map(RootMergeAttribute::Encoded)
                    .map_err(OxmlError::from)
            }),
            Self::Plan(attributes) => attributes.next().cloned().map(Ok),
        }
    }
}
impl RootMergeInput<'_> {
    fn attributes(&self) -> RootMergeIter<'_> {
        match self {
            Self::Record { element, .. } => RootMergeIter::Record(element.attributes()),
            Self::Plan { attributes, .. } => RootMergeIter::Plan(attributes.iter()),
        }
    }
    fn resolver(&self) -> &quick_xml::name::NamespaceResolver {
        match self {
            Self::Record { resolver, .. } => resolver,
            Self::Plan { resolver, .. } => resolver,
        }
    }
    fn decoder(&self) -> quick_xml::encoding::Decoder {
        match self {
            Self::Record { element, .. } => element.decoder(),
            Self::Plan { decoder, .. } => *decoder,
        }
    }
    fn uses_w14(&self) -> bool {
        match self {
            Self::Record { raw, .. } => uses_w14_qualified_name(raw),
            Self::Plan { uses_w14, .. } => *uses_w14,
        }
    }
}

impl RootAttributePlan {
    pub(super) fn merge_run_root(
        &self,
        target: &mut FieldXmlStart<'_, '_>,
        original: &BytesStart<'_>,
        prefixes: &[String],
        role: XmlWriteRole,
    ) -> Result<()> {
        let mut attributes = self
            .attributes
            .iter()
            .map(|(name, value)| RootMergeAttribute::Decoded {
                name: name.as_bytes(),
                value,
            })
            .collect::<Vec<_>>();
        let mut resolver = quick_xml::name::NamespaceResolver::default();
        for (count, prefix) in self.used_prefixes.iter().enumerate() {
            // These are precisely the declaration attributes that the old
            // canonical metadata writer appended, in the same order.
            let namespace = prefixes
                .iter()
                .filter_map(|value| value.strip_prefix('\0')?.split_once('\0'))
                .find(|(bound, _)| bound.as_bytes() == prefix)
                .map(|(_, uri)| uri)
                .ok_or_else(|| {
                    OxmlError::InvalidValue(format!(
                        "root attribute prefix `{}` is unbound",
                        String::from_utf8_lossy(prefix)
                    ))
                })?;
            let name = original
                .attributes()
                .find_map(|attribute| {
                    let attribute = attribute.ok()?;
                    (attribute.key.as_namespace_binding()
                        == Some(if prefix.is_empty() {
                            quick_xml::name::PrefixDeclaration::Default
                        } else {
                            quick_xml::name::PrefixDeclaration::Named(prefix)
                        }))
                    .then_some(attribute.key.0)
                })
                .ok_or_else(|| {
                    OxmlError::InvalidValue("missing retained root namespace declaration".into())
                })?;
            attributes.push(RootMergeAttribute::Decoded {
                name,
                value: namespace,
            });
            if count >= resolver.max_declarations_per_element() {
                return Err(quick_xml::Error::Namespace(
                    quick_xml::name::NamespaceError::TooManyDeclarations(
                        resolver.max_declarations_per_element(),
                    ),
                )
                .into());
            }
            // The resolver's namespace facts retain the exact escaped spelling
            // it previously read from the synthetic record, not semantic URI
            // equality. Fence any newly escaped URI allocation before growth.
            let escaped_len = escaped_source_len(namespace)?;
            if escaped_len != namespace.len()
                && let Some(emission) = target.emission
            {
                emission.reserve_inner_xml(escaped_len)?;
            }
            let encoded = quick_xml::escape::escape(namespace);
            resolver
                .add(
                    if prefix.is_empty() {
                        quick_xml::name::PrefixDeclaration::Default
                    } else {
                        quick_xml::name::PrefixDeclaration::Named(prefix)
                    },
                    Namespace(encoded.as_bytes()),
                )
                .map_err(quick_xml::Error::from)?;
        }
        // Evaluate the old lexical detector over the exact canonical pieces.
        // No complete metadata header is serialized or reparsed.
        let mut window = [0u8; 5];
        let mut seen = 0usize;
        let mut uses_w14 = false;
        let mut visit = |bytes: &[u8]| {
            for &byte in bytes {
                window.rotate_left(1);
                window[4] = byte;
                seen = seen.saturating_add(1);
                uses_w14 |= seen >= 5
                    && matches!(window[0], b'<' | b'/' | b' ' | b'\t' | b'\r' | b'\n')
                    && &window[1..] == b"w14:";
            }
        };
        visit(b"<rdocxRootAttributes");
        for attribute in &attributes {
            let RootMergeAttribute::Decoded { name, value } = attribute else {
                unreachable!("plan attribute")
            };
            visit(b" ");
            visit(name);
            visit(b"=\"");
            for &byte in value.as_bytes() {
                visit(escaped_source_piece(byte).unwrap_or(std::slice::from_ref(&byte)));
            }
            visit(b"\"");
        }
        visit(b"/>");
        let input = RootMergeInput::Plan {
            attributes,
            resolver,
            uses_w14,
            decoder: original.decoder(),
        };
        merge_root_attributes(&mut target.start, &input, None, role, target.emission)?;
        target.reserved = target.start.as_ref().len();
        Ok(())
    }
}

pub(super) fn push_root_attribute_record_in(
    target: &mut BytesStart<'_>,
    raw: &[u8],
    replaced: Option<(&str, &str)>,
    role: XmlWriteRole,
    emission: Option<&FieldXmlEmission>,
) -> Result<()> {
    let mut reader = NsReader::from_reader(raw);
    let mut buffer = Vec::new();
    let element = loop {
        match reader.read_resolved_event_into(&mut buffer)? {
            (_, Event::Empty(element)) if element.name().as_ref() == ROOT_ATTRIBUTES_ELEMENT => {
                break element.into_owned();
            }
            (_, Event::Eof) => {
                return Err(OxmlError::InvalidValue(
                    "invalid retained root-attribute record".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    };
    merge_root_attributes(
        target,
        &RootMergeInput::Record {
            element: &element,
            resolver: reader.resolver(),
            raw,
        },
        replaced,
        role,
        emission,
    )
}

fn merge_root_attributes(
    target: &mut BytesStart<'_>,
    input: &RootMergeInput<'_>,
    replaced: Option<(&str, &str)>,
    role: XmlWriteRole,
    emission: Option<&FieldXmlEmission>,
) -> Result<()> {
    let existing = target
        .attributes()
        .filter_map(|attribute| attribute.ok())
        .map(|attribute| attribute.key.as_ref().to_vec())
        .collect::<HashSet<_>>();
    if role == XmlWriteRole::Source {
        // The immutable consumer supplies source meaning. Publication's
        // producer-prefix rewriting must never change these source QNames.
        for attribute in input.attributes() {
            let attribute = attribute?;
            if !existing.contains(attribute.name()) {
                attribute.push_source(target, emission)?;
            }
        }
        return Ok(());
    }
    // The generated owner and its modeled descendants use canonical w.
    // A retained producer binding must keep its expanded attribute names
    // without rebinding that generated owner.
    let is_run = target.name().as_ref() == b"w:r";
    let mut foreign_bindings = Vec::new();
    let mut occupied_prefixes = HashSet::<String>::new();
    for attribute in target
        .attributes()
        .map(|attribute| {
            attribute
                .map(RootMergeAttribute::Encoded)
                .map_err(OxmlError::from)
        })
        .chain(input.attributes())
    {
        let attribute = attribute?;
        let name = std::str::from_utf8(attribute.name())?;
        if let Some(prefix) = name.strip_prefix("xmlns:") {
            occupied_prefixes.insert(prefix.to_owned());
        } else if let Some((prefix, _)) = name.split_once(':') {
            occupied_prefixes.insert(prefix.to_owned());
        }
        let generated_uri = match name {
            "xmlns:w" => Some(crate::namespace::W_NS),
            "xmlns:wp"
                if is_run
                    && target.attributes().any(|attribute| {
                        attribute.is_ok_and(|attribute| {
                            attribute.key.as_ref() == b"xmlns:wp"
                                && attribute.value.as_ref() == WP_NS.as_bytes()
                        })
                    }) =>
            {
                Some(WP_NS)
            }
            "xmlns:r"
                if is_run
                    && target.attributes().any(|attribute| {
                        attribute.is_ok_and(|attribute| {
                            attribute.key.as_ref() == b"xmlns:r"
                                && attribute.value.as_ref() == R_NS.as_bytes()
                        })
                    }) =>
            {
                Some(R_NS)
            }
            _ => None,
        };
        if let Some(generated) = generated_uri {
            let value = attribute.decoded(input.decoder())?;
            if value != generated {
                let prefix = name.strip_prefix("xmlns:").unwrap_or_default();
                if !foreign_bindings
                    .iter()
                    .any(|(existing, _)| existing == prefix)
                {
                    foreign_bindings.push((prefix.to_owned(), value.into_owned()));
                }
            }
        }
    }

    let mut value_prefixes = HashSet::<String>::new();
    {
        for attribute in input.attributes() {
            let attribute = attribute?;
            let (namespace, local) = input
                .resolver()
                .resolve_attribute(quick_xml::name::QName(attribute.name()));
            let namespace = match namespace {
                ResolveResult::Bound(Namespace(uri)) => Some(std::str::from_utf8(uri)?),
                _ => None,
            };
            let value = attribute.decoded(input.decoder())?;
            for prefix in root_attribute_value_prefixes(namespace, local.as_ref(), &value) {
                value_prefixes.insert(prefix.to_owned());
                if foreign_bindings
                    .iter()
                    .any(|(foreign, _)| foreign == prefix)
                {
                    return Err(OxmlError::InvalidValue(format!(
                        "element XML namespace-valued root attribute referencing foreign {prefix} is unsupported"
                    )));
                }
                if !prefix.is_empty() && prefix != "xml" && !input.resolver().bindings().any(|(binding, _)| matches!(binding, quick_xml::name::PrefixDeclaration::Named(bound) if bound == prefix.as_bytes())) {
                    return Err(OxmlError::InvalidValue("element XML namespace-valued root attribute has an unbound prefix".into()));
                }
            }
        }
    }
    let mut producer_prefixes = Vec::new();
    for (foreign, namespace) in foreign_bindings {
        let mut suffix = 0usize;
        let prefix = loop {
            let prefix = if suffix == 0 {
                "rdocxProducer".to_owned()
            } else {
                format!("rdocxProducer{suffix}")
            };
            if occupied_prefixes.insert(prefix.clone()) {
                break prefix;
            }
            suffix = suffix.checked_add(1).ok_or_else(|| {
                OxmlError::InvalidValue("retained producer namespace prefix space exhausted".into())
            })?;
        };
        if let Some(emission) = emission {
            let escaped = escaped_source_len(&namespace)?;
            let length = 6usize
                .checked_add(prefix.len())
                .and_then(|n| n.checked_add(4))
                .and_then(|n| n.checked_add(escaped))
                .ok_or_else(|| {
                    OxmlError::InvalidValue("normalized namespace attribute length overflow".into())
                })?;
            emission.reserve_inner_xml(length)?;
        }
        target.push_attribute((format!("xmlns:{prefix}").as_str(), namespace.as_str()));
        producer_prefixes.push((foreign, prefix));
    }
    for attribute in input.attributes() {
        let attribute = attribute?;
        let name = attribute.name();
        let lexical_name = std::str::from_utf8(name)?;
        if lexical_name.strip_prefix("xmlns:").is_some_and(|prefix| {
            producer_prefixes
                .iter()
                .any(|(foreign, _)| foreign == prefix)
        }) {
            continue;
        }
        let producer = lexical_name.split_once(':').and_then(|(prefix, local)| {
            producer_prefixes
                .iter()
                .find(|(foreign, _)| foreign == prefix)
                .map(|(_, alias)| (alias, local))
        });
        if existing.contains(name) && producer.is_none() {
            continue;
        }
        // `w14` is bound by the part root that owns the element, the same
        // assumption the authored `w14:paraId` write already makes. Rebinding
        // it here would make a reopened save differ from the save it came
        // from, purely by a declaration that changes nothing. `w` is in scope
        // already, since the element's own `w:` name needs it, and a writer
        // that shadows it declares it on the element first. Word writes
        // `w:rsid*` on nearly every paragraph and run, so repeating `w` here
        // repeated it hundreds of times in an edited part.
        if (name == b"xmlns:w14"
            && attribute.value_is(W14_NS.as_bytes())
            && !value_prefixes.contains("w14")
            && emitted_bindings().guarantees_w14_name()
            && (input.uses_w14() || replaced.is_some_and(|(namespace, _)| namespace == W14_NS)))
            || (name == b"xmlns:w" && attribute.value_is(crate::namespace::W_NS.as_bytes()))
        {
            continue;
        }
        let root_bindings = ROOT_BINDINGS.with(Cell::get);
        if (root_bindings & ROOT_R_BINDING != 0
            && name == b"xmlns:r"
            && attribute.value_is(R_NS.as_bytes()))
            || (root_bindings & ROOT_MC_BINDING != 0
                && name == b"xmlns:mc"
                && attribute.value_is(MC_NS.as_bytes()))
            || (root_bindings & ROOT_WP_BINDING != 0
                && name == b"xmlns:wp"
                && attribute.value_is(WP_NS.as_bytes()))
        {
            continue;
        }
        if !namespace_declaration(name)
            && let Some((namespace, local)) = replaced
        {
            let (resolved, resolved_local) = input
                .resolver()
                .resolve_attribute(quick_xml::name::QName(attribute.name()));
            if resolved_local.as_ref() == local.as_bytes()
                && matches!(resolved, ResolveResult::Bound(Namespace(uri)) if uri == namespace.as_bytes())
            {
                continue;
            }
        }
        let name = std::str::from_utf8(name)?;
        let value = attribute.decoded(input.decoder())?;
        if let Some(emission) = emission {
            let key = if let Some((prefix, local)) = producer {
                prefix
                    .len()
                    .checked_add(1)
                    .and_then(|n| n.checked_add(local.len()))
            } else {
                Some(name.len())
            };
            let escaped = escaped_source_len(&value)?;
            let length = key
                .and_then(|n| n.checked_add(4))
                .and_then(|n| n.checked_add(escaped))
                .ok_or_else(|| {
                    OxmlError::InvalidValue("normalized root attribute length overflow".into())
                })?;
            emission.reserve_inner_xml(length)?;
        }
        if let Some((prefix, local)) = producer {
            target.push_attribute((format!("{prefix}:{local}").as_str(), value.as_ref()));
        } else {
            target.push_attribute((name, value.as_ref()));
        }
    }
    Ok(())
}

/// Remove `w14:paraId` and `w14:textId` from the retained start-tag
/// attributes of a paragraph or table row, given its `extra_xml`.
///
/// A copy must not share them with its source, and Word assigns new ones to
/// an element that has none. Every other retained attribute stays, with the
/// declarations its prefix needs.
#[doc(hidden)]
pub fn drop_w14_paragraph_identities(extra_xml: &mut Vec<(usize, Vec<u8>)>) -> Result<()> {
    let mut index = 0;
    while index < extra_xml.len() {
        let (position, raw) = &extra_xml[index];
        if *position == ROOT_ATTRIBUTES_POSITION && is_root_attribute_record(raw) {
            match root_attribute_record_without_w14_identities(raw)? {
                Some(record) => extra_xml[index].1 = record,
                None => {
                    extra_xml.remove(index);
                    continue;
                }
            }
        }
        index += 1;
    }
    Ok(())
}

/// Return the record without its w14 identities, or `None` when nothing else
/// is left in it.
fn root_attribute_record_without_w14_identities(raw: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut reader = NsReader::from_reader(raw);
    let mut buffer = Vec::new();
    let element = loop {
        match reader.read_resolved_event_into(&mut buffer)? {
            (_, Event::Empty(element)) if element.name().as_ref() == ROOT_ATTRIBUTES_ELEMENT => {
                break element.into_owned();
            }
            (_, Event::Eof) => {
                return Err(OxmlError::InvalidValue(
                    "invalid retained root-attribute record".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    };
    let mut dropped = false;
    let mut attributes = Vec::new();
    let mut declarations = Vec::new();
    for attribute in element.attributes() {
        let attribute = attribute?;
        let name = std::str::from_utf8(attribute.key.as_ref())?.to_owned();
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?
            .into_owned();
        if namespace_declaration(name.as_bytes()) {
            declarations.push((name, value));
            continue;
        }
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W14_NS.as_bytes())
            && matches!(local.as_ref(), b"paraId" | b"textId")
        {
            dropped = true;
            continue;
        }
        attributes.push((name, value));
    }
    if !dropped {
        return Ok(Some(raw.to_vec()));
    }
    if attributes.is_empty() {
        return Ok(None);
    }
    let mut record = BytesStart::new(std::str::from_utf8(ROOT_ATTRIBUTES_ELEMENT)?);
    for (name, value) in &attributes {
        record.push_attribute((name.as_str(), value.as_str()));
    }
    // The record declares exactly the prefixes its attributes use, so a
    // declaration only the dropped identities used goes with them.
    for (name, value) in &declarations {
        let prefix = name.strip_prefix("xmlns:").unwrap_or_default();
        if attributes.iter().any(|(attribute, _)| {
            attribute
                .split_once(':')
                .is_some_and(|(used, _)| used == prefix)
        }) {
            record.push_attribute((name.as_str(), value.as_str()));
        }
    }
    let mut writer = Writer::new(Vec::new());
    writer.write_event(Event::Empty(record))?;
    Ok(Some(writer.into_inner()))
}

/// Declare the canonical `w14` prefix on the root of a serialized part whose
/// content uses the prefix while the root does not bind it.
///
/// A retained root-attribute record writes `w14:paraId` and `w14:textId`
/// without their declaration, because Word and python-docx declare `w14` on
/// the part root. A root oxoffice wrote does not, and a producer may declare the
/// prefix on the element that uses it, so the part declares it here once. A
/// root that binds `w14` itself is left as it is.
#[doc(hidden)]
pub fn declare_w14_on_part_root(xml: &mut Vec<u8>) -> Result<()> {
    declare_w14_on_part_root_with_insertion(xml).map(|_| ())
}

pub(crate) fn declare_w14_on_part_root_with_insertion(
    xml: &mut Vec<u8>,
) -> Result<Option<(usize, usize)>> {
    let Some((at, declaration)) = plan_w14_on_part_root(xml)? else {
        return Ok(None);
    };
    let length = declaration.len();
    xml.splice(at..at, declaration.into_bytes());
    Ok(Some((at, length)))
}

// This is the same declaration decision for ordinary and scoped producers.
// Borrowed events avoid copying the root while selecting its insertion point.
pub(crate) fn plan_w14_on_part_root(xml: &[u8]) -> Result<Option<(usize, String)>> {
    let mut reader = Reader::from_reader(xml);
    let root_end = loop {
        match reader.read_event()? {
            Event::Start(root) => {
                for attribute in root.attributes() {
                    if attribute?.key.as_ref() == b"xmlns:w14" {
                        return Ok(None);
                    }
                }
                let origin =
                    crate::document::reader_input_origin(xml, &reader).ok_or_else(|| {
                        OxmlError::InvalidValue("w14 root reader origin is invalid".into())
                    })?;
                break (reader.buffer_position() as usize)
                    .checked_add(origin)
                    .ok_or_else(|| OxmlError::InvalidValue("w14 root position overflow".into()))?;
            }
            Event::Empty(_) | Event::Eof => return Ok(None),
            _ => {}
        }
    };
    // Preserve the existing conservative raw-name selector, including text
    // which happens to match. Only the actual physical origin is corrected.
    Ok(uses_w14_qualified_name(&xml[root_end..])
        .then(|| (root_end - 1, format!(r#" xmlns:w14="{W14_NS}""#))))
}

fn uses_w14_qualified_name(xml: &[u8]) -> bool {
    xml.windows(5).any(|window| {
        matches!(window[0], b'<' | b'/' | b' ' | b'\t' | b'\r' | b'\n') && &window[1..] == b"w14:"
    })
}

pub(crate) fn write_raw_with_word_override_in<W: std::io::Write>(
    writer: &mut Writer<W>,
    raw: &[u8],
    foreign_word_namespace: Option<&str>,
    emission: Option<&FieldXmlEmission>,
) -> Result<()> {
    let Some(namespace) = foreign_word_namespace else {
        writer.get_mut().write_all(raw)?;
        return Ok(());
    };
    if !raw_uses_external_word_binding(raw) {
        writer.get_mut().write_all(raw)?;
        return Ok(());
    }
    let (produced, prepaid) = raw_with_root_word_binding_in(raw, namespace, emission)?;
    let _transport = emission
        .filter(|_| prepaid)
        .map(|emission| emission.prepaid_xml_transport(&produced))
        .transpose()?;
    writer.get_mut().write_all(&produced)?;
    Ok(())
}

// Only declarations the writer actually retains on this physical root can
// replace a synthetic child binding. Namespace-only roots retain no record.
pub(crate) fn bindings_outside_retained_root(
    bindings: &[(String, String)],
    root: Option<&BytesStart<'_>>,
    word_prefixes: &[String],
) -> Result<Vec<(String, String)>> {
    let record = root
        .map(|root| capture_root_attribute_record(root, word_prefixes))
        .transpose()?
        .flatten();
    let declared = match record {
        Some(record) => {
            let mut reader = Reader::from_reader(record.as_slice());
            match reader.read_event()? {
                Event::Empty(root) => raw_namespace_declarations(&root)?,
                _ => {
                    return Err(OxmlError::InvalidValue(
                        "retained root binding record is malformed".into(),
                    ));
                }
            }
        }
        None => Vec::new(),
    };
    Ok(bindings
        .iter()
        .filter(|binding| {
            !declared.contains(binding) || (binding.0 == "w" && binding.1 != crate::namespace::W_NS)
        })
        .cloned()
        .collect())
}

pub(crate) fn field_scope_bindings(word_prefixes: &[String]) -> Vec<(&str, &str)> {
    let mut source = Vec::<(&str, &str)>::new();
    for entry in word_prefixes {
        if let Some((prefix, namespace)) = entry
            .strip_prefix('\0')
            .and_then(|entry| entry.split_once('\0'))
        {
            source.push((prefix, namespace));
        }
    }
    for prefix in word_prefixes
        .iter()
        .filter(|entry| !entry.starts_with('\0'))
    {
        if !source.iter().any(|(known, _)| known == prefix) {
            source.push((prefix, crate::namespace::W_NS));
        }
    }
    source
}

// The existing field current-value envelope, also used by retained raw owners.
pub(super) fn affected_source_dynamic_work(bytes: usize) -> Option<usize> {
    bytes.checked_mul(crate::namespace::W_NS.len() + 64)
}

pub(crate) fn retained_owner_work(
    raw_bytes: usize,
    scope: &[String],
    current_bytes: usize,
) -> Result<usize> {
    checked_field_scope_bytes(scope)?
        .checked_add(current_bytes)
        .and_then(affected_source_dynamic_work)
        .and_then(|work| work.checked_add(raw_bytes))
        .ok_or_else(|| {
            OxmlError::InvalidValue("retained owner affected-source work overflow".into())
        })
}

pub(crate) fn retained_binding_work(
    raw_bytes: usize,
    bindings: &[(String, String)],
    current_bytes: usize,
) -> Result<usize> {
    bindings
        .iter()
        .try_fold(current_bytes, |total, (prefix, namespace)| {
            total
                .checked_add(prefix.len())
                .and_then(|total| total.checked_add(namespace.len()))
        })
        .and_then(affected_source_dynamic_work)
        .and_then(|work| work.checked_add(raw_bytes))
        .ok_or_else(|| {
            OxmlError::InvalidValue("retained binding affected-source work overflow".into())
        })
}

/// The same affected-source arithmetic over already-decoded borrowed scope
/// bytes. Callers that pool frame facts once use this instead of cloning
/// every prefix and URI into an owned binding vector.
pub(crate) fn retained_source_envelope(scope_bytes: usize, raw_bytes: usize) -> Result<usize> {
    affected_source_dynamic_work(scope_bytes)
        .and_then(|work| work.checked_add(raw_bytes))
        .ok_or_else(|| OxmlError::InvalidValue("retained source envelope overflow".into()))
}

// Publication is distinct from inherited-context source patches. Every raw
// sibling root is closed from this exact field owner's immutable capture.

pub(super) fn published_field_roots_in<'a>(
    raw: &'a [u8],
    field: &Field,
    publication_scope: &[(&str, &str)],
    emission: Option<&FieldXmlEmission>,
) -> Result<(Cow<'a, [u8]>, bool)> {
    match published_field_root_plan(raw, field, publication_scope)? {
        Some(plan) => emit_retained_roots_in(raw, plan, emission),
        None => Ok((Cow::Borrowed(raw), false)),
    }
}

pub(super) fn published_field_root_plan<'a>(
    raw: &[u8],
    field: &'a Field,
    publication_scope: &[(&str, &str)],
) -> Result<Option<RetainedRootPlan<'a>>> {
    crate::document::admit_word_model(crate::document::WordOwner::Field(field))?;
    let budget = field.normalization_work_budget()?;
    let FieldSource::Parsed { word_prefixes, .. } = field.source.as_ref() else {
        return Ok(None);
    };
    let snapshot = emitted_bindings();
    let mut destination = snapshot.bindings_for(raw);
    for binding in publication_scope {
        destination.retain(|(prefix, _)| prefix != &binding.0);
        destination.push(*binding);
    }
    let plan = retained_root_plan(raw, word_prefixes, &destination, budget)?;
    Ok(Some(plan))
}

pub(crate) fn retained_raw_roots<'a>(
    raw: &'a [u8],
    prefixes: &[String],
    destination: &[(&str, &str)],
    budget: usize,
) -> Result<Cow<'a, [u8]>> {
    let plan = retained_root_plan(raw, prefixes, destination, budget)?;
    emit_retained_roots(raw, plan)
}

pub(crate) struct RetainedRootPlan<'a> {
    source: Vec<(&'a str, &'a str)>,
    roots: Vec<(usize, Vec<usize>)>,
    projected: usize,
}

impl RetainedRootPlan<'_> {
    pub(crate) fn added_bytes(&self, raw: &[u8]) -> usize {
        self.projected - raw.len()
    }
}

pub(crate) fn retained_root_plan<'a>(
    raw: &[u8],
    prefixes: &'a [String],
    destination: &[(&str, &str)],
    budget: usize,
) -> Result<RetainedRootPlan<'a>> {
    retained_binding_plan(raw, &field_scope_bindings(prefixes), destination, budget)
}

pub(crate) fn retained_binding_plan<'a>(
    raw: &[u8],
    bindings: &[(&'a str, &'a str)],
    destination: &[(&str, &str)],
    budget: usize,
) -> Result<RetainedRootPlan<'a>> {
    let mut source = bindings.to_vec();
    // Absence of a default declaration is the original empty namespace, not
    // permission to inherit a newly introduced publication default.
    if !source.iter().any(|(prefix, _)| prefix.is_empty()) {
        source.push(("", ""));
    }
    if source.is_empty() {
        return Ok(RetainedRootPlan {
            source,
            roots: Vec::new(),
            projected: raw.len(),
        });
    }
    let escaped_len = |value: &str| -> Result<usize> {
        escaped_source_len(value)
            .map_err(|_| OxmlError::InvalidValue("field namespace escaped length overflow".into()))
    };
    let lengths = source
        .iter()
        .map(|(prefix, namespace)| {
            // space + xmlns[:prefix] + =" + escaped URI + "
            9usize
                .checked_add(if prefix.is_empty() {
                    0
                } else {
                    prefix.len().checked_add(1).ok_or_else(|| {
                        OxmlError::InvalidValue("field namespace prefix length overflow".into())
                    })?
                })
                .and_then(|size| size.checked_add(escaped_len(namespace).ok()?))
                .ok_or_else(|| {
                    OxmlError::InvalidValue("field namespace declaration length overflow".into())
                })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut projected = raw.len();
    if projected > budget {
        return Err(OxmlError::InvalidValue(
            "field namespace publication exceeds normalization work budget".into(),
        ));
    }
    let mut roots = Vec::new();
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader.read_event_into(&mut buffer)?;
        let (open_end, end, empty) = match event {
            Event::Start(root) => {
                let open_end = reader.buffer_position() as usize;
                reader.read_to_end_into(root.name(), &mut Vec::new())?;
                (open_end, reader.buffer_position() as usize, false)
            }
            Event::Empty(_) => (
                reader.buffer_position() as usize,
                reader.buffer_position() as usize,
                true,
            ),
            Event::Eof => break,
            _ => {
                buffer.clear();
                continue;
            }
        };
        // Namespace-valued attributes must be recognized in their COMPLETE
        // original frame. A matching current mc binding still supplies the
        // namespace that gives Ignorable/PreserveElements their QName semantics.
        let mut required = external_binding_indices_used_by_raw(&raw[start..end], &source)?;
        required.retain(|index| {
            let binding = source[*index];
            !destination.contains(&binding)
        });
        for index in &required {
            projected = projected
                .checked_add(lengths[*index])
                .filter(|size| *size <= budget)
                .ok_or_else(|| {
                    OxmlError::InvalidValue(
                        "field namespace publication exceeds normalization work budget".into(),
                    )
                })?;
        }
        if !required.is_empty() {
            roots.push((open_end - if empty { 2 } else { 1 }, required));
        }
        buffer.clear();
    }
    Ok(RetainedRootPlan {
        source,
        roots,
        projected,
    })
}

pub(crate) fn generated_subtree_end(reader: &mut Reader<&[u8]>, name: &[u8]) -> Result<usize> {
    let mut depth = 1u32;
    loop {
        match reader.read_event()? {
            Event::Start(start) if start.name().as_ref() == name => depth += 1,
            Event::End(end) if end.name().as_ref() == name => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(reader.buffer_position() as usize)
}

pub(crate) fn emit_retained_roots<'a>(
    raw: &'a [u8],
    plan: RetainedRootPlan<'_>,
) -> Result<Cow<'a, [u8]>> {
    Ok(emit_retained_roots_in(raw, plan, None)?.0)
}

pub(crate) fn emit_retained_roots_in<'a>(
    raw: &'a [u8],
    plan: RetainedRootPlan<'_>,
    emission: Option<&FieldXmlEmission>,
) -> Result<(Cow<'a, [u8]>, bool)> {
    let (bytes, prepaid, _) = emit_retained_roots_mapped_in(raw, plan, emission, None)?;
    Ok((bytes, prepaid))
}

pub(crate) fn emit_retained_roots_mapped_in<'a>(
    raw: &'a [u8],
    plan: RetainedRootPlan<'_>,
    emission: Option<&FieldXmlEmission>,
    source_map: Option<&crate::document::WordSourceMap>,
) -> Result<(Cow<'a, [u8]>, bool, Option<crate::document::WordSourceMap>)> {
    let RetainedRootPlan {
        source,
        roots,
        projected,
    } = plan;
    if let Some(map) = source_map
        && map.len() != raw.len()
    {
        return Err(OxmlError::InvalidValue(
            "retained root Word map length mismatch".into(),
        ));
    }
    if roots.is_empty() {
        return Ok((
            Cow::Borrowed(raw),
            false,
            source_map
                .cloned()
                .map(crate::document::WordSourceMap::word_only),
        ));
    }
    let prepaid = emission
        .map(|emission| emission.reserve_inner_xml(projected))
        .transpose()?
        .unwrap_or(false);
    // No closed root, escaped URI clone or expanded buffer exists before the
    // complete borrowed proof above. Emission is a single bounded buffer pass.
    let plan = RetainedRootPlan {
        source,
        roots,
        projected,
    };
    let mut output = Vec::with_capacity(projected);
    let mut mapped = source_map.map(|_| crate::document::WordSourceMap::generated(0));
    let mut raw_cursor = 0usize;
    visit_retained_root_pieces(raw, &plan, |piece| {
        let before = output.len();
        match piece {
            RetainedRootPiece::Transport(bytes) => {
                output.extend_from_slice(bytes);
                if let Some(map) = &mut mapped {
                    let end = raw_cursor.checked_add(bytes.len()).ok_or_else(|| {
                        OxmlError::InvalidValue("retained Word source position overflow".into())
                    })?;
                    // Insertions are in parsed opening headers. Payload facts
                    // are made only after a real w:t opening event, so none
                    // straddles these header boundaries.
                    map.extend(
                        &source_map
                            .expect("selected retained Word map")
                            .slice(raw_cursor..end)?
                            .word_only(),
                    )?;
                    raw_cursor = end;
                }
            }
            RetainedRootPiece::Generated(bytes) => {
                output.extend_from_slice(bytes);
                if let Some(map) = &mut mapped {
                    map.extend(&crate::document::WordSourceMap::generated(bytes.len()))?;
                }
            }
            RetainedRootPiece::Escaped(value) => {
                append_escaped_source(&mut output, value);
                if let Some(map) = &mut mapped {
                    map.extend(&crate::document::WordSourceMap::generated(
                        output.len() - before,
                    ))?;
                }
            }
        }
        Ok(())
    })?;
    debug_assert_eq!(output.len(), projected);
    Ok((Cow::Owned(output), prepaid, mapped))
}

enum RetainedRootPiece<'a> {
    Transport(&'a [u8]),
    Generated(&'a [u8]),
    Escaped(&'a str),
}

// The existing owned record emitter and the concrete final validation writer
// are both actual consumers of this one selected-root emission order.
fn visit_retained_root_pieces(
    raw: &[u8],
    plan: &RetainedRootPlan<'_>,
    mut emit: impl FnMut(RetainedRootPiece<'_>) -> Result<()>,
) -> Result<()> {
    let mut cursor = 0;
    for (at, required) in &plan.roots {
        emit(RetainedRootPiece::Transport(&raw[cursor..*at]))?;
        for &index in required {
            let prefix = plan.source[index].0;
            emit(RetainedRootPiece::Generated(b" xmlns"))?;
            if !prefix.is_empty() {
                emit(RetainedRootPiece::Generated(b":"))?;
                emit(RetainedRootPiece::Generated(prefix.as_bytes()))?;
            }
            emit(RetainedRootPiece::Generated(b"=\""))?;
            emit(RetainedRootPiece::Escaped(plan.source[index].1))?;
            emit(RetainedRootPiece::Generated(b"\""))?;
        }
        cursor = *at;
    }
    emit(RetainedRootPiece::Transport(&raw[cursor..]))
}

pub(super) fn write_retained_roots_final_transport<W: std::io::Write>(
    writer: &mut Writer<W>,
    raw: &[u8],
    plan: &RetainedRootPlan<'_>,
    emission: &FieldXmlEmission,
    word_map: Option<(&WordOperationWork, &crate::document::WordSourceMap)>,
) -> Result<()> {
    // This caller is the exact live preservation writer. The immutable raw
    // record already exists. Only newly emitted closure bytes are generated,
    // and no separate rewritten whole-record buffer is materialized.
    let mut raw_cursor = 0usize;
    visit_retained_root_pieces(raw, plan, |piece| {
        match piece {
            RetainedRootPiece::Transport(bytes) => {
                let end = raw_cursor.checked_add(bytes.len()).ok_or_else(|| {
                    OxmlError::InvalidValue("retained Word source position overflow".into())
                })?;
                let _mapped = word_map
                    .map(|(work, map)| {
                        work.mapped_source_record(bytes, map.slice(raw_cursor..end)?.word_only())
                    })
                    .transpose()?;
                writer.get_mut().write_all(bytes)?;
                raw_cursor = end;
            }
            RetainedRootPiece::Generated(bytes) => {
                emission.reserve_inner_xml(bytes.len())?;
                writer.get_mut().write_all(bytes)?;
            }
            RetainedRootPiece::Escaped(value) => {
                emission.reserve_inner_xml(escaped_source_len(value)?)?;
                for &byte in value.as_bytes() {
                    writer.get_mut().write_all(
                        escaped_source_piece(byte).unwrap_or(std::slice::from_ref(&byte)),
                    )?;
                }
            }
        }
        Ok(())
    })
}

pub(crate) fn raw_with_external_bindings(
    raw: &[u8],
    external_bindings: &[(String, String)],
) -> Result<Vec<u8>> {
    raw_with_external_bindings_in(raw, external_bindings, None)
}

pub(crate) fn raw_with_external_bindings_in(
    raw: &[u8],
    external_bindings: &[(String, String)],
    emission: Option<&FieldXmlEmission>,
) -> Result<Vec<u8>> {
    Ok(raw_with_external_bindings_and_map(raw, external_bindings, emission, None)?.0)
}

pub(crate) fn raw_with_external_bindings_and_map(
    raw: &[u8],
    external_bindings: &[(String, String)],
    emission: Option<&FieldXmlEmission>,
    source_map: Option<Arc<crate::document::WordSourceMap>>,
) -> Result<(Vec<u8>, Option<Arc<crate::document::WordSourceMap>>)> {
    let bindings = external_bindings_used_by_raw(raw, external_bindings)?;
    if bindings.is_empty() {
        if let Some(emission) = emission {
            emission.reserve_inner_xml(raw.len())?;
        }
        return Ok((raw.to_vec(), source_map));
    }
    raw_with_root_bindings_and_map_in(raw, &bindings, false, emission, source_map)
}

pub(super) fn external_bindings_used_by_raw(
    raw: &[u8],
    external_bindings: &[(String, String)],
) -> Result<Vec<(String, String)>> {
    let borrowed = external_bindings
        .iter()
        .map(|(prefix, namespace)| (prefix.as_str(), namespace.as_str()))
        .collect::<Vec<_>>();
    Ok(external_binding_indices_used_by_raw(raw, &borrowed)?
        .into_iter()
        .map(|index| external_bindings[index].clone())
        .collect())
}

pub(super) fn external_binding_indices_used_by_raw(
    raw: &[u8],
    external_bindings: &[(&str, &str)],
) -> Result<Vec<usize>> {
    if external_bindings.is_empty() {
        return Ok(Vec::new());
    }
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut scopes = Vec::<Vec<(String, String)>>::new();
    let mut required = Vec::<usize>::new();
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) => {
                let declarations = raw_namespace_declarations(&element)?;
                record_external_bindings_used(
                    &element,
                    &scopes,
                    &declarations,
                    external_bindings,
                    &mut required,
                )?;
                scopes.push(declarations);
            }
            Event::Empty(element) => {
                let declarations = raw_namespace_declarations(&element)?;
                record_external_bindings_used(
                    &element,
                    &scopes,
                    &declarations,
                    external_bindings,
                    &mut required,
                )?;
            }
            Event::End(_) => {
                scopes.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    required.sort_unstable();
    Ok(required)
}

pub(crate) fn raw_namespace_declarations(
    element: &BytesStart<'_>,
) -> Result<Vec<(String, String)>> {
    let mut declarations = Vec::new();
    for attribute in element.attributes() {
        let attribute = attribute?;
        let name = attribute.key.as_ref();
        let prefix = if name == b"xmlns" {
            ""
        } else if let Some(prefix) = name.strip_prefix(b"xmlns:") {
            std::str::from_utf8(prefix)?
        } else {
            continue;
        };
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?
            .into_owned();
        declarations.push((prefix.to_owned(), value));
    }
    Ok(declarations)
}

fn record_external_bindings_used(
    element: &BytesStart<'_>,
    scopes: &[Vec<(String, String)>],
    declarations: &[(String, String)],
    external_bindings: &[(&str, &str)],
    required: &mut Vec<usize>,
) -> Result<()> {
    let element_name = element.name();
    let element_prefix = qualified_name_prefix(element_name.as_ref()).unwrap_or("");
    record_external_prefix_used(
        element_prefix,
        scopes,
        declarations,
        external_bindings,
        required,
    );
    for attribute in element.attributes().filter_map(|attribute| attribute.ok()) {
        let name = attribute.key.as_ref();
        if name == b"xmlns" || name.starts_with(b"xmlns:") {
            continue;
        }
        if let Some(prefix) = qualified_name_prefix(name) {
            record_external_prefix_used(prefix, scopes, declarations, external_bindings, required);
            let namespace = declarations
                .iter()
                .rev()
                .chain(scopes.iter().rev().flat_map(|scope| scope.iter().rev()))
                .find(|(candidate, _)| candidate == prefix)
                .map(|(_, namespace)| namespace.as_str())
                .or_else(|| {
                    external_bindings
                        .iter()
                        .find(|(candidate, _)| *candidate == prefix)
                        .map(|(_, namespace)| *namespace)
                });
            let local = name.rsplit(|byte| *byte == b':').next().unwrap_or(name);
            let namespace_value = namespace == Some(crate::namespace::MC_NS)
                && matches!(
                    local,
                    b"Ignorable"
                        | b"MustUnderstand"
                        | b"ProcessContent"
                        | b"PreserveAttributes"
                        | b"PreserveElements"
                )
                || namespace == Some("http://www.w3.org/2001/XMLSchema-instance")
                    && local == b"type";
            if !namespace_value {
                continue;
            }
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?;
            for prefix in root_attribute_value_prefixes(namespace, local, &value) {
                record_external_prefix_used(
                    prefix,
                    scopes,
                    declarations,
                    external_bindings,
                    required,
                );
            }
        }
    }
    Ok(())
}

fn record_external_prefix_used(
    prefix: &str,
    scopes: &[Vec<(String, String)>],
    declarations: &[(String, String)],
    external_bindings: &[(&str, &str)],
    required: &mut Vec<usize>,
) {
    let internally_bound = declarations
        .iter()
        .rev()
        .any(|(candidate, _)| candidate == prefix)
        || scopes
            .iter()
            .rev()
            .any(|scope| scope.iter().rev().any(|(candidate, _)| candidate == prefix));
    if !internally_bound
        && let Some(index) = external_bindings
            .iter()
            .position(|(candidate, _)| *candidate == prefix)
        && !required.contains(&index)
    {
        required.push(index);
    }
}

pub(super) fn qualified_name_prefix(name: &[u8]) -> Option<&str> {
    let separator = name.iter().position(|byte| *byte == b':')?;
    std::str::from_utf8(&name[..separator]).ok()
}

pub(super) fn raw_with_root_bindings_in(
    raw: &[u8],
    bindings: &[(String, String)],
    source_values: bool,
    emission: Option<&FieldXmlEmission>,
) -> Result<Vec<u8>> {
    Ok(raw_with_root_bindings_and_map_in(raw, bindings, source_values, emission, None)?.0)
}

fn raw_with_root_bindings_and_map_in(
    raw: &[u8],
    bindings: &[(String, String)],
    source_values: bool,
    emission: Option<&FieldXmlEmission>,
    source_map: Option<Arc<crate::document::WordSourceMap>>,
) -> Result<(Vec<u8>, Option<Arc<crate::document::WordSourceMap>>)> {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader.read_event()?;
        let empty = matches!(event, Event::Empty(_));
        let element = match event {
            Event::Start(element) | Event::Empty(element) => element,
            Event::Eof => {
                if let Some(emission) = emission {
                    emission.reserve_inner_xml(raw.len())?;
                }
                return Ok((raw.to_vec(), source_map));
            }
            _ => continue,
        };
        let end = reader.buffer_position() as usize;
        // Exactly the payload and delimiters emitted by quick-xml's existing
        // Start/Empty writer, with the same lexical or semantic value policy.
        let mut length = start
            .checked_add(element.as_ref().len())
            .and_then(|n| n.checked_add(if empty { 3 } else { 2 }))
            .and_then(|n| n.checked_add(raw.len() - end))
            .ok_or_else(|| OxmlError::InvalidValue("raw namespace output overflow".into()))?;
        for (prefix, namespace) in bindings {
            let key = if prefix.is_empty() {
                5
            } else {
                6usize
                    .checked_add(prefix.len())
                    .ok_or_else(|| OxmlError::InvalidValue("raw namespace QName overflow".into()))?
            };
            let value = if source_values {
                namespace.len()
            } else {
                escaped_source_len(namespace)?
            };
            length = length
                .checked_add(key)
                .and_then(|n| n.checked_add(4))
                .and_then(|n| n.checked_add(value))
                .ok_or_else(|| OxmlError::InvalidValue("raw namespace output overflow".into()))?;
        }
        if let Some(emission) = emission {
            emission.reserve_inner_xml(length)?;
        }
        let mut output = Vec::with_capacity(length);
        output.extend_from_slice(&raw[..start]);
        output.push(b'<');
        output.extend_from_slice(element.as_ref());
        for (prefix, namespace) in bindings {
            output.extend_from_slice(b" xmlns");
            if !prefix.is_empty() {
                output.push(b':');
                output.extend_from_slice(prefix.as_bytes());
            }
            output.extend_from_slice(b"=\"");
            if source_values {
                output.extend_from_slice(namespace.as_bytes());
            } else {
                append_escaped_source(&mut output, namespace);
            }
            output.push(b'"');
        }
        output.extend_from_slice(if empty { b"/>" } else { b">" });
        output.extend_from_slice(&raw[end..]);
        debug_assert_eq!(output.len(), length);
        let mapped = source_map
            .map(|map| -> Result<_> {
                // These are the actual copied prefix/payload and suffix ranges of
                // this producer. Namespace insertions have no incoming authority.
                let payload_end = start + 1 + element.as_ref().len();
                let delimiter = if empty {
                    b"/>".as_slice()
                } else {
                    b">".as_slice()
                };
                let delimiter_start = end - delimiter.len();
                let mut joined = crate::document::WordSourceMap::generated(0);
                joined.extend(&map.slice(0..start)?)?;
                if raw.get(start..start + 1) == Some(b"<")
                    && raw.get(start + 1..payload_end) == Some(element.as_ref())
                    && payload_end == delimiter_start
                    && raw.get(delimiter_start..end) == Some(delimiter)
                {
                    joined.extend(&map.slice(start..payload_end)?)?;
                    joined.extend(&crate::document::WordSourceMap::generated(
                        length - raw.len(),
                    ))?;
                    joined.extend(&map.slice(delimiter_start..raw.len())?)?;
                } else {
                    // A canonicalized opening remains Generated. The untouched
                    // suffix still comes from this exact consumed source range.
                    joined.extend(&crate::document::WordSourceMap::generated(
                        length - start - (raw.len() - end),
                    ))?;
                    joined.extend(&map.slice(end..raw.len())?)?;
                }
                Ok(Arc::new(joined))
            })
            .transpose()?;
        return Ok((output, mapped));
    }
}

fn raw_uses_external_word_binding(raw: &[u8]) -> bool {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut external_binding = vec![true];
    let mut saw_root = false;
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) => {
                let (declares_word, uses_word) = raw_word_binding_use(&element);
                if !saw_root {
                    saw_root = true;
                    if declares_word {
                        return false;
                    }
                }
                let uses_external =
                    external_binding.last().copied().unwrap_or(true) && !declares_word;
                if uses_external && uses_word {
                    return true;
                }
                external_binding.push(uses_external);
            }
            Ok(Event::Empty(element)) => {
                let (declares_word, uses_word) = raw_word_binding_use(&element);
                if !saw_root {
                    saw_root = true;
                    if declares_word {
                        return false;
                    }
                }
                let uses_external =
                    external_binding.last().copied().unwrap_or(true) && !declares_word;
                if uses_external && uses_word {
                    return true;
                }
            }
            Ok(Event::End(_)) => {
                if external_binding.len() > 1 {
                    external_binding.pop();
                }
            }
            Ok(Event::Eof) => return false,
            Err(_) => return false,
            _ => {}
        }
        buffer.clear();
    }
}

fn raw_word_binding_use(element: &BytesStart<'_>) -> (bool, bool) {
    let declares_word = element
        .attributes()
        .filter_map(|attribute| attribute.ok())
        .any(|attribute| attribute.key.as_ref() == b"xmlns:w");
    let uses_word = qualified_name_uses_prefix(element.name().as_ref(), b"w")
        || element
            .attributes()
            .filter_map(|attribute| attribute.ok())
            .any(|attribute| qualified_name_uses_prefix(attribute.key.as_ref(), b"w"));
    (declares_word, uses_word)
}

fn qualified_name_uses_prefix(name: &[u8], prefix: &[u8]) -> bool {
    name.iter()
        .position(|byte| *byte == b':')
        .is_some_and(|separator| &name[..separator] == prefix)
}

fn raw_with_root_word_binding_in<'raw>(
    raw: &'raw [u8],
    namespace: &str,
    emission: Option<&FieldXmlEmission>,
) -> Result<(Cow<'raw, [u8]>, bool)> {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    loop {
        let event = reader.read_event()?;
        let close = match event {
            Event::Start(_) => 1,
            Event::Empty(_) => 2,
            Event::Eof => return Ok((Cow::Borrowed(raw), false)),
            _ => continue,
        };
        let at = reader.buffer_position() as usize - close;
        let escaped = escaped_source_len(namespace)?;
        let length = raw
            .len()
            .checked_add(11)
            .and_then(|n| n.checked_add(escaped))
            .ok_or_else(|| OxmlError::InvalidValue("raw Word binding output overflow".into()))?;
        let prepaid = emission
            .map(|emission| emission.reserve_inner_xml(length))
            .transpose()?
            .unwrap_or(false);
        let mut output = Vec::with_capacity(length);
        output.extend_from_slice(&raw[..at]);
        output.extend_from_slice(b" xmlns:w=\"");
        append_escaped_source(&mut output, namespace);
        output.push(b'"');
        output.extend_from_slice(&raw[at..]);
        debug_assert_eq!(length, output.len());
        return Ok((Cow::Owned(output), prepaid));
    }
}
