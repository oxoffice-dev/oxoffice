//! Table elements: `CT_Tbl`, `CT_Row`, `CT_Tc` and related types.

use crate::text::XmlWriteRole;
use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::{Reader, Writer, XmlVersion};

use crate::borders::CT_BorderEdge;
use crate::content_control::{CT_Sdt, SdtContent, SdtOwner};
use crate::drawing::AnchorAlignH;
use crate::error::{OxmlError, Result};
use crate::namespace::matches_local_name;
use crate::numbering::{
    local_namespace_overrides, merged_owner_bindings, namespace_bindings, word_prefixes_at,
};
use crate::properties::{
    CT_Shd, get_word_val_attr, is_word_attribute, is_word_element, parse_integer_measurement,
    parse_word_toggle,
};
use crate::raw_xml::{capture_element, capture_empty_element};
use crate::revision::{CT_Revision, RevisionKind};
use crate::shared::ST_Jc;
use crate::text::{
    CT_P, ROOT_ATTRIBUTES_POSITION, capture_root_attribute_record, is_root_attribute_record,
};
use crate::units::Twips;

/// Concrete scalar kernels used by ordinary constructors and source facts.
#[derive(Clone, Copy)]
pub(crate) enum TableScalar {
    Measurement,
    /// A `CT_TblWidth` `w:w`: an integer, a percent or a universal measure.
    Width,
    /// A cell margin `w:w`: an integer or a universal measure, not a percent.
    MarginWidth,
    Unsigned,
    PositiveSpan,
    Merge,
}
impl TableScalar {
    pub(crate) fn for_names(element: &[u8], attribute: &[u8]) -> Option<Self> {
        match (element, attribute) {
            (b"tblW" | b"tblInd" | b"tblCellSpacing" | b"tcW" | b"wBefore" | b"wAfter", b"w") => {
                Some(Self::Width)
            }
            (b"gridCol", b"w")
            | (
                b"tblpPr",
                b"leftFromText" | b"rightFromText" | b"topFromText" | b"bottomFromText" | b"tblpX"
                | b"tblpY",
            )
            | (b"trHeight", b"val") => Some(Self::Measurement),
            (
                b"tblStyleRowBandSize" | b"tblStyleColBandSize" | b"gridBefore" | b"gridAfter",
                b"val",
            ) => Some(Self::Unsigned),
            (b"gridSpan", b"val") => Some(Self::PositiveSpan),
            (b"vMerge", b"val") => Some(Self::Merge),
            _ => None,
        }
    }

    pub(crate) fn unsigned(self, value: &str) -> Result<u32> {
        let value = value.parse()?;
        if matches!(self, Self::PositiveSpan) && value == 0 {
            return Err(OxmlError::InvalidValue(
                "w:gridSpan must be positive".to_owned(),
            ));
        }
        Ok(value)
    }
    pub(crate) fn merge(value: Option<&str>) -> Result<VMerge> {
        match value {
            Some("restart") => Ok(VMerge::Restart),
            Some("continue") | None => Ok(VMerge::Continue),
            Some(value) => Err(OxmlError::InvalidValue(format!(
                "invalid w:vMerge value {value}"
            ))),
        }
    }
    pub(crate) fn decode(self, element: &[u8], attribute: &[u8], value: &str) -> Result<()> {
        match self {
            Self::Measurement => {
                parse_integer_measurement::<i32>(element, attribute, value).map(|_| ())
            }
            Self::Width => parse_measurement_or_percent(element, attribute, value).map(|_| ()),
            Self::MarginWidth => parse_margin_width(element, attribute, value).map(|_| ()),
            Self::Unsigned | Self::PositiveSpan => self.unsigned(value).map(|_| ()),
            Self::Merge => Self::merge(Some(value)).map(|_| ()),
        }
    }
}

pub(crate) const MAX_RECOGNIZED_TABLE_NESTING: usize = 32;
/// Known sequence boundaries for the concrete table, row and cell owners.
/// Unknown preserved children do not change a modeled boundary.
pub(crate) fn checked_owner_child_slot(parent: &[u8], child: &[u8]) -> Result<Option<(u8, bool)>> {
    let slot = match (parent, child) {
        (b"tbl", b"tblPr") | (b"tr", b"tblPrEx") | (b"tc", b"tcPr") => Some((0, false)),
        (b"tbl", b"tblGrid") | (b"tr", b"trPr") => Some((1, false)),
        (b"tbl", b"tr" | b"sdt") | (b"tr", b"tc" | b"sdt") => Some((2, true)),
        (b"tc", b"p" | b"tbl" | b"sdt") => Some((1, true)),
        (
            b"tbl" | b"tr" | b"tc",
            b"p" | b"r" | b"tbl" | b"tr" | b"tc" | b"tblPr" | b"tblGrid" | b"tblPrEx" | b"trPr"
            | b"tcPr" | b"pPr" | b"rPr" | b"sectPr",
        ) => {
            return Err(OxmlError::InvalidValue(
                "element XML has a known child at the wrong table owner".into(),
            ));
        }
        _ => None,
    };
    Ok(slot)
}
/// Check native table property values only on their actual checked owner route.
/// Ordinary imported-source readers retain their preservation behavior.
pub(crate) fn validate_checked_property_child(
    element: &BytesStart<'_>,
    prefixes: &[String],
    parent: &[u8],
) -> Result<bool> {
    let name = element.local_name();
    let local = name.as_ref();
    let value = get_word_val_attr(element, prefixes)?;
    let recognized = match (parent, local) {
        (b"tblPr" | b"tblPrEx", b"jc") => {
            if value
                .as_deref()
                .is_none_or(|value| ST_Jc::from_str(value).is_err())
            {
                return Err(OxmlError::InvalidValue(
                    "element XML table justification requires a valid w:val".into(),
                ));
            }
            true
        }
        (b"tcPr", b"gridSpan") => {
            if value
                .as_deref()
                .and_then(|value| value.parse::<u32>().ok())
                .is_none_or(|span| span == 0)
            {
                return Err(OxmlError::InvalidValue(
                    "element XML cell gridSpan requires a positive w:val".into(),
                ));
            }
            true
        }
        (b"tcPr", b"hMerge" | b"vMerge") => {
            if value
                .as_deref()
                .is_some_and(|value| !matches!(value, "restart" | "continue"))
            {
                return Err(OxmlError::InvalidValue(
                    "element XML cell merge has an invalid w:val".into(),
                ));
            }
            true
        }
        (b"tcPr", b"vAlign") => {
            if value
                .as_deref()
                .is_none_or(|value| !matches!(value, "top" | "center" | "bottom" | "both"))
            {
                return Err(OxmlError::InvalidValue(
                    "element XML cell alignment requires a valid w:val".into(),
                ));
            }
            true
        }
        (b"tblPr" | b"tblPrEx", b"tblW" | b"tblInd" | b"tblCellSpacing") | (b"tcPr", b"tcW") => {
            for attribute in element.attributes() {
                let attribute = attribute?;
                if is_word_attribute(attribute.key.as_ref(), b"type", prefixes) {
                    let value = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?;
                    if !matches!(value.as_ref(), "auto" | "nil" | "dxa" | "pct") {
                        return Err(OxmlError::InvalidValue(
                            "element XML table width has an invalid type".into(),
                        ));
                    }
                }
            }
            true
        }
        (b"tblPr" | b"tblPrEx", b"tblLayout") => {
            let mut found = false;
            for attribute in element.attributes() {
                let attribute = attribute?;
                if is_word_attribute(attribute.key.as_ref(), b"type", prefixes) {
                    let value = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?;
                    if !matches!(value.as_ref(), "fixed" | "autofit") {
                        return Err(OxmlError::InvalidValue(
                            "element XML table layout has an invalid type".into(),
                        ));
                    }
                    found = true;
                }
            }
            if !found {
                return Err(OxmlError::InvalidValue(
                    "element XML table layout requires w:type".into(),
                ));
            }
            true
        }
        _ => false,
    };
    Ok(recognized)
}

/// Write any captured raw XML that belongs immediately before position `pos`.
///
/// Table children we do not model are stored as `(position, raw)` pairs so
/// they can be put back where they were found, the same way `CT_P` handles
/// its own unknown children.
fn write_extras_at<W: std::io::Write>(
    writer: &mut Writer<W>,
    extra_xml: &[(usize, Vec<u8>)],
    pos: usize,
) -> Result<()> {
    for (at, raw) in extra_xml {
        if *at == pos {
            writer.get_mut().write_all(raw)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)] // Preserve the actual boundary and inherited emission contexts.
fn write_boundary<W: std::io::Write>(
    writer: &mut Writer<W>,
    extra_xml: &[(usize, Vec<u8>)],
    controls: &[(usize, usize, CT_Sdt)],
    pos: usize,
    role: XmlWriteRole,
    source_context: Option<&[(&str, &str)]>,
    work: &mut crate::text::WordOperationWork,
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<()> {
    let extras = extra_xml
        .iter()
        .filter(|(at, _)| *at == pos)
        .map(|(_, raw)| raw)
        .collect::<Vec<_>>();
    for raw_before in 0..=extras.len() {
        for (_, _, control) in controls
            .iter()
            .filter(|(at, before, _)| *at == pos && *before == raw_before)
        {
            control.write_xml_role_in(writer, role, source_context, work, emission)?;
        }
        if let Some(raw) = extras.get(raw_before) {
            writer.get_mut().write_all(raw)?;
        }
    }
    Ok(())
}

fn preserved_table_raw_bindings(scope: &[String]) -> Vec<(String, String)> {
    namespace_bindings(scope)
        .into_iter()
        .filter(|(prefix, namespace)| prefix != "w" || namespace != crate::namespace::W_NS)
        .collect()
}

// ---- Table border types ----

/// `CT_TblBorders` — Table-level borders.
#[derive(Debug, Clone, Default)]
pub struct CT_TblBorders {
    pub top: Option<CT_BorderEdge>,
    pub bottom: Option<CT_BorderEdge>,
    pub left: Option<CT_BorderEdge>,
    pub right: Option<CT_BorderEdge>,
    pub inside_h: Option<CT_BorderEdge>,
    pub inside_v: Option<CT_BorderEdge>,
    /// Other border edges retained verbatim.
    pub extra_xml: Vec<Vec<u8>>,
    /// Whether this element was read from a part. A read one is written back
    /// even when it carries no edge; one built through the API is written
    /// only when it carries one.
    #[doc(hidden)]
    pub from_source: bool,
}

impl PartialEq for CT_TblBorders {
    fn eq(&self, other: &Self) -> bool {
        let Self {
            top,
            bottom,
            left,
            right,
            inside_h,
            inside_v,
            extra_xml,
            from_source: _,
        } = self;
        *top == other.top
            && *bottom == other.bottom
            && *left == other.left
            && *right == other.right
            && *inside_h == other.inside_h
            && *inside_v == other.inside_v
            && *extra_xml == other.extra_xml
            && self.writes_empty_element() == other.writes_empty_element()
    }
}

impl CT_TblBorders {
    /// An empty `<w:tblBorders/>` or `<w:tcBorders/>` read from a part,
    /// written back as it was.
    pub(crate) fn empty_from_source() -> Self {
        Self {
            from_source: true,
            ..Self::default()
        }
    }

    /// Whether its owner writes this element: when it carries an edge, or
    /// when it was read from a part.
    pub fn writes_element(&self) -> bool {
        !self.is_empty() || self.from_source
    }

    fn writes_empty_element(&self) -> bool {
        self.from_source && self.is_empty()
    }

    pub fn from_xml(reader: &mut Reader<&[u8]>) -> Result<Self> {
        Self::from_xml_with_prefixes_and_owner_bindings(reader, &["w".to_owned()], &[])
    }

    fn from_xml_with_prefixes_and_owner_bindings(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
    ) -> Result<Self> {
        let mut borders = CT_TblBorders::empty_from_source();
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(name.as_ref(), b"top", &prefixes) {
                        borders.top =
                            Some(CT_BorderEdge::from_xml_attrs_with_prefixes(e, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"bottom", &prefixes) {
                        borders.bottom =
                            Some(CT_BorderEdge::from_xml_attrs_with_prefixes(e, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"left", &prefixes)
                        || is_word_element(name.as_ref(), b"start", &prefixes)
                    {
                        borders.left =
                            Some(CT_BorderEdge::from_xml_attrs_with_prefixes(e, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"right", &prefixes)
                        || is_word_element(name.as_ref(), b"end", &prefixes)
                    {
                        borders.right =
                            Some(CT_BorderEdge::from_xml_attrs_with_prefixes(e, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"insideH", &prefixes) {
                        borders.inside_h =
                            Some(CT_BorderEdge::from_xml_attrs_with_prefixes(e, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"insideV", &prefixes) {
                        borders.inside_v =
                            Some(CT_BorderEdge::from_xml_attrs_with_prefixes(e, &prefixes)?);
                    } else {
                        borders
                            .extra_xml
                            .push(crate::text::raw_with_external_bindings(
                                &capture_empty_element(e)?,
                                owner_bindings,
                            )?);
                    }
                }
                Ok(Event::Start(ref e)) => {
                    borders
                        .extra_xml
                        .push(crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            owner_bindings,
                        )?)
                }
                Ok(Event::End(ref e))
                    if matches_local_name(e.name().as_ref(), b"tblBorders")
                        || matches_local_name(e.name().as_ref(), b"tcBorders") =>
                {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        Ok(borders)
    }

    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>, tag: &str) -> Result<()> {
        self.write_xml_core(writer, tag, XmlWriteRole::Publication, None, None)
    }

    fn write_xml_core<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        tag: &str,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let source = role == XmlWriteRole::Source;
        let mut root = crate::text::FieldXmlStart::new(tag, emission)?;
        let prefix = if source {
            Some(root.qualify(&[], role, source_context)?)
        } else {
            None
        };
        if self.is_empty() {
            return root.write(writer, true);
        }
        root.write(writer, false)?;
        for (local, qualified, edge) in [
            ("top", "w:top", &self.top),
            ("left", "w:left", &self.left),
            ("bottom", "w:bottom", &self.bottom),
            ("right", "w:right", &self.right),
            ("insideH", "w:insideH", &self.inside_h),
            ("insideV", "w:insideV", &self.inside_v),
        ] {
            if let Some(edge) = edge {
                edge.write_xml_in(
                    writer,
                    if source { local } else { qualified },
                    role,
                    source_context,
                    emission,
                )?;
            }
        }
        for raw in &self.extra_xml {
            writer.get_mut().write_all(raw)?;
        }
        if let Some(prefix) = prefix {
            let local = tag.split_once(':').map_or(tag, |(_, local)| local);
            if let Some(emission) = emission {
                emission.write_word_end(writer, &prefix, local)?;
            } else {
                writer.write_event(Event::End(BytesEnd::new(format!("{prefix}:{local}"))))?;
            }
        } else if let Some(emission) = emission {
            emission.write_end_name(writer, tag)?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(tag)))?;
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.top.is_none()
            && self.bottom.is_none()
            && self.left.is_none()
            && self.right.is_none()
            && self.inside_h.is_none()
            && self.inside_v.is_none()
            && self.extra_xml.is_empty()
    }

    /// Whether a modelled edge set here draws a line: one set to `nil` or
    /// `none` draws none.
    pub fn draws_an_edge(&self) -> bool {
        [
            &self.top,
            &self.bottom,
            &self.left,
            &self.right,
            &self.inside_h,
            &self.inside_v,
        ]
        .into_iter()
        .flatten()
        .any(|edge| !edge.val.is_none())
    }
}

/// Table cell margin (a single edge width).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CT_TblCellMar {
    pub top: Option<Twips>,
    pub bottom: Option<Twips>,
    pub left: Option<Twips>,
    pub right: Option<Twips>,
}

impl CT_TblCellMar {
    fn parse_edge(e: &BytesStart, word_prefixes: &[String]) -> Result<Option<Twips>> {
        for attr in e.attributes() {
            let attr = attr?;
            if is_word_attribute(attr.key.as_ref(), b"w", word_prefixes) {
                let value = std::str::from_utf8(&attr.value)?;
                let val = parse_margin_width(e.name().as_ref(), attr.key.as_ref(), value)?;
                return Ok(Some(Twips(val)));
            }
        }
        Ok(None)
    }

    pub fn from_xml(reader: &mut Reader<&[u8]>) -> Result<Self> {
        Self::from_xml_with_prefixes(reader, &["w".to_owned()])
    }

    fn from_xml_with_prefixes(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
    ) -> Result<Self> {
        let mut mar = CT_TblCellMar::default();
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(name.as_ref(), b"top", &prefixes) {
                        mar.top = Self::parse_edge(e, &prefixes)?;
                    } else if is_word_element(name.as_ref(), b"bottom", &prefixes) {
                        mar.bottom = Self::parse_edge(e, &prefixes)?;
                    } else if is_word_element(name.as_ref(), b"left", &prefixes)
                        || is_word_element(name.as_ref(), b"start", &prefixes)
                    {
                        mar.left = Self::parse_edge(e, &prefixes)?;
                    } else if is_word_element(name.as_ref(), b"right", &prefixes)
                        || is_word_element(name.as_ref(), b"end", &prefixes)
                    {
                        mar.right = Self::parse_edge(e, &prefixes)?;
                    }
                }
                Ok(Event::End(ref e))
                    if matches_local_name(e.name().as_ref(), b"tblCellMar")
                        || matches_local_name(e.name().as_ref(), b"tcMar") =>
                {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        Ok(mar)
    }

    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.to_xml_with_tag(writer, "w:tblCellMar")
    }

    fn to_xml_with_tag<W: std::io::Write>(&self, writer: &mut Writer<W>, tag: &str) -> Result<()> {
        self.write_xml_in(writer, tag, None)
    }

    fn write_xml_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        tag: &str,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        crate::text::FieldXmlStart::new(tag, emission)?.write(writer, false)?;

        fn write_edge<W: std::io::Write>(
            writer: &mut Writer<W>,
            tag: &str,
            val: Twips,
            emission: Option<&crate::text::FieldXmlEmission>,
        ) -> Result<()> {
            let mut buf = itoa::Buffer::new();
            let mut e = crate::text::FieldXmlStart::new(tag, emission)?;
            e.push("w:w", buf.format(val.0))?;
            e.push("w:type", "dxa")?;
            e.write(writer, true)?;
            Ok(())
        }

        if let Some(t) = self.top {
            write_edge(writer, "w:top", t, emission)?;
        }
        if let Some(l) = self.left {
            write_edge(writer, "w:left", l, emission)?;
        }
        if let Some(b) = self.bottom {
            write_edge(writer, "w:bottom", b, emission)?;
        }
        if let Some(r) = self.right {
            write_edge(writer, "w:right", r, emission)?;
        }

        if let Some(emission) = emission {
            emission.write_end_name(writer, tag)?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(tag)))?;
        }
        Ok(())
    }
}

// ---- Table width ----

/// Table width specification.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_TblWidth {
    /// Width value
    pub w: i32,
    /// Width type: "dxa" (twips), "pct" (50ths of a percent), "auto", "nil"
    pub width_type: String,
}

impl CT_TblWidth {
    pub fn dxa(twips: i32) -> Self {
        CT_TblWidth {
            w: twips,
            width_type: "dxa".to_string(),
        }
    }

    pub fn pct(fiftieths: i32) -> Self {
        CT_TblWidth {
            w: fiftieths,
            width_type: "pct".to_string(),
        }
    }

    pub fn auto() -> Self {
        CT_TblWidth {
            w: 0,
            width_type: "auto".to_string(),
        }
    }

    pub fn from_xml_attrs(e: &BytesStart) -> Result<Self> {
        let word_prefixes = word_prefixes_at(e, &["w".to_owned()])?.into_owned();
        Self::from_xml_attrs_with_prefixes(e, &word_prefixes)
    }

    fn from_xml_attrs_with_prefixes(e: &BytesStart, word_prefixes: &[String]) -> Result<Self> {
        let mut w = 0;
        let mut width_type = "dxa".to_string();
        let mut spelled_type = None;

        for attr in e.attributes() {
            let attr = attr?;
            let key = attr.key.as_ref();
            let val = std::str::from_utf8(&attr.value)?;
            if is_word_attribute(key, b"w", word_prefixes) {
                (w, spelled_type) = parse_measurement_or_percent(e.name().as_ref(), key, val)?;
            } else if is_word_attribute(key, b"type", word_prefixes) {
                width_type = val.to_string();
            }
        }
        // A percent or a unit spelled in `w:w` says what the number is.
        if let Some(spelled_type) = spelled_type {
            width_type = spelled_type.to_owned();
        }

        Ok(CT_TblWidth { w, width_type })
    }

    pub fn write_xml<W: std::io::Write>(&self, writer: &mut Writer<W>, tag: &str) -> Result<()> {
        self.write_xml_core(writer, tag, XmlWriteRole::Publication, None, None)
    }

    fn write_xml_core<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        tag: &str,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let mut e = crate::text::FieldXmlStart::new(tag, emission)?;
        let prefix = if role == XmlWriteRole::Source {
            e.qualify(&[], role, source_context)?
        } else {
            "w".to_owned()
        };
        let mut buf = itoa::Buffer::new();
        e.push_word(&prefix, "w", buf.format(self.w))?;
        e.push_word(&prefix, "type", self.width_type.as_str())?;
        e.write(writer, true)?;
        Ok(())
    }
}

// ---- Table grid column ----

/// `CT_TblGridCol` — A column definition in the table grid.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_TblGridCol {
    /// Column width in twips
    pub width: Twips,
}

