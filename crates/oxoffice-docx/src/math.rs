//! MathML and LaTeX conversion for the normalized OfficeMath tree.

use std::collections::HashSet;
use std::fmt::Write as _;

use oxoffice_docx_oxml::math::{
    FractionType, MathAccent, MathArgument, MathDelimiter, MathExpression, MathFraction, MathLimit,
    MathMatrix, MathMatrixRow, MathNary, MathPreSubSuperscript, MathRadical, MathRun, MathScript,
    MathStyle, MathSubSuperscript,
};
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

use crate::{Error, Result};

const MATHML_NS: &str = "http://www.w3.org/1998/Math/MathML";
const MAX_INPUT_BYTES: usize = 1024 * 1024;
const MAX_EVENTS: usize = 100_000;
const MAX_DEPTH: usize = 64;
const MAX_NODES: usize = 50_000;
const MAX_ROWS: usize = 256;
const MAX_COLUMNS: usize = 256;
const MAX_TEXT_BYTES: usize = 512 * 1024;
const MAX_DIAGNOSTICS: usize = 1024;

/// One stable source location and message for a lossy equation conversion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MathConversionDiagnostic {
    /// Format-specific element path or byte offset.
    pub path: String,
    /// Stable description of the content that was not represented.
    pub message: String,
}

/// A converted value together with every ordered loss diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MathConversionResult<T> {
    /// The normalized equation tree or canonical serialized text.
    pub value: T,
    /// Ordered diagnostics produced while converting the value.
    pub diagnostics: Vec<MathConversionDiagnostic>,
}

#[derive(Clone, Debug)]
struct XmlAttribute {
    namespace: Option<String>,
    local: String,
    value: String,
}

#[derive(Clone, Debug)]
enum XmlChild {
    Element(XmlNode),
    Text(String),
}

#[derive(Clone, Debug)]
struct XmlNode {
    namespace: Option<String>,
    local: String,
    attributes: Vec<XmlAttribute>,
    children: Vec<XmlChild>,
}

impl XmlNode {
    fn elements(&self) -> impl Iterator<Item = &XmlNode> {
        self.children.iter().filter_map(|child| match child {
            XmlChild::Element(element) => Some(element),
            XmlChild::Text(_) => None,
        })
    }

    fn attribute(&self, local: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|attribute| attribute.namespace.is_none() && attribute.local == local)
            .map(|attribute| attribute.value.as_str())
    }

    fn text(&self) -> String {
        let mut value = String::new();
        for child in &self.children {
            if let XmlChild::Text(text) = child {
                value.push_str(text);
            }
        }
        value
    }
}

struct Diagnostics {
    values: Vec<MathConversionDiagnostic>,
}

impl Diagnostics {
    fn new() -> Self {
        Self { values: Vec::new() }
    }

    fn push(&mut self, path: impl Into<String>, message: impl Into<String>) -> Result<()> {
        if self.values.len() >= MAX_DIAGNOSTICS {
            return Err(math_error("conversion exceeds the diagnostic limit"));
        }
        self.values.push(MathConversionDiagnostic {
            path: path.into(),
            message: message.into(),
        });
        Ok(())
    }
}

/// Import Presentation MathML into the normalized OfficeMath argument tree.
pub fn equation_from_mathml(input: &str) -> Result<MathConversionResult<MathArgument>> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(math_error("MathML input exceeds the byte limit"));
    }
    let root = parse_mathml_xml(input.as_bytes())?;
    if root.namespace.as_deref() != Some(MATHML_NS) || root.local != "math" {
        return Err(math_error(
            "MathML root must be math in the W3C MathML namespace",
        ));
    }
    let mut diagnostics = Diagnostics::new();
    let mut nodes = 0_usize;
    diagnose_attributes(&root, "/math[1]", &[], &mut diagnostics)?;
    let mut value = mathml_children_to_argument(&root, "/math[1]", &mut diagnostics, &mut nodes)?;
    normalize_argument(&mut value);
    validate_tree(&value).map_err(math_error)?;
    Ok(MathConversionResult {
        value,
        diagnostics: diagnostics.values,
    })
}

/// Export a normalized OfficeMath argument tree as canonical Presentation MathML.
pub fn equation_to_mathml(expression: &MathArgument) -> MathConversionResult<String> {
    let mut diagnostics = Diagnostics::new();
    if let Err(message) = validate_tree(expression) {
        return MathConversionResult {
            value: String::new(),
            diagnostics: vec![MathConversionDiagnostic {
                path: "/math[1]".to_owned(),
                message,
            }],
        };
    }
    let mut normalized = expression.clone();
    normalize_argument(&mut normalized);
    let mut output = format!(r#"<math xmlns="{MATHML_NS}">"#);
    let write_failed =
        write_mathml_argument(&normalized, "/math[1]", &mut output, &mut diagnostics).is_err();
    if write_failed {
        output.clear();
        diagnostics.values = vec![MathConversionDiagnostic {
            path: "/math[1]".to_owned(),
            message: "conversion exceeds the diagnostic limit".to_owned(),
        }];
    } else {
        output.push_str("</math>");
        if output.len() > MAX_INPUT_BYTES {
            output.clear();
            diagnostics.values = vec![MathConversionDiagnostic {
                path: "/math[1]".to_owned(),
                message: "serialized MathML exceeds the reader byte limit".to_owned(),
            }];
        } else if equation_from_mathml(&output).is_err() {
            output.clear();
            diagnostics.values = vec![MathConversionDiagnostic {
                path: "/math[1]".to_owned(),
                message: "serialized MathML exceeds the reader structural limits".to_owned(),
            }];
        }
    }
    MathConversionResult {
        value: output,
        diagnostics: diagnostics.values,
    }
}

/// Whether the raw paragraph child `index` is one of the paragraph's typed
/// equations, which are kept as raw children at their slot.
pub(crate) fn raw_is_equation(paragraph: &oxoffice_docx_oxml::text::CT_P, index: usize) -> bool {
    let run_index = paragraph.extra_xml[index].0;
    let slot = paragraph.extra_xml[..index]
        .iter()
        .filter(|(at, _)| *at == run_index)
        .count();
    paragraph
        .equations
        .iter()
        .any(|(at, equation_slot, _)| *at == run_index && *equation_slot == slot)
}

/// The equation writer of the HTML, Markdown and EPUB exports: MathML and
/// LaTeX through [`equation_to_mathml`] and [`equation_to_latex`]. Every
/// lossy conversion, and every equation it cannot write, is reported.
#[derive(Default)]
pub(crate) struct ExportEquations {
    diagnostics: std::cell::RefCell<Vec<String>>,
    /// The equations written so far, which numbers them in the messages.
    count: std::cell::Cell<usize>,
}

impl ExportEquations {
    /// The messages of the conversions so far, in order.
    pub(crate) fn into_diagnostics(self) -> Vec<String> {
        self.diagnostics.into_inner()
    }

    fn write(
        &self,
        equation: &oxoffice_docx_oxml::math::CT_OMath,
        format: &str,
        convert: fn(&MathArgument) -> MathConversionResult<String>,
    ) -> Option<String> {
        let number = self.count.get() + 1;
        self.count.set(number);
        let mut diagnostics = self.diagnostics.borrow_mut();
        // What the reader keeps outside the typed expressions (m:func,
        // m:groupChr, m:eqArr, a tracked change...) is not converted.
        let mut container = equation.clone();
        container.expressions.clear();
        let unsupported = container.has_unsupported_content();
        if equation.expressions.is_empty() {
            diagnostics.push(format!(
                "equation {number} left out of the {format}: none of its content is in the supported subset"
            ));
            return None;
        }
        let result = convert(&MathArgument::new(equation.expressions.clone()));
        if result.value.is_empty() {
            let reason = result
                .diagnostics
                .first()
                .map_or("no content", |diagnostic| diagnostic.message.as_str());
            diagnostics.push(format!(
                "equation {number} left out of the {format}: {reason}"
            ));
            return None;
        }
        if unsupported {
            diagnostics.push(format!(
                "equation {number} written as {format} without its content outside the supported subset"
            ));
        }
        for diagnostic in &result.diagnostics {
            diagnostics.push(format!(
                "equation {number} written as {format} at {}: {}",
                diagnostic.path, diagnostic.message
            ));
        }
        Some(result.value)
    }
}

impl oxoffice_docx_html::EquationWriter for ExportEquations {
    fn mathml(&self, equation: &oxoffice_docx_oxml::math::CT_OMath) -> Option<String> {
        self.write(equation, "MathML", equation_to_mathml)
    }

    fn latex(&self, equation: &oxoffice_docx_oxml::math::CT_OMath) -> Option<String> {
        self.write(equation, "LaTeX", equation_to_latex)
    }

    fn display(&self, display: &oxoffice_docx_oxml::math::CT_OMathPara) {
        let mut wrapper = display.clone();
        wrapper.equations.clear();
        let number = self.count.get() + 1;
        let mut diagnostics = self.diagnostics.borrow_mut();
        if wrapper.has_unsupported_content() {
            diagnostics.push(format!(
                "display group of equation {number}: its content outside the supported subset was left out"
            ));
        }
        if display.properties.justification.is_some() {
            diagnostics.push(format!(
                "display group of equation {number}: its justification was left out"
            ));
        }
    }
}

/// Import the supported LaTeX subset into the normalized OfficeMath argument tree.
pub fn equation_from_latex(input: &str) -> Result<MathConversionResult<MathArgument>> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(math_error("LaTeX input exceeds the byte limit"));
    }
    let mut parser = LatexParser::new(input);
    let mut value = parser.parse_argument(None)?;
    parser.skip_whitespace();
    if !parser.at_end() {
        return Err(parser.error("unexpected trailing LaTeX input"));
    }
    normalize_argument(&mut value);
    Ok(MathConversionResult {
        value,
        diagnostics: parser.diagnostics.values,
    })
}

/// Export a normalized OfficeMath argument tree as canonical LaTeX.
pub fn equation_to_latex(expression: &MathArgument) -> MathConversionResult<String> {
    let mut diagnostics = Diagnostics::new();
    if let Err(message) = validate_tree(expression) {
        return MathConversionResult {
            value: String::new(),
            diagnostics: vec![MathConversionDiagnostic {
                path: "byte:0".to_owned(),
                message,
            }],
        };
    }
    if diagnose_empty_latex_runs(expression, "byte:0", &mut diagnostics).is_err() {
        return MathConversionResult {
            value: String::new(),
            diagnostics: vec![MathConversionDiagnostic {
                path: "byte:0".to_owned(),
                message: "conversion exceeds the diagnostic limit".to_owned(),
            }],
        };
    }
    let mut normalized = expression.clone();
    normalize_argument(&mut normalized);
    let mut output = String::new();
    if write_latex_argument(&normalized, "byte:0", &mut output, &mut diagnostics).is_err() {
        output.clear();
        diagnostics.values = vec![MathConversionDiagnostic {
            path: "byte:0".to_owned(),
            message: "conversion exceeds the diagnostic limit".to_owned(),
        }];
    } else if output.len() > MAX_INPUT_BYTES {
        output.clear();
        diagnostics.values = vec![MathConversionDiagnostic {
            path: "byte:0".to_owned(),
            message: "serialized LaTeX exceeds the reader byte limit".to_owned(),
        }];
    } else if equation_from_latex(&output).is_err() {
        output.clear();
        diagnostics.values = vec![MathConversionDiagnostic {
            path: "byte:0".to_owned(),
            message: "serialized LaTeX exceeds the reader structural limits".to_owned(),
        }];
    }
    MathConversionResult {
        value: output,
        diagnostics: diagnostics.values,
    }
}

fn math_error(message: impl Into<String>) -> Error {
    Error::Other(format!("math conversion error: {}", message.into()))
}

fn parse_mathml_xml(xml: &[u8]) -> Result<XmlNode> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut stack = Vec::<XmlNode>::new();
    let mut root = None;
    let mut events = 0_usize;
    let mut text_bytes = 0_usize;
    loop {
        let offset = reader.buffer_position();
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| math_error(format!("malformed MathML at byte {offset}: {error}")))?;
        events = events
            .checked_add(1)
            .ok_or_else(|| math_error("MathML event count overflowed"))?;
        if events > MAX_EVENTS {
            return Err(math_error("MathML input exceeds the event limit"));
        }
        let namespace = resolved_namespace(namespace, offset)?;
        match event {
            Event::Start(element) => {
                if stack.len() >= MAX_DEPTH {
                    return Err(math_error("MathML input exceeds the depth limit"));
                }
                stack.push(xml_node(&reader, namespace, &element, offset)?);
            }
            Event::Empty(element) => {
                if stack.len() >= MAX_DEPTH {
                    return Err(math_error("MathML input exceeds the depth limit"));
                }
                attach_xml_node(
                    &mut stack,
                    &mut root,
                    xml_node(&reader, namespace, &element, offset)?,
                )?;
            }
            Event::End(_) => {
                let node = stack
                    .pop()
                    .ok_or_else(|| math_error("unmatched MathML end tag"))?;
                attach_xml_node(&mut stack, &mut root, node)?;
            }
            Event::Text(text) => {
                let decoded = text
                    .xml10_content()
                    .map_err(|error| math_error(format!("invalid MathML text: {error}")))?;
                let value = quick_xml::escape::unescape(&decoded)
                    .map_err(|error| math_error(format!("unresolved MathML entity: {error}")))?
                    .into_owned();
                append_xml_text(&mut stack, value, &mut text_bytes)?;
            }
            Event::CData(text) => {
                let value = text
                    .xml10_content()
                    .map_err(|error| math_error(format!("invalid MathML CDATA: {error}")))?
                    .into_owned();
                append_xml_text(&mut stack, value, &mut text_bytes)?;
            }
            Event::GeneralRef(reference) => {
                let value = if let Some(character) = reference
                    .resolve_char_ref()
                    .map_err(|error| math_error(format!("invalid MathML reference: {error}")))?
                {
                    character.to_string()
                } else {
                    let reference: &[u8] = &reference;
                    match reference {
                        b"amp" => "&",
                        b"lt" => "<",
                        b"gt" => ">",
                        b"apos" => "'",
                        b"quot" => "\"",
                        _ => return Err(math_error("unresolved MathML entity is unsupported")),
                    }
                    .to_owned()
                };
                append_xml_text(&mut stack, value, &mut text_bytes)?;
            }
            Event::DocType(_) => return Err(math_error("MathML DTD declarations are forbidden")),
            Event::Eof => break,
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) => {}
        }
        buffer.clear();
    }
    if !stack.is_empty() {
        return Err(math_error("MathML input has an unclosed element"));
    }
    root.ok_or_else(|| math_error("MathML input has no root element"))
}

