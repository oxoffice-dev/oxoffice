//! WordprocessingML structured document tags (`w:sdt`).

use crate::date_format::WordDateTime;
use crate::text::XmlWriteRole;
use std::io::Write;
use std::ops::Range;

use oxoffice_core::xml_reader::NsReader;

use oxoffice_core::xml_reader::Reader;
use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::{Writer, XmlVersion};

use crate::error::{OxmlError, Result};
use crate::namespace::{W_NS, matches_local_name};
use crate::numbering::{
    local_namespace_overrides, namespace_bindings, parse_scoped_rpr_with_observation,
    word_prefixes_at,
};
use crate::properties::{CT_RPr, is_word_element};
use crate::raw_xml::{capture_element, capture_empty};
use crate::revision::CT_Revision;
use crate::table::{CT_Row, CT_Tbl, CT_Tc};
use crate::text::{
    AcceptedRunPath, AcceptedRunPathSegment, CT_P, CT_R, Field, LegacyFormFieldValue, RunContent,
};

/// The bounded content-control type markers that oxoffice reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdtType {
    RichText,
    PlainText,
    Picture,
    CheckBox,
    ComboBox,
    DropDownList,
    Date,
    DocumentPartList,
    DocumentPartObject,
    Group,
    RepeatingSection,
    RepeatingSectionItem,
    Citation,
    Equation,
    Bibliography,
}

impl SdtType {
    fn from_element(local: &[u8]) -> Option<Self> {
        match local {
            b"richText" => Some(Self::RichText),
            b"text" => Some(Self::PlainText),
            b"picture" => Some(Self::Picture),
            b"checkbox" => Some(Self::CheckBox),
            b"comboBox" => Some(Self::ComboBox),
            b"dropDownList" => Some(Self::DropDownList),
            b"date" => Some(Self::Date),
            b"docPartList" => Some(Self::DocumentPartList),
            b"docPartObj" => Some(Self::DocumentPartObject),
            b"group" => Some(Self::Group),
            b"repeatingSection" => Some(Self::RepeatingSection),
            b"repeatingSectionItem" => Some(Self::RepeatingSectionItem),
            b"citation" => Some(Self::Citation),
            b"equation" => Some(Self::Equation),
            b"bibliography" => Some(Self::Bibliography),
            _ => None,
        }
    }

    fn element_name(self) -> &'static str {
        match self {
            Self::RichText => "w:richText",
            Self::PlainText => "w:text",
            Self::Picture => "w:picture",
            Self::CheckBox => "w14:checkbox",
            Self::ComboBox => "w:comboBox",
            Self::DropDownList => "w:dropDownList",
            Self::Date => "w:date",
            Self::DocumentPartList => "w:docPartList",
            Self::DocumentPartObject => "w:docPartObj",
            Self::Group => "w:group",
            Self::RepeatingSection => "w15:repeatingSection",
            Self::RepeatingSectionItem => "w15:repeatingSectionItem",
            Self::Citation => "w:citation",
            Self::Equation => "w:equation",
            Self::Bibliography => "w:bibliography",
        }
    }
}

/// The optional custom XML binding carried by `w:dataBinding`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CT_DataBinding {
    pub prefix_mappings: Option<String>,
    pub xpath: Option<String>,
    pub store_item_id: Option<String>,
    extra_attributes: Vec<(String, String)>,
}

impl CT_DataBinding {
    fn parse(start: &BytesStart<'_>, inherited: &[String]) -> Result<Self> {
        let prefixes = word_prefixes_at(start, inherited)?;
        let mut binding = Self::default();
        for attribute in start.attributes() {
            let attribute = attribute?;
            let key = attribute.key.as_ref();
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())?
                .into_owned();
            if is_word_attribute(key, b"prefixMappings", &prefixes) {
                binding.prefix_mappings = Some(value);
            } else if is_word_attribute(key, b"xpath", &prefixes) {
                binding.xpath = Some(value);
            } else if is_word_attribute(key, b"storeItemID", &prefixes) {
                binding.store_item_id = Some(value);
            } else {
                binding
                    .extra_attributes
                    .push((std::str::from_utf8(key)?.to_owned(), value));
            }
        }
        Ok(binding)
    }

    fn to_xml<W: Write>(
        &self,
        writer: &mut Writer<W>,
        original: &[String],
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let mut qualified = crate::text::FieldXmlStart::new("w:dataBinding", emission)?;
        for (name, value) in &self.extra_attributes {
            qualified.push(name, value)?;
        }
        let prefix = qualified.qualify(original, role, source_context)?;
        let mut start = crate::text::FieldXmlStart::word(&prefix, "dataBinding", emission)?;
        if let Some(value) = &self.prefix_mappings {
            start.push_word(&prefix, "prefixMappings", value)?;
        }
        if let Some(value) = &self.xpath {
            start.push_word(&prefix, "xpath", value)?;
        }
        if let Some(value) = &self.store_item_id {
            start.push_word(&prefix, "storeItemID", value)?;
        }
        for (name, value) in &self.extra_attributes {
            start.push(name, value)?;
        }
        for attribute in qualified.element().attributes() {
            let attribute = attribute?;
            let name = std::str::from_utf8(attribute.key.as_ref())?;
            if name.starts_with("xmlns:")
                && !self.extra_attributes.iter().any(|(key, _)| key == name)
            {
                start.push(name, crate::namespace::W_NS)?;
            }
        }
        start.write(writer, true)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
enum PropertySlot {
    Alias,
    Tag,
    Id,
    Type,
    DataBinding,
    Raw(Vec<u8>),
}

/// What a content control holds as a form field, read from its properties.
/// Non-exhaustive, so more facts can join without breaking callers.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SdtFormFacts {
    pub lock: SdtLock,
    /// `w:showingPlcHdr`: the content shows the placeholder, not a value.
    pub showing_placeholder: bool,
    pub payload: SdtFormPayload,
}

/// `w:lock`: what the control protects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SdtLock {
    #[default]
    Unlocked,
    /// The control cannot be deleted; its content can be edited.
    SdtLocked,
    /// The content cannot be edited.
    ContentLocked,
    /// Neither the control nor its content can change.
    SdtContentLocked,
}

impl SdtLock {
    /// Whether the content cannot be edited.
    pub fn content_locked(self) -> bool {
        matches!(self, Self::ContentLocked | Self::SdtContentLocked)
    }
}

/// The kind of a content control and the facts its kind carries.
/// Non-exhaustive, so more kinds can join without breaking callers.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum SdtFormPayload {
    /// `w:text`.
    PlainText {
        multiline: bool,
    },
    /// `w:richText`, or no type element at all, which Word reads the same.
    RichText,
    /// `w14:checkbox`, with the symbol and font of each state.
    CheckBox {
        checked: bool,
        checked_symbol: (char, String),
        unchecked_symbol: (char, String),
    },
    DropDownList {
        items: Vec<SdtListItem>,
    },
    ComboBox {
        items: Vec<SdtListItem>,
    },
    /// `w:date`, with its `w:dateFormat` picture.
    Date {
        format: Option<String>,
    },
    Picture,
    /// Groups, repeating sections, galleries, citations, equations,
    /// bibliographies.
    Other,
}

/// The local part of a possibly prefixed XML name.
fn local_part(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, local)| local)
}

/// One entry of a drop-down list or combo box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdtListItem {
    /// What the list shows.
    pub display: String,
    /// What the control stores when the entry is chosen.
    pub value: String,
}

/// Typed properties and ordered raw slots from `w:sdtPr`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CT_SdtPr {
    pub alias: Option<String>,
    pub tag: Option<String>,
    pub id: Option<i32>,
    pub control_type: Option<SdtType>,
    pub data_binding: Option<CT_DataBinding>,
    extra_attributes: Vec<(String, String)>,
    slots: Vec<PropertySlot>,
    type_element: Option<TypeElement>,
    source_prefixes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct TypeElement {
    control_type: SdtType,
    name: String,
    attributes: Vec<(String, String)>,
    children: Vec<u8>,
    namespace_bindings: Vec<(String, String)>,
}

struct ValueChild {
    name: Vec<u8>,
    value: Option<String>,
    display: Option<String>,
}

struct CheckboxChild {
    value: Option<Range<usize>>,
    font: Option<Range<usize>>,
    insert_attribute: usize,
}

/// Required decoded scalar properties, shared by the constructor and admission.
pub(crate) enum ControlScalar {
    Alias(Option<String>),
    Tag(Option<String>),
    Id(Option<i32>),
}
impl ControlScalar {
    pub(crate) fn for_local(local: &[u8]) -> Option<Self> {
        match local {
            b"alias" => Some(Self::Alias(None)),
            b"tag" => Some(Self::Tag(None)),
            b"id" => Some(Self::Id(None)),
            _ => None,
        }
    }
    pub(crate) fn decode(self, value: Option<String>) -> Result<Self> {
        let value = value.ok_or_else(|| OxmlError::MissingElement("w:val attribute".to_owned()))?;
        Ok(match self {
            Self::Alias(_) => Self::Alias(Some(value)),
            Self::Tag(_) => Self::Tag(Some(value)),
            Self::Id(_) => Self::Id(Some(value.parse()?)),
        })
    }
    fn apply(self, properties: &mut CT_SdtPr) {
        match self {
            Self::Alias(value) => {
                properties.alias = value;
                properties.slots.push(PropertySlot::Alias);
            }
            Self::Tag(value) => {
                properties.tag = value;
                properties.slots.push(PropertySlot::Tag);
            }
            Self::Id(value) => {
                properties.id = value;
                properties.slots.push(PropertySlot::Id);
            }
        }
    }
}

impl CT_SdtPr {
    /// Resolve the bounded text, dropdown and Gregorian date value conventions;
    /// a date shows in its `w:dateFormat`, named in its `w:lid` language.
    pub fn prepare_text_value(&self, value: &str) -> Result<(String, String, Option<String>)> {
        oxoffice_core::xml::reject_non_xml_characters("content-control value", value)?;
        match self.control_type {
            Some(SdtType::Date) => {
                let at = WordDateTime::parse_value(value)?;
                let children = self.value_children()?;
                let metadata = |name: &[u8]| -> Result<Option<&str>> {
                    let mut matches = children.iter().filter(|child| child.name == name);
                    let first = matches.next();
                    if matches.next().is_some() {
                        return Err(OxmlError::InvalidValue(
                            "duplicate date metadata".to_owned(),
                        ));
                    }
                    Ok(first.and_then(|child| child.value.as_deref()))
                };
                let calendar = metadata(b"calendar")?;
                if !matches!(calendar, None | Some("gregorian" | "gregorianUs")) {
                    return Err(OxmlError::InvalidValue(
                        "unsupported date calendar".to_owned(),
                    ));
                }
                let Some(pattern) = metadata(b"dateFormat")? else {
                    return Err(OxmlError::InvalidValue("missing date format".to_owned()));
                };
                crate::date_format::check_time_shown(pattern, at)?;
                let display =
                    crate::date_format::format_word_date(pattern, metadata(b"lid")?, calendar, at)?;
                let full_date = at.full_date();
                let bound = match metadata(b"storeMappedDataAs")? {
                    None | Some("text") => display.clone(),
                    Some("date") => at.date(),
                    Some("dateTime") => full_date.clone(),
                    _ => {
                        return Err(OxmlError::InvalidValue(
                            "unsupported date storage mapping".to_owned(),
                        ));
                    }
                };
                Ok((display, bound, Some(full_date)))
            }
            Some(SdtType::DropDownList | SdtType::ComboBox) => {
                let label = if self.control_type == Some(SdtType::ComboBox) {
                    "combo box"
                } else {
                    "dropdown"
                };
                let children = self.value_children()?;
                let items: Vec<_> = children
                    .iter()
                    .filter(|child| child.name == b"listItem")
                    .collect();
                let choices: Vec<_> = items
                    .iter()
                    .filter_map(|item| item.display.as_deref().or(item.value.as_deref()))
                    .collect();
                let mut selected = None;
                for item in items {
                    let display = item.display.as_deref().or(item.value.as_deref());
                    if item.value.as_deref() == Some(value) || display == Some(value) {
                        let pair = match (display, item.value.as_deref()) {
                            (Some(display), Some(bound)) => (display.to_owned(), bound.to_owned()),
                            _ => {
                                return Err(OxmlError::InvalidValue(format!(
                                    "{label} item has no value, choices: {choices:?}"
                                )));
                            }
                        };
                        if selected.as_ref().is_some_and(|previous| previous != &pair) {
                            return Err(OxmlError::InvalidValue(format!(
                                "ambiguous {label} value, choices: {choices:?}"
                            )));
                        }
                        selected = Some(pair);
                    }
                }
                // A combo box also takes text outside its items, as Word lets
                // one; a drop-down list takes only an item.
                let (display, bound) = match selected {
                    Some(pair) => pair,
                    None if self.control_type == Some(SdtType::ComboBox) => {
                        (value.to_owned(), value.to_owned())
                    }
                    None => {
                        return Err(OxmlError::InvalidValue(format!(
                            "unknown {label} value {value:?}, choices: {choices:?}"
                        )));
                    }
                };
                Ok((display, bound, None))
            }
            _ => Ok((value.to_owned(), value.to_owned(), None)),
        }
    }

    /// Remove qualified placeholder metadata and optionally set a selected date.
    pub fn set_value_metadata(&mut self, full_date: Option<&str>) -> Result<()> {
        let date_attribute = if let Some(value) = full_date {
            if self.control_type != Some(SdtType::Date)
                || WordDateTime::parse_full_date(value).is_none()
            {
                return Err(OxmlError::InvalidValue(
                    "fullDate must be a Gregorian YYYY-MM-DDTHH:MM:SSZ".to_owned(),
                ));
            }
            let element = self.type_element.as_ref().ok_or_else(|| {
                OxmlError::InvalidValue("date type payload is missing".to_owned())
            })?;
            let mut start = BytesStart::new(element.name.as_str());
            for (name, value) in &element.attributes {
                start.push_attribute((name.as_str(), value.as_str()));
            }
            let prefixes = word_prefixes_at(&start, &self.source_prefixes)?;
            let matching: Vec<_> = element
                .attributes
                .iter()
                .enumerate()
                .filter(|(_, (name, _))| is_word_attribute(name.as_bytes(), b"fullDate", &prefixes))
                .map(|(index, _)| index)
                .collect();
            if matching.len() > 1 {
                return Err(OxmlError::InvalidValue(
                    "duplicate qualified fullDate attribute".to_owned(),
                ));
            }
            let prefix = prefixes
                .iter()
                .find(|prefix| !prefix.is_empty() && !prefix.starts_with('\0'))
                .cloned();
            Some((matching.first().copied(), prefix))
        } else {
            None
        };
        let type_name = self
            .type_element
            .as_ref()
            .map(|element| -> Result<Option<String>> {
                let mut start = BytesStart::new(element.name.as_str());
                for (name, value) in &element.attributes {
                    start.push_attribute((name.as_str(), value.as_str()));
                }
                let prefixes = word_prefixes_at(&start, &self.source_prefixes)?;
                let local = element
                    .control_type
                    .element_name()
                    .split_once(':')
                    .unwrap()
                    .1;
                if !matches!(
                    element.control_type,
                    SdtType::CheckBox | SdtType::RepeatingSection | SdtType::RepeatingSectionItem
                ) && !is_word_element(element.name.as_bytes(), local.as_bytes(), &prefixes)
                {
                    let prefix = prefixes
                        .iter()
                        .find(|prefix| !prefix.starts_with('\0'))
                        .ok_or_else(|| {
                            OxmlError::InvalidValue(
                                "selected type has no qualified Word prefix".to_owned(),
                            )
                        })?;
                    Ok(Some(if prefix.is_empty() {
                        local.to_owned()
                    } else {
                        format!("{prefix}:{local}")
                    }))
                } else {
                    Ok(None)
                }
            })
            .transpose()?
            .flatten();
        let mut remove = Vec::new();
        for (index, slot) in self.slots.iter().enumerate() {
            if let PropertySlot::Raw(raw) = slot {
                let mut reader = Reader::from_reader(raw.as_slice());
                if let Event::Start(start) | Event::Empty(start) = reader.read_event()? {
                    let prefixes = word_prefixes_at(&start, &self.source_prefixes)?;
                    if is_word_element(start.name().as_ref(), b"showingPlcHdr", &prefixes) {
                        remove.push(index);
                    }
                }
            }
        }
        if let (Some(value), Some((existing, prefix))) = (full_date, date_attribute) {
            let element = self.type_element.as_mut().unwrap();
            if let Some(index) = existing {
                element.attributes[index].1 = value.to_owned();
            } else if let Some(prefix) = prefix {
                element
                    .attributes
                    .push((format!("{prefix}:fullDate"), value.to_owned()));
            } else {
                let mut prefix = "rdocxW".to_owned();
                while element
                    .namespace_bindings
                    .iter()
                    .any(|(bound, _)| bound == &prefix)
                    || element
                        .attributes
                        .iter()
                        .any(|(name, _)| name == &format!("xmlns:{prefix}"))
                {
                    prefix.push('W');
                }
                element
                    .attributes
                    .push((format!("xmlns:{prefix}"), W_NS.to_owned()));
                element
                    .namespace_bindings
                    .push((prefix.clone(), W_NS.to_owned()));
                element
                    .attributes
                    .push((format!("{prefix}:fullDate"), value.to_owned()));
            }
        }
        if let Some(name) = type_name {
            self.type_element.as_mut().unwrap().name = name;
        }
        for index in remove.into_iter().rev() {
            self.slots.remove(index);
        }
        Ok(())
    }

    fn type_children_xml(&self) -> Result<(Vec<u8>, usize)> {
        let element = self.type_element.as_ref().ok_or_else(|| {
            OxmlError::InvalidValue("content-control type payload is missing".to_owned())
        })?;
        let mut bindings = element.namespace_bindings.clone();
        for prefix in &self.source_prefixes {
            if !prefix.starts_with('\0') && !bindings.iter().any(|(bound, _)| bound == prefix) {
                bindings.push((prefix.clone(), W_NS.to_owned()));
            }
        }
        let mut start = BytesStart::new("scope");
        for (prefix, namespace) in bindings {
            let name = if prefix.is_empty() {
                "xmlns".to_owned()
            } else {
                format!("xmlns:{prefix}")
            };
            start.push_attribute((name.as_str(), namespace.as_str()));
        }
        let mut writer = Writer::new(Vec::new());
        writer.write_event(Event::Start(start))?;
        let offset = writer.get_ref().len();
        writer.get_mut().extend_from_slice(&element.children);
        writer.write_event(Event::End(BytesEnd::new("scope")))?;
        Ok((writer.into_inner(), offset))
    }