/// Read a `w:val` attribute that carries free text rather than a token.
///
/// `get_word_val_attr` returns the attribute exactly as written, which is
/// right for an enumeration or an identifier and wrong for prose. A caption
/// or a description can contain `&` or `<`, so the writer escapes it and the
/// reader has to put it back. A malformed entity is kept verbatim rather than
/// failing the parse.
fn unescaped_word_val_attr(e: &BytesStart, word_prefixes: &[String]) -> Result<Option<String>> {
    Ok(get_word_val_attr(e, word_prefixes)?.map(|value| {
        quick_xml::escape::unescape(&value)
            .map(|unescaped| unescaped.into_owned())
            .unwrap_or(value)
    }))
}

// ---- Floating table position ----

/// `ST_TblAnchor` — what a floating table's offset is measured from.
///
/// Three values, not the eight of `ST_RelativeFromH`. A parse type that
/// admitted the other five would admit states `w:tblpPr` cannot write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_TblAnchor {
    Margin,
    Page,
    Text,
}

impl ST_TblAnchor {
    /// Parse an anchor. An unrecognised value reads as no anchor, which falls
    /// back to Word's default rather than inventing a position.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "margin" => Some(Self::Margin),
            "page" => Some(Self::Page),
            "text" => Some(Self::Text),
            _ => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Margin => "margin",
            Self::Page => "page",
            Self::Text => "text",
        }
    }
}

/// `ST_YAlign` — the vertical alignment spelling `w:tblpYSpec` accepts.
///
/// A separate type from `AnchorAlignV` because `tblpYSpec` adds `inline`.
/// Folding that into an optional alignment plus a flag would make an inline
/// spec that also carries an alignment representable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_YAlign {
    Inline,
    Top,
    Center,
    Bottom,
    Inside,
    Outside,
}

impl ST_YAlign {
    /// Parse an alignment. An unrecognised value reads as no alignment.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "inline" => Some(Self::Inline),
            "top" => Some(Self::Top),
            "center" => Some(Self::Center),
            "bottom" => Some(Self::Bottom),
            "inside" => Some(Self::Inside),
            "outside" => Some(Self::Outside),
            _ => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Top => "top",
            Self::Center => "center",
            Self::Bottom => "bottom",
            Self::Inside => "inside",
            Self::Outside => "outside",
        }
    }
}

/// `ST_TblOverlap` — whether a floating table may overlap another float.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_TblOverlap {
    Never,
    Overlap,
}

impl ST_TblOverlap {
    /// Parse an overlap policy. An unrecognised value reads as absent.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "never" => Some(Self::Never),
            "overlap" => Some(Self::Overlap),
            _ => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Overlap => "overlap",
        }
    }
}

/// `CT_TblPPr` — the floating table position.
///
/// Modeled and authored here. Layout does not read it yet, so a floating
/// table round-trips exactly and renders in the flow.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CT_TblPPr {
    pub left_from_text: Option<Twips>,
    pub right_from_text: Option<Twips>,
    pub top_from_text: Option<Twips>,
    pub bottom_from_text: Option<Twips>,
    pub horz_anchor: Option<ST_TblAnchor>,
    pub vert_anchor: Option<ST_TblAnchor>,
    pub tbl_p_x: Option<Twips>,
    /// `w:tblpXSpec`, which reuses the drawing anchor's five horizontal
    /// alignments because the two vocabularies are identical.
    pub tbl_p_x_spec: Option<AnchorAlignH>,
    pub tbl_p_y: Option<Twips>,
    pub tbl_p_y_spec: Option<ST_YAlign>,
}

/// Read a `w:w` of `ST_MeasurementOrPercent`: an integer (twips or
/// fiftieths of a percent, as `w:type` says), a percent such as `"33.3%"`,
/// or a universal measure such as `"2in"`. The last two also give the type
/// they imply, `pct` or `dxa`, and are stored in those units.
fn parse_measurement_or_percent(
    element: &[u8],
    attribute: &[u8],
    value: &str,
) -> Result<(i32, Option<&'static str>)> {
    // Twips per unit, as numerator over denominator.
    const UNITS: [(&str, i128, i128); 6] = [
        ("in", 1440, 1),
        ("cm", 144000, 254),
        ("mm", 14400, 254),
        ("pt", 20, 1),
        ("pc", 240, 1),
        ("pi", 240, 1),
    ];
    let spelled = value
        .strip_suffix('%')
        .map(|number| (number, 50, 1, "pct"))
        .or_else(|| {
            UNITS.iter().find_map(|(unit, numerator, denominator)| {
                value
                    .strip_suffix(unit)
                    .map(|number| (number, *numerator, *denominator, "dxa"))
            })
        });
    let Some((number, numerator, denominator, width_type)) = spelled else {
        return Ok((parse_integer_measurement(element, attribute, value)?, None));
    };
    let scaled = scaled_decimal(number, numerator, denominator).ok_or_else(|| {
        OxmlError::InvalidValue(format!(
            "{}/@{} {value:?} is not a measurement or a percent",
            String::from_utf8_lossy(element),
            String::from_utf8_lossy(attribute),
        ))
    })?;
    Ok((scaled, Some(width_type)))
}

/// Read a cell margin `w:w`: twips, written as an integer or a universal
/// measure. A margin is kept in twips, so a percent is refused.
fn parse_margin_width(element: &[u8], attribute: &[u8], value: &str) -> Result<i32> {
    match parse_measurement_or_percent(element, attribute, value)? {
        (_, Some("pct")) => Err(OxmlError::InvalidValue(format!(
            "{}/@{} {value:?}: a cell margin cannot be a percent",
            String::from_utf8_lossy(element),
            String::from_utf8_lossy(attribute),
        ))),
        (twips, _) => Ok(twips),
    }
}

/// `-?[0-9]+(\.[0-9]+)?` times `numerator / denominator`, rounded half away
/// from zero in decimal arithmetic from the first eighteen fraction digits,
/// if it fits an `i32`.
fn scaled_decimal(number: &str, numerator: i128, denominator: i128) -> Option<i32> {
    let (negative, unsigned) = match number.strip_prefix('-') {
        Some(unsigned) => (true, unsigned),
        None => (false, number),
    };
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || (unsigned.contains('.') && fraction.is_empty())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    // The smallest scale is 20 twips a point, so eleven whole digits never
    // fit an i32; with eighteen fraction digits the products stay well
    // inside an i128, and checked arithmetic refuses anything else.
    let whole = whole.trim_start_matches('0');
    let fraction = &fraction[..fraction.len().min(18)];
    if whole.len() > 10 {
        return None;
    }
    // "0", "00" or "-0" leave no digit once the zeros are trimmed.
    let digits = format!("{whole}{fraction}");
    let digits: i128 = if digits.is_empty() {
        0
    } else {
        digits.parse().ok()?
    };
    let scale = 10i128
        .checked_pow(fraction.len() as u32)?
        .checked_mul(denominator)?;
    let product = digits.checked_mul(numerator)?;
    let rounded = product
        .checked_mul(2)?
        .checked_add(scale)?
        .checked_div(scale.checked_mul(2)?)?;
    let signed = if negative { -rounded } else { rounded };
    i32::try_from(signed).ok()
}

#[allow(non_snake_case)]
impl CT_TblPPr {
    fn from_xml_attrs_with_prefixes(e: &BytesStart, word_prefixes: &[String]) -> Result<Self> {
        let mut position = CT_TblPPr::default();
        for attr in e.attributes() {
            let attr = attr?;
            let key = attr.key.as_ref();
            let value = std::str::from_utf8(&attr.value)?;
            let measurement = || parse_integer_measurement(e.name().as_ref(), key, value);
            if is_word_attribute(key, b"leftFromText", word_prefixes) {
                position.left_from_text = Some(Twips(measurement()?));
            } else if is_word_attribute(key, b"rightFromText", word_prefixes) {
                position.right_from_text = Some(Twips(measurement()?));
            } else if is_word_attribute(key, b"topFromText", word_prefixes) {
                position.top_from_text = Some(Twips(measurement()?));
            } else if is_word_attribute(key, b"bottomFromText", word_prefixes) {
                position.bottom_from_text = Some(Twips(measurement()?));
            } else if is_word_attribute(key, b"horzAnchor", word_prefixes) {
                position.horz_anchor = ST_TblAnchor::parse(value);
            } else if is_word_attribute(key, b"vertAnchor", word_prefixes) {
                position.vert_anchor = ST_TblAnchor::parse(value);
            } else if is_word_attribute(key, b"tblpX", word_prefixes) {
                position.tbl_p_x = Some(Twips(measurement()?));
            } else if is_word_attribute(key, b"tblpXSpec", word_prefixes) {
                position.tbl_p_x_spec = AnchorAlignH::parse(value);
            } else if is_word_attribute(key, b"tblpY", word_prefixes) {
                position.tbl_p_y = Some(Twips(measurement()?));
            } else if is_word_attribute(key, b"tblpYSpec", word_prefixes) {
                position.tbl_p_y_spec = ST_YAlign::parse(value);
            }
        }
        Ok(position)
    }

    pub fn write_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.write_xml_role(writer, XmlWriteRole::Publication, None)
    }

    pub(crate) fn write_xml_role<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<()> {
        self.write_xml_role_in(writer, role, source_context, None)
    }

    pub(crate) fn write_xml_role_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let mut start = crate::text::FieldXmlStart::new("w:tblpPr", emission)?;
        let prefix = if role == XmlWriteRole::Source {
            start.qualify(&[], role, source_context)?
        } else {
            "w".to_owned()
        };

        let mut buf = itoa::Buffer::new();
        let mut e = start;
        for (name, value) in [
            ("leftFromText", self.left_from_text),
            ("rightFromText", self.right_from_text),
            ("topFromText", self.top_from_text),
            ("bottomFromText", self.bottom_from_text),
        ] {
            if let Some(value) = value {
                e.push_word(&prefix, name, buf.format(value.0))?;
            }
        }
        if let Some(anchor) = self.horz_anchor {
            e.push_word(&prefix, "horzAnchor", anchor.to_str())?;
        }
        if let Some(anchor) = self.vert_anchor {
            e.push_word(&prefix, "vertAnchor", anchor.to_str())?;
        }
        if let Some(spec) = self.tbl_p_x_spec {
            e.push_word(&prefix, "tblpXSpec", spec.to_str())?;
        }
        if let Some(value) = self.tbl_p_x {
            e.push_word(&prefix, "tblpX", buf.format(value.0))?;
        }
        if let Some(spec) = self.tbl_p_y_spec {
            e.push_word(&prefix, "tblpYSpec", spec.to_str())?;
        }
        if let Some(value) = self.tbl_p_y {
            e.push_word(&prefix, "tblpY", buf.format(value.0))?;
        }
        e.write(writer, true)?;
        Ok(())
    }
}

// ---- Table properties ----

/// `CT_TblPr` — Table properties.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CT_TblPr {
    /// Table style ID
    pub style_id: Option<String>,
    /// `w:tblpPr` — the floating table position.
    ///
    /// Boxed, like the other composite members of this family, so a table
    /// stays cheap on the stack. Test threads build whole documents by value
    /// against a 2 MiB ceiling.
    pub float_position: Option<Box<CT_TblPPr>>,
    /// `w:tblOverlap` — whether this float may overlap another.
    pub overlap: Option<ST_TblOverlap>,
    /// `w:bidiVisual` — reverse visual column placement.
    pub bidi_visual: Option<bool>,
    /// `w:tblStyleRowBandSize` — rows per horizontal conditional band.
    ///
    /// A count, not a length, so no unit constructor applies. Absent means one
    /// row per band, which is what Word assumes.
    pub row_band_size: Option<u32>,
    /// `w:tblStyleColBandSize` — columns per vertical conditional band.
    pub column_band_size: Option<u32>,
    /// Table width
    pub width: Option<CT_TblWidth>,
    /// Table alignment
    pub jc: Option<ST_Jc>,
    /// `w:tblCellSpacing` — the gap between adjacent cell content boxes.
    pub cell_spacing: Option<CT_TblWidth>,
    /// Table borders
    pub borders: Option<CT_TblBorders>,
    /// Default cell margins
    pub cell_margin: Option<CT_TblCellMar>,
    /// Table layout: "fixed" or "autofit"
    pub layout: Option<String>,
    /// Table indent from left margin
    pub indent: Option<CT_TblWidth>,
    /// Table shading/background
    pub shading: Option<CT_Shd>,
    /// Which parts of the table style's conditional formatting apply.
    pub look: Option<CT_TblLook>,
    /// `w:tblCaption` — the accessible table caption.
    pub caption: Option<String>,
    /// `w:tblDescription` — the accessible table description.
    pub description: Option<String>,
    /// Prior table properties from the schema-final `w:tblPrChange`.
    pub change: Option<CT_Revision>,
    /// Malformed table property changes retained verbatim.
    pub revision_xml: Vec<Vec<u8>>,
    /// Unmodelled property children retained at their schema insertion slots.
    #[doc(hidden)]
    pub extra_xml: Vec<(usize, Vec<u8>)>,
}

/// `w:tblLook` — which parts of a table style's conditional formatting apply.
///
/// The style reference in `w:tblStyle` says *which* style to use. This says
/// which of its conditional parts to turn on: header row emphasis, banding,
/// first-column formatting. Dropping it leaves the style name intact and the
/// table rendered with base formatting only, which reads as the style having
/// been lost.
///
/// `w:val` is a legacy bitmask carrying the same information. Both are kept,
/// because writers disagree about which one to emit and readers disagree about
/// which one to trust.
#[derive(Debug, Clone, PartialEq, Default)]
#[allow(non_snake_case)]
pub struct CT_TblLook {
    /// Legacy bitmask form, e.g. "04A0".
    pub val: Option<String>,
    pub first_row: Option<bool>,
    pub last_row: Option<bool>,
    pub first_column: Option<bool>,
    pub last_column: Option<bool>,
    pub no_h_band: Option<bool>,
    pub no_v_band: Option<bool>,
}

/// Read an OOXML boolean attribute, which may be written as 1/0 or true/false.
fn parse_ooxml_bool(value: &str) -> Option<bool> {
    match value {
        "1" | "true" | "on" => Some(true),
        "0" | "false" | "off" => Some(false),
        _ => None,
    }
}

fn ooxml_bool_str(value: bool) -> &'static str {
    if value { "1" } else { "0" }
}

#[allow(non_snake_case)]
impl CT_TblLook {
    pub fn from_xml_attrs(e: &BytesStart) -> Result<Self> {
        let mut look = CT_TblLook::default();
        for attr in e.attributes().flatten() {
            let value = std::str::from_utf8(&attr.value)?;
            let key = attr.key.as_ref();
            if matches_local_name(key, b"val") {
                look.val = Some(value.to_string());
            } else if matches_local_name(key, b"firstRow") {
                look.first_row = parse_ooxml_bool(value);
            } else if matches_local_name(key, b"lastRow") {
                look.last_row = parse_ooxml_bool(value);
            } else if matches_local_name(key, b"firstColumn") {
                look.first_column = parse_ooxml_bool(value);
            } else if matches_local_name(key, b"lastColumn") {
                look.last_column = parse_ooxml_bool(value);
            } else if matches_local_name(key, b"noHBand") {
                look.no_h_band = parse_ooxml_bool(value);
            } else if matches_local_name(key, b"noVBand") {
                look.no_v_band = parse_ooxml_bool(value);
            }
        }
        Ok(look)
    }