fn resolved_namespace(namespace: ResolveResult<'_>, offset: u64) -> Result<Option<String>> {
    match namespace {
        ResolveResult::Bound(namespace) => Ok(Some(
            std::str::from_utf8(namespace.as_ref())
                .map_err(|_| math_error(format!("namespace at byte {offset} is not UTF-8")))?
                .to_owned(),
        )),
        ResolveResult::Unbound => Ok(None),
        ResolveResult::Unknown(_) => Err(math_error(format!(
            "unresolved namespace prefix at byte {offset}"
        ))),
    }
}

fn xml_node(
    reader: &NsReader<&[u8]>,
    namespace: Option<String>,
    element: &BytesStart<'_>,
    offset: u64,
) -> Result<XmlNode> {
    let local = std::str::from_utf8(element.local_name().as_ref())
        .map_err(|_| math_error(format!("element name at byte {offset} is not UTF-8")))?
        .to_owned();
    let mut attributes = Vec::new();
    let mut seen = HashSet::new();
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| math_error(format!("invalid attribute at byte {offset}: {error}")))?;
        if attribute.key.as_ref() == b"xmlns" || attribute.key.as_ref().starts_with(b"xmlns:") {
            continue;
        }
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        let namespace = resolved_namespace(namespace, offset)?;
        let local = std::str::from_utf8(local.as_ref())
            .map_err(|_| math_error(format!("attribute name at byte {offset} is not UTF-8")))?
            .to_owned();
        if !seen.insert((namespace.clone(), local.clone())) {
            return Err(math_error(format!(
                "duplicate expanded attribute at byte {offset}"
            )));
        }
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
            .map_err(|error| math_error(format!("invalid attribute at byte {offset}: {error}")))?
            .into_owned();
        attributes.push(XmlAttribute {
            namespace,
            local,
            value,
        });
    }
    Ok(XmlNode {
        namespace,
        local,
        attributes,
        children: Vec::new(),
    })
}

fn attach_xml_node(stack: &mut [XmlNode], root: &mut Option<XmlNode>, node: XmlNode) -> Result<()> {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(XmlChild::Element(node));
    } else if root.replace(node).is_some() {
        return Err(math_error("MathML input has more than one root element"));
    }
    Ok(())
}

fn append_xml_text(stack: &mut [XmlNode], value: String, total: &mut usize) -> Result<()> {
    *total = total
        .checked_add(value.len())
        .ok_or_else(|| math_error("MathML text size overflowed"))?;
    if *total > MAX_TEXT_BYTES {
        return Err(math_error("MathML input exceeds the text limit"));
    }
    let Some(parent) = stack.last_mut() else {
        if value.trim().is_empty() {
            return Ok(());
        }
        return Err(math_error("MathML text appears outside the root element"));
    };
    parent.children.push(XmlChild::Text(value));
    Ok(())
}

fn mathml_children_to_argument(
    node: &XmlNode,
    path: &str,
    diagnostics: &mut Diagnostics,
    nodes: &mut usize,
) -> Result<MathArgument> {
    let mut expressions = Vec::new();
    let mut positions = std::collections::HashMap::<&str, usize>::new();
    let mut previous_element = None::<&XmlNode>;
    let mut leading_text_space = false;
    for (index, child) in node.children.iter().enumerate() {
        match child {
            XmlChild::Text(text) if !text.trim().is_empty() => {
                diagnostics.push(path, "text outside a MathML token was discarded")?
            }
            XmlChild::Text(_) => {}
            XmlChild::Element(element) => {
                let previous = previous_element.replace(element);
                let position = positions.entry(element.local.as_str()).or_default();
                *position += 1;
                let child_path = format!("{path}/{}[{position}]", element.local);
                // A function marker qualifies the immediately preceding upright name.
                // The historical lim carrier already has its implicit function role.
                if element.namespace.as_deref() == Some(MATHML_NS)
                    && element.local == "mo"
                    && element.attributes.is_empty()
                    && element.elements().next().is_none()
                    && element.text() == "\u{2061}"
                    && let Some(MathExpression::Run(run)) = expressions.last_mut()
                    && ((run.text == "lim" && run.properties.normal == Some(true))
                        || (run.properties.style == Some(MathStyle::Plain)
                            && run.properties.normal.is_none()
                            && !run.text.is_empty()
                            && run.text.chars().all(char::is_alphabetic)))
                {
                    run.properties.normal = Some(true);
                    run.properties.style = None;
                    continue;
                }
                // Pandoc writes a trailing text space both lexically and as mspace.
                // Consume only this proved boundary pair, never arbitrary math spaces.
                if element.namespace.as_deref() == Some(MATHML_NS)
                    && element.local == "mspace"
                    && element.attributes.len() == 1
                    && element.attribute("width") == Some("0.333em")
                    && element.children.is_empty()
                    && previous.is_some_and(|node| {
                        node.namespace.as_deref() == Some(MATHML_NS) && node.local == "mtext"
                    })
                    && let Some(MathExpression::Run(run)) = expressions.last_mut()
                    && run.properties.normal == Some(true)
                    && run.properties.style == Some(MathStyle::Plain)
                {
                    if !run.text.ends_with(' ') {
                        run.text.push(' ');
                    }
                    continue;
                }
                if element.namespace.as_deref() == Some(MATHML_NS)
                    && element.local == "mspace"
                    && element.attributes.len() == 1
                    && element.attribute("width") == Some("0.333em")
                    && element.children.is_empty()
                    && node.children[index + 1..]
                        .iter()
                        .find_map(|child| match child {
                            XmlChild::Element(element) => Some(element),
                            _ => None,
                        })
                        .is_some_and(|next| {
                            next.namespace.as_deref() == Some(MATHML_NS) && next.local == "mtext"
                        })
                {
                    leading_text_space = true;
                    continue;
                }
                let mut converted =
                    mathml_element_to_expressions(element, &child_path, diagnostics, nodes)?;
                if leading_text_space {
                    if let Some(MathExpression::Run(run)) = converted.first_mut()
                        && !run.text.starts_with(' ')
                    {
                        run.text.insert(0, ' ');
                    }
                    leading_text_space = false;
                }
                expressions.extend(converted);
            }
        }
    }
    let mut rebuilt = Vec::with_capacity(expressions.len());
    let mut expressions = expressions.into_iter().peekable();
    while let Some(mut expression) = expressions.next() {
        if let MathExpression::Nary(nary) = &mut expression
            && nary.base.expressions.is_empty()
            && let Some(base) = expressions.next()
        {
            nary.base = MathArgument::new(vec![base]);
        }
        rebuilt.push(expression);
    }
    let mut argument = MathArgument::new(rebuilt);
    normalize_argument(&mut argument);
    Ok(argument)
}

fn mathml_element_to_expressions(
    node: &XmlNode,
    path: &str,
    diagnostics: &mut Diagnostics,
    nodes: &mut usize,
) -> Result<Vec<MathExpression>> {
    *nodes = nodes
        .checked_add(1)
        .ok_or_else(|| math_error("MathML node count overflowed"))?;
    if *nodes > MAX_NODES {
        return Err(math_error("MathML input exceeds the node limit"));
    }
    if node.namespace.as_deref() != Some(MATHML_NS) {
        diagnostics.push(path, "foreign-namespace element was discarded")?;
        return Ok(Vec::new());
    }
    let allowed_attributes = match node.local.as_str() {
        "mover" => &["accent"][..],
        "munder" => &["accentunder"][..],
        "munderover" => &["accent", "accentunder"][..],
        "mfenced" => &["open", "close", "separators"][..],
        "mi" | "mn" | "mo" | "mtext" => &["mathvariant"][..],
        "mspace" => &["width"][..],
        "mfrac" => &["linethickness"][..],
        "mstyle" => &["displaystyle"][..],
        _ => &[][..],
    };
    diagnose_attributes(node, path, allowed_attributes, diagnostics)?;
    if !matches!(
        node.local.as_str(),
        "math" | "mrow" | "mi" | "mn" | "mo" | "mtext" | "msqrt" | "mfenced"
    ) {
        diagnose_structural_text(node, path, diagnostics)?;
    }
    match node.local.as_str() {
        "mstyle" => {
            if !matches!(
                node.attribute("displaystyle"),
                None | Some("true" | "false")
            ) {
                diagnostics.push(
                    format!("{path}/@displaystyle"),
                    "unsupported MathML displaystyle was discarded",
                )?;
            }
            // Forced display/text style has no distinct typed OfficeMath carrier.
            Ok(mathml_children_to_argument(node, path, diagnostics, nodes)?.expressions)
        }
        "math" | "mrow" => {
            if node.local == "mrow"
                && let Some(delimiter) = parse_explicit_fenced_row(node, path, diagnostics, nodes)?
            {
                return Ok(vec![MathExpression::Delimiter(delimiter)]);
            }
            if node.local == "mrow"
                && let Some(nary) = parse_explicit_nary_row(node, path, diagnostics, nodes)?
            {
                return Ok(vec![MathExpression::Nary(nary)]);
            }
            Ok(mathml_children_to_argument(node, path, diagnostics, nodes)?.expressions)
        }
        "mi" | "mn" | "mo" | "mtext" => {
            for (index, _) in node.elements().enumerate() {
                diagnostics.push(
                    format!("{path}/*[{}]", index + 1),
                    "nested MathML token content was discarded",
                )?;
            }
            let mut run = MathRun::new(node.text());
            if node.local == "mtext" {
                run.properties.normal = Some(true);
                run.properties.style = Some(MathStyle::Plain);
            }
            if node.local == "mo" && run.text.contains('\u{2061}') {
                diagnostics.push(path, "out-of-context MathML function marker was retained")?;
            }
            if let Some(variant) = node.attribute("mathvariant") {
                if variant == "normal" {
                    if node.local != "mtext" {
                        if run.text == "lim" {
                            run.properties.normal = Some(true);
                        } else {
                            run.properties.style = Some(MathStyle::Plain);
                        }
                    }
                } else {
                    diagnostics.push(
                        format!("{path}/@mathvariant"),
                        "unsupported MathML mathvariant was discarded",
                    )?;
                }
            }
            Ok(vec![run.into()])
        }
        "mspace" => {
            for (index, _) in node.elements().enumerate() {
                diagnostics.push(
                    format!("{path}/*[{}]", index + 1),
                    "nested MathML spacing content was discarded",
                )?;
            }
            if let Some(space) = match node.attribute("width") {
                Some("0.167em" | "thinmathspace") => Some("\u{2009}"),
                Some("0.278em") => Some("\u{2004}"),
                Some("1em" | "1.0em") => Some("\u{2001}"),
                _ => None,
            } {
                Ok(vec![MathRun::new(space).into()])
            } else {
                diagnostics.push(
                    format!("{path}/@width"),
                    "unsupported MathML spacing width was discarded",
                )?;
                Ok(Vec::new())
            }
        }
        "mfrac" => {
            let children = mathml_element_children(node);
            require_child_count(path, &children, 2)?;
            let mut fraction = MathFraction::new(
                mathml_node_to_argument(children[0], &format!("{path}/*[1]"), diagnostics, nodes)?,
                mathml_node_to_argument(children[1], &format!("{path}/*[2]"), diagnostics, nodes)?,
            );
            match node.attribute("linethickness") {
                Some("0" | "0px") => fraction.fraction_type = FractionType::NoBar,
                None => {}
                Some(_) => diagnostics.push(
                    format!("{path}/@linethickness"),
                    "unsupported MathML fraction line thickness was discarded",
                )?,
            }
            Ok(vec![MathExpression::Fraction(fraction)])
        }
        "msub" | "msup" => {
            let children = mathml_element_children(node);
            require_child_count(path, &children, 2)?;
            if let Some(operator) =
                mathml_nary_operator(children[0], &format!("{path}/*[1]"), diagnostics)?
            {
                let mut nary = MathNary::new(operator, MathArgument::default());
                let script = mathml_node_to_argument(
                    children[1],
                    &format!("{path}/*[2]"),
                    diagnostics,
                    nodes,
                )?;
                if node.local == "msub" {
                    nary.subscript = script;
                    nary.hide_subscript = false;
                } else {
                    nary.superscript = script;
                    nary.hide_superscript = false;
                }
                return Ok(vec![MathExpression::Nary(nary)]);
            }
            let script = MathScript::new(
                mathml_node_to_argument(children[0], &format!("{path}/*[1]"), diagnostics, nodes)?,
                mathml_node_to_argument(children[1], &format!("{path}/*[2]"), diagnostics, nodes)?,
            );
            Ok(vec![if node.local == "msub" {
                MathExpression::Subscript(script)
            } else {
                MathExpression::Superscript(script)
            }])
        }
        "msubsup" => {
            let children = mathml_element_children(node);
            require_child_count(path, &children, 3)?;
            if let Some(operator) =
                mathml_nary_operator(children[0], &format!("{path}/*[1]"), diagnostics)?
            {
                let mut nary = MathNary::new(operator, MathArgument::default());
                nary.subscript = mathml_node_to_argument(
                    children[1],
                    &format!("{path}/*[2]"),
                    diagnostics,
                    nodes,
                )?;
                nary.superscript = mathml_node_to_argument(
                    children[2],
                    &format!("{path}/*[3]"),
                    diagnostics,
                    nodes,
                )?;
                nary.hide_subscript = false;
                nary.hide_superscript = false;
                return Ok(vec![MathExpression::Nary(nary)]);
            }
            Ok(vec![MathExpression::SubSuperscript(
                MathSubSuperscript::new(
                    mathml_node_to_argument(
                        children[0],
                        &format!("{path}/*[1]"),
                        diagnostics,
                        nodes,
                    )?,
                    mathml_node_to_argument(
                        children[1],
                        &format!("{path}/*[2]"),
                        diagnostics,
                        nodes,
                    )?,
                    mathml_node_to_argument(
                        children[2],
                        &format!("{path}/*[3]"),
                        diagnostics,
                        nodes,
                    )?,
                ),
            )])
        }
        "mmultiscripts" => parse_mathml_multiscripts(node, path, diagnostics, nodes),
        "msqrt" => Ok(vec![MathExpression::Radical(MathRadical::new(
            mathml_children_to_argument(node, path, diagnostics, nodes)?,
        ))]),
        "mroot" => {
            let children = mathml_element_children(node);
            require_child_count(path, &children, 2)?;
            Ok(vec![MathExpression::Radical(MathRadical::with_degree(
                mathml_node_to_argument(children[1], &format!("{path}/*[2]"), diagnostics, nodes)?,
                mathml_node_to_argument(children[0], &format!("{path}/*[1]"), diagnostics, nodes)?,
            ))])
        }
        "mtable" => parse_mathml_matrix(node, path, diagnostics, nodes),
        "munder" | "mover" | "munderover" => {
            parse_mathml_limits_or_accent(node, path, diagnostics, nodes)
        }
        "mfenced" => parse_mathml_fenced(node, path, diagnostics, nodes),
        "semantics" => {
            diagnostics.push(path, "MathML semantics metadata was discarded")?;
            let Some((index, content)) = node
                .elements()
                .enumerate()
                .find(|(_, child)| is_supported_mathml_element(child))
            else {
                return Ok(Vec::new());
            };
            mathml_element_to_expressions(
                content,
                &format!("{path}/*[{}]", index + 1),
                diagnostics,
                nodes,
            )
        }
        "none" | "mprescripts" => {
            diagnose_empty_mathml_marker(node, path, diagnostics)?;
            diagnostics.push(
                path,
                "MathML structural marker outside multiscripts was discarded",
            )?;
            Ok(Vec::new())
        }
        _ => {
            diagnostics.push(
                path,
                format!("unsupported MathML element {} was discarded", node.local),
            )?;
            Ok(Vec::new())
        }
    }
}

