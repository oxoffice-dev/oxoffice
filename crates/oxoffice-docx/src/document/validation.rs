//! Checking a document against the schema and the package rules: findings and reports.

use super::*;

/// Severity of a document section validation finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationSeverity {
    Error,
    Warning,
}

/// One stable section validation fact, in document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationFinding {
    pub severity: ValidationSeverity,
    pub message: String,
}

/// Read-only validation results over the current document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    pub findings: Vec<ValidationFinding>,
}

impl ValidationReport {
    /// Warnings do not invalidate a document.
    pub fn is_valid(&self) -> bool {
        self.findings
            .iter()
            .all(|finding| finding.severity != ValidationSeverity::Error)
    }
}

// Selected MC branches and preserved property-change subtrees determine the
// exact style-reference stream. Capabilities share the embedded-content authority.
#[derive(Default)]
struct StyleReferenceScope {
    excluded: bool,
    alternate: Option<(bool, bool, bool)>, // saw choice, selected branch, saw fallback
}

fn document_choice_supported(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
) -> std::result::Result<bool, String> {
    let mut requires = None;
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|e| e.to_string())?;
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if matches!(namespace, ResolveResult::Unbound) && local.as_ref() == b"Requires" {
            if requires.is_some() {
                return Err("MC Choice has duplicate Requires".into());
            }
            requires = Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                    .map_err(|e| e.to_string())?
                    .into_owned(),
            );
        }
    }
    Ok(requires.is_some_and(|requires| {
        let mut prefixes = requires.split_ascii_whitespace();
        prefixes.clone().next().is_some()
            && prefixes
                .all(|prefix| crate::embedded::must_understand_prefix_is_valid(reader, prefix))
    }))
}

pub(super) fn document_style_references(
    xml: &[u8],
) -> std::result::Result<Vec<(&'static str, String)>, String> {
    let mut reader = NsReader::from_reader(xml);
    let mut open: Vec<StyleReferenceScope> = Vec::new();
    let mut roots = 0usize;
    let mut references = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let (namespace, event) = match reader.read_resolved_event_into(&mut buffer) {
            Ok(read) => read,
            Err(error) => return Err(format!("{error} at byte {}", reader.error_position())),
        };
        let is_word =
            matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes());
        let is_compatibility =
            matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == MC_NS.as_bytes());
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                if open.is_empty() {
                    roots += 1;
                    if roots > 1 {
                        return Err("the part has more than one root element".into());
                    }
                }
                let local = element.local_name();
                let mut excluded = open.last().is_some_and(|scope| scope.excluded)
                    || (is_word && local.as_ref().ends_with(b"PrChange"));
                if let Some(state) = open.last_mut().and_then(|scope| scope.alternate.as_mut()) {
                    if is_compatibility && local.as_ref() == b"Choice" {
                        if state.2 {
                            return Err("MC Choice follows Fallback".into());
                        }
                        state.0 = true;
                        let selected = !state.1 && document_choice_supported(&reader, element)?;
                        state.1 |= selected;
                        excluded |= !selected;
                    } else if is_compatibility && local.as_ref() == b"Fallback" {
                        if !state.0 || state.2 {
                            return Err(
                                "MC Fallback has no preceding Choice or is duplicated".into()
                            );
                        }
                        state.2 = true;
                        excluded |= state.1;
                        state.1 = true;
                    } else {
                        // A foreign lookalike is opaque, never an admitted branch.
                        excluded = true;
                    }
                } else if is_compatibility && matches!(local.as_ref(), b"Choice" | b"Fallback") {
                    excluded = true;
                }
                let kind = match local.as_ref() {
                    b"pStyle" => Some("paragraph"),
                    b"rStyle" => Some("character"),
                    b"tblStyle" => Some("table"),
                    _ => None,
                };
                if let Some(kind) = kind.filter(|_| is_word && !excluded)
                    && let Some(style_id) = document_word_value(&reader, element)?
                    && !style_id.is_empty()
                {
                    references.push((kind, style_id));
                }
                if matches!(event, Event::Start(_)) {
                    open.push(StyleReferenceScope {
                        excluded,
                        alternate: (is_compatibility && local.as_ref() == b"AlternateContent")
                            .then_some((false, false, false)),
                    });
                }
            }
            Event::End(_) => {
                open.pop();
            }
            Event::Text(text) if open.is_empty() && !text.iter().all(u8::is_ascii_whitespace) => {
                return Err("the part has text outside its root element".into());
            }
            Event::CData(_) if open.is_empty() => {
                return Err("the part has CDATA outside its root element".into());
            }
            Event::GeneralRef(_) if open.is_empty() => {
                return Err("the part has text outside its root element".into());
            }
            Event::Eof if roots == 0 => return Err("the part has no root element".into()),
            Event::Eof if !open.is_empty() => {
                return Err(format!(
                    "the part ends inside {} unclosed element(s)",
                    open.len()
                ));
            }
            Event::Eof => return Ok(references),
            _ => {}
        }
        buffer.clear();
    }
}

