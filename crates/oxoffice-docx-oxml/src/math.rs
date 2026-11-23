//! Typed support for the Transitional OfficeMath vocabulary.

use std::io::Write;

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer, XmlVersion};

use crate::error::{OxmlError, Result};
use crate::namespace::{M_NS, W_NS};
use crate::properties::CT_RPr;
use crate::raw_xml::capture_element;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Preservation {
    attributes: Vec<(String, String)>,
    raw_children: Vec<(usize, Vec<u8>)>,
    modeled_children: Vec<PreservedChild>,
    inherited_bindings: Vec<(String, String)>,
    property_raw_children: Vec<(usize, Vec<u8>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreservedChild {
    name: String,
    raw: Vec<u8>,
    bindings: Vec<(String, String)>,
}

#[derive(Debug)]
struct ParsedElement {
    preservation: Preservation,
    bindings: Vec<(String, String)>,
    children: Vec<Vec<u8>>,
}

/// One inline OfficeMath equation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CT_OMath {
    pub expressions: Vec<MathExpression>,
    preservation: Preservation,
}

impl CT_OMath {
    pub fn new(expressions: Vec<MathExpression>) -> Self {
        Self {
            expressions,
            preservation: Preservation::default(),
        }
    }

    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        Self::from_raw(xml, &[])
    }

    pub(crate) fn from_raw(xml: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        validate_xml_depth(xml)?;
        require_safe_fixed_math_prefix(xml, inherited)?;
        let mut parsed = parse_element(xml, inherited)?;
        require_root(xml, &parsed.bindings, b"oMath")?;
        keep_used_root_bindings(&mut parsed.preservation, xml, inherited)?;
        let mut expressions = Vec::new();
        for child in parsed.children {
            if let Some(expression) = MathExpression::from_raw(&child, &parsed.bindings)? {
                expressions.push(expression);
            } else {
                parsed
                    .preservation
                    .raw_children
                    .push((expressions.len(), child));
            }
        }
        Ok(Self {
            expressions,
            preservation: parsed.preservation,
        })
    }

    pub fn to_xml(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new(Vec::new());
        self.write_xml_in(&mut writer, true, None)?;
        Ok(writer.into_inner())
    }

    /// Whether this equation retains content outside the typed OfficeMath subset.
    pub fn has_unsupported_content(&self) -> bool {
        preservation_has_unsupported_content(&self.preservation)
            || self
                .expressions
                .iter()
                .any(MathExpression::has_unsupported_content)
    }

    pub(crate) fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        declare_math: bool,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let root = math_start("oMath", &self.preservation, declare_math, emission)?;
        root.write(writer, false)?;
        write_raw_slot(writer, &self.preservation, 0)?;
        for (index, expression) in self.expressions.iter().enumerate() {
            expression.write_xml_in(writer, emission)?;
            write_raw_slot(writer, &self.preservation, index + 1)?;
        }
        write_raw_tail(writer, &self.preservation, self.expressions.len() + 1)?;
        writer.write_event(Event::End(BytesEnd::new("m:oMath")))?;
        Ok(())
    }
}

/// One display OfficeMath paragraph.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CT_OMathPara {
    pub properties: MathParagraphProperties,
    pub equations: Vec<CT_OMath>,
    preservation: Preservation,
}

impl CT_OMathPara {
    pub fn new(equations: Vec<CT_OMath>) -> Self {
        Self {
            equations,
            ..Self::default()
        }
    }

    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        Self::from_raw(xml, &[])
    }

    pub(crate) fn from_raw(xml: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        validate_xml_depth(xml)?;
        require_safe_fixed_math_prefix(xml, inherited)?;
        if !valid_officemath_paragraph_shape(xml, inherited)? {
            return Err(OxmlError::InvalidValue(
                "invalid OfficeMath paragraph child sequence".to_owned(),
            ));
        }
        let mut parsed = parse_element(xml, inherited)?;
        require_root(xml, &parsed.bindings, b"oMathPara")?;
        keep_used_root_bindings(&mut parsed.preservation, xml, inherited)?;
        let mut properties = MathParagraphProperties::default();
        let mut equations = Vec::new();
        let mut modeled = 0usize;
        for child in parsed.children {
            match math_local_name(&child, &parsed.bindings)?.as_deref() {
                Some("oMathParaPr") if modeled == 0 => {
                    properties = MathParagraphProperties::from_raw(&child, &parsed.bindings)?;
                    preserve_modeled_child(
                        &mut parsed.preservation,
                        "oMathParaPr",
                        &child,
                        &parsed.bindings,
                    );
                    modeled += 1;
                }
                Some("oMath") => {
                    equations.push(CT_OMath::from_raw(&child, &parsed.bindings)?);
                    modeled += 1;
                }
                _ => parsed.preservation.raw_children.push((modeled, child)),
            }
        }
        Ok(Self {
            properties,
            equations,
            preservation: parsed.preservation,
        })
    }

    pub fn to_xml(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new(Vec::new());
        self.write_xml_in(&mut writer, true, None)?;
        Ok(writer.into_inner())
    }

    /// Whether this display equation retains content outside the typed subset.
    pub fn has_unsupported_content(&self) -> bool {
        preservation_has_unsupported_content(&self.preservation)
            || property_preservation_has_unsupported_content(
                &self.properties.preservation,
                "oMathParaPr",
            )
            || self.equations.iter().any(CT_OMath::has_unsupported_content)
    }

    pub(crate) fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        declare_math: bool,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        if self.equations.is_empty() {
            return Err(OxmlError::InvalidValue(
                "OfficeMath display requires at least one equation".to_owned(),
            ));
        }
        let root = math_start("oMathPara", &self.preservation, declare_math, emission)?;
        root.write(writer, false)?;
        let had_properties = has_preserved_modeled_child(&self.preservation, "oMathParaPr");
        let has_properties = !self.properties.is_empty() || had_properties;
        if has_properties && !had_properties {
            self.properties.write_xml_in(writer, emission)?;
        }
        let mut modeled = 0usize;
        write_raw_slot(writer, &self.preservation, modeled)?;
        if has_properties && had_properties {
            self.properties.write_xml_in(writer, emission)?;
            modeled += 1;
            write_raw_slot(writer, &self.preservation, modeled)?;
        }
        for equation in &self.equations {
            equation.write_xml_in(writer, false, emission)?;
            modeled += 1;
            write_raw_slot(writer, &self.preservation, modeled)?;
        }
        write_raw_tail(writer, &self.preservation, modeled + 1)?;
        writer.write_event(Event::End(BytesEnd::new("m:oMathPara")))?;
        Ok(())
    }
}

/// Inline or display OfficeMath at a paragraph boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfficeMath {
    Inline(CT_OMath),
    Display(CT_OMathPara),
}

impl OfficeMath {
    pub fn inline(expressions: Vec<MathExpression>) -> Self {
        Self::Inline(CT_OMath::new(expressions))
    }

    pub fn display(equations: Vec<CT_OMath>) -> Self {
        Self::Display(CT_OMathPara::new(equations))
    }

    pub fn to_xml(&self) -> Result<Vec<u8>> {
        match self {
            Self::Inline(value) => value.to_xml(),
            Self::Display(value) => value.to_xml(),
        }
    }

    /// This equation as its model writes it, in one canonical writing: names
    /// keep their prefixes, namespace declarations go and each element
    /// declares the prefixes it names (in a name or in a markup-compatibility
    /// or `xsi:type` value) whose binding is not held yet, attributes are
    /// sorted, values and text read through their references and escaped one
    /// way, and an element without content is written empty. Two copies of
    /// one equation, one a save wrote again with its declarations elsewhere
    /// or a default spelled out, give the same bytes; any change of a name,
    /// a binding, an attribute or text gives other bytes.
    #[doc(hidden)]
    pub fn canonical_xml(&self) -> Result<Vec<u8>> {
        canonical_element(&self.to_xml()?)
    }

    /// Whether this value retains content outside the typed OfficeMath subset.
    pub fn has_unsupported_content(&self) -> bool {
        match self {
            Self::Inline(value) => value.has_unsupported_content(),
            Self::Display(value) => value.has_unsupported_content(),
        }
    }

    pub(crate) fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Option<Self>> {
        if !fixed_math_prefix_is_safe(raw, inherited)? {
            return Ok(None);
        }
        Ok(match math_local_name(raw, inherited)?.as_deref() {
            Some("oMath") => Some(Self::Inline(CT_OMath::from_raw(raw, inherited)?)),
            Some("oMathPara") if valid_officemath_paragraph_shape(raw, inherited)? => {
                Some(Self::Display(CT_OMathPara::from_raw(raw, inherited)?))
            }
            Some("oMathPara") => None,
            _ => None,
        })
    }

    pub(crate) fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        match self {
            Self::Inline(value) => value.write_xml_in(writer, true, emission),
            Self::Display(value) => value.write_xml_in(writer, true, emission),
        }
    }
}

/// One expression in the normalized OfficeMath tree.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MathExpression {
    Run(MathRun),
    Fraction(MathFraction),
    Subscript(MathScript),
    Superscript(MathScript),
    SubSuperscript(MathSubSuperscript),
    PreSubSuperscript(MathPreSubSuperscript),
    Radical(MathRadical),
    Matrix(MathMatrix),
    LowerLimit(MathLimit),
    UpperLimit(MathLimit),
    Nary(MathNary),
    Delimiter(MathDelimiter),
    Accent(MathAccent),
    Function(MathFunction),
    Bar(MathBar),
    Box(MathBox),
    BorderBox(MathBorderBox),
    GroupCharacter(MathGroupCharacter),
    Phantom(MathPhantom),
    EquationArray(MathEquationArray),
}

impl MathExpression {
    /// The arguments of this expression, in document order: a matrix's
    /// cells row by row, an equation array's rows.
    pub fn arguments(&self) -> Vec<&MathArgument> {
        match self {
            Self::Run(_) => Vec::new(),
            Self::Fraction(value) => vec![&value.numerator, &value.denominator],
            Self::Subscript(value) | Self::Superscript(value) => vec![&value.base, &value.script],
            Self::SubSuperscript(value) => {
                vec![&value.base, &value.subscript, &value.superscript]
            }
            Self::PreSubSuperscript(value) => {
                vec![&value.subscript, &value.superscript, &value.base]
            }
            Self::Radical(value) => vec![&value.degree, &value.base],
            Self::Matrix(value) => value.rows.iter().flat_map(|row| &row.cells).collect(),
            Self::LowerLimit(value) | Self::UpperLimit(value) => vec![&value.base, &value.limit],
            Self::Nary(value) => vec![&value.subscript, &value.superscript, &value.base],
            Self::Delimiter(value) => value.arguments.iter().collect(),
            Self::Accent(value) => vec![&value.base],
            Self::Function(value) => vec![&value.name, &value.argument],
            Self::Bar(value) => vec![&value.base],
            Self::Box(value) => vec![&value.base],
            Self::BorderBox(value) => vec![&value.base],
            Self::GroupCharacter(value) => vec![&value.base],
            Self::Phantom(value) => vec![&value.base],
            Self::EquationArray(value) => value.rows.iter().collect(),
        }
    }

    /// [`Self::arguments`], to change them.
    pub fn arguments_mut(&mut self) -> Vec<&mut MathArgument> {
        match self {
            Self::Run(_) => Vec::new(),
            Self::Fraction(value) => vec![&mut value.numerator, &mut value.denominator],
            Self::Subscript(value) | Self::Superscript(value) => {
                vec![&mut value.base, &mut value.script]
            }
            Self::SubSuperscript(value) => {
                vec![
                    &mut value.base,
                    &mut value.subscript,
                    &mut value.superscript,
                ]
            }
            Self::PreSubSuperscript(value) => {
                vec![
                    &mut value.subscript,
                    &mut value.superscript,
                    &mut value.base,
                ]
            }
            Self::Radical(value) => vec![&mut value.degree, &mut value.base],
            Self::Matrix(value) => value
                .rows
                .iter_mut()
                .flat_map(|row| &mut row.cells)
                .collect(),
            Self::LowerLimit(value) | Self::UpperLimit(value) => {
                vec![&mut value.base, &mut value.limit]
            }
            Self::Nary(value) => vec![
                &mut value.subscript,
                &mut value.superscript,
                &mut value.base,
            ],
            Self::Delimiter(value) => value.arguments.iter_mut().collect(),
            Self::Accent(value) => vec![&mut value.base],
            Self::Function(value) => vec![&mut value.name, &mut value.argument],
            Self::Bar(value) => vec![&mut value.base],
            Self::Box(value) => vec![&mut value.base],
            Self::BorderBox(value) => vec![&mut value.base],
            Self::GroupCharacter(value) => vec![&mut value.base],
            Self::Phantom(value) => vec![&mut value.base],
            Self::EquationArray(value) => value.rows.iter_mut().collect(),
        }
    }

    /// Whether this expression or one of its arguments retains unsupported content.
    pub fn has_unsupported_content(&self) -> bool {
        match self {
            Self::Run(value) => {
                run_preservation_has_unsupported_content(&value.preservation)
                    || property_preservation_has_unsupported_content(
                        &value.properties.preservation,
                        "rPr",
                    )
                    || math_text_has_unsupported_content(&value.preservation)
            }
            Self::Fraction(value) => {
                preservation_has_unsupported_content(&value.preservation)
                    || property_container_has_unsupported_content(&value.preservation, "fPr")
                    || value.numerator.has_unsupported_content()
                    || value.denominator.has_unsupported_content()
            }
            Self::Subscript(value) => script_has_unsupported_content(value, "sSubPr"),
            Self::Superscript(value) => script_has_unsupported_content(value, "sSupPr"),
            Self::SubSuperscript(value) => three_argument_script_has_unsupported_content(
                &value.preservation,
                "sSubSupPr",
                &value.base,
                &value.subscript,
                &value.superscript,
            ),
            Self::PreSubSuperscript(value) => three_argument_script_has_unsupported_content(
                &value.preservation,
                "sPrePr",
                &value.base,
                &value.subscript,
                &value.superscript,
            ),
            Self::Radical(value) => {
                preservation_has_unsupported_content(&value.preservation)
                    || property_container_has_unsupported_content(&value.preservation, "radPr")
                    || value.degree.has_unsupported_content()
                    || value.base.has_unsupported_content()
            }
            Self::Matrix(value) => {
                preservation_has_unsupported_content(&value.preservation)
                    || property_container_has_unsupported_content(&value.preservation, "mPr")
                    || value.rows.iter().any(|row| {
                        preservation_has_unsupported_content(&row.preservation)
                            || row.cells.iter().any(MathArgument::has_unsupported_content)
                    })
            }
            Self::LowerLimit(value) => limit_has_unsupported_content(value, "limLowPr"),
            Self::UpperLimit(value) => limit_has_unsupported_content(value, "limUppPr"),
            Self::Nary(value) => {
                preservation_has_unsupported_content(&value.preservation)
                    || property_container_has_unsupported_content(&value.preservation, "naryPr")
                    || value.base.has_unsupported_content()
                    || value.subscript.has_unsupported_content()
                    || value.superscript.has_unsupported_content()
            }
            Self::Delimiter(value) => {
                preservation_has_unsupported_content(&value.preservation)
                    || property_container_has_unsupported_content(&value.preservation, "dPr")
                    || value
                        .arguments
                        .iter()
                        .any(MathArgument::has_unsupported_content)
            }
            Self::Accent(value) => {
                preservation_has_unsupported_content(&value.preservation)
                    || property_container_has_unsupported_content(&value.preservation, "accPr")
                    || value.base.has_unsupported_content()
            }
            Self::Function(value) => {
                preservation_has_unsupported_content(&value.preservation)
                    || property_container_has_unsupported_content(&value.preservation, "funcPr")
                    || value.name.has_unsupported_content()
                    || value.argument.has_unsupported_content()
            }
            Self::Bar(value) => {
                based_has_unsupported_content(&value.preservation, "barPr", &value.base)
            }
            Self::Box(value) => {
                based_has_unsupported_content(&value.preservation, "boxPr", &value.base)
            }
            Self::BorderBox(value) => {
                based_has_unsupported_content(&value.preservation, "borderBoxPr", &value.base)
            }
            Self::GroupCharacter(value) => {
                based_has_unsupported_content(&value.preservation, "groupChrPr", &value.base)
            }
            Self::Phantom(value) => {
                based_has_unsupported_content(&value.preservation, "phantPr", &value.base)
            }
            Self::EquationArray(value) => {
                preservation_has_unsupported_content(&value.preservation)
                    || property_container_has_unsupported_content(&value.preservation, "eqArrPr")
                    || value.rows.iter().any(MathArgument::has_unsupported_content)
            }
        }
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Option<Self>> {
        if !valid_expression_shape(raw, inherited)? {
            return Ok(None);
        }
        Ok(match math_local_name(raw, inherited)?.as_deref() {
            Some("r") => Some(Self::Run(MathRun::from_raw(raw, inherited)?)),
            Some("f") => Some(Self::Fraction(MathFraction::from_raw(raw, inherited)?)),
            Some("sSub") => Some(Self::Subscript(MathScript::from_raw(raw, inherited)?)),
            Some("sSup") => Some(Self::Superscript(MathScript::from_raw(raw, inherited)?)),
            Some("sSubSup") => Some(Self::SubSuperscript(MathSubSuperscript::from_raw(
                raw, inherited,
            )?)),
            Some("sPre") => Some(Self::PreSubSuperscript(MathPreSubSuperscript::from_raw(
                raw, inherited,
            )?)),
            Some("rad") => Some(Self::Radical(MathRadical::from_raw(raw, inherited)?)),
            Some("m") => Some(Self::Matrix(MathMatrix::from_raw(raw, inherited)?)),
            Some("limLow") => Some(Self::LowerLimit(MathLimit::from_raw(raw, inherited)?)),
            Some("limUpp") => Some(Self::UpperLimit(MathLimit::from_raw(raw, inherited)?)),
            Some("nary") => Some(Self::Nary(MathNary::from_raw(raw, inherited)?)),
            Some("d") => Some(Self::Delimiter(MathDelimiter::from_raw(raw, inherited)?)),
            Some("acc") => Some(Self::Accent(MathAccent::from_raw(raw, inherited)?)),
            Some("func") => Some(Self::Function(MathFunction::from_raw(raw, inherited)?)),
            Some("bar") => Some(Self::Bar(MathBar::from_raw(raw, inherited)?)),
            Some("box") => Some(Self::Box(MathBox::from_raw(raw, inherited)?)),
            Some("borderBox") => Some(Self::BorderBox(MathBorderBox::from_raw(raw, inherited)?)),
            Some("groupChr") => Some(Self::GroupCharacter(MathGroupCharacter::from_raw(
                raw, inherited,
            )?)),
            Some("phant") => Some(Self::Phantom(MathPhantom::from_raw(raw, inherited)?)),
            Some("eqArr") => Some(Self::EquationArray(MathEquationArray::from_raw(
                raw, inherited,
            )?)),
            _ => None,
        })
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        match self {
            Self::Run(value) => value.write_xml_in(writer, emission),
            Self::Fraction(value) => value.write_xml_in(writer, emission),
            Self::Subscript(value) => value.write_xml_in(writer, "sSub", emission),
            Self::Superscript(value) => value.write_xml_in(writer, "sSup", emission),
            Self::SubSuperscript(value) => value.write_xml_in(writer, emission),
            Self::PreSubSuperscript(value) => value.write_xml_in(writer, emission),
            Self::Radical(value) => value.write_xml_in(writer, emission),
            Self::Matrix(value) => value.write_xml_in(writer, emission),
            Self::LowerLimit(value) => value.write_xml_in(writer, "limLow", emission),
            Self::UpperLimit(value) => value.write_xml_in(writer, "limUpp", emission),
            Self::Nary(value) => value.write_xml_in(writer, emission),
            Self::Delimiter(value) => value.write_xml_in(writer, emission),
            Self::Accent(value) => value.write_xml_in(writer, emission),
            Self::Function(value) => value.write_xml_in(writer, emission),
            Self::Bar(value) => value.write_xml_in(writer, emission),
            Self::Box(value) => value.write_xml_in(writer, emission),
            Self::BorderBox(value) => value.write_xml_in(writer, emission),
            Self::GroupCharacter(value) => value.write_xml_in(writer, emission),
            Self::Phantom(value) => value.write_xml_in(writer, emission),
            Self::EquationArray(value) => value.write_xml_in(writer, emission),
        }
    }
}

impl From<MathRun> for MathExpression {
    fn from(value: MathRun) -> Self {
        Self::Run(value)
    }
}

/// A recursively nested OfficeMath argument.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MathArgument {
    pub expressions: Vec<MathExpression>,
    preservation: Preservation,
}