fn mathml_node_to_argument(
    node: &XmlNode,
    path: &str,
    diagnostics: &mut Diagnostics,
    nodes: &mut usize,
) -> Result<MathArgument> {
    Ok(MathArgument::new(mathml_element_to_expressions(
        node,
        path,
        diagnostics,
        nodes,
    )?))
}

fn mathml_element_children(node: &XmlNode) -> Vec<&XmlNode> {
    node.elements().collect()
}

fn require_child_count(path: &str, children: &[&XmlNode], expected: usize) -> Result<()> {
    if children.len() != expected {
        return Err(math_error(format!(
            "{path} requires {expected} element children"
        )));
    }
    Ok(())
}

fn diagnose_attributes(
    node: &XmlNode,
    path: &str,
    allowed: &[&str],
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    for attribute in &node.attributes {
        if attribute.namespace.is_some() || !allowed.contains(&attribute.local.as_str()) {
            diagnostics.push(
                format!("{path}/@{}", attribute.local),
                "unsupported MathML attribute was discarded",
            )?;
        }
    }
    Ok(())
}

fn diagnose_structural_text(
    node: &XmlNode,
    path: &str,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    for child in &node.children {
        if let XmlChild::Text(text) = child
            && !text.trim().is_empty()
        {
            diagnostics.push(path, "text outside a MathML token was discarded")?;
        }
    }
    Ok(())
}

fn diagnose_token_children(
    node: &XmlNode,
    path: &str,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    for (index, _) in node.elements().enumerate() {
        diagnostics.push(
            format!("{path}/*[{}]", index + 1),
            "nested MathML token content was discarded",
        )?;
    }
    Ok(())
}

fn mathml_nary_operator(
    node: &XmlNode,
    path: &str,
    diagnostics: &mut Diagnostics,
) -> Result<Option<String>> {
    if node.namespace.as_deref() != Some(MATHML_NS)
        || node.local != "mo"
        || !is_nary_operator(&node.text())
    {
        return Ok(None);
    }
    diagnose_attributes(node, path, &["largeop"], diagnostics)?;
    diagnose_boolean_attribute(node, path, "largeop", diagnostics)?;
    if node.attribute("largeop") == Some("false") {
        diagnostics.push(
            format!("{path}/@largeop"),
            "non-large MathML operator was normalized as n-ary",
        )?;
    }
    diagnose_token_children(node, path, diagnostics)?;
    Ok(Some(node.text()))
}

fn diagnose_empty_mathml_marker(
    node: &XmlNode,
    path: &str,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    diagnose_attributes(node, path, &[], diagnostics)?;
    diagnose_structural_text(node, path, diagnostics)?;
    if node.elements().next().is_some() {
        diagnostics.push(
            path,
            "content inside a MathML structural marker was discarded",
        )?;
    }
    Ok(())
}

fn diagnose_boolean_attribute(
    node: &XmlNode,
    path: &str,
    local: &str,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    if let Some(value) = node.attribute(local)
        && !matches!(value, "true" | "false")
    {
        diagnostics.push(
            format!("{path}/@{local}"),
            "unsupported MathML attribute value was discarded",
        )?;
    }
    Ok(())
}

fn diagnose_enum_attribute(
    node: &XmlNode,
    path: &str,
    local: &str,
    allowed: &[&str],
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    if let Some(value) = node.attribute(local)
        && !allowed.contains(&value)
    {
        diagnostics.push(
            format!("{path}/@{local}"),
            "unsupported MathML attribute value was discarded",
        )?;
    }
    Ok(())
}

fn is_supported_mathml_element(node: &XmlNode) -> bool {
    node.namespace.as_deref() == Some(MATHML_NS)
        && matches!(
            node.local.as_str(),
            "math"
                | "mrow"
                | "mstyle"
                | "mi"
                | "mn"
                | "mo"
                | "mtext"
                | "mfrac"
                | "msub"
                | "msup"
                | "msubsup"
                | "mmultiscripts"
                | "msqrt"
                | "mroot"
                | "mtable"
                | "munder"
                | "mover"
                | "munderover"
                | "mfenced"
                | "semantics"
        )
}

fn parse_mathml_multiscripts(
    node: &XmlNode,
    path: &str,
    diagnostics: &mut Diagnostics,
    nodes: &mut usize,
) -> Result<Vec<MathExpression>> {
    let children = mathml_element_children(node);
    if children.is_empty() {
        return Err(math_error(format!("{path} requires a base")));
    }
    let base = mathml_node_to_argument(children[0], &format!("{path}/*[1]"), diagnostics, nodes)?;
    let marker = children.iter().position(|child| {
        child.namespace.as_deref() == Some(MATHML_NS) && child.local == "mprescripts"
    });
    if let Some(index) = marker {
        diagnose_empty_mathml_marker(
            children[index],
            &format!("{path}/*[{}]", index + 1),
            diagnostics,
        )?;
    }
    let (post, pre) = match marker {
        Some(index) => (&children[1..index], &children[index + 1..]),
        None => (&children[1..], &children[0..0]),
    };
    if post.len() > 2 || pre.len() > 2 || post.len() == 1 || pre.len() == 1 {
        diagnostics.push(path, "extra MathML multiscript pairs were discarded")?;
    }
    if !post.is_empty() && !pre.is_empty() {
        diagnostics.push(
            path,
            "mixed pre-scripts and post-scripts cannot be represented",
        )?;
        return Ok(Vec::new());
    }
    let pair = if pre.is_empty() { post } else { pre };
    let pair_start = if pre.is_empty() {
        1
    } else {
        marker.expect("pre-scripts require a marker") + 1
    };
    let subscript = pair
        .first()
        .map(|child| {
            mathml_optional_script(
                child,
                &format!("{path}/*[{}]", pair_start + 1),
                diagnostics,
                nodes,
            )
        })
        .transpose()?
        .unwrap_or_default();
    let superscript = pair
        .get(1)
        .map(|child| {
            mathml_optional_script(
                child,
                &format!("{path}/*[{}]", pair_start + 2),
                diagnostics,
                nodes,
            )
        })
        .transpose()?
        .unwrap_or_default();
    Ok(vec![if pre.is_empty() {
        MathExpression::SubSuperscript(MathSubSuperscript::new(base, subscript, superscript))
    } else {
        MathExpression::PreSubSuperscript(MathPreSubSuperscript::new(base, subscript, superscript))
    }])
}

fn mathml_optional_script(
    node: &XmlNode,
    path: &str,
    diagnostics: &mut Diagnostics,
    nodes: &mut usize,
) -> Result<MathArgument> {
    if node.namespace.as_deref() == Some(MATHML_NS) && node.local == "none" {
        diagnose_empty_mathml_marker(node, path, diagnostics)?;
        return Ok(MathArgument::default());
    }
    mathml_node_to_argument(node, path, diagnostics, nodes)
}

fn parse_mathml_matrix(
    node: &XmlNode,
    path: &str,
    diagnostics: &mut Diagnostics,
    nodes: &mut usize,
) -> Result<Vec<MathExpression>> {
    let row_nodes = mathml_element_children(node);
    if row_nodes.is_empty() || row_nodes.len() > MAX_ROWS {
        return Err(math_error(format!("{path} exceeds the matrix row limit")));
    }
    let mut rows = Vec::new();
    let mut width = None;
    for (row_index, row) in row_nodes.iter().enumerate() {
        if row.namespace.as_deref() != Some(MATHML_NS) || row.local != "mtr" {
            return Err(math_error(format!("{path} contains a non-row child")));
        }
        let row_path = format!("{path}/mtr[{}]", row_index + 1);
        diagnose_attributes(row, &row_path, &[], diagnostics)?;
        diagnose_structural_text(row, &row_path, diagnostics)?;
        let cells = mathml_element_children(row);
        if cells.is_empty() || cells.len() > MAX_COLUMNS {
            return Err(math_error(format!(
                "{path} exceeds the matrix column limit"
            )));
        }
        if width
            .replace(cells.len())
            .is_some_and(|value| value != cells.len())
        {
            return Err(math_error(format!("{path} has a ragged matrix")));
        }
        let mut converted = Vec::new();
        for (column_index, cell) in cells.iter().enumerate() {
            if cell.namespace.as_deref() != Some(MATHML_NS) || cell.local != "mtd" {
                return Err(math_error(format!("{path} contains a non-cell child")));
            }
            let cell_path = format!("{row_path}/mtd[{}]", column_index + 1);
            diagnose_attributes(cell, &cell_path, &[], diagnostics)?;
            converted.push(mathml_children_to_argument(
                cell,
                &cell_path,
                diagnostics,
                nodes,
            )?);
        }
        rows.push(MathMatrixRow::new(converted));
    }
    Ok(vec![MathExpression::Matrix(MathMatrix::new(rows))])
}

fn parse_mathml_limits_or_accent(
    node: &XmlNode,
    path: &str,
    diagnostics: &mut Diagnostics,
    nodes: &mut usize,
) -> Result<Vec<MathExpression>> {
    diagnose_boolean_attribute(node, path, "accent", diagnostics)?;
    diagnose_boolean_attribute(node, path, "accentunder", diagnostics)?;
    let children = mathml_element_children(node);
    let expected = if node.local == "munderover" { 3 } else { 2 };
    require_child_count(path, &children, expected)?;
    let token_accent = children.get(1).is_some_and(|child| {
        child.namespace.as_deref() == Some(MATHML_NS)
            && child.local == "mo"
            && child.attribute("accent") == Some("true")
    });
    let accent = node.attribute("accent") == Some("true")
        || node.attribute("accentunder") == Some("true")
        || token_accent;
    if accent {
        if node.local != "mover" {
            diagnostics.push(path, "under-accent MathML cannot be represented")?;
            return Ok(Vec::new());
        }
        if children[1].namespace.as_deref() != Some(MATHML_NS) || children[1].local != "mo" {
            let _ = mathml_element_to_expressions(
                children[1],
                &format!("{path}/*[2]"),
                diagnostics,
                nodes,
            )?;
            diagnostics.push(path, "MathML accent requires an mo token")?;
            return Ok(Vec::new());
        }
        diagnose_attributes(
            children[1],
            &format!("{path}/*[2]"),
            &["accent"],
            diagnostics,
        )?;
        diagnose_boolean_attribute(children[1], &format!("{path}/*[2]"), "accent", diagnostics)?;
        diagnose_token_children(children[1], &format!("{path}/*[2]"), diagnostics)?;
        let character = match children[1].text().as_str() {
            "‾" | "¯" => "̄".to_owned(),
            "→" => "⃗".to_owned(),
            value => value.to_owned(),
        };
        if character.chars().count() != 1 {
            diagnostics.push(path, "MathML accent must contain one character")?;
            return Ok(Vec::new());
        }
        return Ok(vec![MathExpression::Accent(MathAccent::new(
            character,
            mathml_node_to_argument(children[0], &format!("{path}/*[1]"), diagnostics, nodes)?,
        ))]);
    }
    if let Some(operator) = mathml_nary_operator(children[0], &format!("{path}/*[1]"), diagnostics)?
    {
        let mut nary = MathNary::new(operator, MathArgument::default());
        nary.hide_subscript = node.local == "mover";
        nary.hide_superscript = node.local == "munder";
        if node.local == "munder" || node.local == "munderover" {
            nary.subscript =
                mathml_node_to_argument(children[1], &format!("{path}/*[2]"), diagnostics, nodes)?;
        }
        if node.local == "mover" {
            nary.superscript =
                mathml_node_to_argument(children[1], &format!("{path}/*[2]"), diagnostics, nodes)?;
        } else if node.local == "munderover" {
            nary.superscript =
                mathml_node_to_argument(children[2], &format!("{path}/*[3]"), diagnostics, nodes)?;
        }
        return Ok(vec![MathExpression::Nary(nary)]);
    }
    if node.local == "munder" {
        Ok(vec![MathExpression::LowerLimit(MathLimit::new(
            mathml_node_to_argument(children[0], &format!("{path}/*[1]"), diagnostics, nodes)?,
            mathml_node_to_argument(children[1], &format!("{path}/*[2]"), diagnostics, nodes)?,
        ))])
    } else if node.local == "mover" {
        Ok(vec![MathExpression::UpperLimit(MathLimit::new(
            mathml_node_to_argument(children[0], &format!("{path}/*[1]"), diagnostics, nodes)?,
            mathml_node_to_argument(children[1], &format!("{path}/*[2]"), diagnostics, nodes)?,
        ))])
    } else {
        diagnostics.push(path, "non-operator under-over pair cannot be represented")?;
        Ok(Vec::new())
    }
}

fn parse_mathml_fenced(
    node: &XmlNode,
    path: &str,
    diagnostics: &mut Diagnostics,
    nodes: &mut usize,
) -> Result<Vec<MathExpression>> {
    diagnose_structural_text(node, path, diagnostics)?;
    let open = node.attribute("open").unwrap_or("(");
    let close = node.attribute("close").unwrap_or(")");
    let separators = node.attribute("separators").unwrap_or(",");
    if open.chars().count() > 1 || close.chars().count() > 1 || separators.chars().count() > 1 {
        return Err(math_error(format!("{path} has a non-scalar fence")));
    }
    let mut arguments = Vec::new();
    for (index, child) in node.elements().enumerate() {
        arguments.push(mathml_node_to_argument(
            child,
            &format!("{path}/*[{}]", index + 1),
            diagnostics,
            nodes,
        )?);
    }
    if arguments.is_empty() {
        return Err(math_error(format!("{path} requires fenced content")));
    }
    let mut delimiter = MathDelimiter::new(open, close, arguments);
    if delimiter.arguments.len() > 1 {
        delimiter.separator_character = separators.to_owned();
    }
    Ok(vec![MathExpression::Delimiter(delimiter)])
}