/// Return the `w:val` attribute of a WordprocessingML element.
fn document_word_value(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
) -> std::result::Result<Option<String>, String> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| error.to_string())?;
        let (namespace, local_name) = reader.resolver().resolve_attribute(attribute.key);
        if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
            && local_name.as_ref() == b"val"
        {
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                .map_err(|error| error.to_string())?;
            return Ok(Some(value.into_owned()));
        }
    }
    Ok(None)
}

// Finite validator-only tables, matching the transitional body/p/r child
// inventories. These do not become parser admission or a full schema engine.
const BODY_CHILDREN: &[&str] = &["altChunk", "customXml", "sdt", "p", "tbl", "sectPr"];
const PARAGRAPH_CHILDREN: &[&str] = &[
    "pPr",
    "r",
    "bdo",
    "dir",
    "fldSimple",
    "hyperlink",
    "smartTag",
    "customXml",
    "sdt",
    "subDoc",
    "contentPart",
];
const RANGE_CHILDREN: &[&str] = &[
    "bookmarkStart",
    "bookmarkEnd",
    "commentRangeStart",
    "commentRangeEnd",
    "customXmlDelRangeStart",
    "customXmlDelRangeEnd",
    "customXmlInsRangeStart",
    "customXmlInsRangeEnd",
    "customXmlMoveFromRangeStart",
    "customXmlMoveFromRangeEnd",
    "customXmlMoveToRangeStart",
    "customXmlMoveToRangeEnd",
    "moveFromRangeStart",
    "moveFromRangeEnd",
    "moveToRangeStart",
    "moveToRangeEnd",
    "ins",
    "del",
    "moveFrom",
    "moveTo",
    "permStart",
    "permEnd",
    "proofErr",
];
const RUN_CHILDREN: &[&str] = &[
    "rPr",
    "br",
    "t",
    "delText",
    "instrText",
    "delInstrText",
    "noBreakHyphen",
    "softHyphen",
    "dayShort",
    "monthShort",
    "yearShort",
    "dayLong",
    "monthLong",
    "yearLong",
    "annotationRef",
    "footnoteRef",
    "endnoteRef",
    "separator",
    "continuationSeparator",
    "sym",
    "pgNum",
    "cr",
    "tab",
    "object",
    "pict",
    "fldChar",
    "ruby",
    "footnoteReference",
    "endnoteReference",
    "commentReference",
    "drawing",
    "ptab",
    "lastRenderedPageBreak",
    "contentPart",
];
const PARAGRAPH_JC: &[&str] = &[
    "start",
    "center",
    "end",
    "both",
    "mediumKashida",
    "distribute",
    "numTab",
    "highKashida",
    "lowKashida",
    "thaiDistribute",
    "left",
    "right",
];
const TABLE_JC: &[&str] = &["start", "center", "end", "left", "right"];
const ELEMENT_ONLY: &[&str] = &[
    "body",
    "p",
    "r",
    "pPr",
    "rPr",
    "tbl",
    "tblPr",
    "tblGrid",
    "tr",
    "trPr",
    "tc",
    "tcPr",
    "sectPr",
    "sdt",
    "sdtPr",
    "sdtContent",
    "hyperlink",
    "fldSimple",
    "smartTag",
    "customXml",
    "ins",
    "del",
    "moveFrom",
    "moveTo",
    "dir",
    "bdo",
];
const MAX_POSITIVE_COORDINATE: i64 = 27_273_042_316_900;
const MAX_STORY_FINDINGS: usize = 10;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SchemaNamespace {
    Word,
    Math,
    Drawing,
    WordDrawing,
    KnownContainer,
    Foreign,
}

impl SchemaNamespace {
    fn resolved(namespace: &ResolveResult<'_>) -> Self {
        match namespace {
            ResolveResult::Bound(Namespace(uri)) if *uri == W_NS.as_bytes() => Self::Word,
            ResolveResult::Bound(Namespace(uri)) if *uri == M_NS.as_bytes() => Self::Math,
            ResolveResult::Bound(Namespace(uri)) if *uri == drawing_ns::A.as_bytes() => {
                Self::Drawing
            }
            ResolveResult::Bound(Namespace(uri)) if *uri == drawing_ns::WP.as_bytes() => {
                Self::WordDrawing
            }
            ResolveResult::Bound(Namespace(uri))
                if [
                    MC_NS.as_bytes(),
                    drawing_ns::PIC.as_bytes(),
                    b"http://schemas.microsoft.com/office/word/2010/wordprocessingShape",
                    b"http://schemas.microsoft.com/office/word/2010/wordprocessingGroup",
                    b"urn:schemas-microsoft-com:vml",
                    b"urn:schemas-microsoft-com:office:office",
                ]
                .contains(uri) =>
            {
                Self::KnownContainer
            }
            _ => Self::Foreign,
        }
    }
}