impl MathArgument {
    pub fn new(expressions: Vec<MathExpression>) -> Self {
        Self {
            expressions,
            preservation: Preservation::default(),
        }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::new(vec![MathRun::new(text).into()])
    }

    /// The expressions of an equation array's row before and after its
    /// first `&`, the column the rows align on, with every `&` left out of
    /// the run text. A row with no `&` is all after it.
    pub fn split_at_alignment(&self) -> (MathArgument, MathArgument) {
        let mut before = Vec::new();
        let mut after = Vec::new();
        let mut aligned = false;
        for expression in &self.expressions {
            match expression {
                MathExpression::Run(run) if run.text.contains('&') => {
                    let with_text = |text: &str| {
                        let mut part = run.clone();
                        part.text = text.replace('&', "");
                        MathExpression::Run(part)
                    };
                    if aligned {
                        after.push(with_text(&run.text));
                        continue;
                    }
                    let (head, tail) = run.text.split_once('&').unwrap_or_default();
                    if !head.is_empty() {
                        before.push(with_text(head));
                    }
                    if !tail.is_empty() {
                        after.push(with_text(tail));
                    }
                    aligned = true;
                }
                other if aligned => after.push(other.clone()),
                other => before.push(other.clone()),
            }
        }
        if !aligned {
            return (MathArgument::default(), MathArgument::new(before));
        }
        (MathArgument::new(before), MathArgument::new(after))
    }

    /// Whether this argument retains content outside the typed OfficeMath subset.
    pub fn has_unsupported_content(&self) -> bool {
        // An argument may end with the control properties of its construct.
        let bindings = element_bindings(&self.preservation);
        let mut preservation = self.preservation.clone();
        preservation
            .raw_children
            .retain(|(_, raw)| !is_control_properties(raw, &bindings));
        preservation_has_unsupported_content(&preservation)
            || self
                .expressions
                .iter()
                .any(MathExpression::has_unsupported_content)
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        let mut parsed = parse_element(raw, inherited)?;
        let mut expressions = Vec::new();
        for child in parsed.children {
            if let Some(expression) = MathExpression::from_raw(&child, &parsed.bindings)? {
                expressions.push(expression);
            } else {
                parsed
                    .preservation
                    .raw_children
                    .push((expressions.len(), child));
            }
        }
        Ok(Self {
            expressions,
            preservation: parsed.preservation,
        })
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        tag: &str,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        write_container(
            writer,
            tag,
            &self.preservation,
            |writer| {
                write_raw_slot(writer, &self.preservation, 0)?;
                for (index, expression) in self.expressions.iter().enumerate() {
                    expression.write_xml_in(writer, emission)?;
                    write_raw_slot(writer, &self.preservation, index + 1)?;
                }
                write_raw_tail(writer, &self.preservation, self.expressions.len() + 1)
            },
            emission,
        )
    }
}

/// OfficeMath run formatting needed by authoring and layout.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MathRunProperties {
    pub style: Option<MathStyle>,
    pub normal: Option<bool>,
    pub literal: Option<bool>,
    pub script: Option<MathScriptStyle>,
    pub break_before: bool,
    pub break_alignment: Option<u8>,
    preservation: Preservation,
}

/// Mathematical glyph style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathStyle {
    Plain,
    Bold,
    Italic,
    BoldItalic,
}

impl MathStyle {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "p" => Some(Self::Plain),
            "b" => Some(Self::Bold),
            "i" => Some(Self::Italic),
            "bi" => Some(Self::BoldItalic),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "p",
            Self::Bold => "b",
            Self::Italic => "i",
            Self::BoldItalic => "bi",
        }
    }
}

/// Script category for a math run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathScriptStyle {
    Roman,
    Script,
    Fraktur,
    DoubleStruck,
    SansSerif,
    Monospace,
}

impl MathScriptStyle {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "roman" => Some(Self::Roman),
            "script" => Some(Self::Script),
            "fraktur" => Some(Self::Fraktur),
            "double-struck" => Some(Self::DoubleStruck),
            "sans-serif" => Some(Self::SansSerif),
            "monospace" => Some(Self::Monospace),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Roman => "roman",
            Self::Script => "script",
            Self::Fraktur => "fraktur",
            Self::DoubleStruck => "double-struck",
            Self::SansSerif => "sans-serif",
            Self::Monospace => "monospace",
        }
    }
}

/// Text-bearing OfficeMath run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MathRun {
    pub text: String,
    pub properties: MathRunProperties,
    preservation: Preservation,
}

impl MathRun {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    /// The WordprocessingML run properties (`w:rPr`) of this run, which Word
    /// writes in every math run: its font, size and colour.
    pub fn word_properties(&self) -> Option<CT_RPr> {
        let bindings = element_bindings(&self.preservation);
        self.preservation
            .raw_children
            .iter()
            .find_map(|(_, raw)| word_run_properties(raw, &bindings))
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        let mut parsed = parse_element(raw, inherited)?;
        let mut properties = MathRunProperties::default();
        let mut text = String::new();
        let mut modeled = 0usize;
        for child in parsed.children {
            match math_local_name(&child, &parsed.bindings)?.as_deref() {
                Some("rPr") if modeled == 0 => {
                    properties = parse_run_properties(&child, &parsed.bindings)?;
                    preserve_modeled_child(
                        &mut parsed.preservation,
                        "rPr",
                        &child,
                        &parsed.bindings,
                    );
                    modeled += 1;
                }
                Some("t") => {
                    text.push_str(&element_text(&child)?);
                    preserve_modeled_child(&mut parsed.preservation, "t", &child, &parsed.bindings);
                    modeled += 1;
                }
                _ => parsed.preservation.raw_children.push((modeled, child)),
            }
        }
        Ok(Self {
            text,
            properties,
            preservation: parsed.preservation,
        })
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        write_container(
            writer,
            "r",
            &self.preservation,
            |writer| {
                let had_properties = has_preserved_modeled_child(&self.preservation, "rPr");
                let emit_properties = !self.properties.is_empty() || had_properties;
                if emit_properties && !had_properties {
                    write_run_properties(writer, &self.properties, emission)?;
                }
                let mut modeled = 0usize;
                write_raw_slot(writer, &self.preservation, modeled)?;
                if emit_properties && had_properties {
                    write_run_properties(writer, &self.properties, emission)?;
                    modeled += 1;
                    write_raw_slot(writer, &self.preservation, modeled)?;
                }
                write_math_text(writer, &self.text, &self.preservation, emission)?;
                modeled += 1;
                write_raw_slot(writer, &self.preservation, modeled)
            },
            emission,
        )
    }
}

impl MathRunProperties {
    fn is_empty(&self) -> bool {
        self.style.is_none()
            && self.normal.is_none()
            && self.literal.is_none()
            && self.script.is_none()
            && !self.break_before
            && self.break_alignment.is_none()
            && self.preservation == Preservation::default()
    }
}

/// Fraction bar form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FractionType {
    #[default]
    Bar,
    Skewed,
    Linear,
    NoBar,
}

impl FractionType {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "bar" => Some(Self::Bar),
            "skw" => Some(Self::Skewed),
            "lin" => Some(Self::Linear),
            "noBar" => Some(Self::NoBar),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Bar => "bar",
            Self::Skewed => "skw",
            Self::Linear => "lin",
            Self::NoBar => "noBar",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathFraction {
    pub fraction_type: FractionType,
    pub numerator: MathArgument,
    pub denominator: MathArgument,
    preservation: Preservation,
}

impl MathFraction {
    pub fn new(numerator: MathArgument, denominator: MathArgument) -> Self {
        Self {
            fraction_type: FractionType::Bar,
            numerator,
            denominator,
            preservation: Preservation::default(),
        }
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        let mut parsed = parse_element(raw, inherited)?;
        let mut fraction_type = FractionType::Bar;
        let mut numerator = MathArgument::default();
        let mut denominator = MathArgument::default();
        let mut modeled = 0usize;
        for child in parsed.children {
            match math_local_name(&child, &parsed.bindings)?.as_deref() {
                Some("fPr") if modeled == 0 => {
                    if let Some(value) =
                        child_property_value(&child, &parsed.bindings, "fPr", "type")?
                    {
                        fraction_type = FractionType::parse(&value).unwrap_or_default();
                    }
                    preserve_modeled_child(
                        &mut parsed.preservation,
                        "fPr",
                        &child,
                        &parsed.bindings,
                    );
                    modeled += 1;
                }
                Some("num") => {
                    numerator = MathArgument::from_raw(&child, &parsed.bindings)?;
                    modeled += 1;
                }
                Some("den") => {
                    denominator = MathArgument::from_raw(&child, &parsed.bindings)?;
                    modeled += 1;
                }
                _ => parsed.preservation.raw_children.push((modeled, child)),
            }
        }
        Ok(Self {
            fraction_type,
            numerator,
            denominator,
            preservation: parsed.preservation,
        })
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        write_container(
            writer,
            "f",
            &self.preservation,
            |writer| {
                let mut modeled = write_leading_property_container(
                    writer,
                    "fPr",
                    &[Property::text("type", self.fraction_type.as_str())],
                    &self.preservation,
                    true,
                    emission,
                )?;
                self.numerator.write_xml_in(writer, "num", emission)?;
                modeled += 1;
                write_raw_slot(writer, &self.preservation, modeled)?;
                self.denominator.write_xml_in(writer, "den", emission)?;
                modeled += 1;
                write_raw_slot(writer, &self.preservation, modeled)
            },
            emission,
        )
    }
}

/// Base and one script argument, used for subscript and superscript forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathScript {
    pub base: MathArgument,
    pub script: MathArgument,
    pub alignment: Option<bool>,
    preservation: Preservation,
}