    fn from_xml_attrs_with_prefixes(e: &BytesStart, word_prefixes: &[String]) -> Result<Self> {
        let mut look = CT_TblLook::default();
        for attr in e.attributes().flatten() {
            let value = std::str::from_utf8(&attr.value)?;
            let key = attr.key.as_ref();
            if is_word_attribute(key, b"val", word_prefixes) {
                look.val = Some(value.to_string());
            } else if is_word_attribute(key, b"firstRow", word_prefixes) {
                look.first_row = parse_ooxml_bool(value);
            } else if is_word_attribute(key, b"lastRow", word_prefixes) {
                look.last_row = parse_ooxml_bool(value);
            } else if is_word_attribute(key, b"firstColumn", word_prefixes) {
                look.first_column = parse_ooxml_bool(value);
            } else if is_word_attribute(key, b"lastColumn", word_prefixes) {
                look.last_column = parse_ooxml_bool(value);
            } else if is_word_attribute(key, b"noHBand", word_prefixes) {
                look.no_h_band = parse_ooxml_bool(value);
            } else if is_word_attribute(key, b"noVBand", word_prefixes) {
                look.no_v_band = parse_ooxml_bool(value);
            }
        }
        Ok(look)
    }

    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.write_xml_in(writer, None)
    }

    fn write_xml_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let mut e = crate::text::FieldXmlStart::new("w:tblLook", emission)?;
        if let Some(ref val) = self.val {
            e.push("w:val", val.as_str())?;
        }
        for (name, value) in [
            ("w:firstRow", self.first_row),
            ("w:lastRow", self.last_row),
            ("w:firstColumn", self.first_column),
            ("w:lastColumn", self.last_column),
            ("w:noHBand", self.no_h_band),
            ("w:noVBand", self.no_v_band),
        ] {
            if let Some(value) = value {
                e.push(name, ooxml_bool_str(value))?;
            }
        }
        e.write(writer, true)?;
        Ok(())
    }
}

#[allow(non_snake_case)]
impl CT_TblPr {
    pub fn from_xml(reader: &mut Reader<&[u8]>) -> Result<Self> {
        Self::from_xml_with_prefixes(reader, &["w".to_owned()])
    }

    pub(crate) fn from_xml_with_prefixes(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
    ) -> Result<Self> {
        Self::from_xml_with_prefixes_and_owner_bindings(reader, word_prefixes, &[])
    }

    pub(crate) fn from_xml_with_prefixes_and_owner_bindings(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
    ) -> Result<Self> {
        let mut pr = CT_TblPr::default();
        let mut change_raw_index = 0usize;
        let mut boundary = 0usize;
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    let (at, next) = tbl_pr_raw_boundary(name.as_ref(), boundary, &prefixes);
                    if is_word_element(name.as_ref(), b"tblStyle", &prefixes) {
                        pr.style_id = get_word_val_attr(e, &prefixes)?;
                    } else if is_word_element(name.as_ref(), b"tblpPr", &prefixes) {
                        pr.float_position = Some(Box::new(
                            CT_TblPPr::from_xml_attrs_with_prefixes(e, &prefixes)?,
                        ));
                    } else if is_word_element(name.as_ref(), b"tblOverlap", &prefixes) {
                        pr.overlap = get_word_val_attr(e, &prefixes)?
                            .as_deref()
                            .and_then(ST_TblOverlap::parse);
                    } else if is_word_element(name.as_ref(), b"bidiVisual", &prefixes) {
                        pr.bidi_visual = Some(parse_word_toggle(e, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"tblCellSpacing", &prefixes) {
                        pr.cell_spacing =
                            Some(CT_TblWidth::from_xml_attrs_with_prefixes(e, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"tblCaption", &prefixes) {
                        pr.caption = unescaped_word_val_attr(e, &prefixes)?;
                    } else if is_word_element(name.as_ref(), b"tblDescription", &prefixes) {
                        pr.description = unescaped_word_val_attr(e, &prefixes)?;
                    } else if is_word_element(name.as_ref(), b"tblStyleRowBandSize", &prefixes) {
                        if let Some(val) = get_word_val_attr(e, &prefixes)? {
                            pr.row_band_size = Some(TableScalar::Unsigned.unsigned(&val)?);
                        }
                    } else if is_word_element(name.as_ref(), b"tblStyleColBandSize", &prefixes) {
                        if let Some(val) = get_word_val_attr(e, &prefixes)? {
                            pr.column_band_size = Some(TableScalar::Unsigned.unsigned(&val)?);
                        }
                    } else if is_word_element(name.as_ref(), b"tblW", &prefixes) {
                        pr.width = Some(CT_TblWidth::from_xml_attrs_with_prefixes(e, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"jc", &prefixes) {
                        if let Some(val) = get_word_val_attr(e, &prefixes)? {
                            pr.jc = ST_Jc::from_str(&val).ok();
                        }
                    } else if is_word_element(name.as_ref(), b"tblLayout", &prefixes) {
                        for attribute in e.attributes() {
                            let attribute = attribute?;
                            if is_word_attribute(attribute.key.as_ref(), b"type", &prefixes) {
                                pr.layout =
                                    Some(std::str::from_utf8(&attribute.value)?.to_string());
                                break;
                            }
                        }
                    } else if is_word_element(name.as_ref(), b"tblInd", &prefixes) {
                        pr.indent = Some(CT_TblWidth::from_xml_attrs_with_prefixes(e, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"shd", &prefixes) {
                        pr.shading = Some(CT_Shd::from_xml_attrs_with_prefixes(e, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"tblLook", &prefixes) {
                        pr.look = Some(CT_TblLook::from_xml_attrs_with_prefixes(e, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"tblBorders", &prefixes)
                        && carries_only_namespace_declarations(e)?
                    {
                        pr.borders = Some(CT_TblBorders::empty_from_source());
                    } else if is_word_element(name.as_ref(), b"tblPrChange", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_empty_element(e)?,
                            owner_bindings,
                        )?;
                        if let Some(revision) = CT_Revision::from_raw(raw.clone(), &prefixes) {
                            if let Some(previous) = pr.change.replace(revision) {
                                pr.revision_xml
                                    .insert(change_raw_index, previous.into_raw_xml());
                            }
                            change_raw_index = pr.revision_xml.len();
                        } else {
                            pr.revision_xml.push(raw);
                        }
                    } else if matches_local_name(name.as_ref(), b"tblPrChange") {
                        pr.revision_xml
                            .push(crate::text::raw_with_external_bindings(
                                &capture_empty_element(e)?,
                                owner_bindings,
                            )?);
                    } else {
                        pr.extra_xml.push((
                            at,
                            crate::text::raw_with_external_bindings(
                                &capture_empty_element(e)?,
                                owner_bindings,
                            )?,
                        ));
                    }
                    boundary = next;
                }
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    let (at, next) = tbl_pr_raw_boundary(name.as_ref(), boundary, &prefixes);
                    if is_word_element(name.as_ref(), b"bidiVisual", &prefixes) {
                        // A toggle a producer wrote with an explicit end tag.
                        pr.bidi_visual = Some(parse_word_toggle(e, &prefixes)?);
                        reader.read_to_end_into(e.name(), &mut Vec::new())?;
                    } else if is_word_element(name.as_ref(), b"tblBorders", &prefixes) {
                        let local_bindings = local_namespace_overrides(e, word_prefixes)?;
                        let border_bindings =
                            merged_owner_bindings(owner_bindings, &local_bindings);
                        pr.borders =
                            Some(CT_TblBorders::from_xml_with_prefixes_and_owner_bindings(
                                reader,
                                &prefixes,
                                &border_bindings,
                            )?);
                    } else if is_word_element(name.as_ref(), b"tblCellMar", &prefixes) {
                        pr.cell_margin =
                            Some(CT_TblCellMar::from_xml_with_prefixes(reader, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"tblPrChange", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            owner_bindings,
                        )?;
                        if let Some(revision) = CT_Revision::from_raw(raw.clone(), &prefixes) {
                            if let Some(previous) = pr.change.replace(revision) {
                                pr.revision_xml
                                    .insert(change_raw_index, previous.into_raw_xml());
                            }
                            change_raw_index = pr.revision_xml.len();
                        } else {
                            pr.revision_xml.push(raw);
                        }
                    } else if matches_local_name(name.as_ref(), b"tblPrChange") {
                        pr.revision_xml
                            .push(crate::text::raw_with_external_bindings(
                                &capture_element(reader, e)?,
                                owner_bindings,
                            )?);
                    } else {
                        pr.extra_xml.push((
                            at,
                            crate::text::raw_with_external_bindings(
                                &capture_element(reader, e)?,
                                owner_bindings,
                            )?,
                        ));
                    }
                    boundary = next;
                }
                Ok(Event::End(ref e)) if matches_local_name(e.name().as_ref(), b"tblPr") => {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        Ok(pr)
    }

    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.write_xml_role(writer, XmlWriteRole::Publication, None)
    }

    pub(crate) fn write_xml_role<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<()> {
        self.write_xml_role_in(writer, role, source_context, None)
    }

    pub(crate) fn write_xml_role_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let mut root = crate::text::FieldXmlStart::new("w:tblPr", emission)?;
        let prefix = if role == XmlWriteRole::Source {
            root.qualify(&[], role, source_context)?
        } else {
            "w".to_owned()
        };
        root.write(writer, false)?;

        write_extras_at(writer, &self.extra_xml, 0)?;
        if let Some(ref style_id) = self.style_id {
            let mut e = crate::text::FieldXmlStart::word(&prefix, "tblStyle", emission)?;
            e.push_word(&prefix, "val", style_id.as_str())?;
            e.write(writer, true)?;
        }

        write_extras_at(writer, &self.extra_xml, 1)?;
        if let Some(position) = &self.float_position {
            position.write_xml_role_in(writer, role, source_context, emission)?;
        }

        write_extras_at(writer, &self.extra_xml, 2)?;
        if let Some(overlap) = self.overlap {
            let mut e = crate::text::FieldXmlStart::word(&prefix, "tblOverlap", emission)?;
            e.push_word(&prefix, "val", overlap.to_str())?;
            e.write(writer, true)?;
        }

        write_extras_at(writer, &self.extra_xml, 3)?;
        if let Some(bidi) = self.bidi_visual {
            crate::run_properties::write_word_toggle(
                writer,
                "bidiVisual",
                bidi,
                role,
                source_context,
                emission,
            )?;
        }

        write_extras_at(writer, &self.extra_xml, 4)?;
        if let Some(size) = self.row_band_size {
            let mut buffer = itoa::Buffer::new();
            let mut e = crate::text::FieldXmlStart::word(&prefix, "tblStyleRowBandSize", emission)?;
            e.push_word(&prefix, "val", buffer.format(size))?;
            e.write(writer, true)?;
        }

        write_extras_at(writer, &self.extra_xml, 5)?;
        if let Some(size) = self.column_band_size {
            let mut buffer = itoa::Buffer::new();
            let mut e = crate::text::FieldXmlStart::word(&prefix, "tblStyleColBandSize", emission)?;
            e.push_word(&prefix, "val", buffer.format(size))?;
            e.write(writer, true)?;
        }

        write_extras_at(writer, &self.extra_xml, 6)?;
        if let Some(ref width) = self.width {
            width.write_xml_core(writer, "w:tblW", role, source_context, emission)?;
        }

        write_extras_at(writer, &self.extra_xml, 7)?;
        if let Some(jc) = self.jc {
            let mut e = crate::text::FieldXmlStart::word(&prefix, "jc", emission)?;
            e.push_word(&prefix, "val", jc.to_str())?;
            e.write(writer, true)?;
        }

        write_extras_at(writer, &self.extra_xml, 8)?;
        if let Some(ref spacing) = self.cell_spacing {
            spacing.write_xml_core(writer, "w:tblCellSpacing", role, source_context, emission)?;
        }

        write_extras_at(writer, &self.extra_xml, 9)?;
        if let Some(ref indent) = self.indent {
            indent.write_xml_core(writer, "w:tblInd", role, source_context, emission)?;
        }

        write_extras_at(writer, &self.extra_xml, 10)?;
        if let Some(ref borders) = self.borders
            && borders.writes_element()
        {
            borders.write_xml_core(writer, "w:tblBorders", role, source_context, emission)?;
        }

        write_extras_at(writer, &self.extra_xml, 11)?;
        if let Some(ref shd) = self.shading {
            if role == XmlWriteRole::Source {
                shd.write_xml_in(writer, "shd", role, source_context, emission)?;
            } else {
                shd.write_xml_in(writer, "w:shd", role, source_context, emission)?;
            }
        }

        write_extras_at(writer, &self.extra_xml, 12)?;
        if let Some(ref layout) = self.layout {
            let mut e = crate::text::FieldXmlStart::word(&prefix, "tblLayout", emission)?;
            e.push_word(&prefix, "type", layout.as_str())?;
            e.write(writer, true)?;
        }

        write_extras_at(writer, &self.extra_xml, 13)?;
        if let Some(ref cell_margin) = self.cell_margin {
            cell_margin.write_xml_in(writer, "w:tblCellMar", emission)?;
        }

        write_extras_at(writer, &self.extra_xml, 14)?;
        if let Some(ref look) = self.look {
            look.write_xml_in(writer, emission)?;
        }

        write_extras_at(writer, &self.extra_xml, 15)?;
        if let Some(ref caption) = self.caption {
            let mut e = crate::text::FieldXmlStart::word(&prefix, "tblCaption", emission)?;
            e.push_word(&prefix, "val", caption.as_str())?;
            e.write(writer, true)?;
        }

        write_extras_at(writer, &self.extra_xml, 16)?;
        if let Some(ref description) = self.description {
            let mut e = crate::text::FieldXmlStart::word(&prefix, "tblDescription", emission)?;
            e.push_word(&prefix, "val", description.as_str())?;
            e.write(writer, true)?;
        }

        write_extras_at(writer, &self.extra_xml, 17)?;
        write_extras_at(writer, &self.extra_xml, 18)?;
        for raw in &self.revision_xml {
            writer.get_mut().write_all(raw)?;
        }
        if let Some(change) = &self.change {
            change.write_xml_with_role_override(writer, None, role, source_context, emission)?;
        }

        if let Some(emission) = emission {
            emission.write_word_end(writer, &prefix, "tblPr")?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(format!("{prefix}:tblPr"))))?;
        }
        Ok(())
    }
}

fn tbl_pr_raw_boundary(name: &[u8], current: usize, word_prefixes: &[String]) -> (usize, usize) {
    let schema_children = [
        b"tblStyle".as_slice(),
        b"tblpPr".as_slice(),
        b"tblOverlap".as_slice(),
        b"bidiVisual".as_slice(),
        b"tblStyleRowBandSize".as_slice(),
        b"tblStyleColBandSize".as_slice(),
        b"tblW".as_slice(),
        b"jc".as_slice(),
        b"tblCellSpacing".as_slice(),
        b"tblInd".as_slice(),
        b"tblBorders".as_slice(),
        b"shd".as_slice(),
        b"tblLayout".as_slice(),
        b"tblCellMar".as_slice(),
        b"tblLook".as_slice(),
        b"tblCaption".as_slice(),
        b"tblDescription".as_slice(),
        b"tblPrChange".as_slice(),
    ];

    schema_children
        .iter()
        .position(|child| is_word_element(name, child, word_prefixes))
        .map_or((current, current), |index| (index, index + 1))
}

// ---- Table grid ----

/// `CT_TblGrid` — Defines the column structure of a table.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CT_TblGrid {
    /// Active columns used for current table layout.
    pub columns: Vec<CT_TblGridCol>,
    /// The preserved historical `w:tblGridChange` subtree, when present.
    ///
    /// Historical columns are retained for round-trip fidelity and never
    /// participate in current table layout.
    pub grid_change_xml: Option<Vec<u8>>,
    /// Unmodelled direct children retained verbatim.
    pub extra_xml: Vec<Vec<u8>>,
}

#[allow(non_snake_case)]
impl CT_TblGrid {
    pub fn from_xml(reader: &mut Reader<&[u8]>) -> Result<Self> {
        Self::from_xml_with_prefixes(reader, &["w".to_owned()])
    }

    pub(crate) fn from_xml_with_prefixes(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
    ) -> Result<Self> {
        let mut columns = Vec::new();
        let mut grid_change_xml = None;
        let mut extra_xml = Vec::new();
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(ref e)) => {
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(e.name().as_ref(), b"gridCol", &prefixes) {
                        columns.push(Self::parse_grid_column(e, &prefixes)?);
                    } else {
                        let raw = capture_empty_element(e)?;
                        extra_xml.push(crate::text::raw_with_external_bindings(
                            &raw,
                            &preserved_table_raw_bindings(&prefixes),
                        )?);
                    }
                }
                Ok(Event::Start(ref e)) => {
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(e.name().as_ref(), b"gridCol", &prefixes) {
                        columns.push(Self::parse_grid_column(e, &prefixes)?);
                        capture_element(reader, e)?;
                    } else if is_word_element(e.name().as_ref(), b"tblGridChange", &prefixes) {
                        if grid_change_xml.is_some() {
                            return Err(OxmlError::InvalidValue(
                                "duplicate w:tblGridChange".to_owned(),
                            ));
                        }
                        let raw = capture_element(reader, e)?;
                        grid_change_xml = Some(crate::text::raw_with_external_bindings(
                            &raw,
                            &preserved_table_raw_bindings(&prefixes),
                        )?);
                    } else {
                        let raw = capture_element(reader, e)?;
                        extra_xml.push(crate::text::raw_with_external_bindings(
                            &raw,
                            &preserved_table_raw_bindings(&prefixes),
                        )?);
                    }
                }
                Ok(Event::End(ref e))
                    if is_word_element(e.name().as_ref(), b"tblGrid", word_prefixes) =>
                {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        Ok(CT_TblGrid {
            columns,
            grid_change_xml,
            extra_xml,
        })
    }

    fn parse_grid_column(
        element: &BytesStart<'_>,
        word_prefixes: &[String],
    ) -> Result<CT_TblGridCol> {
        let mut width = Twips(0);
        for attr in element.attributes() {
            let attr = attr?;
            if is_word_attribute(attr.key.as_ref(), b"w", word_prefixes) {
                let value = std::str::from_utf8(&attr.value)?;
                width = Twips(parse_integer_measurement(
                    element.name().as_ref(),
                    attr.key.as_ref(),
                    value,
                )?);
            }
        }
        Ok(CT_TblGridCol { width })
    }

    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.write_xml_role(writer, XmlWriteRole::Publication, None)
    }

    pub(crate) fn write_xml_role<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<()> {
        self.write_xml_role_in(writer, role, source_context, None)
    }

    pub(crate) fn write_xml_role_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let mut start = crate::text::FieldXmlStart::new("w:tblGrid", emission)?;
        let prefix = if role == XmlWriteRole::Source {
            start.qualify(&[], role, source_context)?
        } else {
            "w".to_owned()
        };

        let mut buf = itoa::Buffer::new();
        start.write(writer, false)?;

        for col in &self.columns {
            let mut e = crate::text::FieldXmlStart::word(&prefix, "gridCol", emission)?;
            e.push_word(&prefix, "w", buf.format(col.width.0))?;
            e.write(writer, true)?;
        }

        for raw in &self.extra_xml {
            writer.get_mut().write_all(raw)?;
        }
        if let Some(raw) = &self.grid_change_xml {
            writer.get_mut().write_all(raw)?;
        }

        if let Some(emission) = emission {
            emission.write_word_end(writer, &prefix, "tblGrid")?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(format!("{prefix}:tblGrid"))))?;
        }
        Ok(())
    }
}

// ---- Row properties ----

/// Vertical merge state for a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VMerge {
    /// Start of a vertical merge group
    Restart,
    /// Continuation of the merge group above
    Continue,
}

/// `CT_TrPr` — Table row properties.
///
/// Two values are equal when they carry the same properties and both, or
/// neither, write an empty element.
#[derive(Debug, Clone, Default)]
pub struct CT_TrPr {
    /// Row height in twips
    pub height: Option<Twips>,
    /// Row height rule: "exact" or "atLeast"
    pub height_rule: Option<String>,
    /// Repeat as header row on each page
    pub header: Option<bool>,
    /// Row alignment
    pub jc: Option<ST_Jc>,
    /// Number of grid columns omitted before the first cell.
    pub grid_before: Option<u32>,
    /// Number of grid columns omitted after the final cell.
    pub grid_after: Option<u32>,
    /// `w:wBefore` — the width of the omitted leading grid columns.
    pub width_before: Option<CT_TblWidth>,
    /// `w:wAfter` — the width of the omitted trailing grid columns.
    pub width_after: Option<CT_TblWidth>,
    /// `w:tblCellSpacing` — this row's gap between cell content boxes.
    pub cell_spacing: Option<CT_TblWidth>,
    /// `w:hidden` — the row is authored but not displayed.
    pub hidden: Option<bool>,
    /// Allow row to break across pages
    pub cant_split: Option<bool>,
    /// `w:cnfStyle` — which conditional parts of the table style this row is.
    ///
    /// Word writes this alongside `w:tblLook` and needs both to reproduce a
    /// styled table. Dropping it loses the header-row and banding emphasis.
    pub cnf_style: Option<String>,
    /// A `w:cnfStyle` read in its boolean-attribute form: the value it reads
    /// as and the element as written, written back while `cnf_style` keeps
    /// that value.
    #[doc(hidden)]
    pub cnf_style_xml: Option<(String, Vec<u8>)>,
    /// Contextual row insertion and deletion markers in schema order.
    pub revision_markers: Vec<CT_Revision>,
    /// Malformed row markers retained verbatim.
    pub revision_xml: Vec<Vec<u8>>,
    /// Schema slots for malformed row markers, parallel to `revision_xml`.
    #[doc(hidden)]
    pub revision_xml_positions: Vec<usize>,
    /// Other row properties retained at their schema insertion slots.
    pub extra_xml: Vec<(usize, Vec<u8>)>,
    /// Whether `extra_xml` holds a Word `w:moveFrom` marker with an id and an
    /// author, which `Document::accept_all` removes the row for.
    #[doc(hidden)]
    pub moved_away: bool,
    /// Whether this element was read from a part. A read one is written back
    /// even when it carries nothing; one built through the API is written
    /// only when it carries something.
    #[doc(hidden)]
    pub from_source: bool,
}

impl PartialEq for CT_TrPr {
    fn eq(&self, other: &Self) -> bool {
        let Self {
            height,
            height_rule,
            header,
            jc,
            grid_before,
            grid_after,
            width_before,
            width_after,
            cell_spacing,
            hidden,
            cant_split,
            cnf_style,
            cnf_style_xml: _,
            revision_markers,
            revision_xml,
            revision_xml_positions,
            extra_xml,
            moved_away,
            from_source: _,
        } = self;
        *height == other.height
            && *height_rule == other.height_rule
            && *header == other.header
            && *jc == other.jc
            && *grid_before == other.grid_before
            && *grid_after == other.grid_after
            && *width_before == other.width_before
            && *width_after == other.width_after
            && *cell_spacing == other.cell_spacing
            && *hidden == other.hidden
            && *cant_split == other.cant_split
            && *cnf_style == other.cnf_style
            && *revision_markers == other.revision_markers
            && *revision_xml == other.revision_xml
            && *revision_xml_positions == other.revision_xml_positions
            && *extra_xml == other.extra_xml
            && *moved_away == other.moved_away
            && self.writes_empty_element() == other.writes_empty_element()
    }
}

#[allow(non_snake_case)]
impl CT_TrPr {
    /// A `<w:trPr/>` read from a part, written back as it was.
    pub(crate) fn empty_from_source() -> Self {
        Self {
            from_source: true,
            ..Self::default()
        }
    }