struct StoryScanFrame {
    namespace: SchemaNamespace,
    local: String,
    opaque: bool,
    deleted: bool,
    children: usize,
    section_seen: bool,
    radical_degree: bool,
    radical_expression: bool,
}

struct StoryScanContext<'a> {
    part: &'a str,
    relationships: HashSet<&'a str>,
    notes: &'a [HashSet<i32>; 2],
    findings: Vec<ValidationFinding>,
    suppressed: usize,
    suppressed_error: bool,
    bookmarks: BTreeMap<i32, usize>,
}

impl StoryScanContext<'_> {
    fn finding(&mut self, severity: ValidationSeverity, message: String) {
        if self.findings.len() < MAX_STORY_FINDINGS {
            self.findings.push(ValidationFinding {
                severity,
                message: format!("part {}: {message}", self.part),
            });
        } else {
            self.suppressed += 1;
            self.suppressed_error |= severity == ValidationSeverity::Error;
        }
    }

    fn error(&mut self, message: String) {
        self.finding(ValidationSeverity::Error, message);
    }

    fn child(&mut self, parent: &mut StoryScanFrame, namespace: SchemaNamespace, local: &str) {
        if parent.opaque || namespace == SchemaNamespace::Foreign {
            return;
        }
        if parent.namespace == SchemaNamespace::Word && namespace == SchemaNamespace::Word {
            let allowed = match parent.local.as_str() {
                "body" => Some(BODY_CHILDREN),
                "p" => Some(PARAGRAPH_CHILDREN),
                "r" => Some(RUN_CHILDREN),
                _ => None,
            };
            let range =
                matches!(parent.local.as_str(), "body" | "p") && RANGE_CHILDREN.contains(&local);
            if allowed.is_some_and(|allowed| !allowed.contains(&local) && !range) {
                self.error(format!("w:{local} is not allowed in w:{}", parent.local));
            }
            if ((parent.local == "p" && local == "pPr") || (parent.local == "r" && local == "rPr"))
                && parent.children != 0
            {
                self.error(format!(
                    "w:{local} is not the first child of w:{}",
                    parent.local
                ));
            }
            if parent.local == "body" {
                if parent.section_seen {
                    self.error(format!(
                        "w:{local} follows w:sectPr, the last child of w:body"
                    ));
                }
                parent.section_seen |= local == "sectPr";
            }
        }
        if parent.namespace == SchemaNamespace::Math
            && parent.local == "rad"
            && namespace == SchemaNamespace::Math
        {
            parent.radical_degree |= local == "deg";
            parent.radical_expression |= local == "e";
        }
        parent.children += 1;
    }

    fn close(&mut self, frame: StoryScanFrame) {
        if !frame.opaque && frame.namespace == SchemaNamespace::Math && frame.local == "rad" {
            if !frame.radical_degree {
                self.error("m:rad is missing required child m:deg".into());
            }
            if !frame.radical_expression {
                self.error("m:rad is missing required child m:e".into());
            }
        }
    }

    fn element(
        &mut self,
        reader: &NsReader<&[u8]>,
        element: &BytesStart<'_>,
        namespace: SchemaNamespace,
        open: &mut Vec<StoryScanFrame>,
        empty: bool,
    ) -> std::result::Result<(), String> {
        let local = std::str::from_utf8(element.local_name().as_ref())
            .map_err(|error| error.to_string())?
            .to_owned();
        let opaque =
            open.last().is_some_and(|frame| frame.opaque) || namespace == SchemaNamespace::Foreign;
        let deleted = open.last().is_some_and(|frame| frame.deleted)
            || (!opaque && namespace == SchemaNamespace::Word && local == "del");
        let mut attributes = BTreeMap::new();
        for attribute in element.attributes() {
            let attribute = attribute.map_err(|error| error.to_string())?;
            let (attribute_namespace, name) = reader.resolver().resolve_attribute(attribute.key);
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                .map_err(|error| error.to_string())?;
            if matches!(&attribute_namespace, ResolveResult::Bound(Namespace(uri)) if *uri == b"http://schemas.openxmlformats.org/officeDocument/2006/relationships")
                && !self.relationships.contains(value.as_ref())
            {
                self.error(format!(
                    "{}:{} references dangling relationship id {value}",
                    local,
                    String::from_utf8_lossy(name.as_ref())
                ));
            }
            if word_element(&attribute_namespace) {
                attributes.insert(name.as_ref().to_vec(), value.into_owned());
            } else if !opaque
                && matches!(
                    namespace,
                    SchemaNamespace::Drawing | SchemaNamespace::WordDrawing
                )
                && matches!(local.as_str(), "ext" | "extent")
                && matches!(attribute_namespace, ResolveResult::Unbound)
                && matches!(name.as_ref(), b"cx" | b"cy")
                && value
                    .parse::<i64>()
                    .ok()
                    .is_none_or(|value| !(0..=MAX_POSITIVE_COORDINATE).contains(&value))
            {
                self.error(format!(
                    "{local}/@{}={value:?} is not ST_PositiveCoordinate",
                    String::from_utf8_lossy(name.as_ref())
                ));
            }
        }
        if let Some(parent) = open.last_mut() {
            self.child(parent, namespace, &local);
        }
        if !opaque && namespace == SchemaNamespace::Word {
            if matches!(local.as_str(), "ins" | "del" | "moveFrom" | "moveTo") {
                for name in [b"id".as_slice(), b"author".as_slice()] {
                    if !attributes.contains_key(name) {
                        self.error(format!(
                            "w:{local} is missing required w:{}",
                            String::from_utf8_lossy(name)
                        ));
                    }
                }
            }
            if let Some(value) = attributes.get(b"val".as_slice()) {
                if local == "jc" {
                    let table = open.last().is_some_and(|frame| {
                        frame.namespace == SchemaNamespace::Word
                            && matches!(frame.local.as_str(), "tblPr" | "tblPrEx")
                    });
                    let allowed = if table { TABLE_JC } else { PARAGRAPH_JC };
                    if !allowed.contains(&value.as_str()) {
                        self.error(format!(
                            "w:jc w:val={value:?} is not an {} value",
                            if table { "ST_JcTable" } else { "ST_Jc" }
                        ));
                    }
                } else if local == "color"
                    && value != "auto"
                    && !(value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
                {
                    self.error(format!("w:color w:val={value:?} is not ST_HexColor"));
                }
            }
            if matches!(local.as_str(), "footnoteReference" | "endnoteReference") {
                let family = usize::from(local == "endnoteReference");
                let value = attributes.get(b"id".as_slice());
                if value
                    .and_then(|value| value.parse::<i32>().ok())
                    .is_none_or(|id| !self.notes[family].contains(&id))
                {
                    self.error(format!(
                        "w:{local} references missing {} id {}",
                        if family == 0 { "footnote" } else { "endnote" },
                        value.map_or("(absent)", String::as_str)
                    ));
                }
            }
            if deleted && matches!(local.as_str(), "t" | "instrText") {
                self.error(format!(
                    "w:{local} inside w:del must use w:{}",
                    if local == "t" {
                        "delText"
                    } else {
                        "delInstrText"
                    }
                ));
            }
            if local == "bookmarkStart"
                && let Some(id) = attributes
                    .get(b"id".as_slice())
                    .and_then(|value| value.parse::<i32>().ok())
            {
                *self.bookmarks.entry(id).or_default() += 1;
            }
        }
        let frame = StoryScanFrame {
            namespace,
            local,
            opaque,
            deleted,
            children: 0,
            section_seen: false,
            radical_degree: false,
            radical_expression: false,
        };
        if empty {
            self.close(frame);
        } else {
            open.push(frame);
        }
        Ok(())
    }

    fn text(&mut self, open: &[StoryScanFrame], whitespace: bool) {
        if !whitespace
            && let Some(frame) = open.last()
            && !frame.opaque
            && frame.namespace == SchemaNamespace::Word
            && ELEMENT_ONLY.contains(&frame.local.as_str())
        {
            self.error(format!(
                "text directly inside w:{} is not allowed",
                frame.local
            ));
        }
    }
}