fn parse_explicit_fenced_row(
    node: &XmlNode,
    path: &str,
    diagnostics: &mut Diagnostics,
    nodes: &mut usize,
) -> Result<Option<MathDelimiter>> {
    let children = mathml_element_children(node);
    if children.len() < 2 {
        return Ok(None);
    }
    let first = children[0];
    let last = children[children.len() - 1];
    if !is_mathml_fence(first, true) || !is_mathml_fence(last, false) {
        return Ok(None);
    }
    let mut arguments = Vec::new();
    let mut current = Vec::new();
    let mut separator = None::<String>;
    diagnose_attributes(
        first,
        &format!("{path}/*[1]"),
        &["fence", "stretchy", "form"],
        diagnostics,
    )?;
    diagnose_boolean_attribute(first, &format!("{path}/*[1]"), "fence", diagnostics)?;
    diagnose_boolean_attribute(first, &format!("{path}/*[1]"), "stretchy", diagnostics)?;
    diagnose_enum_attribute(
        first,
        &format!("{path}/*[1]"),
        "form",
        &["prefix", "infix", "postfix"],
        diagnostics,
    )?;
    diagnose_token_children(first, &format!("{path}/*[1]"), diagnostics)?;
    diagnose_attributes(
        last,
        &format!("{path}/*[{}]", children.len()),
        &["fence", "stretchy", "form"],
        diagnostics,
    )?;
    diagnose_boolean_attribute(
        last,
        &format!("{path}/*[{}]", children.len()),
        "fence",
        diagnostics,
    )?;
    diagnose_boolean_attribute(
        last,
        &format!("{path}/*[{}]", children.len()),
        "stretchy",
        diagnostics,
    )?;
    diagnose_enum_attribute(
        last,
        &format!("{path}/*[{}]", children.len()),
        "form",
        &["prefix", "infix", "postfix"],
        diagnostics,
    )?;
    diagnose_token_children(last, &format!("{path}/*[{}]", children.len()), diagnostics)?;
    for (index, child) in children[1..children.len() - 1].iter().enumerate() {
        if child.namespace.as_deref() == Some(MATHML_NS)
            && child.local == "mo"
            && child.attribute("separator") == Some("true")
            && child.text().chars().count() <= 1
        {
            diagnose_attributes(
                child,
                &format!("{path}/*[{}]", index + 2),
                &["separator"],
                diagnostics,
            )?;
            diagnose_boolean_attribute(
                child,
                &format!("{path}/*[{}]", index + 2),
                "separator",
                diagnostics,
            )?;
            diagnose_token_children(child, &format!("{path}/*[{}]", index + 2), diagnostics)?;
            let candidate = child.text();
            if separator
                .as_ref()
                .is_some_and(|current| current != &candidate)
            {
                diagnostics.push(
                    format!("{path}/*[{}]", index + 2),
                    "mixed MathML delimiter separators were normalized to the first character",
                )?;
            } else {
                separator.get_or_insert(candidate);
            }
            arguments.push(MathArgument::new(current));
            current = Vec::new();
        } else {
            current.extend(mathml_element_to_expressions(
                child,
                &format!("{path}/*[{}]", index + 2),
                diagnostics,
                nodes,
            )?);
        }
    }
    arguments.push(MathArgument::new(current));
    let mut delimiter = MathDelimiter::new(first.text(), last.text(), arguments);
    if delimiter.arguments.len() > 1 {
        delimiter.separator_character = separator.unwrap_or_else(|| ",".to_owned());
    }
    if delimiter.begin_character == "("
        && delimiter.end_character == ")"
        && matches!(delimiter.arguments.as_slice(), [argument] if matches!(argument.expressions.as_slice(), [MathExpression::Fraction(f)] if f.fraction_type == FractionType::NoBar))
        && first.attribute("stretchy") == Some("true")
        && last.attribute("stretchy") == Some("true")
    {
        delimiter.separator_character.clear();
        delimiter.grow = Some(true);
    }
    Ok(Some(delimiter))
}

fn is_mathml_fence(node: &XmlNode, leading: bool) -> bool {
    if node.namespace.as_deref() != Some(MATHML_NS) || node.local != "mo" {
        return false;
    }
    let character = node.text();
    let expected_form = if leading { "prefix" } else { "postfix" };
    let fence = node.attribute("fence");
    let stretchy = node.attribute("stretchy");
    let form = node.attribute("form");
    character.chars().count() <= 1
        && fence != Some("false")
        && stretchy != Some("false")
        && form.is_none_or(|value| value == expected_form)
        && (fence == Some("true") || (stretchy == Some("true") && form == Some(expected_form)))
}

fn parse_explicit_nary_row(
    node: &XmlNode,
    path: &str,
    diagnostics: &mut Diagnostics,
    nodes: &mut usize,
) -> Result<Option<MathNary>> {
    let children = mathml_element_children(node);
    if children.len() != 2 {
        return Ok(None);
    }
    let mut nary = if children[0].namespace.as_deref() == Some(MATHML_NS)
        && children[0].local == "mo"
        && children[0].attribute("largeop") == Some("true")
        && is_nary_operator(&children[0].text())
    {
        let operator = mathml_nary_operator(children[0], &format!("{path}/*[1]"), diagnostics)?
            .expect("the guarded MathML operator is n-ary");
        MathNary::new(operator, MathArgument::default())
    } else {
        let head_children = mathml_element_children(children[0]);
        if children[0].namespace.as_deref() != Some(MATHML_NS)
            || !matches!(
                children[0].local.as_str(),
                "msub" | "msup" | "msubsup" | "munder" | "mover" | "munderover"
            )
            || !head_children.first().is_some_and(|operator| {
                operator.namespace.as_deref() == Some(MATHML_NS)
                    && operator.local == "mo"
                    && is_nary_operator(&operator.text())
            })
        {
            return Ok(None);
        }
        let converted = mathml_element_to_expressions(
            children[0],
            &format!("{path}/*[1]"),
            diagnostics,
            nodes,
        )?;
        let [MathExpression::Nary(nary)] = converted.as_slice() else {
            return Ok(None);
        };
        if !nary.base.expressions.is_empty() {
            return Ok(None);
        }
        nary.clone()
    };
    nary.base = mathml_node_to_argument(children[1], &format!("{path}/*[2]"), diagnostics, nodes)?;
    Ok(Some(nary))
}

fn normalize_argument(argument: &mut MathArgument) {
    for expression in &mut argument.expressions {
        normalize_expression(expression);
    }
    let mut normalized: Vec<MathExpression> = Vec::with_capacity(argument.expressions.len());
    for expression in argument.expressions.drain(..) {
        let expression_is_fully_modelled = !expression.has_unsupported_content();
        let previous_is_fully_modelled = normalized
            .last()
            .is_some_and(|previous| !previous.has_unsupported_content());
        if let MathExpression::Run(run) = &expression
            && let Some(MathExpression::Run(previous)) = normalized.last_mut()
            && expression_is_fully_modelled
            && previous_is_fully_modelled
            && previous.properties == Default::default()
            && run.properties == Default::default()
        {
            previous.text.push_str(&run.text);
            continue;
        }
        normalized.push(expression);
    }
    argument.expressions = normalized;
}

fn normalize_expression(expression: &mut MathExpression) {
    match expression {
        MathExpression::Run(_) => {}
        MathExpression::Fraction(value) => {
            normalize_argument(&mut value.numerator);
            normalize_argument(&mut value.denominator);
        }
        MathExpression::Subscript(value) | MathExpression::Superscript(value) => {
            normalize_argument(&mut value.base);
            normalize_argument(&mut value.script);
        }
        MathExpression::SubSuperscript(value) => {
            normalize_argument(&mut value.base);
            normalize_argument(&mut value.subscript);
            normalize_argument(&mut value.superscript);
        }
        MathExpression::PreSubSuperscript(value) => {
            normalize_argument(&mut value.base);
            normalize_argument(&mut value.subscript);
            normalize_argument(&mut value.superscript);
        }
        MathExpression::Radical(value) => {
            normalize_argument(&mut value.degree);
            normalize_argument(&mut value.base);
        }
        MathExpression::Matrix(value) => {
            for row in &mut value.rows {
                for cell in &mut row.cells {
                    normalize_argument(cell);
                }
            }
        }
        MathExpression::LowerLimit(value) | MathExpression::UpperLimit(value) => {
            normalize_argument(&mut value.base);
            normalize_argument(&mut value.limit);
        }
        MathExpression::Nary(value) => {
            normalize_argument(&mut value.base);
            normalize_argument(&mut value.subscript);
            normalize_argument(&mut value.superscript);
        }
        MathExpression::Delimiter(value) => {
            for argument in &mut value.arguments {
                normalize_argument(argument);
            }
        }
        MathExpression::Accent(value) => normalize_argument(&mut value.base),
    }
}

fn validate_tree(argument: &MathArgument) -> std::result::Result<(), String> {
    fn visit(
        argument: &MathArgument,
        depth: usize,
        nodes: &mut usize,
        text: &mut usize,
    ) -> std::result::Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("equation tree exceeds the depth limit".to_owned());
        }
        for expression in &argument.expressions {
            *nodes = nodes
                .checked_add(1)
                .ok_or_else(|| "equation node count overflowed".to_owned())?;
            if *nodes > MAX_NODES {
                return Err("equation tree exceeds the node limit".to_owned());
            }
            match expression {
                MathExpression::Run(run) => {
                    *text = text
                        .checked_add(run.text.len())
                        .ok_or_else(|| "equation text size overflowed".to_owned())?;
                    if *text > MAX_TEXT_BYTES {
                        return Err("equation tree exceeds the text limit".to_owned());
                    }
                }
                MathExpression::Fraction(value) => {
                    visit(&value.numerator, depth + 1, nodes, text)?;
                    visit(&value.denominator, depth + 1, nodes, text)?;
                }
                MathExpression::Subscript(value) | MathExpression::Superscript(value) => {
                    visit(&value.base, depth + 1, nodes, text)?;
                    visit(&value.script, depth + 1, nodes, text)?;
                }
                MathExpression::SubSuperscript(value) => {
                    visit(&value.base, depth + 1, nodes, text)?;
                    visit(&value.subscript, depth + 1, nodes, text)?;
                    visit(&value.superscript, depth + 1, nodes, text)?;
                }
                MathExpression::PreSubSuperscript(value) => {
                    visit(&value.base, depth + 1, nodes, text)?;
                    visit(&value.subscript, depth + 1, nodes, text)?;
                    visit(&value.superscript, depth + 1, nodes, text)?;
                }
                MathExpression::Radical(value) => {
                    visit(&value.degree, depth + 1, nodes, text)?;
                    visit(&value.base, depth + 1, nodes, text)?;
                }
                MathExpression::Matrix(value) => {
                    if value.rows.is_empty() || value.rows.len() > MAX_ROWS {
                        return Err("equation matrix exceeds the row limit".to_owned());
                    }
                    let mut width = None;
                    for row in &value.rows {
                        if row.cells.is_empty() || row.cells.len() > MAX_COLUMNS {
                            return Err("equation matrix exceeds the column limit".to_owned());
                        }
                        if width
                            .replace(row.cells.len())
                            .is_some_and(|previous| previous != row.cells.len())
                        {
                            return Err("equation matrix is ragged".to_owned());
                        }
                        for cell in &row.cells {
                            visit(cell, depth + 1, nodes, text)?;
                        }
                    }
                }
                MathExpression::LowerLimit(value) | MathExpression::UpperLimit(value) => {
                    visit(&value.base, depth + 1, nodes, text)?;
                    visit(&value.limit, depth + 1, nodes, text)?;
                }
                MathExpression::Nary(value) => {
                    visit(&value.base, depth + 1, nodes, text)?;
                    visit(&value.subscript, depth + 1, nodes, text)?;
                    visit(&value.superscript, depth + 1, nodes, text)?;
                }
                MathExpression::Delimiter(value) => {
                    if value.arguments.is_empty() {
                        return Err("equation delimiter has no arguments".to_owned());
                    }
                    for child in &value.arguments {
                        visit(child, depth + 1, nodes, text)?;
                    }
                }
                MathExpression::Accent(value) => visit(&value.base, depth + 1, nodes, text)?,
            }
        }
        Ok(())
    }
    visit(argument, 0, &mut 0, &mut 0)
}

fn argument_has_direct_unsupported_content(argument: &MathArgument) -> bool {
    let mut direct = argument.clone();
    direct.expressions.clear();
    direct.has_unsupported_content()
}

fn expression_has_direct_unsupported_content(expression: &MathExpression) -> bool {
    let mut direct = expression.clone();
    match &mut direct {
        MathExpression::Run(_) => {}
        MathExpression::Fraction(value) => {
            value.numerator = MathArgument::default();
            value.denominator = MathArgument::default();
        }
        MathExpression::Subscript(value) | MathExpression::Superscript(value) => {
            value.base = MathArgument::default();
            value.script = MathArgument::default();
        }
        MathExpression::SubSuperscript(value) => {
            value.base = MathArgument::default();
            value.subscript = MathArgument::default();
            value.superscript = MathArgument::default();
        }
        MathExpression::PreSubSuperscript(value) => {
            value.base = MathArgument::default();
            value.subscript = MathArgument::default();
            value.superscript = MathArgument::default();
        }
        MathExpression::Radical(value) => {
            value.degree = MathArgument::default();
            value.base = MathArgument::default();
        }
        MathExpression::Matrix(value) => {
            for row in &mut value.rows {
                row.cells.clear();
            }
        }
        MathExpression::LowerLimit(value) | MathExpression::UpperLimit(value) => {
            value.base = MathArgument::default();
            value.limit = MathArgument::default();
        }
        MathExpression::Nary(value) => {
            value.base = MathArgument::default();
            value.subscript = MathArgument::default();
            value.superscript = MathArgument::default();
        }
        MathExpression::Delimiter(value) => value.arguments.clear(),
        MathExpression::Accent(value) => value.base = MathArgument::default(),
    }
    direct.has_unsupported_content()
}

fn write_mathml_argument(
    argument: &MathArgument,
    path: &str,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    if argument_has_direct_unsupported_content(argument) {
        diagnostics.push(path, "unmodelled OfficeMath argument content was discarded")?;
    }
    for (index, expression) in argument.expressions.iter().enumerate() {
        write_mathml_expression(
            expression,
            &format!("{path}/*[{}]", index + 1),
            output,
            diagnostics,
        )?;
    }
    Ok(())
}

fn run_has_unconverted_properties(run: &MathRun, normal_supported: bool) -> bool {
    let properties = &run.properties;
    properties
        .style
        .is_some_and(|style| style != MathStyle::Plain)
        || (properties.normal.is_some() && !(normal_supported && properties.normal == Some(true)))
        || properties.literal.is_some()
        || properties.script.is_some()
        || properties.break_before
        || properties.break_alignment.is_some()
}