    pub fn from_xml(reader: &mut Reader<&[u8]>) -> Result<Self> {
        Self::from_xml_with_prefixes_and_owner_bindings(reader, &["w".to_owned()], &[])
    }

    pub(crate) fn from_xml_with_prefixes(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
    ) -> Result<Self> {
        Self::from_xml_with_prefixes_and_owner_bindings(reader, word_prefixes, &[])
    }

    fn from_xml_with_prefixes_and_owner_bindings(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
    ) -> Result<Self> {
        let mut pr = CT_TrPr::empty_from_source();
        let mut boundary = 0usize;
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    let (at, next) = tr_pr_raw_boundary(name.as_ref(), boundary, &prefixes);
                    // A child without the value it is modelled by stays
                    // verbatim in the raw branch below.
                    if is_word_element(name.as_ref(), b"trHeight", &prefixes)
                        && read_row_height(e, &prefixes, &mut pr)?
                    {
                        // Modelled by `read_row_height`.
                    } else if is_word_element(name.as_ref(), b"tblHeader", &prefixes) {
                        pr.header = Some(parse_word_toggle(e, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"jc", &prefixes)
                        && let Some(jc) = get_word_val_attr(e, &prefixes)?
                            .and_then(|val| ST_Jc::from_str(&val).ok())
                    {
                        pr.jc = Some(jc);
                    } else if is_word_element(name.as_ref(), b"gridBefore", &prefixes)
                        && let Some(val) = get_word_val_attr(e, &prefixes)?
                    {
                        pr.grid_before = Some(TableScalar::Unsigned.unsigned(&val)?);
                    } else if is_word_element(name.as_ref(), b"gridAfter", &prefixes)
                        && let Some(val) = get_word_val_attr(e, &prefixes)?
                    {
                        pr.grid_after = Some(TableScalar::Unsigned.unsigned(&val)?);
                    } else if is_word_element(name.as_ref(), b"wBefore", &prefixes) {
                        pr.width_before =
                            Some(CT_TblWidth::from_xml_attrs_with_prefixes(e, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"wAfter", &prefixes) {
                        pr.width_after =
                            Some(CT_TblWidth::from_xml_attrs_with_prefixes(e, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"tblCellSpacing", &prefixes) {
                        pr.cell_spacing =
                            Some(CT_TblWidth::from_xml_attrs_with_prefixes(e, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"hidden", &prefixes) {
                        pr.hidden = Some(parse_word_toggle(e, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"cnfStyle", &prefixes)
                        && let Some(val) = get_word_val_attr(e, &prefixes)?
                    {
                        pr.cnf_style = Some(val);
                    } else if is_word_element(name.as_ref(), b"cnfStyle", &prefixes)
                        && let Some(val) = cnf_style_from_booleans(e, &prefixes)?
                    {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_empty_element(e)?,
                            owner_bindings,
                        )?;
                        pr.cnf_style_xml = Some((val.clone(), raw));
                        pr.cnf_style = Some(val);
                    } else if is_word_element(name.as_ref(), b"cantSplit", &prefixes) {
                        pr.cant_split = Some(parse_word_toggle(e, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"ins", &prefixes)
                        || is_word_element(name.as_ref(), b"del", &prefixes)
                    {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_empty_element(e)?,
                            owner_bindings,
                        )?;
                        if let Some(revision) = CT_Revision::from_raw(raw.clone(), &prefixes) {
                            pr.revision_markers.push(revision);
                        } else {
                            pr.revision_xml.push(raw);
                            pr.revision_xml_positions.push(at);
                        }
                    } else {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_empty_element(e)?,
                            owner_bindings,
                        )?;
                        pr.moved_away |= is_word_element(name.as_ref(), b"moveFrom", &prefixes)
                            && CT_Revision::from_raw(raw.clone(), &prefixes).is_some();
                        pr.extra_xml.push((at, raw));
                    }
                    boundary = next;
                }
                Ok(Event::Start(ref e)) => {
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    let (at, next) = tr_pr_raw_boundary(e.name().as_ref(), boundary, &prefixes);
                    if is_word_element(e.name().as_ref(), b"tblHeader", &prefixes) {
                        pr.header = Some(parse_word_toggle(e, &prefixes)?);
                        reader.read_to_end_into(e.name(), &mut Vec::new())?;
                    } else if is_word_element(e.name().as_ref(), b"cantSplit", &prefixes) {
                        pr.cant_split = Some(parse_word_toggle(e, &prefixes)?);
                        reader.read_to_end_into(e.name(), &mut Vec::new())?;
                    } else if is_word_element(e.name().as_ref(), b"hidden", &prefixes) {
                        pr.hidden = Some(parse_word_toggle(e, &prefixes)?);
                        reader.read_to_end_into(e.name(), &mut Vec::new())?;
                    } else if is_word_element(e.name().as_ref(), b"ins", &prefixes)
                        || is_word_element(e.name().as_ref(), b"del", &prefixes)
                    {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            owner_bindings,
                        )?;
                        if let Some(revision) = CT_Revision::from_raw(raw.clone(), &prefixes) {
                            pr.revision_markers.push(revision);
                        } else {
                            pr.revision_xml.push(raw);
                            pr.revision_xml_positions.push(at);
                        }
                    } else {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            owner_bindings,
                        )?;
                        pr.moved_away |= is_word_element(e.name().as_ref(), b"moveFrom", &prefixes)
                            && CT_Revision::from_raw(raw.clone(), &prefixes).is_some();
                        pr.extra_xml.push((at, raw));
                    }
                    boundary = next;
                }
                Ok(Event::End(ref e)) if matches_local_name(e.name().as_ref(), b"trPr") => {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        Ok(pr)
    }

    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.write_xml_role(writer, XmlWriteRole::Publication, None)
    }

    pub(crate) fn write_xml_role<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<()> {
        self.write_xml_role_in(writer, role, source_context, None)
    }

    pub(crate) fn write_xml_role_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let _source_scope =
            (role == XmlWriteRole::Source).then(|| crate::text::root_binding_scope(0));
        let empty = self.is_empty();
        if empty && !self.from_source {
            return Ok(());
        }

        let mut root = crate::text::FieldXmlStart::new("w:trPr", emission)?;
        let prefix = if role == XmlWriteRole::Source {
            root.qualify(&[], role, source_context)?
        } else {
            "w".to_owned()
        };
        if empty {
            return root.write(writer, true);
        }
        root.write(writer, false)?;

        // cnfStyle comes first in the schema sequence for both trPr and tcPr.
        write_extras_at(writer, &self.extra_xml, 0)?;
        if let Some(ref cnf) = self.cnf_style {
            if let Some((read_as, raw)) = &self.cnf_style_xml
                && read_as == cnf
            {
                writer.get_mut().write_all(raw)?;
            } else {
                let mut e = crate::text::FieldXmlStart::word(&prefix, "cnfStyle", emission)?;
                e.push_word(&prefix, "val", cnf.as_str())?;
                e.write(writer, true)?;
            }
        }

        write_extras_at(writer, &self.extra_xml, 1)?;
        write_extras_at(writer, &self.extra_xml, 2)?;
        if let Some(grid_before) = self.grid_before {
            let mut buffer = itoa::Buffer::new();
            let mut e = crate::text::FieldXmlStart::word(&prefix, "gridBefore", emission)?;
            e.push_word(&prefix, "val", buffer.format(grid_before))?;
            e.write(writer, true)?;
        }

        write_extras_at(writer, &self.extra_xml, 3)?;
        if let Some(grid_after) = self.grid_after {
            let mut buffer = itoa::Buffer::new();
            let mut e = crate::text::FieldXmlStart::word(&prefix, "gridAfter", emission)?;
            e.push_word(&prefix, "val", buffer.format(grid_after))?;
            e.write(writer, true)?;
        }

        write_extras_at(writer, &self.extra_xml, 4)?;
        if let Some(ref width) = self.width_before {
            width.write_xml_core(writer, "w:wBefore", role, source_context, emission)?;
        }

        write_extras_at(writer, &self.extra_xml, 5)?;
        if let Some(ref width) = self.width_after {
            width.write_xml_core(writer, "w:wAfter", role, source_context, emission)?;
        }

        write_extras_at(writer, &self.extra_xml, 6)?;
        if let Some(cant_split) = self.cant_split {
            crate::run_properties::write_word_toggle(
                writer,
                "cantSplit",
                cant_split,
                role,
                source_context,
                emission,
            )?;
        }

        write_extras_at(writer, &self.extra_xml, 7)?;
        // A rule without a height is written as read: the value is optional.
        if self.height.is_some() || self.height_rule.is_some() {
            let mut buf = itoa::Buffer::new();
            let mut e = crate::text::FieldXmlStart::word(&prefix, "trHeight", emission)?;
            if let Some(height) = self.height {
                e.push_word(&prefix, "val", buf.format(height.0))?;
            }
            if let Some(ref rule) = self.height_rule {
                e.push_word(&prefix, "hRule", rule.as_str())?;
            }
            e.write(writer, true)?;
        }

        write_extras_at(writer, &self.extra_xml, 8)?;
        if let Some(header) = self.header {
            crate::run_properties::write_word_toggle(
                writer,
                "tblHeader",
                header,
                role,
                source_context,
                emission,
            )?;
        }

        write_extras_at(writer, &self.extra_xml, 9)?;
        if let Some(ref spacing) = self.cell_spacing {
            spacing.write_xml_core(writer, "w:tblCellSpacing", role, source_context, emission)?;
        }

        write_extras_at(writer, &self.extra_xml, 10)?;
        if let Some(jc) = self.jc {
            let mut e = crate::text::FieldXmlStart::word(&prefix, "jc", emission)?;
            e.push_word(&prefix, "val", jc.to_str())?;
            e.write(writer, true)?;
        }

        write_extras_at(writer, &self.extra_xml, 11)?;
        if let Some(hidden) = self.hidden {
            crate::run_properties::write_word_toggle(
                writer,
                "hidden",
                hidden,
                role,
                source_context,
                emission,
            )?;
        }

        write_extras_at(writer, &self.extra_xml, 12)?;
        self.write_revision_xml_at(writer, 12)?;
        for revision in self
            .revision_markers
            .iter()
            .filter(|revision| revision.kind() == RevisionKind::Insertion)
        {
            revision.write_xml_with_role_override(writer, None, role, source_context, emission)?;
        }
        write_extras_at(writer, &self.extra_xml, 13)?;
        self.write_revision_xml_at(writer, 13)?;
        for revision in self
            .revision_markers
            .iter()
            .filter(|revision| revision.kind() == RevisionKind::Deletion)
        {
            revision.write_xml_with_role_override(writer, None, role, source_context, emission)?;
        }
        if self.revision_xml_positions.len() != self.revision_xml.len() {
            for raw in &self.revision_xml {
                writer.get_mut().write_all(raw)?;
            }
        }
        for revision in self.revision_markers.iter().filter(|revision| {
            !matches!(
                revision.kind(),
                RevisionKind::Insertion | RevisionKind::Deletion
            )
        }) {
            revision.write_xml_with_role_override(writer, None, role, source_context, emission)?;
        }
        write_extras_at(writer, &self.extra_xml, 14)?;
        write_extras_at(writer, &self.extra_xml, 15)?;

        if let Some(emission) = emission {
            emission.write_word_end(writer, &prefix, "trPr")?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(format!("{prefix}:trPr"))))?;
        }
        Ok(())
    }

    /// Whether a retained child holds XML a row edit cannot copy. A
    /// `w:trHeight` without attributes means the default height: it holds
    /// nothing to copy.
    #[doc(hidden)]
    pub fn has_opaque_xml(&self) -> bool {
        self.extra_xml
            .iter()
            .any(|(_, raw)| !is_bare_row_height(raw))
    }

    /// Drop a retained `w:trHeight` without attributes, which a height set
    /// through the API replaces.
    #[doc(hidden)]
    pub fn remove_bare_height(&mut self) {
        self.extra_xml.retain(|(_, raw)| !is_bare_row_height(raw));
    }

    fn is_empty(&self) -> bool {
        self.height.is_none()
            && self.height_rule.is_none()
            && self.header.is_none()
            && self.jc.is_none()
            && self.cant_split.is_none()
            && self.cnf_style.is_none()
            && self.grid_before.is_none()
            && self.grid_after.is_none()
            && self.width_before.is_none()
            && self.width_after.is_none()
            && self.cell_spacing.is_none()
            && self.hidden.is_none()
            && self.revision_markers.is_empty()
            && self.revision_xml.is_empty()
            && self.revision_xml_positions.is_empty()
            && self.extra_xml.is_empty()
    }

    fn writes_empty_element(&self) -> bool {
        self.from_source && self.is_empty()
    }

    fn write_revision_xml_at<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        position: usize,
    ) -> Result<()> {
        if self.revision_xml_positions.len() == self.revision_xml.len() {
            for (raw, _) in self
                .revision_xml
                .iter()
                .zip(&self.revision_xml_positions)
                .filter(|(_, at)| **at == position)
            {
                writer.get_mut().write_all(raw)?;
            }
        }
        Ok(())
    }
}

/// Read a self-closing `w:trHeight` into the row model. Its value is
/// optional (a missing one means 0), so a rule alone is modelled too. One
/// without either attribute, or with any other attribute, returns false and
/// stays verbatim.
fn read_row_height(e: &BytesStart<'_>, prefixes: &[String], pr: &mut CT_TrPr) -> Result<bool> {
    let mut height = None;
    let mut rule = None;
    let mut foreign = false;
    for attr in e.attributes() {
        let attr = attr?;
        let key = attr.key.as_ref();
        let val = std::str::from_utf8(&attr.value)?;
        if is_word_attribute(key, b"val", prefixes) {
            height = Some(Twips(parse_integer_measurement(
                e.name().as_ref(),
                key,
                val,
            )?));
        } else if is_word_attribute(key, b"hRule", prefixes) {
            rule = Some(val.to_string());
        } else if key != b"xmlns" && !key.starts_with(b"xmlns:") {
            foreign = true;
        }
    }
    if foreign || (height.is_none() && rule.is_none()) {
        return Ok(false);
    }
    pr.height = height;
    pr.height_rule = rule;
    Ok(true)
}

/// Whether retained row XML is a Word `w:trHeight` element without
/// attributes or content. Retained XML keeps the prefixes of its part, so a
/// `w` prefix it does not bind itself is the part's Word prefix.
fn is_bare_row_height(raw: &[u8]) -> bool {
    let mut reader = quick_xml::NsReader::from_reader(raw);
    let Ok((namespace, Event::Empty(root))) = reader.read_resolved_event() else {
        return false;
    };
    let word = match namespace {
        quick_xml::name::ResolveResult::Bound(quick_xml::name::Namespace(ns)) => {
            ns == crate::namespace::W_NS.as_bytes()
        }
        quick_xml::name::ResolveResult::Unbound => false,
        quick_xml::name::ResolveResult::Unknown(prefix) => prefix == b"w",
    };
    word && root.local_name().as_ref() == b"trHeight"
        && carries_only_namespace_declarations(&root).unwrap_or(false)
        && matches!(reader.read_event(), Ok(Event::Eof))
}

/// The Transitional boolean attributes of `w:cnfStyle`, at their positions
/// in its twelve-character `w:val`.
const CNF_STYLE_BOOLEANS: [&[u8]; 12] = [
    b"firstRow",
    b"lastRow",
    b"firstColumn",
    b"lastColumn",
    b"oddVBand",
    b"evenVBand",
    b"oddHBand",
    b"evenHBand",
    b"firstRowLastColumn",
    b"firstRowFirstColumn",
    b"lastRowLastColumn",
    b"lastRowFirstColumn",
];

/// The `w:val` a `w:cnfStyle` written with its boolean attributes only reads
/// as. None when it has a `w:val`, no boolean, or any other attribute or
/// value, so that such an element stays verbatim.
fn cnf_style_from_booleans(e: &BytesStart<'_>, prefixes: &[String]) -> Result<Option<String>> {
    let mut bits = [b'0'; 12];
    let mut any = false;
    for attribute in e.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        if key == b"xmlns" || key.starts_with(b"xmlns:") {
            continue;
        }
        let Some(index) = CNF_STYLE_BOOLEANS
            .iter()
            .position(|local| is_word_attribute(key, local, prefixes))
        else {
            return Ok(None);
        };
        let Some(on) = parse_ooxml_bool(std::str::from_utf8(&attribute.value)?) else {
            return Ok(None);
        };
        bits[index] = if on { b'1' } else { b'0' };
        any = true;
    }
    Ok(any.then(|| String::from_utf8_lossy(&bits).into_owned()))
}

/// Whether a self-closing container carries nothing but namespace
/// declarations, so it reads as the empty modelled container.
fn carries_only_namespace_declarations(e: &BytesStart<'_>) -> Result<bool> {
    for attribute in e.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        if key != b"xmlns" && !key.starts_with(b"xmlns:") {
            return Ok(false);
        }
    }
    Ok(true)
}

fn tr_pr_raw_boundary(name: &[u8], current: usize, word_prefixes: &[String]) -> (usize, usize) {
    let schema_children = [
        b"cnfStyle".as_slice(),
        b"divId".as_slice(),
        b"gridBefore".as_slice(),
        b"gridAfter".as_slice(),
        b"wBefore".as_slice(),
        b"wAfter".as_slice(),
        b"cantSplit".as_slice(),
        b"trHeight".as_slice(),
        b"tblHeader".as_slice(),
        b"tblCellSpacing".as_slice(),
        b"jc".as_slice(),
        b"hidden".as_slice(),
        b"ins".as_slice(),
        b"del".as_slice(),
        b"trPrChange".as_slice(),
    ];

    schema_children
        .iter()
        .position(|child| is_word_element(name, child, word_prefixes))
        .map_or((current, current), |index| (index, index + 1))
}

// ---- Cell properties ----

/// `ST_VerticalJc` -- vertical alignment within a cell or a section page.
///
/// `#[non_exhaustive]` so a later ECMA value is an addition rather than a
/// second breaking change for every downstream match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ST_VerticalJc {
    Top,
    Center,
    Bottom,
    /// `both`, ECMA's vertical distribution. Laid out as `Top` for now.
    Both,
}

