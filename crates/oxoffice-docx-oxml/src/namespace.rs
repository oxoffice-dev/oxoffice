//! OOXML namespace constants.

use oxoffice_core::xml_reader::NsReader;
use quick_xml::XmlVersion;
use quick_xml::events::Event;
use quick_xml::name::{Namespace, QName, ResolveResult};

use crate::error::Result;

/// WordprocessingML main namespace
pub const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
/// WordprocessingML namespace prefix
pub const W_PREFIX: &[u8] = b"w";
/// Transitional OfficeMath namespace.
pub const M_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";
/// Canonical OfficeMath namespace prefix.
pub const M_PREFIX: &[u8] = b"m";

pub use oxoffice_core::xml::{MC_NS, R_NS, matches_local_name};

/// The prefixes that an `mc:Ignorable` or `mc:MustUnderstand` attribute in
/// `xml` lists without a namespace declaration in scope.
///
/// Each finding is the attribute's local name and the prefix, once per pair,
/// in document order. Markup Compatibility (ECMA-376 Part 3) requires every
/// listed prefix to be declared, so a part with a finding is not conformant.
pub fn undeclared_compatibility_prefixes(xml: &[u8]) -> Result<Vec<(String, String)>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut findings: Vec<(String, String)> = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) | Event::Empty(element) => {
                for attribute in element.attributes() {
                    let attribute = attribute?;
                    let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                    let local = std::str::from_utf8(local.as_ref())?;
                    if namespace != ResolveResult::Bound(Namespace(MC_NS.as_bytes()))
                        || !matches!(local, "Ignorable" | "MustUnderstand")
                    {
                        continue;
                    }
                    let value = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?;
                    for prefix in value.split_ascii_whitespace() {
                        let probe = format!("{prefix}:probe");
                        let (bound, _) = reader.resolver().resolve_element(QName(probe.as_bytes()));
                        if !matches!(bound, ResolveResult::Bound(_))
                            && !findings
                                .iter()
                                .any(|(name, known)| name == local && known == prefix)
                        {
                            findings.push((local.to_owned(), prefix.to_owned()));
                        }
                    }
                }
            }
            Event::Eof => return Ok(findings),
            _ => {}
        }
        buffer.clear();
    }
}
