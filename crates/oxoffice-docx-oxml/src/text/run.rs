//! The run model: run content, special characters, markers, CT_R and its raw writing.

use super::*;

/// Content that can appear inside a run.
#[derive(Debug, Clone, PartialEq)]
pub enum RunContent {
    Text(CT_Text),
    /// Deleted text projected from `w:delText` inside a deletion wrapper.
    DeletedText(CT_Text),
    Tab,
    /// A `w:br`: its kind, and where the text after it restarts beside
    /// floating objects (`w:clear`).
    Break(BreakType, BreakClear),
    Drawing(CT_Drawing),
    /// A simple or complex Word field.
    Field(Field),
    /// A footnote reference (`<w:footnoteReference w:id="..."/>`).
    FootnoteRef {
        id: i32,
        /// Custom mark displayed by this reference instead of a numeric label.
        custom_mark: Option<String>,
    },
    /// An endnote reference (`<w:endnoteReference w:id="..."/>`).
    EndnoteRef {
        id: i32,
        /// Custom mark displayed by this reference instead of a numeric label.
        custom_mark: Option<String>,
    },
    /// A comment reference (`<w:commentReference w:id="..."/>`).
    CommentReference {
        id: i32,
        /// Number of raw run children that precede this reference.
        raw_before: usize,
    },
    /// A symbol character from a specific font (`<w:sym w:font="..." w:char="..."/>`).
    Symbol {
        /// The font the code point is looked up in.
        font: String,
        /// The code point, written back as four upper-case hex digits.
        char_code: u16,
    },
    /// One of the Word special characters: a carriage return reads as a line
    /// break, a non-breaking hyphen as `-`, a positional tab as a tab and a
    /// soft hyphen as nothing.
    SpecialCharacter(SpecialCharacter),
}

/// A Word special character run child.
///
/// The four share one placement contract, so they share one `RunContent`
/// variant rather than taking one each across the ten files that match on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialCharacter {
    /// `<w:cr/>`, a carriage return that breaks the line.
    CarriageReturn,
    /// `<w:noBreakHyphen/>`, a hyphen that is not a break opportunity.
    NoBreakHyphen,
    /// `<w:softHyphen/>`, a hyphen that renders only at a line break.
    SoftHyphen,
    /// `<w:ptab/>`, an absolutely positioned tab.
    PositionalTab {
        alignment: ST_PTabAlignment,
        relative_to: ST_PTabRelativeTo,
        leader: ST_PTabLeader,
    },
}

/// `ST_PTabAlignment` — how content aligns against a positional tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ST_PTabAlignment {
    Left,
    Center,
    Right,
}

/// `ST_PTabRelativeTo` — what a positional tab position is measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ST_PTabRelativeTo {
    Margin,
    Indent,
}

/// `ST_PTabLeader` — the leader drawn across a positional tab gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ST_PTabLeader {
    None,
    Dot,
    Hyphen,
    Underscore,
    MiddleDot,
}

impl ST_PTabAlignment {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "left" => Some(ST_PTabAlignment::Left),
            "center" => Some(ST_PTabAlignment::Center),
            "right" => Some(ST_PTabAlignment::Right),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            ST_PTabAlignment::Left => "left",
            ST_PTabAlignment::Center => "center",
            ST_PTabAlignment::Right => "right",
        }
    }
}

impl ST_PTabRelativeTo {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "margin" => Some(ST_PTabRelativeTo::Margin),
            "indent" => Some(ST_PTabRelativeTo::Indent),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            ST_PTabRelativeTo::Margin => "margin",
            ST_PTabRelativeTo::Indent => "indent",
        }
    }
}

impl ST_PTabLeader {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "none" => Some(ST_PTabLeader::None),
            "dot" => Some(ST_PTabLeader::Dot),
            "hyphen" => Some(ST_PTabLeader::Hyphen),
            "underscore" => Some(ST_PTabLeader::Underscore),
            "middleDot" => Some(ST_PTabLeader::MiddleDot),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            ST_PTabLeader::None => "none",
            ST_PTabLeader::Dot => "dot",
            ST_PTabLeader::Hyphen => "hyphen",
            ST_PTabLeader::Underscore => "underscore",
            ST_PTabLeader::MiddleDot => "middleDot",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunChildAction {
    Raw,
    Properties,
    IgnoreEmptyProperties,
    Text,
    DeletedText,
    Drawing,
    CommentReference,
    AlternateContent,
    Pict,
    Tab,
    Break,
    FootnoteReference,
    EndnoteReference,
    Special,
}

impl RunChildAction {
    pub(crate) fn for_names(local: &[u8], empty: bool, word: bool, alternate: bool) -> Self {
        use RunChildAction as A;
        if alternate && !empty {
            return A::AlternateContent;
        }
        if !word {
            return A::Raw;
        }
        if empty {
            match local {
                b"rPr" => A::IgnoreEmptyProperties,
                b"tab" => A::Tab,
                b"br" => A::Break,
                b"footnoteReference" => A::FootnoteReference,
                b"endnoteReference" => A::EndnoteReference,
                b"commentReference" => A::CommentReference,
                b"sym" | b"ptab" | b"cr" | b"noBreakHyphen" | b"softHyphen" => A::Special,
                _ => A::Raw,
            }
        } else {
            match local {
                b"rPr" => A::Properties,
                b"t" => A::Text,
                b"delText" => A::DeletedText,
                b"drawing" => A::Drawing,
                b"commentReference" => A::CommentReference,
                b"pict" => A::Pict,
                _ => A::Raw,
            }
        }
    }

    fn for_element(start: &BytesStart<'_>, prefixes: &[String], empty: bool) -> Self {
        let name = start.name();
        let local = start.local_name();
        Self::for_names(
            local.as_ref(),
            empty,
            is_word_element(name.as_ref(), local.as_ref(), prefixes),
            is_element_in_namespace(
                name.as_ref(),
                b"AlternateContent",
                crate::namespace::MC_NS,
                prefixes,
            ),
        )
    }

    pub(crate) fn retains_raw(self, special_typed: bool) -> bool {
        matches!(self, Self::Raw | Self::AlternateContent | Self::Pict)
            || (self == Self::Special && !special_typed)
    }
}

/// The actual run text decoder, including its permissive entity fallback.
pub(crate) fn decode_run_text_span(encoded: &quick_xml::events::BytesText<'_>) -> Result<String> {
    encoded
        .decode()
        .map_err(|error| OxmlError::InvalidValue(error.to_string()))?;
    Ok(crate::xml_text::decode_escaped(encoded))
}

/// Ordered primitive decision for one actual run child occurrence.
/// Recursive constructors remain explicit actions, without a claimed outcome.
pub(crate) struct RunChildPlan {
    pub(crate) action: RunChildAction,
    pub(crate) leaf: Option<RunLeafValue>,
}
pub(crate) enum RunLeafValue {
    Tab,
    Break(BreakType, BreakClear),
    Footnote {
        id: i32,
        custom_mark: Option<String>,
    },
    Endnote {
        id: i32,
        custom_mark: Option<String>,
    },
    Comment(i32),
    Special(SpecialRunValue),
}
impl RunLeafValue {
    fn into_content(self, raw_before: usize) -> RunContent {
        match self {
            Self::Tab => RunContent::Tab,
            Self::Break(kind, clear) => RunContent::Break(kind, clear),
            Self::Footnote { id, custom_mark } => RunContent::FootnoteRef { id, custom_mark },
            Self::Endnote { id, custom_mark } => RunContent::EndnoteRef { id, custom_mark },
            Self::Comment(id) => RunContent::CommentReference { id, raw_before },
            Self::Special(value) => value.into_content(),
        }
    }
}
impl RunChildPlan {
    pub(crate) fn read(
        start: &BytesStart<'_>,
        prefixes: &[String],
        action: RunChildAction,
    ) -> Result<Self> {
        use RunChildAction as A;
        let leaf = match action {
            A::Tab => Some(RunLeafValue::Tab),
            A::Break => Some(RunLeafValue::Break(
                optional_word_attribute(start, b"type", prefixes)
                    .map(|value| match value.as_bytes() {
                        b"page" => BreakType::Page,
                        b"column" => BreakType::Column,
                        _ => BreakType::Line,
                    })
                    .unwrap_or(BreakType::Line),
                optional_word_attribute(start, b"clear", prefixes)
                    .map(|value| BreakClear::parse(&value))
                    .unwrap_or_default(),
            )),
            A::FootnoteReference | A::EndnoteReference => {
                let id = optional_word_attribute(start, b"id", prefixes)
                    .and_then(|value| value.parse::<i32>().ok())
                    .unwrap_or(0);
                let custom_mark = optional_word_attribute(start, b"customMarkFollows", prefixes)
                    .filter(|value| crate::shared::ST_OnOff::parse(value) == Some(true))
                    .map(|_| String::new());
                Some(if action == A::FootnoteReference {
                    RunLeafValue::Footnote { id, custom_mark }
                } else {
                    RunLeafValue::Endnote { id, custom_mark }
                })
            }
            A::CommentReference => Some(RunLeafValue::Comment(required_word_i32_attribute(
                start, b"id", prefixes,
            )?)),
            A::Special => {
                parse_typed_special_run_child(start, prefixes)?.map(RunLeafValue::Special)
            }
            _ => None,
        };
        Ok(Self { action, leaf })
    }
}

/// Decoded leaf scalars only. No recursive model or expanded namespace record.
pub(crate) enum SpecialRunValue {
    Symbol { font: String, char_code: u16 },
    Character(SpecialCharacter),
}
impl SpecialRunValue {
    fn into_content(self) -> RunContent {
        match self {
            Self::Symbol { font, char_code } => RunContent::Symbol { font, char_code },
            Self::Character(character) => RunContent::SpecialCharacter(character),
        }
    }
}

/// Read `<w:sym/>` as typed content.
///
/// A symbol whose `w:char` is not four hex digits, or that carries an
/// attribute outside the modeled pair, stays raw so nothing is lost.
fn parse_symbol(e: &BytesStart<'_>, prefixes: &[String]) -> Result<Option<SpecialRunValue>> {
    let mut font = None;
    let mut char_code = None;
    for attribute in e.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        let value = std::str::from_utf8(&attribute.value)?;
        if is_word_attribute(key, b"font", prefixes) {
            font = Some(oxoffice_core::xml::attribute_text(&attribute)?.into_owned());
        } else if is_word_attribute(key, b"char", prefixes) {
            if value.len() != 4 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Ok(None);
            }
            char_code = u16::from_str_radix(value, 16).ok();
        } else if !key.starts_with(b"xmlns") {
            return Ok(None);
        }
    }
    Ok(match (font, char_code) {
        (Some(font), Some(char_code)) => Some(SpecialRunValue::Symbol { font, char_code }),
        _ => None,
    })
}