impl ST_VerticalJc {
    pub fn from_str(s: &str) -> Self {
        match s {
            "center" => Self::Center,
            "bottom" => Self::Bottom,
            "both" => Self::Both,
            _ => Self::Top,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Center => "center",
            Self::Bottom => "bottom",
            Self::Both => "both",
        }
    }
}

/// `CT_TcPr` — Table cell properties.
///
/// Two values are equal when they carry the same properties and both, or
/// neither, write an empty element.
#[derive(Debug, Clone, Default)]
pub struct CT_TcPr {
    /// Cell width
    pub width: Option<CT_TblWidth>,
    /// Horizontal merge (number of grid columns spanned)
    pub grid_span: Option<u32>,
    /// Legacy horizontal merge state.
    pub h_merge: Option<String>,
    /// Vertical merge
    pub v_merge: Option<VMerge>,
    /// Cell borders
    pub borders: Option<CT_TblBorders>,
    /// Cell shading
    pub shading: Option<CT_Shd>,
    /// Vertical alignment
    pub v_align: Option<ST_VerticalJc>,
    /// No-wrap text
    pub no_wrap: Option<bool>,
    /// Per-cell margins.
    pub cell_margin: Option<CT_TblCellMar>,
    /// Text direction
    pub text_direction: Option<String>,
    /// `w:cnfStyle` — which conditional parts of the table style this cell is.
    pub cnf_style: Option<String>,
    /// A `w:cnfStyle` read in its boolean-attribute form: the value it reads
    /// as and the element as written, written back while `cnf_style` keeps
    /// that value.
    #[doc(hidden)]
    pub cnf_style_xml: Option<(String, Vec<u8>)>,
    /// Unmodelled property children retained at their schema insertion slots.
    #[doc(hidden)]
    pub extra_xml: Vec<(usize, Vec<u8>)>,
    /// Whether this element was read from a part. A read one is written back
    /// even when it carries nothing, as the `<w:tcPr/>` Google Docs writes in
    /// every cell; one built through the API is written only when it carries
    /// something.
    #[doc(hidden)]
    pub from_source: bool,
}

impl PartialEq for CT_TcPr {
    fn eq(&self, other: &Self) -> bool {
        let Self {
            width,
            grid_span,
            h_merge,
            v_merge,
            borders,
            shading,
            v_align,
            no_wrap,
            cell_margin,
            text_direction,
            cnf_style,
            cnf_style_xml: _,
            extra_xml,
            from_source: _,
        } = self;
        *width == other.width
            && *grid_span == other.grid_span
            && *h_merge == other.h_merge
            && *v_merge == other.v_merge
            && *borders == other.borders
            && *shading == other.shading
            && *v_align == other.v_align
            && *no_wrap == other.no_wrap
            && *cell_margin == other.cell_margin
            && *text_direction == other.text_direction
            && *cnf_style == other.cnf_style
            && *extra_xml == other.extra_xml
            && self.writes_empty_element() == other.writes_empty_element()
    }
}

#[allow(non_snake_case)]
impl CT_TcPr {
    /// A `<w:tcPr/>` read from a part, written back as it was.
    pub(crate) fn empty_from_source() -> Self {
        Self {
            from_source: true,
            ..Self::default()
        }
    }

    pub fn from_xml(reader: &mut Reader<&[u8]>) -> Result<Self> {
        Self::from_xml_with_prefixes_and_owner_bindings(reader, &["w".to_owned()], &[])
    }

    pub(crate) fn from_xml_with_prefixes(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
    ) -> Result<Self> {
        Self::from_xml_with_prefixes_and_owner_bindings(reader, word_prefixes, &[])
    }

    fn from_xml_with_prefixes_and_owner_bindings(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
    ) -> Result<Self> {
        let mut pr = CT_TcPr::empty_from_source();
        let mut boundary = 0;
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(ref e)) => {
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    let (at, next) = tc_pr_raw_boundary(e.name().as_ref(), boundary, &prefixes);
                    if Self::parse_property_element(e, &mut pr, &prefixes)? {
                        boundary = next;
                    } else if is_word_element(e.name().as_ref(), b"cnfStyle", &prefixes)
                        && let Some(val) = cnf_style_from_booleans(e, &prefixes)?
                    {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_empty_element(e)?,
                            owner_bindings,
                        )?;
                        pr.cnf_style_xml = Some((val.clone(), raw));
                        pr.cnf_style = Some(val);
                        boundary = next;
                    } else {
                        pr.extra_xml.push((
                            at,
                            crate::text::raw_with_external_bindings(
                                &capture_empty_element(e)?,
                                owner_bindings,
                            )?,
                        ));
                        boundary = next;
                    }
                }
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    let (at, next) = tc_pr_raw_boundary(name.as_ref(), boundary, &prefixes);
                    if is_word_element(name.as_ref(), b"tcBorders", &prefixes) {
                        let local_bindings = local_namespace_overrides(e, word_prefixes)?;
                        let border_bindings =
                            merged_owner_bindings(owner_bindings, &local_bindings);
                        pr.borders =
                            Some(CT_TblBorders::from_xml_with_prefixes_and_owner_bindings(
                                reader,
                                &prefixes,
                                &border_bindings,
                            )?);
                        boundary = next;
                    } else if is_word_element(name.as_ref(), b"tcMar", &prefixes) {
                        pr.cell_margin =
                            Some(CT_TblCellMar::from_xml_with_prefixes(reader, &prefixes)?);
                        boundary = next;
                    } else if Self::parse_property_element(e, &mut pr, &prefixes)? {
                        reader.read_to_end_into(name, &mut Vec::new())?;
                        boundary = next;
                    } else {
                        pr.extra_xml.push((
                            at,
                            crate::text::raw_with_external_bindings(
                                &capture_element(reader, e)?,
                                owner_bindings,
                            )?,
                        ));
                        boundary = next;
                    }
                }
                Ok(Event::End(ref e)) if matches_local_name(e.name().as_ref(), b"tcPr") => {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        Ok(pr)
    }

    fn parse_property_element(
        e: &BytesStart<'_>,
        pr: &mut Self,
        word_prefixes: &[String],
    ) -> Result<bool> {
        let name = e.name();
        if is_word_element(name.as_ref(), b"tcW", word_prefixes) {
            pr.width = Some(CT_TblWidth::from_xml_attrs_with_prefixes(e, word_prefixes)?);
        } else if is_word_element(name.as_ref(), b"gridSpan", word_prefixes)
            && let Some(val) = get_word_val_attr(e, word_prefixes)?
        {
            let span = TableScalar::PositiveSpan.unsigned(&val)?;
            pr.grid_span = Some(span);
        } else if is_word_element(name.as_ref(), b"hMerge", word_prefixes) {
            pr.h_merge = Some(get_word_val_attr(e, word_prefixes)?.unwrap_or_default());
        } else if is_word_element(name.as_ref(), b"vMerge", word_prefixes) {
            pr.v_merge = Some(TableScalar::merge(
                get_word_val_attr(e, word_prefixes)?.as_deref(),
            )?);
        } else if is_word_element(name.as_ref(), b"vAlign", word_prefixes)
            && let Some(val) = get_word_val_attr(e, word_prefixes)?
        {
            pr.v_align = Some(ST_VerticalJc::from_str(&val));
        } else if is_word_element(name.as_ref(), b"shd", word_prefixes) {
            pr.shading = Some(CT_Shd::from_xml_attrs_with_prefixes(e, word_prefixes)?);
        } else if is_word_element(name.as_ref(), b"cnfStyle", word_prefixes)
            && let Some(val) = get_word_val_attr(e, word_prefixes)?
        {
            pr.cnf_style = Some(val);
        } else if is_word_element(name.as_ref(), b"noWrap", word_prefixes) {
            pr.no_wrap = Some(parse_word_toggle(e, word_prefixes)?);
        } else if is_word_element(name.as_ref(), b"tcMar", word_prefixes) {
            // This leaf path handles self-closing containers. Keep genuine
            // unknown attributes raw rather than hiding them behind a default.
            if !carries_only_namespace_declarations(e)? {
                return Ok(false);
            }
            pr.cell_margin = Some(CT_TblCellMar::default());
        } else if is_word_element(name.as_ref(), b"tcBorders", word_prefixes) {
            if !carries_only_namespace_declarations(e)? {
                return Ok(false);
            }
            pr.borders = Some(CT_TblBorders::empty_from_source());
        } else if is_word_element(name.as_ref(), b"textDirection", word_prefixes)
            && let Some(val) = get_word_val_attr(e, word_prefixes)?
        {
            pr.text_direction = Some(val);
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.write_xml_role(writer, XmlWriteRole::Publication, None)
    }

    pub(crate) fn write_xml_role<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<()> {
        self.write_xml_role_in(writer, role, source_context, None)
    }

    pub(crate) fn write_xml_role_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        if self.is_empty() {
            return Ok(());
        }
        let empty = self.carries_nothing();

        let mut start = crate::text::FieldXmlStart::new("w:tcPr", emission)?;
        let prefix = if role == XmlWriteRole::Source {
            start.qualify(&[], role, source_context)?
        } else {
            "w".to_owned()
        };
        if empty {
            return start.write(writer, true);
        }

        start.write(writer, false)?;

        write_extras_at(writer, &self.extra_xml, 0)?;
        if let Some(ref cnf) = self.cnf_style {
            if let Some((read_as, raw)) = &self.cnf_style_xml
                && read_as == cnf
            {
                writer.get_mut().write_all(raw)?;
            } else {
                let mut e = crate::text::FieldXmlStart::word(&prefix, "cnfStyle", emission)?;
                e.push_word(&prefix, "val", cnf.as_str())?;
                e.write(writer, true)?;
            }
        }

        write_extras_at(writer, &self.extra_xml, 1)?;
        if let Some(ref width) = self.width {
            width.write_xml_core(writer, "w:tcW", role, source_context, emission)?;
        }

        write_extras_at(writer, &self.extra_xml, 2)?;
        if let Some(grid_span) = self.grid_span
            && grid_span > 1
        {
            let mut buf = itoa::Buffer::new();
            let mut e = crate::text::FieldXmlStart::word(&prefix, "gridSpan", emission)?;
            e.push_word(&prefix, "val", buf.format(grid_span))?;
            e.write(writer, true)?;
        }

        // Unmodelled w:hMerge is preserved at boundary 3.
        write_extras_at(writer, &self.extra_xml, 3)?;
        if let Some(h_merge) = &self.h_merge {
            let mut e = crate::text::FieldXmlStart::word(&prefix, "hMerge", emission)?;
            if !h_merge.is_empty() {
                e.push_word(&prefix, "val", h_merge.as_str())?;
            }
            e.write(writer, true)?;
        }
        write_extras_at(writer, &self.extra_xml, 4)?;
        if let Some(ref vm) = self.v_merge {
            let mut e = crate::text::FieldXmlStart::word(&prefix, "vMerge", emission)?;
            match vm {
                VMerge::Restart => e.push_word(&prefix, "val", "restart")?,
                VMerge::Continue => {} // empty element
            }
            e.write(writer, true)?;
        }

        write_extras_at(writer, &self.extra_xml, 5)?;
        if let Some(ref borders) = self.borders
            && borders.writes_element()
        {
            borders.write_xml_core(writer, "w:tcBorders", role, source_context, emission)?;
        }

        write_extras_at(writer, &self.extra_xml, 6)?;
        if let Some(ref shd) = self.shading {
            if role == XmlWriteRole::Source {
                shd.write_xml_in(writer, "shd", role, source_context, emission)?;
            } else {
                shd.write_xml_in(writer, "w:shd", role, source_context, emission)?;
            }
        }

        write_extras_at(writer, &self.extra_xml, 7)?;
        if let Some(no_wrap) = self.no_wrap {
            crate::run_properties::write_word_toggle(
                writer,
                "noWrap",
                no_wrap,
                role,
                source_context,
                emission,
            )?;
        }

        write_extras_at(writer, &self.extra_xml, 8)?;
        if let Some(ref cell_margin) = self.cell_margin {
            cell_margin.write_xml_in(writer, "w:tcMar", emission)?;
        }
        write_extras_at(writer, &self.extra_xml, 9)?;
        if let Some(ref td) = self.text_direction {
            let mut e = crate::text::FieldXmlStart::word(&prefix, "textDirection", emission)?;
            e.push_word(&prefix, "val", td.as_str())?;
            e.write(writer, true)?;
        }

        // Unmodelled w:tcFitText is preserved at boundary 10.
        write_extras_at(writer, &self.extra_xml, 10)?;
        write_extras_at(writer, &self.extra_xml, 11)?;
        if let Some(ref va) = self.v_align {
            let mut e = crate::text::FieldXmlStart::word(&prefix, "vAlign", emission)?;
            e.push_word(&prefix, "val", va.to_str())?;
            e.write(writer, true)?;
        }

        // The remaining standard Word children occupy boundaries 12 to 17.
        for boundary in 12..=18 {
            write_extras_at(writer, &self.extra_xml, boundary)?;
        }

        if let Some(emission) = emission {
            emission.write_word_end(writer, &prefix, "tcPr")?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(format!("{prefix}:tcPr"))))?;
        }
        Ok(())
    }

    /// Whether these properties write nothing: an empty `w:tcPr` built through
    /// the API is left out, while one read from a part is written back.
    pub fn is_empty(&self) -> bool {
        self.carries_nothing() && !self.from_source
    }

    /// Whether no property is set, whether or not the element is written.
    fn carries_nothing(&self) -> bool {
        self.width.is_none()
            && self.grid_span.is_none()
            && self.h_merge.is_none()
            && self.v_merge.is_none()
            && self.borders.is_none()
            && self.shading.is_none()
            && self.v_align.is_none()
            && self.no_wrap.is_none()
            && self.cell_margin.is_none()
            && self.text_direction.is_none()
            && self.cnf_style.is_none()
            && self.extra_xml.is_empty()
    }

    fn writes_empty_element(&self) -> bool {
        self.from_source && self.carries_nothing()
    }
}