fn document_story_findings(
    package: &OpcPackage,
    part: &str,
    xml: &[u8],
    notes: &[HashSet<i32>; 2],
) -> Vec<ValidationFinding> {
    let mut context = StoryScanContext {
        part,
        relationships: package
            .get_part_rels(part)
            .into_iter()
            .flat_map(|relationships| &relationships.items)
            .map(|relationship| relationship.id.as_str())
            .collect(),
        notes,
        findings: Vec::new(),
        suppressed: 0,
        suppressed_error: false,
        bookmarks: BTreeMap::new(),
    };
    let mut reader = NsReader::from_reader(xml);
    let mut open = Vec::new();
    let mut buffer = Vec::new();
    while let Ok((namespace, event)) = reader.read_resolved_event_into(&mut buffer) {
        let namespace = SchemaNamespace::resolved(&namespace);
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                if context
                    .element(
                        &reader,
                        element,
                        namespace,
                        &mut open,
                        matches!(event, Event::Empty(_)),
                    )
                    .is_err()
                {
                    break;
                }
            }
            Event::End(_) => {
                if let Some(frame) = open.pop() {
                    context.close(frame);
                }
            }
            Event::Text(text) => context.text(&open, text.iter().all(u8::is_ascii_whitespace)),
            Event::CData(text) => context.text(&open, text.iter().all(u8::is_ascii_whitespace)),
            Event::GeneralRef(reference) => context.text(
                &open,
                reference
                    .resolve_char_ref()
                    .ok()
                    .flatten()
                    .is_some_and(xml_1_0_whitespace),
            ),
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let bookmarks = std::mem::take(&mut context.bookmarks);
    for (id, count) in bookmarks {
        if count > 1 {
            context.finding(
                ValidationSeverity::Warning,
                format!("bookmark id {id} is used by {count} bookmarks"),
            );
        }
    }
    if context.suppressed != 0 {
        context.findings.push(ValidationFinding {
            severity: if context.suppressed_error {
                ValidationSeverity::Error
            } else {
                ValidationSeverity::Warning
            },
            message: format!(
                "part {part}: {} additional subset finding(s) omitted",
                context.suppressed
            ),
        });
    }
    context.findings
}