/// Read `<w:ptab/>` as typed content.
///
/// All three attributes are required by the schema, so a positional tab
/// missing one, or carrying a token outside the inventory, stays raw.
fn parse_positional_tab(
    e: &BytesStart<'_>,
    prefixes: &[String],
) -> Result<Option<SpecialRunValue>> {
    let mut alignment = None;
    let mut relative_to = None;
    let mut leader = None;
    for attribute in e.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        let value = std::str::from_utf8(&attribute.value)?;
        if is_word_attribute(key, b"alignment", prefixes) {
            alignment = ST_PTabAlignment::from_str(value);
        } else if is_word_attribute(key, b"relativeTo", prefixes) {
            relative_to = ST_PTabRelativeTo::from_str(value);
        } else if is_word_attribute(key, b"leader", prefixes) {
            leader = ST_PTabLeader::from_str(value);
        } else if !key.starts_with(b"xmlns") {
            return Ok(None);
        }
    }
    Ok(match (alignment, relative_to, leader) {
        (Some(alignment), Some(relative_to), Some(leader)) => Some(SpecialRunValue::Character(
            SpecialCharacter::PositionalTab {
                alignment,
                relative_to,
                leader,
            },
        )),
        _ => None,
    })
}

/// Read `<w:cr/>`, `<w:noBreakHyphen/>` and `<w:softHyphen/>` as typed content.
///
/// These are `CT_Empty`, so an element carrying any attribute other than a
/// namespace declaration stays raw.
fn parse_empty_special_character(
    e: &BytesStart<'_>,
    prefixes: &[String],
) -> Result<Option<SpecialRunValue>> {
    let name = e.name();
    let character = if is_word_element(name.as_ref(), b"cr", prefixes) {
        SpecialCharacter::CarriageReturn
    } else if is_word_element(name.as_ref(), b"noBreakHyphen", prefixes) {
        SpecialCharacter::NoBreakHyphen
    } else if is_word_element(name.as_ref(), b"softHyphen", prefixes) {
        SpecialCharacter::SoftHyphen
    } else {
        return Ok(None);
    };
    for attribute in e.attributes() {
        if !attribute?.key.as_ref().starts_with(b"xmlns") {
            return Ok(None);
        }
    }
    Ok(Some(SpecialRunValue::Character(character)))
}

/// Read the typed run children, or `None` to keep the element raw.
pub(crate) fn parse_typed_special_run_child(
    e: &BytesStart<'_>,
    prefixes: &[String],
) -> Result<Option<SpecialRunValue>> {
    if is_word_element(e.name().as_ref(), b"sym", prefixes) {
        parse_symbol(e, prefixes)
    } else if is_word_element(e.name().as_ref(), b"ptab", prefixes) {
        parse_positional_tab(e, prefixes)
    } else {
        parse_empty_special_character(e, prefixes)
    }
}

fn write_special_character<W: std::io::Write>(
    writer: &mut Writer<W>,
    character: SpecialCharacter,
    word_prefix: &str,
    emission: Option<&FieldXmlEmission>,
) -> Result<()> {
    match character {
        SpecialCharacter::CarriageReturn => {
            FieldXmlStart::word(word_prefix, "cr", emission)?.write(writer, true)?;
        }
        SpecialCharacter::NoBreakHyphen => {
            FieldXmlStart::word(word_prefix, "noBreakHyphen", emission)?.write(writer, true)?;
        }
        SpecialCharacter::SoftHyphen => {
            FieldXmlStart::word(word_prefix, "softHyphen", emission)?.write(writer, true)?;
        }
        SpecialCharacter::PositionalTab {
            alignment,
            relative_to,
            leader,
        } => {
            let mut e = FieldXmlStart::word(word_prefix, "ptab", emission)?;
            e.push_word(word_prefix, "alignment", alignment.as_str())?;
            e.push_word(word_prefix, "relativeTo", relative_to.as_str())?;
            e.push_word(word_prefix, "leader", leader.as_str())?;
            e.write(writer, true)?;
        }
    }
    Ok(())
}

/// A typed comment range boundary at a run insertion point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentRangeMarker {
    Start {
        id: i32,
        run_index: usize,
        /// Number of raw children at this run boundary that precede the marker.
        raw_before: usize,
        /// Whether the source marker contained a child element or visible text.
        has_child_content: bool,
    },
    End {
        id: i32,
        run_index: usize,
        /// Number of raw children at this run boundary that precede the marker.
        raw_before: usize,
        /// Whether the source marker contained a child element or visible text.
        has_child_content: bool,
    },
}

impl PartialEq for BookmarkMarker {
    fn eq(&self, other: &Self) -> bool {
        self.start == other.start
            && self.id == other.id
            && self.name == other.name
            && self.run_index == other.run_index
            && self.raw_before == other.raw_before
            && self.projected_run_index == other.projected_run_index
            && self.tracked_run_index == other.tracked_run_index
            && self.has_child_content == other.has_child_content
    }
}

impl Eq for BookmarkMarker {}

impl std::fmt::Debug for BookmarkMarker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BookmarkMarker")
            .field("start", &self.start)
            .field("id", &self.id)
            .field("name", &self.name)
            .field("run_index", &self.run_index)
            .field("raw_before", &self.raw_before)
            .field("projected_run_index", &self.projected_run_index)
            .field("tracked_run_index", &self.tracked_run_index)
            .field("has_child_content", &self.has_child_content)
            .finish()
    }
}

impl BookmarkMarker {
    pub fn is_start(&self) -> bool {
        self.start
    }

    pub fn id(&self) -> Option<i32> {
        self.id
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn run_index(&self) -> usize {
        self.run_index
    }

    /// Number of preserved raw children before this marker at its run boundary.
    #[doc(hidden)]
    pub fn raw_before(&self) -> usize {
        self.raw_before
    }

    /// Whether the source marker contained a child element or visible text.
    pub fn has_child_content(&self) -> bool {
        self.has_child_content
    }

    /// Accepted-view run boundary represented by this marker.
    #[doc(hidden)]
    pub fn projected_run_index(&self) -> usize {
        self.projected_run_index
    }

    /// Tracked-view run boundary represented by this marker.
    #[doc(hidden)]
    pub fn tracked_run_index(&self) -> usize {
        self.tracked_run_index
    }
}

impl CommentRangeMarker {
    pub(super) fn run_index(&self) -> usize {
        match self {
            Self::Start { run_index, .. } | Self::End { run_index, .. } => *run_index,
        }
    }

    pub(super) fn raw_before(&self) -> usize {
        match self {
            Self::Start { raw_before, .. } | Self::End { raw_before, .. } => *raw_before,
        }
    }
}

/// Types of breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakType {
    Line,
    Page,
    Column,
}

/// `ST_BrClear`: where the text after a break restarts beside the floating
/// objects that wrap text. Only a line break uses it; a page or column break
/// keeps it for the file's sake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BreakClear {
    /// Absent or `none`: the text restarts on the next line.
    #[default]
    None,
    /// Below the floats on the left.
    Left,
    /// Below the floats on the right.
    Right,
    /// Below every float.
    All,
}

impl BreakClear {
    /// Read a `w:clear` value; an unknown value reads as `None`.
    pub fn parse(value: &str) -> Self {
        match value {
            "left" => Self::Left,
            "right" => Self::Right,
            "all" => Self::All,
            _ => Self::None,
        }
    }

    /// The `w:clear` value to write, `None` for none (written as absent).
    pub fn as_attribute(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Left => Some("left"),
            Self::Right => Some("right"),
            Self::All => Some("all"),
        }
    }
}

/// `CT_R` — A run of text with uniform formatting.
#[derive(Debug, Clone, PartialEq)]
#[allow(non_snake_case)]
pub struct CT_R {
    pub properties: Option<CT_RPr>,
    pub content: Vec<RunContent>,
    /// Unknown child elements captured as raw XML.
    pub extra_xml: Vec<Vec<u8>>,
    /// Encoded raw-child positions among properties and typed run content.
    #[doc(hidden)]
    pub extra_xml_positions: Vec<usize>,
    /// Drawings read out of an `mc:AlternateContent` block, for layout only.
    ///
    /// Never serialised. The verbatim copy in `extra_xml` is what gets
    /// written, so emitting these as well would duplicate the element.
    pub alt_drawings: Vec<CT_Drawing>,
}

const RAW_LEGACY_HORIZONTAL_RULE_FLAG: usize = 1usize << (usize::BITS - 1);
const RAW_ALTERNATE_CONTENT_DRAWING_FLAG: usize = 1usize << (usize::BITS - 2);
const RAW_ROOT_ATTRIBUTES_FLAG: usize = 1usize << (usize::BITS - 3);
pub(super) const RAW_COMMENT_REFERENCE_FLAG: usize = 1usize << (usize::BITS - 4);
const RAW_RUN_LOCAL_BINDINGS_FLAG: usize = 1usize << (usize::BITS - 5);
const RAW_CHILD_FLAGS: usize = RAW_LEGACY_HORIZONTAL_RULE_FLAG
    | RAW_ALTERNATE_CONTENT_DRAWING_FLAG
    | RAW_ROOT_ATTRIBUTES_FLAG
    | RAW_COMMENT_REFERENCE_FLAG
    | RAW_RUN_LOCAL_BINDINGS_FLAG;
const RAW_CHILD_POSITION_MASK: usize = !RAW_CHILD_FLAGS;
const VML_NAMESPACE: &[u8] = b"urn:schemas-microsoft-com:vml";
const OFFICE_NAMESPACE: &[u8] = b"urn:schemas-microsoft-com:office:office";

fn resolved_name_matches(
    namespace: &ResolveResult<'_>,
    local_name: &[u8],
    expected_namespace: &[u8],
    expected_local_name: &[u8],
) -> bool {
    local_name == expected_local_name
        && matches!(namespace, ResolveResult::Bound(Namespace(uri)) if *uri == expected_namespace)
}

fn rect_has_enabled_horizontal_rule(reader: &NsReader<&[u8]>, element: &BytesStart<'_>) -> bool {
    let mut enabled = None;
    for attribute in element.attributes() {
        let Ok(attribute) = attribute else {
            return false;
        };
        let (namespace, local_name) = reader.resolver().resolve_attribute(attribute.key);
        if resolved_name_matches(&namespace, local_name.as_ref(), OFFICE_NAMESPACE, b"hr") {
            if enabled.is_some() {
                return false;
            }
            let Ok(value) =
                attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
            else {
                return false;
            };
            enabled = Some(matches!(value.as_bytes(), b"t" | b"true"));
        }
    }
    enabled == Some(true)
}

