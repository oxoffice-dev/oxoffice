//! Closing the namespaces of a content fragment.

use super::*;

impl FragmentNamespaceClosure {
    pub(super) fn read(
        xml: &[u8],
        scope: &std::collections::BTreeMap<String, String>,
        pool: &mut CaptureDeclarations,
    ) -> Result<Self> {
        let invalid = |message: String| OxmlError::InvalidValue(message);
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(false);
        let (opening_end, empty, existing) = loop {
            let event = reader
                .read_event()
                .map_err(|error| invalid(format!("content fragment scan failed: {error}")))?;
            let empty = matches!(event, Event::Empty(_));
            match event {
                Event::Start(element) | Event::Empty(element) => {
                    let mut existing = std::collections::HashSet::new();
                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(|error| {
                            invalid(format!("content fragment attribute scan failed: {error}"))
                        })?;
                        let name = attribute.key.as_ref();
                        if name == b"xmlns" {
                            existing.insert(String::new());
                        } else if let Some(prefix) = name.strip_prefix(b"xmlns:") {
                            existing.insert(
                                std::str::from_utf8(prefix)
                                    .map_err(|error| {
                                        invalid(format!(
                                            "content fragment namespace prefix is invalid: {error}"
                                        ))
                                    })?
                                    .to_owned(),
                            );
                        }
                    }
                    break (xml.len() - reader.get_ref().len(), empty, existing);
                }
                Event::Eof => {
                    return Err(invalid("content fragment has no root element".into()));
                }
                _ => {}
            }
        };
        let mut bindings = Vec::new();
        let mut bytes = 0usize;
        for (prefix, namespace) in scope {
            if prefix == "xml" || existing.contains(prefix) {
                continue;
            }
            let index = pool.intern(prefix, namespace)?;
            bytes = bytes
                .checked_add(pool.declaration_bytes(index)?)
                .ok_or_else(|| invalid("content fragment namespace byte overflow".into()))?;
            bindings.push(index);
        }
        let insertion = opening_end
            .checked_sub(if empty { 2 } else { 1 })
            .ok_or_else(|| invalid("content fragment opening tag is incomplete".into()))?;
        Ok(Self {
            insertion,
            bindings,
            bytes,
            original_w_binding: scope.contains_key("w") || existing.contains("w"),
        })
    }

    pub(super) fn write(&self, xml: &[u8], pool: &CaptureDeclarations) -> Result<Vec<u8>> {
        let length = xml
            .len()
            .checked_add(self.bytes)
            .ok_or_else(|| OxmlError::InvalidValue("content fragment byte overflow".into()))?;
        let mut closed = Vec::with_capacity(length);
        closed.extend_from_slice(&xml[..self.insertion]);
        self.write_declarations(&mut closed, pool);
        closed.extend_from_slice(&xml[self.insertion..]);
        debug_assert_eq!(closed.len(), length);
        Ok(closed)
    }

    pub(super) fn write_declarations(&self, closed: &mut Vec<u8>, pool: &CaptureDeclarations) {
        for &index in &self.bindings {
            let prefix = pool.prefix(index);
            closed.extend_from_slice(b" xmlns");
            if !prefix.is_empty() {
                closed.push(b':');
                closed.extend_from_slice(prefix.as_bytes());
            }
            closed.extend_from_slice(b"=\"");
            closed.extend_from_slice(quick_xml::escape::escape(pool.namespace(index)).as_bytes());
            closed.push(b'"');
        }
    }
}

/// Close a retained fragment with every missing caller-supplied binding.
/// Local declarations, original bytes and the caller's binding order survive.
#[doc(hidden)]
pub fn close_content_fragment_namespaces(
    xml: &[u8],
    scope: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let mut pool = CaptureDeclarations::default();
    FragmentNamespaceClosure::read(xml, scope, &mut pool)?.write(xml, &pool)
}