fn document_note_ids(package: &OpcPackage, relationship_type: &str, local: &[u8]) -> HashSet<i32> {
    let mut ids = HashSet::new();
    let Some(main) = package.main_document_part() else {
        return ids;
    };
    let Some(relationship) = package.get_part_rels(&main).and_then(|relationships| {
        relationships.items.iter().find(|relationship| {
            relationship.rel_type == relationship_type && relationship_is_internal(relationship)
        })
    }) else {
        return ids;
    };
    let part = OpcPackage::resolve_rel_target(&main, &relationship.target);
    let Some(xml) = package.get_part(&part) else {
        return ids;
    };
    let mut reader = NsReader::from_reader(xml);
    let mut depth = 0usize;
    let mut qualified_root = false;
    let mut buffer = Vec::new();
    while let Ok((namespace, event)) = reader.read_resolved_event_into(&mut buffer) {
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                if depth == 0 {
                    qualified_root = word_element(&namespace)
                        && element.local_name().as_ref()
                            == if local == b"footnote" {
                                &b"footnotes"[..]
                            } else {
                                &b"endnotes"[..]
                            };
                }
                if qualified_root
                    && depth == 1
                    && word_element(&namespace)
                    && element.local_name().as_ref() == local
                    && editable_note_owner(&reader, element).unwrap_or(false)
                {
                    for attribute in element.attributes().flatten() {
                        let (namespace, name) = reader.resolver().resolve_attribute(attribute.key);
                        if word_element(&namespace)
                            && name.as_ref() == b"id"
                            && let Ok(value) = attribute.decoded_and_normalized_value(
                                XmlVersion::Implicit1_0,
                                reader.decoder(),
                            )
                            && let Ok(id) = value.parse::<i32>()
                        {
                            ids.insert(id);
                        }
                    }
                }
                if matches!(event, Event::Start(_)) {
                    depth += 1;
                }
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    ids
}

fn document_xml_lexical_fault(xml: &[u8]) -> Option<String> {
    // Match the existing OPC UTF-8/BOM and UTF-16 signature frontier without
    // replacing or rewriting the stored part. Other encodings stay unproved.
    let utf16 = if let Some(body) = xml.strip_prefix(&[0xff, 0xfe]) {
        Some((body, true))
    } else if let Some(body) = xml.strip_prefix(&[0xfe, 0xff]) {
        Some((body, false))
    } else if xml.len() >= 4 && xml[1] == 0 && xml[3] == 0 {
        Some((xml, true))
    } else if xml.len() >= 4 && xml[0] == 0 && xml[2] == 0 {
        Some((xml, false))
    } else {
        None
    };
    let decoded;
    let xml = if let Some((body, little_endian)) = utf16 {
        if !body.len().is_multiple_of(2) {
            return Some("odd UTF-16 byte length".into());
        }
        let units = body
            .chunks_exact(2)
            .map(|pair| {
                if little_endian {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                }
            })
            .collect::<Vec<_>>();
        decoded = match String::from_utf16(&units) {
            Ok(decoded) => decoded,
            Err(error) => return Some(error.to_string()),
        };
        decoded.as_bytes()
    } else {
        xml.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(xml)
    };
    if let Err(error) = oxoffice_core::xml::validate_strict_xml_1_0(xml) {
        return Some(format!("{error:?}"));
    }
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut depth = 0usize;
    let mut roots = 0usize;
    let mut doctype = false;
    loop {
        let before = reader.buffer_position();
        match reader.read_event() {
            Ok(Event::Decl(_)) if before != 0 => {
                return Some("XML declaration is not first".into());
            }
            Ok(Event::DocType(_)) => {
                if doctype || roots != 0 || depth != 0 {
                    return Some("document type is not in the prolog".into());
                }
                doctype = true;
            }
            Ok(Event::Start(_)) => {
                if depth == 0 {
                    roots += 1;
                }
                depth += 1;
            }
            Ok(Event::Empty(_)) if depth == 0 => roots += 1,
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::Text(text)) if depth == 0 && !text.iter().all(u8::is_ascii_whitespace) => {
                return Some("text outside root element".into());
            }
            Ok(Event::CData(_) | Event::GeneralRef(_)) if depth == 0 => {
                return Some("text outside root element".into());
            }
            Ok(Event::Eof) => {
                return if roots == 1 && depth == 0 {
                    None
                } else {
                    Some("expected exactly one complete root element".into())
                };
            }
            Err(error) => return Some(error.to_string()),
            _ => {}
        }
    }
}