fn is_legacy_horizontal_rule(raw_xml: &[u8], inherited_namespaces: &[(String, String)]) -> bool {
    let scoped_xml = if inherited_namespaces.is_empty() {
        None
    } else {
        let mut wrapper = BytesStart::new("rdocx-scope");
        let names = inherited_namespaces
            .iter()
            .map(|(prefix, _)| {
                if prefix.is_empty() {
                    "xmlns".to_owned()
                } else {
                    format!("xmlns:{prefix}")
                }
            })
            .collect::<Vec<_>>();
        for ((_, namespace), name) in inherited_namespaces.iter().zip(&names) {
            wrapper.push_attribute((name.as_str(), namespace.as_str()));
        }
        let mut writer = Writer::new(Vec::new());
        if writer.write_event(Event::Start(wrapper)).is_err() {
            return false;
        }
        writer.get_mut().extend_from_slice(raw_xml);
        if writer
            .write_event(Event::End(BytesEnd::new("rdocx-scope")))
            .is_err()
        {
            return false;
        }
        Some(writer.into_inner())
    };
    let mut reader = NsReader::from_reader(scoped_xml.as_deref().unwrap_or(raw_xml));
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut state = u8::from(scoped_xml.is_some()) * 4;
    let mut found_rect = false;

    loop {
        let Ok((namespace, event)) = reader.read_resolved_event_into(&mut buffer) else {
            return false;
        };
        match event {
            Event::Start(element) if state == 4 && element.name().as_ref() == b"rdocx-scope" => {
                state = 0;
            }
            Event::Start(element) if state == 0 => {
                if !resolved_name_matches(
                    &namespace,
                    element.local_name().as_ref(),
                    crate::namespace::W_NS.as_bytes(),
                    b"pict",
                ) {
                    return false;
                }
                state = 1;
            }
            Event::Start(element) if state == 1 && !found_rect => {
                if !resolved_name_matches(
                    &namespace,
                    element.local_name().as_ref(),
                    VML_NAMESPACE,
                    b"rect",
                ) || !rect_has_enabled_horizontal_rule(&reader, &element)
                {
                    return false;
                }
                found_rect = true;
                state = 2;
            }
            Event::Empty(element) if state == 1 && !found_rect => {
                if !resolved_name_matches(
                    &namespace,
                    element.local_name().as_ref(),
                    VML_NAMESPACE,
                    b"rect",
                ) || !rect_has_enabled_horizontal_rule(&reader, &element)
                {
                    return false;
                }
                found_rect = true;
            }
            Event::End(element) if state == 2 => {
                if !resolved_name_matches(
                    &namespace,
                    element.local_name().as_ref(),
                    VML_NAMESPACE,
                    b"rect",
                ) {
                    return false;
                }
                state = 1;
            }
            Event::End(element) if state == 1 && found_rect => {
                if !resolved_name_matches(
                    &namespace,
                    element.local_name().as_ref(),
                    crate::namespace::W_NS.as_bytes(),
                    b"pict",
                ) {
                    return false;
                }
                state = 3;
            }
            Event::End(element)
                if state == 3
                    && scoped_xml.is_some()
                    && element.name().as_ref() == b"rdocx-scope" =>
            {
                state = 4;
            }
            Event::Text(text) if text.as_ref().iter().all(u8::is_ascii_whitespace) => {}
            Event::Eof => {
                let finished = if scoped_xml.is_some() {
                    state == 4
                } else {
                    state == 3
                };
                return finished && found_rect;
            }
            _ => return false,
        }
        buffer.clear();
    }
}

#[allow(non_snake_case)]
impl CT_R {
    pub fn new(text: &str) -> Self {
        CT_R {
            properties: None,
            content: vec![RunContent::Text(CT_Text::new(text))],
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
            alt_drawings: Vec::new(),
        }
    }

    pub(crate) fn from_empty_root(root: &BytesStart<'_>, word_prefixes: &[String]) -> Result<Self> {
        let mut run = Self {
            properties: None,
            content: Vec::new(),
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
            alt_drawings: Vec::new(),
        };
        if let Some(record) = capture_root_attribute_record(root, word_prefixes)? {
            run.extra_xml.push(record);
            run.extra_xml_positions.push(RAW_ROOT_ATTRIBUTES_FLAG);
        }
        Ok(run)
    }

    /// Decode a raw-child boundary stored in `extra_xml_positions`.
    #[doc(hidden)]
    pub fn raw_child_position(encoded: usize) -> usize {
        encoded & RAW_CHILD_POSITION_MASK
    }

    /// Report whether a raw child was parsed as a legacy horizontal rule.
    #[doc(hidden)]
    pub fn raw_child_is_legacy_horizontal_rule(encoded: usize) -> bool {
        encoded & RAW_LEGACY_HORIZONTAL_RULE_FLAG != 0
    }

    /// Report whether a raw carrier retains attributes from the run root.
    #[doc(hidden)]
    pub fn raw_child_is_root_attributes(encoded: usize) -> bool {
        encoded & RAW_ROOT_ATTRIBUTES_FLAG != 0
    }

    /// Report whether this raw slot has one selected drawing projection.
    #[doc(hidden)]
    pub fn raw_child_has_alternate_content_drawing(encoded: usize) -> bool {
        encoded & RAW_ALTERNATE_CONTENT_DRAWING_FLAG != 0
    }

    /// Replace a raw-child boundary without changing its parsed classification.
    #[doc(hidden)]
    pub fn set_raw_child_position(encoded: &mut usize, position: usize) {
        debug_assert_eq!(position & RAW_CHILD_FLAGS, 0);
        *encoded = (position & RAW_CHILD_POSITION_MASK) | (*encoded & RAW_CHILD_FLAGS);
    }

    fn encode_raw_child_position(
        position: usize,
        legacy_horizontal_rule: bool,
        alternate_content_drawing: bool,
    ) -> usize {
        debug_assert_eq!(position & RAW_CHILD_FLAGS, 0);
        position
            | if legacy_horizontal_rule {
                RAW_LEGACY_HORIZONTAL_RULE_FLAG
            } else {
                0
            }
            | if alternate_content_drawing {
                RAW_ALTERNATE_CONTENT_DRAWING_FLAG
            } else {
                0
            }
    }

    /// Get the combined text of all text content in this run, as stored.
    ///
    /// A raw reader: it reads text, tabs, breaks and the result of a complex
    /// field, but leaves out the result of every simple field (`w:fldSimple`),
    /// parsed from a file or added with the facade. Layout offsets and field
    /// result caches count on that (#150 lists the callers).
    /// [`Self::accepted_text`] reads the text a reader of the document sees.
    pub fn text(&self) -> String {
        let mut text = String::new();
        for content in &self.content {
            match content {
                RunContent::Field(field) => {
                    if let Some(projected) = field.effective_projected_text() {
                        text.push_str(&projected);
                    }
                }
                other => text.push_str(Self::content_text(other)),
            }
        }
        text
    }

    /// Get the text a reader of the document sees in this run: that of
    /// [`Self::text`], with the result of every field, simple ones included,
    /// as `CT_P::accepted_text` reads the run.
    #[doc(hidden)]
    pub fn accepted_text(&self) -> String {
        let mut text = String::new();
        super::append_current_run_text(self, &mut text);
        text
    }

    /// The text one content item contributes to [`Self::text`].
    pub(super) fn content_text(content: &RunContent) -> &str {
        match content {
            RunContent::Text(t) | RunContent::DeletedText(t) => &t.text,
            RunContent::Tab => "\t",
            RunContent::Break(..) => "\n",
            RunContent::Drawing(_) => "", // Drawings have no text content
            RunContent::Field(field) => field.projected_text().unwrap_or(""),
            RunContent::FootnoteRef { custom_mark, .. }
            | RunContent::EndnoteRef { custom_mark, .. } => custom_mark.as_deref().unwrap_or(""),
            RunContent::CommentReference { .. } => "",
            // A symbol is font-encoded rather than Unicode, so it contributes
            // nothing. A non-breaking hyphen and a positional tab read as Word
            // shows them, a hyphen and a tab; a soft hyphen shows only at a
            // line end and reads as nothing.
            RunContent::Symbol { .. } => "",
            RunContent::SpecialCharacter(SpecialCharacter::CarriageReturn) => "\n",
            RunContent::SpecialCharacter(SpecialCharacter::NoBreakHyphen) => "-",
            RunContent::SpecialCharacter(SpecialCharacter::PositionalTab { .. }) => "\t",
            RunContent::SpecialCharacter(_) => "",
        }
    }

    /// The literal text that contributes to a public split offset.
    pub(super) fn literal_text(content: &RunContent) -> &str {
        match content {
            RunContent::Text(text) | RunContent::DeletedText(text) => &text.text,
            RunContent::Tab
            | RunContent::Break(..)
            | RunContent::Drawing(_)
            | RunContent::Field(_)
            | RunContent::FootnoteRef { .. }
            | RunContent::EndnoteRef { .. }
            | RunContent::CommentReference { .. }
            | RunContent::Symbol { .. }
            | RunContent::SpecialCharacter(_) => "",
        }
    }

    pub(super) fn literal_len(&self) -> usize {
        self.content
            .iter()
            .map(Self::literal_text)
            .map(str::chars)
            .map(Iterator::count)
            .sum()
    }

    /// Where a split at a literal `offset` with `edge` cuts this run, or `None`
    /// when one side would show nothing, as at either end of the run for
    /// [`SplitEdge::Plain`].
    pub(crate) fn split_point(&self, offset: usize, edge: SplitEdge) -> Option<RunCut> {
        let len = self.literal_len();
        let at_edge = match edge {
            SplitEdge::Plain => false,
            SplitEdge::SpanStart => offset == 0,
            SplitEdge::SpanEnd => offset == len,
        };
        if offset > len || ((offset == 0 || offset == len) && !at_edge) {
            return None;
        }
        // A cut between items falls in the gap `[gap_start, gap_end)` of
        // text-free items between the text before it and the text after it.
        let mut end = 0;
        let mut gap_start = 0;
        let mut gap_end = self.content.len();
        for (index, content) in self.content.iter().enumerate() {
            let width = Self::literal_text(content).chars().count();
            if width == 0 {
                continue;
            }
            if end >= offset {
                gap_end = index;
                break;
            }
            if end + width > offset {
                return Some(RunCut::Inside {
                    index,
                    within: offset - end,
                });
            }
            gap_start = index + 1;
            end += width;
        }
        // The first run keeps the content items before `index`, and the raw
        // children up to `head_raw` content items.
        let (index, head_raw) = match edge {
            SplitEdge::SpanStart => (gap_end, gap_end),
            SplitEdge::Plain | SplitEdge::SpanEnd => (gap_start, gap_start.saturating_sub(1)),
        };
        let properties = usize::from(self.properties.is_some());
        let positioned = self.extra_xml_positions.len() == self.extra_xml.len();
        let shown_raw = |in_first: bool| {
            self.extra_xml.iter().enumerate().any(|(at, raw)| {
                let encoded = positioned.then(|| self.extra_xml_positions[at]);
                // Raw children without positions stay in the first run.
                let first = encoded.is_none_or(|encoded| {
                    Self::raw_child_position(encoded) <= properties + head_raw
                });
                first == in_first && !Self::raw_child_shows_nothing(raw, encoded)
            })
        };
        let head_shows = index > 0 || shown_raw(true);
        let tail_shows = index < self.content.len() || shown_raw(false);
        (head_shows && tail_shows).then_some(RunCut::Between { index, head_raw })
    }

