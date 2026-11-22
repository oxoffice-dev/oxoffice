//! Field XML emission: records, source records and the start-tag writer every model writer charges its output to.

use super::*;

// Source rewrite algorithms need both their existing ordinary Vec mode and
// inherited record mode, including their real len/splice operations.
pub(super) enum FieldXmlRecord {
    Ordinary(Vec<u8>),
    Inherited {
        output: NormalizedWordOutput,
        _outer: PrepaidXmlTransport<'static>,
    },
}

impl FieldXmlRecord {
    pub(super) fn new(emission: &FieldXmlEmission) -> Self {
        if emission.enabled {
            let (output, outer) = emission.new_source_record_output();
            Self::Inherited {
                output,
                _outer: outer,
            }
        } else {
            Self::Ordinary(Vec::new())
        }
    }

    pub(super) fn replace_generated(&mut self, bytes: Vec<u8>) {
        match self {
            Self::Ordinary(output) => *output = bytes,
            Self::Inherited { output, .. } => {
                output.source_map = crate::document::WordSourceMap::generated(bytes.len());
                output.bytes = bytes;
            }
        }
        // Keep this record's existing outer scope alive. Replacing an owned
        // buffer is not another scope entry, debit, input grant or receipt.
    }

    pub(super) fn len(&self) -> usize {
        match self {
            Self::Ordinary(bytes) => bytes.len(),
            Self::Inherited { output, .. } => output.bytes.len(),
        }
    }

    pub(super) fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Ordinary(bytes) => bytes,
            Self::Inherited { output, .. } => &output.bytes,
        }
    }

    pub(super) fn splice(&mut self, range: std::ops::Range<usize>, bytes: &[u8]) -> Result<()> {
        let length = self.len();
        if range.start > range.end || range.end > length {
            return Err(OxmlError::InvalidValue(
                "field XML record splice boundary".into(),
            ));
        }
        let projected = (length - range.len())
            .checked_add(bytes.len())
            .ok_or_else(|| OxmlError::InvalidValue("field XML record splice overflow".into()))?;
        match self {
            Self::Ordinary(output) => {
                output.splice(range, bytes.iter().copied());
            }
            Self::Inherited { output, .. } => {
                let map = crate::document::WordSourceMap::concat(&[
                    output.source_map.slice(0..range.start)?,
                    crate::document::WordSourceMap::generated(bytes.len()),
                    output.source_map.slice(range.end..length)?,
                ])?;
                if output.local_limit.is_some_and(|limit| projected > limit) {
                    output
                        .work
                        .lock()
                        .expect("normalization ledger poisoned")
                        .refused = true;
                    return Err(std::io::Error::other(
                        "normalized result exceeds its preflight source work",
                    )
                    .into());
                }
                let emission = FieldXmlEmission {
                    enabled: true,
                    output: Arc::clone(&output.work),
                    word_capture: output.word_capture.clone(),
                };
                emission.reserve_inner_xml(bytes.len())?;
                output.bytes.splice(range, bytes.iter().copied());
                output.source_map = map;
                // New inserted copies pay before growth. Surviving bytes are
                // not charged again, removed mappings disappear, and spent
                // work or input authority is never refunded or renewed.
            }
        }
        Ok(())
    }

    pub(super) fn into_word_mapped_parts(
        self,
    ) -> (Vec<u8>, Option<crate::document::WordSourceMap>) {
        match self {
            Self::Ordinary(bytes) => (bytes, None),
            Self::Inherited { output, _outer } => {
                let (bytes, map) = output.into_mapped_parts();
                (bytes, Some(map.word_only()))
            }
        }
    }

    pub(super) fn into_bytes(self) -> Vec<u8> {
        match self {
            Self::Ordinary(bytes) => bytes,
            Self::Inherited { output, _outer } => output.into_mapped_parts().0,
        }
    }
}

impl std::io::Write for FieldXmlRecord {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Ordinary(output) => output.write(bytes),
            Self::Inherited { output, .. } => output.write(bytes),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Ordinary(output) => output.flush(),
            Self::Inherited { output, .. } => output.flush(),
        }
    }
}