fn validate_document_read(
    read: std::result::Result<OpcPackage, oxoffice_opc::OpcError>,
) -> Result<ValidationReport> {
    match read {
        Ok(package) => validate_document_package(package),
        Err(oxoffice_opc::OpcError::InvalidXmlCharacter {
            part,
            code_point,
            line,
            column,
        }) => Ok(document_validation_report(
            vec![format!(
                "part {part} is not well-formed XML: it holds U+{code_point:04X} at line {line}, column {column}, a character XML 1.0 forbids"
            )],
            Vec::new(),
        )),
        Err(error) => Err(error.into()),
    }
}

/// Validate serialized document bytes without changing a live document.
/// Non-package input and unexplained document-open failures remain errors.
pub fn validate_document_bytes(bytes: &[u8]) -> Result<ValidationReport> {
    validate_document_read(OpcPackage::from_reader(std::io::Cursor::new(bytes)))
}

/// Validate a document package at a path with the same findings as byte validation.
pub fn validate_document_file(path: impl AsRef<Path>) -> Result<ValidationReport> {
    validate_document_read(OpcPackage::open(path))
}

fn document_validation_report(errors: Vec<String>, warnings: Vec<String>) -> ValidationReport {
    ValidationReport {
        findings: errors
            .into_iter()
            .map(|message| ValidationFinding {
                severity: ValidationSeverity::Error,
                message,
            })
            .chain(warnings.into_iter().map(|message| ValidationFinding {
                severity: ValidationSeverity::Warning,
                message,
            }))
            .collect(),
    }
}