    /// Whether a raw run child shows nothing in the paragraph: a record of run
    /// root attributes or namespace bindings, a last rendered page break, or a
    /// comment reference. `encoded` is its entry of `extra_xml_positions`.
    #[doc(hidden)]
    pub fn raw_child_shows_nothing(raw: &[u8], encoded: Option<usize>) -> bool {
        if encoded.is_some_and(|encoded| {
            encoded
                & (RAW_ROOT_ATTRIBUTES_FLAG
                    | RAW_RUN_LOCAL_BINDINGS_FLAG
                    | RAW_COMMENT_REFERENCE_FLAG)
                != 0
        }) {
            return true;
        }
        let name = raw
            .strip_prefix(b"<")
            .unwrap_or(raw)
            .split(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>'))
            .next()
            .unwrap_or_default();
        let local = name.rsplit(|byte| *byte == b':').next().unwrap_or_default();
        matches!(
            local,
            b"lastRenderedPageBreak" | b"commentReference" | b"rdocxRootAttributes"
        )
    }

    /// Split this run at a Unicode scalar offset of its literal text.
    ///
    /// This run keeps the content before `offset`, and the returned run, with
    /// cloned properties, receives the rest. `edge` says where the text-free
    /// content at the cut goes (tabs, breaks, fields, drawings, raw children).
    /// Nothing changes on error.
    pub(super) fn split_off_at(
        &mut self,
        offset: usize,
        edge: SplitEdge,
    ) -> std::result::Result<CT_R, RunSplitError> {
        let len = self.literal_len();
        let Some(cut) = self.split_point(offset, edge) else {
            return Err(RunSplitError::OffsetOutOfRange { offset, len });
        };
        let (tail_content, removed_prefix_content, head_boundary) = match cut {
            RunCut::Inside { index, within } => {
                let rest = match &mut self.content[index] {
                    RunContent::Text(text) => RunContent::Text(text.split_off_at(within)),
                    RunContent::DeletedText(text) => {
                        RunContent::DeletedText(text.split_off_at(within))
                    }
                    _ => unreachable!("an interior literal offset falls inside literal text"),
                };
                let mut tail = self.content.split_off(index + 1);
                tail.insert(0, rest);
                (tail, index, index)
            }
            RunCut::Between { index, head_raw } => (self.content.split_off(index), index, head_raw),
        };
        let mut tail = CT_R {
            properties: self.properties.clone(),
            content: tail_content,
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
            alt_drawings: Vec::new(),
        };
        // A raw child position counts the modeled children before it, so a
        // child after the first `head_boundary` content items moves to the tail.
        let head_boundary = usize::from(self.properties.is_some()) + head_boundary;
        if self.extra_xml_positions.len() == self.extra_xml.len() {
            let raw_children = std::mem::take(&mut self.extra_xml);
            let positions = std::mem::take(&mut self.extra_xml_positions);
            let mut alternate_drawings = std::mem::take(&mut self.alt_drawings).into_iter();
            for (raw, mut encoded) in raw_children.into_iter().zip(positions) {
                let position = Self::raw_child_position(encoded);
                let alternate_drawing = Self::raw_child_has_alternate_content_drawing(encoded)
                    .then(|| alternate_drawings.next())
                    .flatten();
                if encoded & RAW_RUN_LOCAL_BINDINGS_FLAG != 0
                    || (Self::raw_child_is_root_attributes(encoded)
                        && (raw.starts_with(b"<rdocxRootAttributes xmlns:")
                            || raw.starts_with(b"<rdocxRootAttributes xmlns=")))
                {
                    // Namespace-only records are constructed with declarations
                    // first. Real root-attribute records put their payload first.
                    // Scope follows both pieces without duplicating owner attrs.
                    self.extra_xml.push(raw.clone());
                    self.extra_xml_positions.push(encoded);
                    tail.extra_xml.push(raw);
                    tail.extra_xml_positions.push(encoded);
                    continue;
                }
                if position <= head_boundary {
                    self.extra_xml.push(raw);
                    self.extra_xml_positions.push(encoded);
                    self.alt_drawings.extend(alternate_drawing);
                } else {
                    Self::set_raw_child_position(&mut encoded, position - removed_prefix_content);
                    tail.extra_xml.push(raw);
                    tail.extra_xml_positions.push(encoded);
                    tail.alt_drawings.extend(alternate_drawing);
                }
            }
            self.alt_drawings.extend(alternate_drawings);
        }
        let head_raw_children = self.extra_xml.len();
        for content in &mut tail.content {
            if let RunContent::CommentReference { raw_before, .. } = content {
                *raw_before = raw_before.saturating_sub(head_raw_children);
            }
        }
        Ok(tail)
    }

    /// Replace typed run content while retaining every raw child boundary.
    #[doc(hidden)]
    pub fn replace_content(&mut self, content: Vec<RunContent>) {
        self.retain_comment_reference_sources(None);
        let property_boundary = usize::from(self.properties.is_some());
        let replacement_end = property_boundary + content.len();
        for position in &mut self.extra_xml_positions {
            if Self::raw_child_position(*position) > property_boundary {
                Self::set_raw_child_position(position, replacement_end);
            }
        }
        self.content = content;
    }

    /// Append typed content without moving raw children after existing content.
    #[doc(hidden)]
    pub fn append_content(&mut self, content: RunContent) {
        self.content.push(content);
    }

    /// Materialize run properties in their required first-child position.
    #[doc(hidden)]
    pub fn ensure_properties(&mut self) -> &mut CT_RPr {
        if self.properties.is_none() {
            for position in &mut self.extra_xml_positions {
                let shifted = Self::raw_child_position(*position) + 1;
                Self::set_raw_child_position(position, shifted);
            }
            self.properties = Some(CT_RPr::default());
        }
        self.properties.as_mut().expect("properties were inserted")
    }

    /// Remap raw boundaries after selected typed content children are removed.
    #[doc(hidden)]
    pub fn remap_removed_content(&mut self, removed: &[bool]) {
        let property_boundary = usize::from(self.properties.is_some());
        self.retain_comment_reference_sources(Some(removed));
        for position in &mut self.extra_xml_positions {
            let decoded = Self::raw_child_position(*position);
            let content_boundary = decoded.saturating_sub(property_boundary);
            let remapped = decoded.saturating_sub(
                removed
                    .iter()
                    .take(content_boundary.min(removed.len()))
                    .filter(|remove| **remove)
                    .count(),
            );
            Self::set_raw_child_position(position, remapped);
        }
    }

    fn retain_comment_reference_sources(&mut self, removed: Option<&[bool]>) {
        if self.extra_xml.len() != self.extra_xml_positions.len() {
            return;
        }
        let property_boundary = usize::from(self.properties.is_some());
        let keep = self
            .extra_xml_positions
            .iter()
            .map(|encoded| {
                *encoded & RAW_COMMENT_REFERENCE_FLAG == 0
                    || removed.is_some_and(|removed| {
                        !removed
                            .get(
                                Self::raw_child_position(*encoded)
                                    .saturating_sub(property_boundary),
                            )
                            .copied()
                            .unwrap_or(true)
                    })
            })
            .collect::<Vec<_>>();
        let mut index = 0;
        self.extra_xml.retain(|_| {
            let retain = keep[index];
            index += 1;
            retain
        });
        let mut index = 0;
        self.extra_xml_positions.retain(|_| {
            let retain = keep[index];
            index += 1;
            retain
        });
    }

    /// Remove selected comment-reference content and retain surrounding raw XML.
    #[doc(hidden)]
    pub fn remove_comment_references(&mut self, ids: &[i32]) -> bool {
        let removed = self
            .content
            .iter()
            .map(|content| {
                matches!(content, RunContent::CommentReference { id, .. } if ids.contains(id))
            })
            .collect::<Vec<_>>();
        let removed_any = removed.iter().any(|remove| *remove);
        if removed_any {
            self.remap_removed_content(&removed);
            self.content = self
                .content
                .drain(..)
                .zip(removed)
                .filter_map(|(content, remove)| (!remove).then_some(content))
                .collect();
        }
        removed_any
    }

    pub fn from_xml(reader: &mut Reader<&[u8]>) -> Result<Self> {
        crate::document::admit_word_reader(reader, &["w".to_owned()], b"r", None)?;
        Self::from_xml_with_prefixes(
            reader,
            &["w".to_owned(), format!("\0mc\0{}", crate::namespace::MC_NS)],
        )
    }

    /// Parse one namespace-complete regular run for checked owner replacement.
    #[doc(hidden)]
    pub fn from_xml_fragment_checked(xml: &[u8]) -> Result<Self> {
        crate::document::admit_word_xml(xml, &[], None)?;
        validate_word_owner_fragment(xml, b"r", true)?;
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(false);
        loop {
            match reader.read_event()? {
                Event::Start(root) => {
                    let prefixes = word_prefixes_at(&root, &[])?;
                    return Self::from_xml_with_prefixes_and_root(
                        &mut reader,
                        &prefixes,
                        Some(&root),
                        None,
                    );
                }
                Event::Empty(root) => {
                    let prefixes = word_prefixes_at(&root, &[])?;
                    return Self::from_empty_root(&root, &prefixes);
                }
                Event::Eof => return Err(OxmlError::MissingElement("w:r root".into())),
                _ => {}
            }
        }
    }

    pub(crate) fn from_xml_with_prefixes(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
    ) -> Result<Self> {
        Self::from_xml_with_prefixes_and_root(reader, word_prefixes, None, None)
    }

    pub(crate) fn from_xml_with_prefixes_and_root(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        root: Option<&BytesStart<'_>>,
        text_facts: Option<&mut Vec<ParsedWordText>>,
    ) -> Result<Self> {
        Self::from_xml_with_prefixes_and_root_in(reader, word_prefixes, root, text_facts, None)
    }

    pub(crate) fn from_xml_with_prefixes_and_root_in(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        root: Option<&BytesStart<'_>>,
        text_facts: Option<&mut Vec<ParsedWordText>>,
        cursor: Option<&crate::document::ControlSourceCursor<'_>>,
    ) -> Result<Self> {
        Self::from_xml_with_property_observation(
            reader,
            word_prefixes,
            root,
            text_facts,
            cursor,
            None,
        )
    }

    pub(crate) fn from_xml_with_property_observation(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        root: Option<&BytesStart<'_>>,
        mut text_facts: Option<&mut Vec<ParsedWordText>>,
        cursor: Option<&crate::document::ControlSourceCursor<'_>>,
        mut ordinary: Option<&mut Option<crate::document::WordOrdinaryFact>>,
    ) -> Result<Self> {
        if let Some(output) = &mut ordinary {
            **output = None;
        }
        let mut selected_property = None;
        if let Some(facts) = &mut text_facts {
            facts.clear();
        }
        let mut properties = None;
        let mut content = Vec::new();
        let mut extra_xml = Vec::new();
        let mut extra_xml_positions = Vec::new();
        let mut alt_drawings = Vec::new();
        let mut modeled_children = 0usize;
        // Whether the next `w:t` is the custom mark of the reference just
        // read. Only whitespace, XML comments, processing instructions and a
        // rendered page break may come between them, as the note removal
        // reads it.
        let mut mark_open = false;
        let mut buf = Vec::new();
        loop {
            let event_start = ordinary.as_ref().map(|_| reader.buffer_position());
            let mark_was_open = std::mem::take(&mut mark_open);
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    let plan = RunChildPlan::read(
                        e,
                        &prefixes,
                        RunChildAction::for_element(e, &prefixes, false),
                    )?;
                    let action = plan.action;
                    if action == RunChildAction::Properties {
                        let (raw, mapped) =
                            if let (Some(cursor), Some(start)) = (cursor, event_start) {
                                cursor.capture_element(reader, e, start)?
                            } else {
                                (capture_element(reader, e)?, None)
                            };
                        let mut observed = None;
                        let parsed = crate::numbering::parse_scoped_rpr_with_observation(
                            &raw,
                            word_prefixes,
                            &[],
                            mapped.as_ref().map(|_| &mut observed),
                        )?;
                        // Properties the writer writes nothing for, such as
                        // LibreOffice's `<w:rPr></w:rPr>`, read as the
                        // self-closing `<w:rPr/>` skipped below: no formatting,
                        // and no child slot the writer would not write.
                        if !parsed.writes_nothing() {
                            properties = Some(parsed);
                            // The later successful rPr assignment replaces its
                            // own observation, including a new unproved property.
                            selected_property = observed
                                .and_then(|range| mapped.as_ref()?.property_observation(range));
                            modeled_children += 1;
                        }
                    } else if action == RunChildAction::Text {
                        let preserve = e.attributes().any(|a| {
                            a.ok()
                                .map(|a| {
                                    a.key.as_ref() == b"xml:space"
                                        && a.value.as_ref() == b"preserve"
                                })
                                .unwrap_or(false)
                        });
                        // `read_text` returns the raw markup span, so entity
                        // references in it still need resolving.
                        let encoded_start = reader.buffer_position() as usize;
                        let encoded = reader.read_text(name)?;
                        let encoded_range = encoded_start..encoded_start + encoded.as_ref().len();
                        let text = decode_run_text_span(&encoded)?;
                        match content.last_mut() {
                            Some(RunContent::FootnoteRef {
                                custom_mark: Some(mark),
                                ..
                            })
                            | Some(RunContent::EndnoteRef {
                                custom_mark: Some(mark),
                                ..
                            }) if mark_was_open => *mark = text,
                            _ => {
                                if let Some(facts) = &mut text_facts {
                                    facts.push(ParsedWordText {
                                        drawing: None,
                                        content: content.len(),
                                        encoded: encoded_range,
                                    });
                                }
                                content.push(RunContent::Text(CT_Text {
                                    text,
                                    preserve_space: preserve,
                                }));
                            }
                        }
                        modeled_children += 1;
                    } else if action == RunChildAction::DeletedText {
                        let preserve = e.attributes().any(|a| {
                            a.ok().is_some_and(|a| {
                                a.key.as_ref() == b"xml:space" && a.value.as_ref() == b"preserve"
                            })
                        });
                        let encoded = reader.read_text(name)?;
                        let text = decode_run_text_span(&encoded)?;
                        content.push(RunContent::DeletedText(CT_Text {
                            text,
                            preserve_space: preserve,
                        }));
                        modeled_children += 1;
                    } else if action == RunChildAction::Drawing {
                        let mut drawing_maps = crate::drawing::DrawingSourceMaps::default();
                        let drawing = CT_Drawing::from_xml_with_prefixes_in(
                            reader,
                            &prefixes,
                            word_prefixes,
                            cursor,
                            text_facts.as_ref().map(|_| &mut drawing_maps),
                        )?;
                        if let Some(facts) = &mut text_facts {
                            facts.push(ParsedWordText {
                                content: content.len(),
                                encoded: 0..0,
                                drawing: Some(drawing_maps),
                            });
                        }
                        content.push(RunContent::Drawing(drawing));
                        modeled_children += 1;
                    } else if action == RunChildAction::CommentReference {
                        let raw = capture_element(reader, e)?;
                        let mut source = Reader::from_reader(raw.as_slice());
                        source.read_event()?;
                        let has_payload = !matches!(source.read_event()?, Event::End(_));
                        if has_payload || comment_reference_has_payload_attributes(e, &prefixes) {
                            extra_xml.push(close_comment_reference_source(&raw, &prefixes)?);
                            extra_xml_positions.push(modeled_children | RAW_COMMENT_REFERENCE_FLAG);
                        }
                        content.push(
                            plan.leaf
                                .expect("comment primitive")
                                .into_content(extra_xml.len()),
                        );
                        modeled_children += 1;
                    } else if action == RunChildAction::AlternateContent {
                        // Keep the block verbatim so the VML fallback survives
                        // a write, and separately read the DrawingML out of it
                        // so layout can see the shape. alt_drawings is never
                        // serialised, the raw copy below is what gets written.
                        let raw = capture_element(reader, e)?;
                        let drawing =
                            crate::drawing::parse_alternate_content_admitted(&raw, &prefixes);
                        let has_drawing = drawing.is_some();
                        if let Some(drawing) = drawing {
                            alt_drawings.push(drawing);
                        }
                        extra_xml.push(raw);
                        extra_xml_positions.push(Self::encode_raw_child_position(
                            modeled_children,
                            false,
                            has_drawing,
                        ));
                    } else {
                        mark_open = mark_was_open
                            && is_word_element(name.as_ref(), b"lastRenderedPageBreak", &prefixes);
                        // Capture unknown child elements as raw XML
                        let is_word_pict = action == RunChildAction::Pict;
                        let raw = capture_element(reader, e)?;
                        let legacy_horizontal_rule = is_word_pict
                            && is_legacy_horizontal_rule(&raw, &namespace_bindings(word_prefixes));
                        extra_xml.push(raw);
                        extra_xml_positions.push(Self::encode_raw_child_position(
                            modeled_children,
                            legacy_horizontal_rule,
                            false,
                        ));
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    let plan = RunChildPlan::read(
                        e,
                        &prefixes,
                        RunChildAction::for_element(e, &prefixes, true),
                    )?;
                    if plan.action == RunChildAction::CommentReference
                        && comment_reference_has_payload_attributes(e, &prefixes)
                    {
                        extra_xml.push(close_comment_reference_source(
                            &capture_empty_element(e)?,
                            &prefixes,
                        )?);
                        extra_xml_positions.push(modeled_children | RAW_COMMENT_REFERENCE_FLAG);
                    }
                    if let Some(leaf) = plan.leaf {
                        mark_open = matches!(
                            leaf,
                            RunLeafValue::Footnote {
                                custom_mark: Some(_),
                                ..
                            } | RunLeafValue::Endnote {
                                custom_mark: Some(_),
                                ..
                            }
                        );
                        content.push(leaf.into_content(extra_xml.len()));
                        modeled_children += 1;
                    } else if plan.action != RunChildAction::IgnoreEmptyProperties {
                        mark_open = mark_was_open
                            && is_word_element(
                                e.name().as_ref(),
                                b"lastRenderedPageBreak",
                                &prefixes,
                            );
                        // Capture unknown empty child elements (e.g.
                        // w:commentReference) as raw XML, mirroring the
                        // Event::Start fallback above.
                        //
                        // A self-closing <w:rPr/> is deliberately skipped.
                        // extra_xml is re-emitted after the run content, but
                        // CT_R requires w:rPr to be the first child, so
                        // capturing it here would move it past <w:t> and
                        // produce schema-invalid output. An empty rPr carries
                        // no formatting, so dropping it loses nothing.
                        let raw = capture_empty_element(e)?;
                        extra_xml.push(raw);
                        extra_xml_positions.push(modeled_children);
                    }
                }
                Ok(event @ (Event::Comment(_) | Event::PI(_))) => {
                    mark_open = mark_was_open;
                    extra_xml.push(capture_standalone_event(event.into_owned())?);
                    extra_xml_positions.push(modeled_children);
                }
                Ok(Event::End(ref e)) if matches_local_name(e.name().as_ref(), b"r") => {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                // Whitespace keeps the mark slot open; other text ends it.
                Ok(Event::Text(ref text)) if text.iter().all(|byte| byte.is_ascii_whitespace()) => {
                    mark_open = mark_was_open;
                }
                _ => {}
            }
            buf.clear();
        }

        let retained_attributes = root
            .map(|root| capture_root_attribute_record(root, word_prefixes))
            .transpose()?
            .flatten();
        let mut retained_bindings = Vec::new();
        // Canonical w is supplied by the generated run itself. Recording it
        // would introduce a synthetic raw slot and break source fixed points.
        if !extra_xml.is_empty() {
            let external_bindings = namespace_bindings(word_prefixes)
                .into_iter()
                .filter(|(prefix, uri)| prefix != "w" || uri != crate::namespace::W_NS)
                .collect::<Vec<_>>();
            for raw in &extra_xml {
                for binding in external_bindings_used_by_raw(raw, &external_bindings)? {
                    if !retained_bindings.contains(&binding) {
                        retained_bindings.push(binding);
                    }
                }
            }
        }
        let mut drawing_bindings = None;
        for drawing in content.iter().filter_map(|item| match item {
            RunContent::Drawing(drawing) => Some(drawing),
            _ => None,
        }) {
            for raw in drawing
                .inline
                .as_ref()
                .and_then(|inline| inline.raw_xml.as_deref())
                .into_iter()
                .chain(
                    drawing
                        .anchor
                        .as_ref()
                        .and_then(|anchor| anchor.raw_xml.as_deref()),
                )
            {
                let bindings =
                    drawing_bindings.get_or_insert_with(|| namespace_bindings(word_prefixes));
                for binding in external_bindings_used_by_raw(raw, bindings)? {
                    if !retained_bindings.contains(&binding) {
                        retained_bindings.push(binding);
                    }
                }
            }
        }
        let mut local_bindings = Vec::new();
        if let Some(root) = root {
            let declarations = raw_namespace_declarations(root)?;
            for drawing in content.iter().filter_map(|item| match item {
                RunContent::Drawing(drawing) => Some(drawing),
                _ => None,
            }) {
                for raw in drawing
                    .inline
                    .as_ref()
                    .and_then(|inline| inline.raw_xml.as_deref())
                    .into_iter()
                    .chain(
                        drawing
                            .anchor
                            .as_ref()
                            .and_then(|anchor| anchor.raw_xml.as_deref()),
                    )
                {
                    for (prefix, _) in external_bindings_used_by_raw(raw, &declarations)? {
                        if prefix == "w"
                            && declarations.iter().any(|(bound, uri)| {
                                bound == &prefix && uri == crate::namespace::W_NS
                            })
                        {
                            continue;
                        }
                        if !local_bindings.contains(&prefix) {
                            local_bindings.push(prefix);
                        }
                    }
                }
            }
        }
        if let Some(record) = retained_attributes {
            extra_xml.push(record);
            extra_xml_positions.push(RAW_ROOT_ATTRIBUTES_FLAG);
        }
        if !retained_bindings.is_empty() {
            let mut record = BytesStart::new(std::str::from_utf8(ROOT_ATTRIBUTES_ELEMENT)?);
            for (prefix, uri) in retained_bindings {
                let name = if prefix.is_empty() {
                    "xmlns".to_owned()
                } else {
                    format!("xmlns:{prefix}")
                };
                record.push_attribute((name.as_str(), uri.as_str()));
            }
            let mut writer = Writer::new(Vec::new());
            writer.write_event(Event::Empty(record))?;
            extra_xml.push(writer.into_inner());
            extra_xml_positions.push(RAW_ROOT_ATTRIBUTES_FLAG);
        }
        if !local_bindings.is_empty() {
            let mut record = BytesStart::new(std::str::from_utf8(ROOT_ATTRIBUTES_ELEMENT)?);
            if let Some(root) = root {
                for attribute in root.attributes() {
                    let attribute = attribute?;
                    let name = attribute.key.as_ref();
                    let prefix = if name == b"xmlns" {
                        Some("")
                    } else {
                        name.strip_prefix(b"xmlns:")
                            .map(std::str::from_utf8)
                            .transpose()?
                    };
                    if prefix
                        .is_some_and(|prefix| local_bindings.iter().any(|local| local == prefix))
                    {
                        // Preserve the actual declaration's lexical value, including references.
                        oxoffice_core::xml::push_raw_attribute(&mut record, attribute);
                    }
                }
            }
            let mut writer = Writer::new(Vec::new());
            writer.write_event(Event::Empty(record))?;
            extra_xml.push(writer.into_inner());
            extra_xml_positions.push(RAW_ROOT_ATTRIBUTES_FLAG | RAW_RUN_LOCAL_BINDINGS_FLAG);
        }

        if let Some(output) = ordinary {
            *output = selected_property;
        }
        Ok(CT_R {
            properties,
            content,
            extra_xml,
            extra_xml_positions,
            alt_drawings,
        })
    }

    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.to_xml_with_word_override(writer, None)
    }