impl MathScript {
    pub fn new(base: MathArgument, script: MathArgument) -> Self {
        Self {
            base,
            script,
            alignment: None,
            preservation: Preservation::default(),
        }
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        let mut parsed = parse_element(raw, inherited)?;
        let root = math_local_name(raw, inherited)?.unwrap_or_default();
        let script_tag = if root == "sSub" { "sub" } else { "sup" };
        let property_tag = if root == "sSub" { "sSubPr" } else { "sSupPr" };
        let mut base = MathArgument::default();
        let mut script = MathArgument::default();
        let mut alignment = None;
        let mut modeled = 0usize;
        for child in parsed.children {
            match math_local_name(&child, &parsed.bindings)?.as_deref() {
                Some(local) if local == property_tag && modeled == 0 => {
                    alignment = child_on_off_value(&child, &parsed.bindings, "alnScr")?;
                    preserve_modeled_child(
                        &mut parsed.preservation,
                        property_tag,
                        &child,
                        &parsed.bindings,
                    );
                    modeled += 1;
                }
                Some("e") => {
                    base = MathArgument::from_raw(&child, &parsed.bindings)?;
                    modeled += 1;
                }
                Some(local) if local == script_tag => {
                    script = MathArgument::from_raw(&child, &parsed.bindings)?;
                    modeled += 1;
                }
                _ => parsed.preservation.raw_children.push((modeled, child)),
            }
        }
        Ok(Self {
            base,
            script,
            alignment,
            preservation: parsed.preservation,
        })
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        tag: &str,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let property_tag = if tag == "sSub" { "sSubPr" } else { "sSupPr" };
        let script_tag = if tag == "sSub" { "sub" } else { "sup" };
        write_container(
            writer,
            tag,
            &self.preservation,
            |writer| {
                let properties = self
                    .alignment
                    .map(|value| Property::boolean("alnScr", value))
                    .into_iter()
                    .collect::<Vec<_>>();
                let emit_properties = self.alignment.is_some()
                    || has_preserved_modeled_child(&self.preservation, property_tag);
                let mut modeled = write_leading_property_container(
                    writer,
                    property_tag,
                    &properties,
                    &self.preservation,
                    emit_properties,
                    emission,
                )?;
                self.base.write_xml_in(writer, "e", emission)?;
                modeled += 1;
                write_raw_slot(writer, &self.preservation, modeled)?;
                self.script.write_xml_in(writer, script_tag, emission)?;
                modeled += 1;
                write_raw_slot(writer, &self.preservation, modeled)
            },
            emission,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathSubSuperscript {
    pub base: MathArgument,
    pub subscript: MathArgument,
    pub superscript: MathArgument,
    pub alignment: Option<bool>,
    preservation: Preservation,
}

impl MathSubSuperscript {
    pub fn new(base: MathArgument, subscript: MathArgument, superscript: MathArgument) -> Self {
        Self {
            base,
            subscript,
            superscript,
            alignment: None,
            preservation: Preservation::default(),
        }
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        parse_three_argument_script(raw, inherited, false).map(|value| Self {
            base: value.0,
            subscript: value.1,
            superscript: value.2,
            alignment: value.3,
            preservation: value.4,
        })
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        write_three_argument_script(
            writer,
            "sSubSup",
            "sSubSupPr",
            &self.base,
            &self.subscript,
            &self.superscript,
            self.alignment,
            &self.preservation,
            false,
            emission,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathPreSubSuperscript {
    pub base: MathArgument,
    pub subscript: MathArgument,
    pub superscript: MathArgument,
    pub alignment: Option<bool>,
    preservation: Preservation,
}

impl MathPreSubSuperscript {
    pub fn new(base: MathArgument, subscript: MathArgument, superscript: MathArgument) -> Self {
        Self {
            base,
            subscript,
            superscript,
            alignment: None,
            preservation: Preservation::default(),
        }
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        parse_three_argument_script(raw, inherited, true).map(|value| Self {
            base: value.0,
            subscript: value.1,
            superscript: value.2,
            alignment: value.3,
            preservation: value.4,
        })
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        write_three_argument_script(
            writer,
            "sPre",
            "sPrePr",
            &self.base,
            &self.subscript,
            &self.superscript,
            self.alignment,
            &self.preservation,
            true,
            emission,
        )
    }
}

type ThreeArgumentScript = (
    MathArgument,
    MathArgument,
    MathArgument,
    Option<bool>,
    Preservation,
);

fn parse_three_argument_script(
    raw: &[u8],
    inherited: &[(String, String)],
    pre: bool,
) -> Result<ThreeArgumentScript> {
    let mut parsed = parse_element(raw, inherited)?;
    let property_tag = if pre { "sPrePr" } else { "sSubSupPr" };
    let mut base = MathArgument::default();
    let mut subscript = MathArgument::default();
    let mut superscript = MathArgument::default();
    let mut alignment = None;
    let mut modeled = 0usize;
    for child in parsed.children {
        match math_local_name(&child, &parsed.bindings)?.as_deref() {
            Some(local) if local == property_tag && modeled == 0 => {
                alignment = child_on_off_value(&child, &parsed.bindings, "alnScr")?;
                preserve_modeled_child(
                    &mut parsed.preservation,
                    property_tag,
                    &child,
                    &parsed.bindings,
                );
                modeled += 1;
            }
            Some("e") => {
                base = MathArgument::from_raw(&child, &parsed.bindings)?;
                modeled += 1;
            }
            Some("sub") => {
                subscript = MathArgument::from_raw(&child, &parsed.bindings)?;
                modeled += 1;
            }
            Some("sup") => {
                superscript = MathArgument::from_raw(&child, &parsed.bindings)?;
                modeled += 1;
            }
            _ => parsed.preservation.raw_children.push((modeled, child)),
        }
    }
    Ok((base, subscript, superscript, alignment, parsed.preservation))
}

#[allow(clippy::too_many_arguments)]
fn write_three_argument_script<W: Write>(
    writer: &mut Writer<W>,
    root_tag: &str,
    property_tag: &str,
    base: &MathArgument,
    subscript: &MathArgument,
    superscript: &MathArgument,
    alignment: Option<bool>,
    preservation: &Preservation,
    pre: bool,
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<()> {
    write_container(
        writer,
        root_tag,
        preservation,
        |writer| {
            let properties = alignment
                .map(|value| Property::boolean("alnScr", value))
                .into_iter()
                .collect::<Vec<_>>();
            let emit_properties =
                alignment.is_some() || has_preserved_modeled_child(preservation, property_tag);
            let mut modeled = write_leading_property_container(
                writer,
                property_tag,
                &properties,
                preservation,
                emit_properties,
                emission,
            )?;
            if pre {
                subscript.write_xml_in(writer, "sub", emission)?;
                modeled += 1;
                write_raw_slot(writer, preservation, modeled)?;
                superscript.write_xml_in(writer, "sup", emission)?;
                modeled += 1;
                write_raw_slot(writer, preservation, modeled)?;
                base.write_xml_in(writer, "e", emission)?;
            } else {
                base.write_xml_in(writer, "e", emission)?;
                modeled += 1;
                write_raw_slot(writer, preservation, modeled)?;
                subscript.write_xml_in(writer, "sub", emission)?;
                modeled += 1;
                write_raw_slot(writer, preservation, modeled)?;
                superscript.write_xml_in(writer, "sup", emission)?;
            }
            modeled += 1;
            write_raw_slot(writer, preservation, modeled)
        },
        emission,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathRadical {
    pub degree: MathArgument,
    pub base: MathArgument,
    pub hide_degree: bool,
    degree_present: bool,
    preservation: Preservation,
}

impl MathRadical {
    pub fn new(base: MathArgument) -> Self {
        Self {
            degree: MathArgument::default(),
            base,
            hide_degree: true,
            degree_present: true,
            preservation: Preservation::default(),
        }
    }

    pub fn with_degree(degree: MathArgument, base: MathArgument) -> Self {
        Self {
            degree,
            base,
            hide_degree: false,
            degree_present: true,
            preservation: Preservation::default(),
        }
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        let mut parsed = parse_element(raw, inherited)?;
        let mut degree = MathArgument::default();
        let mut base = MathArgument::default();
        let mut hide_degree = false;
        let mut degree_present = false;
        let mut modeled = 0usize;
        for child in parsed.children {
            match math_local_name(&child, &parsed.bindings)?.as_deref() {
                Some("radPr") if modeled == 0 => {
                    hide_degree =
                        child_on_off_value(&child, &parsed.bindings, "degHide")?.unwrap_or(false);
                    preserve_modeled_child(
                        &mut parsed.preservation,
                        "radPr",
                        &child,
                        &parsed.bindings,
                    );
                    modeled += 1;
                }
                Some("deg") => {
                    degree = MathArgument::from_raw(&child, &parsed.bindings)?;
                    degree_present = true;
                    modeled += 1;
                }
                Some("e") => {
                    base = MathArgument::from_raw(&child, &parsed.bindings)?;
                    modeled += 1;
                }
                _ => parsed.preservation.raw_children.push((modeled, child)),
            }
        }
        Ok(Self {
            degree,
            base,
            hide_degree,
            degree_present,
            preservation: parsed.preservation,
        })
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        write_container(
            writer,
            "rad",
            &self.preservation,
            |writer| {
                let mut modeled = write_leading_property_container_before_raw(
                    writer,
                    "radPr",
                    &[Property::boolean("degHide", self.hide_degree)],
                    &self.preservation,
                    true,
                    emission,
                )?;
                let emit_degree = self.degree_present
                    || !self.degree.expressions.is_empty()
                    || self.degree.has_unsupported_content();
                if self.degree_present {
                    write_raw_slot(writer, &self.preservation, modeled)?;
                    self.degree.write_xml_in(writer, "deg", emission)?;
                    modeled += 1;
                } else if emit_degree {
                    self.degree.write_xml_in(writer, "deg", emission)?;
                }
                write_raw_slot(writer, &self.preservation, modeled)?;
                self.base.write_xml_in(writer, "e", emission)?;
                modeled += 1;
                write_raw_slot(writer, &self.preservation, modeled)
            },
            emission,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathJustification {
    Left,
    Center,
    Right,
    CenterGroup,
}

impl MathJustification {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "left" => Some(Self::Left),
            "center" => Some(Self::Center),
            "right" => Some(Self::Right),
            "centerGroup" => Some(Self::CenterGroup),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
            Self::CenterGroup => "centerGroup",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MathParagraphProperties {
    pub justification: Option<MathJustification>,
    preservation: Preservation,
}

impl MathParagraphProperties {
    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        Ok(Self {
            justification: child_property_value(raw, inherited, "oMathParaPr", "jc")?
                .and_then(|value| MathJustification::parse(&value)),
            preservation: preserve_unrecognized_property_children(raw, inherited, &["jc"])?,
        })
    }

    fn is_empty(&self) -> bool {
        self.justification.is_none() && self.preservation == Preservation::default()
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let properties = self
            .justification
            .map(|value| Property::text("jc", value.as_str()))
            .into_iter()
            .collect::<Vec<_>>();
        write_property_container_with_preservation(
            writer,
            "oMathParaPr",
            &properties,
            &self.preservation,
            emission,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixBaseJustification {
    Top,
    Center,
    Bottom,
}

impl MatrixBaseJustification {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "top" => Some(Self::Top),
            "center" => Some(Self::Center),
            "bottom" => Some(Self::Bottom),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Center => "center",
            Self::Bottom => "bottom",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MathMatrixProperties {
    pub base_justification: Option<MatrixBaseJustification>,
    pub hide_placeholders: Option<bool>,
    pub row_spacing_rule: Option<u8>,
    pub column_gap_rule: Option<u8>,
    pub row_spacing: Option<u16>,
    pub column_spacing: Option<u32>,
    pub column_gap: Option<u32>,
}

/// How a matrix column aligns its cells (`m:mcJc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MatrixColumnJustification {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MathMatrixRow {
    pub cells: Vec<MathArgument>,
    preservation: Preservation,
}

impl MathMatrixRow {
    pub fn new(cells: Vec<MathArgument>) -> Self {
        Self {
            cells,
            preservation: Preservation::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MathMatrix {
    pub properties: MathMatrixProperties,
    pub rows: Vec<MathMatrixRow>,
    preservation: Preservation,
}

impl MathMatrix {
    pub fn new(rows: Vec<MathMatrixRow>) -> Self {
        Self {
            rows,
            ..Self::default()
        }
    }

    /// How column `column` (from 0) aligns its cells, read from the
    /// column properties (`m:mcs`) the matrix was read with: centred when
    /// they say nothing of it.
    pub fn column_justification(&self, column: usize) -> MatrixColumnJustification {
        let Some(properties) = self
            .preservation
            .modeled_children
            .iter()
            .find(|child| child.name == "mPr")
        else {
            return MatrixColumnJustification::default();
        };
        matrix_columns(&properties.raw, &properties.bindings)
            .and_then(|columns| {
                columns
                    .into_iter()
                    .flat_map(|(count, justification)| {
                        std::iter::repeat_n(justification, count as usize)
                    })
                    .nth(column)
            })
            .unwrap_or_default()
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        let mut parsed = parse_element(raw, inherited)?;
        let mut properties = MathMatrixProperties::default();
        let mut rows = Vec::new();
        let mut modeled = 0usize;
        for child in parsed.children {
            match math_local_name(&child, &parsed.bindings)?.as_deref() {
                Some("mPr") if modeled == 0 => {
                    properties.base_justification =
                        child_property_value(&child, &parsed.bindings, "mPr", "baseJc")?
                            .and_then(|value| MatrixBaseJustification::parse(&value));
                    properties.row_spacing =
                        child_property_value(&child, &parsed.bindings, "mPr", "rSp")?
                            .and_then(|value| value.parse().ok());
                    properties.column_spacing =
                        child_property_value(&child, &parsed.bindings, "mPr", "cSp")?
                            .and_then(|value| value.parse().ok());
                    properties.hide_placeholders =
                        child_on_off_value(&child, &parsed.bindings, "plcHide")?;
                    properties.row_spacing_rule =
                        child_property_value(&child, &parsed.bindings, "mPr", "rSpRule")?
                            .and_then(|value| value.parse().ok());
                    properties.column_gap_rule =
                        child_property_value(&child, &parsed.bindings, "mPr", "cGpRule")?
                            .and_then(|value| value.parse().ok());
                    properties.column_gap =
                        child_property_value(&child, &parsed.bindings, "mPr", "cGp")?
                            .and_then(|value| value.parse().ok());
                    preserve_modeled_child(
                        &mut parsed.preservation,
                        "mPr",
                        &child,
                        &parsed.bindings,
                    );
                    modeled += 1;
                }
                Some("mr") => {
                    rows.push(parse_matrix_row(&child, &parsed.bindings)?);
                    modeled += 1;
                }
                _ => parsed.preservation.raw_children.push((modeled, child)),
            }
        }
        Ok(Self {
            properties,
            rows,
            preservation: parsed.preservation,
        })
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        if self.rows.is_empty() || self.rows.iter().any(|row| row.cells.is_empty()) {
            return Err(OxmlError::InvalidValue(
                "OfficeMath matrix requires at least one nonempty row".to_owned(),
            ));
        }
        if [
            self.properties.row_spacing_rule,
            self.properties.column_gap_rule,
        ]
        .into_iter()
        .flatten()
        .any(|rule| rule > 4)
        {
            return Err(OxmlError::InvalidValue(
                "OfficeMath matrix spacing rules go up to 4".to_owned(),
            ));
        }
        write_container(
            writer,
            "m",
            &self.preservation,
            |writer| {
                let properties = [
                    self.properties
                        .base_justification
                        .map(|value| Property::text("baseJc", value.as_str())),
                    self.properties
                        .hide_placeholders
                        .map(|value| Property::boolean("plcHide", value)),
                    self.properties
                        .row_spacing_rule
                        .map(|value| Property::unsigned("rSpRule", value.into())),
                    self.properties
                        .column_gap_rule
                        .map(|value| Property::unsigned("cGpRule", value.into())),
                    self.properties
                        .row_spacing
                        .map(|value| Property::unsigned("rSp", value.into())),
                    self.properties
                        .column_spacing
                        .map(|value| Property::unsigned("cSp", value.into())),
                    self.properties
                        .column_gap
                        .map(|value| Property::unsigned("cGp", value.into())),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
                let emit_properties = !properties.is_empty()
                    || has_preserved_modeled_child(&self.preservation, "mPr");
                let mut modeled = write_leading_property_container(
                    writer,
                    "mPr",
                    &properties,
                    &self.preservation,
                    emit_properties,
                    emission,
                )?;
                for row in &self.rows {
                    write_container(
                        writer,
                        "mr",
                        &row.preservation,
                        |writer| {
                            write_raw_slot(writer, &row.preservation, 0)?;
                            for (index, cell) in row.cells.iter().enumerate() {
                                cell.write_xml_in(writer, "e", emission)?;
                                write_raw_slot(writer, &row.preservation, index + 1)?;
                            }
                            write_raw_tail(writer, &row.preservation, row.cells.len() + 1)
                        },
                        emission,
                    )?;
                    modeled += 1;
                    write_raw_slot(writer, &self.preservation, modeled)?;
                }
                write_raw_tail(writer, &self.preservation, modeled + 1)
            },
            emission,
        )
    }
}

fn parse_matrix_row(raw: &[u8], inherited: &[(String, String)]) -> Result<MathMatrixRow> {
    let mut parsed = parse_element(raw, inherited)?;
    let mut cells = Vec::new();
    for child in parsed.children {
        if math_local_name(&child, &parsed.bindings)?.as_deref() == Some("e") {
            cells.push(MathArgument::from_raw(&child, &parsed.bindings)?);
        } else {
            parsed.preservation.raw_children.push((cells.len(), child));
        }
    }
    Ok(MathMatrixRow {
        cells,
        preservation: parsed.preservation,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathLimit {
    pub base: MathArgument,
    pub limit: MathArgument,
    preservation: Preservation,
}

impl MathLimit {
    pub fn new(base: MathArgument, limit: MathArgument) -> Self {
        Self {
            base,
            limit,
            preservation: Preservation::default(),
        }
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        let mut parsed = parse_element(raw, inherited)?;
        let mut base = MathArgument::default();
        let mut limit = MathArgument::default();
        let mut modeled = 0usize;
        for child in parsed.children {
            match math_local_name(&child, &parsed.bindings)?.as_deref() {
                Some(property @ ("limLowPr" | "limUppPr")) if modeled == 0 => {
                    preserve_modeled_child(
                        &mut parsed.preservation,
                        property,
                        &child,
                        &parsed.bindings,
                    );
                    modeled += 1;
                }
                Some("e") => {
                    base = MathArgument::from_raw(&child, &parsed.bindings)?;
                    modeled += 1;
                }
                Some("lim") => {
                    limit = MathArgument::from_raw(&child, &parsed.bindings)?;
                    modeled += 1;
                }
                _ => parsed.preservation.raw_children.push((modeled, child)),
            }
        }
        Ok(Self {
            base,
            limit,
            preservation: parsed.preservation,
        })
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        tag: &str,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        write_container(
            writer,
            tag,
            &self.preservation,
            |writer| {
                let property_tag = if tag == "limLow" {
                    "limLowPr"
                } else {
                    "limUppPr"
                };
                let mut modeled = 0usize;
                write_raw_slot(writer, &self.preservation, modeled)?;
                if has_preserved_modeled_child(&self.preservation, property_tag) {
                    write_property_container_from_parent(
                        writer,
                        property_tag,
                        &[],
                        &self.preservation,
                        emission,
                    )?;
                    modeled += 1;
                    write_raw_slot(writer, &self.preservation, modeled)?;
                }
                self.base.write_xml_in(writer, "e", emission)?;
                modeled += 1;
                write_raw_slot(writer, &self.preservation, modeled)?;
                self.limit.write_xml_in(writer, "lim", emission)?;
                modeled += 1;
                write_raw_slot(writer, &self.preservation, modeled)
            },
            emission,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitLocation {
    SubSuperscript,
    UnderOver,
}

impl LimitLocation {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "subSup" => Some(Self::SubSuperscript),
            "undOvr" => Some(Self::UnderOver),
            _ => None,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::SubSuperscript => "subSup",
            Self::UnderOver => "undOvr",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathNary {
    pub character: String,
    pub base: MathArgument,
    pub subscript: MathArgument,
    pub superscript: MathArgument,
    pub hide_subscript: bool,
    pub hide_superscript: bool,
    pub grow: Option<bool>,
    pub limit_location: Option<LimitLocation>,
    subscript_present: bool,
    superscript_present: bool,
    // Google Docs writes an n-ary without its m:e.
    base_present: bool,
    preservation: Preservation,
}

impl MathNary {
    pub fn new(character: impl Into<String>, base: MathArgument) -> Self {
        Self {
            character: character.into(),
            base,
            subscript: MathArgument::default(),
            superscript: MathArgument::default(),
            hide_subscript: true,
            hide_superscript: true,
            grow: None,
            limit_location: None,
            subscript_present: false,
            superscript_present: false,
            base_present: true,
            preservation: Preservation::default(),
        }
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        let mut parsed = parse_element(raw, inherited)?;
        let mut value = Self::new("∫", MathArgument::default());
        value.hide_subscript = false;
        value.hide_superscript = false;
        value.base_present = false;
        let mut modeled = 0usize;
        for child in parsed.children {
            match math_local_name(&child, &parsed.bindings)?.as_deref() {
                Some("naryPr") if modeled == 0 => {
                    value.character =
                        child_property_value(&child, &parsed.bindings, "naryPr", "chr")?
                            .unwrap_or(value.character);
                    value.hide_subscript =
                        child_on_off_value(&child, &parsed.bindings, "subHide")?.unwrap_or(false);
                    value.hide_superscript =
                        child_on_off_value(&child, &parsed.bindings, "supHide")?.unwrap_or(false);
                    value.grow = child_on_off_value(&child, &parsed.bindings, "grow")?;
                    value.limit_location =
                        child_property_value(&child, &parsed.bindings, "naryPr", "limLoc")?
                            .and_then(|value| LimitLocation::parse(&value));
                    preserve_modeled_child(
                        &mut parsed.preservation,
                        "naryPr",
                        &child,
                        &parsed.bindings,
                    );
                    modeled += 1;
                }
                Some("sub") => {
                    value.subscript = MathArgument::from_raw(&child, &parsed.bindings)?;
                    value.subscript_present = true;
                    modeled += 1;
                }
                Some("sup") => {
                    value.superscript = MathArgument::from_raw(&child, &parsed.bindings)?;
                    value.superscript_present = true;
                    modeled += 1;
                }
                Some("e") => {
                    value.base = MathArgument::from_raw(&child, &parsed.bindings)?;
                    value.base_present = true;
                    modeled += 1;
                }
                _ => parsed.preservation.raw_children.push((modeled, child)),
            }
        }
        value.preservation = parsed.preservation;
        Ok(value)
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        if self.character.chars().count() > 1 {
            return Err(OxmlError::InvalidValue(
                "OfficeMath n-ary character must contain at most one Unicode scalar".to_owned(),
            ));
        }
        write_container(
            writer,
            "nary",
            &self.preservation,
            |writer| {
                let mut properties = vec![Property::text("chr", &self.character)];
                if let Some(value) = self.limit_location {
                    properties.push(Property::text("limLoc", value.as_str()));
                }
                if let Some(value) = self.grow {
                    properties.push(Property::boolean("grow", value));
                }
                properties.push(Property::boolean("subHide", self.hide_subscript));
                properties.push(Property::boolean("supHide", self.hide_superscript));
                let mut modeled = write_leading_property_container_before_raw(
                    writer,
                    "naryPr",
                    &properties,
                    &self.preservation,
                    true,
                    emission,
                )?;
                let emit_subscript = self.subscript_present
                    || !self.subscript.expressions.is_empty()
                    || self.subscript.has_unsupported_content();
                if self.subscript_present {
                    write_raw_slot(writer, &self.preservation, modeled)?;
                    self.subscript.write_xml_in(writer, "sub", emission)?;
                    modeled += 1;
                } else if emit_subscript {
                    self.subscript.write_xml_in(writer, "sub", emission)?;
                }
                let emit_superscript = self.superscript_present
                    || !self.superscript.expressions.is_empty()
                    || self.superscript.has_unsupported_content();
                if self.superscript_present {
                    write_raw_slot(writer, &self.preservation, modeled)?;
                    self.superscript.write_xml_in(writer, "sup", emission)?;
                    modeled += 1;
                } else if emit_superscript {
                    self.superscript.write_xml_in(writer, "sup", emission)?;
                }
                write_raw_slot(writer, &self.preservation, modeled)?;
                if self.base_present
                    || !self.base.expressions.is_empty()
                    || self.base.has_unsupported_content()
                {
                    self.base.write_xml_in(writer, "e", emission)?;
                    modeled += 1;
                    write_raw_slot(writer, &self.preservation, modeled)?;
                }
                Ok(())
            },
            emission,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathDelimiter {
    pub begin_character: String,
    pub end_character: String,
    pub separator_character: String,
    pub grow: Option<bool>,
    pub arguments: Vec<MathArgument>,
    preservation: Preservation,
}

impl MathDelimiter {
    pub fn new(
        begin: impl Into<String>,
        end: impl Into<String>,
        arguments: Vec<MathArgument>,
    ) -> Self {
        Self {
            begin_character: begin.into(),
            end_character: end.into(),
            separator_character: "|".to_owned(),
            grow: None,
            arguments,
            preservation: Preservation::default(),
        }
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        let mut parsed = parse_element(raw, inherited)?;
        let mut value = Self::new("(", ")", Vec::new());
        let mut modeled = 0usize;
        for child in parsed.children {
            match math_local_name(&child, &parsed.bindings)?.as_deref() {
                Some("dPr") if modeled == 0 => {
                    value.begin_character =
                        child_property_value(&child, &parsed.bindings, "dPr", "begChr")?
                            .unwrap_or(value.begin_character);
                    value.end_character =
                        child_property_value(&child, &parsed.bindings, "dPr", "endChr")?
                            .unwrap_or(value.end_character);
                    value.separator_character =
                        child_property_value(&child, &parsed.bindings, "dPr", "sepChr")?
                            .unwrap_or(value.separator_character);
                    value.grow = child_on_off_value(&child, &parsed.bindings, "grow")?;
                    preserve_modeled_child(
                        &mut parsed.preservation,
                        "dPr",
                        &child,
                        &parsed.bindings,
                    );
                    modeled += 1;
                }
                Some("e") => {
                    value
                        .arguments
                        .push(MathArgument::from_raw(&child, &parsed.bindings)?);
                    modeled += 1;
                }
                _ => parsed.preservation.raw_children.push((modeled, child)),
            }
        }
        value.preservation = parsed.preservation;
        Ok(value)
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        if self.arguments.is_empty()
            || [
                &self.begin_character,
                &self.separator_character,
                &self.end_character,
            ]
            .iter()
            .any(|value| value.chars().count() > 1)
        {
            return Err(OxmlError::InvalidValue(
                "OfficeMath delimiter requires arguments and scalar characters".to_owned(),
            ));
        }
        write_container(
            writer,
            "d",
            &self.preservation,
            |writer| {
                let mut properties = vec![
                    Property::text("begChr", &self.begin_character),
                    Property::text("sepChr", &self.separator_character),
                    Property::text("endChr", &self.end_character),
                ];
                if let Some(value) = self.grow {
                    properties.push(Property::boolean("grow", value));
                }
                let mut modeled = write_leading_property_container(
                    writer,
                    "dPr",
                    &properties,
                    &self.preservation,
                    true,
                    emission,
                )?;
                for argument in &self.arguments {
                    argument.write_xml_in(writer, "e", emission)?;
                    modeled += 1;
                    write_raw_slot(writer, &self.preservation, modeled)?;
                }
                write_raw_tail(writer, &self.preservation, modeled + 1)
            },
            emission,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathAccent {
    pub character: String,
    pub base: MathArgument,
    preservation: Preservation,
}

impl MathAccent {
    pub fn new(character: impl Into<String>, base: MathArgument) -> Self {
        Self {
            character: character.into(),
            base,
            preservation: Preservation::default(),
        }
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        let mut parsed = parse_element(raw, inherited)?;
        let mut character = "̂".to_owned();
        let mut base = MathArgument::default();
        let mut modeled = 0usize;
        for child in parsed.children {
            match math_local_name(&child, &parsed.bindings)?.as_deref() {
                Some("accPr") if modeled == 0 => {
                    character = child_property_value(&child, &parsed.bindings, "accPr", "chr")?
                        .unwrap_or(character);
                    preserve_modeled_child(
                        &mut parsed.preservation,
                        "accPr",
                        &child,
                        &parsed.bindings,
                    );
                    modeled += 1;
                }
                Some("e") => {
                    base = MathArgument::from_raw(&child, &parsed.bindings)?;
                    modeled += 1;
                }
                _ => parsed.preservation.raw_children.push((modeled, child)),
            }
        }
        Ok(Self {
            character,
            base,
            preservation: parsed.preservation,
        })
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        if self.character.chars().count() > 1 {
            return Err(OxmlError::InvalidValue(
                "OfficeMath accent character must contain at most one Unicode scalar".to_owned(),
            ));
        }
        write_container(
            writer,
            "acc",
            &self.preservation,
            |writer| {
                let mut modeled = write_leading_property_container(
                    writer,
                    "accPr",
                    &[Property::text("chr", &self.character)],
                    &self.preservation,
                    true,
                    emission,
                )?;
                self.base.write_xml_in(writer, "e", emission)?;
                modeled += 1;
                write_raw_slot(writer, &self.preservation, modeled)
            },
            emission,
        )
    }
}

/// Above or below: where a bar or a group character sits (`ST_TopBot`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathVerticalPosition {
    Top,
    Bottom,
}

impl MathVerticalPosition {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "top" => Some(Self::Top),
            "bot" => Some(Self::Bottom),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bot",
        }
    }
}

/// A function apply, `m:func`: a function name such as "log" and its argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathFunction {
    pub name: MathArgument,
    pub argument: MathArgument,
    preservation: Preservation,
}

impl MathFunction {
    pub fn new(name: MathArgument, argument: MathArgument) -> Self {
        Self {
            name,
            argument,
            preservation: Preservation::default(),
        }
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        let mut parsed = parse_element(raw, inherited)?;
        let mut name = MathArgument::default();
        let mut argument = MathArgument::default();
        let mut modeled = 0usize;
        for child in parsed.children {
            match math_local_name(&child, &parsed.bindings)?.as_deref() {
                Some("funcPr") if modeled == 0 => {
                    preserve_modeled_child(
                        &mut parsed.preservation,
                        "funcPr",
                        &child,
                        &parsed.bindings,
                    );
                    modeled += 1;
                }
                Some("fName") => {
                    name = MathArgument::from_raw(&child, &parsed.bindings)?;
                    modeled += 1;
                }
                Some("e") => {
                    argument = MathArgument::from_raw(&child, &parsed.bindings)?;
                    modeled += 1;
                }
                _ => parsed.preservation.raw_children.push((modeled, child)),
            }
        }
        Ok(Self {
            name,
            argument,
            preservation: parsed.preservation,
        })
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        write_container(
            writer,
            "func",
            &self.preservation,
            |writer| {
                let emit = has_preserved_modeled_child(&self.preservation, "funcPr");
                let mut modeled = write_leading_property_container(
                    writer,
                    "funcPr",
                    &[],
                    &self.preservation,
                    emit,
                    emission,
                )?;
                self.name.write_xml_in(writer, "fName", emission)?;
                modeled += 1;
                write_raw_slot(writer, &self.preservation, modeled)?;
                self.argument.write_xml_in(writer, "e", emission)?;
                modeled += 1;
                write_raw_slot(writer, &self.preservation, modeled)
            },
            emission,
        )
    }
}

/// A bar over or under its base, `m:bar`. Without a position it is below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathBar {
    pub position: Option<MathVerticalPosition>,
    pub base: MathArgument,
    preservation: Preservation,
}

impl MathBar {
    pub fn new(position: Option<MathVerticalPosition>, base: MathArgument) -> Self {
        Self {
            position,
            base,
            preservation: Preservation::default(),
        }
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        let mut position = None;
        let (base, preservation) =
            parse_based_construct(raw, inherited, "barPr", |raw, bindings| {
                position = child_property_value(raw, bindings, "barPr", "pos")?
                    .and_then(|value| MathVerticalPosition::parse(&value));
                Ok(())
            })?;
        Ok(Self {
            position,
            base,
            preservation,
        })
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let properties = self
            .position
            .map(|value| Property::text("pos", value.as_str()))
            .into_iter()
            .collect::<Vec<_>>();
        write_based_construct(
            writer,
            "bar",
            "barPr",
            &properties,
            &self.base,
            &self.preservation,
            emission,
        )
    }
}

/// A box, `m:box`: its base drawn as is, the flags change how it breaks and
/// aligns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathBox {
    pub operator_emulator: Option<bool>,
    pub no_break: Option<bool>,
    pub differential: Option<bool>,
    pub alignment: Option<bool>,
    pub base: MathArgument,
    preservation: Preservation,
}

impl MathBox {
    pub fn new(base: MathArgument) -> Self {
        Self {
            operator_emulator: None,
            no_break: None,
            differential: None,
            alignment: None,
            base,
            preservation: Preservation::default(),
        }
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        let mut flags = [None; 4];
        let (base, preservation) =
            parse_based_construct(raw, inherited, "boxPr", |raw, bindings| {
                for (flag, name) in flags.iter_mut().zip(["opEmu", "noBreak", "diff", "aln"]) {
                    *flag = child_on_off_value(raw, bindings, name)?;
                }
                Ok(())
            })?;
        let [operator_emulator, no_break, differential, alignment] = flags;
        Ok(Self {
            operator_emulator,
            no_break,
            differential,
            alignment,
            base,
            preservation,
        })
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let properties = on_off_properties(&[
            ("opEmu", self.operator_emulator),
            ("noBreak", self.no_break),
            ("diff", self.differential),
            ("aln", self.alignment),
        ]);
        write_based_construct(
            writer,
            "box",
            "boxPr",
            &properties,
            &self.base,
            &self.preservation,
            emission,
        )
    }
}

/// A border around its base, `m:borderBox`, with sides hidden and strikes
/// drawn across as its flags say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathBorderBox {
    pub hide_top: Option<bool>,
    pub hide_bottom: Option<bool>,
    pub hide_left: Option<bool>,
    pub hide_right: Option<bool>,
    pub strike_horizontal: Option<bool>,
    pub strike_vertical: Option<bool>,
    pub strike_bottom_left_to_top_right: Option<bool>,
    pub strike_top_left_to_bottom_right: Option<bool>,
    pub base: MathArgument,
    preservation: Preservation,
}

const BORDER_BOX_FLAGS: [&str; 8] = [
    "hideTop",
    "hideBot",
    "hideLeft",
    "hideRight",
    "strikeH",
    "strikeV",
    "strikeBLTR",
    "strikeTLBR",
];

impl MathBorderBox {
    pub fn new(base: MathArgument) -> Self {
        Self::with_flags([None; 8], base, Preservation::default())
    }

    fn with_flags(
        flags: [Option<bool>; 8],
        base: MathArgument,
        preservation: Preservation,
    ) -> Self {
        let [
            hide_top,
            hide_bottom,
            hide_left,
            hide_right,
            strike_horizontal,
            strike_vertical,
            strike_bottom_left_to_top_right,
            strike_top_left_to_bottom_right,
        ] = flags;
        Self {
            hide_top,
            hide_bottom,
            hide_left,
            hide_right,
            strike_horizontal,
            strike_vertical,
            strike_bottom_left_to_top_right,
            strike_top_left_to_bottom_right,
            base,
            preservation,
        }
    }

    fn flags(&self) -> [Option<bool>; 8] {
        [
            self.hide_top,
            self.hide_bottom,
            self.hide_left,
            self.hide_right,
            self.strike_horizontal,
            self.strike_vertical,
            self.strike_bottom_left_to_top_right,
            self.strike_top_left_to_bottom_right,
        ]
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        let mut flags = [None; 8];
        let (base, preservation) =
            parse_based_construct(raw, inherited, "borderBoxPr", |raw, bindings| {
                for (flag, name) in flags.iter_mut().zip(BORDER_BOX_FLAGS) {
                    *flag = child_on_off_value(raw, bindings, name)?;
                }
                Ok(())
            })?;
        Ok(Self::with_flags(flags, base, preservation))
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let flags = BORDER_BOX_FLAGS
            .into_iter()
            .zip(self.flags())
            .collect::<Vec<_>>();
        write_based_construct(
            writer,
            "borderBox",
            "borderBoxPr",
            &on_off_properties(&flags),
            &self.base,
            &self.preservation,
            emission,
        )
    }
}

/// A character stretched over or under its base, `m:groupChr`: by default a
/// bottom curly bracket (U+23DF) below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathGroupCharacter {
    pub character: Option<String>,
    pub position: Option<MathVerticalPosition>,
    pub vertical_justification: Option<MathVerticalPosition>,
    pub base: MathArgument,
    preservation: Preservation,
}

impl MathGroupCharacter {
    pub fn new(base: MathArgument) -> Self {
        Self {
            character: None,
            position: None,
            vertical_justification: None,
            base,
            preservation: Preservation::default(),
        }
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        let (mut character, mut position, mut vertical_justification) = (None, None, None);
        let (base, preservation) =
            parse_based_construct(raw, inherited, "groupChrPr", |raw, bindings| {
                character = child_property_value(raw, bindings, "groupChrPr", "chr")?;
                position = child_property_value(raw, bindings, "groupChrPr", "pos")?
                    .and_then(|value| MathVerticalPosition::parse(&value));
                vertical_justification =
                    child_property_value(raw, bindings, "groupChrPr", "vertJc")?
                        .and_then(|value| MathVerticalPosition::parse(&value));
                Ok(())
            })?;
        Ok(Self {
            character,
            position,
            vertical_justification,
            base,
            preservation,
        })
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        if self
            .character
            .as_ref()
            .is_some_and(|value| value.chars().count() > 1)
        {
            return Err(OxmlError::InvalidValue(
                "OfficeMath group character must contain at most one Unicode scalar".to_owned(),
            ));
        }
        let properties = [
            self.character
                .as_deref()
                .map(|value| Property::text("chr", value)),
            self.position
                .map(|value| Property::text("pos", value.as_str())),
            self.vertical_justification
                .map(|value| Property::text("vertJc", value.as_str())),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        write_based_construct(
            writer,
            "groupChr",
            "groupChrPr",
            &properties,
            &self.base,
            &self.preservation,
            emission,
        )
    }
}

/// A phantom, `m:phant`: its base takes room, drawn only when `show` is on,
/// each dimension zeroed when asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathPhantom {
    pub show: Option<bool>,
    pub zero_width: Option<bool>,
    pub zero_ascent: Option<bool>,
    pub zero_descent: Option<bool>,
    pub transparent: Option<bool>,
    pub base: MathArgument,
    preservation: Preservation,
}

const PHANTOM_FLAGS: [&str; 5] = ["show", "zeroWid", "zeroAsc", "zeroDesc", "transp"];

impl MathPhantom {
    pub fn new(base: MathArgument) -> Self {
        Self {
            show: None,
            zero_width: None,
            zero_ascent: None,
            zero_descent: None,
            transparent: None,
            base,
            preservation: Preservation::default(),
        }
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        let mut flags = [None; 5];
        let (base, preservation) =
            parse_based_construct(raw, inherited, "phantPr", |raw, bindings| {
                for (flag, name) in flags.iter_mut().zip(PHANTOM_FLAGS) {
                    *flag = child_on_off_value(raw, bindings, name)?;
                }
                Ok(())
            })?;
        let [show, zero_width, zero_ascent, zero_descent, transparent] = flags;
        Ok(Self {
            show,
            zero_width,
            zero_ascent,
            zero_descent,
            transparent,
            base,
            preservation,
        })
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let flags = [
            self.show,
            self.zero_width,
            self.zero_ascent,
            self.zero_descent,
            self.transparent,
        ];
        let flags = PHANTOM_FLAGS.into_iter().zip(flags).collect::<Vec<_>>();
        write_based_construct(
            writer,
            "phant",
            "phantPr",
            &on_off_properties(&flags),
            &self.base,
            &self.preservation,
            emission,
        )
    }
}

/// Equations stacked and aligned on their `&` marks, `m:eqArr`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathEquationArray {
    pub base_justification: Option<MatrixBaseJustification>,
    pub max_distribution: Option<bool>,
    pub object_distribution: Option<bool>,
    pub row_spacing_rule: Option<u8>,
    pub row_spacing: Option<u16>,
    pub rows: Vec<MathArgument>,
    preservation: Preservation,
}

impl MathEquationArray {
    pub fn new(rows: Vec<MathArgument>) -> Self {
        Self {
            base_justification: None,
            max_distribution: None,
            object_distribution: None,
            row_spacing_rule: None,
            row_spacing: None,
            rows,
            preservation: Preservation::default(),
        }
    }

    fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        let mut parsed = parse_element(raw, inherited)?;
        let mut value = Self::new(Vec::new());
        let mut modeled = 0usize;
        for child in parsed.children {
            match math_local_name(&child, &parsed.bindings)?.as_deref() {
                Some("eqArrPr") if modeled == 0 => {
                    let bindings = &parsed.bindings;
                    value.base_justification =
                        child_property_value(&child, bindings, "eqArrPr", "baseJc")?
                            .and_then(|value| MatrixBaseJustification::parse(&value));
                    value.max_distribution = child_on_off_value(&child, bindings, "maxDist")?;
                    value.object_distribution = child_on_off_value(&child, bindings, "objDist")?;
                    value.row_spacing_rule =
                        child_property_value(&child, bindings, "eqArrPr", "rSpRule")?
                            .and_then(|value| value.parse().ok());
                    value.row_spacing = child_property_value(&child, bindings, "eqArrPr", "rSp")?
                        .and_then(|value| value.parse().ok());
                    preserve_modeled_child(&mut parsed.preservation, "eqArrPr", &child, bindings);
                    modeled += 1;
                }
                Some("e") => {
                    value
                        .rows
                        .push(MathArgument::from_raw(&child, &parsed.bindings)?);
                    modeled += 1;
                }
                _ => parsed.preservation.raw_children.push((modeled, child)),
            }
        }
        value.preservation = parsed.preservation;
        Ok(value)
    }

    fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        if self.rows.is_empty() || self.row_spacing_rule.is_some_and(|rule| rule > 4) {
            return Err(OxmlError::InvalidValue(
                "OfficeMath equation array requires rows and a row spacing rule up to 4".to_owned(),
            ));
        }
        write_container(
            writer,
            "eqArr",
            &self.preservation,
            |writer| {
                let mut properties = Vec::new();
                if let Some(value) = self.base_justification {
                    properties.push(Property::text("baseJc", value.as_str()));
                }
                properties.extend(on_off_properties(&[
                    ("maxDist", self.max_distribution),
                    ("objDist", self.object_distribution),
                ]));
                if let Some(value) = self.row_spacing_rule {
                    properties.push(Property::unsigned("rSpRule", value.into()));
                }
                if let Some(value) = self.row_spacing {
                    properties.push(Property::unsigned("rSp", value.into()));
                }
                let emit = !properties.is_empty()
                    || has_preserved_modeled_child(&self.preservation, "eqArrPr");
                let mut modeled = write_leading_property_container(
                    writer,
                    "eqArrPr",
                    &properties,
                    &self.preservation,
                    emit,
                    emission,
                )?;
                for row in &self.rows {
                    row.write_xml_in(writer, "e", emission)?;
                    modeled += 1;
                    write_raw_slot(writer, &self.preservation, modeled)?;
                }
                write_raw_tail(writer, &self.preservation, modeled + 1)
            },
            emission,
        )
    }
}

/// Reads a construct made of the property container `property_tag` and one
/// base `m:e`, handing the container to `properties`.
fn parse_based_construct(
    raw: &[u8],
    inherited: &[(String, String)],
    property_tag: &str,
    mut properties: impl FnMut(&[u8], &[(String, String)]) -> Result<()>,
) -> Result<(MathArgument, Preservation)> {
    let mut parsed = parse_element(raw, inherited)?;
    let mut base = MathArgument::default();
    let mut modeled = 0usize;
    for child in parsed.children {
        match math_local_name(&child, &parsed.bindings)?.as_deref() {
            Some(local) if local == property_tag && modeled == 0 => {
                properties(&child, &parsed.bindings)?;
                preserve_modeled_child(
                    &mut parsed.preservation,
                    property_tag,
                    &child,
                    &parsed.bindings,
                );
                modeled += 1;
            }
            Some("e") => {
                base = MathArgument::from_raw(&child, &parsed.bindings)?;
                modeled += 1;
            }
            _ => parsed.preservation.raw_children.push((modeled, child)),
        }
    }
    Ok((base, parsed.preservation))
}

/// Writes what [`parse_based_construct`] reads: the property container when
/// it was in the source or holds a property, then the base.
fn write_based_construct<W: Write>(
    writer: &mut Writer<W>,
    tag: &str,
    property_tag: &str,
    properties: &[Property<'_>],
    base: &MathArgument,
    preservation: &Preservation,
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<()> {
    write_container(
        writer,
        tag,
        preservation,
        |writer| {
            let emit =
                !properties.is_empty() || has_preserved_modeled_child(preservation, property_tag);
            let mut modeled = write_leading_property_container(
                writer,
                property_tag,
                properties,
                preservation,
                emit,
                emission,
            )?;
            base.write_xml_in(writer, "e", emission)?;
            modeled += 1;
            write_raw_slot(writer, preservation, modeled)
        },
        emission,
    )
}

/// The on/off properties that are set, in the order given.
fn on_off_properties<'a>(flags: &[(&'a str, Option<bool>)]) -> Vec<Property<'a>> {
    flags
        .iter()
        .filter_map(|(name, value)| value.map(|value| Property::boolean(name, value)))
        .collect()
}

/// Document-wide defaults from `w:settings/m:mathPr`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MathProperties {
    pub math_font: Option<String>,
    pub justification: Option<MathJustification>,
    pub left_margin: Option<u32>,
    pub right_margin: Option<u32>,
    pub pre_spacing: Option<u32>,
    pub post_spacing: Option<u32>,
    pub inter_spacing: Option<u32>,
    pub intra_spacing: Option<u32>,
    pub wrap_indent: Option<u32>,
    pub small_fraction: Option<bool>,
    pub display_defaults: Option<bool>,
    pub integral_limit_location: Option<LimitLocation>,
    pub nary_limit_location: Option<LimitLocation>,
    preservation: Preservation,
}

impl MathProperties {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the settings subtree retains content outside the typed subset.
    pub fn has_unsupported_content(&self) -> bool {
        property_preservation_has_unsupported_content(&self.preservation, "mathPr")
    }

    pub(crate) fn from_raw(raw: &[u8], inherited: &[(String, String)]) -> Result<Self> {
        require_safe_fixed_math_prefix(raw, inherited)?;
        let mut parsed = parse_element(raw, inherited)?;
        require_root(raw, &parsed.bindings, b"mathPr")?;
        let mut value = Self::default();
        let mut modeled = 0usize;
        let mut seen = Vec::new();
        let mut last_rank = None;
        for child in parsed.children {
            let local = math_local_name(&child, &parsed.bindings)?;
            let Some(name) = local.as_deref() else {
                let key = property_raw_key("mathPr", None, &mut last_rank, false);
                parsed.preservation.property_raw_children.push((key, child));
                continue;
            };
            let duplicate = seen.iter().any(|seen_name| seen_name == name);
            if !supported_properties("mathPr").contains(&name)
                || duplicate
                || !property_leaf_is_valid("mathPr", name, &child, &parsed.bindings)?
            {
                let key = property_raw_key("mathPr", Some(name), &mut last_rank, duplicate);
                parsed.preservation.property_raw_children.push((key, child));
                continue;
            }
            property_raw_key("mathPr", Some(name), &mut last_rank, false);
            let parsed_value = root_value(&child, &parsed.bindings)?
                .or_else(|| property_value_default("mathPr", name).map(str::to_owned));
            let recognized = match name {
                "mathFont" => {
                    value.math_font = parsed_value;
                    true
                }
                "defJc" => {
                    value.justification =
                        parsed_value.as_deref().and_then(MathJustification::parse);
                    true
                }
                "lMargin" => {
                    value.left_margin = parse_u32(parsed_value);
                    true
                }
                "rMargin" => {
                    value.right_margin = parse_u32(parsed_value);
                    true
                }
                "preSp" => {
                    value.pre_spacing = parse_u32(parsed_value);
                    true
                }
                "postSp" => {
                    value.post_spacing = parse_u32(parsed_value);
                    true
                }
                "interSp" => {
                    value.inter_spacing = parse_u32(parsed_value);
                    true
                }
                "intraSp" => {
                    value.intra_spacing = parse_u32(parsed_value);
                    true
                }
                "wrapIndent" => {
                    value.wrap_indent = parse_u32(parsed_value);
                    true
                }
                "smallFrac" => {
                    value.small_fraction = parse_present_on_off(parsed_value.as_deref());
                    true
                }
                "dispDef" => {
                    value.display_defaults = parse_present_on_off(parsed_value.as_deref());
                    true
                }
                "intLim" => {
                    value.integral_limit_location =
                        parsed_value.as_deref().and_then(LimitLocation::parse);
                    true
                }
                "naryLim" => {
                    value.nary_limit_location =
                        parsed_value.as_deref().and_then(LimitLocation::parse);
                    true
                }
                _ => false,
            };
            if recognized {
                seen.push(name.to_owned());
                preserve_modeled_child(&mut parsed.preservation, name, &child, &parsed.bindings);
                modeled += 1;
            } else {
                parsed.preservation.raw_children.push((modeled, child));
            }
        }
        value.preservation = parsed.preservation;
        Ok(value)
    }

    pub(crate) fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.write_xml_in(writer, None)
    }

    pub(crate) fn write_xml_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let mut properties = Vec::new();
        if let Some(value) = &self.math_font {
            properties.push(Property::text("mathFont", value));
        }
        if let Some(value) = self.small_fraction {
            properties.push(Property::boolean("smallFrac", value));
        }
        if let Some(value) = self.display_defaults {
            properties.push(Property::boolean("dispDef", value));
        }
        if let Some(value) = self.left_margin {
            properties.push(Property::unsigned("lMargin", value.into()));
        }
        if let Some(value) = self.right_margin {
            properties.push(Property::unsigned("rMargin", value.into()));
        }
        if let Some(value) = self.justification {
            properties.push(Property::text("defJc", value.as_str()));
        }
        for (name, value) in [
            ("preSp", self.pre_spacing),
            ("postSp", self.post_spacing),
            ("interSp", self.inter_spacing),
            ("intraSp", self.intra_spacing),
            ("wrapIndent", self.wrap_indent),
        ] {
            if let Some(value) = value {
                properties.push(Property::unsigned(name, value.into()));
            }
        }
        if let Some(value) = self.integral_limit_location {
            properties.push(Property::text("intLim", value.as_str()));
        }
        if let Some(value) = self.nary_limit_location {
            properties.push(Property::text("naryLim", value.as_str()));
        }
        write_property_container_with_preservation(
            writer,
            "mathPr",
            &properties,
            &self.preservation,
            emission,
        )
    }
}

pub(crate) fn is_math_element(name: &[u8], local: &[u8], scope: &[String]) -> bool {
    let (prefix, actual_local) = name
        .iter()
        .position(|byte| *byte == b':')
        .map_or((&b""[..], name), |at| (&name[..at], &name[at + 1..]));
    actual_local == local
        && scope.iter().any(|binding| {
            binding
                .strip_prefix('\0')
                .and_then(|binding| binding.split_once('\0'))
                .is_some_and(|(candidate, namespace)| {
                    candidate.as_bytes() == prefix && namespace == M_NS
                })
        })
}

fn parse_run_properties(raw: &[u8], inherited: &[(String, String)]) -> Result<MathRunProperties> {
    let mut parsed = parse_element(raw, inherited)?;
    let mut value = MathRunProperties::default();
    let mut modeled = 0usize;
    let mut seen = Vec::new();
    let mut last_rank = None;
    for child in parsed.children {
        let local = math_local_name(&child, &parsed.bindings)?;
        let Some(name) = local.as_deref() else {
            let key = property_raw_key("rPr", None, &mut last_rank, false);
            parsed.preservation.property_raw_children.push((key, child));
            continue;
        };
        let duplicate = seen.iter().any(|seen_name| seen_name == name);
        if !supported_properties("rPr").contains(&name)
            || duplicate
            || !property_leaf_is_valid("rPr", name, &child, &parsed.bindings)?
        {
            let key = property_raw_key("rPr", Some(name), &mut last_rank, duplicate);
            parsed.preservation.property_raw_children.push((key, child));
            continue;
        }
        property_raw_key("rPr", Some(name), &mut last_rank, false);
        let parsed_value = root_value(&child, &parsed.bindings)?
            .or_else(|| property_value_default("rPr", name).map(str::to_owned));
        let recognized = match name {
            "sty" => {
                value.style = parsed_value.as_deref().and_then(MathStyle::parse);
                true
            }
            "nor" => {
                value.normal = parse_present_on_off(parsed_value.as_deref());
                true
            }
            "lit" => {
                value.literal = parse_present_on_off(parsed_value.as_deref());
                true
            }
            "scr" => {
                value.script = parsed_value.as_deref().and_then(MathScriptStyle::parse);
                true
            }
            "brk" => {
                let alignment = root_attribute(&child, &parsed.bindings, b"alnAt")?;
                match alignment {
                    Some(alignment) => match alignment.parse::<u8>() {
                        Ok(0) | Err(_) => false,
                        Ok(alignment) => {
                            value.break_before = true;
                            value.break_alignment = Some(alignment);
                            true
                        }
                    },
                    None => {
                        value.break_before = true;
                        true
                    }
                }
            }
            _ => false,
        };
        if recognized {
            seen.push(name.to_owned());
            preserve_modeled_child(&mut parsed.preservation, name, &child, &parsed.bindings);
            modeled += 1;
        } else {
            parsed.preservation.raw_children.push((modeled, child));
        }
    }
    value.preservation = parsed.preservation;
    Ok(value)
}

fn write_run_properties<W: Write>(
    writer: &mut Writer<W>,
    value: &MathRunProperties,
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<()> {
    let mut properties = Vec::new();
    if let Some(literal) = value.literal {
        properties.push(Property::boolean("lit", literal));
    }
    if let Some(normal) = value.normal {
        properties.push(Property::boolean("nor", normal));
    }
    if let Some(script) = value.script {
        properties.push(Property::text("scr", script.as_str()));
    }
    if let Some(style) = value.style {
        properties.push(Property::text("sty", style.as_str()));
    }
    if let Some(alignment) = value.break_alignment {
        properties.push(Property::attribute("brk", "alnAt", alignment.into()));
    } else if value.break_before {
        properties.push(Property::marker("brk"));
    }
    write_property_container_with_preservation(
        writer,
        "rPr",
        &properties,
        &value.preservation,
        emission,
    )
}

enum PropertyValue<'a> {
    Text(&'a str),
    Unsigned(u64),
}
struct Property<'a> {
    name: &'a str,
    attribute: Option<(&'a str, PropertyValue<'a>)>,
}
impl<'a> Property<'a> {
    fn text(name: &'a str, value: &'a str) -> Self {
        Self {
            name,
            attribute: Some(("val", PropertyValue::Text(value))),
        }
    }
    fn boolean(name: &'a str, value: bool) -> Self {
        Self {
            name,
            attribute: Some(("val", PropertyValue::Text(if value { "1" } else { "0" }))),
        }
    }
    fn unsigned(name: &'a str, value: u64) -> Self {
        Self {
            name,
            attribute: Some(("val", PropertyValue::Unsigned(value))),
        }
    }
    fn attribute(name: &'a str, attribute: &'a str, value: u64) -> Self {
        Self {
            name,
            attribute: Some((attribute, PropertyValue::Unsigned(value))),
        }
    }
    fn marker(name: &'a str) -> Self {
        Self {
            name,
            attribute: None,
        }
    }
    fn attribute_value<'b>(&'b self, decimal: &'b mut itoa::Buffer) -> Option<(&'b str, &'b str)> {
        let (name, value) = self.attribute.as_ref()?;
        let value = match value {
            PropertyValue::Text(value) => *value,
            PropertyValue::Unsigned(value) => decimal.format(*value),
        };
        Some((name, value))
    }
}

fn write_property_container<W: Write>(
    writer: &mut Writer<W>,
    tag: &str,
    properties: &[Property<'_>],
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<()> {
    write_property_container_with_preservation(
        writer,
        tag,
        properties,
        &Preservation::default(),
        emission,
    )
}

fn write_math_text<W: Write>(
    writer: &mut Writer<W>,
    text_value: &str,
    parent: &Preservation,
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<()> {
    let mut preservation = if let Some(source) = parent
        .modeled_children
        .iter()
        .find(|child| child.name == "t")
    {
        parse_element(&source.raw, &source.bindings)?.preservation
    } else {
        Preservation::default()
    };
    preservation
        .attributes
        .retain(|(name, _)| name != "xml:space");
    let mut start = math_start("t", &preservation, false, emission)?;
    if text_value.starts_with(char::is_whitespace) || text_value.ends_with(char::is_whitespace) {
        start.push("xml:space", "preserve")?;
    }
    start.write(writer, false)?;
    if let Some(emission) = emission {
        emission.write_text(writer, text_value, None)?;
    } else {
        let text = BytesText::from_escaped(crate::text::escaped_source(text_value));
        writer.write_event(Event::Text(text))?;
    }
    writer.write_event(Event::End(BytesEnd::new("m:t")))?;
    Ok(())
}

fn preserve_modeled_child(
    preservation: &mut Preservation,
    name: &str,
    raw: &[u8],
    bindings: &[(String, String)],
) {
    preservation.modeled_children.push(PreservedChild {
        name: name.to_owned(),
        raw: raw.to_vec(),
        bindings: bindings.to_vec(),
    });
}

fn preservation_has_unsupported_content(preservation: &Preservation) -> bool {
    preservation
        .attributes
        .iter()
        .any(|(name, _)| name != "xmlns" && !name.starts_with("xmlns:"))
        || !preservation.raw_children.is_empty()
        || !preservation.property_raw_children.is_empty()
}

fn property_preservation_has_unsupported_content(
    preservation: &Preservation,
    container: &str,
) -> bool {
    preservation_has_unsupported_content(preservation)
        || preservation.modeled_children.iter().any(|child| {
            parse_element(&child.raw, &child.bindings).map_or(true, |parsed| {
                let modeled_attribute = property_modeled_attribute(container, &child.name);
                parsed.preservation.attributes.iter().any(|(name, _)| {
                    if name == "xmlns" || name.starts_with("xmlns:") {
                        return false;
                    }
                    expanded_attribute_name(name.as_bytes(), &parsed.bindings).is_none_or(
                        |(namespace, local)| {
                            namespace != M_NS || local.as_slice() != modeled_attribute
                        },
                    )
                }) || !parsed.children.is_empty()
            })
        })
}

fn property_container_has_unsupported_content(preservation: &Preservation, tag: &str) -> bool {
    let Some(source) = preservation
        .modeled_children
        .iter()
        .find(|child| child.name == tag)
    else {
        return false;
    };
    preserve_property_container(
        &source.raw,
        &source.bindings,
        tag,
        supported_properties(tag),
    )
    .map_or(true, |mut value| {
        // The control properties format the construct's own characters with
        // the run properties Word writes in every construct: understood.
        let bindings = element_bindings(&value);
        value.property_raw_children.retain(|(_, raw)| {
            !is_control_properties(raw, &bindings)
                && !(tag == "mPr" && matrix_column_properties(raw, &bindings).is_some())
        });
        property_preservation_has_unsupported_content(&value, tag)
    })
}

/// [`preservation_has_unsupported_content`] for a math run, whose `w:rPr`
/// is understood (see [`MathRun::word_properties`]).
fn run_preservation_has_unsupported_content(preservation: &Preservation) -> bool {
    let bindings = element_bindings(preservation);
    let mut word_properties = 0;
    let other_raw_child = preservation.raw_children.iter().any(|(_, raw)| {
        if is_untracked_word_run_properties(raw, &bindings) {
            word_properties += 1;
            false
        } else {
            true
        }
    });
    other_raw_child
        || word_properties > 1
        || !preservation.property_raw_children.is_empty()
        || preservation
            .attributes
            .iter()
            .any(|(name, _)| name != "xmlns" && !name.starts_with("xmlns:"))
}

/// The namespace bindings in scope inside an element: those it inherits and
/// those it declares.
fn element_bindings(preservation: &Preservation) -> Vec<(String, String)> {
    let mut bindings = preservation.inherited_bindings.clone();
    for (name, value) in &preservation.attributes {
        let prefix = if name == "xmlns" {
            ""
        } else if let Some(prefix) = name.strip_prefix("xmlns:") {
            prefix
        } else {
            continue;
        };
        bindings.retain(|(candidate, _)| candidate != prefix);
        bindings.push((prefix.to_owned(), value.clone()));
    }
    bindings
}

/// Parses `raw` when it is a WordprocessingML `w:rPr` with no attribute.
fn word_run_properties(raw: &[u8], inherited: &[(String, String)]) -> Option<CT_RPr> {
    let mut bindings = inherited.to_vec();
    let mut reader = Reader::from_reader(raw);
    let mut buffer = Vec::new();
    loop {
        let (element, empty) = match reader.read_event_into(&mut buffer).ok()? {
            Event::Start(element) => (element.into_owned(), false),
            Event::Empty(element) => (element.into_owned(), true),
            Event::Eof => return None,
            _ => {
                buffer.clear();
                continue;
            }
        };
        merge_bindings(&mut bindings, &element).ok()?;
        let (namespace, local) = expanded_name(element.name().as_ref(), &bindings)?;
        let attributed = extra_attributes(&element)
            .ok()?
            .iter()
            .any(|(name, _)| name != "xmlns" && !name.starts_with("xmlns:"));
        if namespace != W_NS || local != b"rPr" || attributed {
            return None;
        }
        if empty {
            return Some(CT_RPr::default());
        }
        let prefixes: Vec<String> = bindings
            .iter()
            .filter(|(_, namespace)| namespace == W_NS)
            .map(|(prefix, _)| prefix.clone())
            .collect();
        return CT_RPr::from_xml_with_prefixes_and_owner_bindings(
            &mut reader,
            &prefixes,
            &bindings,
        )
        .ok();
    }
}

/// Whether `raw` is a `w:rPr` the layout reads: run properties that track
/// no insertion, deletion or move.
fn is_untracked_word_run_properties(raw: &[u8], inherited: &[(String, String)]) -> bool {
    word_run_properties(raw, inherited).is_some()
        && parse_element(raw, inherited).is_ok_and(|parsed| {
            parsed.children.iter().all(|child| {
                root_name(child, &parsed.bindings).is_ok_and(|name| {
                    name.is_some_and(|(namespace, local)| {
                        namespace != W_NS
                            || !matches!(
                                local.as_slice(),
                                b"ins" | b"del" | b"moveFrom" | b"moveTo"
                            )
                    })
                })
            })
        })
}

/// The columns of a matrix's `m:mPr`, as (count, justification) per `m:mc`
/// of its `m:mcs`; `None` when it has no well-formed `m:mcs`.
fn matrix_columns(
    raw: &[u8],
    inherited: &[(String, String)],
) -> Option<Vec<(u32, MatrixColumnJustification)>> {
    let parsed = parse_element(raw, inherited).ok()?;
    parsed.children.iter().find_map(|child| {
        (math_local_name(child, &parsed.bindings).ok()?.as_deref() == Some("mcs"))
            .then(|| matrix_column_properties(child, &parsed.bindings))
            .flatten()
    })
}

/// Reads an `m:mcs`: every child an `m:mc` holding an `m:mcPr` of an
/// `m:count` (1 to 64, 1 by default) and an `m:mcJc` (`left`, `center` or
/// `right`), with nothing else.
fn matrix_column_properties(
    raw: &[u8],
    inherited: &[(String, String)],
) -> Option<Vec<(u32, MatrixColumnJustification)>> {
    fn only_namespaces(preservation: &Preservation) -> bool {
        preservation
            .attributes
            .iter()
            .all(|(name, _)| name == "xmlns" || name.starts_with("xmlns:"))
    }
    let named = |raw: &[u8], bindings: &[(String, String)], name: &str| {
        math_local_name(raw, bindings).ok().flatten().as_deref() == Some(name)
    };
    if !named(raw, inherited, "mcs") {
        return None;
    }
    let mcs = parse_element(raw, inherited).ok()?;
    if !only_namespaces(&mcs.preservation) {
        return None;
    }
    let mut columns = Vec::new();
    for mc in &mcs.children {
        let column = parse_element(mc, &mcs.bindings).ok()?;
        let [properties] = column.children.as_slice() else {
            return None;
        };
        if !named(mc, &mcs.bindings, "mc")
            || !only_namespaces(&column.preservation)
            || !named(properties, &column.bindings, "mcPr")
        {
            return None;
        }
        let leaves = parse_element(properties, &column.bindings).ok()?;
        if !only_namespaces(&leaves.preservation) {
            return None;
        }
        let (mut count, mut justification) = (1, MatrixColumnJustification::default());
        for leaf in &leaves.children {
            let parsed = parse_element(leaf, &leaves.bindings).ok()?;
            if !parsed.children.is_empty() {
                return None;
            }
            let value = root_value(leaf, &leaves.bindings).ok()??;
            match math_local_name(leaf, &leaves.bindings).ok()?.as_deref() {
                Some("count") => {
                    count = value
                        .parse()
                        .ok()
                        .filter(|count| (1..=64).contains(count))?
                }
                Some("mcJc") => {
                    justification = match value.as_str() {
                        "left" => MatrixColumnJustification::Left,
                        "center" => MatrixColumnJustification::Center,
                        "right" => MatrixColumnJustification::Right,
                        _ => return None,
                    }
                }
                _ => return None,
            }
        }
        columns.push((count, justification));
    }
    Some(columns)
}

/// Whether `raw` is an `m:ctrlPr` holding only untracked run properties.
fn is_control_properties(raw: &[u8], inherited: &[(String, String)]) -> bool {
    parse_element(raw, inherited).is_ok_and(|parsed| {
        math_local_name(raw, inherited).is_ok_and(|name| name.as_deref() == Some("ctrlPr"))
            && parsed
                .preservation
                .attributes
                .iter()
                .all(|(name, _)| name == "xmlns" || name.starts_with("xmlns:"))
            && parsed
                .children
                .iter()
                .all(|child| is_untracked_word_run_properties(child, &parsed.bindings))
    })
}

fn math_text_has_unsupported_content(preservation: &Preservation) -> bool {
    let Some(source) = preservation
        .modeled_children
        .iter()
        .find(|child| child.name == "t")
    else {
        return false;
    };
    parse_element(&source.raw, &source.bindings).map_or(true, |parsed| {
        parsed.preservation.attributes.iter().any(|(name, value)| {
            if name == "xmlns" || name.starts_with("xmlns:") {
                false
            } else if name == "xml:space" {
                !matches!(value.as_str(), "default" | "preserve")
            } else {
                true
            }
        })
    })
}

fn script_has_unsupported_content(value: &MathScript, property_tag: &str) -> bool {
    preservation_has_unsupported_content(&value.preservation)
        || property_container_has_unsupported_content(&value.preservation, property_tag)
        || value.base.has_unsupported_content()
        || value.script.has_unsupported_content()
}

fn three_argument_script_has_unsupported_content(
    preservation: &Preservation,
    property_tag: &str,
    base: &MathArgument,
    subscript: &MathArgument,
    superscript: &MathArgument,
) -> bool {
    preservation_has_unsupported_content(preservation)
        || property_container_has_unsupported_content(preservation, property_tag)
        || base.has_unsupported_content()
        || subscript.has_unsupported_content()
        || superscript.has_unsupported_content()
}

fn based_has_unsupported_content(
    preservation: &Preservation,
    property_tag: &str,
    base: &MathArgument,
) -> bool {
    preservation_has_unsupported_content(preservation)
        || property_container_has_unsupported_content(preservation, property_tag)
        || base.has_unsupported_content()
}

fn limit_has_unsupported_content(value: &MathLimit, property_tag: &str) -> bool {
    preservation_has_unsupported_content(&value.preservation)
        || property_container_has_unsupported_content(&value.preservation, property_tag)
        || value.base.has_unsupported_content()
        || value.limit.has_unsupported_content()
}

fn has_preserved_modeled_child(preservation: &Preservation, name: &str) -> bool {
    preservation
        .modeled_children
        .iter()
        .any(|child| child.name == name)
}

fn write_leading_property_container<W: Write>(
    writer: &mut Writer<W>,
    tag: &str,
    properties: &[Property<'_>],
    preservation: &Preservation,
    emit: bool,
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<usize> {
    let modeled = write_leading_property_container_before_raw(
        writer,
        tag,
        properties,
        preservation,
        emit,
        emission,
    )?;
    write_raw_slot(writer, preservation, modeled)?;
    Ok(modeled)
}

fn write_leading_property_container_before_raw<W: Write>(
    writer: &mut Writer<W>,
    tag: &str,
    properties: &[Property<'_>],
    preservation: &Preservation,
    emit: bool,
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<usize> {
    let existed_in_source = has_preserved_modeled_child(preservation, tag);
    if emit && existed_in_source {
        write_raw_slot(writer, preservation, 0)?;
        write_property_container_from_parent(writer, tag, properties, preservation, emission)?;
        Ok(1)
    } else {
        if emit {
            write_property_container_from_parent(writer, tag, properties, preservation, emission)?;
        }
        Ok(0)
    }
}

fn write_property_container_from_parent<W: Write>(
    writer: &mut Writer<W>,
    tag: &str,
    properties: &[Property<'_>],
    parent: &Preservation,
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<()> {
    let Some(source) = parent
        .modeled_children
        .iter()
        .find(|child| child.name == tag)
    else {
        return write_property_container(writer, tag, properties, emission);
    };
    let preservation = preserve_property_container(
        &source.raw,
        &source.bindings,
        tag,
        supported_properties(tag),
    )?;
    write_property_container_with_preservation(writer, tag, properties, &preservation, emission)
}

fn write_property_container_with_preservation<W: Write>(
    writer: &mut Writer<W>,
    tag: &str,
    properties: &[Property<'_>],
    preservation: &Preservation,
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<()> {
    let declare_math = tag == "mathPr";
    let start = math_start(tag, preservation, declare_math, emission)?;
    start.write(writer, false)?;
    let result = if preservation.property_raw_children.is_empty() {
        (|| {
            write_raw_slot(writer, preservation, 0)?;
            for (index, property) in properties.iter().enumerate() {
                write_property(writer, property, preservation, emission)?;
                if index + 1 < properties.len() {
                    write_raw_slot(writer, preservation, index + 1)?;
                }
            }
            write_raw_tail(writer, preservation, properties.len())
        })()
    } else {
        let full_order = full_property_order(tag);
        let mut emissions = preservation
            .property_raw_children
            .iter()
            .enumerate()
            .map(|(source_order, (key, raw))| (*key, source_order, None, Some(raw)))
            .collect::<Vec<_>>();
        emissions.extend(properties.iter().enumerate().map(|(index, property)| {
            let rank = full_order
                .iter()
                .position(|name| *name == property.name)
                .unwrap_or(full_order.len());
            (rank * 3 + 1, index, Some(property), None)
        }));
        emissions.sort_by_key(|(key, source_order, _, _)| (*key, *source_order));
        for (_, _, property, raw) in emissions {
            if let Some(source) = preservation
                .modeled_children
                .iter()
                .find(|child| property.is_some_and(|property| child.name == property.name))
            {
                write_property_leaf(
                    writer,
                    property.expect("source requires a property"),
                    &source.raw,
                    &source.bindings,
                    emission,
                )?;
            } else if let Some(property) = property {
                write_new_property(writer, property, emission)?;
            } else if let Some(raw) = raw {
                writer.get_mut().write_all(raw)?;
            }
        }
        Ok(())
    };
    result?;
    write_math_end(writer, tag, emission)?;
    Ok(())
}

fn write_property<W: Write>(
    writer: &mut Writer<W>,
    property: &Property<'_>,
    preservation: &Preservation,
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<()> {
    if let Some(source) = preservation
        .modeled_children
        .iter()
        .find(|child| child.name == property.name)
    {
        write_property_leaf(writer, property, &source.raw, &source.bindings, emission)
    } else {
        write_new_property(writer, property, emission)
    }
}

fn write_new_property<W: Write>(
    writer: &mut Writer<W>,
    property: &Property<'_>,
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<()> {
    let mut child = crate::text::FieldXmlStart::word("m", property.name, emission)?;
    let mut decimal = itoa::Buffer::new();
    if let Some((name, value)) = property.attribute_value(&mut decimal) {
        push_property_value(&mut child, name, value)?;
    }
    child.write(writer, true)?;
    Ok(())
}

/// Push the `m:` attribute of a property, a tab, line feed or carriage
/// return escaped so that attribute-value normalisation keeps it, as in a
/// math font name of the settings.
fn push_property_value(
    start: &mut crate::text::FieldXmlStart<'_, '_>,
    name: &str,
    value: &str,
) -> Result<()> {
    start.push_raw_word(
        "m",
        name,
        &oxoffice_core::xml::escape_attribute_value(value),
    )
}

fn write_property_leaf<W: Write>(
    writer: &mut Writer<W>,
    property: &Property<'_>,
    raw: &[u8],
    inherited: &[(String, String)],
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<()> {
    let parsed = parse_element(raw, inherited)?;
    let mut preservation = parsed.preservation;
    let modeled_attribute = property
        .attribute
        .as_ref()
        .map(|(name, _)| name.as_bytes())
        .unwrap_or(b"alnAt");
    preservation.attributes.retain(|(name, _)| {
        expanded_attribute_name(name.as_bytes(), &parsed.bindings)
            .is_none_or(|(namespace, local)| namespace != M_NS || local != modeled_attribute)
    });
    let mut start = math_start(property.name, &preservation, false, emission)?;
    let mut decimal = itoa::Buffer::new();
    if let Some((name, value)) = property.attribute_value(&mut decimal) {
        push_property_value(&mut start, name, value)?;
    }
    if parsed.children.is_empty() {
        start.write(writer, true)?;
    } else {
        start.write(writer, false)?;
        for child in parsed.children {
            writer.get_mut().write_all(&child)?;
        }
        write_math_end(writer, property.name, emission)?;
    }
    Ok(())
}

fn write_raw_tail<W: Write>(
    writer: &mut Writer<W>,
    preservation: &Preservation,
    from_slot: usize,
) -> Result<()> {
    for (_, raw) in preservation
        .raw_children
        .iter()
        .filter(|(position, _)| *position >= from_slot)
    {
        writer.get_mut().write_all(raw)?;
    }
    Ok(())
}

fn supported_properties(tag: &str) -> &'static [&'static str] {
    match tag {
        "rPr" => &["lit", "nor", "scr", "sty", "brk"],
        "fPr" => &["type"],
        "sSubPr" | "sSupPr" | "sSubSupPr" | "sPrePr" => &["alnScr"],
        "radPr" => &["degHide"],
        "mPr" => &[
            "baseJc", "plcHide", "rSpRule", "cGpRule", "rSp", "cSp", "cGp",
        ],
        "limLowPr" | "limUppPr" => &[],
        "naryPr" => &["chr", "limLoc", "grow", "subHide", "supHide"],
        "dPr" => &["begChr", "sepChr", "endChr", "grow"],
        "accPr" => &["chr"],
        "funcPr" => &[],
        "barPr" => &["pos"],
        "boxPr" => &["opEmu", "noBreak", "diff", "aln"],
        "borderBoxPr" => &BORDER_BOX_FLAGS,
        "groupChrPr" => &["chr", "pos", "vertJc"],
        "phantPr" => &PHANTOM_FLAGS,
        "eqArrPr" => &["baseJc", "maxDist", "objDist", "rSpRule", "rSp"],
        "oMathParaPr" => &["jc"],
        "mathPr" => &[
            "mathFont",
            "smallFrac",
            "dispDef",
            "lMargin",
            "rMargin",
            "defJc",
            "preSp",
            "postSp",
            "interSp",
            "intraSp",
            "wrapIndent",
            "intLim",
            "naryLim",
        ],
        _ => &[],
    }
}

fn property_value_default(container: &str, property: &str) -> Option<&'static str> {
    match (container, property) {
        ("rPr", "scr") => Some("roman"),
        ("rPr", "sty") => Some("i"),
        ("fPr", "type") => Some("bar"),
        ("mPr", "baseJc") => Some("center"),
        ("mPr", "rSp" | "cSp") => Some("0"),
        ("naryPr" | "accPr", "chr") => Some(""),
        ("naryPr", "limLoc") => Some("undOvr"),
        // Google Docs writes a bar's position with no value.
        ("barPr" | "groupChrPr", "pos") => Some("bot"),
        ("dPr", "begChr" | "sepChr" | "endChr") => Some(""),
        ("oMathParaPr", "jc") => Some("centerGroup"),
        ("mathPr", "defJc") => Some("centerGroup"),
        ("mathPr", "intLim") => Some("subSup"),
        ("mathPr", "naryLim") => Some("undOvr"),
        ("mathPr", "lMargin" | "rMargin" | "wrapIndent") => Some("1440"),
        _ => None,
    }
}

fn property_modeled_attribute(container: &str, property: &str) -> &'static [u8] {
    if container == "rPr" && property == "brk" {
        b"alnAt"
    } else {
        b"val"
    }
}

fn full_property_order(tag: &str) -> &'static [&'static str] {
    match tag {
        "rPr" => &["lit", "nor", "scr", "sty", "brk", "aln"],
        "fPr" => &["type", "ctrlPr"],
        "sSubPr" | "sSupPr" | "sSubSupPr" | "sPrePr" => &["alnScr", "ctrlPr"],
        "radPr" => &["degHide", "ctrlPr"],
        "mPr" => &[
            "baseJc", "plcHide", "rSpRule", "cGpRule", "rSp", "cSp", "cGp", "mcs", "ctrlPr",
        ],
        "limLowPr" | "limUppPr" => &["ctrlPr"],
        "naryPr" => &["chr", "limLoc", "grow", "subHide", "supHide", "ctrlPr"],
        "dPr" => &["begChr", "sepChr", "endChr", "grow", "shp", "ctrlPr"],
        "accPr" => &["chr", "ctrlPr"],
        "funcPr" => &["ctrlPr"],
        "barPr" => &["pos", "ctrlPr"],
        "boxPr" => &["opEmu", "noBreak", "diff", "brk", "aln", "ctrlPr"],
        "borderBoxPr" => &[
            "hideTop",
            "hideBot",
            "hideLeft",
            "hideRight",
            "strikeH",
            "strikeV",
            "strikeBLTR",
            "strikeTLBR",
            "ctrlPr",
        ],
        "groupChrPr" => &["chr", "pos", "vertJc", "ctrlPr"],
        "phantPr" => &["show", "zeroWid", "zeroAsc", "zeroDesc", "transp", "ctrlPr"],
        "eqArrPr" => &["baseJc", "maxDist", "objDist", "rSpRule", "rSp", "ctrlPr"],
        "oMathParaPr" => &["jc"],
        "mathPr" => &[
            "mathFont",
            "brkBin",
            "brkBinSub",
            "smallFrac",
            "dispDef",
            "lMargin",
            "rMargin",
            "defJc",
            "preSp",
            "postSp",
            "interSp",
            "intraSp",
            "wrapIndent",
            "wrapRight",
            "intLim",
            "naryLim",
        ],
        _ => &[],
    }
}

fn property_raw_key(
    tag: &str,
    local: Option<&str>,
    last_rank: &mut Option<usize>,
    duplicate: bool,
) -> usize {
    if let Some(rank) = local.and_then(|local| {
        full_property_order(tag)
            .iter()
            .position(|name| *name == local)
    }) {
        *last_rank = Some(rank);
        if duplicate {
            rank * 3 + 2
        } else {
            rank * 3 + 1
        }
    } else {
        last_rank.map_or(0, |rank| rank * 3 + 2)
    }
}

fn preserve_property_container(
    raw: &[u8],
    inherited: &[(String, String)],
    tag: &str,
    recognized: &[&str],
) -> Result<Preservation> {
    let mut parsed = parse_element(raw, inherited)?;
    let mut seen = Vec::new();
    let mut last_rank = None;
    for child in parsed.children {
        let local = math_local_name(&child, &parsed.bindings)?;
        if let Some(local) = local
            .as_deref()
            .filter(|local| recognized.contains(local) && !seen.iter().any(|name| name == local))
        {
            seen.push(local.to_owned());
            property_raw_key(tag, Some(local), &mut last_rank, false);
            preserve_modeled_child(&mut parsed.preservation, local, &child, &parsed.bindings);
        } else {
            let duplicate = local
                .as_deref()
                .is_some_and(|local| seen.iter().any(|name| name == local));
            let key = property_raw_key(tag, local.as_deref(), &mut last_rank, duplicate);
            parsed.preservation.property_raw_children.push((key, child));
        }
    }
    Ok(parsed.preservation)
}

fn write_container<W: Write>(
    writer: &mut Writer<W>,
    tag: &str,
    preservation: &Preservation,
    children: impl FnOnce(&mut Writer<W>) -> Result<()>,
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<()> {
    let start = math_start(tag, preservation, false, emission)?;
    start.write(writer, false)?;
    children(writer)?;
    write_math_end(writer, tag, emission)?;
    Ok(())
}

fn math_start<'e>(
    tag: &str,
    preservation: &Preservation,
    declare_math: bool,
    emission: Option<&'e crate::text::FieldXmlEmission>,
) -> Result<crate::text::FieldXmlStart<'e, 'static>> {
    let mut start = crate::text::FieldXmlStart::word("m", tag, emission)?;
    if declare_math {
        start.push("xmlns:m", M_NS)?;
        for (prefix, namespace) in &preservation.inherited_bindings {
            let retained = preservation.attributes.iter().any(|(candidate, _)| {
                if prefix.is_empty() {
                    candidate == "xmlns"
                } else {
                    candidate.strip_prefix("xmlns:") == Some(prefix.as_str())
                }
            });
            if prefix != "m" && prefix != "xml" && !retained {
                if prefix.is_empty() {
                    start.push("xmlns", namespace)?;
                } else {
                    start.push_word("xmlns", prefix, namespace)?;
                }
            }
        }
    }
    for (name, value) in &preservation.attributes {
        if name != "xmlns:m" {
            start.push(name, value)?;
        }
    }
    Ok(start)
}

fn write_math_end<W: Write>(
    writer: &mut Writer<W>,
    tag: &str,
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<()> {
    if let Some(emission) = emission {
        emission.write_word_end(writer, "m", tag)
    } else {
        writer.write_event(Event::End(BytesEnd::new(format!("m:{tag}"))))?;
        Ok(())
    }
}

fn write_raw_slot<W: Write>(
    writer: &mut Writer<W>,
    preservation: &Preservation,
    slot: usize,
) -> Result<()> {
    for (_, raw) in preservation
        .raw_children
        .iter()
        .filter(|(position, _)| *position == slot)
    {
        writer.get_mut().write_all(raw)?;
    }
    Ok(())
}

/// An equation root carries the bindings of its read scope so that it stays
/// well formed wherever it is written. Keep only those its subtree names: a
/// root written with every binding of its source scope would carry the
/// whole document root (or a reader's private wrapper prefix) into each
/// saved or compared copy.
fn keep_used_root_bindings(
    preservation: &mut Preservation,
    xml: &[u8],
    inherited: &[(String, String)],
) -> Result<()> {
    preservation.inherited_bindings = crate::text::external_bindings_used_by_raw(xml, inherited)?;
    Ok(())
}

/// See [`OfficeMath::canonical_xml`]. `raw` is one self-contained element.
fn canonical_element(raw: &[u8]) -> Result<Vec<u8>> {
    use quick_xml::name::{Namespace, QName, ResolveResult};
    let scan_error =
        |error: &dyn std::fmt::Display| OxmlError::InvalidValue(format!("equation scan: {error}"));
    let mut source = Vec::with_capacity(raw.len() + 32);
    source.extend_from_slice(b"<rdocxScope>");
    source.extend_from_slice(raw);
    source.extend_from_slice(b"</rdocxScope>");
    let mut reader = quick_xml::NsReader::from_reader(source.as_slice());
    reader.config_mut().trim_text(false);
    let mut output = Vec::with_capacity(raw.len());
    // The bindings the output holds at each open element, whether the last
    // start tag is still open (an end right after it makes it empty), and the
    // text read since the last tag.
    let mut declared = vec![std::collections::BTreeMap::<String, String>::new()];
    let mut open = false;
    let mut text = String::new();
    let flush = |output: &mut Vec<u8>, open: &mut bool, text: &mut String| {
        if std::mem::take(open) {
            output.push(b'>');
        }
        output.extend_from_slice(quick_xml::escape::escape(text.as_str()).as_bytes());
        text.clear();
    };
    let mut inside = false;
    let mut buffer = Vec::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| scan_error(&error))?;
        let (element, empty) = match event {
            Event::Start(_) if !inside => {
                inside = true;
                buffer.clear();
                continue;
            }
            Event::Start(element) => (element, false),
            Event::Empty(element) => (element, true),
            Event::End(element) => {
                if declared.len() == 1 {
                    break;
                }
                declared.pop();
                if open && text.is_empty() {
                    output.extend_from_slice(b"/>");
                    open = false;
                } else {
                    flush(&mut output, &mut open, &mut text);
                    output.extend_from_slice(b"</");
                    output.extend_from_slice(element.name().as_ref());
                    output.push(b'>');
                }
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
                text.push_str(&oxoffice_core::xml_text::normalize_line_ends(
                    &content.decode().map_err(|error| scan_error(&error))?,
                ));
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
            other => {
                flush(&mut output, &mut open, &mut text);
                Writer::new(&mut output).write_event(other)?;
                buffer.clear();
                continue;
            }
        };
        flush(&mut output, &mut open, &mut text);
        let resolver = reader.resolver();
        let mut scope = declared.last().cloned().unwrap_or_default();
        let mut declarations = std::collections::BTreeMap::new();
        // Declare `prefix` (empty: the default namespace) with what it
        // resolves to here, unless the output already holds that binding.
        let mut bind = |prefix: &str| -> Result<()> {
            let probe = if prefix.is_empty() {
                b"x".to_vec()
            } else {
                format!("{prefix}:x").into_bytes()
            };
            let uri = match resolver.resolve_element(QName(&probe)).0 {
                ResolveResult::Bound(Namespace(uri)) => std::str::from_utf8(uri)?.to_owned(),
                // An unprefixed name outside any default namespace, under an
                // element that declared one, undeclares it.
                ResolveResult::Unbound if prefix.is_empty() => String::new(),
                _ => return Ok(()),
            };
            if prefix != "xml" && scope.get(prefix).map(String::as_str).unwrap_or_default() != uri {
                scope.insert(prefix.to_owned(), uri.clone());
                declarations.insert(prefix.to_owned(), uri);
            }
            Ok(())
        };
        let name = element.name();
        let prefix_of = |name: QName<'_>| -> Result<String> {
            Ok(name
                .prefix()
                .map(|prefix| std::str::from_utf8(prefix.into_inner()))
                .transpose()?
                .unwrap_or_default()
                .to_owned())
        };
        bind(&prefix_of(name)?)?;
        let (element_namespace, element_local) = resolver.resolve_element(name);
        let choice = matches!(
            element_namespace,
            ResolveResult::Bound(Namespace(uri)) if uri == crate::namespace::MC_NS.as_bytes()
        ) && element_local.as_ref() == b"Choice";
        let mut attributes = Vec::new();
        for attribute in element.attributes() {
            let attribute = attribute.map_err(|error| scan_error(&error))?;
            let key = attribute.key.as_ref();
            if key == b"xmlns" || key.starts_with(b"xmlns:") {
                continue;
            }
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .map_err(|error| scan_error(&error))?
                .into_owned();
            if attribute.key.prefix().is_some() {
                bind(&prefix_of(attribute.key)?)?;
                let (namespace, local) = resolver.resolve_attribute(attribute.key);
                let namespace = match namespace {
                    ResolveResult::Bound(Namespace(uri)) => Some(std::str::from_utf8(uri)?),
                    _ => None,
                };
                for named in
                    crate::text::root_attribute_value_prefixes(namespace, local.as_ref(), &value)
                {
                    bind(named)?;
                }
            } else if choice && key == b"Requires" {
                for named in value.split_ascii_whitespace() {
                    bind(named)?;
                }
            }
            attributes.push((key.to_vec(), value));
        }
        attributes.sort();
        output.push(b'<');
        output.extend_from_slice(name.as_ref());
        let declarations = declarations.into_iter().map(|(prefix, uri)| {
            let name = if prefix.is_empty() {
                b"xmlns".to_vec()
            } else {
                format!("xmlns:{prefix}").into_bytes()
            };
            (name, uri)
        });
        for (key, value) in declarations.chain(attributes) {
            output.push(b' ');
            output.extend_from_slice(&key);
            output.extend_from_slice(b"=\"");
            output.extend_from_slice(quick_xml::escape::escape(value.as_str()).as_bytes());
            output.push(b'"');
        }
        if empty {
            output.extend_from_slice(b"/>");
        } else {
            open = true;
            declared.push(scope);
        }
        buffer.clear();
    }
    flush(&mut output, &mut open, &mut text);
    Ok(output)
}

fn parse_element(raw: &[u8], inherited: &[(String, String)]) -> Result<ParsedElement> {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut bindings = inherited.to_vec();
    let mut preservation = Preservation {
        inherited_bindings: inherited.to_vec(),
        ..Preservation::default()
    };
    let mut children = Vec::new();
    let mut inside = false;
    let mut saw_root = false;
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) if !inside => {
                saw_root = true;
                merge_bindings(&mut bindings, &element)?;
                preservation.attributes = extra_attributes(&element)?;
                inside = true;
            }
            Event::Empty(element) if !inside => {
                saw_root = true;
                merge_bindings(&mut bindings, &element)?;
                preservation.attributes = extra_attributes(&element)?;
                break;
            }
            Event::Start(element) if inside => {
                children.push(capture_element(&mut reader, &element)?)
            }
            Event::Empty(element) if inside => children.push(capture_empty(&element)?),
            Event::Text(text) if inside && !text.as_ref().iter().all(u8::is_ascii_whitespace) => {
                children.push(capture_event(Event::Text(text.into_owned()))?)
            }
            Event::GeneralRef(entity) if inside => {
                children.push(capture_event(Event::GeneralRef(entity.into_owned()))?)
            }
            Event::Comment(comment) if inside => {
                children.push(capture_event(Event::Comment(comment.into_owned()))?)
            }
            Event::PI(pi) if inside => children.push(capture_event(Event::PI(pi.into_owned()))?),
            Event::End(_) if inside => break,
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !saw_root {
        return Err(OxmlError::MissingElement("OfficeMath element".to_owned()));
    }
    Ok(ParsedElement {
        preservation,
        bindings,
        children,
    })
}

const MAX_OFFICEMATH_XML_DEPTH: usize = 128;

fn validate_xml_depth(raw: &[u8]) -> Result<()> {
    let mut reader = Reader::from_reader(raw);
    let mut depth = 0usize;
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(_) => {
                depth += 1;
                if depth > MAX_OFFICEMATH_XML_DEPTH {
                    return Err(OxmlError::InvalidValue(format!(
                        "OfficeMath XML nesting exceeds {MAX_OFFICEMATH_XML_DEPTH} elements"
                    )));
                }
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof if depth == 0 => return Ok(()),
            Event::Eof => {
                return Err(OxmlError::MissingElement(
                    "OfficeMath closing element".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn valid_expression_shape(raw: &[u8], inherited: &[(String, String)]) -> Result<bool> {
    let Some(root) = math_local_name(raw, inherited)? else {
        return Ok(true);
    };
    let parsed = parse_element(raw, inherited)?;
    let property_tag = match root.as_str() {
        "r" => Some("rPr"),
        "f" => Some("fPr"),
        "sSub" => Some("sSubPr"),
        "sSup" => Some("sSupPr"),
        "sSubSup" => Some("sSubSupPr"),
        "sPre" => Some("sPrePr"),
        "rad" => Some("radPr"),
        "m" => Some("mPr"),
        "limLow" => Some("limLowPr"),
        "limUpp" => Some("limUppPr"),
        "nary" => Some("naryPr"),
        "d" => Some("dPr"),
        "acc" => Some("accPr"),
        "func" => Some("funcPr"),
        "bar" => Some("barPr"),
        "box" => Some("boxPr"),
        "borderBox" => Some("borderBoxPr"),
        "groupChr" => Some("groupChrPr"),
        "phant" => Some("phantPr"),
        "eqArr" => Some("eqArrPr"),
        _ => None,
    };
    if let Some(property_tag) = property_tag {
        for child in &parsed.children {
            if math_local_name(child, &parsed.bindings)?.as_deref() == Some(property_tag)
                && !property_container_is_valid(child, &parsed.bindings, property_tag)?
            {
                return Ok(false);
            }
        }
    }
    if root == "r" {
        for child in &parsed.children {
            if math_local_name(child, &parsed.bindings)?.as_deref() == Some("t")
                && math_text_has_non_text_nodes(child)?
            {
                return Ok(false);
            }
        }
    }
    let allowed: &[&str] = match root.as_str() {
        "r" => &["rPr", "t"],
        "f" => &["fPr", "num", "den"],
        "sSub" => &["sSubPr", "e", "sub"],
        "sSup" => &["sSupPr", "e", "sup"],
        "sSubSup" => &["sSubSupPr", "e", "sub", "sup"],
        "sPre" => &["sPrePr", "sub", "sup", "e"],
        "rad" => &["radPr", "deg", "e"],
        "m" => &["mPr", "mr"],
        "limLow" => &["limLowPr", "e", "lim"],
        "limUpp" => &["limUppPr", "e", "lim"],
        "nary" => &["naryPr", "sub", "sup", "e"],
        "d" => &["dPr", "e"],
        "acc" => &["accPr", "e"],
        "func" => &["funcPr", "fName", "e"],
        "bar" => &["barPr", "e"],
        "box" => &["boxPr", "e"],
        "borderBox" => &["borderBoxPr", "e"],
        "groupChr" => &["groupChrPr", "e"],
        "phant" => &["phantPr", "e"],
        "eqArr" => &["eqArrPr", "e"],
        _ => return Ok(true),
    };
    let sequence = parsed
        .children
        .iter()
        .filter_map(|child| math_local_name(child, &parsed.bindings).transpose())
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|local| allowed.contains(&local.as_str()))
        .collect::<Vec<_>>();
    let valid = match root.as_str() {
        "r" => {
            let text_start = usize::from(sequence.first().is_some_and(|value| value == "rPr"));
            sequence.len() == text_start + 1 && sequence[text_start] == "t"
        }
        "f" => sequence_matches(&sequence, "fPr", &["num", "den"]),
        "sSub" => sequence_matches(&sequence, "sSubPr", &["e", "sub"]),
        "sSup" => sequence_matches(&sequence, "sSupPr", &["e", "sup"]),
        "sSubSup" => sequence_matches(&sequence, "sSubSupPr", &["e", "sub", "sup"]),
        "sPre" => sequence_matches(&sequence, "sPrePr", &["sub", "sup", "e"]),
        "rad" => sequence_matches_with_optional(&sequence, "radPr", &["deg"], &["e"]),
        "limLow" => sequence_matches(&sequence, "limLowPr", &["e", "lim"]),
        "limUpp" => sequence_matches(&sequence, "limUppPr", &["e", "lim"]),
        // Google Docs writes an n-ary without its m:e, the summand after it.
        "nary" => {
            sequence_matches_with_optional(&sequence, "naryPr", &["sub", "sup"], &["e"])
                || sequence_matches_with_optional(&sequence, "naryPr", &["sub", "sup"], &[])
        }
        "acc" => sequence_matches(&sequence, "accPr", &["e"]),
        "func" => sequence_matches(&sequence, "funcPr", &["fName", "e"]),
        "bar" => sequence_matches(&sequence, "barPr", &["e"]),
        "box" => sequence_matches(&sequence, "boxPr", &["e"]),
        "borderBox" => sequence_matches(&sequence, "borderBoxPr", &["e"]),
        "groupChr" => sequence_matches(&sequence, "groupChrPr", &["e"]),
        "phant" => sequence_matches(&sequence, "phantPr", &["e"]),
        "d" | "eqArr" => {
            let property = if root == "d" { "dPr" } else { "eqArrPr" };
            let argument_start =
                usize::from(sequence.first().is_some_and(|value| value == property));
            sequence.len() > argument_start
                && sequence[argument_start..].iter().all(|value| value == "e")
        }
        "m" => {
            let row_start = usize::from(sequence.first().is_some_and(|value| value == "mPr"));
            let mut rows_are_valid = true;
            for child in &parsed.children {
                if math_local_name(child, &parsed.bindings)?.as_deref() == Some("mr") {
                    rows_are_valid &= valid_matrix_row(child, &parsed.bindings)?;
                }
            }
            sequence.len() > row_start
                && sequence[row_start..].iter().all(|value| value == "mr")
                && rows_are_valid
        }
        _ => true,
    };
    Ok(valid)
}

fn valid_officemath_paragraph_shape(raw: &[u8], inherited: &[(String, String)]) -> Result<bool> {
    let parsed = parse_element(raw, inherited)?;
    let sequence = parsed
        .children
        .iter()
        .filter_map(|child| math_local_name(child, &parsed.bindings).transpose())
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|local| local == "oMathParaPr" || local == "oMath")
        .collect::<Vec<_>>();
    let equation_start = usize::from(sequence.first().is_some_and(|value| value == "oMathParaPr"));
    if sequence.len() <= equation_start
        || !sequence[equation_start..]
            .iter()
            .all(|value| value == "oMath")
    {
        return Ok(false);
    }
    for child in &parsed.children {
        if math_local_name(child, &parsed.bindings)?.as_deref() == Some("oMathParaPr")
            && !property_container_is_valid(child, &parsed.bindings, "oMathParaPr")?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn property_container_is_valid(
    raw: &[u8],
    inherited: &[(String, String)],
    tag: &str,
) -> Result<bool> {
    let parsed = parse_element(raw, inherited)?;
    let supported = supported_properties(tag);
    let mut last_index = None;
    for child in &parsed.children {
        let Some(local) = math_local_name(child, &parsed.bindings)? else {
            continue;
        };
        let Some(index) = supported.iter().position(|name| *name == local) else {
            continue;
        };
        if last_index.is_some_and(|previous| index <= previous)
            || !property_leaf_is_valid(tag, &local, child, &parsed.bindings)?
        {
            return Ok(false);
        }
        last_index = Some(index);
    }
    Ok(true)
}

fn property_leaf_is_valid(
    container: &str,
    property: &str,
    raw: &[u8],
    inherited: &[(String, String)],
) -> Result<bool> {
    let modeled_attribute = property_modeled_attribute(container, property);
    if root_has_ambiguous_modeled_attribute(raw, inherited, modeled_attribute)? {
        return Ok(false);
    }
    let value = root_value(raw, inherited)?
        .or_else(|| property_value_default(container, property).map(str::to_owned));
    Ok(match (container, property) {
        ("rPr", "lit" | "nor")
        | ("sSubPr" | "sSupPr" | "sSubSupPr" | "sPrePr", "alnScr")
        | ("radPr", "degHide")
        | ("naryPr", "grow" | "subHide" | "supHide")
        | ("dPr", "grow")
        | ("boxPr", "opEmu" | "noBreak" | "diff" | "aln")
        | ("borderBoxPr", _)
        | ("phantPr", _)
        | ("eqArrPr", "maxDist" | "objDist")
        | ("mPr", "plcHide") => value
            .as_deref()
            .is_none_or(|value| crate::shared::ST_OnOff::parse(value).is_some()),
        ("rPr", "scr") => value
            .as_deref()
            .is_some_and(|value| MathScriptStyle::parse(value).is_some()),
        ("rPr", "sty") => value
            .as_deref()
            .is_some_and(|value| MathStyle::parse(value).is_some()),
        ("rPr", "brk") => root_attribute(raw, inherited, b"alnAt")?
            .as_deref()
            .is_none_or(|value| value.parse::<u8>().is_ok_and(|value| value > 0)),
        ("fPr", "type") => value
            .as_deref()
            .is_some_and(|value| FractionType::parse(value).is_some()),
        ("mPr", "baseJc") => value
            .as_deref()
            .is_some_and(|value| MatrixBaseJustification::parse(value).is_some()),
        ("mPr", "rSp") => value
            .as_deref()
            .is_some_and(|value| value.parse::<u16>().is_ok()),
        ("mPr", "cSp") => value
            .as_deref()
            .is_some_and(|value| value.parse::<u32>().is_ok()),
        ("barPr" | "groupChrPr", "pos") | ("groupChrPr", "vertJc") => value
            .as_deref()
            .is_some_and(|value| MathVerticalPosition::parse(value).is_some()),
        ("eqArrPr", "baseJc") => value
            .as_deref()
            .is_some_and(|value| MatrixBaseJustification::parse(value).is_some()),
        ("eqArrPr" | "mPr", "rSpRule") | ("mPr", "cGpRule") => value
            .as_deref()
            .is_some_and(|value| value.parse::<u8>().is_ok_and(|value| value <= 4)),
        ("eqArrPr", "rSp") => value
            .as_deref()
            .is_some_and(|value| value.parse::<u16>().is_ok()),
        ("mPr", "cGp") => value
            .as_deref()
            .is_some_and(|value| value.parse::<u32>().is_ok()),
        ("naryPr", "limLoc") => value
            .as_deref()
            .is_some_and(|value| LimitLocation::parse(value).is_some()),
        ("oMathParaPr", "jc") => value
            .as_deref()
            .is_some_and(|value| MathJustification::parse(value).is_some()),
        ("naryPr" | "accPr" | "groupChrPr", "chr") | ("dPr", "begChr" | "sepChr" | "endChr") => {
            value
                .as_deref()
                .is_none_or(|value| value.chars().count() <= 1)
        }
        ("mathPr", "mathFont") => value.as_deref().is_some_and(|value| !value.is_empty()),
        ("mathPr", "smallFrac" | "dispDef") => value
            .as_deref()
            .is_none_or(|value| crate::shared::ST_OnOff::parse(value).is_some()),
        (
            "mathPr",
            "lMargin" | "rMargin" | "preSp" | "postSp" | "interSp" | "intraSp" | "wrapIndent",
        ) => value
            .as_deref()
            .is_some_and(|value| value.parse::<u32>().is_ok()),
        ("mathPr", "defJc") => value
            .as_deref()
            .is_some_and(|value| MathJustification::parse(value).is_some()),
        ("mathPr", "intLim" | "naryLim") => value
            .as_deref()
            .is_some_and(|value| LimitLocation::parse(value).is_some()),
        _ => true,
    })
}

fn sequence_matches(sequence: &[String], optional: &str, required: &[&str]) -> bool {
    let start = usize::from(sequence.first().is_some_and(|value| value == optional));
    sequence.len() == start + required.len()
        && sequence[start..]
            .iter()
            .zip(required)
            .all(|(actual, expected)| actual == expected)
}

fn sequence_matches_with_optional(
    sequence: &[String],
    property: &str,
    optional: &[&str],
    required: &[&str],
) -> bool {
    let mut index = usize::from(sequence.first().is_some_and(|value| value == property));
    for child in optional {
        if sequence.get(index).is_some_and(|value| value == child) {
            index += 1;
        }
    }
    sequence.len() == index + required.len()
        && sequence[index..]
            .iter()
            .zip(required)
            .all(|(actual, expected)| actual == expected)
}

fn valid_matrix_row(raw: &[u8], inherited: &[(String, String)]) -> Result<bool> {
    let parsed = parse_element(raw, inherited)?;
    let cells = parsed
        .children
        .iter()
        .filter_map(|child| math_local_name(child, &parsed.bindings).transpose())
        .collect::<Result<Vec<_>>>()?;
    Ok(!cells.is_empty() && cells.iter().all(|local| local == "e"))
}

fn element_text(raw: &[u8]) -> Result<String> {
    let mut reader = Reader::from_reader(raw);
    let mut buffer = Vec::new();
    let mut inside = false;
    let mut text = String::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(_) if !inside => inside = true,
            Event::Empty(_) if !inside => return Ok(text),
            Event::Start(_) if inside => {
                return Err(OxmlError::UnexpectedElement(
                    "nested element in OfficeMath text".to_owned(),
                ));
            }
            Event::Text(value) if inside => text.push_str(&crate::xml_text::normalize_line_ends(
                std::str::from_utf8(value.as_ref())?,
            )),
            Event::CData(value) if inside => text.push_str(&crate::xml_text::normalize_line_ends(
                std::str::from_utf8(value.as_ref())?,
            )),
            Event::GeneralRef(value) if inside => {
                let name = std::str::from_utf8(value.as_ref())?;
                let entity = format!("&{name};");
                let decoded = quick_xml::escape::unescape(&entity).map_err(|error| {
                    OxmlError::InvalidValue(format!("invalid OfficeMath text entity: {error}"))
                })?;
                text.push_str(&decoded);
            }
            Event::End(_) if inside => return Ok(text),
            Event::Empty(_) if inside => {
                return Err(OxmlError::UnexpectedElement(
                    "nested empty element in OfficeMath text".to_owned(),
                ));
            }
            Event::Eof => {
                return Err(OxmlError::MissingElement("OfficeMath text end".to_owned()));
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn math_text_has_non_text_nodes(raw: &[u8]) -> Result<bool> {
    let mut reader = Reader::from_reader(raw);
    let mut inside = false;
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) if !inside => {
                if math_text_has_invalid_space(&element)? {
                    return Ok(true);
                }
                inside = true;
            }
            Event::Empty(element) if !inside => {
                return math_text_has_invalid_space(&element);
            }
            Event::Start(_) | Event::Empty(_) if inside => return Ok(true),
            Event::Comment(_) | Event::PI(_) if inside => return Ok(true),
            Event::End(_) if inside => return Ok(false),
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

fn math_text_has_invalid_space(element: &BytesStart<'_>) -> Result<bool> {
    for attribute in element.attributes() {
        let attribute = attribute?;
        if attribute.key.as_ref() == b"xml:space" {
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?;
            if !matches!(value.as_ref(), "default" | "preserve") {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn capture_empty(element: &BytesStart<'_>) -> Result<Vec<u8>> {
    capture_event(Event::Empty(element.to_owned().into_owned()))
}
fn capture_event(event: Event<'static>) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    Writer::new(&mut output).write_event(event)?;
    Ok(output)
}

fn merge_bindings(bindings: &mut Vec<(String, String)>, element: &BytesStart<'_>) -> Result<()> {
    for attribute in element.attributes() {
        let attribute = attribute?;
        let name = attribute.key.as_ref();
        let prefix = if name == b"xmlns" {
            Some("")
        } else {
            name.strip_prefix(b"xmlns:")
                .and_then(|value| std::str::from_utf8(value).ok())
        };
        if let Some(prefix) = prefix {
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?
                .into_owned();
            bindings.retain(|(candidate, _)| candidate != prefix);
            bindings.push((prefix.to_owned(), value));
        }
    }
    Ok(())
}

pub(crate) fn fixed_math_prefix_is_safe(
    raw: &[u8],
    inherited: &[(String, String)],
) -> Result<bool> {
    let mut reader = Reader::from_reader(raw);
    let mut root_m_binding = inherited
        .iter()
        .rev()
        .find(|(prefix, _)| prefix == "m")
        .map(|(_, namespace)| namespace.clone());
    let mut saw_root = false;
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) | Event::Empty(element) => {
                let mut local_m_binding = None;
                for attribute in element.attributes() {
                    let attribute = attribute?;
                    if attribute.key.as_ref() == b"xmlns:m" {
                        local_m_binding = Some(
                            attribute
                                .decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    element.decoder(),
                                )?
                                .into_owned(),
                        );
                    }
                }
                if !saw_root {
                    if local_m_binding.is_some() {
                        root_m_binding = local_m_binding;
                    }
                    if root_m_binding.as_deref().is_some_and(|value| value != M_NS) {
                        return Ok(false);
                    }
                    saw_root = true;
                } else if local_m_binding
                    .as_deref()
                    .is_some_and(|value| value != M_NS)
                {
                    return Ok(false);
                }
            }
            Event::Eof => return Ok(true),
            _ => {}
        }
        buffer.clear();
    }
}

fn require_safe_fixed_math_prefix(raw: &[u8], inherited: &[(String, String)]) -> Result<()> {
    if fixed_math_prefix_is_safe(raw, inherited)? {
        Ok(())
    } else {
        Err(OxmlError::InvalidValue(
            "OfficeMath source conflicts with the canonical m prefix".to_owned(),
        ))
    }
}

fn extra_attributes(element: &BytesStart<'_>) -> Result<Vec<(String, String)>> {
    let mut attributes = Vec::new();
    for attribute in element.attributes() {
        let attribute = attribute?;
        let name = std::str::from_utf8(attribute.key.as_ref())?.to_owned();
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?
            .into_owned();
        attributes.push((name, value));
    }
    Ok(attributes)
}

fn expanded_name(name: &[u8], bindings: &[(String, String)]) -> Option<(String, Vec<u8>)> {
    let (prefix, local) = name
        .iter()
        .position(|byte| *byte == b':')
        .map_or((&b""[..], name), |at| (&name[..at], &name[at + 1..]));
    let prefix = std::str::from_utf8(prefix).ok()?;
    let namespace = bindings
        .iter()
        .rev()
        .find(|(candidate, _)| candidate == prefix)?
        .1
        .clone();
    Some((namespace, local.to_vec()))
}

fn expanded_attribute_name(
    name: &[u8],
    bindings: &[(String, String)],
) -> Option<(String, Vec<u8>)> {
    if !name.contains(&b':') {
        return None;
    }
    expanded_name(name, bindings)
}

fn root_name(raw: &[u8], inherited: &[(String, String)]) -> Result<Option<(String, Vec<u8>)>> {
    let mut reader = Reader::from_reader(raw);
    let mut bindings = inherited.to_vec();
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) | Event::Empty(element) => {
                merge_bindings(&mut bindings, &element)?;
                return Ok(expanded_name(element.name().as_ref(), &bindings));
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
        buffer.clear();
    }
}

fn math_local_name(raw: &[u8], inherited: &[(String, String)]) -> Result<Option<String>> {
    Ok(root_name(raw, inherited)?.and_then(|(namespace, local)| {
        (namespace == M_NS).then(|| String::from_utf8_lossy(&local).into_owned())
    }))
}

fn require_root(raw: &[u8], inherited: &[(String, String)], expected: &[u8]) -> Result<()> {
    if root_name(raw, inherited)?
        .is_some_and(|(namespace, local)| namespace == M_NS && local == expected)
    {
        Ok(())
    } else {
        Err(OxmlError::MissingElement(format!(
            "OfficeMath {} root",
            String::from_utf8_lossy(expected)
        )))
    }
}

fn root_value(raw: &[u8], inherited: &[(String, String)]) -> Result<Option<String>> {
    root_attribute(raw, inherited, b"val")
}

fn root_has_ambiguous_modeled_attribute(
    raw: &[u8],
    inherited: &[(String, String)],
    expected: &[u8],
) -> Result<bool> {
    let parsed = parse_element(raw, inherited)?;
    let mut modeled_count = 0;
    for (name, _) in &parsed.preservation.attributes {
        if name == "xmlns" || name.starts_with("xmlns:") {
            continue;
        }
        let local = name
            .as_bytes()
            .rsplit(|byte| *byte == b':')
            .next()
            .unwrap_or_default();
        if local != expected {
            continue;
        }
        if expanded_attribute_name(name.as_bytes(), &parsed.bindings)
            .is_some_and(|(namespace, local)| namespace == M_NS && local == expected)
        {
            modeled_count += 1;
        } else {
            return Ok(true);
        }
    }
    Ok(modeled_count > 1)
}

fn root_attribute(
    raw: &[u8],
    inherited: &[(String, String)],
    expected: &[u8],
) -> Result<Option<String>> {
    let mut reader = Reader::from_reader(raw);
    let mut bindings = inherited.to_vec();
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) | Event::Empty(element) => {
                merge_bindings(&mut bindings, &element)?;
                for attribute in element.attributes() {
                    let attribute = attribute?;
                    if expanded_attribute_name(attribute.key.as_ref(), &bindings)
                        .is_some_and(|(namespace, local)| namespace == M_NS && local == expected)
                    {
                        return Ok(Some(
                            attribute
                                .decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    element.decoder(),
                                )?
                                .into_owned(),
                        ));
                    }
                }
                return Ok(None);
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
        buffer.clear();
    }
}

fn child_property_value(
    raw: &[u8],
    inherited: &[(String, String)],
    container: &str,
    property: &str,
) -> Result<Option<String>> {
    let parsed = parse_element(raw, inherited)?;
    for child in parsed.children {
        if math_local_name(&child, &parsed.bindings)?.as_deref() == Some(property) {
            return Ok(root_value(&child, &parsed.bindings)?
                .or_else(|| property_value_default(container, property).map(str::to_owned)));
        }
    }
    Ok(None)
}

fn child_on_off_value(
    raw: &[u8],
    inherited: &[(String, String)],
    property: &str,
) -> Result<Option<bool>> {
    let parsed = parse_element(raw, inherited)?;
    for child in parsed.children {
        if math_local_name(&child, &parsed.bindings)?.as_deref() == Some(property) {
            return Ok(parse_present_on_off(
                root_value(&child, &parsed.bindings)?.as_deref(),
            ));
        }
    }
    Ok(None)
}

fn preserve_unrecognized_property_children(
    raw: &[u8],
    inherited: &[(String, String)],
    recognized: &[&str],
) -> Result<Preservation> {
    let mut parsed = parse_element(raw, inherited)?;
    let mut seen = Vec::new();
    let mut last_rank = None;
    for child in parsed.children {
        let local = math_local_name(&child, &parsed.bindings)?;
        if let Some(local) = local
            .as_deref()
            .filter(|local| recognized.contains(local) && !seen.iter().any(|name| name == local))
        {
            seen.push(local.to_owned());
            property_raw_key("oMathParaPr", Some(local), &mut last_rank, false);
            preserve_modeled_child(&mut parsed.preservation, local, &child, &parsed.bindings);
        } else {
            let duplicate = local
                .as_deref()
                .is_some_and(|local| seen.iter().any(|name| name == local));
            let key = property_raw_key("oMathParaPr", local.as_deref(), &mut last_rank, duplicate);
            parsed.preservation.property_raw_children.push((key, child));
        }
    }
    Ok(parsed.preservation)
}

fn parse_present_on_off(value: Option<&str>) -> Option<bool> {
    match value {
        Some(value) => crate::shared::ST_OnOff::parse(value),
        None => Some(true),
    }
}
fn parse_u32(value: Option<String>) -> Option<u32> {
    value.and_then(|value| value.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::{
        CT_OMath, MathExpression, MathVerticalPosition, MatrixBaseJustification,
        MatrixColumnJustification, canonical_element,
    };

    const M: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";
    const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

    fn canonical(xml: &str) -> Vec<u8> {
        canonical_element(xml.as_bytes()).unwrap()
    }

    /// An equation of `body`, read, checked to hold nothing unsupported and
    /// to write itself back as it was read.
    fn round_trip(body: &str) -> MathExpression {
        let xml = format!(r#"<m:oMath xmlns:m="{M}" xmlns:w="{W}">{body}</m:oMath>"#);
        let equation = CT_OMath::from_xml(xml.as_bytes()).unwrap();
        assert!(!equation.has_unsupported_content(), "{body}");
        assert_eq!(
            String::from_utf8(canonical(
                &String::from_utf8(equation.to_xml().unwrap()).unwrap()
            ))
            .unwrap(),
            String::from_utf8(canonical(&xml)).unwrap(),
            "{body}"
        );
        let [expression] = equation.expressions.as_slice() else {
            panic!("one expression: {body}");
        };
        expression.clone()
    }

    #[test]
    fn every_officemath_object_is_typed_and_written_back_as_read() {
        let control = r#"<m:ctrlPr><w:rPr><w:rFonts w:ascii="Cambria Math"/></w:rPr></m:ctrlPr>"#;
        let x = r#"<m:e><m:r><m:t>x</m:t></m:r></m:e>"#;
        let MathExpression::Function(function) = round_trip(&format!(
            r#"<m:func><m:funcPr>{control}</m:funcPr><m:fName><m:r><m:rPr><m:sty m:val="p"/></m:rPr><m:t>log</m:t></m:r></m:fName>{x}</m:func>"#
        )) else {
            panic!("a function");
        };
        assert_eq!(function.name.expressions.len(), 1);
        let MathExpression::Bar(bar) = round_trip(&format!(
            r#"<m:bar><m:barPr><m:pos m:val="top"/>{control}</m:barPr>{x}</m:bar>"#
        )) else {
            panic!("a bar");
        };
        assert_eq!(bar.position, Some(MathVerticalPosition::Top));
        let MathExpression::Box(boxed) = round_trip(&format!(
            r#"<m:box><m:boxPr><m:opEmu m:val="1"/></m:boxPr>{x}</m:box>"#
        )) else {
            panic!("a box");
        };
        assert_eq!(boxed.operator_emulator, Some(true));
        let MathExpression::BorderBox(border) = round_trip(&format!(
            r#"<m:borderBox><m:borderBoxPr><m:hideTop m:val="1"/><m:strikeBLTR m:val="1"/></m:borderBoxPr>{x}</m:borderBox>"#
        )) else {
            panic!("a border box");
        };
        assert_eq!(
            (
                border.hide_top,
                border.hide_left,
                border.strike_bottom_left_to_top_right
            ),
            (Some(true), None, Some(true))
        );
        let MathExpression::GroupCharacter(group) = round_trip(&format!(
            r#"<m:groupChr><m:groupChrPr><m:chr m:val="⏞"/><m:pos m:val="top"/><m:vertJc m:val="bot"/></m:groupChrPr>{x}</m:groupChr>"#
        )) else {
            panic!("a group character");
        };
        assert_eq!(
            (
                group.character.as_deref(),
                group.position,
                group.vertical_justification
            ),
            (
                Some("⏞"),
                Some(MathVerticalPosition::Top),
                Some(MathVerticalPosition::Bottom)
            )
        );
        let MathExpression::Phantom(phantom) = round_trip(&format!(
            r#"<m:phant><m:phantPr><m:show m:val="0"/><m:zeroWid m:val="1"/></m:phantPr>{x}</m:phant>"#
        )) else {
            panic!("a phantom");
        };
        assert_eq!(
            (phantom.show, phantom.zero_width),
            (Some(false), Some(true))
        );
        let MathExpression::EquationArray(array) = round_trip(&format!(
            r#"<m:eqArr><m:eqArrPr><m:baseJc m:val="top"/><m:rSpRule m:val="3"/><m:rSp m:val="120"/></m:eqArrPr>{x}<m:e><m:r><m:t>y&amp;=1</m:t></m:r></m:e></m:eqArr>"#
        )) else {
            panic!("an equation array");
        };
        assert_eq!(
            (
                array.rows.len(),
                array.base_justification,
                array.row_spacing_rule,
                array.row_spacing
            ),
            (2, Some(MatrixBaseJustification::Top), Some(3), Some(120))
        );
        let (before, after) = array.rows[1].split_at_alignment();
        let text = |argument: &super::MathArgument| match argument.expressions.as_slice() {
            [MathExpression::Run(run)] => run.text.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            (text(&before), text(&after)),
            ("y".to_owned(), "=1".to_owned())
        );
        let MathExpression::Matrix(matrix) = round_trip(
            r#"<m:m><m:mPr><m:plcHide m:val="1"/><m:cGpRule m:val="2"/><m:cGp m:val="60"/><m:mcs><m:mc><m:mcPr><m:count m:val="2"/><m:mcJc m:val="left"/></m:mcPr></m:mc><m:mc><m:mcPr><m:mcJc m:val="right"/></m:mcPr></m:mc></m:mcs></m:mPr><m:mr><m:e/><m:e/><m:e/></m:mr></m:m>"#,
        ) else {
            panic!("a matrix");
        };
        assert_eq!(
            (
                matrix.properties.hide_placeholders,
                matrix.properties.column_gap_rule,
                matrix.properties.column_gap
            ),
            (Some(true), Some(2), Some(60))
        );
        assert_eq!(
            (0..4)
                .map(|column| matrix.column_justification(column))
                .collect::<Vec<_>>(),
            [
                MatrixColumnJustification::Left,
                MatrixColumnJustification::Left,
                MatrixColumnJustification::Right,
                MatrixColumnJustification::Center,
            ]
        );
        // Google Docs writes an n-ary with no m:e and a bar position with no
        // value; Word's default places a bar below.
        let MathExpression::Nary(nary) = round_trip(
            r#"<m:nary><m:naryPr><m:chr m:val="∑"/><m:subHide m:val="0"/><m:supHide m:val="0"/></m:naryPr><m:sub><m:r><m:t>i</m:t></m:r></m:sub><m:sup><m:r><m:t>N</m:t></m:r></m:sup></m:nary>"#,
        ) else {
            panic!("an n-ary");
        };
        assert!(nary.base.expressions.is_empty());
        let xml = format!(
            r#"<m:oMath xmlns:m="{M}"><m:bar><m:barPr><m:pos/></m:barPr>{x}</m:bar></m:oMath>"#
        );
        let equation = CT_OMath::from_xml(xml.as_bytes()).unwrap();
        assert!(!equation.has_unsupported_content());
        assert!(matches!(
            equation.expressions.as_slice(),
            [MathExpression::Bar(bar)] if bar.position == Some(MathVerticalPosition::Bottom)
        ));
    }

    #[test]
    fn a_canonical_equation_leaves_out_where_and_how_it_was_declared() {
        let declared_once = format!(
            r#"<m:oMath xmlns:m="{M}" xmlns:w="{W}" xmlns:r="urn:unused"><m:r><w:rPr><w:i/></w:rPr><m:t>a&apos;</m:t></m:r></m:oMath>"#
        );
        let declared_inside = format!(
            r#"<m:oMath xmlns:m="{M}"><m:r><w:rPr xmlns:w="{W}"><w:i></w:i></w:rPr><m:t>a'</m:t></m:r></m:oMath>"#
        );
        assert_eq!(canonical(&declared_once), canonical(&declared_inside));
    }

    #[test]
    fn a_canonical_equation_keeps_what_its_names_and_values_resolve_to() {
        // A default namespace undeclared under one that declared it.
        let undeclared = r#"<a xmlns="urn:a"><x xmlns=""/></a>"#;
        let inherited = r#"<a xmlns="urn:a"><x/></a>"#;
        assert_ne!(canonical(undeclared), canonical(inherited));
        // A prefix only a markup-compatibility value names.
        let ignorable = |uri: &str| {
            canonical(&format!(
                r#"<m:oMath xmlns:m="{M}" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:x="{uri}" mc:Ignorable="x"/>"#
            ))
        };
        assert_ne!(ignorable("urn:a"), ignorable("urn:b"));
        // A prefix `mc:Choice` requires.
        let requires = |uri: &str| {
            canonical(&format!(
                r#"<m:oMath xmlns:m="{M}" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:x="{uri}"><mc:AlternateContent><mc:Choice Requires="x"/></mc:AlternateContent></m:oMath>"#
            ))
        };
        assert_ne!(requires("urn:a"), requires("urn:b"));
    }
}