    fn value_children(&self) -> Result<Vec<ValueChild>> {
        for slot in &self.slots {
            if let PropertySlot::Raw(raw) = slot {
                let mut reader = Reader::from_reader(raw.as_slice());
                if let Event::Start(start) | Event::Empty(start) = reader.read_event()? {
                    let prefixes = word_prefixes_at(&start, &self.source_prefixes)?;
                    let local = start.local_name();
                    if is_supported_type_element(start.name().as_ref(), local.as_ref(), &prefixes)
                        && SdtType::from_element(local.as_ref()).is_some()
                    {
                        return Err(OxmlError::InvalidValue(
                            "conflicting content-control type metadata".to_owned(),
                        ));
                    }
                }
            }
        }
        let (xml, _) = self.type_children_xml()?;
        let mut reader = NsReader::from_reader(xml.as_slice());
        let mut depth = 0usize;
        let mut children = Vec::new();
        let is_word_namespace = |namespace: quick_xml::name::ResolveResult<'_>| {
            matches!(namespace, quick_xml::name::ResolveResult::Bound(ns)
                if std::str::from_utf8(ns.as_ref()).ok()
                    .and_then(|uri| quick_xml::escape::unescape(uri).ok())
                    .is_some_and(|uri| uri == W_NS))
        };
        loop {
            let (namespace, event) = reader.read_resolved_event()?;
            let qualified = is_word_namespace(namespace);
            let (child, empty) = match event {
                Event::Start(child) => (child, false),
                Event::Empty(child) => (child, true),
                Event::End(_) => {
                    depth = depth.saturating_sub(1);
                    continue;
                }
                Event::Eof => return Ok(children),
                _ => continue,
            };
            if depth == 1 && qualified {
                let mut record = ValueChild {
                    name: child.local_name().as_ref().to_vec(),
                    value: None,
                    display: None,
                };
                for attribute in child.attributes() {
                    let attribute = attribute?;
                    let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                    if !is_word_namespace(namespace) {
                        continue;
                    }
                    let destination = match local.as_ref() {
                        b"val" if record.name != b"listItem" => &mut record.value,
                        b"value" if record.name == b"listItem" => &mut record.value,
                        b"displayText" => &mut record.display,
                        _ => continue,
                    };
                    if destination.is_some() {
                        return Err(OxmlError::InvalidValue(
                            "duplicate qualified value metadata attribute".to_owned(),
                        ));
                    }
                    *destination = Some(
                        attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, child.decoder())?
                            .into_owned(),
                    );
                }
                children.push(record);
            }
            if !empty {
                depth += 1;
            }
        }
    }

    /// The control's form facts: lock, placeholder, kind and its payload.
    pub fn form_facts(&self) -> Result<SdtFormFacts> {
        let lock = match self.raw_property(b"lock")?.flatten().as_deref() {
            Some("sdtLocked") => SdtLock::SdtLocked,
            Some("contentLocked") => SdtLock::ContentLocked,
            Some("sdtContentLocked") => SdtLock::SdtContentLocked,
            _ => SdtLock::Unlocked,
        };
        let showing_placeholder = self.showing_placeholder()?;
        let payload = match self.control_type {
            None | Some(SdtType::RichText) => SdtFormPayload::RichText,
            Some(SdtType::PlainText) => SdtFormPayload::PlainText {
                multiline: self
                    .type_element
                    .iter()
                    .flat_map(|element| &element.attributes)
                    .find(|(name, _)| local_part(name) == "multiLine")
                    .and_then(|(_, value)| crate::shared::ST_OnOff::parse(value))
                    .unwrap_or(false),
            },
            Some(SdtType::CheckBox) => SdtFormPayload::CheckBox {
                checked: self.checkbox_checked()?,
                checked_symbol: self.checkbox_state(true)?,
                unchecked_symbol: self.checkbox_state(false)?,
            },
            Some(SdtType::DropDownList) => SdtFormPayload::DropDownList {
                items: self.list_items()?,
            },
            Some(SdtType::ComboBox) => SdtFormPayload::ComboBox {
                items: self.list_items()?,
            },
            Some(SdtType::Date) => SdtFormPayload::Date {
                format: self
                    .value_children()?
                    .into_iter()
                    .find(|child| child.name == b"dateFormat")
                    .and_then(|child| child.value),
            },
            Some(SdtType::Picture) => SdtFormPayload::Picture,
            Some(_) => SdtFormPayload::Other,
        };
        Ok(SdtFormFacts {
            lock,
            showing_placeholder,
            payload,
        })
    }

    /// `w:showingPlcHdr`: whether the content shows the placeholder, not a
    /// value.
    pub fn showing_placeholder(&self) -> Result<bool> {
        Ok(match self.raw_property(b"showingPlcHdr")? {
            None => false,
            Some(None) => true,
            Some(Some(value)) => crate::shared::ST_OnOff::parse(&value).unwrap_or(false),
        })
    }

    /// `w:rPr`: the run properties of text that replaces the placeholder
    /// (ECMA-376 17.5.2.27), or `None` when the control has none.
    pub fn run_properties(&self) -> Result<Option<CT_RPr>> {
        for slot in &self.slots {
            let PropertySlot::Raw(raw) = slot else {
                continue;
            };
            let mut reader = Reader::from_reader(raw.as_slice());
            let (Event::Start(start) | Event::Empty(start)) = reader.read_event()? else {
                continue;
            };
            let prefixes = word_prefixes_at(&start, &self.source_prefixes)?;
            if is_word_element(start.name().as_ref(), b"rPr", &prefixes) {
                return parse_scoped_rpr_with_observation(raw, &self.source_prefixes, &[], None)
                    .map(Some);
            }
        }
        Ok(None)
    }

    /// A raw property element of the main namespace by local name: `Some`
    /// with its `val` when present.
    fn raw_property(&self, wanted: &[u8]) -> Result<Option<Option<String>>> {
        for slot in &self.slots {
            let PropertySlot::Raw(raw) = slot else {
                continue;
            };
            let mut reader = Reader::from_reader(raw.as_slice());
            let (Event::Start(start) | Event::Empty(start)) = reader.read_event()? else {
                continue;
            };
            let prefixes = word_prefixes_at(&start, &self.source_prefixes)?;
            if !is_word_element(start.name().as_ref(), wanted, &prefixes) {
                continue;
            }
            return Ok(Some(word_attribute_value(&start, b"val", &prefixes)?));
        }
        Ok(None)
    }

    /// The `w:listItem` entries. A missing display shows the value, as the
    /// schema says; a missing value is empty.
    fn list_items(&self) -> Result<Vec<SdtListItem>> {
        Ok(self
            .value_children()?
            .into_iter()
            .filter(|child| child.name == b"listItem")
            .map(|child| {
                let value = child.value.unwrap_or_default();
                let display = child.display.unwrap_or_else(|| value.clone());
                SdtListItem { display, value }
            })
            .collect())
    }

    /// `w14:checked`: unchecked when absent, checked when present without
    /// a value, as an on/off property reads.
    fn checkbox_checked(&self) -> Result<bool> {
        let Some(child) = self.checkbox_child(b"checked")? else {
            return Ok(false);
        };
        let Some(range) = child.value else {
            return Ok(true);
        };
        let element = self.type_element.as_ref().ok_or_else(|| {
            OxmlError::InvalidValue("checkbox type payload is missing".to_owned())
        })?;
        let value = std::str::from_utf8(&element.children[range])?;
        crate::shared::ST_OnOff::parse(value).ok_or_else(|| {
            OxmlError::InvalidValue(format!("checkbox checked value {value:?} is not on or off"))
        })
    }

    /// The selected checkbox glyph and its physical state font travel together.
    pub fn checkbox_state(&self, checked: bool) -> Result<(char, String)> {
        let name = if checked {
            b"checkedState".as_slice()
        } else {
            b"uncheckedState".as_slice()
        };
        let child = self.checkbox_child(name)?;
        let element = self.type_element.as_ref().ok_or_else(|| {
            OxmlError::InvalidValue("checkbox type payload is missing".to_owned())
        })?;
        let read = |range: &Range<usize>| -> Result<String> {
            Ok(
                quick_xml::escape::unescape(std::str::from_utf8(&element.children[range.clone()])?)
                    .map_err(|error| {
                        OxmlError::InvalidValue(format!("checkbox state decode failed: {error}"))
                    })?
                    .into_owned(),
            )
        };
        let code = child
            .as_ref()
            .and_then(|child| child.value.as_ref())
            .map(read)
            .transpose()?
            .unwrap_or_else(|| if checked { "2612" } else { "2610" }.to_owned());
        let glyph = u32::from_str_radix(&code, 16)
            .ok()
            .and_then(char::from_u32)
            .ok_or_else(|| {
                OxmlError::InvalidValue(format!(
                    "checkbox state glyph {code:?} is not a Unicode scalar"
                ))
            })?;
        oxoffice_core::xml::reject_non_xml_characters("checkbox state glyph", &glyph.to_string())?;
        let font = child
            .as_ref()
            .and_then(|child| child.font.as_ref())
            .map(read)
            .transpose()?
            .unwrap_or_else(|| "MS Gothic".to_owned());
        oxoffice_core::xml::reject_non_xml_characters("checkbox state font", &font)?;
        if font.trim().is_empty() {
            return Err(OxmlError::InvalidValue(
                "checkbox state font is empty".to_owned(),
            ));
        }
        Ok((glyph, font))
    }

    /// Change only the qualified checked value, preserving all other raw bytes.
    pub fn set_checkbox_checked(&mut self, checked: bool) -> Result<()> {
        let child = self.checkbox_child(b"checked")?;
        let element = self.type_element.as_mut().ok_or_else(|| {
            OxmlError::InvalidValue("checkbox type payload is missing".to_owned())
        })?;
        let value = if checked {
            b"1".as_slice()
        } else {
            b"0".as_slice()
        };
        if let Some(child) = child {
            if let Some(range) = child.value {
                element.children.splice(range, value.iter().copied());
            } else {
                let mut prefix = "rdocxW14".to_owned();
                let mut suffix = 0usize;
                while element
                    .namespace_bindings
                    .iter()
                    .any(|(bound, _)| bound == &prefix)
                    || element
                        .children
                        .windows(prefix.len())
                        .any(|bytes| bytes == prefix.as_bytes())
                {
                    suffix += 1;
                    prefix = format!("rdocxW14{suffix}");
                }
                let attribute = format!(
                    " xmlns:{prefix}=\"http://schemas.microsoft.com/office/word/2010/wordml\" {prefix}:val=\"{}\"",
                    u8::from(checked)
                );
                element.children.splice(
                    child.insert_attribute..child.insert_attribute,
                    attribute.bytes(),
                );
            }
        } else {
            let checked_xml = format!(
                "<rdocxW14:checked xmlns:rdocxW14=\"http://schemas.microsoft.com/office/word/2010/wordml\" rdocxW14:val=\"{}\"/>",
                u8::from(checked)
            );
            element.children.splice(0..0, checked_xml.bytes());
        }
        Ok(())
    }

    fn checkbox_child(&self, wanted: &[u8]) -> Result<Option<CheckboxChild>> {
        const W14: &[u8] = b"http://schemas.microsoft.com/office/word/2010/wordml";
        if self.control_type != Some(SdtType::CheckBox) {
            return Err(OxmlError::InvalidValue(
                "only a checkbox has a checked state".to_owned(),
            ));
        }
        let (xml, offset) = self.type_children_xml()?;
        let mut reader = NsReader::from_reader(xml.as_slice());
        let mut depth = 0usize;
        let mut found = None;
        loop {
            let before = reader.buffer_position() as usize;
            let (namespace, event) = reader.read_resolved_event()?;
            let qualified = matches!(namespace, quick_xml::name::ResolveResult::Bound(ns) if ns.as_ref() == W14);
            let empty = matches!(event, Event::Empty(_));
            match event {
                Event::Start(child) | Event::Empty(child) => {
                    if depth == 1 && qualified && child.local_name().as_ref() == wanted {
                        if found.is_some() {
                            return Err(OxmlError::InvalidValue(
                                "duplicate qualified checkbox state child".to_owned(),
                            ));
                        }
                        let mut record = CheckboxChild {
                            value: None,
                            font: None,
                            insert_attribute: before
                                + oxoffice_core::xml_element::start_tag(&xml[before..])?
                                    .attributes_end
                                - offset,
                        };
                        for attribute in child.attributes() {
                            let attribute = attribute?;
                            let (namespace, local) =
                                reader.resolver().resolve_attribute(attribute.key);
                            if !matches!(namespace, quick_xml::name::ResolveResult::Bound(ns) if ns.as_ref() == W14)
                            {
                                continue;
                            }
                            let destination = match local.as_ref() {
                                b"val" => &mut record.value,
                                b"font" => &mut record.font,
                                _ => continue,
                            };
                            if destination.is_some() {
                                return Err(OxmlError::InvalidValue(
                                    "duplicate qualified checkbox state attribute".to_owned(),
                                ));
                            }
                            let relative = attribute.value.as_ptr() as usize
                                - child.as_ref().as_ptr() as usize;
                            let begin = before + 1 + relative - offset;
                            *destination = Some(begin..begin + attribute.value.len());
                        }
                        found = Some(record);
                    }
                    if !empty {
                        depth += 1;
                    }
                }
                Event::End(_) => depth = depth.saturating_sub(1),
                Event::Eof => return Ok(found),
                _ => {}
            }
        }
    }
    fn from_xml_with_prefixes(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        inherited: &[String],
    ) -> Result<Self> {
        let prefixes = word_prefixes_at(start, inherited)?;
        let mut properties = Self {
            extra_attributes: capture_attributes(start, &prefixes, &[])?,
            source_prefixes: prefixes.to_vec(),
            ..Self::default()
        };
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Start(child)) => {
                    let child_prefixes = word_prefixes_at(&child, &prefixes)?;
                    if properties.parse_modelled(&child, &child_prefixes)? {
                        if let (Some(control_type), Some(PropertySlot::Type)) =
                            (properties.control_type, properties.slots.last())
                        {
                            let raw = capture_element(reader, &child)?;
                            let element = std::str::from_utf8(&raw)
                                .ok()
                                .and_then(|raw| oxoffice_core::xml_element::element_span(raw).ok());
                            let children = element
                                .map(|element| &raw[element.content])
                                .ok_or_else(|| {
                                    OxmlError::MissingElement(format!(
                                        "{} end",
                                        String::from_utf8_lossy(child.name().as_ref())
                                    ))
                                })?;
                            properties.type_element = Some(TypeElement {
                                control_type,
                                name: std::str::from_utf8(child.name().as_ref())?.to_owned(),
                                attributes: capture_attributes(&child, &child_prefixes, &[])?,
                                children: children.to_vec(),
                                namespace_bindings: namespace_bindings(&child_prefixes),
                            });
                        } else {
                            reader.read_to_end_into(child.name(), &mut Vec::new())?;
                        }
                    } else {
                        properties
                            .slots
                            .push(PropertySlot::Raw(capture_element(reader, &child)?));
                    }
                }
                Ok(Event::Empty(child)) => {
                    let child_prefixes = word_prefixes_at(&child, &prefixes)?;
                    if properties.parse_modelled(&child, &child_prefixes)? {
                        if let (Some(control_type), Some(PropertySlot::Type)) =
                            (properties.control_type, properties.slots.last())
                        {
                            properties.type_element = Some(TypeElement {
                                control_type,
                                name: std::str::from_utf8(child.name().as_ref())?.to_owned(),
                                attributes: capture_attributes(&child, &child_prefixes, &[])?,
                                children: Vec::new(),
                                namespace_bindings: namespace_bindings(&child_prefixes),
                            });
                        }
                    } else {
                        properties.slots.push(PropertySlot::Raw(capture_empty(
                            &child,
                            reader.empty_writing(),
                        )?));
                    }
                }
                Ok(Event::End(end)) if matches_local_name(end.name().as_ref(), b"sdtPr") => break,
                Ok(Event::Eof) => {
                    return Err(OxmlError::MissingElement("w:sdtPr end".to_owned()));
                }
                Ok(event) => push_event_raw(&mut properties.slots, event)?,
                Err(error) => return Err(error.into()),
            }
            buffer.clear();
        }
        Ok(properties)
    }

    fn parse_modelled(&mut self, child: &BytesStart<'_>, prefixes: &[String]) -> Result<bool> {
        let name = child.name();
        let local = name
            .as_ref()
            .rsplit(|byte| *byte == b':')
            .next()
            .unwrap_or(name.as_ref());
        if let Some(scalar) = ControlScalar::for_local(local)
            .filter(|_| is_word_element(name.as_ref(), local, prefixes))
        {
            scalar
                .decode(word_attribute_value(child, b"val", prefixes)?)?
                .apply(self);
        } else if is_word_element(name.as_ref(), b"dataBinding", prefixes) {
            self.data_binding = Some(CT_DataBinding::parse(child, prefixes)?);
            self.slots.push(PropertySlot::DataBinding);
        } else if let Some(control_type) = SdtType::from_element(local)
            && is_supported_type_element(name.as_ref(), local, prefixes)
        {
            if self.control_type.is_some() {
                return Ok(false);
            }
            self.control_type = Some(control_type);
            self.slots.push(PropertySlot::Type);
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    fn publication_budget(&self) -> Result<usize> {
        let raw_bytes = self
            .slots
            .iter()
            .try_fold(0usize, |total, slot| {
                total.checked_add(if let PropertySlot::Raw(raw) = slot {
                    raw.len()
                } else {
                    0
                })
            })
            .ok_or_else(|| OxmlError::InvalidValue("control property source overflow".into()))?;
        let current = attribute_bytes(&self.extra_attributes)?
            .checked_add(self.alias.as_ref().map_or(0, String::len))
            .and_then(|bytes| bytes.checked_add(self.tag.as_ref().map_or(0, String::len)))
            .ok_or_else(|| OxmlError::InvalidValue("control property work overflow".into()))?;
        crate::text::retained_owner_work(raw_bytes, &self.source_prefixes, current)
    }

    fn preflight_publication(&self) -> Result<()> {
        let (start, _) = crate::text::generated_word_start(
            {
                let mut root = BytesStart::new("w:sdtPr");
                for (name, value) in &self.extra_attributes {
                    root.push_attribute((name.as_str(), value.as_str()));
                }
                root
            },
            &self.source_prefixes,
            XmlWriteRole::Publication,
            None,
        )?;
        let _scope = crate::text::emitted_binding_scope(&start, false, false)?;
        let raw = self
            .slots
            .iter()
            .filter_map(|slot| {
                if let PropertySlot::Raw(raw) = slot {
                    Some(raw.as_slice())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let raw_bytes = checked_lengths(&raw.iter().map(|raw| raw.len()).collect::<Vec<_>>())?;
        let mut current = attribute_bytes(&self.extra_attributes)?;
        current = current
            .checked_add(self.alias.as_ref().map_or(0, String::len))
            .and_then(|bytes| bytes.checked_add(self.tag.as_ref().map_or(0, String::len)))
            .ok_or_else(|| OxmlError::InvalidValue("control property work overflow".into()))?;
        let budget = crate::text::retained_owner_work(raw_bytes, &self.source_prefixes, current)?;
        preflight_owned_raw(&raw, &self.source_prefixes, budget, raw_bytes)?;
        if let Some(element) = self
            .type_element
            .as_ref()
            .filter(|element| Some(element.control_type) == self.control_type)
        {
            let mut start = BytesStart::new(element.name.as_str());
            for (name, value) in &element.attributes {
                start.push_attribute((name.as_str(), value.as_str()));
            }
            let mut writer = Writer::new(Vec::new());
            if element.children.is_empty() {
                writer.write_event(Event::Empty(start))?;
            } else {
                writer.write_event(Event::Start(start))?;
                writer.get_mut().extend_from_slice(&element.children);
                writer.write_event(Event::End(BytesEnd::new(element.name.as_str())))?;
            }
            let bytes = writer.into_inner();
            let bindings = element
                .namespace_bindings
                .iter()
                .map(|(prefix, namespace)| (prefix.as_str(), namespace.as_str()))
                .collect::<Vec<_>>();
            let destination = crate::text::emitted_bindings();
            let type_budget = crate::text::retained_binding_work(
                bytes.len(),
                &element.namespace_bindings,
                attribute_bytes(&element.attributes)?,
            )?;
            crate::text::retained_binding_plan(
                &bytes,
                &bindings,
                &destination.bindings_for(&bytes),
                type_budget,
            )?;
        }
        Ok(())
    }

    fn write_xml_role<W: Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let budget = if role == XmlWriteRole::Publication {
            self.publication_budget()?
        } else {
            0
        };
        if role == XmlWriteRole::Publication {
            self.preflight_publication()?;
        }

        let mut start = crate::text::FieldXmlStart::new("w:sdtPr", emission)?;
        for (name, value) in &self.extra_attributes {
            start.push(name, value)?;
        }
        let word_prefix = start.qualify(&self.source_prefixes, role, None)?;
        let _property_scope = crate::text::emitted_binding_scope(start.element(), false, false)?;
        start.write(writer, false)?;
        let mut alias_written = false;
        let mut tag_written = false;
        let mut id_written = false;
        let mut type_written = false;
        let mut binding_written = false;
        for slot in &self.slots {
            match slot {
                PropertySlot::Alias if !alias_written => {
                    write_val_element(
                        writer,
                        &word_prefix,
                        "alias",
                        self.alias.as_deref(),
                        emission,
                    )?;
                    alias_written = true;
                }
                PropertySlot::Tag if !tag_written => {
                    write_val_element(writer, &word_prefix, "tag", self.tag.as_deref(), emission)?;
                    tag_written = true;
                }
                PropertySlot::Id if !id_written => {
                    if let Some(id) = self.id {
                        let mut buffer = itoa::Buffer::new();
                        let value = buffer.format(id);
                        write_val_element(writer, &word_prefix, "id", Some(value), emission)?;
                    }
                    id_written = true;
                }
                PropertySlot::Type if !type_written => {
                    self.write_type(writer, role, &word_prefix, emission)?;
                    type_written = true;
                }
                PropertySlot::DataBinding if !binding_written => {
                    if let Some(binding) = &self.data_binding {
                        binding.to_xml(
                            writer,
                            &self.source_prefixes,
                            role,
                            source_context,
                            emission,
                        )?;
                    }
                    binding_written = true;
                }
                PropertySlot::Raw(raw) => {
                    write_owned_raw_in(writer, raw, &self.source_prefixes, role, budget, emission)?
                }
                _ => {}
            }
        }
        if !alias_written {
            write_val_element(
                writer,
                &word_prefix,
                "alias",
                self.alias.as_deref(),
                emission,
            )?;
        }
        if !tag_written {
            write_val_element(writer, &word_prefix, "tag", self.tag.as_deref(), emission)?;
        }
        if !id_written && let Some(id) = self.id {
            let mut buffer = itoa::Buffer::new();
            let value = buffer.format(id);
            write_val_element(writer, &word_prefix, "id", Some(value), emission)?;
        }
        if !type_written {
            self.write_type(writer, role, &word_prefix, emission)?;
        }
        if !binding_written && let Some(binding) = &self.data_binding {
            binding.to_xml(
                writer,
                &self.source_prefixes,
                role,
                source_context,
                emission,
            )?;
        }
        if let Some(emission) = emission {
            emission.write_word_end(writer, &word_prefix, "sdtPr")?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(format!("{word_prefix}:sdtPr"))))?;
        }
        Ok(())
    }

    fn write_type<W: Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        word_prefix: &str,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let Some(control_type) = self.control_type else {
            return Ok(());
        };
        let original = self
            .type_element
            .as_ref()
            .filter(|element| element.control_type == control_type);
        let raw = if let Some(emission) = emission {
            let (output, _outer) = emission.new_source_record_output();
            let mut element = Writer::new(output);
            self.write_type_record(
                &mut element,
                control_type,
                original,
                role,
                word_prefix,
                Some(emission),
            )?;
            element.into_inner().into_mapped_parts().0
        } else {
            let mut element = Writer::new(Vec::new());
            self.write_type_record(
                &mut element,
                control_type,
                original,
                role,
                word_prefix,
                None,
            )?;
            element.into_inner()
        };
        if let Some(original) = original.filter(|_| role == XmlWriteRole::Publication) {
            let bindings = original
                .namespace_bindings
                .iter()
                .map(|(prefix, namespace)| (prefix.as_str(), namespace.as_str()))
                .collect::<Vec<_>>();
            let destination = crate::text::emitted_bindings();
            let budget = crate::text::retained_binding_work(
                raw.len(),
                &original.namespace_bindings,
                attribute_bytes(&original.attributes)?,
            )?;
            let plan = crate::text::retained_binding_plan(
                &raw,
                &bindings,
                &destination.bindings_for(&raw),
                budget,
            )?;
            let (produced, prepaid) = crate::text::emit_retained_roots_in(&raw, plan, emission)?;
            let _transport = emission
                .filter(|_| prepaid)
                .map(|emission| emission.prepaid_xml_transport(&produced))
                .transpose()?;
            // The unchanged produced type record was already funded above.
            let _unchanged = emission
                .filter(|_| !prepaid)
                .map(|emission| emission.prepaid_xml_transport(&produced))
                .transpose()?;
            writer.get_mut().write_all(&produced)?;
        } else {
            let _transport = emission
                .map(|emission| emission.prepaid_xml_transport(&raw))
                .transpose()?;
            writer.get_mut().write_all(&raw)?;
        }
        Ok(())
    }

    // This same substantive record producer has Vec and checked normalized sinks.
    fn write_type_record<W: Write>(
        &self,
        writer: &mut Writer<W>,
        control_type: SdtType,
        original: Option<&TypeElement>,
        role: XmlWriteRole,
        word_prefix: &str,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let authored_name = control_type.element_name();
        let mut start = if let Some(original) = original {
            crate::text::FieldXmlStart::new(&original.name, emission)?
        } else if let Some(local) = authored_name.strip_prefix("w:") {
            crate::text::FieldXmlStart::word(word_prefix, local, emission)?
        } else {
            crate::text::FieldXmlStart::new(authored_name, emission)?
        };
        let children = if let Some(original) = original {
            for (key, value) in &original.attributes {
                start.push(key, value)?;
            }
            original.children.as_slice()
        } else {
            &[]
        };
        if original.is_none() {
            let extension = if authored_name.starts_with("w14:") {
                Some((
                    "w14",
                    "http://schemas.microsoft.com/office/word/2010/wordml",
                ))
            } else if authored_name.starts_with("w15:") {
                Some((
                    "w15",
                    "http://schemas.microsoft.com/office/word/2012/wordml",
                ))
            } else {
                None
            };
            if let Some(binding) = extension {
                let destination = crate::text::emitted_bindings();
                let bindings = if role == XmlWriteRole::Source {
                    crate::text::field_scope_bindings(&self.source_prefixes)
                } else {
                    destination.bindings_for(if binding.0 == "w14" {
                        b"<w14:checkbox/>"
                    } else {
                        b"<w15:repeatingSection/>"
                    })
                };
                if !bindings.contains(&binding) {
                    start.push_word("xmlns", binding.0, binding.1)?;
                }
            }
        }
        if children.is_empty() {
            start.write(writer, true)?;
        } else {
            start.write(writer, false)?;
            writer.get_mut().write_all(children)?;
            if let Some(original) = original {
                if let Some(emission) = emission {
                    emission.write_end_name(writer, &original.name)?;
                } else {
                    writer.write_event(Event::End(BytesEnd::new(original.name.as_str())))?;
                }
            } else if let Some(local) = authored_name.strip_prefix("w:") {
                if let Some(emission) = emission {
                    emission.write_word_end(writer, word_prefix, local)?;
                } else {
                    writer
                        .write_event(Event::End(BytesEnd::new(format!("{word_prefix}:{local}"))))?;
                }
            } else if let Some(emission) = emission {
                emission.write_end_name(writer, authored_name)?;
            } else {
                writer.write_event(Event::End(BytesEnd::new(authored_name)))?;
            }
        }
        Ok(())
    }
}