    pub(crate) fn to_xml_with_word_override<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        foreign_word_namespace: Option<&str>,
    ) -> Result<()> {
        self.write_xml_role(
            writer,
            foreign_word_namespace,
            XmlWriteRole::Publication,
            None,
            None,
            None,
        )
    }

    pub(crate) fn write_xml_role<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        foreign_word_namespace: Option<&str>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&FieldXmlEmission>,
        word_capture: Option<&Arc<std::sync::Mutex<TemplateWordCapture>>>,
    ) -> Result<()> {
        let _source_scope = (role == XmlWriteRole::Source).then(|| root_binding_scope(0));

        crate::document::admit_word_model(crate::document::WordOwner::Run(self))?;
        let mut localized = Vec::new();
        let (mut run, raw_bindings) = run_xml_root_in(
            self,
            (role == XmlWriteRole::Publication)
                .then_some(foreign_word_namespace)
                .flatten(),
            &mut localized,
            role,
            emission,
        )?;
        let mut source_bindings = source_context.unwrap_or(&[]).to_vec();
        let local_bindings = raw_namespace_declarations(run.element())?;
        for (prefix, uri) in &local_bindings {
            source_bindings.retain(|(known, _)| *known != prefix);
            source_bindings.push((prefix, uri));
        }
        if role == XmlWriteRole::Source {
            // General raw slots have the existing implicit canonical-w
            // contract. Only the run's own foreign dependency can displace it.
            if !raw_bindings
                .iter()
                .any(|(prefix, uri)| prefix == "w" && uri != crate::namespace::W_NS)
            {
                if !source_bindings.contains(&("w", crate::namespace::W_NS)) {
                    run.push("xmlns:w", crate::namespace::W_NS)?;
                }
                source_bindings.retain(|(prefix, _)| *prefix != "w");
                source_bindings.push(("w", crate::namespace::W_NS));
            }
            let implicit_empty =
                !raw_bindings.iter().any(|(prefix, _)| prefix.is_empty())
                    && self.extra_xml_positions.iter().zip(&self.extra_xml).any(
                        |(position, raw)| {
                            !CT_R::raw_child_is_root_attributes(*position)
                                && external_binding_indices_used_by_raw(raw, &[("", "")])
                                    .is_ok_and(|needed| !needed.is_empty())
                        },
                    );
            if implicit_empty && !source_bindings.contains(&("", "")) {
                run.push("xmlns", "")?;
                source_bindings.retain(|(prefix, _)| !prefix.is_empty());
                source_bindings.push(("", ""));
            }
        }
        let word_prefix = if role == XmlWriteRole::Source {
            run.qualify(&[], role, Some(&source_bindings))?
        } else {
            run.restore_word_binding()?;
            "w".to_owned()
        };
        if role == XmlWriteRole::Source {
            source_bindings.retain(|(prefix, _)| *prefix != word_prefix);
            source_bindings.push((&word_prefix, crate::namespace::W_NS));
        }
        let child_context = (role == XmlWriteRole::Source).then_some(source_bindings.as_slice());
        let mut source_budget = if role == XmlWriteRole::Source {
            let raw_bytes = self
                .extra_xml
                .iter()
                .try_fold(0usize, |sum, raw| sum.checked_add(raw.len()))
                .ok_or_else(|| OxmlError::InvalidValue("run source raw byte overflow".into()))?;
            let scope_bytes = source_bindings
                .iter()
                .try_fold(0usize, |sum, (prefix, namespace)| {
                    sum.checked_add(prefix.len())
                        .and_then(|sum| sum.checked_add(namespace.len()))
                })
                .ok_or_else(|| OxmlError::InvalidValue("run source scope byte overflow".into()))?;
            Some(
                affected_source_dynamic_work(scope_bytes)
                    .and_then(|work| work.checked_add(raw_bytes))
                    .ok_or_else(|| {
                        OxmlError::InvalidValue("run source raw budget overflow".into())
                    })?,
            )
        } else {
            None
        };
        if let (Some(budget), Some(destination)) = (source_budget, child_context) {
            let mut needed = 0usize;
            for (position, raw) in self.extra_xml_positions.iter().zip(&self.extra_xml) {
                if CT_R::raw_child_is_root_attributes(*position) {
                    continue;
                }
                needed = needed
                    .checked_add(run_source_raw_added(raw, &raw_bindings, destination)?)
                    .ok_or_else(|| {
                        OxmlError::InvalidValue("run source raw growth overflow".into())
                    })?;
            }
            if needed > budget {
                return Err(OxmlError::InvalidValue(
                    "run source raw growth exceeds admitted source work".into(),
                ));
            }
        }
        let mut raw_context = RunRawContext {
            foreign_word_namespace,
            bindings: &raw_bindings,
            localized: &localized,
            source_context: child_context,
            source_budget: source_budget.as_mut(),
            emission,
        };
        let _run_scope = emitted_binding_scope(run.element(), false, false)?;
        run.write(writer, false)?;

        let ordered_raw = self.extra_xml_positions.len() == self.extra_xml.len();
        let mut typed_boundary = 0usize;
        if ordered_raw {
            write_run_raw_boundary(writer, self, typed_boundary, &mut raw_context)?;
        }
        if let Some(ref props) = self.properties {
            props.write_xml_role(
                writer,
                foreign_word_namespace,
                role,
                child_context,
                emission,
            )?;
            typed_boundary += 1;
            if ordered_raw {
                write_run_raw_boundary(writer, self, typed_boundary, &mut raw_context)?;
            }
        }

        let mut raw_written = 0;
        for (index, item) in self.content.iter().enumerate() {
            // An empty custom mark before a symbol or deleted text is not
            // written: that child is the mark Word reads, and an empty
            // `w:t` written before it would take its place.
            let mark_follows = matches!(
                self.content.get(index + 1),
                Some(RunContent::Symbol { .. } | RunContent::DeletedText(_))
            );
            match item {
                RunContent::Text(t) => {
                    let mut e = FieldXmlStart::word(&word_prefix, "t", emission)?;
                    if t.preserve_space {
                        e.push("xml:space", "preserve")?;
                    }
                    e.write(writer, false)?;
                    let word_text = template_word_text_input(word_capture, t);
                    if let Some(emission) = emission {
                        emission.write_text(writer, &t.text, word_text)?;
                    } else {
                        let event = BytesText::new(&t.text);
                        let _word = word_text
                            .map(|input| WordPayloadTransport::new(event.as_ref(), input))
                            .transpose()?;
                        writer.write_event(Event::Text(event.borrow()))?;
                    }
                    if let Some(emission) = emission {
                        emission.write_word_end(writer, &word_prefix, "t")?;
                    } else {
                        writer
                            .write_event(Event::End(BytesEnd::new(format!("{word_prefix}:t"))))?;
                    }
                }
                RunContent::DeletedText(t) => {
                    let mut e = FieldXmlStart::word(&word_prefix, "delText", emission)?;
                    if t.preserve_space {
                        e.push("xml:space", "preserve")?;
                    }
                    e.write(writer, false)?;
                    if let Some(emission) = emission {
                        emission.write_text(writer, &t.text, None)?;
                    } else {
                        let event = BytesText::new(&t.text);
                        writer.write_event(Event::Text(event))?;
                    }
                    if let Some(emission) = emission {
                        emission.write_word_end(writer, &word_prefix, "delText")?;
                    } else {
                        writer.write_event(Event::End(BytesEnd::new(format!(
                            "{word_prefix}:delText"
                        ))))?;
                    }
                }
                RunContent::Tab => {
                    FieldXmlStart::word(&word_prefix, "tab", emission)?.write(writer, true)?;
                }
                RunContent::Break(bt, clear) => {
                    let mut e = FieldXmlStart::word(&word_prefix, "br", emission)?;
                    let clear = clear.as_attribute();
                    match bt {
                        BreakType::Page => e.push_word(&word_prefix, "type", "page")?,
                        BreakType::Column => e.push_word(&word_prefix, "type", "column")?,
                        // Word spells a clearing line break with its type.
                        BreakType::Line if clear.is_some() => {
                            e.push_word(&word_prefix, "type", "textWrapping")?
                        }
                        BreakType::Line => {}
                    }
                    if let Some(clear) = clear {
                        e.push_word(&word_prefix, "clear", clear)?;
                    }
                    e.write(writer, true)?;
                }
                RunContent::Drawing(d) => {
                    // Notes retain their existing local drawing preparation.
                    // Supply only this concrete drawing with the scoped
                    // declaration context, leaving ordinary Text emission
                    // and every unrelated paragraph child unchanged.
                    let note_emission = word_capture.and_then(|capture| {
                        let facts = capture.lock().expect("Word text capture poisoned");
                        facts.note_drawing_bindings.then(|| FieldXmlEmission {
                            enabled: false,
                            output: Arc::clone(&facts.output),
                            word_capture: Some(Arc::clone(capture)),
                        })
                    });
                    d.write_xml_role_in(
                        writer,
                        role,
                        child_context,
                        emission.or(note_emission.as_ref()),
                    )?;
                }
                RunContent::Field(_) => {
                    // Field runs are serialized at the paragraph level as <w:fldSimple>
                }
                RunContent::FootnoteRef { id, custom_mark } => {
                    let mut buf = itoa::Buffer::new();
                    let mut e = FieldXmlStart::word(&word_prefix, "footnoteReference", emission)?;
                    e.push_word(&word_prefix, "id", buf.format(*id))?;
                    if custom_mark.is_some() {
                        e.push_word(&word_prefix, "customMarkFollows", "1")?;
                    }
                    e.write(writer, true)?;
                    if let Some(mark) = custom_mark
                        && !(mark.is_empty() && mark_follows)
                    {
                        let mut text = FieldXmlStart::word(&word_prefix, "t", emission)?;
                        text.push("xml:space", "preserve")?;
                        text.write(writer, false)?;
                        if let Some(emission) = emission {
                            emission.write_text(writer, mark, None)?;
                        } else {
                            writer.write_event(Event::Text(quick_xml::events::BytesText::new(
                                mark,
                            )))?;
                        }
                        if let Some(emission) = emission {
                            emission.write_word_end(writer, &word_prefix, "t")?;
                        } else {
                            writer.write_event(Event::End(BytesEnd::new(format!(
                                "{word_prefix}:t"
                            ))))?;
                        }
                    }
                }
                RunContent::EndnoteRef { id, custom_mark } => {
                    let mut buf = itoa::Buffer::new();
                    let mut e = FieldXmlStart::word(&word_prefix, "endnoteReference", emission)?;
                    e.push_word(&word_prefix, "id", buf.format(*id))?;
                    if custom_mark.is_some() {
                        e.push_word(&word_prefix, "customMarkFollows", "1")?;
                    }
                    e.write(writer, true)?;
                    if let Some(mark) = custom_mark
                        && !(mark.is_empty() && mark_follows)
                    {
                        let mut text = FieldXmlStart::word(&word_prefix, "t", emission)?;
                        text.push("xml:space", "preserve")?;
                        text.write(writer, false)?;
                        if let Some(emission) = emission {
                            emission.write_text(writer, mark, None)?;
                        } else {
                            writer.write_event(Event::Text(quick_xml::events::BytesText::new(
                                mark,
                            )))?;
                        }
                        if let Some(emission) = emission {
                            emission.write_word_end(writer, &word_prefix, "t")?;
                        } else {
                            writer.write_event(Event::End(BytesEnd::new(format!(
                                "{word_prefix}:t"
                            ))))?;
                        }
                    }
                }
                RunContent::Symbol { font, char_code } => {
                    let mut e = FieldXmlStart::word(&word_prefix, "sym", emission)?;
                    e.push_word(&word_prefix, "font", font.as_str())?;
                    const HEX: &[u8; 16] = b"0123456789ABCDEF";
                    let code = [
                        HEX[usize::from(*char_code >> 12)],
                        HEX[usize::from((*char_code >> 8) & 15)],
                        HEX[usize::from((*char_code >> 4) & 15)],
                        HEX[usize::from(*char_code & 15)],
                    ];
                    e.push_word(&word_prefix, "char", std::str::from_utf8(&code)?)?;
                    e.write(writer, true)?;
                }
                RunContent::SpecialCharacter(character) => {
                    write_special_character(writer, *character, &word_prefix, emission)?;
                }
                RunContent::CommentReference { id, raw_before } => {
                    if !ordered_raw {
                        for (index, raw) in self
                            .extra_xml
                            .iter()
                            .enumerate()
                            .take((*raw_before).min(self.extra_xml.len()))
                            .skip(raw_written)
                        {
                            if !self
                                .extra_xml_positions
                                .get(index)
                                .is_some_and(|position| *position & RAW_COMMENT_REFERENCE_FLAG != 0)
                            {
                                write_run_raw_source(writer, raw, &mut raw_context)?;
                            }
                            raw_written += 1;
                        }
                    }
                    if let Some(raw) = self
                        .extra_xml_positions
                        .iter()
                        .zip(&self.extra_xml)
                        .find_map(|(position, raw)| {
                            (*position & RAW_COMMENT_REFERENCE_FLAG != 0
                                && Self::raw_child_position(*position) == typed_boundary
                                && retained_comment_reference_id(raw) == Some(*id))
                            .then_some(raw)
                        })
                    {
                        write_run_raw_source(writer, raw, &mut raw_context)?;
                        typed_boundary += 1;
                        if ordered_raw {
                            write_run_raw_boundary(writer, self, typed_boundary, &mut raw_context)?;
                        }
                        continue;
                    }
                    let mut buf = itoa::Buffer::new();
                    let mut e = FieldXmlStart::word(&word_prefix, "commentReference", emission)?;
                    e.push_word(&word_prefix, "id", buf.format(*id))?;
                    e.write(writer, true)?;
                }
            }
            typed_boundary += 1;
            if ordered_raw {
                write_run_raw_boundary(writer, self, typed_boundary, &mut raw_context)?;
            }
        }

        // Write captured unknown child elements
        if !ordered_raw {
            for (index, raw) in self.extra_xml.iter().enumerate().skip(raw_written) {
                if is_root_attribute_record(raw)
                    || self
                        .extra_xml_positions
                        .get(index)
                        .is_some_and(|position| *position & RAW_COMMENT_REFERENCE_FLAG != 0)
                {
                    continue;
                }
                write_run_raw_source(writer, raw, &mut raw_context)?;
            }
        }

        if let Some(emission) = emission {
            emission.write_word_end(writer, &word_prefix, "r")?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(format!("{word_prefix}:r"))))?;
        }
        Ok(())
    }
}