impl<'a> FieldXmlSourceRecord<'a> {
    pub(super) fn record(
        &mut self,
        range: std::ops::Range<usize>,
        pieces: Vec<FieldSourcePiece<'a>>,
    ) -> Result<()> {
        if field_source_pieces_equal(
            &pieces,
            self.original
                .get(range.clone())
                .ok_or_else(|| OxmlError::InvalidValue("simple source event bounds".into()))?,
        ) {
            return Ok(());
        }
        if self
            .edits
            .last()
            .is_some_and(|previous| previous.range.end > range.start)
        {
            return Err(OxmlError::InvalidValue(
                "simple source event edits overlap".into(),
            ));
        }
        self.edits
            .push(FieldSourceEdit::new(self.original.len(), range, pieces)?);
        Ok(())
    }

    pub(super) fn finish(
        self,
        emission: &FieldXmlEmission,
        word_map: Option<(
            &crate::document::WordSourceMap,
            &mut Option<crate::document::WordSourceMap>,
        )>,
    ) -> Result<Cow<'a, [u8]>> {
        if self.edits.is_empty() {
            if let Some((input, output)) = word_map {
                *output = Some(input.clone().word_only());
            }
            return Ok(Cow::Borrowed(self.original));
        }
        let mut mapped = word_map
            .as_ref()
            .map(|_| crate::document::WordSourceMap::generated(0));
        let mut pieces = Vec::new();
        let mut at = 0;
        for edit in self.edits {
            pieces.push(FieldSourcePiece::Raw(&self.original[at..edit.range.start]));
            if let Some(map) = &mut mapped {
                let input = word_map.as_ref().expect("selected source recipe map").0;
                map.extend(&input.slice(at..edit.range.start)?.word_only())?;
                map.extend(&crate::document::WordSourceMap::generated(
                    edit.replacement_bytes,
                ))?;
            }
            at = edit.range.end;
            pieces.extend(edit.replacement);
        }
        pieces.push(FieldSourcePiece::Raw(&self.original[at..]));
        if let Some(map) = &mut mapped {
            map.extend(
                &word_map
                    .as_ref()
                    .expect("selected source recipe map")
                    .0
                    .slice(at..self.original.len())?
                    .word_only(),
            )?;
        }
        let plan = FieldSourceEdit::new(self.original.len(), 0..self.original.len(), pieces)?;
        emission.reserve_inner_xml(plan.projected)?;
        let bytes = plan.emit(self.original)?;
        if let Some((_, output)) = word_map {
            *output = mapped;
        }
        Ok(Cow::Owned(bytes))
    }
}

fn field_source_pieces_equal(pieces: &[FieldSourcePiece<'_>], raw: &[u8]) -> bool {
    let mut at = 0usize;
    let mut matches = |bytes: &[u8]| {
        let Some(end) = at.checked_add(bytes.len()) else {
            return false;
        };
        if raw.get(at..end) != Some(bytes) {
            return false;
        }
        at = end;
        true
    };
    for piece in pieces {
        match piece {
            FieldSourcePiece::Raw(bytes) | FieldSourcePiece::Literal(bytes) => {
                if !matches(bytes) {
                    return false;
                }
            }
            FieldSourcePiece::Escaped(value) => {
                for byte in value.bytes() {
                    if !matches(escaped_source_piece(byte).unwrap_or(std::slice::from_ref(&byte))) {
                        return false;
                    }
                }
            }
        }
    }
    at == raw.len()
}

// This borrows the actual selected namespace token from its existing live
// scope or declaration. It grants no original range or input-work authority.
pub(super) fn field_source_prefix<'a>(
    prefix: &str,
    raw: &'a [u8],
    root: &BytesStart<'_>,
    inherited: &'a [String],
) -> Result<&'a [u8]> {
    let name = field_attribute_source(raw, root.name().as_ref())?;
    if let Some(colon) = name.iter().position(|byte| *byte == b':')
        && &name[..colon] == prefix.as_bytes()
    {
        return Ok(&name[..colon]);
    }
    for attribute in root.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        if key.strip_prefix(b"xmlns:") == Some(prefix.as_bytes()) {
            return Ok(&field_attribute_source(raw, key)?[6..]);
        }
    }
    inherited
        .iter()
        .find(|known| known.as_str() == prefix)
        .map(|known| known.as_bytes())
        .ok_or_else(|| OxmlError::InvalidValue("selected field prefix lost its scope".into()))
}