fn tc_pr_raw_boundary(name: &[u8], current: usize, word_prefixes: &[String]) -> (usize, usize) {
    if is_word_element(name, b"cnfStyle", word_prefixes) {
        (0, 1)
    } else if is_word_element(name, b"tcW", word_prefixes) {
        (1, 2)
    } else if is_word_element(name, b"gridSpan", word_prefixes) {
        (2, 3)
    } else if is_word_element(name, b"hMerge", word_prefixes) {
        (3, 4)
    } else if is_word_element(name, b"vMerge", word_prefixes) {
        (4, 5)
    } else if is_word_element(name, b"tcBorders", word_prefixes) {
        (5, 6)
    } else if is_word_element(name, b"shd", word_prefixes) {
        (6, 7)
    } else if is_word_element(name, b"noWrap", word_prefixes) {
        (7, 8)
    } else if is_word_element(name, b"tcMar", word_prefixes) {
        (8, 9)
    } else if is_word_element(name, b"textDirection", word_prefixes) {
        (9, 10)
    } else if is_word_element(name, b"tcFitText", word_prefixes) {
        (10, 11)
    } else if is_word_element(name, b"vAlign", word_prefixes) {
        (11, 12)
    } else if is_word_element(name, b"hideMark", word_prefixes) {
        (12, 13)
    } else if is_word_element(name, b"headers", word_prefixes) {
        (13, 14)
    } else if is_word_element(name, b"cellIns", word_prefixes) {
        (14, 15)
    } else if is_word_element(name, b"cellDel", word_prefixes) {
        (15, 16)
    } else if is_word_element(name, b"cellMerge", word_prefixes) {
        (16, 17)
    } else if is_word_element(name, b"tcPrChange", word_prefixes) {
        (17, 18)
    } else {
        (current, current)
    }
}

// ---- Table cell ----

/// Content that can appear inside a table cell.
#[derive(Debug, Clone, PartialEq)]
pub enum CellContent {
    /// A paragraph.
    Paragraph(CT_P),
    /// A nested table.
    Table(CT_Tbl),
    /// A paragraph-level content control.
    ContentControl(CT_Sdt),
}

/// `CT_Tc` — A table cell containing paragraphs and possibly nested tables.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_Tc {
    pub properties: Option<CT_TcPr>,
    /// Cell content (paragraphs and nested tables).
    pub content: Vec<CellContent>,
    /// Raw XML for children we do not model, tagged with the content index they
    /// appeared before so they can be written back in place.
    pub extra_xml: Vec<(usize, Vec<u8>)>,
}

#[allow(non_snake_case)]
impl CT_Tc {
    /// Parse one namespace-complete checked tc owner before staging.
    #[doc(hidden)]
    pub fn from_xml_fragment_checked(xml: &[u8]) -> Result<Self> {
        let admitted = crate::document::admit_word_source(xml, &[], None, None)?;
        let mut controls = admitted.controls();
        crate::text::validate_word_owner_fragment(xml, b"tc", true)?;
        let mut reader = Reader::from_reader(xml);
        loop {
            match reader.read_event()? {
                Event::Start(root) => {
                    let prefixes = word_prefixes_at(&root, &[])?;
                    let bindings = local_namespace_overrides(&root, &[])?;
                    let mut model = Self::from_xml_with_prefixes_and_owner_bindings_at_depth(
                        &mut reader,
                        &prefixes,
                        &bindings,
                        0,
                        Some(&root),
                        Some(&mut controls),
                    )?;
                    model.retain_root_attributes(&root, &prefixes)?;
                    return Ok(model);
                }
                Event::Empty(root) => {
                    let prefixes = word_prefixes_at(&root, &[])?;
                    let mut model = Self::new();
                    model.retain_root_attributes(&root, &prefixes)?;
                    return Ok(model);
                }
                Event::Eof => return Err(OxmlError::MissingElement("element XML tc owner".into())),
                _ => {}
            }
        }
    }

    /// Retain the actual source owner attributes in the existing raw carrier.
    pub(crate) fn retain_root_attributes(
        &mut self,
        root: &BytesStart<'_>,
        prefixes: &[String],
    ) -> Result<()> {
        if let Some(record) = capture_root_attribute_record(root, prefixes)? {
            self.extra_xml.push((ROOT_ATTRIBUTES_POSITION, record));
        }
        Ok(())
    }

    pub fn new() -> Self {
        CT_Tc {
            properties: None,
            // OOXML requires at least one paragraph per cell
            content: vec![CellContent::Paragraph(CT_P::new())],
            extra_xml: Vec::new(),
        }
    }

    /// Get all paragraphs in this cell (excludes nested tables).
    pub fn paragraphs(&self) -> Vec<&CT_P> {
        let mut paragraphs = Vec::new();
        self.collect_paragraphs(&mut paragraphs);
        paragraphs
    }

    /// Get mutable reference to paragraphs (backward compatibility).
    pub fn paragraphs_mut(&mut self) -> Vec<&mut CT_P> {
        self.content
            .iter_mut()
            .filter_map(|c| match c {
                CellContent::Paragraph(p) => Some(p),
                CellContent::Table(_) | CellContent::ContentControl(_) => None,
            })
            .collect()
    }

    /// Get the raw text of the cell's paragraphs, joined with `\n`.
    ///
    /// Each paragraph is read by `CT_P::text`, which leaves out simple field
    /// results and tracked insertions. [`Self::accepted_text`] reads the text
    /// a reader of the document sees.
    pub fn text(&self) -> String {
        self.paragraphs()
            .iter()
            .map(|p| p.text())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Get the text a reader of the document sees in the cell's paragraphs,
    /// each read as `Paragraph::text` reads it, joined with `\n`.
    #[doc(hidden)]
    pub fn accepted_text(&self) -> String {
        self.paragraphs()
            .iter()
            .map(|p| p.accepted_text())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn from_xml(reader: &mut Reader<&[u8]>) -> Result<Self> {
        let admitted = crate::document::admit_word_reader(
            reader,
            &["w".to_owned()],
            b"tc",
            Some(crate::namespace::W_NS),
        )?;
        let mut controls = admitted.controls();
        Self::from_xml_with_prefixes_and_owner_bindings_at_depth(
            reader,
            &["w".to_string()],
            &[],
            0,
            None,
            Some(&mut controls),
        )
    }

    pub(crate) fn from_xml_with_prefixes_and_owner_bindings_at_depth(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
        table_depth: usize,
        root: Option<&BytesStart<'_>>,
        control_facts: Option<&mut crate::document::ControlSourceCursor<'_>>,
    ) -> Result<Self> {
        Self::from_xml_with_prefixes_and_owner_bindings_at_depth_and_maps(
            reader,
            word_prefixes,
            owner_bindings,
            table_depth,
            root,
            control_facts,
            None,
        )
    }

    pub(crate) fn from_xml_with_prefixes_and_owner_bindings_at_depth_and_maps(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
        table_depth: usize,
        root: Option<&BytesStart<'_>>,
        mut control_facts: Option<&mut crate::document::ControlSourceCursor<'_>>,
        destination_maps: Option<&mut crate::text::WordBlockSourceMaps>,
    ) -> Result<Self> {
        let mut pending_maps =
            destination_maps
                .as_ref()
                .map(|maps| crate::text::WordBlockSourceMaps {
                    collect_word_texts: maps.collect_word_texts,
                    collect_properties: maps.collect_properties,
                    ..Default::default()
                });
        let mut source_maps = pending_maps.as_mut();

        let raw_owner_bindings =
            crate::text::bindings_outside_retained_root(owner_bindings, root, word_prefixes)?;
        let mut properties = None;
        let mut content = Vec::new();
        let mut extra_xml = Vec::new();
        let mut buf = Vec::new();

        loop {
            let event_start = reader.buffer_position();
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(name.as_ref(), b"tcPr", &prefixes) {
                        let local_bindings = local_namespace_overrides(e, word_prefixes)?;
                        let property_bindings =
                            merged_owner_bindings(owner_bindings, &local_bindings);
                        properties = Some(CT_TcPr::from_xml_with_prefixes_and_owner_bindings(
                            reader,
                            &prefixes,
                            &property_bindings,
                        )?);
                    } else if is_word_element(name.as_ref(), b"p", &prefixes) {
                        let mut maps = source_maps.as_ref().map(|_| Vec::new());
                        let collect_word = source_maps
                            .as_ref()
                            .is_some_and(|maps| maps.collect_word_texts);
                        let mut word_texts = Vec::new();
                        let collect_properties = source_maps
                            .as_ref()
                            .is_some_and(|maps| maps.collect_properties);
                        let mut ordinary = Vec::new();
                        let mut nested = crate::text::WordBlockSourceMaps::default();
                        let paragraph = CT_P::from_xml_with_read_output(
                            reader,
                            &prefixes,
                            Some(e),
                            control_facts.as_deref_mut(),
                            crate::text::WordParagraphReadOutput {
                                extra_maps: maps.as_mut(),
                                text_facts: collect_word.then_some(&mut word_texts),
                                ordinary: collect_properties.then_some(&mut ordinary),
                                nested_maps: collect_word.then_some(&mut nested),
                                ..Default::default()
                            },
                        )?;
                        let word_texts = collect_word.then(|| {
                            control_facts
                                .as_deref()
                                .and_then(|cursor| cursor.paragraph_word_facts(reader, word_texts))
                                .map(|mut facts| {
                                    facts.nested = std::mem::take(&mut nested);
                                    facts
                                })
                        });
                        if let (Some(output), Some(maps)) = (source_maps.as_deref_mut(), maps) {
                            let mut child = nested;
                            child.paragraph(maps);
                            child.ordinary = ordinary;
                            if let Some(facts) = word_texts {
                                child.word_texts.push((Vec::new(), facts));
                            }
                            output.append(
                                crate::text::WordBlockComponent::CellContent(content.len()),
                                child,
                            );
                        }
                        content.push(CellContent::Paragraph(paragraph));
                    } else if is_word_element(name.as_ref(), b"tbl", &prefixes) {
                        let local_bindings = local_namespace_overrides(e, word_prefixes)?;
                        let table_bindings = merged_owner_bindings(owner_bindings, &local_bindings);
                        let mut child_maps =
                            source_maps
                                .as_ref()
                                .map(|maps| crate::text::WordBlockSourceMaps {
                                    collect_word_texts: maps.collect_word_texts,
                                    collect_properties: maps.collect_properties,
                                    ..Default::default()
                                });
                        let mut table =
                            CT_Tbl::from_xml_with_prefixes_and_owner_bindings_at_depth_and_maps(
                                reader,
                                &prefixes,
                                &table_bindings,
                                table_depth.saturating_add(1),
                                Some(e),
                                control_facts.as_deref_mut(),
                                child_maps.as_mut(),
                            )?;
                        table.retain_root_attributes(e, &prefixes)?;
                        if let (Some(output), Some(child)) =
                            (source_maps.as_deref_mut(), child_maps)
                        {
                            output.append(
                                crate::text::WordBlockComponent::CellContent(content.len()),
                                child,
                            );
                        }
                        content.push(CellContent::Table(table));
                    } else if is_word_element(name.as_ref(), b"sdt", &prefixes) {
                        let captured = capture_element(reader, e)?;
                        let captured_map = control_facts.as_deref().and_then(|facts| {
                            facts.captured_map(
                                reader,
                                event_start..reader.buffer_position(),
                                &captured,
                            )
                        });
                        let (raw, captured_map) = crate::text::raw_with_external_bindings_and_map(
                            &captured,
                            &raw_owner_bindings,
                            None,
                            captured_map,
                        )?;
                        let mut child_maps =
                            source_maps
                                .as_ref()
                                .map(|maps| crate::text::WordBlockSourceMaps {
                                    collect_word_texts: maps.collect_word_texts,
                                    collect_properties: maps.collect_properties,
                                    ..Default::default()
                                });
                        if let Some(sdt) = CT_Sdt::from_raw_with_mapped_context_and_maps(
                            &raw,
                            &prefixes,
                            SdtOwner::Cell,
                            control_facts.as_deref_mut(),
                            captured_map,
                            child_maps.as_mut(),
                        ) {
                            if let (Some(output), Some(child)) =
                                (source_maps.as_deref_mut(), child_maps)
                            {
                                output.append(
                                    crate::text::WordBlockComponent::CellContent(content.len()),
                                    child,
                                );
                            }
                            content.push(CellContent::ContentControl(sdt));
                        } else {
                            extra_xml.push((content.len(), raw));
                        }
                    } else {
                        // Content controls (w:sdt), bookmarks and revision
                        // marks live here. Keep them verbatim rather than
                        // dropping the subtree, which used to delete every
                        // paragraph wrapped in a content control.
                        extra_xml.push((
                            content.len(),
                            crate::text::raw_with_external_bindings(
                                &capture_element(reader, e)?,
                                &raw_owner_bindings,
                            )?,
                        ));
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(name.as_ref(), b"p", &prefixes) {
                        let paragraph = CT_P::from_empty_root(e, &prefixes)?;
                        if let Some(output) = source_maps.as_deref_mut() {
                            let mut child = crate::text::WordBlockSourceMaps::default();
                            child.paragraph(vec![None; paragraph.extra_xml.len()]);
                            if output.collect_word_texts {
                                child.word_texts.push((Vec::new(), None));
                            }
                            output.append(
                                crate::text::WordBlockComponent::CellContent(content.len()),
                                child,
                            );
                        }
                        content.push(CellContent::Paragraph(paragraph));
                    } else if is_word_element(name.as_ref(), b"tbl", &prefixes) {
                        content.push(CellContent::Table(CT_Tbl::new()));
                    } else if is_word_element(name.as_ref(), b"tcPr", &prefixes) {
                        properties = Some(CT_TcPr::empty_from_source());
                    } else {
                        extra_xml.push((
                            content.len(),
                            crate::text::raw_with_external_bindings(
                                &capture_empty_element(e)?,
                                &raw_owner_bindings,
                            )?,
                        ));
                    }
                }
                Ok(Event::End(ref e)) if matches_local_name(e.name().as_ref(), b"tc") => {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        if let (Some(destination), Some(pending)) = (destination_maps, pending_maps) {
            destination.paragraphs.extend(pending.paragraphs);
            destination.word_texts.extend(pending.word_texts);
            destination.word_runs.extend(pending.word_runs);
            destination.word_raws.extend(pending.word_raws);
            destination.ordinary.extend(pending.ordinary);
        }
        Ok(CT_Tc {
            properties,
            content,
            extra_xml,
        })
    }

    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.write_xml_role(writer, XmlWriteRole::Publication, None)
    }

    pub(crate) fn writer_field_roots(&self) -> Vec<&crate::text::Field> {
        let mut fields = Vec::new();
        for child in &self.content {
            fields.extend(match child {
                CellContent::Paragraph(paragraph) => paragraph.writer_field_roots(),
                CellContent::Table(table) => table.writer_field_roots(),
                CellContent::ContentControl(control) => control.writer_field_roots(),
            });
        }
        fields
    }

    pub(crate) fn write_xml_role<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<()> {
        let mut work = crate::text::WordOperationWork::model_fields(self.writer_field_roots())?;
        self.write_xml_role_in(writer, role, source_context, &mut work, None)
    }

    pub(crate) fn write_xml_role_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        work: &mut crate::text::WordOperationWork,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let _source_scope =
            (role == XmlWriteRole::Source).then(|| crate::text::root_binding_scope(0));
        crate::document::admit_word_model(crate::document::WordOwner::Cell(self))?;
        let mut start = crate::text::FieldXmlStart::new("w:tc", emission)?;
        for (position, raw) in &self.extra_xml {
            if *position == ROOT_ATTRIBUTES_POSITION && is_root_attribute_record(raw) {
                start.root_record(raw, None, role)?;
            }
        }
        let prefix = if role == XmlWriteRole::Source {
            start.qualify(&[], role, source_context)?
        } else {
            start.restore_word_binding()?;
            "w".to_owned()
        };
        let local = crate::text::raw_namespace_declarations(start.element())?;
        let mut context = source_context.unwrap_or(&[]).to_vec();
        for (name, uri) in &local {
            context.retain(|(known, _)| *known != name);
            context.push((name, uri));
        }
        let source_context = (role == XmlWriteRole::Source).then_some(context.as_slice());
        let _owner_scope = crate::text::emitted_binding_scope(start.element(), false, false)?;
        start.write(writer, false)?;

        if let Some(ref props) = self.properties {
            props.write_xml_role_in(writer, role, source_context, emission)?;
        }

        for (idx, item) in self.content.iter().enumerate() {
            write_extras_at(writer, &self.extra_xml, idx)?;
            match item {
                CellContent::Paragraph(p) => {
                    p.write_xml_core_in(writer, None, role, source_context, work, emission)?
                }
                CellContent::Table(tbl) => {
                    tbl.write_xml_role_in(writer, role, source_context, work, emission)?
                }
                CellContent::ContentControl(sdt) => {
                    sdt.write_xml_role_in(writer, role, source_context, work, emission)?
                }
            }
        }
        write_extras_at(writer, &self.extra_xml, self.content.len())?;

        if let Some(emission) = emission {
            emission.write_word_end(writer, &prefix, "tc")?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(format!("{prefix}:tc"))))?;
        }
        Ok(())
    }

    pub(crate) fn collect_controls<'a>(&'a self, controls: &mut Vec<&'a CT_Sdt>) {
        for content in &self.content {
            match content {
                CellContent::Paragraph(paragraph) => paragraph.collect_controls(controls),
                CellContent::Table(table) => table.collect_controls(controls),
                CellContent::ContentControl(sdt) => {
                    controls.push(sdt);
                    sdt.collect_controls(SdtOwner::Cell, controls);
                }
            }
        }
    }

    pub(crate) fn collect_paragraphs<'a>(&'a self, paragraphs: &mut Vec<&'a CT_P>) {
        for content in &self.content {
            match content {
                CellContent::Paragraph(paragraph) => paragraphs.push(paragraph),
                CellContent::ContentControl(sdt) => {
                    sdt.collect_paragraphs(SdtOwner::Cell, paragraphs)
                }
                CellContent::Table(_) => {}
            }
        }
    }

    pub(crate) fn collect_tables<'a>(&'a self, tables: &mut Vec<&'a CT_Tbl>) {
        for content in &self.content {
            match content {
                CellContent::Table(table) => tables.push(table),
                CellContent::ContentControl(sdt) => sdt.collect_tables(SdtOwner::Cell, tables),
                CellContent::Paragraph(_) => {}
            }
        }
    }
}

impl Default for CT_Tc {
    fn default() -> Self {
        Self::new()
    }
}

// ---- Table row ----

/// `CT_Row` — A table row containing cells.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_Row {
    /// The optional Word table-property exception for this row.
    ///
    /// Its contents affect table presentation only. The complete raw element
    /// remains the serialization source because the layout model does not
    /// interpret every table-property variant.
    pub table_property_exception: Option<Vec<u8>>,
    pub properties: Option<CT_TrPr>,
    pub cells: Vec<CT_Tc>,
    /// Raw XML for children we do not model, tagged with the cell index they
    /// appeared before so they can be written back in place. The attributes
    /// of the `w:tr` start tag, such as `w:rsidR` or `w14:paraId`, are kept in
    /// one record at position `usize::MAX`, which no cell boundary reaches.
    pub extra_xml: Vec<(usize, Vec<u8>)>,
    /// Typed cell controls at `(cell index, raw children before, control)`.
    pub content_controls: Vec<(usize, usize, CT_Sdt)>,
}