impl Document {
    /// Validate an owned serialized snapshot, retaining this document's source and handles.
    pub fn validate(&self) -> Result<ValidationReport> {
        let mut snapshot = self.clone_for_staging();
        validate_document_bytes(&snapshot.to_bytes()?)
    }
}
fn validate_document_package(package: OpcPackage) -> Result<ValidationReport> {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // --- Structural errors ---

    // Every relationship must point at a part that is actually in the package.
    if let Some(doc_part) = package.main_document_part() {
        if package.get_part(&doc_part).is_none() {
            errors.push(format!("main document part {doc_part} is missing"));
        }
        if let Some(rels) = package.get_part_rels(&doc_part) {
            for rel in &rels.items {
                // External targets live outside the package by definition.
                if rel.target_mode.as_deref() == Some("External") {
                    continue;
                }
                let target = oxoffice_opc::OpcPackage::resolve_rel_target(&doc_part, &rel.target);
                if package.get_part(&target).is_none() {
                    errors.push(format!(
                        "relationship {} points at missing part {target}",
                        rel.id
                    ));
                    continue;
                }
                for kind in [HeaderFooterKind::Header, HeaderFooterKind::Footer] {
                    let content_type = package.content_types.content_type_for(&target);
                    if rel.rel_type == kind.relationship_type()
                        && content_type != Some(kind.content_type())
                    {
                        errors.push(format!(
                            "{} relationship {} points at {target}, which is not a WordprocessingML {} part (content type {})",
                            kind.part_stem(),
                            rel.id,
                            kind.part_stem(),
                            content_type.unwrap_or("none"),
                        ));
                    }
                }
            }
        }
    } else {
        errors.push("package declares no main document relationship".to_string());
    }

    // Every part needs a declared content type.
    for part_name in package.parts.keys() {
        if package.content_types.content_type_for(part_name).is_none() {
            errors.push(format!("part {part_name} has no declared content type"));
        }
    }

    // Markup Compatibility requires every prefix that `mc:Ignorable` or
    // `mc:MustUnderstand` lists to be declared, and a consumer may reject a
    // part that breaks the rule. A part that does not parse as XML is
    // outside this check.
    let mut xml_parts = package
        .parts
        .iter()
        .filter(|(part_name, _)| {
            package
                .content_types
                .content_type_for(part_name)
                .is_some_and(|content_type| content_type.ends_with("xml"))
        })
        .collect::<Vec<_>>();
    xml_parts.sort();
    for (part_name, xml) in xml_parts {
        let Ok(findings) = oxoffice_docx_oxml::namespace::undeclared_compatibility_prefixes(xml)
        else {
            continue;
        };
        for (attribute, prefix) in findings {
            errors.push(format!(
                "part {part_name} lists undeclared prefix `{prefix}` in mc:{attribute}"
            ));
        }
    }

    // Every XML part the main document relates to must be well formed. The
    // style ids that its story parts name are checked once the document opens.
    let mut style_references = Vec::new();
    let mut checked_xml = HashSet::new();
    let notes = [
        document_note_ids(&package, rel_types::FOOTNOTES, b"footnote"),
        document_note_ids(&package, rel_types::ENDNOTES, b"endnote"),
    ];
    if let Some(doc_part) = package.main_document_part() {
        let mut parts = BTreeMap::from([(part_name_identity(&doc_part), (doc_part.clone(), true))]);
        for rel in package
            .get_part_rels(&doc_part)
            .iter()
            .flat_map(|rels| &rels.items)
        {
            if rel.target_mode.as_deref() == Some("External") {
                continue;
            }
            let target = oxoffice_opc::OpcPackage::resolve_rel_target(&doc_part, &rel.target);
            let story = STORY_TEXT_RELATIONSHIPS.contains(&rel.rel_type.as_str());
            parts
                .entry(part_name_identity(&target))
                .and_modify(|(_, has_story_role)| *has_story_role |= story)
                .or_insert((target, story));
        }
        for (_, (part_name, story)) in parts {
            let is_xml = package
                .content_types
                .content_type_for(&part_name)
                .is_some_and(|content_type| {
                    content_type.ends_with("+xml") || content_type.ends_with("/xml")
                });
            let Some(xml) = package.get_part(&part_name).filter(|_| is_xml) else {
                continue;
            };
            if !checked_xml.insert(part_name_identity(&part_name)) {
                continue;
            }
            if let Some(detail) = document_xml_lexical_fault(xml) {
                errors.push(format!("part {part_name} is not well-formed XML: {detail}"));
                continue;
            }
            if story {
                for finding in document_story_findings(&package, &part_name, xml, &notes) {
                    match finding.severity {
                        ValidationSeverity::Error => errors.push(finding.message),
                        ValidationSeverity::Warning => warnings.push(finding.message),
                    }
                }
                match document_style_references(xml) {
                    Err(detail) => {
                        errors.push(format!("part {part_name} is not well-formed XML: {detail}"));
                    }
                    Ok(references) => style_references.extend(
                        references
                            .into_iter()
                            .map(|(kind, style_id)| (kind, style_id, part_name.clone())),
                    ),
                }
            }
        }
    }

    let mut other_xml = package
        .parts
        .iter()
        .filter(|(part, _)| {
            !checked_xml.contains(&part_name_identity(part))
                && package
                    .content_types
                    .content_type_for(part)
                    .is_some_and(|content_type| content_type.to_ascii_lowercase().ends_with("xml"))
        })
        .collect::<Vec<_>>();
    other_xml.sort_by_key(|(part, _)| *part);
    for (part, xml) in other_xml {
        if let Some(detail) = document_xml_lexical_fault(xml) {
            errors.push(format!("part {part} is not well-formed XML: {detail}"));
        }
    }

    // A malformed part can keep the document from opening. The errors found
    // so far explain why, so they are reported together with the failure.
    let doc = match Document::from_package(package) {
        Ok(doc) => doc,
        Err(error) if !errors.is_empty() => {
            errors.push(format!("the document does not open: {error}"));
            return Ok(document_validation_report(errors, warnings));
        }
        Err(error) => return Err(error),
    };

    // Word falls back to the default style for a style id that no style
    // defines, so the style that the document names is silently lost.
    for (kind, style_id, part_name) in style_references {
        let issue = format!("{kind} style {style_id:?} used in {part_name} is not defined");
        if doc.style(&style_id).is_none() && !errors.contains(&issue) {
            errors.push(issue);
        }
    }

    if let Err(error) = doc.validate_comment_ownership() {
        errors.push(format!("invalid comment ownership: {error}"));
    }

    // --- Advisory findings ---

    if doc.content_count() == 0 {
        warnings.push("Document has no content (no paragraphs or tables)".to_string());
    }

    match doc.paragraph_visibility() {
        Ok(visibility) => {
            let empty_count = visibility.iter().filter(|visible| !**visible).count();
            if empty_count > 0 {
                warnings.push(format!("{empty_count} empty paragraph(s) found"));
            }
        }
        Err(error) if !errors.is_empty() => warnings.push(format!(
            "Paragraph visibility could not be determined: {error}"
        )),
        Err(error) => return Err(error),
    }

    let mut prev_level: Option<u32> = None;
    for (level, _) in doc.headings() {
        if let Some(prev) = prev_level
            && level > prev + 1
        {
            warnings.push(format!(
                "Heading level gap: Heading{prev} -> Heading{level} (skipped level(s))"
            ));
        }
        prev_level = Some(level);
    }

    if doc.title().is_none() {
        warnings.push("Missing document title".to_string());
    }
    if doc.author().is_none() {
        warnings.push("Missing document author".to_string());
    }

    for finding in doc.validate_sections().findings {
        match finding.severity {
            ValidationSeverity::Error => errors.push(finding.message),
            ValidationSeverity::Warning => warnings.push(finding.message),
        }
    }

    match dangling_numbering_warnings(&doc) {
        Ok(found) => warnings.extend(found),
        Err(error) => warnings.push(format!(
            "numbering references could not be checked: {error}"
        )),
    }
    Ok(document_validation_report(errors, warnings))
}