fn write_mathml_expression(
    expression: &MathExpression,
    path: &str,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    if expression_has_direct_unsupported_content(expression) {
        diagnostics.push(path, "unmodelled OfficeMath content was discarded")?;
    }
    match expression {
        MathExpression::Run(run) => {
            if run_has_unconverted_properties(run, true) {
                diagnostics.push(
                    format!("{path}/rPr"),
                    "OfficeMath run properties were discarded",
                )?;
            }
            if !is_xml_1_0_text(&run.text) {
                diagnostics.push(
                    path,
                    "OfficeMath run contains a forbidden XML 1.0 character",
                )?;
                return Ok(());
            }
            if run.properties.normal == Some(true) && run.properties.style == Some(MathStyle::Plain)
            {
                let has_content = !run.text.trim_matches(' ').is_empty();
                let leading =
                    has_content && run.text.starts_with(' ') && !run.text.starts_with("  ");
                let trailing = has_content && run.text.ends_with(' ') && !run.text.ends_with("  ");
                if leading {
                    output.push_str("<mspace width=\"0.333em\"/>");
                }
                output.push_str("<mtext>");
                // One lexical boundary space is represented by one explicit marker.
                let text = if leading {
                    run.text.strip_prefix(' ').unwrap_or(&run.text)
                } else {
                    &run.text
                };
                let text = if trailing {
                    text.strip_suffix(' ').unwrap_or(text)
                } else {
                    text
                };
                output.push_str(&xml_escape(text));
                output.push_str("</mtext>");
                if trailing && (!leading || run.text.len() > 1) {
                    output.push_str("<mspace width=\"0.333em\"/>");
                }
            } else if run.properties.normal == Some(true)
                || run.properties.style == Some(MathStyle::Plain)
            {
                output.push_str("<mi mathvariant=\"normal\">");
                output.push_str(&xml_escape(&run.text));
                output.push_str("</mi>");
                if run.properties.normal == Some(true) && run.text != "lim" {
                    output.push_str("<mo>⁡</mo>");
                }
            } else {
                let mut part = String::new();
                for character in run.text.chars() {
                    if let Some(width) = math_space_width(character) {
                        if !part.is_empty() {
                            write!(output, "<mi>{}</mi>", xml_escape(&part))
                                .expect("String writes succeed");
                            part.clear();
                        }
                        write!(output, "<mspace width=\"{width}\"/>")
                            .expect("String writes succeed");
                    } else {
                        part.push(character);
                    }
                }
                if !part.is_empty() || run.text.is_empty() {
                    write!(output, "<mi>{}</mi>", xml_escape(&part))
                        .expect("String writes succeed");
                }
            }
        }
        MathExpression::Fraction(value) => {
            if !matches!(value.fraction_type, FractionType::Bar | FractionType::NoBar) {
                diagnostics.push(path, "non-bar OfficeMath fraction type was discarded")?;
            }
            output.push_str(if value.fraction_type == FractionType::NoBar {
                "<mfrac linethickness=\"0\"><mrow>"
            } else {
                "<mfrac><mrow>"
            });
            write_mathml_argument(&value.numerator, path, output, diagnostics)?;
            output.push_str("</mrow><mrow>");
            write_mathml_argument(&value.denominator, path, output, diagnostics)?;
            output.push_str("</mrow></mfrac>");
        }
        MathExpression::Subscript(value) | MathExpression::Superscript(value) => {
            if value.alignment.is_some() {
                diagnostics.push(path, "OfficeMath script alignment was discarded")?;
            }
            output.push_str(if matches!(expression, MathExpression::Subscript(_)) {
                "<msub><mrow>"
            } else {
                "<msup><mrow>"
            });
            write_mathml_argument(&value.base, path, output, diagnostics)?;
            output.push_str("</mrow><mrow>");
            write_mathml_argument(&value.script, path, output, diagnostics)?;
            output.push_str(if matches!(expression, MathExpression::Subscript(_)) {
                "</mrow></msub>"
            } else {
                "</mrow></msup>"
            });
        }
        MathExpression::SubSuperscript(value) => {
            if value.alignment.is_some() {
                diagnostics.push(path, "OfficeMath script alignment was discarded")?;
            }
            output.push_str("<msubsup><mrow>");
            write_mathml_argument(&value.base, path, output, diagnostics)?;
            output.push_str("</mrow><mrow>");
            write_mathml_argument(&value.subscript, path, output, diagnostics)?;
            output.push_str("</mrow><mrow>");
            write_mathml_argument(&value.superscript, path, output, diagnostics)?;
            output.push_str("</mrow></msubsup>");
        }
        MathExpression::PreSubSuperscript(value) => {
            if value.alignment.is_some() {
                diagnostics.push(path, "OfficeMath script alignment was discarded")?;
            }
            output.push_str("<mmultiscripts><mrow>");
            write_mathml_argument(&value.base, path, output, diagnostics)?;
            output.push_str("</mrow><mprescripts/><mrow>");
            write_mathml_argument(&value.subscript, path, output, diagnostics)?;
            output.push_str("</mrow><mrow>");
            write_mathml_argument(&value.superscript, path, output, diagnostics)?;
            output.push_str("</mrow></mmultiscripts>");
        }
        MathExpression::Radical(value) => {
            if value.hide_degree != value.degree.expressions.is_empty() {
                diagnostics.push(path, "OfficeMath radical degree visibility was discarded")?;
            }
            if value.degree.expressions.is_empty() {
                output.push_str("<msqrt>");
                write_mathml_argument(&value.base, path, output, diagnostics)?;
                output.push_str("</msqrt>");
            } else {
                output.push_str("<mroot><mrow>");
                write_mathml_argument(&value.base, path, output, diagnostics)?;
                output.push_str("</mrow><mrow>");
                write_mathml_argument(&value.degree, path, output, diagnostics)?;
                output.push_str("</mrow></mroot>");
            }
        }
        MathExpression::Matrix(value) => {
            if value.properties != Default::default() {
                diagnostics.push(path, "OfficeMath matrix properties were discarded")?;
            }
            output.push_str("<mtable>");
            for row in &value.rows {
                output.push_str("<mtr>");
                for cell in &row.cells {
                    output.push_str("<mtd>");
                    write_mathml_argument(cell, path, output, diagnostics)?;
                    output.push_str("</mtd>");
                }
                output.push_str("</mtr>");
            }
            output.push_str("</mtable>");
        }
        MathExpression::LowerLimit(value) | MathExpression::UpperLimit(value) => {
            output.push_str(if matches!(expression, MathExpression::LowerLimit(_)) {
                "<munder><mrow>"
            } else {
                "<mover><mrow>"
            });
            write_mathml_argument(&value.base, path, output, diagnostics)?;
            output.push_str("</mrow><mrow>");
            write_mathml_argument(&value.limit, path, output, diagnostics)?;
            output.push_str(if matches!(expression, MathExpression::LowerLimit(_)) {
                "</mrow></munder>"
            } else {
                "</mrow></mover>"
            });
        }
        MathExpression::Nary(value) => write_mathml_nary(value, path, output, diagnostics)?,
        MathExpression::Delimiter(value) => {
            if value.grow.is_some() && canonical_binomial(value).is_none() {
                diagnostics.push(path, "OfficeMath delimiter growth setting was discarded")?;
            }
            if value.begin_character.chars().count() > 1
                || value.end_character.chars().count() > 1
                || value.separator_character.chars().count() > 1
            {
                diagnostics.push(path, "non-scalar OfficeMath delimiter was discarded")?;
                return Ok(());
            }
            if !is_xml_1_0_text(&value.begin_character)
                || !is_xml_1_0_text(&value.end_character)
                || !is_xml_1_0_text(&value.separator_character)
            {
                diagnostics.push(
                    path,
                    "OfficeMath delimiter contains a forbidden XML 1.0 character",
                )?;
                return Ok(());
            }
            write!(
                output,
                "<mrow><mo fence=\"true\" stretchy=\"true\">{}</mo>",
                xml_escape(&value.begin_character)
            )
            .expect("String writes are infallible");
            for (index, argument) in value.arguments.iter().enumerate() {
                if index != 0 {
                    write!(
                        output,
                        "<mo separator=\"true\">{}</mo>",
                        xml_escape(&value.separator_character)
                    )
                    .expect("String writes are infallible");
                }
                write_mathml_argument(argument, path, output, diagnostics)?;
            }
            write!(
                output,
                "<mo fence=\"true\" stretchy=\"true\">{}</mo></mrow>",
                xml_escape(&value.end_character)
            )
            .expect("String writes are infallible");
        }
        MathExpression::Accent(value) => {
            if value.character.chars().count() != 1 {
                diagnostics.push(path, "non-scalar OfficeMath accent was discarded")?;
                return Ok(());
            }
            if !is_xml_1_0_text(&value.character) {
                diagnostics.push(
                    path,
                    "OfficeMath accent contains a forbidden XML 1.0 character",
                )?;
                return Ok(());
            }
            output.push_str("<mover accent=\"true\"><mrow>");
            write_mathml_argument(&value.base, path, output, diagnostics)?;
            write!(
                output,
                "</mrow><mo>{}</mo></mover>",
                xml_escape(&value.character)
            )
            .expect("String writes are infallible");
        }
    }
    Ok(())
}

fn write_mathml_nary(
    value: &MathNary,
    path: &str,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    if !is_nary_operator(&value.character) {
        diagnostics.push(path, "unsupported OfficeMath n-ary operator was discarded")?;
        return Ok(());
    }
    if value.grow.is_some() || value.limit_location.is_some() {
        diagnostics.push(path, "OfficeMath n-ary layout properties were discarded")?;
    }
    let sub = !value.hide_subscript && !value.subscript.expressions.is_empty();
    let sup = !value.hide_superscript && !value.superscript.expressions.is_empty();
    if value.hide_subscript && !value.subscript.expressions.is_empty() {
        diagnostics.push(path, "hidden OfficeMath n-ary subscript was discarded")?;
    } else if !value.hide_subscript && value.subscript.expressions.is_empty() {
        diagnostics.push(path, "OfficeMath n-ary subscript visibility was discarded")?;
    }
    if value.hide_superscript && !value.superscript.expressions.is_empty() {
        diagnostics.push(path, "hidden OfficeMath n-ary superscript was discarded")?;
    } else if !value.hide_superscript && value.superscript.expressions.is_empty() {
        diagnostics.push(
            path,
            "OfficeMath n-ary superscript visibility was discarded",
        )?;
    }
    output.push_str("<mrow>");
    if sub && sup {
        output.push_str("<munderover><mo largeop=\"true\">");
    } else if sub {
        output.push_str("<munder><mo largeop=\"true\">");
    } else if sup {
        output.push_str("<mover><mo largeop=\"true\">");
    }
    if sub || sup {
        output.push_str(&xml_escape(&value.character));
        output.push_str("</mo><mrow>");
        if sub {
            write_mathml_argument(&value.subscript, path, output, diagnostics)?;
        } else {
            write_mathml_argument(&value.superscript, path, output, diagnostics)?;
        }
        output.push_str("</mrow>");
        if sub && sup {
            output.push_str("<mrow>");
            write_mathml_argument(&value.superscript, path, output, diagnostics)?;
            output.push_str("</mrow></munderover>");
        } else if sub {
            output.push_str("</munder>");
        } else {
            output.push_str("</mover>");
        }
    } else {
        write!(
            output,
            "<mo largeop=\"true\">{}</mo>",
            xml_escape(&value.character)
        )
        .expect("String writes are infallible");
    }
    output.push_str("<mrow>");
    write_mathml_argument(&value.base, path, output, diagnostics)?;
    output.push_str("</mrow></mrow>");
    Ok(())
}

fn xml_escape(value: &str) -> String {
    quick_xml::escape::escape(value).into_owned()
}

fn is_xml_1_0_text(value: &str) -> bool {
    value.chars().all(|character| {
        matches!(character, '\u{9}' | '\u{a}' | '\u{d}')
            || ('\u{20}'..='\u{d7ff}').contains(&character)
            || ('\u{e000}'..='\u{fffd}').contains(&character)
            || ('\u{10000}'..='\u{10ffff}').contains(&character)
    })
}

fn is_nary_operator(value: &str) -> bool {
    matches!(value, "∑" | "∏" | "∐" | "∫" | "∬" | "∭" | "∮")
}

struct LatexParser<'a> {
    input: &'a str,
    offset: usize,
    depth: usize,
    nodes: usize,
    text_bytes: usize,
    tokens: usize,
    diagnostics: Diagnostics,
}