impl<'a, 'name> FieldXmlStart<'a, 'name> {
    pub(crate) fn new(name: &'name str, emission: Option<&'a FieldXmlEmission>) -> Result<Self> {
        let prepaid = emission
            .map(|value| value.reserve_inner_xml(name.len()))
            .transpose()?
            .unwrap_or(false);
        Ok(Self {
            start: BytesStart::new(name),
            emission,
            prepaid,
            reserved: name.len(),
        })
    }

    pub(crate) fn word(
        prefix: &str,
        local: &str,
        emission: Option<&'a FieldXmlEmission>,
    ) -> Result<Self> {
        let reserved = prefix
            .len()
            .checked_add(1)
            .and_then(|n| n.checked_add(local.len()))
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalized event QName length overflow".into())
            })?;
        let prepaid = emission
            .map(|value| value.reserve_inner_xml(reserved))
            .transpose()?
            .unwrap_or(false);
        Ok(Self {
            start: BytesStart::new(format!("{prefix}:{local}")),
            emission,
            prepaid,
            reserved,
        })
    }

    pub(super) fn planned_word(
        plan: GeneratedWordName<'_>,
        local: &str,
        emission: Option<&'a FieldXmlEmission>,
    ) -> Result<(Self, String)> {
        let length = plan
            .prefix_len()
            .checked_add(1)
            .and_then(|n| n.checked_add(local.len()))
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalized event QName length overflow".into())
            })?;
        let prepaid = emission
            .map(|value| value.reserve_inner_xml(length))
            .transpose()?
            .unwrap_or(false);
        let prefix = plan.render();
        Ok((
            Self {
                start: BytesStart::new(format!("{prefix}:{local}")),
                emission,
                prepaid,
                reserved: length,
            },
            prefix,
        ))
    }

    pub(super) fn push_planned_word(
        &mut self,
        plan: GeneratedWordName<'_>,
        local: &str,
        value: &str,
    ) -> Result<()> {
        let escaped = escaped_source_len(value)?;
        let length = plan
            .prefix_len()
            .checked_add(1)
            .and_then(|n| n.checked_add(local.len()))
            .and_then(|n| n.checked_add(4))
            .and_then(|n| n.checked_add(escaped))
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalized planned attribute length overflow".into())
            })?;
        if let Some(emission) = self.emission {
            emission.reserve_inner_xml(length)?;
        }
        self.reserved = self.reserved.checked_add(length).ok_or_else(|| {
            OxmlError::InvalidValue("normalized event payload length overflow".into())
        })?;
        let prefix = plan.render();
        self.start
            .push_attribute((format!("{prefix}:{local}").as_str(), value));
        Ok(())
    }

    pub(super) fn push_planned_namespace(
        &mut self,
        plan: GeneratedWordName<'_>,
        namespace: &str,
    ) -> Result<()> {
        let escaped = escaped_source_len(namespace)?;
        let length = 6usize
            .checked_add(plan.prefix_len())
            .and_then(|n| n.checked_add(4))
            .and_then(|n| n.checked_add(escaped))
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalized planned namespace length overflow".into())
            })?;
        if let Some(emission) = self.emission {
            emission.reserve_inner_xml(length)?;
        }
        self.reserved = self.reserved.checked_add(length).ok_or_else(|| {
            OxmlError::InvalidValue("normalized event payload length overflow".into())
        })?;
        let prefix = plan.render();
        self.start
            .push_attribute((format!("xmlns:{prefix}").as_str(), namespace));
        Ok(())
    }

    pub(crate) fn push(&mut self, name: &str, value: &str) -> Result<()> {
        let length = name
            .len()
            .checked_add(4)
            .and_then(|n| {
                escaped_source_len(value)
                    .ok()
                    .and_then(|value| n.checked_add(value))
            })
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalized attribute event length overflow".into())
            })?;
        if let Some(emission) = self.emission {
            emission.reserve_inner_xml(length)?;
        }
        self.reserved = self.reserved.checked_add(length).ok_or_else(|| {
            OxmlError::InvalidValue("normalized event payload length overflow".into())
        })?;
        self.start.push_attribute((name, value));
        Ok(())
    }

    pub(crate) fn push_raw(&mut self, name: &str, value: &str) -> Result<()> {
        let value = oxoffice_core::xml::quote_raw_attribute_value(value);
        let length = name
            .len()
            .checked_add(4)
            .and_then(|n| n.checked_add(value.len()))
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalized retained attribute length overflow".into())
            })?;
        if let Some(emission) = self.emission {
            emission.reserve_inner_xml(length)?;
        }
        self.reserved = self.reserved.checked_add(length).ok_or_else(|| {
            OxmlError::InvalidValue("normalized event payload length overflow".into())
        })?;
        self.start
            .push_attribute(quick_xml::events::attributes::Attribute {
                key: quick_xml::name::QName(name.as_bytes()),
                value: Cow::Borrowed(value.as_bytes()),
            });
        Ok(())
    }

    /// Pushes an attribute kept as read, its value unresolved, fixing only
    /// the literal `"` a single-quoted source may hold.
    pub(super) fn push_raw_attribute(
        &mut self,
        mut attribute: quick_xml::events::attributes::Attribute<'_>,
    ) -> Result<()> {
        if attribute.value.contains(&b'"') {
            let raw = std::str::from_utf8(&attribute.value)?;
            attribute.value = Cow::Owned(
                oxoffice_core::xml::quote_raw_attribute_value(raw)
                    .into_owned()
                    .into_bytes(),
            );
        }
        let length = attribute
            .key
            .as_ref()
            .len()
            .checked_add(4)
            .and_then(|n| n.checked_add(attribute.value.len()))
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalized retained attribute length overflow".into())
            })?;
        if let Some(emission) = self.emission {
            emission.reserve_inner_xml(length)?;
        }
        self.reserved = self.reserved.checked_add(length).ok_or_else(|| {
            OxmlError::InvalidValue("normalized event payload length overflow".into())
        })?;
        self.start.push_attribute(attribute);
        Ok(())
    }

    pub(crate) fn push_raw_word(&mut self, prefix: &str, local: &str, value: &str) -> Result<()> {
        let value = oxoffice_core::xml::quote_raw_attribute_value(value);
        let key = prefix
            .len()
            .checked_add(1)
            .and_then(|n| n.checked_add(local.len()))
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalized retained QName length overflow".into())
            })?;
        let length = key
            .checked_add(4)
            .and_then(|n| n.checked_add(value.len()))
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalized retained attribute length overflow".into())
            })?;
        if let Some(emission) = self.emission {
            emission.reserve_inner_xml(length)?;
        }
        self.reserved = self.reserved.checked_add(length).ok_or_else(|| {
            OxmlError::InvalidValue("normalized event payload length overflow".into())
        })?;
        let name = format!("{prefix}:{local}");
        self.start
            .push_attribute(quick_xml::events::attributes::Attribute {
                key: quick_xml::name::QName(name.as_bytes()),
                value: Cow::Borrowed(value.as_bytes()),
            });
        Ok(())
    }

    pub(crate) fn push_word(&mut self, prefix: &str, local: &str, value: &str) -> Result<()> {
        let key = prefix
            .len()
            .checked_add(1)
            .and_then(|n| n.checked_add(local.len()))
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalized attribute QName length overflow".into())
            })?;
        let escaped = escaped_source_len(value)?;
        let length = key
            .checked_add(4)
            .and_then(|n| n.checked_add(escaped))
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalized attribute event length overflow".into())
            })?;
        if let Some(emission) = self.emission {
            emission.reserve_inner_xml(length)?;
        }
        self.reserved = self.reserved.checked_add(length).ok_or_else(|| {
            OxmlError::InvalidValue("normalized event payload length overflow".into())
        })?;
        self.start
            .push_attribute((format!("{prefix}:{local}").as_str(), value));
        Ok(())
    }

    pub(crate) fn qualify(
        &mut self,
        original: &[String],
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<String> {
        let start = std::mem::replace(&mut self.start, BytesStart::new(""));
        let (start, prefix) =
            generated_word_start_in(start, original, role, source_context, self.emission)?;
        self.reserved = start.as_ref().len();
        self.start = start;
        Ok(prefix)
    }

    pub(crate) fn restore_word_binding(&mut self) -> Result<()> {
        if needs_generated_word_binding(&self.start)? {
            self.push("xmlns:w", crate::namespace::W_NS)?;
        }
        Ok(())
    }

    pub(crate) fn from_start(
        start: BytesStart<'name>,
        emission: Option<&'a FieldXmlEmission>,
    ) -> Result<Self> {
        let reserved = start.as_ref().len();
        let prepaid = emission
            .map(|value| value.reserve_inner_xml(reserved))
            .transpose()?
            .unwrap_or(false);
        Ok(Self {
            start,
            emission,
            prepaid,
            reserved,
        })
    }

    pub(crate) fn from_raw_spans(
        name: &str,
        attributes: &[&str],
        emission: Option<&'a FieldXmlEmission>,
    ) -> Result<Self> {
        let reserved = attributes
            .iter()
            .try_fold(name.len(), |n, part| n.checked_add(part.len()))
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalized lexical event length overflow".into())
            })?;
        let prepaid = emission
            .map(|value| value.reserve_inner_xml(reserved))
            .transpose()?
            .unwrap_or(false);
        let mut content = String::with_capacity(reserved);
        content.push_str(name);
        for part in attributes {
            content.push_str(part);
        }
        Ok(Self {
            start: BytesStart::from_content(content, name.len()),
            emission,
            prepaid,
            reserved,
        })
    }

    pub(crate) fn root_record(
        &mut self,
        raw: &[u8],
        replaced: Option<(&str, &str)>,
        role: XmlWriteRole,
    ) -> Result<()> {
        push_root_attribute_record_in(&mut self.start, raw, replaced, role, self.emission)?;
        self.reserved = self.start.as_ref().len();
        Ok(())
    }

    pub(crate) fn element(&self) -> &BytesStart<'name> {
        &self.start
    }

    pub(crate) fn write(self, writer: &mut Writer<impl std::io::Write>, empty: bool) -> Result<()> {
        debug_assert_eq!(self.reserved, self.start.as_ref().len());
        let _transport = self
            .emission
            .filter(|_| self.prepaid)
            .map(|emission| emission.prepaid_xml_transport(self.start.as_ref()))
            .transpose()?;
        writer.write_event(if empty {
            Event::Empty(self.start.borrow())
        } else {
            Event::Start(self.start.borrow())
        })?;
        Ok(())
    }
}