#[allow(non_snake_case)]
impl CT_Row {
    pub fn new() -> Self {
        CT_Row {
            table_property_exception: None,
            properties: None,
            cells: Vec::new(),
            extra_xml: Vec::new(),
            content_controls: Vec::new(),
        }
    }

    /// Whether accepting every tracked change removes this row, which is the
    /// row rule of `Document::accept_all`: its `w:trPr` carries a `w:del`
    /// marker, or a `w:moveFrom` marker with an id and an author kept as
    /// preserved XML ([`CT_TrPr::moved_away`]).
    #[doc(hidden)]
    pub fn accepted_view_removes(&self) -> bool {
        self.properties.as_ref().is_some_and(|properties| {
            properties.moved_away
                || properties
                    .revision_markers
                    .iter()
                    .any(|marker| marker.kind() == RevisionKind::Deletion)
        })
    }

    /// Report whether one raw row carrier retains root attributes.
    #[doc(hidden)]
    pub fn raw_is_root_attributes(position: usize, raw: &[u8]) -> bool {
        position == ROOT_ATTRIBUTES_POSITION && is_root_attribute_record(raw)
    }

    pub(crate) fn from_empty_root(root: &BytesStart<'_>, word_prefixes: &[String]) -> Result<Self> {
        let mut row = Self::new();
        if let Some(record) = capture_root_attribute_record(root, word_prefixes)? {
            row.extra_xml.push((ROOT_ATTRIBUTES_POSITION, record));
        }
        Ok(row)
    }

    pub fn from_xml(reader: &mut Reader<&[u8]>) -> Result<Self> {
        let admitted = crate::document::admit_word_reader(
            reader,
            &["w".to_owned()],
            b"tr",
            Some(crate::namespace::W_NS),
        )?;
        let mut controls = admitted.controls();
        Self::from_xml_with_prefixes_and_owner_bindings_at_depth(
            reader,
            &["w".to_string()],
            &[],
            0,
            None,
            Some(&mut controls),
        )
    }

    pub(crate) fn from_xml_with_prefixes_and_owner_bindings_at_depth(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
        table_depth: usize,
        root: Option<&BytesStart<'_>>,
        control_facts: Option<&mut crate::document::ControlSourceCursor<'_>>,
    ) -> Result<Self> {
        Self::from_xml_with_prefixes_and_owner_bindings_at_depth_and_maps(
            reader,
            word_prefixes,
            owner_bindings,
            table_depth,
            root,
            control_facts,
            None,
        )
    }

    pub(crate) fn from_xml_with_prefixes_and_owner_bindings_at_depth_and_maps(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
        table_depth: usize,
        root: Option<&BytesStart<'_>>,
        mut control_facts: Option<&mut crate::document::ControlSourceCursor<'_>>,
        destination_maps: Option<&mut crate::text::WordBlockSourceMaps>,
    ) -> Result<Self> {
        let mut pending_maps =
            destination_maps
                .as_ref()
                .map(|maps| crate::text::WordBlockSourceMaps {
                    collect_word_texts: maps.collect_word_texts,
                    collect_properties: maps.collect_properties,
                    ..Default::default()
                });
        let mut source_maps = pending_maps.as_mut();

        let mut table_property_exception = None;
        let mut properties = None;
        let mut cells = Vec::new();
        let mut extra_xml = Vec::new();
        let mut content_controls = Vec::new();
        let mut buf = Vec::new();

        loop {
            let event_start = reader.buffer_position();
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(name.as_ref(), b"tblPrEx", &prefixes)
                        && table_property_exception.is_none()
                    {
                        table_property_exception = Some(crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            owner_bindings,
                        )?);
                    } else if is_word_element(name.as_ref(), b"trPr", &prefixes) {
                        let local_bindings = local_namespace_overrides(e, word_prefixes)?;
                        let property_bindings =
                            merged_owner_bindings(owner_bindings, &local_bindings);
                        properties = Some(CT_TrPr::from_xml_with_prefixes_and_owner_bindings(
                            reader,
                            &prefixes,
                            &property_bindings,
                        )?);
                    } else if is_word_element(name.as_ref(), b"tc", &prefixes) {
                        let local_bindings = local_namespace_overrides(e, word_prefixes)?;
                        let cell_bindings = merged_owner_bindings(owner_bindings, &local_bindings);
                        let mut child_maps =
                            source_maps
                                .as_ref()
                                .map(|maps| crate::text::WordBlockSourceMaps {
                                    collect_word_texts: maps.collect_word_texts,
                                    collect_properties: maps.collect_properties,
                                    ..Default::default()
                                });
                        let mut cell =
                            CT_Tc::from_xml_with_prefixes_and_owner_bindings_at_depth_and_maps(
                                reader,
                                &prefixes,
                                &cell_bindings,
                                table_depth,
                                Some(e),
                                control_facts.as_deref_mut(),
                                child_maps.as_mut(),
                            )?;
                        cell.retain_root_attributes(e, &prefixes)?;
                        if let (Some(output), Some(child)) =
                            (source_maps.as_deref_mut(), child_maps)
                        {
                            output.append(
                                crate::text::WordBlockComponent::RowCell(cells.len()),
                                child,
                            );
                        }
                        cells.push(cell);
                    } else if is_word_element(name.as_ref(), b"sdt", &prefixes) {
                        let captured = capture_element(reader, e)?;
                        let captured_map = control_facts.as_deref().and_then(|facts| {
                            facts.captured_map(
                                reader,
                                event_start..reader.buffer_position(),
                                &captured,
                            )
                        });
                        let (raw, captured_map) = crate::text::raw_with_external_bindings_and_map(
                            &captured,
                            owner_bindings,
                            None,
                            captured_map,
                        )?;
                        let mut child_maps =
                            source_maps
                                .as_ref()
                                .map(|maps| crate::text::WordBlockSourceMaps {
                                    collect_word_texts: maps.collect_word_texts,
                                    collect_properties: maps.collect_properties,
                                    ..Default::default()
                                });
                        if let Some(sdt) = CT_Sdt::from_raw_with_mapped_context_and_maps(
                            &raw,
                            &prefixes,
                            SdtOwner::Row,
                            control_facts.as_deref_mut(),
                            captured_map,
                            child_maps.as_mut(),
                        ) {
                            if let (Some(output), Some(child)) =
                                (source_maps.as_deref_mut(), child_maps)
                            {
                                output.append(
                                    crate::text::WordBlockComponent::RowControl(
                                        content_controls.len(),
                                    ),
                                    child,
                                );
                            }
                            let raw_before = extra_xml
                                .iter()
                                .filter(|(at, _)| *at == cells.len())
                                .count();
                            content_controls.push((cells.len(), raw_before, sdt));
                        } else {
                            extra_xml.push((cells.len(), raw));
                        }
                    } else {
                        // A cell wrapped in a content control used to be
                        // dropped here, leaving a row with no cells at all.
                        extra_xml.push((
                            cells.len(),
                            crate::text::raw_with_external_bindings(
                                &capture_element(reader, e)?,
                                owner_bindings,
                            )?,
                        ));
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(name.as_ref(), b"tblPrEx", &prefixes)
                        && table_property_exception.is_none()
                    {
                        table_property_exception = Some(crate::text::raw_with_external_bindings(
                            &capture_empty_element(e)?,
                            owner_bindings,
                        )?);
                    } else if is_word_element(name.as_ref(), b"tc", &prefixes) {
                        let mut cell = CT_Tc::new();
                        cell.retain_root_attributes(e, &prefixes)?;
                        if let Some(output) = source_maps.as_deref_mut() {
                            // CT_Tc::new creates this one empty paragraph. It
                            // has no physical paragraph or raw extra origins.
                            output.paragraphs.push((
                                vec![
                                    crate::text::WordBlockComponent::RowCell(cells.len()),
                                    crate::text::WordBlockComponent::CellContent(0),
                                ],
                                Vec::new(),
                            ));
                            if output.collect_word_texts {
                                output.word_texts.push((
                                    vec![
                                        crate::text::WordBlockComponent::RowCell(cells.len()),
                                        crate::text::WordBlockComponent::CellContent(0),
                                    ],
                                    None,
                                ));
                            }
                        }
                        cells.push(cell);
                    } else if is_word_element(name.as_ref(), b"trPr", &prefixes) {
                        properties = Some(CT_TrPr::empty_from_source());
                    } else {
                        extra_xml.push((
                            cells.len(),
                            crate::text::raw_with_external_bindings(
                                &capture_empty_element(e)?,
                                owner_bindings,
                            )?,
                        ));
                    }
                }
                Ok(Event::End(ref e)) if matches_local_name(e.name().as_ref(), b"tr") => {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        if let Some(record) = root
            .map(|root| capture_root_attribute_record(root, word_prefixes))
            .transpose()?
            .flatten()
        {
            extra_xml.push((ROOT_ATTRIBUTES_POSITION, record));
        }

        if let (Some(destination), Some(pending)) = (destination_maps, pending_maps) {
            destination.paragraphs.extend(pending.paragraphs);
            destination.word_texts.extend(pending.word_texts);
            destination.word_runs.extend(pending.word_runs);
            destination.word_raws.extend(pending.word_raws);
            destination.ordinary.extend(pending.ordinary);
        }
        Ok(CT_Row {
            table_property_exception,
            properties,
            cells,
            extra_xml,
            content_controls,
        })
    }

    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.write_xml_role(writer, XmlWriteRole::Publication, None)
    }

    pub(crate) fn writer_field_roots(&self) -> Vec<&crate::text::Field> {
        let mut fields = Vec::new();
        for cell in &self.cells {
            fields.extend(cell.writer_field_roots());
        }
        for (_, _, control) in &self.content_controls {
            fields.extend(control.writer_field_roots());
        }
        fields
    }

    pub(crate) fn write_xml_role<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<()> {
        let mut work = crate::text::WordOperationWork::model_fields(self.writer_field_roots())?;
        self.write_xml_role_in(writer, role, source_context, &mut work, None)
    }

    pub(crate) fn write_xml_role_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        work: &mut crate::text::WordOperationWork,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let _source_scope =
            (role == XmlWriteRole::Source).then(|| crate::text::root_binding_scope(0));
        crate::document::admit_word_model(crate::document::WordOwner::Row(self))?;
        let mut start = crate::text::FieldXmlStart::new("w:tr", emission)?;
        for (position, raw) in &self.extra_xml {
            if Self::raw_is_root_attributes(*position, raw) {
                start.root_record(raw, None, role)?;
            }
        }
        let prefix = if role == XmlWriteRole::Source {
            start.qualify(&[], role, source_context)?
        } else {
            start.restore_word_binding()?;
            "w".to_owned()
        };
        let local = crate::text::raw_namespace_declarations(start.element())?;
        let mut context = source_context.unwrap_or(&[]).to_vec();
        for (name, uri) in &local {
            context.retain(|(known, _)| *known != name);
            context.push((name, uri));
        }
        let source_context = (role == XmlWriteRole::Source).then_some(context.as_slice());
        let _owner_scope = crate::text::emitted_binding_scope(start.element(), false, false)?;
        start.write(writer, false)?;

        if let Some(raw) = &self.table_property_exception {
            writer.get_mut().write_all(raw)?;
        }

        if let Some(ref props) = self.properties {
            props.write_xml_role_in(writer, role, source_context, emission)?;
        }

        for (idx, cell) in self.cells.iter().enumerate() {
            write_boundary(
                writer,
                &self.extra_xml,
                &self.content_controls,
                idx,
                role,
                source_context,
                work,
                emission,
            )?;
            cell.write_xml_role_in(writer, role, source_context, work, emission)?;
        }
        write_boundary(
            writer,
            &self.extra_xml,
            &self.content_controls,
            self.cells.len(),
            role,
            source_context,
            work,
            emission,
        )?;

        if let Some(emission) = emission {
            emission.write_word_end(writer, &prefix, "tr")?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(format!("{prefix}:tr"))))?;
        }
        Ok(())
    }

    /// Return direct and content-control-wrapped cells in document order.
    pub fn cells(&self) -> Vec<&CT_Tc> {
        let mut cells = Vec::new();
        self.collect_cells(&mut cells);
        cells
    }

    pub(crate) fn collect_cells<'a>(&'a self, cells: &mut Vec<&'a CT_Tc>) {
        for index in 0..=self.cells.len() {
            for (_, _, sdt) in self
                .content_controls
                .iter()
                .filter(|(at, _, _)| *at == index)
            {
                sdt.collect_cells(SdtOwner::Row, cells);
            }
            if let Some(cell) = self.cells.get(index) {
                cells.push(cell);
            }
        }
    }

    pub(crate) fn collect_controls<'a>(&'a self, controls: &mut Vec<&'a CT_Sdt>) {
        for index in 0..=self.cells.len() {
            for (_, _, sdt) in self
                .content_controls
                .iter()
                .filter(|(at, _, _)| *at == index)
            {
                controls.push(sdt);
                sdt.collect_controls(SdtOwner::Row, controls);
            }
            if let Some(cell) = self.cells.get(index) {
                cell.collect_controls(controls);
            }
        }
    }

    pub(crate) fn collect_paragraphs<'a>(&'a self, paragraphs: &mut Vec<&'a CT_P>) {
        for index in 0..=self.cells.len() {
            for (_, _, sdt) in self
                .content_controls
                .iter()
                .filter(|(at, _, _)| *at == index)
            {
                sdt.collect_paragraphs(SdtOwner::Row, paragraphs);
            }
            if let Some(cell) = self.cells.get(index) {
                cell.collect_paragraphs(paragraphs);
            }
        }
    }
}

impl Default for CT_Row {
    fn default() -> Self {
        Self::new()
    }
}

// ---- Table ----

/// `CT_Tbl` — A table element containing rows.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_Tbl {
    pub properties: Option<CT_TblPr>,
    pub grid: Option<CT_TblGrid>,
    pub rows: Vec<CT_Row>,
    /// Raw XML for children we do not model, tagged with the row index they
    /// appeared before so they can be written back in place.
    pub extra_xml: Vec<(usize, Vec<u8>)>,
    /// Typed row controls at `(row index, raw children before, control)`.
    pub content_controls: Vec<(usize, usize, CT_Sdt)>,
}

#[allow(non_snake_case)]
impl CT_Tbl {
    /// Parse one namespace-complete checked tbl owner before staging.
    #[doc(hidden)]
    pub fn from_xml_fragment_checked(xml: &[u8]) -> Result<Self> {
        let admitted = crate::document::admit_word_source(xml, &[], None, None)?;
        let mut controls = admitted.controls();
        Self::from_xml_fragment_checked_in(xml, &mut controls, None)
    }

    pub(crate) fn from_xml_fragment_checked_in(
        xml: &[u8],
        controls: &mut crate::document::ControlSourceCursor<'_>,
        mut maps: Option<&mut crate::text::WordBlockSourceMaps>,
    ) -> Result<Self> {
        crate::text::validate_word_owner_fragment(xml, b"tbl", true)?;
        let mut reader = Reader::from_reader(xml);
        loop {
            match reader.read_event()? {
                Event::Start(root) => {
                    let prefixes = word_prefixes_at(&root, &[])?;
                    let bindings = local_namespace_overrides(&root, &[])?;
                    let mut model =
                        Self::from_xml_with_prefixes_and_owner_bindings_at_depth_and_maps(
                            &mut reader,
                            &prefixes,
                            &bindings,
                            0,
                            Some(&root),
                            Some(controls),
                            maps.as_deref_mut(),
                        )?;
                    model.retain_root_attributes(&root, &prefixes)?;
                    return Ok(model);
                }
                Event::Empty(root) => {
                    let prefixes = word_prefixes_at(&root, &[])?;
                    let mut model = Self::new();
                    model.retain_root_attributes(&root, &prefixes)?;
                    return Ok(model);
                }
                Event::Eof => {
                    return Err(OxmlError::MissingElement("element XML tbl owner".into()));
                }
                _ => {}
            }
        }
    }

    /// Retain the actual source owner attributes in the existing raw carrier.
    pub(crate) fn retain_root_attributes(
        &mut self,
        root: &BytesStart<'_>,
        prefixes: &[String],
    ) -> Result<()> {
        if let Some(record) = capture_root_attribute_record(root, prefixes)? {
            self.extra_xml.push((ROOT_ATTRIBUTES_POSITION, record));
        }
        Ok(())
    }

    pub fn new() -> Self {
        CT_Tbl {
            properties: None,
            grid: None,
            rows: Vec::new(),
            extra_xml: Vec::new(),
            content_controls: Vec::new(),
        }
    }