/// A typed or preserved child of `w:sdtContent`.
#[derive(Debug, PartialEq)]
pub enum SdtContent {
    Paragraph(CT_P),
    Table(CT_Tbl),
    Row(CT_Row),
    Cell(CT_Tc),
    Run(CT_R),
    ContentControl(CT_Sdt),
    RawXml(Vec<u8>),
}

impl Clone for SdtContent {
    fn clone(&self) -> Self {
        if let Self::ContentControl(control) = self {
            return Self::ContentControl(control.clone());
        }
        clone_non_control_sdt_content(self)
    }
}

// Keep the large modeled leaf variants out of the recursive control frame.
// This helper remains exhaustive even though its current caller selects leaves.
#[inline(never)]
fn clone_non_control_sdt_content(content: &SdtContent) -> SdtContent {
    match content {
        SdtContent::Paragraph(paragraph) => SdtContent::Paragraph(paragraph.clone()),
        SdtContent::Table(table) => SdtContent::Table(table.clone()),
        SdtContent::Row(row) => SdtContent::Row(row.clone()),
        SdtContent::Cell(cell) => SdtContent::Cell(cell.clone()),
        SdtContent::Run(run) => SdtContent::Run(run.clone()),
        SdtContent::ContentControl(control) => SdtContent::ContentControl(control.clone()),
        SdtContent::RawXml(raw) => SdtContent::RawXml(raw.clone()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SdtOwner {
    Standalone,
    Body,
    Table,
    Row,
    Cell,
    Inline,
}

impl SdtOwner {
    pub(crate) fn owns(self, local: &[u8]) -> bool {
        match self {
            Self::Standalone => matches!(local, b"p" | b"tbl" | b"tr" | b"tc" | b"r"),
            Self::Body | Self::Cell => matches!(local, b"p" | b"tbl"),
            Self::Table => local == b"tr",
            Self::Row => local == b"tc",
            Self::Inline => matches!(
                local,
                b"r" | b"fldSimple" | b"ins" | b"del" | b"moveFrom" | b"moveTo"
            ),
        }
    }

    fn owns_content(self, content: &SdtContent) -> bool {
        matches!(content, SdtContent::ContentControl(_))
            || matches!(
                (self, content),
                (
                    Self::Standalone,
                    SdtContent::Paragraph(_)
                        | SdtContent::Table(_)
                        | SdtContent::Row(_)
                        | SdtContent::Cell(_)
                        | SdtContent::Run(_)
                ) | (
                    Self::Body | Self::Cell,
                    SdtContent::Paragraph(_) | SdtContent::Table(_)
                ) | (Self::Table, SdtContent::Row(_))
                    | (Self::Row, SdtContent::Cell(_))
                    | (Self::Inline, SdtContent::Run(_))
            )
    }
}

#[derive(Debug, Clone, PartialEq)]
enum RootSlot {
    Properties,
    Content,
    Raw(Vec<u8>),
}

/// A recursive WordprocessingML structured document tag.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_Sdt {
    pub properties: Option<CT_SdtPr>,
    pub content: Vec<SdtContent>,
    pub(crate) revisions: Vec<(usize, CT_Revision)>,
    extra_attributes: Vec<(String, String)>,
    content_attributes: Vec<(String, String)>,
    slots: Vec<RootSlot>,
    word_prefixes: Vec<String>,
    content_word_prefixes: Vec<String>,
    inline_run_sources: Vec<InlineRunSource>,
    pending_inline_field_edits: Vec<(Vec<u8>, Vec<u8>)>,
}

#[derive(Debug, Clone, PartialEq)]
struct InlineRunSource {
    content_index: usize,
    original: CT_R,
    raw_xml: Vec<u8>,
}

pub(crate) struct InlineFieldProjection {
    pub(crate) paragraph: CT_P,
    pub(crate) run_coverage: Vec<Range<usize>>,
    source: Vec<u8>,
    content_spans: Vec<Range<usize>>,
}

/// One root child of `w:sdt` in the order the writer writes it.
pub(crate) enum WrittenSdtPart<'a> {
    Properties(&'a CT_SdtPr),
    Content,
    Raw(&'a [u8]),
}

impl CT_SdtPr {
    /// The retained property children the writer writes from their bytes
    /// (`w:rPr`, `w:placeholder` and every other unmodelled child).
    pub(crate) fn written_raw_slots(&self) -> impl Iterator<Item = &[u8]> {
        self.slots.iter().filter_map(|slot| match slot {
            PropertySlot::Raw(raw) => Some(raw.as_slice()),
            _ => None,
        })
    }
}

impl CT_Sdt {
    /// The root children the writer writes, in its order: the properties and
    /// the content once each, and the retained root children (`w:sdtEndPr`
    /// among them) from their bytes.
    pub(crate) fn for_each_written_part<'a>(&'a self, mut part: impl FnMut(WrittenSdtPart<'a>)) {
        let mut properties_written = false;
        let mut content_written = false;
        for slot in &self.slots {
            match slot {
                RootSlot::Properties if !properties_written => {
                    if let Some(properties) = &self.properties {
                        part(WrittenSdtPart::Properties(properties));
                    }
                    properties_written = true;
                }
                RootSlot::Content if !content_written => {
                    part(WrittenSdtPart::Content);
                    content_written = true;
                }
                RootSlot::Raw(raw) => part(WrittenSdtPart::Raw(raw)),
                _ => {}
            }
        }
        if !properties_written && let Some(properties) = &self.properties {
            part(WrittenSdtPart::Properties(properties));
        }
        if !content_written
            && (!self.content.is_empty()
                || self
                    .slots
                    .iter()
                    .any(|slot| matches!(slot, RootSlot::Content)))
        {
            part(WrittenSdtPart::Content);
        }
    }

    /// The stored bytes the writer writes for the direct run at
    /// `content_index`, for a run that holds no field (a field run's match
    /// needs the field's writer).
    pub(crate) fn written_inline_run_source(
        &self,
        content_index: usize,
        run: &CT_R,
    ) -> Option<&[u8]> {
        self.inline_run_sources
            .iter()
            .find(|source| source.content_index == content_index && source.original == *run)
            .map(|source| source.raw_xml.as_slice())
    }
}

impl CT_Sdt {
    /// Borrow every actual stored run capture once, retaining absolute indices.
    #[doc(hidden)]
    pub fn retained_resource_run_xml(&self) -> impl Iterator<Item = (usize, &[u8])> {
        self.inline_run_sources
            .iter()
            .map(|source| (source.content_index, source.raw_xml.as_slice()))
    }

    /// Project direct-run legacy fields with their starting content boundary.
    #[doc(hidden)]
    pub fn inline_legacy_form_fields_with_content_indices(&self) -> Result<Vec<(usize, Field)>> {
        let Some(projection) = self.inline_runs_as_paragraph()? else {
            return Ok(Vec::new());
        };
        let paragraph = &projection.paragraph;
        let coverage = &projection.run_coverage;
        let mut fields = Vec::new();
        for (run_index, run) in paragraph.runs.iter().enumerate() {
            for content in &run.content {
                if let RunContent::Field(field) = content {
                    let mut owned = Vec::new();
                    append_inline_legacy_fields(field, &mut owned)?;
                    let span = coverage.get(run_index).ok_or_else(|| {
                        OxmlError::InvalidValue(
                            "inline legacy owner has no physical source association".into(),
                        )
                    })?;
                    fields.extend(owned.into_iter().map(|field| (span.start, field)));
                }
            }
        }
        Ok(fields)
    }

    /// Replace one projected legacy value in direct inline runs.
    #[doc(hidden)]
    pub fn set_inline_legacy_form_value(
        &mut self,
        ordinal: usize,
        value: LegacyFormFieldValue,
    ) -> Result<bool> {
        self.preflight_inline_fact_copy()?;
        let mut work = crate::text::WordOperationWork::model_fields(self.writer_field_roots())?;
        let mut candidate = self.clone();
        if !candidate.set_inline_legacy_form_value_staged(ordinal, value, &mut work)? {
            return Ok(false);
        }
        *self = candidate;
        Ok(true)
    }

    fn set_inline_legacy_form_value_staged(
        &mut self,
        ordinal: usize,
        value: LegacyFormFieldValue,
        work: &mut crate::text::WordOperationWork,
    ) -> Result<bool> {
        let Some(projection) = self.inline_runs_as_paragraph_in(work)? else {
            return Ok(false);
        };
        let InlineFieldProjection {
            mut paragraph,
            run_coverage: coverage,
            source,
            content_spans,
        } = projection;
        let mut remaining = ordinal;
        let mut changed = false;
        for run in &mut paragraph.runs {
            for content in &mut run.content {
                if let RunContent::Field(field) = content
                    && set_nth_inline_legacy_form(field, &mut remaining, &value, work)?
                {
                    changed = true;
                    break;
                }
            }
            if changed {
                break;
            }
        }
        if !changed {
            return Ok(false);
        }
        paragraph.normalize_field_owners_in(work)?;
        let mut plans = Vec::new();
        let mut field_edits = Vec::new();
        let mut run_index = 0usize;
        while run_index < paragraph.runs.len() {
            let original_span = coverage.get(run_index).cloned().ok_or_else(|| {
                OxmlError::InvalidValue("inline legacy run lacks its physical source extent".into())
            })?;
            let mut logical_end = run_index + 1;
            while logical_end < coverage.len() && coverage[logical_end] == original_span {
                logical_end += 1;
            }
            let canonical = original_span.end == original_span.start + 1
                && matches!(self.content.get(original_span.start), Some(SdtContent::Run(run)) if matches!(run.content.as_slice(), [RunContent::Field(_)]));
            if canonical {
                if logical_end != run_index + 1
                    || !matches!(
                        paragraph.runs[run_index].content.as_slice(),
                        [RunContent::Field(_)]
                    )
                {
                    return Err(OxmlError::InvalidValue(
                        "canonical inline field changed its actual logical owner cardinality"
                            .into(),
                    ));
                }
                plans.push((
                    original_span,
                    vec![SdtContent::Run(paragraph.runs[run_index].clone())],
                    Vec::new(),
                ));
            } else {
                let mut replacements = Vec::new();
                let associated = paragraph.field_source_replacements_at_in(
                    run_index,
                    &mut replacements,
                    work,
                )?;
                if associated != logical_end - run_index {
                    return Err(OxmlError::InvalidValue(
                        "legacy field source group changed its complete physical coverage".into(),
                    ));
                }
                if !replacements.is_empty() {
                    if replacements.len() != 1 {
                        return Err(OxmlError::InvalidValue(
                            "legacy physical span has ambiguous replacement owners".into(),
                        ));
                    }
                    let (original, replacement) = replacements.pop().expect("checked source group");
                    let first = content_spans
                        .get(original_span.start)
                        .ok_or_else(|| {
                            OxmlError::InvalidValue("legacy source start is missing".into())
                        })?
                        .start;
                    let last = content_spans
                        .get(original_span.end - 1)
                        .ok_or_else(|| {
                            OxmlError::InvalidValue("legacy source end is missing".into())
                        })?
                        .end;
                    if source.get(first..last) != Some(original.as_slice()) {
                        return Err(OxmlError::InvalidValue("legacy replacement does not match its actual complete source occurrence".into()));
                    }
                    if original != replacement {
                        if self.content[original_span.clone()]
                            .iter()
                            .any(|child| match child {
                                SdtContent::ContentControl(_) => true,
                                SdtContent::Run(run) => run
                                    .content
                                    .iter()
                                    .any(|content| matches!(content, RunContent::Field(_))),
                                _ => false,
                            })
                            || self
                                .revisions
                                .iter()
                                .any(|(at, _)| original_span.contains(at))
                        {
                            return Err(OxmlError::InvalidValue("unproved legacy physical replacement crosses an independently owned field/control/revision".into()));
                        }
                        let mut reader = Reader::from_reader(replacement.as_slice());
                        reader.config_mut().trim_text(false);
                        let mut children = Vec::new();
                        let mut sources = Vec::new();
                        let mut buffer = Vec::new();
                        loop {
                            let event = reader.read_event_into(&mut buffer)?;
                            let empty = matches!(event, Event::Empty(_));
                            match event {
                                Event::Start(child) | Event::Empty(child) => {
                                    let scope =
                                        word_prefixes_at(&child, &self.content_word_prefixes)?;
                                    let raw = if empty {
                                        capture_empty(&child, reader.empty_writing())?
                                    } else {
                                        capture_element(&mut reader, &child)?
                                    };
                                    if is_word_element(child.name().as_ref(), b"r", &scope) {
                                        let run = parse_inline_run(
                                            &raw,
                                            &self.content_word_prefixes,
                                            None,
                                            None,
                                        )?;
                                        sources.push(InlineRunSource {
                                            content_index: children.len(),
                                            original: run.clone(),
                                            raw_xml: raw,
                                        });
                                        children.push(SdtContent::Run(run));
                                    } else {
                                        children.push(SdtContent::RawXml(raw));
                                    }
                                }
                                Event::Eof => break,
                                event => children.push(SdtContent::RawXml(event_to_raw(event)?)),
                            }
                            buffer.clear();
                        }
                        field_edits.push((original, replacement));
                        plans.push((original_span, children, sources));
                    }
                }
            }
            run_index = logical_end;
        }
        plans.sort_by_key(|(span, _, _)| span.start);
        for (span, children, mut replacements) in plans.into_iter().rev() {
            let old_count = span.len();
            let new_count = children.len();
            // Canonical fields retain their immutable lookup source. Physical
            // fallback records are replaced only at the proved top-level extent.
            if replacements.is_empty()
                && old_count == 1
                && new_count == 1
                && matches!(children[0], SdtContent::Run(ref run) if matches!(run.content.as_slice(), [RunContent::Field(_)]))
            {
                self.content.splice(span, children);
                continue;
            }
            self.inline_run_sources
                .retain(|record| !span.contains(&record.content_index));
            for record in &mut self.inline_run_sources {
                if record.content_index >= span.end {
                    record.content_index = record.content_index - old_count + new_count;
                }
            }
            for (at, _) in &mut self.revisions {
                if *at >= span.end {
                    *at = *at - old_count + new_count;
                }
            }
            for record in &mut replacements {
                record.content_index += span.start;
            }
            self.inline_run_sources.extend(replacements);
            self.content.splice(span, children);
        }
        self.inline_run_sources
            .sort_by_key(|record| record.content_index);
        self.pending_inline_field_edits = field_edits;
        Ok(true)
    }

    fn preflight_inline_fact_copy(&self) -> Result<()> {
        crate::document::admit_word_model(crate::document::WordOwner::Control(self))?;
        let mut borrowed_runs = Vec::new();
        self.collect_runs(&mut borrowed_runs);
        let mut controls = vec![self];
        let mut legacy_revisions = Vec::new();
        while let Some(control) = controls.pop() {
            borrowed_runs.extend(control.original_resource_runs());
            legacy_revisions.extend(control.revisions.iter().map(|(_, revision)| revision));
            for child in &control.content {
                if let SdtContent::ContentControl(child) = child {
                    controls.push(child);
                }
            }
        }
        while let Some(revision) = legacy_revisions.pop() {
            if let crate::revision::RevisionContent::Runs(runs) = revision.content() {
                borrowed_runs.extend(runs);
            }
            legacy_revisions.extend(
                revision
                    .nested_revisions()
                    .iter()
                    .map(|(_, revision)| revision),
            );
        }
        crate::text::preflight_inline_field_copies(&borrowed_runs)?;
        Ok(())
    }

    pub(crate) fn inline_runs_as_paragraph(&self) -> Result<Option<InlineFieldProjection>> {
        let mut work = crate::text::WordOperationWork::model_fields(self.writer_field_roots())?;
        self.inline_runs_as_paragraph_in(&mut work)
    }

    fn inline_runs_as_paragraph_in(
        &self,
        work: &mut crate::text::WordOperationWork,
    ) -> Result<Option<InlineFieldProjection>> {
        let _source_scope = crate::text::root_binding_scope(0);
        if !self
            .content
            .iter()
            .any(|content| matches!(content, SdtContent::Run(_)))
        {
            return Ok(None);
        }
        self.preflight_inline_fact_copy()?;
        let mut prepared = self.clone();
        prepared.normalize_field_owners_in(work)?;
        let source_bindings = crate::text::field_scope_bindings(&self.content_word_prefixes);
        let source_context = Some(source_bindings.as_slice());
        let mut source = Vec::new();
        let mut spans = Vec::new();
        let mut physical_runs = Vec::new();
        let mut facts = Vec::new();
        for (index, child) in std::mem::take(&mut prepared.content)
            .into_iter()
            .enumerate()
        {
            let start = source.len();
            let mut writer = Writer::new(Vec::new());
            let child = match child {
                SdtContent::Run(run) => {
                    if matches!(run.content.as_slice(), [RunContent::Field(_)])
                        && let Some(original) = self
                            .inline_run_sources
                            .iter()
                            .find(|record| record.content_index == index)
                    {
                        writer.get_mut().extend_from_slice(&original.raw_xml);
                    } else {
                        self.write_inline_child(
                            &mut writer,
                            index,
                            &run,
                            XmlWriteRole::Source,
                            0,
                            work,
                            None,
                        )?;
                    }
                    physical_runs.push(Some(writer.get_ref().clone()));
                    SdtContent::Run(run)
                }
                SdtContent::ContentControl(control) => {
                    control.write_xml_role_in(
                        &mut writer,
                        XmlWriteRole::Source,
                        source_context,
                        work,
                        None,
                    )?;
                    physical_runs.push(None);
                    SdtContent::ContentControl(control)
                }
                SdtContent::RawXml(raw) => {
                    if let Some((_, revision)) =
                        prepared.revisions.iter().find(|(at, _)| *at == index)
                    {
                        revision.write_xml_role_in(
                            &mut writer,
                            XmlWriteRole::Source,
                            source_context,
                            work,
                            None,
                        )?;
                    } else {
                        writer.get_mut().extend_from_slice(&raw);
                    }
                    physical_runs.push(None);
                    SdtContent::RawXml(writer.get_ref().clone())
                }
                SdtContent::Paragraph(paragraph) => {
                    paragraph.write_xml_core_in(
                        &mut writer,
                        None,
                        XmlWriteRole::Source,
                        source_context,
                        work,
                        None,
                    )?;
                    physical_runs.push(None);
                    SdtContent::RawXml(writer.get_ref().clone())
                }
                SdtContent::Table(table) => {
                    table.write_xml_role_in(
                        &mut writer,
                        XmlWriteRole::Source,
                        source_context,
                        work,
                        None,
                    )?;
                    physical_runs.push(None);
                    SdtContent::RawXml(writer.get_ref().clone())
                }
                SdtContent::Row(row) => {
                    row.write_xml_role_in(
                        &mut writer,
                        XmlWriteRole::Source,
                        source_context,
                        work,
                        None,
                    )?;
                    physical_runs.push(None);
                    SdtContent::RawXml(writer.get_ref().clone())
                }
                SdtContent::Cell(cell) => {
                    cell.write_xml_role_in(
                        &mut writer,
                        XmlWriteRole::Source,
                        source_context,
                        work,
                        None,
                    )?;
                    physical_runs.push(None);
                    SdtContent::RawXml(writer.get_ref().clone())
                }
            };
            source.extend_from_slice(&writer.into_inner());
            spans.push(start..source.len());
            facts.push(child);
        }
        let (paragraph, _, _, coverage) = crate::text::inline_field_paragraph_from_facts(
            facts,
            prepared.revisions,
            physical_runs,
            &source,
            &spans,
            &self.content_word_prefixes,
            false,
            None,
        )?;
        Ok(Some(InlineFieldProjection {
            paragraph,
            run_coverage: coverage,
            source,
            content_spans: spans,
        }))
    }

    fn inline_run_source(
        &self,
        content_index: usize,
        run: &CT_R,
        work: &mut crate::text::WordOperationWork,
    ) -> Result<Option<&InlineRunSource>> {
        for source in &self.inline_run_sources {
            if source.content_index != content_index || source.original != *run {
                continue;
            }
            let matches = match run.content.as_slice() {
                [RunContent::Field(field)] => match field.source_replacement_in(work) {
                    Ok(replacement) => {
                        replacement.is_some_and(|(_, current)| current == source.raw_xml)
                    }
                    Err(error) if work.resource_refused() => return Err(error),
                    Err(_) => false,
                },
                _ => true,
            };
            if matches {
                return Ok(Some(source));
            }
        }
        Ok(None)
    }

    #[allow(clippy::too_many_arguments)] // The actual parent emission scope is independent of source scope.
    fn write_inline_child<W: Write>(
        &self,
        writer: &mut Writer<W>,
        content_index: usize,
        run: &CT_R,
        role: XmlWriteRole,
        budget: usize,
        work: &mut crate::text::WordOperationWork,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        if let Some(source) = self.inline_run_source(content_index, run, work)? {
            if role == XmlWriteRole::Source {
                let _mapped =
                    work.mapped_inline_field_transport(run, &source.original, &source.raw_xml)?;
                writer.get_mut().write_all(&source.raw_xml)?;
            } else {
                let destination = crate::text::emitted_bindings();
                let plan = crate::text::retained_root_plan(
                    &source.raw_xml,
                    &self.content_word_prefixes,
                    &destination.bindings_for(&source.raw_xml),
                    budget,
                )?;
                let map = work
                    .template_word_capture()
                    .map(|_| work.template_word_raw_map(&source.raw_xml))
                    .transpose()?;
                let (produced, _, mapped) = crate::text::emit_retained_roots_mapped_in(
                    &source.raw_xml,
                    plan,
                    None,
                    map.as_ref(),
                )?;
                let _mapped = mapped
                    .map(|map| work.mapped_source_record(&produced, map))
                    .transpose()?;
                writer.get_mut().write_all(&produced)?;
            }
        } else {
            let destination = crate::text::emitted_bindings();
            let bindings = if role == XmlWriteRole::Source {
                crate::text::field_scope_bindings(&self.content_word_prefixes)
            } else {
                destination.bindings()
            };
            let foreign = bindings
                .iter()
                .find(|(prefix, namespace)| *prefix == "w" && *namespace != W_NS)
                .map(|(_, namespace)| *namespace);
            crate::text::write_mixed_field_run(
                writer,
                run,
                foreign,
                (role == XmlWriteRole::Publication).then_some(bindings.as_slice()),
                (role == XmlWriteRole::Source).then_some(bindings.as_slice()),
                work,
                emission,
            )?;
        }
        Ok(())
    }

    /// Return staged source edits for complex fields owned by direct runs.
    #[doc(hidden)]
    pub fn inline_field_source_replacements(&self) -> Vec<(&[u8], &[u8])> {
        self.pending_inline_field_edits
            .iter()
            .map(|(source, replacement)| (source.as_slice(), replacement.as_slice()))
            .collect()
    }

    /// Return revision wrappers at their direct content boundaries.
    #[doc(hidden)]
    pub fn revisions(&self) -> &[(usize, CT_Revision)] {
        &self.revisions
    }

    /// Mutate physical revision projection metadata on a layout-only clone.
    /// This does not publish an edit or rewrite the producer's raw XML.
    #[doc(hidden)]
    pub fn physical_revisions_mut(&mut self) -> &mut [(usize, CT_Revision)] {
        &mut self.revisions
    }

    /// Remove the raw children of `w:sdtContent` at these content indices,
    /// with the revision written in place of one. Children that are not raw
    /// XML stay. The other children keep their place. Returns how many went.
    #[doc(hidden)]
    pub fn remove_raw_children(&mut self, indices: &[usize]) -> usize {
        let mut indices = indices.to_vec();
        indices.sort_unstable();
        indices.dedup();
        let mut removed = 0;
        for index in indices.into_iter().rev() {
            if !matches!(self.content.get(index), Some(SdtContent::RawXml(_))) {
                continue;
            }
            self.revisions.retain(|(boundary, _)| *boundary != index);
            self.remove_content(index);
            removed += 1;
        }
        removed
    }

    pub(crate) fn append_accepted_run_paths(
        &self,
        prefix: &mut Vec<AcceptedRunPathSegment>,
        output: &mut Vec<AcceptedRunPath>,
    ) {
        for boundary in 0..=self.content.len() {
            for (revision_index, (_, revision)) in self
                .revisions
                .iter()
                .enumerate()
                .filter(|(_, (at, _))| *at == boundary)
            {
                prefix.push(AcceptedRunPathSegment::Revision(revision_index));
                revision.append_accepted_run_paths(prefix, output);
                prefix.pop();
            }
            match self.content.get(boundary) {
                Some(SdtContent::Run(_)) => {
                    prefix.push(AcceptedRunPathSegment::Run(boundary));
                    output.push(AcceptedRunPath {
                        segments: prefix.clone(),
                    });
                    prefix.pop();
                }
                Some(SdtContent::ContentControl(control)) => {
                    prefix.push(AcceptedRunPathSegment::ContentControl(boundary));
                    control.append_accepted_run_paths(prefix, output);
                    prefix.pop();
                }
                Some(
                    SdtContent::Paragraph(_)
                    | SdtContent::Table(_)
                    | SdtContent::Row(_)
                    | SdtContent::Cell(_)
                    | SdtContent::RawXml(_),
                )
                | None => {}
            }
        }
    }

    pub(crate) fn accepted_run_segments(&self, path: &[AcceptedRunPathSegment]) -> Option<&CT_R> {
        let (first, rest) = path.split_first()?;
        match *first {
            AcceptedRunPathSegment::Run(index) if rest.is_empty() => {
                match self.content.get(index)? {
                    SdtContent::Run(run) => Some(run),
                    _ => None,
                }
            }
            AcceptedRunPathSegment::ContentControl(index) => match self.content.get(index)? {
                SdtContent::ContentControl(control) => control.accepted_run_segments(rest),
                _ => None,
            },
            AcceptedRunPathSegment::Revision(index) => {
                self.revisions.get(index)?.1.accepted_run_segments(rest)
            }
            AcceptedRunPathSegment::Run(_) => None,
        }
    }

    pub(crate) fn accepted_run_segments_mut(
        &mut self,
        path: &[AcceptedRunPathSegment],
    ) -> Option<&mut CT_R> {
        let (first, rest) = path.split_first()?;
        match *first {
            AcceptedRunPathSegment::Run(index) if rest.is_empty() => {
                match self.content.get_mut(index)? {
                    SdtContent::Run(run) => Some(run),
                    _ => None,
                }
            }
            AcceptedRunPathSegment::ContentControl(index) => match self.content.get_mut(index)? {
                SdtContent::ContentControl(control) => control.accepted_run_segments_mut(rest),
                _ => None,
            },
            AcceptedRunPathSegment::Revision(index) => self
                .revisions
                .get_mut(index)?
                .1
                .accepted_run_segments_mut(rest),
            AcceptedRunPathSegment::Run(_) => None,
        }
    }

    pub(crate) fn has_changed_field_owners(&self) -> bool {
        self.content.iter().any(|content| match content {
            SdtContent::Run(run) => run.content.iter().any(|content| {
                matches!(content,
                RunContent::Field(field) if field.source_has_pending_changes())
            }),
            SdtContent::ContentControl(control) => control.has_changed_field_owners(),
            SdtContent::Paragraph(paragraph) => paragraph.has_changed_field_owners(),
            _ => false,
        }) || self.revisions.iter().any(|(_, revision)| {
            revision
                .content_paragraph()
                .is_some_and(CT_P::has_changed_field_owners)
        })
    }

    pub(crate) fn normalize_field_owners_in(
        &mut self,
        work: &mut crate::text::WordOperationWork,
    ) -> Result<()> {
        for content in &mut self.content {
            match content {
                SdtContent::Run(run) => {
                    for content in &mut run.content {
                        if let RunContent::Field(field) = content {
                            field.normalize_cached_result_owner_in(work)?;
                        }
                    }
                }
                SdtContent::ContentControl(control) => control.normalize_field_owners_in(work)?,
                SdtContent::Paragraph(paragraph) => paragraph.normalize_field_owners_in(work)?,
                _ => {}
            }
        }
        for (_, revision) in &mut self.revisions {
            revision.normalize_pending_fields_in(work)?;
        }
        Ok(())
    }

    pub(crate) fn append_ordered_comment_ranges(
        &self,
        accepted: &mut usize,
        output: &mut Vec<crate::text::CommentRangeMarker>,
    ) {
        for boundary in 0..=self.content.len() {
            for (_, revision) in self.revisions.iter().filter(|(at, _)| *at == boundary) {
                if matches!(
                    revision.kind(),
                    crate::revision::RevisionKind::Insertion
                        | crate::revision::RevisionKind::MoveTo
                ) && let Some(paragraph) = revision.content_paragraph()
                {
                    paragraph.append_ordered_comment_ranges(accepted, output);
                }
            }
            match self.content.get(boundary) {
                Some(SdtContent::Run(_)) => *accepted += 1,
                Some(SdtContent::ContentControl(control)) => {
                    control.append_ordered_comment_ranges(accepted, output)
                }
                Some(SdtContent::RawXml(raw)) => {
                    if let Some(marker) = crate::text::raw_comment_range_marker(
                        raw,
                        &self.content_word_prefixes,
                        *accepted,
                    ) {
                        output.push(marker);
                    }
                }
                _ => {}
            }
        }
    }

    pub(crate) fn insert_ordered_result_comment_ranges(
        &mut self,
        path: &[AcceptedRunPathSegment],
        ranges: &[crate::text::CommentRangeMarker],
    ) -> Result<bool> {
        let Some((first, rest)) = path.split_first() else {
            return Ok(false);
        };
        match *first {
            AcceptedRunPathSegment::Run(base) if rest.is_empty() => {
                let mut insertions = Vec::new();
                for marker in ranges {
                    let (name, id, offset) = match marker {
                        crate::text::CommentRangeMarker::Start { id, run_index, .. } => {
                            ("commentRangeStart", *id, *run_index)
                        }
                        crate::text::CommentRangeMarker::End { id, run_index, .. } => {
                            ("commentRangeEnd", *id, *run_index)
                        }
                    };
                    let boundary = base.checked_add(offset).ok_or_else(|| {
                        OxmlError::InvalidValue("annotation control boundary overflow".into())
                    })?;
                    if boundary > self.content.len() {
                        return Ok(false);
                    }
                    let raw = format!("<w:{name} xmlns:w=\"{W_NS}\" w:id=\"{id}\"/>").into_bytes();
                    insertions.push((boundary, SdtContent::RawXml(raw)));
                }
                // Reverse insertion preserves requested order at equal boundaries.
                for (boundary, child) in insertions.into_iter().rev() {
                    if !self.insert_content(boundary, vec![child]) {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            AcceptedRunPathSegment::ContentControl(index) => match self.content.get_mut(index) {
                Some(SdtContent::ContentControl(control)) => {
                    control.insert_ordered_result_comment_ranges(rest, ranges)
                }
                _ => Ok(false),
            },
            AcceptedRunPathSegment::Revision(index) => match self
                .revisions
                .get_mut(index)
                .and_then(|(_, revision)| revision.ordered_paragraph_mut())
            {
                Some(paragraph) => paragraph.insert_ordered_result_comment_ranges(rest, ranges),
                None => Ok(false),
            },
            AcceptedRunPathSegment::Run(_) => Ok(false),
        }
    }

    pub(crate) fn replace_ordered_result_slot(
        &mut self,
        path: &[AcceptedRunPathSegment],
        replacement: Vec<CT_R>,
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<bool> {
        let own_context = crate::text::field_scope_bindings(&self.content_word_prefixes);
        let source_context = if source_context.is_some() {
            Some(own_context.as_slice())
        } else {
            None
        };
        let Some((first, rest)) = path.split_first() else {
            return Ok(false);
        };
        match *first {
            AcceptedRunPathSegment::Run(index) if rest.is_empty() => {
                if !matches!(self.content.get(index), Some(SdtContent::Run(_))) {
                    return Ok(false);
                }
                if replacement.len() == 1 {
                    return self.replace_accepted_run_segments(
                        path,
                        replacement.into_iter().next().expect("one replacement run"),
                    );
                }
                let count = replacement.len();
                self.remove_content(index);
                Ok(self.insert_content(
                    index,
                    replacement
                        .into_iter()
                        .map(SdtContent::Run)
                        .collect::<Vec<_>>(),
                ) && count <= self.content.len())
            }
            AcceptedRunPathSegment::ContentControl(index) => match self.content.get_mut(index) {
                Some(SdtContent::ContentControl(control)) => {
                    control.replace_ordered_result_slot(rest, replacement, source_context)
                }
                _ => Ok(false),
            },
            AcceptedRunPathSegment::Revision(index) => {
                let Some((_, revision)) = self.revisions.get_mut(index) else {
                    return Ok(false);
                };
                revision.replace_ordered_result_slot(rest, replacement)
            }
            AcceptedRunPathSegment::Run(_) => Ok(false),
        }
    }

    pub(crate) fn replace_accepted_run_segments(
        &mut self,
        path: &[AcceptedRunPathSegment],
        replacement: CT_R,
    ) -> Result<bool> {
        self.replace_accepted_run_segments_in(path, replacement, None, None, false)
    }

    pub(crate) fn replace_accepted_run_segments_in(
        &mut self,
        path: &[AcceptedRunPathSegment],
        replacement: CT_R,
        work: Option<&mut crate::text::WordOperationWork>,
        emission: Option<&crate::text::FieldXmlEmission>,
        prepared_fields: bool,
    ) -> Result<bool> {
        let Some((first, rest)) = path.split_first() else {
            return Ok(false);
        };
        match *first {
            AcceptedRunPathSegment::Run(index) if rest.is_empty() => {
                let Some(SdtContent::Run(run)) = self.content.get_mut(index) else {
                    return Ok(false);
                };
                *run = replacement;
                Ok(true)
            }
            AcceptedRunPathSegment::ContentControl(index) => {
                let Some(SdtContent::ContentControl(control)) = self.content.get_mut(index) else {
                    return Ok(false);
                };
                control.replace_accepted_run_segments_in(
                    rest,
                    replacement,
                    work,
                    emission,
                    prepared_fields,
                )
            }
            AcceptedRunPathSegment::Revision(index) => {
                let Some((_, revision)) = self.revisions.get_mut(index) else {
                    return Ok(false);
                };
                revision.replace_accepted_run_segments_in(
                    rest,
                    replacement,
                    work,
                    emission,
                    prepared_fields,
                )
            }
            AcceptedRunPathSegment::Run(_) => Ok(false),
        }
    }

    pub(crate) fn split_accepted_run_segments_in(
        &mut self,
        path: &[AcceptedRunPathSegment],
        offset: usize,
        work: Option<&mut crate::text::WordOperationWork>,
        emission: Option<&crate::text::FieldXmlEmission>,
        edge: crate::text::SplitEdge,
    ) -> Result<bool> {
        let Some((first, rest)) = path.split_first() else {
            return Ok(false);
        };
        match *first {
            AcceptedRunPathSegment::Run(index) if rest.is_empty() => {
                let Some(SdtContent::Run(run)) = self.content.get(index) else {
                    return Ok(false);
                };
                let mut paragraph = CT_P::new();
                paragraph.runs.push(run.clone());
                paragraph
                    .split_run_with(0, offset, edge)
                    .map_err(|error| OxmlError::InvalidValue(error.to_string()))?;
                let replacement = paragraph.runs.drain(..).map(SdtContent::Run);
                self.content.splice(index..=index, replacement);
                for (boundary, _) in &mut self.revisions {
                    if *boundary > index {
                        *boundary += 1;
                    }
                }
                for source in &mut self.inline_run_sources {
                    if source.content_index > index {
                        source.content_index += 1;
                    }
                }
                Ok(true)
            }
            AcceptedRunPathSegment::ContentControl(index) => {
                let Some(SdtContent::ContentControl(control)) = self.content.get_mut(index) else {
                    return Ok(false);
                };
                control.split_accepted_run_segments_in(rest, offset, work, emission, edge)
            }
            AcceptedRunPathSegment::Revision(index) => {
                let Some((_, revision)) = self.revisions.get_mut(index) else {
                    return Ok(false);
                };
                revision.split_accepted_run_segments_in(rest, offset, work, emission, edge)
            }
            AcceptedRunPathSegment::Run(_) => Ok(false),
        }
    }

    /// Remove the accepted-view run at `path`, and a tracked insertion left
    /// with nothing in it. An emptied nested control stays. Returns false for
    /// a stale path.
    pub(crate) fn remove_accepted_run_segments(
        &mut self,
        path: &[AcceptedRunPathSegment],
    ) -> Result<bool> {
        let Some((first, rest)) = path.split_first() else {
            return Ok(false);
        };
        match *first {
            AcceptedRunPathSegment::Run(index) if rest.is_empty() => {
                if !matches!(self.content.get(index), Some(SdtContent::Run(_))) {
                    return Ok(false);
                }
                self.remove_content(index);
                Ok(true)
            }
            AcceptedRunPathSegment::ContentControl(index) => {
                let Some(SdtContent::ContentControl(control)) = self.content.get_mut(index) else {
                    return Ok(false);
                };
                control.remove_accepted_run_segments(rest)
            }
            AcceptedRunPathSegment::Revision(index) => {
                let Some((_, revision)) = self.revisions.get_mut(index) else {
                    return Ok(false);
                };
                match revision.remove_accepted_run_segments(rest)? {
                    None => Ok(false),
                    Some(emptied) => {
                        if emptied {
                            // The revision is written in place of the raw
                            // child at its boundary, which goes with it.
                            let (boundary, _) = self.revisions.remove(index);
                            self.remove_content(boundary);
                        }
                        Ok(true)
                    }
                }
            }
            AcceptedRunPathSegment::Run(_) => Ok(false),
        }
    }

    /// Insert direct `w:sdtContent` children before `index`, keeping the
    /// revision and retained run source positions after it aligned.
    pub(crate) fn insert_content(&mut self, index: usize, children: Vec<SdtContent>) -> bool {
        if index > self.content.len() {
            return false;
        }
        let count = children.len();
        for (boundary, _) in &mut self.revisions {
            if *boundary >= index {
                *boundary += count;
            }
        }
        for source in &mut self.inline_run_sources {
            if source.content_index >= index {
                source.content_index += count;
            }
        }
        self.content.splice(index..index, children);
        true
    }

    /// Remove the selected comment range markers and the reference runs
    /// that are direct `w:sdtContent` children.
    ///
    /// A run loses only the selected references and is removed when nothing
    /// else remains in it. Nested controls and block content are left to the
    /// caller.
    #[doc(hidden)]
    pub fn remove_comment_anchors(&mut self, ids: &[i32]) {
        // Markers added in this session carry the fixed `w` prefix whatever
        // prefix the source document gave the Word namespace.
        let mut marker_prefixes = vec!["w".to_owned()];
        marker_prefixes.extend(self.content_word_prefixes.iter().cloned());
        let removed = self
            .content
            .iter_mut()
            .map(|child| match child {
                SdtContent::Run(run) => {
                    run.remove_comment_references(ids)
                        && run.content.is_empty()
                        && run.extra_xml.is_empty()
                        && run.alt_drawings.is_empty()
                }
                SdtContent::RawXml(raw) => {
                    crate::text::raw_comment_marker_id(raw, &marker_prefixes)
                        .is_some_and(|id| ids.contains(&id))
                }
                _ => false,
            })
            .collect::<Vec<_>>();
        if !removed.contains(&true) {
            return;
        }
        let kept_before = |index: usize| removed[..index].iter().filter(|remove| !**remove).count();
        self.revisions
            .retain(|(boundary, _)| !removed.get(*boundary).copied().unwrap_or(false));
        for (boundary, _) in &mut self.revisions {
            *boundary = kept_before(*boundary);
        }
        self.inline_run_sources
            .retain(|source| !removed.get(source.content_index).copied().unwrap_or(false));
        for source in &mut self.inline_run_sources {
            source.content_index = kept_before(source.content_index);
        }
        self.content = std::mem::take(&mut self.content)
            .into_iter()
            .zip(removed)
            .filter_map(|(child, remove)| (!remove).then_some(child))
            .collect();
    }

    /// Remap the facade-authored bookmark markers among the direct and nested
    /// inline `w:sdtContent` children.
    pub(crate) fn remap_authored_bookmark_ids(
        &mut self,
        remap: &std::collections::HashMap<i32, i32>,
    ) {
        for content in &mut self.content {
            match content {
                SdtContent::RawXml(raw) => crate::text::remap_authored_bookmark_marker(raw, remap),
                SdtContent::ContentControl(control) => control.remap_authored_bookmark_ids(remap),
                _ => {}
            }
        }
    }

    /// Remove one content child, keeping the revisions and the source bytes
    /// of the later runs at their content index.
    pub(crate) fn remove_content(&mut self, index: usize) {
        if index >= self.content.len() {
            return;
        }
        self.content.remove(index);
        for (boundary, _) in &mut self.revisions {
            if *boundary > index {
                *boundary -= 1;
            }
        }
        self.inline_run_sources
            .retain(|source| source.content_index != index);
        for source in &mut self.inline_run_sources {
            if source.content_index > index {
                source.content_index -= 1;
            }
        }
    }

    pub(crate) fn word_prefixes(&self) -> &[String] {
        &self.word_prefixes
    }

    /// Parse a content control at the reader's current `w:sdt` start.
    pub fn from_xml(reader: &mut Reader<&[u8]>, start: &BytesStart<'_>) -> Result<Self> {
        let prefixes = word_prefixes_at(start, &["w".to_owned()])?.into_owned();
        crate::document::admit_word_reader(
            reader,
            &prefixes,
            b"sdt",
            Some(crate::namespace::W_NS),
        )?;
        Self::from_xml_with_prefixes(reader, start, &["w".to_owned()], SdtOwner::Standalone, None)
    }

    /// Whether a preserved control is admitted by the complete typed parser.
    #[doc(hidden)]
    pub fn story_raw_is_typed(raw: &[u8], inherited: &[String], owner: StorySdtOwner) -> bool {
        let owner = owner.into();
        let Ok(admitted) = crate::document::admit_word_source(raw, inherited, None, Some(owner))
        else {
            return false;
        };
        Self::from_raw_with_context(raw, inherited, owner, Some(&mut admitted.controls())).is_some()
    }

    pub(crate) fn from_raw_with_context(
        raw: &[u8],
        inherited: &[String],
        owner: SdtOwner,
        parent_facts: Option<&mut crate::document::ControlSourceCursor<'_>>,
    ) -> Option<Self> {
        Self::from_raw_with_mapped_context(raw, inherited, owner, parent_facts, None)
    }

    pub(crate) fn from_raw_with_mapped_context(
        raw: &[u8],
        inherited: &[String],
        owner: SdtOwner,
        parent_facts: Option<&mut crate::document::ControlSourceCursor<'_>>,
        captured_map: Option<std::sync::Arc<crate::document::WordSourceMap>>,
    ) -> Option<Self> {
        Self::from_raw_with_mapped_context_and_maps(
            raw,
            inherited,
            owner,
            parent_facts,
            captured_map,
            None,
        )
    }

    pub(crate) fn from_raw_with_mapped_context_and_maps(
        raw: &[u8],
        inherited: &[String],
        owner: SdtOwner,
        parent_facts: Option<&mut crate::document::ControlSourceCursor<'_>>,
        captured_map: Option<std::sync::Arc<crate::document::WordSourceMap>>,
        mut source_maps: Option<&mut crate::text::WordBlockSourceMaps>,
    ) -> Option<Self> {
        let resource_registry = parent_facts
            .as_deref()
            .and_then(crate::document::ControlSourceCursor::field_registry);
        let mut child_facts = match parent_facts {
            Some(parent) if parent.projected_aliases() => match parent.take(owner) {
                Some((true, _)) => return None,
                Some((false, children)) => Some(children),
                None => Some(parent.empty()),
            },
            parent => {
                if matches!(
                    crate::document::control_has_property_measurement_failure_in(
                        raw,
                        inherited,
                        owner,
                        resource_registry.as_ref()
                    ),
                    Ok(true)
                ) {
                    return None;
                }
                parent
                    .filter(|parent| parent.has_field_origins())
                    .map(|parent| parent.empty())
            }
        };

        if let Some(map) = captured_map {
            child_facts = child_facts.map(|facts| facts.bind_capture(raw, map));
        }

        let mut reader = Reader::from_reader(raw);
        reader.config_mut().trim_text(false);
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Start(start)) if matches_local_name(start.name().as_ref(), b"sdt") => {
                    let mut pending =
                        source_maps
                            .as_ref()
                            .map(|maps| crate::text::WordBlockSourceMaps {
                                collect_word_texts: maps.collect_word_texts,
                                collect_properties: maps.collect_properties,
                                ..Default::default()
                            });
                    let parsed = Self::from_xml_with_prefixes_and_maps(
                        &mut reader,
                        &start,
                        inherited,
                        owner,
                        child_facts.as_mut(),
                        pending.as_mut(),
                    )
                    .ok()?;
                    if let (Some(output), Some(pending)) = (source_maps.as_deref_mut(), pending) {
                        output.paragraphs.extend(pending.paragraphs);
                        output.word_texts.extend(pending.word_texts);
                        output.word_runs.extend(pending.word_runs);
                        output.word_raws.extend(pending.word_raws);
                        output.ordinary.extend(pending.ordinary);
                    }
                    return Some(parsed);
                }
                Ok(Event::Eof) | Err(_) => return None,
                Ok(_) => {}
            }
            buffer.clear();
        }
    }

    fn from_xml_with_prefixes(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        inherited: &[String],
        owner: SdtOwner,
        control_facts: Option<&mut crate::document::ControlSourceCursor<'_>>,
    ) -> Result<Self> {
        Self::from_xml_with_prefixes_and_maps(reader, start, inherited, owner, control_facts, None)
    }

    fn from_xml_with_prefixes_and_maps(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        inherited: &[String],
        owner: SdtOwner,
        mut control_facts: Option<&mut crate::document::ControlSourceCursor<'_>>,
        mut source_maps: Option<&mut crate::text::WordBlockSourceMaps>,
    ) -> Result<Self> {
        let prefixes = word_prefixes_at(start, inherited)?;
        let mut sdt = Self {
            properties: None,
            content: Vec::new(),
            revisions: Vec::new(),
            extra_attributes: capture_attributes(start, &prefixes, &[])?,
            content_attributes: Vec::new(),
            slots: Vec::new(),
            word_prefixes: prefixes.to_vec(),
            content_word_prefixes: prefixes.to_vec(),
            inline_run_sources: Vec::new(),
            pending_inline_field_edits: Vec::new(),
        };
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Start(child)) => {
                    let child_prefixes = word_prefixes_at(&child, &prefixes)?;
                    if is_word_element(child.name().as_ref(), b"sdtPr", &child_prefixes)
                        && sdt.properties.is_none()
                    {
                        sdt.properties = Some(CT_SdtPr::from_xml_with_prefixes(
                            reader,
                            &child,
                            &child_prefixes,
                        )?);
                        sdt.slots.push(RootSlot::Properties);
                    } else if is_word_element(child.name().as_ref(), b"sdtContent", &child_prefixes)
                        && !sdt
                            .slots
                            .iter()
                            .any(|slot| matches!(slot, RootSlot::Content))
                    {
                        sdt.content_attributes = capture_attributes(&child, &child_prefixes, &[])?;
                        sdt.content_word_prefixes = child_prefixes.to_vec();
                        sdt.content = parse_content(
                            reader,
                            &child_prefixes,
                            &mut sdt.revisions,
                            &mut sdt.inline_run_sources,
                            owner,
                            control_facts.as_deref_mut(),
                            source_maps.as_deref_mut(),
                        )?;
                        sdt.slots.push(RootSlot::Content);
                    } else {
                        sdt.slots
                            .push(RootSlot::Raw(capture_element(reader, &child)?));
                    }
                }
                Ok(Event::Empty(child)) => {
                    let child_prefixes = word_prefixes_at(&child, &prefixes)?;
                    if is_word_element(child.name().as_ref(), b"sdtContent", &child_prefixes)
                        && !sdt
                            .slots
                            .iter()
                            .any(|slot| matches!(slot, RootSlot::Content))
                    {
                        // Content holding nothing, in either writing.
                        sdt.content_attributes = capture_attributes(&child, &child_prefixes, &[])?;
                        sdt.content_word_prefixes = child_prefixes.to_vec();
                        sdt.slots.push(RootSlot::Content);
                    } else {
                        sdt.slots.push(RootSlot::Raw(capture_empty(
                            &child,
                            reader.empty_writing(),
                        )?));
                    }
                }
                Ok(Event::End(end)) if matches_local_name(end.name().as_ref(), b"sdt") => break,
                Ok(Event::Eof) => {
                    return Err(OxmlError::MissingElement("w:sdt end".to_owned()));
                }
                Ok(event) => push_root_event_raw(&mut sdt.slots, event)?,
                Err(error) => return Err(error.into()),
            }
            buffer.clear();
        }
        Ok(sdt)
    }

    /// Whether the control has a child element or visible direct text.
    pub fn has_child_content(&self) -> bool {
        self.properties.is_some()
            || !self.content.is_empty()
            || self.slots.iter().any(|slot| match slot {
                RootSlot::Properties | RootSlot::Content => true,
                RootSlot::Raw(raw) => raw_fragment_has_child_content(raw),
            })
    }

    /// Transfer model owners only when this checked paragraph is being discarded.
    pub(crate) fn into_discard_parts(self) -> (Vec<SdtContent>, Vec<CT_Revision>, Vec<CT_R>) {
        (
            self.content,
            self.revisions
                .into_iter()
                .map(|(_, revision)| revision)
                .collect(),
            self.inline_run_sources
                .into_iter()
                .map(|source| source.original)
                .collect(),
        )
    }

    /// Stored run originals are cloned even after the current run is edited.
    #[doc(hidden)]
    pub fn original_resource_runs(&self) -> impl Iterator<Item = &CT_R> {
        self.inline_run_sources
            .iter()
            .map(|source| &source.original)
    }

    pub(crate) fn to_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.write_xml_role(writer, XmlWriteRole::Publication, None)
    }

    fn publication_budget(&self, work: &mut crate::text::WordOperationWork) -> Result<usize> {
        let mut raw_bytes = self
            .slots
            .iter()
            .try_fold(0usize, |total, slot| {
                total.checked_add(if let RootSlot::Raw(raw) = slot {
                    raw.len()
                } else {
                    0
                })
            })
            .ok_or_else(|| OxmlError::InvalidValue("control root source overflow".into()))?;
        for (index, child) in self.content.iter().enumerate() {
            let bytes = match child {
                SdtContent::RawXml(raw) => raw.len(),
                SdtContent::Run(run) => self
                    .inline_run_source(index, run, work)?
                    .map_or(0, |source| source.raw_xml.len()),
                _ => 0,
            };
            raw_bytes = raw_bytes
                .checked_add(bytes)
                .ok_or_else(|| OxmlError::InvalidValue("control content source overflow".into()))?;
        }
        let current = attribute_bytes(&self.extra_attributes)?
            .checked_add(attribute_bytes(&self.content_attributes)?)
            .ok_or_else(|| OxmlError::InvalidValue("control current source overflow".into()))?;
        crate::text::retained_owner_work(raw_bytes, &self.word_prefixes, current)?
            .checked_add(crate::text::retained_owner_work(
                0,
                &self.content_word_prefixes,
                0,
            )?)
            .ok_or_else(|| OxmlError::InvalidValue("control scope work overflow".into()))
    }

    fn preflight_publication(&self, work: &mut crate::text::WordOperationWork) -> Result<()> {
        let root_raw = self
            .slots
            .iter()
            .filter_map(|slot| {
                if let RootSlot::Raw(raw) = slot {
                    Some(raw.as_slice())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let mut content_raw = Vec::new();
        for (index, child) in self.content.iter().enumerate() {
            match child {
                SdtContent::Run(run) => {
                    if let Some(source) = self.inline_run_source(index, run, work)? {
                        content_raw.push(source.raw_xml.as_slice());
                    }
                }
                SdtContent::RawXml(raw) if !self.revisions.iter().any(|(at, _)| *at == index) => {
                    content_raw.push(raw.as_slice())
                }
                _ => {}
            }
        }
        let raw_bytes = checked_lengths(
            &root_raw
                .iter()
                .chain(&content_raw)
                .map(|raw| raw.len())
                .collect::<Vec<_>>(),
        )?;
        let current = attribute_bytes(&self.extra_attributes)?
            .checked_add(attribute_bytes(&self.content_attributes)?)
            .ok_or_else(|| OxmlError::InvalidValue("control attribute work overflow".into()))?;
        let budget = crate::text::retained_owner_work(raw_bytes, &self.word_prefixes, current)?
            .checked_add(crate::text::retained_owner_work(
                0,
                &self.content_word_prefixes,
                0,
            )?)
            .ok_or_else(|| OxmlError::InvalidValue("control scope work overflow".into()))?;
        let (start, _) = crate::text::generated_word_start(
            {
                let mut root = BytesStart::new("w:sdt");
                for (name, value) in &self.extra_attributes {
                    root.push_attribute((name.as_str(), value.as_str()));
                }
                root
            },
            &self.word_prefixes,
            XmlWriteRole::Publication,
            None,
        )?;
        let _root_scope = crate::text::emitted_binding_scope(&start, false, false)?;
        let mut projected = preflight_owned_raw(&root_raw, &self.word_prefixes, budget, raw_bytes)?;
        if let Some(properties) = &self.properties {
            properties.preflight_publication()?;
        }
        let (content, _) = crate::text::generated_word_start(
            {
                let mut root = BytesStart::new("w:sdtContent");
                for (name, value) in &self.content_attributes {
                    root.push_attribute((name.as_str(), value.as_str()));
                }
                root
            },
            &self.content_word_prefixes,
            XmlWriteRole::Publication,
            None,
        )?;
        let _content_scope = crate::text::emitted_binding_scope(&content, false, false)?;
        projected =
            preflight_owned_raw(&content_raw, &self.content_word_prefixes, budget, projected)?;
        let _ = projected;
        for child in &self.content {
            if let SdtContent::ContentControl(control) = child {
                control.preflight_publication(work)?;
            }
        }
        Ok(())
    }

    pub(crate) fn writer_field_roots(&self) -> Vec<&Field> {
        let mut fields = Vec::new();
        for child in &self.content {
            match child {
                SdtContent::Run(run) => fields.extend(run.content.iter().filter_map(|content| {
                    if let RunContent::Field(field) = content {
                        Some(field)
                    } else {
                        None
                    }
                })),
                SdtContent::Paragraph(paragraph) => fields.extend(paragraph.writer_field_roots()),
                SdtContent::ContentControl(control) => fields.extend(control.writer_field_roots()),
                SdtContent::Table(table) => fields.extend(table.writer_field_roots()),
                SdtContent::Row(row) => fields.extend(row.writer_field_roots()),
                SdtContent::Cell(cell) => fields.extend(cell.writer_field_roots()),
                SdtContent::RawXml(_) => {}
            }
        }
        fields
    }

    pub(crate) fn write_xml_role<W: Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<()> {
        let mut work = crate::text::WordOperationWork::model_fields(self.writer_field_roots())?;
        self.write_xml_role_in(writer, role, source_context, &mut work, None)
    }

    pub(crate) fn write_xml_role_in<W: Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        work: &mut crate::text::WordOperationWork,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let _source_scope =
            (role == XmlWriteRole::Source).then(|| crate::text::root_binding_scope(0));
        crate::document::admit_word_model(crate::document::WordOwner::Control(self))?;
        let budget = if role == XmlWriteRole::Publication {
            self.publication_budget(work)?
        } else {
            0
        };
        if role == XmlWriteRole::Publication {
            self.preflight_publication(work)?;
        }
        let mut start = crate::text::FieldXmlStart::new("w:sdt", emission)?;
        for (name, value) in &self.extra_attributes {
            start.push(name, value)?;
        }
        let word_prefix = start.qualify(&self.word_prefixes, role, source_context)?;
        let _root_scope = crate::text::emitted_binding_scope(start.element(), false, false)?;
        start.write(writer, false)?;
        let mut properties_written = false;
        let mut content_written = false;
        for slot in &self.slots {
            match slot {
                RootSlot::Properties if !properties_written => {
                    if let Some(properties) = &self.properties {
                        properties.write_xml_role(writer, role, source_context, emission)?;
                    }
                    properties_written = true;
                }
                RootSlot::Content if !content_written => {
                    self.write_content(writer, role, budget, work, emission)?;
                    content_written = true;
                }
                RootSlot::Raw(raw) => {
                    write_owned_raw_in(writer, raw, &self.word_prefixes, role, budget, emission)?
                }
                _ => {}
            }
        }
        if !properties_written && let Some(properties) = &self.properties {
            properties.write_xml_role(writer, role, source_context, emission)?;
        }
        if !content_written
            && (!self.content.is_empty()
                || self
                    .slots
                    .iter()
                    .any(|slot| matches!(slot, RootSlot::Content)))
        {
            self.write_content(writer, role, budget, work, emission)?;
        }
        if let Some(emission) = emission {
            emission.write_word_end(writer, &word_prefix, "sdt")?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(format!("{word_prefix}:sdt"))))?;
        }
        Ok(())
    }

    fn write_content<W: Write>(
        &self,
        writer: &mut Writer<W>,
        role: XmlWriteRole,
        budget: usize,
        work: &mut crate::text::WordOperationWork,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let mut start = crate::text::FieldXmlStart::new("w:sdtContent", emission)?;
        for (name, value) in &self.content_attributes {
            start.push(name, value)?;
        }
        let word_prefix = start.qualify(&self.content_word_prefixes, role, None)?;
        let _content_scope = crate::text::emitted_binding_scope(start.element(), false, false)?;
        start.write(writer, false)?;
        let original = crate::text::field_scope_bindings(&self.content_word_prefixes);
        let source_context = (role == XmlWriteRole::Source).then_some(original.as_slice());
        for (content_index, child) in self.content.iter().enumerate() {
            match child {
                SdtContent::Paragraph(paragraph) => paragraph.write_xml_core_in(
                    writer,
                    None,
                    role,
                    source_context,
                    work,
                    emission,
                )?,
                SdtContent::Table(table) => {
                    table.write_xml_role_in(writer, role, source_context, work, emission)?
                }
                SdtContent::Row(row) => {
                    row.write_xml_role_in(writer, role, source_context, work, emission)?
                }
                SdtContent::Cell(cell) => {
                    cell.write_xml_role_in(writer, role, source_context, work, emission)?
                }
                SdtContent::Run(run) => {
                    self.write_inline_child(
                        writer,
                        content_index,
                        run,
                        role,
                        budget,
                        work,
                        emission,
                    )?;
                }
                SdtContent::ContentControl(sdt) => {
                    sdt.write_xml_role_in(writer, role, source_context, work, emission)?
                }
                SdtContent::RawXml(raw) => {
                    if let Some((_, revision)) = self
                        .revisions
                        .iter()
                        .find(|(boundary, _)| *boundary == content_index)
                    {
                        revision.write_xml_role_in(writer, role, source_context, work, emission)?;
                    } else {
                        write_owned_raw_in(
                            writer,
                            raw,
                            &self.content_word_prefixes,
                            role,
                            budget,
                            emission,
                        )?;
                    }
                }
            }
        }
        writer.write_event(Event::End(BytesEnd::new(format!(
            "{word_prefix}:sdtContent"
        ))))?;
        Ok(())
    }

    /// Every content control nested in this one, at any depth and whatever
    /// it holds (blocks, rows, cells or runs), in document order.
    pub fn nested_content_controls(&self) -> Vec<&CT_Sdt> {
        let mut controls = Vec::new();
        self.collect_controls(SdtOwner::Standalone, &mut controls);
        controls
    }

    pub(crate) fn collect_controls<'a>(&'a self, owner: SdtOwner, controls: &mut Vec<&'a CT_Sdt>) {
        for child in self
            .content
            .iter()
            .filter(|child| owner.owns_content(child))
        {
            match child {
                SdtContent::Paragraph(paragraph) => paragraph.collect_controls(controls),
                SdtContent::Table(table) => table.collect_controls(controls),
                SdtContent::Row(row) => row.collect_controls(controls),
                SdtContent::Cell(cell) => cell.collect_controls(controls),
                SdtContent::Run(_) | SdtContent::RawXml(_) => {}
                SdtContent::ContentControl(sdt) => {
                    controls.push(sdt);
                    sdt.collect_controls(owner, controls);
                }
            }
        }
    }

    pub(crate) fn collect_paragraphs<'a>(
        &'a self,
        owner: SdtOwner,
        paragraphs: &mut Vec<&'a CT_P>,
    ) {
        for child in self
            .content
            .iter()
            .filter(|child| owner.owns_content(child))
        {
            match child {
                SdtContent::Paragraph(paragraph) => paragraphs.push(paragraph),
                // A table written in a block control is a table: its cells
                // are no paragraphs of the story around it, as a direct
                // table's are not.
                SdtContent::Table(_) => {}
                SdtContent::Row(row) => row.collect_paragraphs(paragraphs),
                SdtContent::Cell(cell) => cell.collect_paragraphs(paragraphs),
                SdtContent::ContentControl(sdt) => sdt.collect_paragraphs(owner, paragraphs),
                _ => {}
            }
        }
    }

    pub(crate) fn collect_tables<'a>(&'a self, owner: SdtOwner, tables: &mut Vec<&'a CT_Tbl>) {
        for child in self
            .content
            .iter()
            .filter(|child| owner.owns_content(child))
        {
            match child {
                SdtContent::Table(table) => tables.push(table),
                SdtContent::Cell(cell) => cell.collect_tables(tables),
                SdtContent::ContentControl(sdt) => sdt.collect_tables(owner, tables),
                _ => {}
            }
        }
    }

    pub(crate) fn collect_rows<'a>(&'a self, owner: SdtOwner, rows: &mut Vec<&'a CT_Row>) {
        for child in self
            .content
            .iter()
            .filter(|child| owner.owns_content(child))
        {
            match child {
                SdtContent::Row(row) => rows.push(row),
                SdtContent::Table(table) => table.collect_rows(rows),
                SdtContent::ContentControl(sdt) => sdt.collect_rows(owner, rows),
                _ => {}
            }
        }
    }

    pub(crate) fn collect_cells<'a>(&'a self, owner: SdtOwner, cells: &mut Vec<&'a CT_Tc>) {
        for child in self
            .content
            .iter()
            .filter(|child| owner.owns_content(child))
        {
            match child {
                SdtContent::Cell(cell) => cells.push(cell),
                SdtContent::Row(row) => row.collect_cells(cells),
                SdtContent::ContentControl(sdt) => sdt.collect_cells(owner, cells),
                _ => {}
            }
        }
    }

    pub(crate) fn collect_runs<'a>(&'a self, runs: &mut Vec<&'a CT_R>) {
        for child in self
            .content
            .iter()
            .filter(|child| SdtOwner::Inline.owns_content(child))
        {
            match child {
                SdtContent::Run(run) => runs.push(run),
                SdtContent::Paragraph(paragraph) => paragraph.collect_runs(runs),
                SdtContent::ContentControl(sdt) => sdt.collect_runs(runs),
                _ => {}
            }
        }
    }
}