impl<'a> LatexParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input,
            offset: 0,
            depth: 0,
            nodes: 0,
            text_bytes: 0,
            tokens: 0,
            diagnostics: Diagnostics::new(),
        }
    }

    fn nested(input: &'a str, depth: usize) -> Self {
        let mut parser = Self::new(input);
        parser.depth = depth;
        parser
    }

    fn parse_argument(&mut self, terminator: Option<char>) -> Result<MathArgument> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error("LaTeX input exceeds the depth limit"));
        }
        let mut expressions = Vec::new();
        loop {
            self.skip_whitespace();
            if self.at_end() || terminator.is_some_and(|value| self.peek() == Some(value)) {
                break;
            }
            if latex_command_at(self.input, self.offset)
                .is_some_and(|(command, _)| matches!(command, "right" | "end"))
            {
                break;
            }
            let atom = self.parse_complete_atom()?;
            expressions.extend(atom.expressions);
        }
        if let Some(value) = terminator {
            if self.peek() != Some(value) {
                return Err(self.error(format!("missing closing {value}")));
            }
            self.bump();
        }
        self.depth -= 1;
        let mut argument = MathArgument::new(expressions);
        normalize_argument(&mut argument);
        Ok(argument)
    }

    fn parse_atom(&mut self) -> Result<MathArgument> {
        self.bump_token()?;
        let start = self.offset;
        match self.peek() {
            Some('{') => {
                self.bump();
                self.parse_argument(Some('}'))
            }
            Some('\\') => self.parse_command(),
            Some('&') => {
                self.bump();
                Err(self.error("matrix separator outside a matrix"))
            }
            Some('}') | Some('[') | Some(']') => Err(self.error("unexpected LaTeX delimiter")),
            Some(_) => {
                // TeX scripts bind to one atom, even beside an unscripted run.
                let character = self.bump().expect("the atom has a character");
                let text = character.to_string();
                self.add_text(text.len())?;
                self.add_node()?;
                Ok(MathArgument::text(text))
            }
            None => Err(math_error(format!("missing LaTeX atom at byte {start}"))),
        }
    }

    fn parse_complete_atom(&mut self) -> Result<MathArgument> {
        let atom_offset = self.offset;
        let mut atom = self.parse_atom()?;
        self.skip_whitespace();
        if matches!(atom.expressions.as_slice(), [MathExpression::Nary(_)])
            && let Some((command, end)) = latex_command_at(self.input, self.offset)
            && matches!(command, "limits" | "nolimits")
        {
            self.diagnostics.push(
                format!("byte:{}", self.offset),
                format!("LaTeX \\{command} n-ary placement was discarded"),
            )?;
            self.bump_token()?;
            self.offset = end;
            self.skip_whitespace();
        }
        if matches!(atom.expressions.as_slice(), [MathExpression::Nary(_)])
            && self.input[self.offset..].starts_with("{}")
        {
            self.bump_token()?;
            self.bump();
            self.bump();
        }
        let (subscript, superscript) = self.parse_following_scripts()?;
        atom = self.attach_following_scripts(atom, subscript, superscript, atom_offset)?;
        if let Some(MathExpression::Nary(nary)) = atom.expressions.first_mut()
            && nary.base.expressions.is_empty()
            && !self.at_end()
        {
            nary.base = self.parse_complete_atom()?;
        }
        Ok(atom)
    }

    fn parse_following_scripts(&mut self) -> Result<(Option<MathArgument>, Option<MathArgument>)> {
        let mut subscript = None;
        let mut superscript = None;
        loop {
            self.skip_whitespace();
            match self.peek() {
                Some('_') => {
                    self.bump();
                    if subscript.is_some() {
                        return Err(self.error("duplicate subscript"));
                    }
                    subscript = Some(self.parse_required_argument()?);
                }
                Some('^') => {
                    self.bump();
                    if superscript.is_some() {
                        return Err(self.error("duplicate superscript"));
                    }
                    superscript = Some(self.parse_required_argument()?);
                }
                _ => return Ok((subscript, superscript)),
            }
        }
    }

    fn attach_following_scripts(
        &mut self,
        atom: MathArgument,
        subscript: Option<MathArgument>,
        superscript: Option<MathArgument>,
        offset: usize,
    ) -> Result<MathArgument> {
        if subscript.is_none() && superscript.is_none() {
            return Ok(atom);
        }
        if !atom.expressions.is_empty() {
            return self.apply_scripts(atom, subscript, superscript, offset);
        }
        let base = self.parse_complete_atom()?;
        self.add_node()?;
        Ok(MathArgument::new(vec![MathExpression::PreSubSuperscript(
            MathPreSubSuperscript::new(
                base,
                subscript.unwrap_or_default(),
                superscript.unwrap_or_default(),
            ),
        )]))
    }

    fn parse_command(&mut self) -> Result<MathArgument> {
        let start = self.offset;
        self.bump();
        let command = self.read_command_name();
        match command.as_str() {
            "frac" | "dfrac" | "tfrac" | "binom" => {
                let numerator = self.parse_required_group()?;
                let denominator = self.parse_required_group()?;
                self.add_node()?;
                let mut fraction = MathFraction::new(numerator, denominator);
                let expression = if command == "binom" {
                    fraction.fraction_type = FractionType::NoBar;
                    let mut delimiter = MathDelimiter::new(
                        "(",
                        ")",
                        vec![MathArgument::new(vec![MathExpression::Fraction(fraction)])],
                    );
                    delimiter.separator_character.clear();
                    delimiter.grow = Some(true);
                    self.add_node()?;
                    MathExpression::Delimiter(delimiter)
                } else {
                    // OfficeMath canonicalizes forced TeX styles to a stacked bar.
                    MathExpression::Fraction(fraction)
                };
                Ok(MathArgument::new(vec![expression]))
            }
            "sqrt" => {
                self.skip_whitespace();
                let degree = if self.peek() == Some('[') {
                    self.bump();
                    Some(self.parse_argument(Some(']'))?)
                } else {
                    None
                };
                let base = self.parse_required_group()?;
                self.add_node()?;
                Ok(MathArgument::new(vec![MathExpression::Radical(
                    match degree {
                        Some(degree) => MathRadical::with_degree(degree, base),
                        None => MathRadical::new(base),
                    },
                )]))
            }
            "sum" | "prod" | "coprod" | "int" | "iint" | "iiint" | "oint" => {
                self.add_node()?;
                Ok(MathArgument::new(vec![MathExpression::Nary(
                    MathNary::new(latex_nary_character(&command), MathArgument::default()),
                )]))
            }
            "left" => self.parse_left_right(start),
            "hat" | "widehat" | "bar" | "overline" | "vec" | "overrightarrow" | "tilde"
            | "widetilde" | "dot" | "ddot" => {
                let base = self.parse_required_argument()?;
                self.add_node()?;
                Ok(MathArgument::new(vec![MathExpression::Accent(
                    MathAccent::new(latex_accent_character(&command), base),
                )]))
            }
            "underset" | "overset" => {
                let limit = self.parse_required_group()?;
                let base = self.parse_required_group()?;
                self.add_node()?;
                Ok(MathArgument::new(vec![if command == "underset" {
                    MathExpression::LowerLimit(MathLimit::new(base, limit))
                } else {
                    MathExpression::UpperLimit(MathLimit::new(base, limit))
                }]))
            }
            "begin" => self.parse_matrix_environment(start),
            "operatorname" | "mathrm" | "text" => {
                let text = self.read_literal_group()?;
                self.add_node()?;
                let mut run = MathRun::new(text);
                if command != "mathrm" {
                    run.properties.normal = Some(true);
                }
                if command != "operatorname" {
                    run.properties.style = Some(MathStyle::Plain);
                }
                Ok(MathArgument::new(vec![run.into()]))
            }
            "sin" | "log" | "ln" | "exp" => {
                self.add_text(command.len())?;
                self.add_node()?;
                let mut run = MathRun::new(command);
                run.properties.normal = Some(true);
                Ok(MathArgument::new(vec![run.into()]))
            }
            name if latex_symbol(name).is_some() => {
                let character = latex_symbol(name).expect("matched finite symbol");
                self.add_text(character.len_utf8())?;
                self.add_node()?;
                let mut run = MathRun::new(character.to_string());
                if !matches!(
                    character,
                    'α' | 'β' | 'θ' | 'λ' | 'μ' | 'σ' | 'ω' | 'ε' | 'ϕ' | 'φ' | '∂'
                ) {
                    run.properties.style = Some(MathStyle::Plain);
                }
                Ok(MathArgument::new(vec![run.into()]))
            }
            "pi" | "to" | "rightarrow" | "infty" | "," | ";" | "quad" | "lim" => {
                let text = match command.as_str() {
                    "pi" => "π",
                    "to" | "rightarrow" => "→",
                    "infty" => "∞",
                    "," => "\u{2009}",
                    ";" => "\u{2004}",
                    "quad" => "\u{2001}",
                    _ => "lim",
                };
                self.add_text(text.len())?;
                self.add_node()?;
                let mut run = MathRun::new(text);
                if command == "lim" {
                    run.properties.normal = Some(true);
                }
                Ok(MathArgument::new(vec![run.into()]))
            }
            "!" | " " => {
                self.diagnostics.push(
                    format!("byte:{start}"),
                    format!("unsupported LaTeX spacing \\{command} was discarded"),
                )?;
                Ok(MathArgument::default())
            }
            "{" | "}" | "_" | "^" | "#" | "$" | "%" | "&" | "\\" => {
                self.add_text(command.len())?;
                self.add_node()?;
                Ok(MathArgument::text(command))
            }
            "lbrack" => {
                self.add_text(1)?;
                self.add_node()?;
                Ok(MathArgument::text("["))
            }
            "rbrack" => {
                self.add_text(1)?;
                self.add_node()?;
                Ok(MathArgument::text("]"))
            }
            "backslash" => {
                self.add_text(1)?;
                self.add_node()?;
                Ok(MathArgument::text("\\"))
            }
            _ => {
                self.diagnostics.push(
                    format!("byte:{start}"),
                    format!("unsupported LaTeX command \\{command} was discarded"),
                )?;
                self.skip_optional_lossy_argument()?;
                Ok(MathArgument::default())
            }
        }
    }

    fn parse_left_right(&mut self, start: usize) -> Result<MathArgument> {
        self.skip_whitespace();
        let begin = self.read_delimiter()?;
        let content_start = self.offset;
        let Some(relative_end) = find_matching_right(&self.input[content_start..]) else {
            return Err(math_error(format!("missing \\right for byte {start}")));
        };
        let content_end = content_start + relative_end;
        let content = &self.input[content_start..content_end];
        let (parts, separator) = split_latex_fenced(content)?;
        let mut arguments = Vec::new();
        for (part_offset, part) in parts {
            let mut nested = LatexParser::nested(part, self.depth);
            let argument = nested.parse_argument(None)?;
            nested.skip_whitespace();
            if !nested.at_end() {
                return Err(math_error(format!(
                    "unexpected trailing nested LaTeX input at byte {}",
                    content_start + part_offset + nested.offset
                )));
            }
            arguments.push(argument);
            self.absorb_nested(nested, content_start + part_offset)?;
        }
        self.offset = content_end + "\\right".len();
        self.skip_whitespace();
        let end = self.read_delimiter()?;
        self.add_node()?;
        let mut delimiter = MathDelimiter::new(begin, end, arguments);
        if delimiter.arguments.len() > 1 {
            delimiter.separator_character = separator;
        }
        Ok(MathArgument::new(vec![MathExpression::Delimiter(
            delimiter,
        )]))
    }

    fn parse_matrix_environment(&mut self, start: usize) -> Result<MathArgument> {
        let environment = self.read_raw_group()?;
        let body_start = self.offset;
        let Some((relative_end, relative_after)) =
            find_matching_environment(&self.input[body_start..], &environment)?
        else {
            return Err(math_error(format!(
                "missing \\end{{{environment}}} for byte {start}"
            )));
        };
        let body_end = body_start + relative_end;
        let body = &self.input[body_start..body_end];
        self.offset = body_start + relative_after;
        if !matches!(environment.as_str(), "matrix" | "pmatrix" | "bmatrix") {
            self.diagnostics.push(
                format!("byte:{start}"),
                format!("unsupported LaTeX environment {environment} was discarded"),
            )?;
            return Ok(MathArgument::default());
        }
        let mut rows = Vec::new();
        let mut width = None;
        for row in split_latex_matrix(body)? {
            if rows.len() >= MAX_ROWS {
                return Err(self.error("LaTeX matrix exceeds the row limit"));
            }
            let mut cells = Vec::new();
            for (cell_offset, cell_text) in row {
                if cells.len() >= MAX_COLUMNS {
                    return Err(self.error("LaTeX matrix exceeds the column limit"));
                }
                let mut nested = LatexParser::nested(cell_text, self.depth);
                let cell = nested.parse_argument(None)?;
                nested.skip_whitespace();
                if !nested.at_end() {
                    return Err(math_error(format!(
                        "unexpected trailing nested LaTeX input at byte {}",
                        body_start + cell_offset + nested.offset
                    )));
                }
                cells.push(cell);
                self.absorb_nested(nested, body_start + cell_offset)?;
            }
            if width
                .replace(cells.len())
                .is_some_and(|value| value != cells.len())
            {
                return Err(self.error("LaTeX matrix is ragged"));
            }
            rows.push(MathMatrixRow::new(cells));
        }
        if rows.is_empty() || rows.iter().any(|row| row.cells.is_empty()) {
            return Err(self.error("LaTeX matrix must contain cells"));
        }
        self.add_node()?;
        let matrix = MathExpression::Matrix(MathMatrix::new(rows));
        let expression = match environment.as_str() {
            "pmatrix" => MathExpression::Delimiter(MathDelimiter::new(
                "(",
                ")",
                vec![MathArgument::new(vec![matrix])],
            )),
            "bmatrix" => MathExpression::Delimiter(MathDelimiter::new(
                "[",
                "]",
                vec![MathArgument::new(vec![matrix])],
            )),
            _ => matrix,
        };
        Ok(MathArgument::new(vec![expression]))
    }

    fn apply_scripts(
        &mut self,
        base: MathArgument,
        subscript: Option<MathArgument>,
        superscript: Option<MathArgument>,
        offset: usize,
    ) -> Result<MathArgument> {
        self.add_node()?;
        let expression = if let [MathExpression::Nary(nary)] = base.expressions.as_slice() {
            let mut nary = nary.clone();
            if let Some(subscript) = subscript {
                nary.subscript = subscript;
                nary.hide_subscript = false;
            }
            if let Some(superscript) = superscript {
                nary.superscript = superscript;
                nary.hide_superscript = false;
            }
            MathExpression::Nary(nary)
        } else {
            match (subscript, superscript) {
                (Some(subscript), Some(superscript)) => MathExpression::SubSuperscript(
                    MathSubSuperscript::new(base, subscript, superscript),
                ),
                (Some(script), None) => MathExpression::Subscript(MathScript::new(base, script)),
                (None, Some(script)) => MathExpression::Superscript(MathScript::new(base, script)),
                (None, None) => return Err(math_error(format!("empty scripts at byte {offset}"))),
            }
        };
        Ok(MathArgument::new(vec![expression]))
    }

    fn parse_required_group(&mut self) -> Result<MathArgument> {
        self.skip_whitespace();
        if self.peek() != Some('{') {
            return Err(self.error("expected a braced argument"));
        }
        self.bump();
        self.parse_argument(Some('}'))
    }

    fn parse_required_argument(&mut self) -> Result<MathArgument> {
        self.skip_whitespace();
        self.parse_atom()
    }

    fn read_literal_group(&mut self) -> Result<String> {
        if self.depth >= MAX_DEPTH {
            return Err(self.error("LaTeX literal group exceeds the depth limit"));
        }
        self.skip_whitespace();
        if self.bump() != Some('{') {
            return Err(self.error("expected a braced literal"));
        }
        let mut value = String::new();
        loop {
            self.bump_token()?;
            match self.bump() {
                Some('}') => {
                    if !is_xml_1_0_text(&value) {
                        return Err(
                            self.error("literal group contains a forbidden XML 1.0 character")
                        );
                    }
                    return Ok(value);
                }
                Some('{') => return Err(self.error("nested literal groups are unsupported")),
                Some('\\') => match self.bump() {
                    Some(character @ ('{' | '}' | '\\' | '_' | '^' | '#' | '$' | '%' | '&')) => {
                        self.add_text(character.len_utf8())?;
                        value.push(character);
                    }
                    _ => return Err(self.error("unsupported command in literal group")),
                },
                Some(character) => {
                    self.add_text(character.len_utf8())?;
                    value.push(character);
                }
                None => return Err(self.error("unterminated literal group")),
            }
        }
    }

    fn read_raw_group(&mut self) -> Result<String> {
        self.skip_whitespace();
        if self.peek() != Some('{') {
            return Err(self.error("expected a braced name"));
        }
        self.bump();
        let start = self.offset;
        while !self.at_end() && self.peek() != Some('}') {
            self.bump();
        }
        if self.peek() != Some('}') {
            return Err(self.error("unterminated braced name"));
        }
        let value = self.input[start..self.offset].to_owned();
        self.bump();
        Ok(value)
    }

    fn read_delimiter(&mut self) -> Result<String> {
        if self.peek() == Some('\\') {
            self.bump();
            let name = self.read_command_name();
            let value = match name.as_str() {
                "langle" => "⟨",
                "rangle" => "⟩",
                "lbrace" => "{",
                "rbrace" => "}",
                "vert" | "|" => "|",
                "backslash" => "\\",
                "." => "",
                _ => return Err(self.error("unsupported LaTeX delimiter")),
            };
            if self.input[self.offset..].starts_with("{}") {
                self.bump();
                self.bump();
            }
            return Ok(value.to_owned());
        }
        self.bump()
            .map(|value| {
                if value == '.' {
                    String::new()
                } else {
                    value.to_string()
                }
            })
            .ok_or_else(|| self.error("missing LaTeX delimiter"))
    }

    fn read_command_name(&mut self) -> String {
        let start = self.offset;
        if self.peek().is_some_and(char::is_alphabetic) {
            while self.peek().is_some_and(char::is_alphabetic) {
                self.bump();
            }
        } else {
            self.bump();
        }
        self.input[start..self.offset].to_owned()
    }

    fn skip_optional_lossy_argument(&mut self) -> Result<()> {
        self.skip_whitespace();
        if self.peek() == Some('[') {
            self.bump();
            let _ = self.parse_argument(Some(']'))?;
        }
        self.skip_whitespace();
        if self.peek() == Some('{') {
            self.bump();
            let _ = self.parse_argument(Some('}'))?;
        }
        Ok(())
    }

    fn add_node(&mut self) -> Result<()> {
        self.nodes = self
            .nodes
            .checked_add(1)
            .ok_or_else(|| self.error("LaTeX node count overflowed"))?;
        if self.nodes > MAX_NODES {
            return Err(self.error("LaTeX input exceeds the node limit"));
        }
        Ok(())
    }

    fn absorb_nested(&mut self, nested: LatexParser<'_>, base: usize) -> Result<()> {
        self.nodes = self
            .nodes
            .checked_add(nested.nodes)
            .ok_or_else(|| self.error("LaTeX node count overflowed"))?;
        self.tokens = self
            .tokens
            .checked_add(nested.tokens)
            .ok_or_else(|| self.error("LaTeX token count overflowed"))?;
        self.text_bytes = self
            .text_bytes
            .checked_add(nested.text_bytes)
            .ok_or_else(|| self.error("LaTeX text size overflowed"))?;
        if self.nodes > MAX_NODES || self.tokens > MAX_EVENTS || self.text_bytes > MAX_TEXT_BYTES {
            return Err(self.error("nested LaTeX input exceeds a conversion limit"));
        }
        for diagnostic in nested.diagnostics.values {
            self.diagnostics.push(
                format!("byte:{}", base + diagnostic_byte(&diagnostic.path)),
                diagnostic.message,
            )?;
        }
        Ok(())
    }

    fn add_text(&mut self, bytes: usize) -> Result<()> {
        self.text_bytes = self
            .text_bytes
            .checked_add(bytes)
            .ok_or_else(|| self.error("LaTeX text size overflowed"))?;
        if self.text_bytes > MAX_TEXT_BYTES {
            return Err(self.error("LaTeX input exceeds the text limit"));
        }
        Ok(())
    }

    fn bump_token(&mut self) -> Result<()> {
        self.tokens = self
            .tokens
            .checked_add(1)
            .ok_or_else(|| self.error("LaTeX token count overflowed"))?;
        if self.tokens > MAX_EVENTS {
            return Err(self.error("LaTeX input exceeds the token limit"));
        }
        Ok(())
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn peek(&self) -> Option<char> {
        self.input[self.offset..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let value = self.peek()?;
        self.offset += value.len_utf8();
        Some(value)
    }

    fn at_end(&self) -> bool {
        self.offset == self.input.len()
    }

    fn error(&self, message: impl Into<String>) -> Error {
        math_error(format!("{} at byte {}", message.into(), self.offset))
    }
}

fn diagnostic_byte(path: &str) -> usize {
    path.strip_prefix("byte:")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn latex_command_at(input: &str, offset: usize) -> Option<(&str, usize)> {
    if input.get(offset..)?.chars().next()? != '\\' {
        return None;
    }
    let start = offset + 1;
    let first = input.get(start..)?.chars().next()?;
    let mut end = start + first.len_utf8();
    if first.is_alphabetic() {
        while let Some(character) = input.get(end..)?.chars().next() {
            if !character.is_alphabetic() {
                break;
            }
            end += character.len_utf8();
        }
    }
    Some((&input[start..end], end))
}

fn raw_group_at(input: &str, offset: usize) -> Option<(&str, usize)> {
    if input.get(offset..)?.chars().next()? != '{' {
        return None;
    }
    let start = offset + 1;
    let relative_end = input.get(start..)?.find('}')?;
    let end = start + relative_end;
    Some((&input[start..end], end + 1))
}

fn delimiter_at(input: &str, offset: usize) -> Option<(String, usize)> {
    let character = input.get(offset..)?.chars().next()?;
    if character != '\\' {
        return Some((
            if character == '.' {
                String::new()
            } else {
                character.to_string()
            },
            offset + character.len_utf8(),
        ));
    }
    let (command, end) = latex_command_at(input, offset)?;
    let value = match command {
        "langle" => "⟨",
        "rangle" => "⟩",
        "lbrace" => "{",
        "rbrace" => "}",
        "vert" | "|" => "|",
        "backslash" => "\\",
        "." => "",
        _ => return None,
    };
    let after = if input[end..].starts_with("{}") {
        end + 2
    } else {
        end
    };
    Some((value.to_owned(), after))
}

fn find_matching_right(input: &str) -> Option<usize> {
    let mut depth = 0_usize;
    let mut offset = 0_usize;
    while offset < input.len() {
        if let Some((command, end)) = latex_command_at(input, offset) {
            match command {
                "left" => {
                    depth += 1;
                    offset = delimiter_at(input, end)?.1;
                }
                "right" => {
                    if depth == 0 {
                        return Some(offset);
                    }
                    depth -= 1;
                    offset = delimiter_at(input, end)?.1;
                }
                _ => offset = end,
            }
        } else {
            offset += input[offset..].chars().next()?.len_utf8();
        }
    }
    None
}

fn split_latex_fenced(input: &str) -> Result<(Vec<(usize, &str)>, String)> {
    let mut brace_depth = 0_usize;
    let mut delimiter_depth = 0_usize;
    let mut environment_depth = 0_usize;
    let mut parts = Vec::new();
    let mut start = 0_usize;
    let mut separator = None::<String>;
    let mut offset = 0_usize;
    while offset < input.len() {
        if let Some((command, end)) = latex_command_at(input, offset) {
            match command {
                "begin" | "end" => {
                    let Some((_, after)) = raw_group_at(input, end) else {
                        return Err(math_error(
                            "LaTeX environment command requires a braced name",
                        ));
                    };
                    if command == "begin" {
                        environment_depth += 1;
                    } else {
                        environment_depth = environment_depth.saturating_sub(1);
                    }
                    offset = after;
                }
                "left" => {
                    delimiter_depth += 1;
                    offset = delimiter_at(input, end)
                        .map(|(_, after)| after)
                        .ok_or_else(|| math_error("missing nested LaTeX left delimiter"))?;
                }
                "right" => {
                    delimiter_depth = delimiter_depth.saturating_sub(1);
                    offset = delimiter_at(input, end)
                        .map(|(_, after)| after)
                        .ok_or_else(|| math_error("missing nested LaTeX right delimiter"))?;
                }
                "middle" if brace_depth == 0 && delimiter_depth == 0 && environment_depth == 0 => {
                    let (value, after) = delimiter_at(input, end)
                        .ok_or_else(|| math_error("missing LaTeX middle delimiter"))?;
                    if separator.as_ref().is_some_and(|current| current != &value) {
                        return Err(math_error(
                            "mixed LaTeX delimiter separators are unsupported",
                        ));
                    }
                    separator.get_or_insert(value);
                    parts.push((start, &input[start..offset]));
                    start = after;
                    offset = after;
                }
                _ => offset = end,
            }
            continue;
        }
        let character = input[offset..]
            .chars()
            .next()
            .ok_or_else(|| math_error("invalid LaTeX delimiter scan offset"))?;
        match character {
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            ',' | '|' if brace_depth == 0 && delimiter_depth == 0 && environment_depth == 0 => {
                let value = character.to_string();
                if separator.as_ref().is_some_and(|current| current != &value) {
                    return Err(math_error(
                        "mixed LaTeX delimiter separators are unsupported",
                    ));
                }
                separator.get_or_insert(value);
                parts.push((start, &input[start..offset]));
                start = offset + character.len_utf8();
            }
            _ => {}
        }
        offset += character.len_utf8();
    }
    parts.push((start, &input[start..]));
    Ok((parts, separator.unwrap_or_else(|| "|".to_owned())))
}

fn find_matching_environment(input: &str, outer: &str) -> Result<Option<(usize, usize)>> {
    let mut stack = Vec::<&str>::new();
    let mut offset = 0_usize;
    while offset < input.len() {
        let Some((command, end)) = latex_command_at(input, offset) else {
            offset += input[offset..]
                .chars()
                .next()
                .ok_or_else(|| math_error("invalid LaTeX environment scan offset"))?
                .len_utf8();
            continue;
        };
        if matches!(command, "begin" | "end") {
            let Some((environment, after)) = raw_group_at(input, end) else {
                return Err(math_error(
                    "LaTeX environment command requires a braced name",
                ));
            };
            if command == "begin" {
                stack.push(environment);
            } else if let Some(expected) = stack.pop() {
                if environment != expected {
                    return Err(math_error("mismatched nested LaTeX environment"));
                }
            } else if environment == outer {
                return Ok(Some((offset, after)));
            } else {
                return Err(math_error("mismatched LaTeX environment close"));
            }
            offset = after;
        } else {
            offset = end;
        }
    }
    Ok(None)
}

fn split_latex_matrix(input: &str) -> Result<Vec<Vec<(usize, &str)>>> {
    let mut rows = vec![Vec::new()];
    let mut cell_start = 0_usize;
    let mut brace_depth = 0_usize;
    let mut delimiter_depth = 0_usize;
    let mut environment_depth = 0_usize;
    let mut offset = 0_usize;
    while offset < input.len() {
        if let Some((command, end)) = latex_command_at(input, offset) {
            match command {
                "begin" | "end" => {
                    let (_, after) = raw_group_at(input, end)
                        .ok_or_else(|| math_error("matrix environment name is not braced"))?;
                    if command == "begin" {
                        environment_depth += 1;
                    } else {
                        environment_depth = environment_depth.saturating_sub(1);
                    }
                    offset = after;
                }
                "left" => {
                    delimiter_depth += 1;
                    offset = delimiter_at(input, end)
                        .map(|(_, after)| after)
                        .ok_or_else(|| math_error("matrix left delimiter is missing"))?;
                }
                "right" => {
                    delimiter_depth = delimiter_depth.saturating_sub(1);
                    offset = delimiter_at(input, end)
                        .map(|(_, after)| after)
                        .ok_or_else(|| math_error("matrix right delimiter is missing"))?;
                }
                "\\" if brace_depth == 0 && delimiter_depth == 0 && environment_depth == 0 => {
                    rows.last_mut()
                        .ok_or_else(|| math_error("matrix row state is empty"))?
                        .push((cell_start, &input[cell_start..offset]));
                    rows.push(Vec::new());
                    cell_start = end;
                    offset = end;
                }
                _ => offset = end,
            }
            continue;
        }
        let character = input[offset..]
            .chars()
            .next()
            .ok_or_else(|| math_error("invalid LaTeX matrix scan offset"))?;
        match character {
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            '&' if brace_depth == 0 && delimiter_depth == 0 && environment_depth == 0 => {
                rows.last_mut()
                    .ok_or_else(|| math_error("matrix row state is empty"))?
                    .push((cell_start, &input[cell_start..offset]));
                cell_start = offset + 1;
            }
            _ => {}
        }
        offset += character.len_utf8();
    }
    rows.last_mut()
        .ok_or_else(|| math_error("matrix row state is empty"))?
        .push((cell_start, &input[cell_start..]));
    Ok(rows)
}

fn latex_nary_character(command: &str) -> &'static str {
    match command {
        "sum" => "∑",
        "prod" => "∏",
        "coprod" => "∐",
        "int" => "∫",
        "iint" => "∬",
        "iiint" => "∭",
        "oint" => "∮",
        _ => "∫",
    }
}

fn latex_nary_command(character: &str) -> Option<&'static str> {
    match character {
        "∑" => Some("sum"),
        "∏" => Some("prod"),
        "∐" => Some("coprod"),
        "∫" => Some("int"),
        "∬" => Some("iint"),
        "∭" => Some("iiint"),
        "∮" => Some("oint"),
        _ => None,
    }
}