pub(crate) fn comment_reference_has_payload_attributes(
    element: &BytesStart<'_>,
    prefixes: &[String],
) -> bool {
    let name = element.name();
    let own_binding = qualified_name_prefix(name.as_ref())
        .map_or_else(|| "xmlns".to_owned(), |prefix| format!("xmlns:{prefix}"));
    element.attributes().any(|attribute| {
        attribute.ok().is_none_or(|attribute| {
            if attribute_in_namespace(
                attribute.key.as_ref(),
                b"id",
                crate::namespace::W_NS,
                prefixes,
            ) {
                return false;
            }
            // The element's own Word alias declaration is structural, not opaque
            // payload. Every other declaration or attribute retains its source.
            !(attribute.key.as_ref() == own_binding.as_bytes()
                && attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                    .is_ok_and(|value| value == crate::namespace::W_NS))
        })
    })
}

fn close_comment_reference_source(raw: &[u8], prefixes: &[String]) -> Result<Vec<u8>> {
    let mut bindings = prefixes
        .iter()
        .filter_map(|binding| {
            binding
                .strip_prefix('\0')
                .and_then(|binding| binding.split_once('\0'))
                .map(|(prefix, uri)| (prefix.to_owned(), uri.to_owned()))
        })
        .collect::<Vec<_>>();
    for prefix in prefixes.iter().filter(|prefix| !prefix.starts_with('\0')) {
        if !bindings.iter().any(|(bound, _)| bound == prefix) {
            bindings.push((prefix.clone(), crate::namespace::W_NS.to_owned()));
        }
    }
    raw_with_external_bindings(raw, &bindings)
}