/// The typed parent grammar used to admit a story content control.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorySdtOwner {
    Block,
    Table,
    Row,
    Inline,
}

impl From<StorySdtOwner> for SdtOwner {
    fn from(owner: StorySdtOwner) -> Self {
        match owner {
            StorySdtOwner::Block => Self::Body,
            StorySdtOwner::Table => Self::Table,
            StorySdtOwner::Row => Self::Row,
            StorySdtOwner::Inline => Self::Inline,
        }
    }
}

fn parse_inline_run(
    xml: &[u8],
    inherited: &[String],
    input: Option<&crate::document::WordFieldInput>,
    text_facts: Option<&mut Vec<crate::text::ParsedWordText>>,
) -> Result<CT_R> {
    parse_inline_run_with_property(xml, inherited, input, text_facts, None)
}

fn parse_inline_run_with_property(
    xml: &[u8],
    inherited: &[String],
    input: Option<&crate::document::WordFieldInput>,
    text_facts: Option<&mut Vec<crate::text::ParsedWordText>>,
    mut ordinary: Option<&mut Option<crate::document::WordOrdinaryFact>>,
) -> Result<CT_R> {
    if let Some(output) = &mut ordinary {
        **output = None;
    }
    let cursor = input.map(|input| input.cursor(xml)).transpose()?;
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buffer)?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(start) | Event::Empty(start) => {
                let prefixes = word_prefixes_at(&start, inherited)?;
                if is_word_element(start.name().as_ref(), b"fldSimple", &prefixes) {
                    return crate::text::parse_simple_field(xml, &prefixes, None)?
                        .map(|field| crate::text::field_run(field, None))
                        .ok_or_else(|| {
                            OxmlError::InvalidValue("unsupported inline simple field".into())
                        });
                }
                if is_word_element(start.name().as_ref(), b"r", &prefixes) {
                    return if empty {
                        CT_R::from_empty_root(&start, &prefixes)
                    } else {
                        CT_R::from_xml_with_property_observation(
                            &mut reader,
                            &prefixes,
                            Some(&start),
                            text_facts,
                            cursor.as_ref(),
                            ordinary,
                        )
                    };
                }
                return Err(OxmlError::InvalidValue(
                    "inline control child is not a Word run or simple field".into(),
                ));
            }
            Event::Eof => return Err(OxmlError::MissingElement("inline control run".to_owned())),
            _ => {}
        }
        buffer.clear();
    }
}