/// One warning for each paragraph numbered by a numbering instance the
/// document does not define, or at a level its definition does not have.
/// Word opens such a file and shows the paragraph without a number. The
/// reference is resolved as `Document::validate_numbering_graph` resolves it:
/// direct numbering, then the paragraph style's. `doc` was just opened, so its
/// story parts are the bytes being validated. A part two relationships share
/// is read once, the glossary resolves against its own numbering and styles,
/// and the `mc:Fallback` copy of a text box is not read twice.
fn dangling_numbering_warnings(doc: &Document) -> Result<Vec<String>> {
    let glossary = doc.glossary_numbering()?;
    let mut warnings = Vec::new();
    let mut seen = HashSet::new();
    for part_name in doc.word_story_part_names() {
        if !seen.insert(part_name_identity(&part_name)) {
            continue;
        }
        let Some(xml) = doc
            .package
            .get_part(&part_name)
            .filter(|bytes| identifier_xml_is_well_formed(bytes))
            .filter(|_| !glossary.skipped.contains(&part_name))
        else {
            continue;
        };
        // A glossary resolves against its own numbering and styles.
        let (numbering, styles) = if glossary.part_names.contains(&part_name) {
            (glossary.numbering.as_ref(), &glossary.styles)
        } else {
            (doc.numbering.as_ref(), &*doc.styles)
        };
        for direct in xml_paragraph_numbering_properties(xml)? {
            let Some((num_id, level)) =
                super::numbering_types::effective_numbering_reference(styles, &direct)
            else {
                continue;
            };
            let Some((numbering, instance)) = numbering.and_then(|numbering| {
                numbering
                    .nums
                    .iter()
                    .find(|instance| instance.num_id == num_id)
                    .map(|instance| (numbering, instance))
            }) else {
                warnings.push(format!(
                    "a paragraph in {part_name} is numbered by numbering instance {num_id}, \
                     which the document does not define: Word shows it without a number"
                ));
                continue;
            };
            match definition_levels(numbering, styles, instance.abstract_num_id) {
                None => warnings.push(format!(
                    "a paragraph in {part_name} is numbered by numbering instance {num_id}, \
                     whose definition {} the document does not define: Word shows it \
                     without a number",
                    instance.abstract_num_id
                )),
                Some(levels) if !levels.iter().any(|candidate| candidate.ilvl == level) => {
                    warnings.push(format!(
                        "a paragraph in {part_name} is numbered at level {level} of numbering \
                         instance {num_id}, a level its definition does not have: Word shows \
                         it without a number"
                    ));
                }
                Some(_) => {}
            }
        }
    }
    Ok(warnings)
}

/// The levels of numbering definition `abstract_num_id`. A definition without
/// levels that names a numbering style (`w:numStyleLink`), as Word writes a
/// list that uses a list style, takes the levels of the definition that
/// style's numbering instance uses. `None` when a definition, style or
/// instance on the way is missing.
fn definition_levels<'a>(
    numbering: &'a oxoffice_docx_oxml::numbering::CT_Numbering,
    styles: &oxoffice_docx_oxml::styles::CT_Styles,
    abstract_num_id: u32,
) -> Option<&'a [oxoffice_docx_oxml::numbering::CT_Lvl]> {
    let mut id = abstract_num_id;
    // A list style links once in Word; the bound stops a cycle of links.
    for _ in 0..8 {
        let definition = numbering
            .abstract_nums
            .iter()
            .find(|definition| definition.abstract_num_id == id)?;
        let link = definition
            .levels
            .is_empty()
            .then(|| num_style_link(&definition.extra_xml))
            .flatten();
        let Some(link) = link else {
            return Some(&definition.levels);
        };
        let num_id = styles
            .get_by_id(&link)
            .filter(|style| style.style_type == StyleType::Numbering)
            .and_then(|style| style.ppr.as_ref()?.num_id)?;
        id = numbering
            .nums
            .iter()
            .find(|instance| instance.num_id == num_id)?
            .abstract_num_id;
    }
    None
}

/// The style a definition's raw `w:numStyleLink` child names.
fn num_style_link(raw_children: &[(usize, Vec<u8>)]) -> Option<String> {
    raw_children.iter().find_map(|(_, raw)| {
        let mut reader = quick_xml::Reader::from_reader(raw.as_slice());
        let mut buffer = Vec::new();
        match reader.read_event_into(&mut buffer).ok()? {
            quick_xml::events::Event::Empty(element) | quick_xml::events::Event::Start(element)
                if element.local_name().as_ref() == b"numStyleLink" =>
            {
                element.attributes().flatten().find_map(|attribute| {
                    (attribute.key.local_name().as_ref() == b"val")
                        .then(|| {
                            attribute
                                .decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .ok()
                                .map(|value| value.into_owned())
                        })
                        .flatten()
                })
            }
            _ => None,
        }
    })
}