fn retained_comment_reference_id(raw: &[u8]) -> Option<i32> {
    let mut reader = quick_xml::reader::NsReader::from_reader(raw);
    let (ns, event) = reader.read_resolved_event().ok()?;
    if !matches!(ns, ResolveResult::Bound(namespace) if namespace.as_ref() == crate::namespace::W_NS.as_bytes())
    {
        return None;
    }
    let element = match event {
        Event::Start(element) | Event::Empty(element) => element,
        _ => return None,
    };
    if element.local_name().as_ref() != b"commentReference" {
        return None;
    }
    element.attributes().find_map(|attr| {
        let attr = attr.ok()?;
        let (ns, local) = reader.resolver().resolve_attribute(attr.key);
        (matches!(ns, ResolveResult::Bound(namespace) if namespace.as_ref() == crate::namespace::W_NS.as_bytes())
            && local.as_ref() == b"id").then(|| attr.decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder()).ok()?.parse().ok()).flatten()
    })
}

pub(super) fn run_xml_root(
    run: &CT_R,
    foreign_word_namespace: Option<&str>,
    localized: &mut Vec<(String, String)>,
    role: XmlWriteRole,
) -> Result<(BytesStart<'static>, Vec<(String, String)>)> {
    let (root, bindings) = run_xml_root_in(run, foreign_word_namespace, localized, role, None)?;
    Ok((root.start, bindings))
}

fn run_xml_root_in<'emission>(
    run: &CT_R,
    foreign_word_namespace: Option<&str>,
    localized: &mut Vec<(String, String)>,
    role: XmlWriteRole,
    emission: Option<&'emission FieldXmlEmission>,
) -> Result<(FieldXmlStart<'emission, 'static>, Vec<(String, String)>)> {
    localized.clear();
    let mut root = FieldXmlStart::new("w:r", emission)?;
    if foreign_word_namespace.is_some() {
        root.push("xmlns:w", crate::namespace::W_NS)?;
    }
    let mut bindings = Vec::new();
    let mut lexical = Vec::new();
    let mut local = HashSet::new();
    let mut attributes = Vec::new();
    for (position, raw) in run.extra_xml_positions.iter().zip(&run.extra_xml) {
        if CT_R::raw_child_is_root_attributes(*position) {
            let mut reader = Reader::from_reader(raw.as_slice());
            let Event::Empty(record) = reader.read_event()? else {
                return Err(OxmlError::InvalidValue(
                    "invalid run root attributes".into(),
                ));
            };
            let declarations = raw_namespace_declarations(&record)?;
            let source_local = *position & RAW_RUN_LOCAL_BINDINGS_FLAG != 0;
            for binding in declarations {
                if source_local
                    && bindings
                        .iter()
                        .any(|(prefix, uri)| prefix == &binding.0 && uri != &binding.1)
                {
                    return Err(OxmlError::InvalidValue(
                        "inconsistent source-local run namespace binding".into(),
                    ));
                }
                if !bindings.contains(&binding) {
                    bindings.push(binding);
                }
            }
            for attribute in record.attributes() {
                let attribute = attribute?;
                let name = attribute.key.as_ref();
                let prefix = if name == b"xmlns" {
                    Some("")
                } else {
                    name.strip_prefix(b"xmlns:")
                        .map(std::str::from_utf8)
                        .transpose()?
                };
                if let Some(prefix) = prefix {
                    let value = std::str::from_utf8(&attribute.value)?.to_owned();
                    if let Some((_, spelling)) =
                        lexical.iter_mut().find(|(existing, _)| existing == prefix)
                    {
                        if source_local {
                            *spelling = value;
                        }
                    } else {
                        lexical.push((prefix.to_owned(), value));
                    }
                    if source_local {
                        local.insert(prefix.to_owned());
                    }
                }
            }
            let prefixes = word_prefixes_at(&record, &[])?;
            let scope = prefixes
                .iter()
                .filter_map(|value| value.strip_prefix('\0')?.split_once('\0'))
                .collect::<Vec<_>>();
            let plan = RootAttributePlan::read(&record, &scope)?;
            if !plan.attributes.is_empty() {
                attributes.push((plan, record, prefixes));
            }
        }
    }
    let mut wp_writer = false;
    let mut r_writer = false;
    for drawing in run.content.iter().filter_map(|item| match item {
        RunContent::Drawing(drawing) => Some(drawing),
        _ => None,
    }) {
        if let Some(inline) = &drawing.inline
            && inline.raw_xml.is_none()
        {
            wp_writer = true;
            r_writer |= inline.chart_rel_id.is_none();
        }
        if let Some(anchor) = &drawing.anchor
            && anchor.raw_xml.is_none()
        {
            wp_writer = true;
            r_writer |= anchor.chart_rel_id.is_none();
        }
    }
    for (prefix, value) in &lexical {
        let generated = match prefix.as_str() {
            "w" => Some(crate::namespace::W_NS),
            "wp" if wp_writer => Some(WP_NS),
            "r" if r_writer => Some(R_NS),
            _ => None,
        };
        if let Some(uri) = generated.filter(|_| role == XmlWriteRole::Publication)
            && bindings
                .iter()
                .any(|(bound, actual)| bound == prefix && actual != uri)
        {
            localized.push((prefix.clone(), value.clone()));
            if prefix != "w" {
                root.push_word("xmlns", prefix, uri)?;
            }
        }
    }
    // Root aliases protect only the actual proven canonical target scopes above.
    for (plan, record, prefixes) in attributes {
        plan.merge_run_root(&mut root, &record, &prefixes, role)?;
    }
    let guarantees = ROOT_BINDINGS.with(Cell::get);
    for (prefix, value) in &lexical {
        if (prefix == "w" && role == XmlWriteRole::Publication)
            || localized.iter().any(|(bound, _)| bound == prefix)
        {
            continue;
        }
        let guaranteed = !local.contains(prefix)
            && match prefix.as_str() {
                "r" => guarantees & ROOT_R_BINDING != 0 && value == R_NS,
                "mc" => guarantees & ROOT_MC_BINDING != 0 && value == MC_NS,
                "wp" => guarantees & ROOT_WP_BINDING != 0 && value == WP_NS,
                _ => false,
            };
        if guaranteed {
            continue;
        }
        let present = root.element().attributes().any(|attribute| {
            attribute.is_ok_and(|attribute| {
                let key = attribute.key.as_ref();
                if prefix.is_empty() {
                    key == b"xmlns"
                } else {
                    key.strip_prefix(b"xmlns:") == Some(prefix.as_bytes())
                }
            })
        });
        if !present {
            if prefix.is_empty() {
                root.push_raw("xmlns", value)?;
            } else {
                root.push_raw_word("xmlns", prefix, value)?;
            }
        }
    }
    Ok((root, bindings))
}