fn append_inline_legacy_fields(field: &Field, output: &mut Vec<Field>) -> Result<()> {
    field.validate_legacy_form_owner()?;
    if field.legacy_form.is_some() {
        output.push(field.clone());
    }
    for nested in field.nested_fields_in_source_order() {
        append_inline_legacy_fields(nested, output)?;
    }
    Ok(())
}

fn set_nth_inline_legacy_form(
    field: &mut Field,
    remaining: &mut usize,
    value: &LegacyFormFieldValue,
    work: &mut crate::text::WordOperationWork,
) -> Result<bool> {
    field.set_nth_legacy_form_value_with_work(remaining, value, Some(work))
}

// Keep by-value leaf model construction out of each recursive control frame.
// This boundary addresses the measured ordinary-stack table/control overflow.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn append_modeled_content_child(
    reader: &mut Reader<&[u8]>,
    child: &BytesStart<'_>,
    inherited: &[String],
    empty: bool,
    event_start: u64,
    content: &mut Vec<SdtContent>,
    inline_run_sources: &mut Vec<InlineRunSource>,
    mut control_facts: Option<&mut crate::document::ControlSourceCursor<'_>>,
    mut source_maps: Option<&mut crate::text::WordBlockSourceMaps>,
) -> Result<()> {
    let prefixes = word_prefixes_at(child, inherited)?;
    match (child.local_name().as_ref(), empty) {
        (b"p", false) => {
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
                Some(child),
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
                let mut parsed = nested;
                parsed.paragraph(maps);
                parsed.ordinary = ordinary;
                if let Some(facts) = word_texts {
                    parsed.word_texts.push((Vec::new(), facts));
                }
                output.append(
                    crate::text::WordBlockComponent::SdtContent(content.len()),
                    parsed,
                );
            }
            content.push(SdtContent::Paragraph(paragraph));
        }
        (b"p", true) => {
            let paragraph = CT_P::from_empty_root(child, &prefixes)?;
            if let Some(output) = source_maps.as_deref_mut() {
                let mut parsed = crate::text::WordBlockSourceMaps::default();
                parsed.paragraph(vec![None; paragraph.extra_xml.len()]);
                if output.collect_word_texts {
                    parsed.word_texts.push((Vec::new(), None));
                }
                output.append(
                    crate::text::WordBlockComponent::SdtContent(content.len()),
                    parsed,
                );
            }
            content.push(SdtContent::Paragraph(paragraph));
        }
        (b"tbl", false) => {
            let bindings = local_namespace_overrides(child, inherited)?;
            let mut child_maps =
                source_maps
                    .as_ref()
                    .map(|maps| crate::text::WordBlockSourceMaps {
                        collect_word_texts: maps.collect_word_texts,
                        collect_properties: maps.collect_properties,
                        ..Default::default()
                    });
            let mut table = CT_Tbl::from_xml_with_prefixes_and_owner_bindings_at_depth_and_maps(
                reader,
                &prefixes,
                &bindings,
                0,
                Some(child),
                control_facts,
                child_maps.as_mut(),
            )?;
            table.retain_root_attributes(child, &prefixes)?;
            if let (Some(output), Some(parsed)) = (source_maps.as_deref_mut(), child_maps) {
                output.append(
                    crate::text::WordBlockComponent::SdtContent(content.len()),
                    parsed,
                );
            }
            content.push(SdtContent::Table(table));
        }
        (b"tbl", true) => {
            let mut table = CT_Tbl::new();
            table.retain_root_attributes(child, &prefixes)?;
            content.push(SdtContent::Table(table));
        }
        (b"tr", false) => {
            let bindings = local_namespace_overrides(child, inherited)?;
            let mut child_maps =
                source_maps
                    .as_ref()
                    .map(|maps| crate::text::WordBlockSourceMaps {
                        collect_word_texts: maps.collect_word_texts,
                        collect_properties: maps.collect_properties,
                        ..Default::default()
                    });
            let row = CT_Row::from_xml_with_prefixes_and_owner_bindings_at_depth_and_maps(
                reader,
                &prefixes,
                &bindings,
                0,
                Some(child),
                control_facts,
                child_maps.as_mut(),
            )?;
            if let (Some(output), Some(parsed)) = (source_maps.as_deref_mut(), child_maps) {
                output.append(
                    crate::text::WordBlockComponent::SdtContent(content.len()),
                    parsed,
                );
            }
            content.push(SdtContent::Row(row));
        }
        (b"tr", true) => content.push(SdtContent::Row(CT_Row::from_empty_root(child, &prefixes)?)),
        (b"tc", false) => {
            let bindings = local_namespace_overrides(child, inherited)?;
            let mut child_maps =
                source_maps
                    .as_ref()
                    .map(|maps| crate::text::WordBlockSourceMaps {
                        collect_word_texts: maps.collect_word_texts,
                        collect_properties: maps.collect_properties,
                        ..Default::default()
                    });
            let mut cell = CT_Tc::from_xml_with_prefixes_and_owner_bindings_at_depth_and_maps(
                reader,
                &prefixes,
                &bindings,
                0,
                Some(child),
                control_facts,
                child_maps.as_mut(),
            )?;
            cell.retain_root_attributes(child, &prefixes)?;
            if let (Some(output), Some(parsed)) = (source_maps, child_maps) {
                output.append(
                    crate::text::WordBlockComponent::SdtContent(content.len()),
                    parsed,
                );
            }
            content.push(SdtContent::Cell(cell));
        }
        (b"tc", true) => {
            let mut cell = CT_Tc {
                properties: None,
                content: Vec::new(),
                extra_xml: Vec::new(),
            };
            cell.retain_root_attributes(child, &prefixes)?;
            content.push(SdtContent::Cell(cell));
        }
        (b"fldSimple", _) => {
            let raw_xml = if empty {
                capture_empty(child, reader.empty_writing())?
            } else {
                capture_element(reader, child)?
            };
            let mapped = control_facts.as_deref().and_then(|facts| {
                facts.field_input(reader, event_start..reader.buffer_position(), &raw_xml)
            });
            if let Some(field) =
                crate::text::parse_simple_field(&raw_xml, &prefixes, mapped.as_ref())?
            {
                let run = crate::text::field_run(field, None);
                inline_run_sources.push(InlineRunSource {
                    content_index: content.len(),
                    original: run.clone(),
                    raw_xml,
                });
                content.push(SdtContent::Run(run));
            } else {
                content.push(SdtContent::RawXml(raw_xml));
            }
        }
        (b"r", _) => {
            let collect_word = source_maps
                .as_ref()
                .is_some_and(|maps| maps.collect_word_texts);
            let collect_properties = source_maps
                .as_ref()
                .is_some_and(|maps| maps.collect_properties);
            let mut property = None;
            let mut texts = Vec::new();
            let mut word_input = None;
            let (run, raw_xml) = if empty {
                (
                    CT_R {
                        properties: None,
                        content: Vec::new(),
                        extra_xml: Vec::new(),
                        extra_xml_positions: Vec::new(),
                        alt_drawings: Vec::new(),
                    },
                    capture_empty(child, reader.empty_writing())?,
                )
            } else {
                let (raw, mapped) = if collect_properties {
                    if let Some(cursor) = control_facts.as_deref() {
                        cursor.capture_element(reader, child, event_start)?
                    } else {
                        (capture_element(reader, child)?, None)
                    }
                } else {
                    let raw = capture_element(reader, child)?;
                    let mapped = control_facts.as_deref().and_then(|cursor| {
                        cursor.field_input(reader, event_start..reader.buffer_position(), &raw)
                    });
                    (raw, mapped)
                };
                let run = parse_inline_run_with_property(
                    &raw,
                    inherited,
                    mapped.as_ref(),
                    collect_word.then_some(&mut texts),
                    collect_properties.then_some(&mut property),
                )?;
                word_input = mapped;
                (run, raw)
            };
            if collect_word {
                let facts = word_input.map(|input| crate::text::ParsedWordRunTexts {
                    source: 0..raw_xml.len(),
                    source_map: Some(std::sync::Arc::new(input.projection_parts().0.clone())),
                    texts,
                });
                source_maps
                    .as_deref_mut()
                    .expect("selected Word facts")
                    .word_runs
                    .push((
                        vec![crate::text::WordBlockComponent::SdtContent(content.len())],
                        facts,
                    ));
            }
            inline_run_sources.push(InlineRunSource {
                content_index: content.len(),
                original: run.clone(),
                raw_xml,
            });
            content.push(SdtContent::Run(run));
            if let Some(property) = property {
                source_maps
                    .expect("selected property facts")
                    .ordinary
                    .push(property);
            }
        }
        _ => {
            return Err(OxmlError::InvalidValue(
                "unsupported modeled content-control child".into(),
            ));
        }
    }
    Ok(())
}