fn latex_accent_character(command: &str) -> &'static str {
    match command {
        "hat" | "widehat" => "̂",
        "bar" | "overline" => "̄",
        "vec" | "overrightarrow" => "⃗",
        "tilde" | "widetilde" => "̃",
        "dot" => "̇",
        "ddot" => "̈",
        _ => "̂",
    }
}

fn latex_accent_command(character: &str) -> Option<&'static str> {
    match character {
        "̂" => Some("hat"),
        "̄" => Some("bar"),
        "⃗" => Some("vec"),
        "̃" => Some("tilde"),
        "̇" => Some("dot"),
        "̈" => Some("ddot"),
        _ => None,
    }
}

fn diagnose_empty_latex_runs(
    argument: &MathArgument,
    path: &str,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    for (index, expression) in argument.expressions.iter().enumerate() {
        let expression_path = format!("{path}/{}", index + 1);
        match expression {
            MathExpression::Run(run) if run.text.is_empty() => diagnostics.push(
                expression_path,
                "empty OfficeMath run was discarded by LaTeX",
            )?,
            MathExpression::Run(_) => {}
            MathExpression::Fraction(value) => {
                diagnose_empty_latex_runs(&value.numerator, &expression_path, diagnostics)?;
                diagnose_empty_latex_runs(&value.denominator, &expression_path, diagnostics)?;
            }
            MathExpression::Subscript(value) | MathExpression::Superscript(value) => {
                diagnose_empty_latex_runs(&value.base, &expression_path, diagnostics)?;
                diagnose_empty_latex_runs(&value.script, &expression_path, diagnostics)?;
            }
            MathExpression::SubSuperscript(value) => {
                diagnose_empty_latex_runs(&value.base, &expression_path, diagnostics)?;
                diagnose_empty_latex_runs(&value.subscript, &expression_path, diagnostics)?;
                diagnose_empty_latex_runs(&value.superscript, &expression_path, diagnostics)?;
            }
            MathExpression::PreSubSuperscript(value) => {
                diagnose_empty_latex_runs(&value.base, &expression_path, diagnostics)?;
                diagnose_empty_latex_runs(&value.subscript, &expression_path, diagnostics)?;
                diagnose_empty_latex_runs(&value.superscript, &expression_path, diagnostics)?;
            }
            MathExpression::Radical(value) => {
                diagnose_empty_latex_runs(&value.degree, &expression_path, diagnostics)?;
                diagnose_empty_latex_runs(&value.base, &expression_path, diagnostics)?;
            }
            MathExpression::Matrix(value) => {
                for row in &value.rows {
                    for cell in &row.cells {
                        diagnose_empty_latex_runs(cell, &expression_path, diagnostics)?;
                    }
                }
            }
            MathExpression::LowerLimit(value) | MathExpression::UpperLimit(value) => {
                diagnose_empty_latex_runs(&value.base, &expression_path, diagnostics)?;
                diagnose_empty_latex_runs(&value.limit, &expression_path, diagnostics)?;
            }
            MathExpression::Nary(value) => {
                diagnose_empty_latex_runs(&value.base, &expression_path, diagnostics)?;
                diagnose_empty_latex_runs(&value.subscript, &expression_path, diagnostics)?;
                diagnose_empty_latex_runs(&value.superscript, &expression_path, diagnostics)?;
            }
            MathExpression::Delimiter(value) => {
                for child in &value.arguments {
                    diagnose_empty_latex_runs(child, &expression_path, diagnostics)?;
                }
            }
            MathExpression::Accent(value) => {
                diagnose_empty_latex_runs(&value.base, &expression_path, diagnostics)?;
            }
        }
    }
    Ok(())
}