impl FieldXmlEmission {
    pub(crate) fn note_drawing_bindings(&self) -> bool {
        // The Native preparation visits direct modeled drawings, including
        // controls, but does not descend into a Field's cached result writer.
        let work = self.output.lock().expect("normalization ledger poisoned");
        let field = self.enabled || work.new_field || work.publication_field;
        drop(work);
        !field
            && self.word_capture.as_ref().is_some_and(|capture| {
                capture
                    .lock()
                    .expect("Word text capture poisoned")
                    .note_drawing_bindings
            })
    }

    /// One distinct actual source-record copy, admitted before its destination grows.
    pub(super) fn write_xml_pieces<W: std::io::Write>(
        &self,
        output: &mut W,
        pieces: &[FieldSourcePiece<'_>],
    ) -> Result<()> {
        let length = pieces.iter().try_fold(0usize, |sum, piece| {
            let length = match piece {
                FieldSourcePiece::Raw(bytes) | FieldSourcePiece::Literal(bytes) => bytes.len(),
                FieldSourcePiece::Escaped(value) => escaped_source_len(value)?,
            };
            sum.checked_add(length)
                .ok_or_else(|| OxmlError::InvalidValue("field XML recipe length overflow".into()))
        })?;
        let prepaid = self.reserve_inner_xml(length)?;
        let mut write = |bytes: &[u8]| -> Result<()> {
            if bytes.is_empty() {
                return Ok(());
            }
            // This scope covers only this actual synchronous chunk of the
            // recipe already reserved above. Nothing waits as a future receipt.
            let _transport = prepaid
                .then(|| self.prepaid_xml_transport(bytes))
                .transpose()?;
            output.write_all(bytes)?;
            Ok(())
        };
        for piece in pieces {
            match piece {
                FieldSourcePiece::Raw(bytes) | FieldSourcePiece::Literal(bytes) => write(bytes)?,
                FieldSourcePiece::Escaped(value) => {
                    let mut start = 0;
                    for (index, byte) in value.bytes().enumerate() {
                        if let Some(escaped) = escaped_source_piece(byte) {
                            write(&value.as_bytes()[start..index])?;
                            write(escaped)?;
                            start = index + 1;
                        }
                    }
                    write(&value.as_bytes()[start..])?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn write_copy(
        &self,
        writer: &mut Writer<impl std::io::Write>,
        bytes: &[u8],
    ) -> Result<()> {
        let prepaid = self.reserve_inner_xml(bytes.len())?;
        let _transport = prepaid
            .then(|| self.prepaid_xml_transport(bytes))
            .transpose()?;
        writer.get_mut().write_all(bytes)?;
        Ok(())
    }

    pub(crate) fn write_end_name(
        &self,
        writer: &mut Writer<impl std::io::Write>,
        name: &str,
    ) -> Result<()> {
        let prepaid = self.reserve_inner_xml(name.len())?;
        let _transport = prepaid
            .then(|| self.prepaid_xml_transport(name.as_bytes()))
            .transpose()?;
        writer.write_event(Event::End(BytesEnd::new(name)))?;
        Ok(())
    }

    pub(crate) fn new_source_record_output(
        &self,
    ) -> (NormalizedWordOutput, PrepaidXmlTransport<'static>) {
        let previous = self
            .output
            .lock()
            .expect("normalization ledger poisoned")
            .prepaid_xml
            .take();
        let outer = PrepaidXmlTransport {
            _produced: &[],
            previous,
            work: Arc::clone(&self.output),
        };
        (
            NormalizedWordOutput {
                bytes: Vec::new(),
                source_map: crate::document::WordSourceMap::generated(0),
                local_limit: None,
                field_only: false,
                new_source_record: true,
                final_writer: None,
                word_capture: self.word_capture.clone(),
                work: Arc::clone(&self.output),
            },
            outer,
        )
    }

    pub(crate) fn write_word_attributes(
        &self,
        writer: &mut Writer<impl std::io::Write>,
        plan: GeneratedWordName<'_>,
        local: &str,
        attributes: &[Option<(&str, &str)>],
        empty: bool,
    ) -> Result<String> {
        let overflow =
            || OxmlError::InvalidValue("normalized attribute event length overflow".into());
        let prefix_len = plan.prefix_len();
        let mut length = prefix_len
            .checked_add(1)
            .and_then(|n| n.checked_add(local.len()))
            .ok_or_else(overflow)?;
        if plan.declaration {
            length = length
                .checked_add(6)
                .and_then(|n| n.checked_add(prefix_len))
                .and_then(|n| n.checked_add(crate::namespace::W_NS.len()))
                .and_then(|n| n.checked_add(4))
                .ok_or_else(overflow)?;
        }
        for (name, value) in attributes.iter().flatten() {
            let value_len = escaped_source_len(value)?;
            length = length
                .checked_add(prefix_len)
                .and_then(|n| n.checked_add(1))
                .and_then(|n| n.checked_add(name.len()))
                .and_then(|n| n.checked_add(4))
                .and_then(|n| n.checked_add(value_len))
                .ok_or_else(overflow)?;
        }
        let prepaid = self.reserve_inner_xml(length)?;
        let prefix = plan.render();
        let mut element = BytesStart::new(format!("{prefix}:{local}"));
        if plan.declaration {
            element.push_attribute((format!("xmlns:{prefix}").as_str(), crate::namespace::W_NS));
        }
        for (name, value) in attributes.iter().flatten() {
            element.push_attribute((format!("{prefix}:{name}").as_str(), *value));
        }
        let _transport = prepaid
            .then(|| self.prepaid_xml_transport(element.as_ref()))
            .transpose()?;
        writer.write_event(if empty {
            Event::Empty(element.borrow())
        } else {
            Event::Start(element.borrow())
        })?;
        Ok(prefix)
    }

    pub(crate) fn write_word_end(
        &self,
        writer: &mut Writer<impl std::io::Write>,
        prefix: &str,
        local: &str,
    ) -> Result<()> {
        self.write_word_end_planned(writer, GeneratedWordName::fixed(prefix), local)
            .map(|_| ())
    }

    pub(crate) fn write_word_end_planned(
        &self,
        writer: &mut Writer<impl std::io::Write>,
        plan: GeneratedWordName<'_>,
        local: &str,
    ) -> Result<String> {
        let length = plan
            .prefix_len()
            .checked_add(1)
            .and_then(|n| n.checked_add(local.len()))
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalized end event length overflow".into())
            })?;
        let prepaid = self.reserve_inner_xml(length)?;
        let prefix = plan.render();
        let name = format!("{prefix}:{local}");
        let _transport = prepaid
            .then(|| self.prepaid_xml_transport(name.as_bytes()))
            .transpose()?;
        writer.write_event(Event::End(BytesEnd::new(name.as_str())))?;
        Ok(prefix)
    }

    pub(crate) fn write_word_value<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        prefix: &str,
        local: &str,
        value: &str,
    ) -> Result<()> {
        let overflow = || OxmlError::InvalidValue("normalized value event length overflow".into());
        let name = prefix
            .len()
            .checked_add(1)
            .and_then(|n| n.checked_add(local.len()))
            .ok_or_else(overflow)?;
        let key = prefix.len().checked_add(4).ok_or_else(overflow)?;
        let length = escaped_source_len(value)?
            .checked_add(name)
            .and_then(|n| n.checked_add(key))
            .and_then(|n| n.checked_add(4))
            .ok_or_else(overflow)?;
        let prepaid = self.reserve_inner_xml(length)?;
        let mut element = BytesStart::new(format!("{prefix}:{local}"));
        element.push_attribute((format!("{prefix}:val").as_str(), value));
        let _transport = prepaid
            .then(|| self.prepaid_xml_transport(element.as_ref()))
            .transpose()?;
        writer.write_event(Event::Empty(element.borrow()))?;
        Ok(())
    }

    pub(crate) fn reserve_inner_xml(&self, bytes: usize) -> Result<bool> {
        if !self.enabled {
            return Ok(false);
        }
        let mut output = self.output.lock().expect("normalization ledger poisoned");
        let Some(emitted) = output
            .emitted
            .checked_add(bytes)
            .filter(|n| *n <= output.allowance)
        else {
            output.refused = true;
            return Err(OxmlError::InvalidValue(
                "normalized result exceeds its preflight source work".into(),
            ));
        };
        output.emitted = emitted;
        Ok(true)
    }

    pub(crate) fn prepaid_xml_transport<'a>(
        &self,
        produced: &'a [u8],
    ) -> Result<PrepaidXmlTransport<'a>> {
        let start = produced.as_ptr() as usize;
        let end = start
            .checked_add(produced.len())
            .ok_or_else(|| OxmlError::InvalidValue("prepaid XML payload range overflow".into()))?;
        let mut output = self.output.lock().expect("normalization ledger poisoned");
        let previous = output
            .prepaid_xml
            .replace(RetainedRevisionRange { next: start, end });
        Ok(PrepaidXmlTransport {
            _produced: produced,
            previous,
            work: Arc::clone(&self.output),
        })
    }

    pub(crate) fn write_text<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        value: &str,
        word: Option<TemplateWordTextInput>,
    ) -> Result<()> {
        let length = escaped_source_len(value)?;
        if length == value.len() {
            let _word = word
                .map(|input| WordPayloadTransport::new(value.as_bytes(), input))
                .transpose()?;
            writer.write_event(Event::Text(BytesText::from_escaped(value)))?;
            return Ok(());
        }
        let prepaid = self.reserve_inner_xml(length)?;
        let mut output = Vec::with_capacity(length);
        append_escaped_source(&mut output, value);
        let text = String::from_utf8(output).expect("escaping preserves UTF-8");
        let event = BytesText::from_escaped(text);
        let _transport = prepaid
            .then(|| self.prepaid_xml_transport(event.as_ref()))
            .transpose()?;
        let _word = word
            .map(|input| WordPayloadTransport::new(event.as_ref(), input))
            .transpose()?;
        writer.write_event(Event::Text(event.borrow()))?;
        Ok(())
    }
}