// Variant construction reserves space for the largest modeled leaf. Keep it
// outside the recursive control parser even for retained and nested children.
#[inline(never)]
fn append_retained_sdt_content(
    content: &mut Vec<SdtContent>,
    control: Option<CT_Sdt>,
    raw: Vec<u8>,
) {
    content.push(match control {
        Some(control) => SdtContent::ContentControl(control),
        None => SdtContent::RawXml(raw),
    });
}

// Preserve one source occurrence for each complete projected inline owner.
fn project_complete_inline_fields(
    content: &mut Vec<SdtContent>,
    revisions: &mut Vec<(usize, CT_Revision)>,
    inline_sources: &mut Vec<InlineRunSource>,
    source: &[u8],
    spans: &[Range<usize>],
    prefixes: &[String],
    mapped: Option<&crate::document::WordFieldInput>,
) -> Result<()> {
    let mut complete_candidate = false;
    for record in inline_sources.iter() {
        if crate::text::pure_complex_marker_run(&record.raw_xml, prefixes, "begin")? {
            complete_candidate = true;
            break;
        }
    }
    if !complete_candidate {
        return Ok(());
    }
    let kinds = content
        .iter()
        .map(|child| match child {
            SdtContent::Run(_) => 0u8,
            SdtContent::ContentControl(_) => 1,
            SdtContent::RawXml(_) => 2,
            _ => 3,
        })
        .collect::<Vec<_>>();
    if kinds.contains(&3) {
        return Err(OxmlError::InvalidValue(
            "non-inline model in qualified inline field projection".into(),
        ));
    }
    let revision_indices = revisions
        .iter()
        .map(|(at, _)| *at)
        .collect::<std::collections::HashSet<_>>();
    let mut originals = std::mem::take(inline_sources)
        .into_iter()
        .map(|record| (record.content_index, record))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut physical_runs = vec![None; content.len()];
    for (index, record) in &mut originals {
        physical_runs[*index] = Some(std::mem::take(&mut record.raw_xml));
    }
    let (paragraph, run_sources, comments, _) = crate::text::inline_field_paragraph_from_facts(
        std::mem::take(content),
        std::mem::take(revisions),
        physical_runs,
        source,
        spans,
        prefixes,
        true,
        mapped,
    )?;
    let mut runs = paragraph.runs.into_iter().zip(run_sources);
    let mut controls = paragraph
        .content_controls
        .into_iter()
        .map(|(_, _, _, control)| control);
    let mut raw = paragraph.extra_xml.into_iter().map(|(_, raw)| raw);
    let mut remaining_revisions = paragraph
        .revisions
        .into_iter()
        .map(|(_, _, revision)| revision);
    let mut comments = comments
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut physical = 0usize;
    while physical < kinds.len() {
        let new_index = content.len();
        match kinds[physical] {
            0 => {
                let (run, raw_source) = runs.next().ok_or_else(|| {
                    OxmlError::InvalidValue("inline projected run occurrence disappeared".into())
                })?;
                let raw_source = raw_source.ok_or_else(|| {
                    OxmlError::InvalidValue("inline projected run extent is unproved".into())
                })?;
                let start = spans[physical].start;
                let end = start.checked_add(raw_source.len()).ok_or_else(|| {
                    OxmlError::InvalidValue("inline source extent overflow".into())
                })?;
                if source.get(start..end) != Some(raw_source.as_slice()) {
                    return Err(OxmlError::InvalidValue(
                        "inline projected field does not own its actual physical source occurrence"
                            .into(),
                    ));
                }
                let last = spans[physical..]
                    .iter()
                    .position(|span| span.end == end)
                    .map(|offset| physical + offset)
                    .ok_or_else(|| {
                        OxmlError::InvalidValue(
                            "inline projected field ends inside a physical child".into(),
                        )
                    })?;
                let record = if last == physical {
                    let mut original = originals.remove(&physical).ok_or_else(|| {
                        OxmlError::InvalidValue(
                            "inline original run source slot disappeared".into(),
                        )
                    })?;
                    original.content_index = new_index;
                    original.raw_xml = raw_source;
                    original
                } else {
                    InlineRunSource {
                        content_index: new_index,
                        original: run.clone(),
                        raw_xml: raw_source,
                    }
                };
                inline_sources.push(record);
                content.push(SdtContent::Run(run));
                physical = last + 1;
            }
            1 => {
                content.push(SdtContent::ContentControl(controls.next().ok_or_else(
                    || OxmlError::InvalidValue("inline outside control source disappeared".into()),
                )?));
                physical += 1;
            }
            2 => {
                let bytes = comments
                    .remove(&physical)
                    .or_else(|| raw.next())
                    .ok_or_else(|| {
                        OxmlError::InvalidValue("inline outside raw source disappeared".into())
                    })?;
                if revision_indices.contains(&physical) {
                    revisions.push((
                        new_index,
                        remaining_revisions.next().ok_or_else(|| {
                            OxmlError::InvalidValue(
                                "inline outside revision source disappeared".into(),
                            )
                        })?,
                    ));
                }
                content.push(SdtContent::RawXml(bytes));
                physical += 1;
            }
            _ => unreachable!(),
        }
    }
    if runs.next().is_some()
        || controls.next().is_some()
        || raw.next().is_some()
        || remaining_revisions.next().is_some()
    {
        return Err(OxmlError::InvalidValue(
            "inline projection left an unconsumed current/source occurrence".into(),
        ));
    }
    Ok(())
}