fn write_latex_argument(
    argument: &MathArgument,
    path: &str,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    if argument_has_direct_unsupported_content(argument) {
        diagnostics.push(path, "unmodelled OfficeMath argument content was discarded")?;
    }
    for (index, expression) in argument.expressions.iter().enumerate() {
        write_latex_expression(
            expression,
            &format!("{path}/{}", index + 1),
            output,
            diagnostics,
        )?;
    }
    Ok(())
}

fn write_latex_group(
    argument: &MathArgument,
    path: &str,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    output.push('{');
    write_latex_argument(argument, path, output, diagnostics)?;
    output.push('}');
    Ok(())
}

fn write_latex_expression(
    expression: &MathExpression,
    path: &str,
    output: &mut String,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    if expression_has_direct_unsupported_content(expression) {
        diagnostics.push(path, "unmodelled OfficeMath content was discarded")?;
    }
    match expression {
        MathExpression::Run(run) => {
            let upright = run.properties.normal == Some(true);
            if run_has_unconverted_properties(run, upright) {
                diagnostics.push(path, "OfficeMath run properties were discarded")?;
            }
            let literal = run.properties.style == Some(MathStyle::Plain);
            if upright && literal {
                output.push_str("\\text{");
                output.push_str(&latex_literal_escape(&run.text));
                output.push('}');
            } else if upright {
                if matches!(run.text.as_str(), "lim" | "sin" | "log" | "ln" | "exp") {
                    write!(output, "\\{} ", run.text).expect("String writes succeed");
                } else {
                    write!(
                        output,
                        "\\operatorname{{{}}}",
                        latex_literal_escape(&run.text)
                    )
                    .expect("String writes succeed");
                }
            } else if literal
                && !(run.text.chars().count() == 1
                    && run
                        .text
                        .chars()
                        .next()
                        .and_then(latex_symbol_name)
                        .is_some())
            {
                write!(output, "\\mathrm{{{}}}", latex_literal_escape(&run.text))
                    .expect("String writes succeed");
            } else {
                if run.text.chars().any(|character| {
                    character.is_whitespace() && math_space_width(character).is_none()
                }) {
                    diagnostics.push(path, "OfficeMath run whitespace was discarded by LaTeX")?;
                }
                output.push_str(&latex_escape(&run.text));
            }
        }
        MathExpression::Fraction(value) => {
            if value.fraction_type != FractionType::Bar {
                diagnostics.push(path, "non-bar OfficeMath fraction type was discarded")?;
            }
            output.push_str("\\frac");
            write_latex_group(&value.numerator, path, output, diagnostics)?;
            write_latex_group(&value.denominator, path, output, diagnostics)?;
        }
        MathExpression::Subscript(value) | MathExpression::Superscript(value) => {
            if value.alignment.is_some() {
                diagnostics.push(path, "OfficeMath script alignment was discarded")?;
            }
            write_latex_group(&value.base, path, output, diagnostics)?;
            output.push(if matches!(expression, MathExpression::Subscript(_)) {
                '_'
            } else {
                '^'
            });
            write_latex_group(&value.script, path, output, diagnostics)?;
        }
        MathExpression::SubSuperscript(value) => {
            if value.alignment.is_some() {
                diagnostics.push(path, "OfficeMath script alignment was discarded")?;
            }
            write_latex_group(&value.base, path, output, diagnostics)?;
            output.push('_');
            write_latex_group(&value.subscript, path, output, diagnostics)?;
            output.push('^');
            write_latex_group(&value.superscript, path, output, diagnostics)?;
        }
        MathExpression::PreSubSuperscript(value) => {
            if value.alignment.is_some() {
                diagnostics.push(path, "OfficeMath script alignment was discarded")?;
            }
            output.push_str("{}");
            output.push('_');
            write_latex_group(&value.subscript, path, output, diagnostics)?;
            output.push('^');
            write_latex_group(&value.superscript, path, output, diagnostics)?;
            write_latex_group(&value.base, path, output, diagnostics)?;
        }
        MathExpression::Radical(value) => {
            if value.hide_degree != value.degree.expressions.is_empty() {
                diagnostics.push(path, "OfficeMath radical degree visibility was discarded")?;
            }
            output.push_str("\\sqrt");
            if !value.degree.expressions.is_empty() {
                output.push('[');
                write_latex_argument(&value.degree, path, output, diagnostics)?;
                output.push(']');
            }
            write_latex_group(&value.base, path, output, diagnostics)?;
        }
        MathExpression::Matrix(value) => {
            if value.properties != Default::default() {
                diagnostics.push(path, "OfficeMath matrix properties were discarded")?;
            }
            output.push_str("\\begin{matrix}");
            for (row_index, row) in value.rows.iter().enumerate() {
                if row_index != 0 {
                    output.push_str("\\\\");
                }
                for (cell_index, cell) in row.cells.iter().enumerate() {
                    if cell_index != 0 {
                        output.push('&');
                    }
                    write_latex_argument(cell, path, output, diagnostics)?;
                }
            }
            output.push_str("\\end{matrix}");
        }
        MathExpression::LowerLimit(value) | MathExpression::UpperLimit(value) => {
            output.push_str(if matches!(expression, MathExpression::LowerLimit(_)) {
                "\\underset"
            } else {
                "\\overset"
            });
            write_latex_group(&value.limit, path, output, diagnostics)?;
            write_latex_group(&value.base, path, output, diagnostics)?;
        }
        MathExpression::Nary(value) => {
            let Some(command) = latex_nary_command(&value.character) else {
                diagnostics.push(path, "unsupported OfficeMath n-ary operator was discarded")?;
                return Ok(());
            };
            if value.grow.is_some() || value.limit_location.is_some() {
                diagnostics.push(path, "OfficeMath n-ary layout properties were discarded")?;
            }
            if value.hide_subscript && !value.subscript.expressions.is_empty() {
                diagnostics.push(path, "hidden OfficeMath n-ary subscript was discarded")?;
            } else if !value.hide_subscript && value.subscript.expressions.is_empty() {
                diagnostics.push(path, "OfficeMath n-ary subscript visibility was discarded")?;
            }
            if value.hide_superscript && !value.superscript.expressions.is_empty() {
                diagnostics.push(path, "hidden OfficeMath n-ary superscript was discarded")?;
            } else if !value.hide_superscript && value.superscript.expressions.is_empty() {
                diagnostics.push(
                    path,
                    "OfficeMath n-ary superscript visibility was discarded",
                )?;
            }
            write!(output, "\\{command}").expect("String writes are infallible");
            if !value.hide_subscript && !value.subscript.expressions.is_empty() {
                output.push('_');
                write_latex_group(&value.subscript, path, output, diagnostics)?;
            }
            if !value.hide_superscript && !value.superscript.expressions.is_empty() {
                output.push('^');
                write_latex_group(&value.superscript, path, output, diagnostics)?;
            }
            write_latex_group(&value.base, path, output, diagnostics)?;
        }
        MathExpression::Delimiter(value) if canonical_binomial(value).is_some() => {
            let fraction = canonical_binomial(value).expect("matched canonical binomial");
            output.push_str("\\binom");
            write_latex_group(&fraction.numerator, path, output, diagnostics)?;
            write_latex_group(&fraction.denominator, path, output, diagnostics)?;
        }
        MathExpression::Delimiter(value) => {
            if value.grow.is_some() && canonical_binomial(value).is_none() {
                diagnostics.push(path, "OfficeMath delimiter growth setting was discarded")?;
            }
            if value.begin_character.chars().count() > 1
                || value.end_character.chars().count() > 1
                || value.separator_character.chars().count() > 1
            {
                diagnostics.push(path, "non-scalar OfficeMath delimiter was discarded")?;
                return Ok(());
            }
            let (Some(begin), Some(separator), Some(end)) = (
                latex_delimiter(&value.begin_character),
                latex_delimiter(&value.separator_character),
                latex_delimiter(&value.end_character),
            ) else {
                diagnostics.push(
                    path,
                    "unsupported OfficeMath LaTeX delimiter character was discarded",
                )?;
                return Ok(());
            };
            output.push_str("\\left");
            output.push_str(&begin);
            for (index, argument) in value.arguments.iter().enumerate() {
                if index != 0 {
                    output.push_str("\\middle");
                    output.push_str(&separator);
                }
                write_latex_group(argument, path, output, diagnostics)?;
            }
            output.push_str("\\right");
            output.push_str(&end);
        }
        MathExpression::Accent(value) => {
            let Some(command) = latex_accent_command(&value.character) else {
                diagnostics.push(path, "unsupported OfficeMath accent was discarded")?;
                return Ok(());
            };
            write!(output, "\\{command}").expect("String writes are infallible");
            write_latex_group(&value.base, path, output, diagnostics)?;
        }
    }
    Ok(())
}

fn canonical_binomial(value: &MathDelimiter) -> Option<&MathFraction> {
    if value.begin_character != "("
        || value.end_character != ")"
        || !value.separator_character.is_empty()
        || value.grow != Some(true)
    {
        return None;
    }
    let [argument] = value.arguments.as_slice() else {
        return None;
    };
    let [expression @ MathExpression::Fraction(fraction)] = argument.expressions.as_slice() else {
        return None;
    };
    (fraction.fraction_type == FractionType::NoBar
        && !expression_has_direct_unsupported_content(expression))
    .then_some(fraction)
}

fn math_space_width(character: char) -> Option<&'static str> {
    match character {
        '\u{2009}' => Some("0.167em"),
        '\u{2004}' => Some("0.278em"),
        '\u{2001}' => Some("1.0em"),
        _ => None,
    }
}

fn latex_literal_escape(value: &str) -> String {
    let mut result = String::new();
    for character in value.chars() {
        if matches!(
            character,
            '\\' | '{' | '}' | '_' | '^' | '#' | '$' | '%' | '&'
        ) {
            result.push('\\');
        }
        result.push(character);
    }
    result
}

fn latex_symbol(command: &str) -> Option<char> {
    Some(match command {
        "alpha" => 'α',
        "beta" => 'β',
        "theta" => 'θ',
        "lambda" => 'λ',
        "mu" => 'μ',
        "sigma" => 'σ',
        "omega" => 'ω',
        "varepsilon" => 'ε',
        "phi" => 'ϕ',
        "varphi" => 'φ',
        "Delta" => 'Δ',
        "Sigma" => 'Σ',
        "Omega" => 'Ω',
        "Gamma" => 'Γ',
        "le" | "leq" => '≤',
        "ge" | "geq" => '≥',
        "ne" | "neq" => '≠',
        "approx" => '≈',
        "equiv" => '≡',
        "sim" => '∼',
        "propto" => '∝',
        "in" => '∈',
        "subset" => '⊂',
        "times" => '×',
        "cdot" => '⋅',
        "div" => '÷',
        "pm" => '±',
        "mp" => '∓',
        "cup" => '∪',
        "cap" => '∩',
        "Rightarrow" => '⇒',
        "leftarrow" => '←',
        "partial" => '∂',
        "nabla" => '∇',
        "forall" => '∀',
        "exists" => '∃',
        "degree" => '°',
        "ldots" => '…',
        "cdots" => '⋯',
        _ => return None,
    })
}

fn latex_symbol_name(character: char) -> Option<&'static str> {
    Some(match character {
        'α' => "alpha",
        'β' => "beta",
        'θ' => "theta",
        'λ' => "lambda",
        'μ' => "mu",
        'σ' => "sigma",
        'ω' => "omega",
        'ε' => "varepsilon",
        'ϕ' => "phi",
        'φ' => "varphi",
        'Δ' => "Delta",
        'Σ' => "Sigma",
        'Ω' => "Omega",
        'Γ' => "Gamma",
        '≤' => "leq",
        '≥' => "geq",
        '≠' => "neq",
        '≈' => "approx",
        '≡' => "equiv",
        '∼' => "sim",
        '∝' => "propto",
        '∈' => "in",
        '⊂' => "subset",
        '×' => "times",
        '⋅' => "cdot",
        '÷' => "div",
        '±' => "pm",
        '∓' => "mp",
        '∪' => "cup",
        '∩' => "cap",
        '⇒' => "Rightarrow",
        '←' => "leftarrow",
        '∂' => "partial",
        '∇' => "nabla",
        '∀' => "forall",
        '∃' => "exists",
        '°' => "degree",
        '…' => "ldots",
        '⋯' => "cdots",
        _ => return None,
    })
}

fn latex_escape(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\backslash{}"),
            '\u{2009}' => escaped.push_str("\\,"),
            '\u{2004}' => escaped.push_str("\\;"),
            '\u{2001}' => escaped.push_str("\\quad "),
            character if latex_symbol_name(character).is_some() => {
                escaped.push('\\');
                escaped.push_str(latex_symbol_name(character).expect("matched finite symbol"));
                escaped.push_str("{}");
            }
            '[' => escaped.push_str("\\lbrack{}"),
            ']' => escaped.push_str("\\rbrack{}"),
            '{' | '}' | '_' | '^' | '#' | '$' | '%' | '&' => {
                escaped.push('\\');
                escaped.push(character);
            }
            _ => escaped.push(character),
        }
    }
    escaped
}

fn latex_delimiter(value: &str) -> Option<String> {
    Some(match value {
        "" => ".".to_owned(),
        "{" => "\\lbrace{}".to_owned(),
        "}" => "\\rbrace{}".to_owned(),
        "⟨" => "\\langle{}".to_owned(),
        "⟩" => "\\rangle{}".to_owned(),
        "|" => "\\vert{}".to_owned(),
        "\\" => "\\backslash{}".to_owned(),
        value
            if value
                .chars()
                .any(|character| character.is_alphabetic() || character.is_whitespace()) =>
        {
            return None;
        }
        value => value.to_owned(),
    })
}