fn run_source_raw_added(
    raw: &[u8],
    bindings: &[(String, String)],
    destination: &[(&str, &str)],
) -> Result<usize> {
    let mut original = bindings
        .iter()
        .map(|(prefix, uri)| (prefix.as_str(), uri.as_str()))
        .collect::<Vec<_>>();
    if !original.iter().any(|(prefix, _)| *prefix == "w") {
        original.push(("w", crate::namespace::W_NS));
    }
    if !original.iter().any(|(prefix, _)| prefix.is_empty()) {
        original.push(("", ""));
    }
    let plan = retained_binding_plan(raw, &original, destination, usize::MAX)?;
    Ok(plan.added_bytes(raw))
}

pub(super) fn write_run_raw_source<W: std::io::Write>(
    writer: &mut Writer<W>,
    raw: &[u8],
    context: &mut RunRawContext<'_, '_>,
) -> Result<()> {
    if let Some(destination) = context.source_context {
        let budget = context
            .source_budget
            .as_deref_mut()
            .ok_or_else(|| OxmlError::InvalidValue("run source raw budget missing".into()))?;
        let mut original = context
            .bindings
            .iter()
            .map(|(prefix, uri)| (prefix.as_str(), uri.as_str()))
            .collect::<Vec<_>>();
        if !original.iter().any(|(prefix, _)| *prefix == "w") {
            original.push(("w", crate::namespace::W_NS));
        }
        if !original.iter().any(|(prefix, _)| prefix.is_empty()) {
            original.push(("", ""));
        }
        let plan = retained_binding_plan(raw, &original, destination, *budget)?;
        let added = plan.added_bytes(raw);
        *budget = budget
            .checked_sub(added)
            .ok_or_else(|| OxmlError::InvalidValue("run source raw budget underflow".into()))?;
        let (produced, prepaid) = emit_retained_roots_in(raw, plan, context.emission)?;
        let _transport = context
            .emission
            .filter(|_| prepaid)
            .map(|emission| emission.prepaid_xml_transport(&produced))
            .transpose()?;
        writer.get_mut().write_all(&produced)?;
        return Ok(());
    }
    // QName values need every actual source fact during discovery, including
    // the attribute namespace itself. Only conflicting generated prefixes
    // need a declaration on the opaque subtree.
    let used = external_bindings_used_by_raw(raw, context.bindings)?;
    let needed = context
        .localized
        .iter()
        .filter(|(prefix, _)| used.iter().any(|(used, _)| used == prefix))
        .cloned()
        .collect::<Vec<_>>();
    // These are lexical source values, not newly authored namespace URIs.
    let closed = if needed.is_empty() {
        Cow::Borrowed(raw)
    } else {
        Cow::Owned(raw_with_root_bindings_in(
            raw,
            &needed,
            true,
            context.emission,
        )?)
    };
    let _transport = context
        .emission
        .filter(|_| !needed.is_empty())
        .map(|emission| emission.prepaid_xml_transport(&closed))
        .transpose()?;
    write_raw_with_word_override_in(
        writer,
        &closed,
        context.foreign_word_namespace,
        context.emission,
    )
}

fn write_run_raw_boundary<W: std::io::Write>(
    writer: &mut Writer<W>,
    run: &CT_R,
    boundary: usize,
    context: &mut RunRawContext<'_, '_>,
) -> Result<()> {
    for (position, raw) in run.extra_xml_positions.iter().zip(&run.extra_xml) {
        if !CT_R::raw_child_is_root_attributes(*position)
            && *position & RAW_COMMENT_REFERENCE_FLAG == 0
            && CT_R::raw_child_position(*position) == boundary
        {
            write_run_raw_source(writer, raw, context)?;
        }
    }
    Ok(())
}

/// A hyperlink span that wraps a range of runs.
#[derive(Debug, Clone, PartialEq)]
pub struct HyperlinkSpan {
    /// The relationship ID for the hyperlink target.
    pub rel_id: Option<String>,
    /// Optional anchor within the document (for internal links).
    pub anchor: Option<String>,
    /// Optional user-facing hover text.
    pub tooltip: Option<String>,
    /// Optional location in the hyperlink target document.
    pub doc_location: Option<String>,
    /// Index of the first run in the hyperlink (inclusive).
    pub run_start: usize,
    /// Index of the last run in the hyperlink (exclusive).
    pub run_end: usize,
    /// Unmodeled owner attributes retained when the hyperlink came from XML.
    #[doc(hidden)]
    pub extra_attributes: Vec<(String, String)>,
    /// Raw children at `(relative run boundary, typed revisions before, XML)`.
    #[doc(hidden)]
    pub extra_xml: Vec<(usize, usize, Vec<u8>)>,
    /// Parent raw slot for a revision-only hyperlink preserved as one subtree.
    #[doc(hidden)]
    pub preserved_raw_before: Option<usize>,
}

const HYPERLINK_REVISION_FLAG: usize = 1usize << (usize::BITS - 1);

/// A hyperlink represented by a complex field sequence rather than `w:hyperlink`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComplexFieldHyperlink {
    pub run_start: usize,
    pub run_end: usize,
    pub target: String,
}

pub(super) type ParsedHyperlinkAttributes = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Vec<(String, String)>,
);

pub(crate) fn hyperlink_revision_slot(hyperlink_index: usize) -> usize {
    HYPERLINK_REVISION_FLAG | hyperlink_index
}

#[doc(hidden)]
pub fn hyperlink_revision_index(slot: usize) -> Option<usize> {
    (slot & HYPERLINK_REVISION_FLAG != 0).then_some(slot & !HYPERLINK_REVISION_FLAG)
}

pub(crate) fn field_run(field: Field, properties: Option<CT_RPr>) -> CT_R {
    CT_R {
        properties,
        content: vec![RunContent::Field(field)],
        extra_xml: Vec::new(),
        extra_xml_positions: Vec::new(),
        alt_drawings: Vec::new(),
    }
}

pub(crate) fn parse_run_raw(
    raw: &[u8],
    word_prefixes: &[String],
    text_facts: Option<&mut Vec<ParsedWordText>>,
) -> Result<CT_R> {
    parse_run_raw_with_input(raw, word_prefixes, text_facts, None)
}

pub(crate) fn parse_run_raw_with_input(
    raw: &[u8],
    word_prefixes: &[String],
    text_facts: Option<&mut Vec<ParsedWordText>>,
    input: Option<&crate::document::WordFieldInput>,
) -> Result<CT_R> {
    parse_run_raw_with_observation(raw, word_prefixes, text_facts, input, None)
}

pub(crate) fn parse_run_raw_with_observation(
    raw: &[u8],
    word_prefixes: &[String],
    text_facts: Option<&mut Vec<ParsedWordText>>,
    input: Option<&crate::document::WordFieldInput>,
    mut ordinary: Option<&mut Option<crate::document::WordOrdinaryFact>>,
) -> Result<CT_R> {
    if let Some(output) = &mut ordinary {
        **output = None;
    }
    let cursor = input.map(|input| input.cursor(raw)).transpose()?;
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(start) if matches_local_name(start.name().as_ref(), b"r") => {
                let prefixes = word_prefixes_at(&start, word_prefixes)?;
                return CT_R::from_xml_with_property_observation(
                    &mut reader,
                    &prefixes,
                    Some(&start),
                    text_facts,
                    cursor.as_ref(),
                    ordinary,
                );
            }
            Event::Eof => {
                return Err(OxmlError::MissingElement("w:r".to_owned()));
            }
            _ => {}
        }
        buffer.clear();
    }
}

#[cfg(test)]
mod custom_mark_tests {
    use super::*;

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    fn read(children: &str) -> CT_R {
        CT_R::from_xml_fragment_checked(format!("<w:r {W}>{children}</w:r>").as_bytes()).unwrap()
    }

    fn written(run: &CT_R) -> String {
        let mut writer = Writer::new(Vec::new());
        run.to_xml(&mut writer).unwrap();
        String::from_utf8(writer.into_inner()).unwrap()
    }

    fn mark(run: &CT_R) -> Option<&str> {
        run.content.iter().find_map(|item| match item {
            RunContent::FootnoteRef { custom_mark, .. } => Some(custom_mark.as_deref()?),
            _ => None,
        })
    }

    #[test]
    fn every_on_spelling_of_custom_mark_follows_reads_and_writes_as_set() {
        for value in ["1", "true", "on"] {
            let run = read(&format!(
                r#"<w:footnoteReference w:customMarkFollows="{value}" w:id="2"/><w:t>*</w:t>"#
            ));
            assert_eq!(mark(&run), Some("*"), "{value}");
            let xml = written(&run);
            assert!(xml.contains(r#"w:customMarkFollows="1""#), "{value}: {xml}");
            assert!(
                xml.contains("<w:t xml:space=\"preserve\">*</w:t>"),
                "{value}: {xml}"
            );
        }
        for value in ["0", "false", "off"] {
            let run = read(&format!(
                r#"<w:footnoteReference w:customMarkFollows="{value}" w:id="2"/><w:t>*</w:t>"#
            ));
            assert_eq!(mark(&run), None, "{value}");
        }
    }

    #[test]
    fn the_mark_is_the_first_text_past_a_page_break_comment_or_whitespace_only() {
        let reference = r#"<w:footnoteReference w:customMarkFollows="1" w:id="2"/>"#;
        for (between, expected) in [
            ("<w:lastRenderedPageBreak/>", Some("*")),
            (
                "<w:lastRenderedPageBreak></w:lastRenderedPageBreak>",
                Some("*"),
            ),
            ("<!-- c --><?p?> ", Some("*")),
            ("<w:t></w:t>", Some("")),
            ("<w:tab/>", Some("")),
            ("<w:fldChar w:fldCharType=\"begin\"/>", Some("")),
        ] {
            let run = read(&format!("{reference}{between}<w:t>*</w:t>"));
            assert_eq!(mark(&run), expected, "{between}");
            // Where the slot closed, the text is body text.
            let body = run
                .content
                .iter()
                .any(|item| matches!(item, RunContent::Text(text) if text.text == "*"));
            assert_eq!(body, expected != Some("*"), "{between}");
        }
    }

    #[test]
    fn a_symbol_or_deleted_mark_is_written_back_without_an_empty_text_before_it() {
        let reference = r#"<w:footnoteReference w:id="2" w:customMarkFollows="1"/>"#;
        for mark in [
            r#"<w:sym w:font="Symbol" w:char="F0A7"/>"#,
            "<w:delText>*</w:delText>",
        ] {
            let source = format!("{reference}{mark}");
            let xml = written(&read(&source));
            assert_eq!(xml, format!("<w:r>{source}</w:r>"), "{mark}");
        }
        // A text mark that is empty still writes its `w:t`, so the text after
        // it stays body text.
        let run = read(&format!(r#"{reference}<w:t></w:t><w:t>x</w:t>"#));
        assert_eq!(mark(&run), Some(""));
        let xml = written(&run);
        assert!(
            xml.contains(r#"<w:t xml:space="preserve"></w:t><w:t>x</w:t>"#),
            "{xml}"
        );
    }
}