fn parse_content(
    reader: &mut Reader<&[u8]>,
    inherited: &[String],
    revisions: &mut Vec<(usize, CT_Revision)>,
    inline_run_sources: &mut Vec<InlineRunSource>,
    owner: SdtOwner,
    mut control_facts: Option<&mut crate::document::ControlSourceCursor<'_>>,
    mut source_maps: Option<&mut crate::text::WordBlockSourceMaps>,
) -> Result<Vec<SdtContent>> {
    let physical_source = *reader.get_ref();
    let source_origin = reader.buffer_position() as usize;
    let mapped = control_facts.as_deref().and_then(|facts| {
        facts.field_input(
            reader,
            source_origin as u64..(source_origin + physical_source.len()) as u64,
            physical_source,
        )
    });
    let mut physical_spans = Vec::new();
    let mut content = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let event_start = reader.buffer_position();
        let physical_start = event_start as usize - source_origin;
        let content_before = content.len();
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(child)) => {
                let prefixes = word_prefixes_at(&child, inherited)?;
                let owned_word_child = [
                    b"p".as_slice(),
                    b"tbl".as_slice(),
                    b"tr".as_slice(),
                    b"tc".as_slice(),
                    b"r".as_slice(),
                    b"fldSimple".as_slice(),
                ]
                .into_iter()
                .find(|local| is_word_element(child.name().as_ref(), local, &prefixes));
                if owned_word_child.is_some_and(|local| !owner.owns(local)) {
                    append_retained_sdt_content(
                        &mut content,
                        None,
                        capture_element(reader, &child)?,
                    );
                } else if owned_word_child.is_some() {
                    append_modeled_content_child(
                        reader,
                        &child,
                        inherited,
                        false,
                        event_start,
                        &mut content,
                        inline_run_sources,
                        control_facts.as_deref_mut(),
                        source_maps.as_deref_mut(),
                    )?;
                } else if is_word_element(child.name().as_ref(), b"sdt", &prefixes) {
                    let raw = capture_element(reader, &child)?;
                    let captured_map = control_facts.as_deref().and_then(|facts| {
                        facts.captured_map(reader, event_start..reader.buffer_position(), &raw)
                    });
                    let mut child_maps =
                        source_maps
                            .as_ref()
                            .map(|maps| crate::text::WordBlockSourceMaps {
                                collect_word_texts: maps.collect_word_texts,
                                collect_properties: maps.collect_properties,
                                ..Default::default()
                            });
                    let parsed = CT_Sdt::from_raw_with_mapped_context_and_maps(
                        &raw,
                        &prefixes,
                        owner,
                        control_facts.as_deref_mut(),
                        captured_map,
                        child_maps.as_mut(),
                    );
                    if parsed.is_some()
                        && let (Some(output), Some(parsed_maps)) =
                            (source_maps.as_deref_mut(), child_maps)
                    {
                        output.append(
                            crate::text::WordBlockComponent::SdtContent(content.len()),
                            parsed_maps,
                        );
                    }
                    append_retained_sdt_content(&mut content, parsed, raw);
                } else {
                    let raw = capture_element(reader, &child)?;
                    let mapped = control_facts.as_deref().and_then(|facts| {
                        facts.field_input(reader, event_start..reader.buffer_position(), &raw)
                    });
                    let mut revision_maps = source_maps
                        .as_ref()
                        .filter(|maps| maps.collect_word_texts)
                        .map(|_| crate::text::WordBlockSourceMaps::default());
                    if let Some(revision) = CT_Revision::from_raw_with_read_output(
                        raw.clone(),
                        &prefixes,
                        true,
                        mapped.as_ref(),
                        revision_maps.as_mut(),
                    )? {
                        if let (Some(output), Some(child)) =
                            (source_maps.as_deref_mut(), revision_maps)
                        {
                            output.append(
                                crate::text::WordBlockComponent::ControlRevision(revisions.len()),
                                child,
                            );
                        }
                        if let (Some(output), Some(mapped)) =
                            (source_maps.as_deref_mut(), mapped.as_ref())
                            && output.collect_word_texts
                        {
                            output.word_raws.push((
                                vec![crate::text::WordBlockComponent::ControlRevision(
                                    revisions.len(),
                                )],
                                std::sync::Arc::new(mapped.projection_parts().0.clone()),
                            ));
                        }
                        revisions.push((content.len(), revision));
                    }
                    append_retained_sdt_content(&mut content, None, raw);
                }
            }
            Ok(Event::Empty(child)) => {
                let prefixes = word_prefixes_at(&child, inherited)?;
                let owned_word_child = [
                    b"p".as_slice(),
                    b"tbl".as_slice(),
                    b"tr".as_slice(),
                    b"tc".as_slice(),
                    b"r".as_slice(),
                    b"fldSimple".as_slice(),
                ]
                .into_iter()
                .find(|local| is_word_element(child.name().as_ref(), local, &prefixes));
                if owned_word_child.is_some_and(|local| !owner.owns(local)) {
                    append_retained_sdt_content(
                        &mut content,
                        None,
                        capture_empty(&child, reader.empty_writing())?,
                    );
                } else if owned_word_child.is_some() {
                    append_modeled_content_child(
                        reader,
                        &child,
                        inherited,
                        true,
                        event_start,
                        &mut content,
                        inline_run_sources,
                        control_facts.as_deref_mut(),
                        source_maps.as_deref_mut(),
                    )?;
                } else {
                    let raw = capture_empty(&child, reader.empty_writing())?;
                    let mapped = control_facts.as_deref().and_then(|facts| {
                        facts.field_input(reader, event_start..reader.buffer_position(), &raw)
                    });
                    let mut revision_maps = source_maps
                        .as_ref()
                        .filter(|maps| maps.collect_word_texts)
                        .map(|_| crate::text::WordBlockSourceMaps::default());
                    if let Some(revision) = CT_Revision::from_raw_with_read_output(
                        raw.clone(),
                        &prefixes,
                        true,
                        mapped.as_ref(),
                        revision_maps.as_mut(),
                    )? {
                        if let (Some(output), Some(child)) =
                            (source_maps.as_deref_mut(), revision_maps)
                        {
                            output.append(
                                crate::text::WordBlockComponent::ControlRevision(revisions.len()),
                                child,
                            );
                        }
                        if let (Some(output), Some(mapped)) =
                            (source_maps.as_deref_mut(), mapped.as_ref())
                            && output.collect_word_texts
                        {
                            output.word_raws.push((
                                vec![crate::text::WordBlockComponent::ControlRevision(
                                    revisions.len(),
                                )],
                                std::sync::Arc::new(mapped.projection_parts().0.clone()),
                            ));
                        }
                        revisions.push((content.len(), revision));
                    }
                    append_retained_sdt_content(&mut content, None, raw);
                }
            }
            Ok(Event::End(end)) if matches_local_name(end.name().as_ref(), b"sdtContent") => break,
            Ok(Event::Eof) => {
                return Err(OxmlError::MissingElement("w:sdtContent end".to_owned()));
            }
            Ok(event) => append_retained_sdt_content(&mut content, None, event_to_raw(event)?),
            Err(error) => return Err(error.into()),
        }
        if owner == SdtOwner::Inline {
            if content.len() != content_before + 1 {
                return Err(OxmlError::InvalidValue(
                    "inline physical source child cardinality changed during parse".into(),
                ));
            }
            physical_spans.push(physical_start..reader.buffer_position() as usize - source_origin);
        }
        buffer.clear();
    }
    if owner == SdtOwner::Inline {
        project_complete_inline_fields(
            &mut content,
            revisions,
            inline_run_sources,
            physical_source,
            &physical_spans,
            inherited,
            mapped.as_ref(),
        )?;
    }
    Ok(content)
}