    /// Whether accepting every tracked change removes this whole table, as
    /// `Document::accept_all` does: it has a row, and every row it owns, rows
    /// inside its row-level content controls included, is removed by
    /// [`CT_Row::accepted_view_removes`]. Rows of nested tables do not count.
    #[doc(hidden)]
    pub fn accepted_view_removes(&self) -> bool {
        fn control_rows<'a>(control: &'a CT_Sdt, rows: &mut Vec<&'a CT_Row>) {
            for content in &control.content {
                match content {
                    SdtContent::Row(row) => rows.push(row),
                    SdtContent::ContentControl(nested) => control_rows(nested, rows),
                    _ => {}
                }
            }
        }
        let mut rows = self.rows.iter().collect::<Vec<_>>();
        for (_, _, control) in &self.content_controls {
            control_rows(control, &mut rows);
        }
        !rows.is_empty() && rows.iter().all(|row| row.accepted_view_removes())
    }

    pub fn from_xml(reader: &mut Reader<&[u8]>) -> Result<Self> {
        let admitted = crate::document::admit_word_reader(
            reader,
            &["w".to_owned()],
            b"tbl",
            Some(crate::namespace::W_NS),
        )?;
        let mut controls = admitted.controls();
        Self::from_xml_with_prefixes_and_owner_bindings_at_depth(
            reader,
            &["w".to_string()],
            &[],
            0,
            None,
            Some(&mut controls),
        )
    }

    pub(crate) fn from_xml_with_prefixes_and_owner_bindings_at_depth(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
        table_depth: usize,
        root: Option<&BytesStart<'_>>,
        control_facts: Option<&mut crate::document::ControlSourceCursor<'_>>,
    ) -> Result<Self> {
        Self::from_xml_with_prefixes_and_owner_bindings_at_depth_and_maps(
            reader,
            word_prefixes,
            owner_bindings,
            table_depth,
            root,
            control_facts,
            None,
        )
    }

    pub(crate) fn from_xml_with_prefixes_and_owner_bindings_at_depth_and_maps(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
        table_depth: usize,
        root: Option<&BytesStart<'_>>,
        mut control_facts: Option<&mut crate::document::ControlSourceCursor<'_>>,
        destination_maps: Option<&mut crate::text::WordBlockSourceMaps>,
    ) -> Result<Self> {
        let mut pending_maps =
            destination_maps
                .as_ref()
                .map(|maps| crate::text::WordBlockSourceMaps {
                    collect_word_texts: maps.collect_word_texts,
                    collect_properties: maps.collect_properties,
                    ..Default::default()
                });
        let mut source_maps = pending_maps.as_mut();

        let raw_owner_bindings =
            crate::text::bindings_outside_retained_root(owner_bindings, root, word_prefixes)?;
        if table_depth >= MAX_RECOGNIZED_TABLE_NESTING {
            return Err(OxmlError::InvalidValue(
                "recognized model nesting exceeds table limit".to_owned(),
            ));
        }
        let mut properties = None;
        let mut grid = None;
        let mut rows = Vec::new();
        let mut extra_xml = Vec::new();
        let mut content_controls = Vec::new();
        let mut buf = Vec::new();

        loop {
            let event_start = reader.buffer_position();
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(name.as_ref(), b"tblPr", &prefixes) {
                        let local_bindings = local_namespace_overrides(e, word_prefixes)?;
                        let property_bindings =
                            merged_owner_bindings(owner_bindings, &local_bindings);
                        properties = Some(CT_TblPr::from_xml_with_prefixes_and_owner_bindings(
                            reader,
                            &prefixes,
                            &property_bindings,
                        )?);
                    } else if is_word_element(name.as_ref(), b"tblGrid", &prefixes) {
                        grid = Some(CT_TblGrid::from_xml_with_prefixes(reader, &prefixes)?);
                    } else if is_word_element(name.as_ref(), b"tr", &prefixes) {
                        let local_bindings = local_namespace_overrides(e, word_prefixes)?;
                        let row_bindings = merged_owner_bindings(owner_bindings, &local_bindings);
                        let mut child_maps =
                            source_maps
                                .as_ref()
                                .map(|maps| crate::text::WordBlockSourceMaps {
                                    collect_word_texts: maps.collect_word_texts,
                                    collect_properties: maps.collect_properties,
                                    ..Default::default()
                                });
                        let row =
                            CT_Row::from_xml_with_prefixes_and_owner_bindings_at_depth_and_maps(
                                reader,
                                &prefixes,
                                &row_bindings,
                                table_depth,
                                Some(e),
                                control_facts.as_deref_mut(),
                                child_maps.as_mut(),
                            )?;
                        if let (Some(output), Some(child)) =
                            (source_maps.as_deref_mut(), child_maps)
                        {
                            output.append(
                                crate::text::WordBlockComponent::TableRow(rows.len()),
                                child,
                            );
                        }
                        rows.push(row);
                    } else if is_word_element(name.as_ref(), b"sdt", &prefixes) {
                        let captured = capture_element(reader, e)?;
                        let captured_map = control_facts.as_deref().and_then(|facts| {
                            facts.captured_map(
                                reader,
                                event_start..reader.buffer_position(),
                                &captured,
                            )
                        });
                        let (raw, captured_map) = crate::text::raw_with_external_bindings_and_map(
                            &captured,
                            &raw_owner_bindings,
                            None,
                            captured_map,
                        )?;
                        let mut child_maps =
                            source_maps
                                .as_ref()
                                .map(|maps| crate::text::WordBlockSourceMaps {
                                    collect_word_texts: maps.collect_word_texts,
                                    collect_properties: maps.collect_properties,
                                    ..Default::default()
                                });
                        if let Some(sdt) = CT_Sdt::from_raw_with_mapped_context_and_maps(
                            &raw,
                            &prefixes,
                            SdtOwner::Table,
                            control_facts.as_deref_mut(),
                            captured_map,
                            child_maps.as_mut(),
                        ) {
                            if let (Some(output), Some(child)) =
                                (source_maps.as_deref_mut(), child_maps)
                            {
                                output.append(
                                    crate::text::WordBlockComponent::TableControl(
                                        content_controls.len(),
                                    ),
                                    child,
                                );
                            }
                            let raw_before =
                                extra_xml.iter().filter(|(at, _)| *at == rows.len()).count();
                            content_controls.push((rows.len(), raw_before, sdt));
                        } else {
                            extra_xml.push((rows.len(), raw));
                        }
                    } else if matches_local_name(name.as_ref(), b"tblGrid") {
                        let raw = capture_element(reader, e)?;
                        let raw_bindings = merged_owner_bindings(
                            owner_bindings,
                            &preserved_table_raw_bindings(&prefixes),
                        );
                        extra_xml.push((
                            rows.len(),
                            crate::text::raw_with_external_bindings(&raw, &raw_bindings)?,
                        ));
                    } else {
                        // Rows wrapped in a content control used to be dropped
                        // here, which silently deleted whole tables.
                        extra_xml.push((
                            rows.len(),
                            crate::text::raw_with_external_bindings(
                                &capture_element(reader, e)?,
                                &raw_owner_bindings,
                            )?,
                        ));
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    // tblPr and tblGrid have fixed positions ahead of the rows,
                    // so a self-closing one must not be re-emitted from here.
                    if is_word_element(name.as_ref(), b"tblPr", &prefixes) {
                        properties = Some(CT_TblPr::default());
                    } else if is_word_element(name.as_ref(), b"tblGrid", &prefixes) {
                        grid = Some(CT_TblGrid::default());
                    } else if is_word_element(name.as_ref(), b"tr", &prefixes) {
                        rows.push(CT_Row::from_empty_root(e, &prefixes)?);
                    } else if matches_local_name(name.as_ref(), b"tblGrid") {
                        let raw = capture_empty_element(e)?;
                        let raw_bindings = merged_owner_bindings(
                            owner_bindings,
                            &preserved_table_raw_bindings(&prefixes),
                        );
                        extra_xml.push((
                            rows.len(),
                            crate::text::raw_with_external_bindings(&raw, &raw_bindings)?,
                        ));
                    } else if !is_word_element(name.as_ref(), b"tblPr", &prefixes) {
                        extra_xml.push((
                            rows.len(),
                            crate::text::raw_with_external_bindings(
                                &capture_empty_element(e)?,
                                &raw_owner_bindings,
                            )?,
                        ));
                    }
                }
                Ok(Event::End(ref e)) if matches_local_name(e.name().as_ref(), b"tbl") => {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        if let (Some(destination), Some(pending)) = (destination_maps, pending_maps) {
            destination.paragraphs.extend(pending.paragraphs);
            destination.word_texts.extend(pending.word_texts);
            destination.word_runs.extend(pending.word_runs);
            destination.word_raws.extend(pending.word_raws);
            destination.ordinary.extend(pending.ordinary);
        }
        Ok(CT_Tbl {
            properties,
            grid,
            rows,
            extra_xml,
            content_controls,
        })
    }

    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.write_xml_role(writer, XmlWriteRole::Publication, None)
    }

    pub(crate) fn writer_field_roots(&self) -> Vec<&crate::text::Field> {
        let mut fields = Vec::new();
        for row in &self.rows {
            fields.extend(row.writer_field_roots());
        }
        for (_, _, control) in &self.content_controls {
            fields.extend(control.writer_field_roots());
        }
        fields
    }

    pub(crate) fn write_xml_role<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<()> {
        let mut work = crate::text::WordOperationWork::model_fields(self.writer_field_roots())?;
        self.write_xml_role_in(writer, role, source_context, &mut work, None)
    }

    pub(crate) fn write_xml_role_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        work: &mut crate::text::WordOperationWork,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let _source_scope =
            (role == XmlWriteRole::Source).then(|| crate::text::root_binding_scope(0));
        crate::document::admit_word_model(crate::document::WordOwner::Table(self))?;
        let mut start = crate::text::FieldXmlStart::new("w:tbl", emission)?;
        for (position, raw) in &self.extra_xml {
            if *position == ROOT_ATTRIBUTES_POSITION && is_root_attribute_record(raw) {
                start.root_record(raw, None, role)?;
            }
        }
        let prefix = if role == XmlWriteRole::Source {
            start.qualify(&[], role, source_context)?
        } else {
            start.restore_word_binding()?;
            "w".to_owned()
        };
        let local = crate::text::raw_namespace_declarations(start.element())?;
        let mut context = source_context.unwrap_or(&[]).to_vec();
        for (name, uri) in &local {
            context.retain(|(known, _)| *known != name);
            context.push((name, uri));
        }
        let source_context = (role == XmlWriteRole::Source).then_some(context.as_slice());
        let _owner_scope = crate::text::emitted_binding_scope(start.element(), false, false)?;
        start.write(writer, false)?;

        if let Some(ref props) = self.properties {
            props.write_xml_role_in(writer, role, source_context, emission)?;
        }

        if let Some(ref grid) = self.grid {
            grid.write_xml_role_in(writer, role, source_context, emission)?;
        }

        for (idx, row) in self.rows.iter().enumerate() {
            write_boundary(
                writer,
                &self.extra_xml,
                &self.content_controls,
                idx,
                role,
                source_context,
                work,
                emission,
            )?;
            row.write_xml_role_in(writer, role, source_context, work, emission)?;
        }
        write_boundary(
            writer,
            &self.extra_xml,
            &self.content_controls,
            self.rows.len(),
            role,
            source_context,
            work,
            emission,
        )?;

        if let Some(emission) = emission {
            emission.write_word_end(writer, &prefix, "tbl")?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(format!("{prefix}:tbl"))))?;
        }
        Ok(())
    }

    /// Return direct and content-control-wrapped rows in document order.
    pub fn rows(&self) -> Vec<&CT_Row> {
        let mut rows = Vec::new();
        self.collect_rows(&mut rows);
        rows
    }

    pub(crate) fn collect_rows<'a>(&'a self, rows: &mut Vec<&'a CT_Row>) {
        for index in 0..=self.rows.len() {
            for (_, _, sdt) in self
                .content_controls
                .iter()
                .filter(|(at, _, _)| *at == index)
            {
                sdt.collect_rows(SdtOwner::Table, rows);
            }
            if let Some(row) = self.rows.get(index) {
                rows.push(row);
            }
        }
    }

    pub(crate) fn collect_controls<'a>(&'a self, controls: &mut Vec<&'a CT_Sdt>) {
        for index in 0..=self.rows.len() {
            for (_, _, sdt) in self
                .content_controls
                .iter()
                .filter(|(at, _, _)| *at == index)
            {
                controls.push(sdt);
                sdt.collect_controls(SdtOwner::Table, controls);
            }
            if let Some(row) = self.rows.get(index) {
                row.collect_controls(controls);
            }
        }
    }

    pub(crate) fn collect_paragraphs<'a>(&'a self, paragraphs: &mut Vec<&'a CT_P>) {
        for index in 0..=self.rows.len() {
            for (_, _, sdt) in self
                .content_controls
                .iter()
                .filter(|(at, _, _)| *at == index)
            {
                sdt.collect_paragraphs(SdtOwner::Table, paragraphs);
            }
            if let Some(row) = self.rows.get(index) {
                row.collect_paragraphs(paragraphs);
            }
        }
    }
}

impl Default for CT_Tbl {
    fn default() -> Self {
        Self::new()
    }
}

/// Modeled source attributes follow the concrete table/grid/row/cell readers.
pub(crate) fn source_modeled_attributes(
    parent: &[u8],
    local: &[u8],
) -> Option<&'static [&'static [u8]]> {
    Some(match (parent, local) {
        (
            b"tblPr" | b"tblPrEx",
            b"tblStyle"
            | b"tblOverlap"
            | b"bidiVisual"
            | b"jc"
            | b"tblCaption"
            | b"tblDescription"
            | b"tblStyleRowBandSize"
            | b"tblStyleColBandSize",
        ) => &[b"val"],
        (b"tblPr" | b"tblPrEx", b"tblpPr") => &[
            b"leftFromText",
            b"rightFromText",
            b"topFromText",
            b"bottomFromText",
            b"vertAnchor",
            b"horzAnchor",
            b"tblpX",
            b"tblpXSpec",
            b"tblpY",
            b"tblpYSpec",
        ],
        (b"tblPr" | b"tblPrEx", b"tblW" | b"tblInd" | b"tblCellSpacing")
        | (b"trPr", b"wBefore" | b"wAfter" | b"tblCellSpacing")
        | (b"tcPr", b"tcW") => &[b"w", b"type"],
        (b"tblPr" | b"tblPrEx", b"tblLayout") => &[b"type"],
        (b"tblPr" | b"tblPrEx", b"tblLook") => &[
            b"val",
            b"firstRow",
            b"lastRow",
            b"firstColumn",
            b"lastColumn",
            b"noHBand",
            b"noVBand",
        ],
        (b"tblPr" | b"tblPrEx", b"shd") | (b"tcPr", b"shd") => {
            crate::properties::source_shading_attributes()
        }
        (b"tblPr" | b"tblPrEx", b"tblBorders" | b"tblCellMar" | b"tblPrChange")
        | (b"tcPr", b"tcBorders" | b"tcMar") => &[],
        (b"tblGrid", b"gridCol") => &[b"w"],
        (b"trPr", b"trHeight") => &[b"val", b"hRule"],
        (
            b"trPr",
            b"tblHeader" | b"jc" | b"gridBefore" | b"gridAfter" | b"hidden" | b"cnfStyle"
            | b"cantSplit",
        ) => &[b"val"],
        (
            b"tcPr",
            b"gridSpan" | b"hMerge" | b"vMerge" | b"vAlign" | b"cnfStyle" | b"noWrap"
            | b"textDirection",
        ) => &[b"val"],
        (b"tblCellMar" | b"tcMar", b"top" | b"left" | b"start" | b"bottom" | b"right" | b"end") => {
            &[b"w", b"type"]
        }
        (
            b"tblBorders" | b"tcBorders",
            b"top" | b"left" | b"start" | b"bottom" | b"right" | b"end" | b"insideH" | b"insideV",
        ) => crate::properties::source_border_attributes(),
        _ => return None,
    })
}

pub(crate) fn source_value_is_modeled(
    parent: &[u8],
    local: &[u8],
    key: &[u8],
    value: &str,
) -> bool {
    if parent == b"tcPr" && local == b"vAlign" && key == b"val" {
        return ST_VerticalJc::from_str(value).to_str() == value;
    }
    if parent == b"tcPr" && local == b"vMerge" && key == b"val" {
        return matches!(value, "restart" | "continue");
    }
    if key == b"val" && local == b"jc" && matches!(parent, b"tblPr" | b"tblPrEx" | b"trPr") {
        return ST_Jc::from_str(value).is_ok();
    }
    if key == b"val" && local == b"tblOverlap" {
        return ST_TblOverlap::parse(value).is_some();
    }
    if key == b"val"
        && matches!(
            (parent, local),
            (b"tblPr" | b"tblPrEx", b"bidiVisual")
                | (b"trPr", b"tblHeader" | b"hidden" | b"cantSplit")
                | (b"tcPr", b"noWrap")
        )
    {
        return parse_ooxml_bool(value).is_some();
    }
    if local == b"tblLook" && key != b"val" {
        return parse_ooxml_bool(value).is_some();
    }
    if local == b"tblpPr" {
        if matches!(key, b"horzAnchor" | b"vertAnchor") {
            return ST_TblAnchor::parse(value).is_some();
        }
        if key == b"tblpXSpec" {
            return AnchorAlignH::parse(value).is_some();
        }
        if key == b"tblpYSpec" {
            return ST_YAlign::parse(value).is_some();
        }
    }
    true
}

#[cfg(test)]
mod empty_property_tests {
    use super::*;

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    fn read(xml: &str) -> CT_Tbl {
        CT_Tbl::from_xml_fragment_checked(xml.as_bytes()).unwrap()
    }

    /// One row of one cell, with these row and cell properties.
    fn table(row: &str, cell: &str) -> CT_Tbl {
        read(&format!(
            r#"<w:tbl {W}><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr>{row}<w:tc>{cell}<w:p/></w:tc></w:tr></w:tbl>"#
        ))
    }

    /// The table written, then read again.
    fn reread(table: &CT_Tbl) -> CT_Tbl {
        let mut writer = Writer::new(Vec::new());
        table.to_xml(&mut writer).unwrap();
        let xml = String::from_utf8(writer.into_inner()).unwrap();
        read(&xml.replacen("<w:tbl", &format!("<w:tbl {W}"), 1))
    }

    #[test]
    fn row_and_cell_properties_read_back_as_the_model_that_wrote_them() {
        // Empty, as Google Docs writes `w:tcPr` and LibreOffice `w:trPr`.
        let empty = table("<w:trPr/>", "<w:tcPr></w:tcPr>");
        assert_eq!(reread(&empty), empty);
        assert_ne!(empty, table("", ""), "an empty element is not no element");

        // Read empty, then given a value.
        let mut edited = empty.clone();
        edited.rows[0].properties.as_mut().unwrap().cant_split = Some(true);
        edited.rows[0].cells[0].properties.as_mut().unwrap().v_align = Some(ST_VerticalJc::Center);
        assert_eq!(reread(&edited), edited);

        // Built with a value: the same properties, written the same way.
        let mut built = table("", "");
        built.rows[0].properties = Some(CT_TrPr {
            cant_split: Some(true),
            ..CT_TrPr::default()
        });
        built.rows[0].cells[0].properties = Some(CT_TcPr {
            v_align: Some(ST_VerticalJc::Center),
            ..CT_TcPr::default()
        });
        assert_eq!(reread(&built), built);
        assert_eq!(built, edited);
    }

    #[test]
    fn border_containers_read_empty_are_written_and_built_empty_are_not() {
        let read_empty = table("", "<w:tcPr><w:tcBorders></w:tcBorders></w:tcPr>");
        assert_eq!(reread(&read_empty), read_empty);
        let self_closed = table("", "<w:tcPr><w:tcBorders/></w:tcPr>");
        assert_eq!(self_closed, read_empty);
        assert_ne!(read_empty, table("", "<w:tcPr/>"));

        // Built through the API without an edge: nothing to write.
        let mut built = table("", "<w:tcPr/>");
        built.rows[0].cells[0].properties.as_mut().unwrap().borders =
            Some(CT_TblBorders::default());
        let mut writer = Writer::new(Vec::new());
        built.to_xml(&mut writer).unwrap();
        let xml = String::from_utf8(writer.into_inner()).unwrap();
        assert!(!xml.contains("tcBorders"), "{xml}");
        assert_eq!(reread(&built), table("", "<w:tcPr/>"));
    }
}