fn checked_lengths(lengths: &[usize]) -> Result<usize> {
    lengths
        .iter()
        .try_fold(0usize, |total, length| total.checked_add(*length))
        .ok_or_else(|| OxmlError::InvalidValue("control source work overflow".into()))
}

fn attribute_bytes(attributes: &[(String, String)]) -> Result<usize> {
    attributes
        .iter()
        .try_fold(0usize, |total, (name, value)| {
            total
                .checked_add(name.len())
                .and_then(|total| total.checked_add(value.len()))
        })
        .ok_or_else(|| OxmlError::InvalidValue("control attribute work overflow".into()))
}

fn preflight_owned_raw(
    raw: &[&[u8]],
    original: &[String],
    budget: usize,
    mut projected: usize,
) -> Result<usize> {
    let destination = crate::text::emitted_bindings();
    for raw in raw {
        let bindings = destination.bindings_for(raw);
        let plan = crate::text::retained_root_plan(raw, original, &bindings, budget)?;
        projected = projected
            .checked_add(plan.added_bytes(raw))
            .filter(|bytes| *bytes <= budget)
            .ok_or_else(|| {
                OxmlError::InvalidValue(
                    "control namespace publication exceeds affected-source work budget".into(),
                )
            })?;
    }
    Ok(projected)
}

fn write_owned_raw_in<W: Write>(
    writer: &mut Writer<W>,
    raw: &[u8],
    original: &[String],
    role: XmlWriteRole,
    budget: usize,
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<()> {
    if role == XmlWriteRole::Source {
        if let Some(emission) = emission {
            emission.write_copy(writer, raw)?;
        } else {
            writer.get_mut().write_all(raw)?;
        }
    } else {
        let destination = crate::text::emitted_bindings();
        let plan =
            crate::text::retained_root_plan(raw, original, &destination.bindings_for(raw), budget)?;
        let (produced, prepaid) = crate::text::emit_retained_roots_in(raw, plan, emission)?;
        if let Some(emission) = emission {
            if prepaid {
                let _transport = emission.prepaid_xml_transport(&produced)?;
                writer.get_mut().write_all(&produced)?;
            } else {
                emission.write_copy(writer, &produced)?;
            }
        } else {
            writer.get_mut().write_all(&produced)?;
        }
    }
    Ok(())
}

fn write_val_element<W: Write>(
    writer: &mut Writer<W>,
    prefix: &str,
    local: &str,
    value: Option<&str>,
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<()> {
    if let Some(value) = value {
        let mut start = crate::text::FieldXmlStart::word(prefix, local, emission)?;
        start.push_word(prefix, "val", value)?;
        start.write(writer, true)?;
    }
    Ok(())
}

fn word_attribute_value(
    start: &BytesStart<'_>,
    local: &[u8],
    prefixes: &[String],
) -> Result<Option<String>> {
    for attribute in start.attributes() {
        let attribute = attribute?;
        if is_word_attribute(attribute.key.as_ref(), local, prefixes) {
            return Ok(Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

fn is_word_attribute(key: &[u8], local: &[u8], prefixes: &[String]) -> bool {
    let Some(separator) = key.iter().position(|byte| *byte == b':') else {
        return false;
    };
    key.get(separator + 1..) == Some(local)
        && prefixes
            .iter()
            .any(|prefix| prefix.as_bytes() == &key[..separator])
}

fn is_supported_type_element(name: &[u8], local: &[u8], prefixes: &[String]) -> bool {
    if local != b"checkbox" && is_word_element(name, local, prefixes) {
        return true;
    }
    let prefix = match name.iter().position(|byte| *byte == b':') {
        Some(separator) => &name[..separator],
        None if local == b"checkbox" => b"".as_slice(),
        None => return false,
    };
    prefixes.iter().any(|binding| {
        let Some(rest) = binding.strip_prefix('\0') else {
            return false;
        };
        let Some((bound_prefix, namespace)) = rest.split_once('\0') else {
            return false;
        };
        bound_prefix.as_bytes() == prefix
            && if local == b"checkbox" {
                namespace == "http://schemas.microsoft.com/office/word/2010/wordml"
            } else {
                matches!(
                    namespace,
                    "http://schemas.microsoft.com/office/word/2010/wordml"
                        | "http://schemas.microsoft.com/office/word/2012/wordml"
                )
            }
    })
}

fn capture_attributes(
    start: &BytesStart<'_>,
    prefixes: &[String],
    modelled: &[&[u8]],
) -> Result<Vec<(String, String)>> {
    let mut attributes = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute?;
        if modelled
            .iter()
            .any(|local| is_word_attribute(attribute.key.as_ref(), local, prefixes))
        {
            continue;
        }
        attributes.push((
            std::str::from_utf8(attribute.key.as_ref())?.to_owned(),
            attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())?
                .into_owned(),
        ));
    }
    Ok(attributes)
}

fn event_to_raw(event: Event<'_>) -> Result<Vec<u8>> {
    let mut writer = Writer::new(Vec::new());
    writer.write_event(event.into_owned())?;
    Ok(writer.into_inner())
}

fn push_event_raw(slots: &mut Vec<PropertySlot>, event: Event<'_>) -> Result<()> {
    let raw = event_to_raw(event)?;
    if !raw.is_empty() {
        slots.push(PropertySlot::Raw(raw));
    }
    Ok(())
}

fn push_root_event_raw(slots: &mut Vec<RootSlot>, event: Event<'_>) -> Result<()> {
    let raw = event_to_raw(event)?;
    if !raw.is_empty() {
        slots.push(RootSlot::Raw(raw));
    }
    Ok(())
}

fn raw_fragment_has_child_content(raw: &[u8]) -> bool {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(_) | Event::Empty(_)) => return true,
            Ok(Event::Text(text)) if !text.as_ref().iter().all(u8::is_ascii_whitespace) => {
                return true;
            }
            Ok(Event::CData(text)) if !text.as_ref().iter().all(u8::is_ascii_whitespace) => {
                return true;
            }
            Ok(Event::GeneralRef(reference)) => match reference.resolve_char_ref() {
                Ok(Some(character)) if !character.is_ascii_whitespace() => return true,
                Ok(Some(_)) => {}
                Ok(None) => return true,
                Err(_) => return false,
            },
            Ok(Event::Eof) | Err(_) => return false,
            _ => {}
        }
        buffer.clear();
    }
}

/// The actual sdtPr parser models only these property leaves/attributes.
pub(crate) fn source_modeled_attributes(
    parent: &[u8],
    local: &[u8],
) -> Option<&'static [&'static [u8]]> {
    if parent != b"sdtPr" {
        return None;
    }
    match local {
        b"alias" | b"tag" | b"id" => Some(&[b"val"]),
        b"dataBinding" => Some(&[b"prefixMappings", b"xpath", b"storeItemID"]),
        // Type element attributes and payload are explicitly retained source state.
        _ if SdtType::from_element(local).is_some() => Some(&[]),
        _ => None,
    }
}

#[cfg(test)]
mod form_facts_tests {
    use super::*;

    const NAMESPACES: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml""#;

    /// The properties of a block control with these properties and content.
    fn sdt_properties(properties: &str) -> CT_SdtPr {
        let xml = format!(
            "<w:sdt {NAMESPACES}><w:sdtPr>{properties}</w:sdtPr><w:sdtContent><w:p><w:r><w:t>x</w:t></w:r></w:p></w:sdtContent></w:sdt>"
        );
        let mut reader = Reader::from_reader(xml.as_bytes());
        let Event::Start(start) = reader.read_event().unwrap() else {
            panic!("a start tag");
        };
        let start = start.into_owned();
        CT_Sdt::from_xml(&mut reader, &start)
            .unwrap()
            .properties
            .unwrap()
    }

    /// The form facts of a block control with these properties and content.
    fn facts(properties: &str) -> SdtFormFacts {
        sdt_properties(properties).form_facts().unwrap()
    }

    #[test]
    fn a_control_reads_its_own_run_properties_under_any_prefix_of_the_main_namespace() {
        let word = sdt_properties(
            r#"<w:rPr><w:rFonts w:ascii="Arial"/><w:b/><w:color w:val="1F3864"/></w:rPr><w:id w:val="1"/><w:showingPlcHdr/>"#,
        )
        .run_properties()
        .unwrap()
        .expect("the control has run properties");
        assert_eq!(word.bold, Some(true));
        assert_eq!(word.font_ascii.as_deref(), Some("Arial"));
        assert_eq!(word.color.as_deref(), Some("1F3864"));
        let prefixed = sdt_properties(
            r#"<ns0:rPr xmlns:ns0="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><ns0:i/></ns0:rPr>"#,
        )
        .run_properties()
        .unwrap()
        .expect("run properties under another prefix");
        assert_eq!(prefixed.italic, Some(true));
        // None of another namespace, and none at all.
        for properties in [
            r#"<x:rPr xmlns:x="urn:example"><x:b/></x:rPr><w:id w:val="1"/>"#,
            r#"<w:id w:val="1"/>"#,
        ] {
            assert_eq!(
                sdt_properties(properties).run_properties().unwrap(),
                None,
                "{properties}"
            );
        }
    }

    #[test]
    fn a_word_plain_text_control_reads_its_lock_placeholder_and_multiline() {
        let facts = facts(
            r#"<w:alias w:val="Address"/><w:tag w:val="address"/><w:id w:val="1"/><w:lock w:val="contentLocked"/><w:placeholder><w:docPart w:val="DefaultPlaceholder_1081868574"/></w:placeholder><w:showingPlcHdr/><w:text w:multiLine="1"/>"#,
        );
        assert_eq!(facts.lock, SdtLock::ContentLocked);
        assert!(facts.lock.content_locked());
        assert!(facts.showing_placeholder);
        assert_eq!(facts.payload, SdtFormPayload::PlainText { multiline: true });
    }

    #[test]
    fn a_control_without_lock_placeholder_or_type_is_unlocked_rich_text() {
        let plain = facts(r#"<w:id w:val="7"/>"#);
        assert_eq!(plain.lock, SdtLock::Unlocked);
        assert!(!plain.showing_placeholder);
        assert_eq!(plain.payload, SdtFormPayload::RichText);
        assert_eq!(facts(r#"<w:richText/>"#).payload, SdtFormPayload::RichText);
        assert_eq!(
            facts(r#"<w:showingPlcHdr w:val="0"/><w:lock w:val="sdtLocked"/>"#),
            SdtFormFacts {
                lock: SdtLock::SdtLocked,
                showing_placeholder: false,
                payload: SdtFormPayload::RichText,
            }
        );
    }

    #[test]
    fn a_word_checkbox_reads_its_state_and_both_symbols() {
        let facts = facts(
            r#"<w:id w:val="2"/><w14:checkbox><w14:checked w14:val="1"/><w14:checkedState w14:val="2612" w14:font="MS Gothic"/><w14:uncheckedState w14:val="2610" w14:font="MS Gothic"/></w14:checkbox>"#,
        );
        assert_eq!(
            facts.payload,
            SdtFormPayload::CheckBox {
                checked: true,
                checked_symbol: ('\u{2612}', "MS Gothic".to_owned()),
                unchecked_symbol: ('\u{2610}', "MS Gothic".to_owned()),
            }
        );
    }

    #[test]
    fn a_checkbox_reads_an_explicit_unchecked_value_and_a_bare_checked_element() {
        let checked =
            |element: &str| match facts(&format!("<w14:checkbox>{element}</w14:checkbox>")).payload
            {
                SdtFormPayload::CheckBox { checked, .. } => checked,
                other => panic!("a check box, not {other:?}"),
            };
        assert!(
            !checked(r#"<w14:checked w14:val="0"/>"#),
            "Word writes this on every unchecked box"
        );
        assert!(!checked(r#"<w14:checked w14:val="false"/>"#));
        assert!(checked(r#"<w14:checked w14:val="1"/>"#));
        assert!(
            checked(r#"<w14:checked/>"#),
            "an on/off property without a value is on"
        );
    }

    #[test]
    fn a_plain_text_control_is_single_line_and_every_lock_reads() {
        assert_eq!(
            facts(r#"<w:text/>"#).payload,
            SdtFormPayload::PlainText { multiline: false }
        );
        assert_eq!(
            facts(r#"<w:text w:multiLine="0"/>"#).payload,
            SdtFormPayload::PlainText { multiline: false }
        );
        for (value, lock) in [
            ("sdtLocked", SdtLock::SdtLocked),
            ("contentLocked", SdtLock::ContentLocked),
            ("sdtContentLocked", SdtLock::SdtContentLocked),
            ("unlocked", SdtLock::Unlocked),
        ] {
            let read = facts(&format!(r#"<w:lock w:val="{value}"/>"#)).lock;
            assert_eq!(read, lock, "{value}");
        }
        assert!(SdtLock::SdtContentLocked.content_locked());
        assert!(!SdtLock::SdtLocked.content_locked());
    }

    #[test]
    fn a_lock_and_placeholder_read_under_any_prefix_of_the_main_namespace() {
        let main = facts(
            r#"<ns0:lock xmlns:ns0="http://schemas.openxmlformats.org/wordprocessingml/2006/main" ns0:val="contentLocked"/><ns0:showingPlcHdr xmlns:ns0="http://schemas.openxmlformats.org/wordprocessingml/2006/main"/>"#,
        );
        assert_eq!(main.lock, SdtLock::ContentLocked);
        assert!(main.showing_placeholder);
        // A value from another namespace is not Word's, before or after it.
        for lock in [
            r#"<w:lock xmlns:x="urn:example" x:val="contentLocked" w:val="sdtLocked"/>"#,
            r#"<w:lock xmlns:x="urn:example" w:val="sdtLocked" x:val="contentLocked"/>"#,
        ] {
            assert_eq!(facts(lock).lock, SdtLock::SdtLocked, "{lock}");
        }
    }

    #[test]
    fn a_lock_or_placeholder_from_another_namespace_is_not_word_s() {
        let facts = facts(
            r#"<x:lock xmlns:x="urn:example" x:val="contentLocked"/><x:showingPlcHdr xmlns:x="urn:example"/>"#,
        );
        assert_eq!(facts.lock, SdtLock::Unlocked);
        assert!(!facts.showing_placeholder);
    }

    #[test]
    fn a_checkbox_with_custom_symbols_and_no_checked_element_is_unchecked() {
        let facts = facts(
            r#"<w14:checkbox><w14:checkedState w14:val="0052" w14:font="Wingdings 2"/><w14:uncheckedState w14:val="00A3" w14:font="Wingdings 2"/></w14:checkbox>"#,
        );
        assert_eq!(
            facts.payload,
            SdtFormPayload::CheckBox {
                checked: false,
                checked_symbol: ('R', "Wingdings 2".to_owned()),
                unchecked_symbol: ('\u{a3}', "Wingdings 2".to_owned()),
            }
        );
    }

    #[test]
    fn list_controls_read_their_entries_as_word_and_libreoffice_write_them() {
        let word = facts(
            r#"<w:dropDownList><w:listItem w:displayText="Choose an item." w:value=""/><w:listItem w:displayText="France" w:value="fr"/><w:listItem w:displayText="Spain" w:value="Spain"/></w:dropDownList>"#,
        );
        assert_eq!(
            word.payload,
            SdtFormPayload::DropDownList {
                items: vec![
                    SdtListItem {
                        display: "Choose an item.".to_owned(),
                        value: String::new(),
                    },
                    SdtListItem {
                        display: "France".to_owned(),
                        value: "fr".to_owned(),
                    },
                    SdtListItem {
                        display: "Spain".to_owned(),
                        value: "Spain".to_owned(),
                    },
                ],
            }
        );
        // A missing display shows the value, a missing value is empty.
        let partial = facts(
            r#"<w:comboBox><w:listItem w:value="Lyon"/><w:listItem w:displayText="Paris"/></w:comboBox>"#,
        );
        assert_eq!(
            partial.payload,
            SdtFormPayload::ComboBox {
                items: vec![
                    SdtListItem {
                        display: "Lyon".to_owned(),
                        value: "Lyon".to_owned(),
                    },
                    SdtListItem {
                        display: "Paris".to_owned(),
                        value: String::new(),
                    },
                ],
            }
        );
    }

    #[test]
    fn a_date_control_reads_its_format() {
        let word = facts(
            r#"<w:date w:fullDate="2026-11-21T00:00:00Z"><w:dateFormat w:val="dd/MM/yyyy"/><w:lid w:val="fr-FR"/><w:storeMappedDataAs w:val="dateTime"/><w:calendar w:val="gregorian"/></w:date>"#,
        );
        assert_eq!(
            word.payload,
            SdtFormPayload::Date {
                format: Some("dd/MM/yyyy".to_owned()),
            }
        );
        assert_eq!(
            facts(r#"<w:date/>"#).payload,
            SdtFormPayload::Date { format: None }
        );
    }

    #[test]
    fn containers_and_pictures_have_no_form_payload() {
        assert_eq!(facts(r#"<w:group/>"#).payload, SdtFormPayload::Other);
        assert_eq!(facts(r#"<w:picture/>"#).payload, SdtFormPayload::Picture);
    }
}
