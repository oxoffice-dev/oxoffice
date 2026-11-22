//! Word font-table parsing and schema-ordered serialization.

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, Event};
use quick_xml::{Reader, Writer, XmlVersion};

use crate::error::{OxmlError, Result};
use crate::namespace::{R_NS, W_NS};
use crate::properties::{is_word_attribute, is_word_element, word_prefixes_at};
use crate::raw_xml::{capture_element, capture_empty_element, capture_event};

const RDOCX_FONT_NS: &str = "urn:rdocx:font-embedding:1";
const MC_NS: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const FONT_CHILD_COUNT: usize = 11;

/// The four embedded-face positions defined by `CT_Font`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FontFaceKind {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl FontFaceKind {
    fn local_name(self) -> &'static str {
        match self {
            Self::Regular => "embedRegular",
            Self::Bold => "embedBold",
            Self::Italic => "embedItalic",
            Self::BoldItalic => "embedBoldItalic",
        }
    }

    fn slot(self) -> usize {
        match self {
            Self::Regular => 7,
            Self::Bold => 8,
            Self::Italic => 9,
            Self::BoldItalic => 10,
        }
    }
}

/// Relationship-backed metadata for one embedded face.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddedFontReference {
    pub kind: FontFaceKind,
    pub relationship_id: String,
    pub font_key: String,
    pub subsetted: Option<bool>,
    pub authorized: Option<bool>,
    pub license_identity: Option<String>,
    raw_attributes: Vec<(String, String)>,
    raw_inner: Option<Vec<u8>>,
}

impl EmbeddedFontReference {
    pub fn new(
        kind: FontFaceKind,
        relationship_id: String,
        font_key: String,
        subsetted: Option<bool>,
        authorized: Option<bool>,
        license_identity: Option<String>,
    ) -> Self {
        Self {
            kind,
            relationship_id,
            font_key,
            subsetted,
            authorized,
            license_identity,
            raw_attributes: Vec::new(),
            raw_inner: None,
        }
    }
}

/// One `w:font` record, including retained producer XML.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FontRecord {
    pub name: String,
    pub alternate_name: Option<String>,
    pub family: Option<String>,
    pub pitch: Option<String>,
    pub embedded_fonts: Vec<EmbeddedFontReference>,
    alternate_name_raw_attributes: Vec<(String, String)>,
    alternate_name_raw_inner: Option<Vec<u8>>,
    family_raw_attributes: Vec<(String, String)>,
    family_raw_inner: Option<Vec<u8>>,
    pitch_raw_attributes: Vec<(String, String)>,
    pitch_raw_inner: Option<Vec<u8>>,
    raw_attributes: Vec<(String, String)>,
    raw_children: Vec<(usize, Vec<u8>)>,
    /// Set when the record is read: a child kept in `raw_children` names a
    /// relationship. Only the relationships of modelled embedded faces are
    /// cleaned up after the record, so removing it would leave this one.
    raw_children_name_relationship: bool,
    /// The slots of the `w:altName`, `w:family`, `w:pitch` and embedded face
    /// elements kept in `raw_children` because the model cannot read them
    /// (no `w:val`, or an element inside). They read as
    /// unset, so setting one would write the element a second time.
    kept_field_slots: Vec<usize>,
}

impl FontRecord {
    fn new(name: String) -> Self {
        Self {
            name,
            alternate_name: None,
            family: None,
            pitch: None,
            embedded_fonts: Vec::new(),
            alternate_name_raw_attributes: Vec::new(),
            alternate_name_raw_inner: None,
            family_raw_attributes: Vec::new(),
            family_raw_inner: None,
            pitch_raw_attributes: Vec::new(),
            pitch_raw_inner: None,
            raw_attributes: Vec::new(),
            raw_children: Vec::new(),
            raw_children_name_relationship: false,
            kept_field_slots: Vec::new(),
        }
    }

    /// Whether an attribute this record keeps, at any depth and in any
    /// namespace, has the value `value`. Its modelled faces' relationship ids
    /// are fields, not kept attributes.
    fn mentions_value(&self, value: &str) -> bool {
        raw_attributes_mention(&self.raw_attributes, value)
            || raw_attributes_mention(&self.alternate_name_raw_attributes, value)
            || raw_attributes_mention(&self.family_raw_attributes, value)
            || raw_attributes_mention(&self.pitch_raw_attributes, value)
            || self
                .embedded_fonts
                .iter()
                .any(|embedded| raw_attributes_mention(&embedded.raw_attributes, value))
            || self
                .raw_children
                .iter()
                .any(|(_, raw)| raw_xml_mentions_value(raw, value))
    }

    /// Keep a child this model does not read, verbatim at its schema slot,
    /// or after the last known child when it has none.
    fn retain_field_or_child(
        &mut self,
        slot: Option<usize>,
        boundary: usize,
        raw: Vec<u8>,
        relationship_prefixes: &[String],
    ) {
        if let Some(field @ (0 | 3 | 5 | 7..=10)) = slot {
            self.kept_field_slots.push(field);
        }
        self.retain_child(slot.unwrap_or(boundary), raw, relationship_prefixes);
    }

    /// Keep a child this model does not read, verbatim at `slot`.
    fn retain_child(&mut self, slot: usize, raw: Vec<u8>, relationship_prefixes: &[String]) {
        self.raw_children_name_relationship |=
            raw_xml_names_relationship(&raw, relationship_prefixes);
        self.raw_children.push((slot, raw));
    }
}

/// Relationship-resolved `word/fontTable.xml` state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FontTable {
    fonts: Vec<FontRecord>,
    ignorable_prefixes: Vec<String>,
    raw_attributes: Vec<(String, String)>,
    raw_children: Vec<(usize, Vec<u8>)>,
}

impl FontTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        let mut reader = Reader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer)? {
                Event::Start(start) => {
                    let prefixes = word_prefixes_at(&start, &[])?;
                    if is_word_element(start.name().as_ref(), b"fonts", &prefixes) {
                        reject_conflicting_writer_prefixes(&start)?;
                        return Self::read(&mut reader, &start, prefixes.into_owned());
                    }
                    return Err(OxmlError::UnexpectedElement(element_name(&start)));
                }
                Event::Empty(start) => {
                    let prefixes = word_prefixes_at(&start, &[])?;
                    if is_word_element(start.name().as_ref(), b"fonts", &prefixes) {
                        reject_conflicting_writer_prefixes(&start)?;
                        let mc_prefixes = namespace_prefixes_at(&start, &[], MC_NS)?;
                        let ignorable_prefixes =
                            qualified_attribute(&start, b"Ignorable", &mc_prefixes)?
                                .map(|value| {
                                    value.split_ascii_whitespace().map(str::to_owned).collect()
                                })
                                .unwrap_or_default();
                        return Ok(Self {
                            ignorable_prefixes,
                            raw_attributes: raw_attributes(
                                &start,
                                &[],
                                &prefixes,
                                &[(b"Ignorable", &mc_prefixes)],
                            )?,
                            ..Self::default()
                        });
                    }
                    return Err(OxmlError::UnexpectedElement(element_name(&start)));
                }
                Event::Eof => return Err(OxmlError::MissingElement("w:fonts".to_owned())),
                _ => {}
            }
            buffer.clear();
        }
    }

    fn read(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        root_prefixes: Vec<String>,
    ) -> Result<Self> {
        let relationship_prefixes = namespace_prefixes_at(start, &[], R_NS)?;
        let extension_prefixes = namespace_prefixes_at(start, &[], RDOCX_FONT_NS)?;
        let mc_prefixes = namespace_prefixes_at(start, &[], MC_NS)?;
        let ignorable_prefixes = qualified_attribute(start, b"Ignorable", &mc_prefixes)?
            .map(|value| value.split_ascii_whitespace().map(str::to_owned).collect())
            .unwrap_or_default();
        let mut table = Self {
            fonts: Vec::new(),
            ignorable_prefixes,
            raw_attributes: raw_attributes(
                start,
                &[],
                &root_prefixes,
                &[(b"Ignorable", &mc_prefixes)],
            )?,
            raw_children: Vec::new(),
        };
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer)? {
                Event::Start(child) => {
                    let prefixes = word_prefixes_at(&child, &root_prefixes)?;
                    if is_word_element(child.name().as_ref(), b"font", &prefixes) {
                        table.fonts.push(read_font(
                            reader,
                            &child,
                            prefixes.into_owned(),
                            namespace_prefixes_at(&child, &relationship_prefixes, R_NS)?,
                            namespace_prefixes_at(&child, &extension_prefixes, RDOCX_FONT_NS)?,
                        )?);
                    } else {
                        table
                            .raw_children
                            .push((table.fonts.len(), capture_element(reader, &child)?));
                    }
                }
                Event::Empty(child) => {
                    let prefixes = word_prefixes_at(&child, &root_prefixes)?;
                    if is_word_element(child.name().as_ref(), b"font", &prefixes) {
                        table.fonts.push(read_empty_font(&child, &prefixes)?);
                    } else {
                        table
                            .raw_children
                            .push((table.fonts.len(), capture_empty_element(&child)?));
                    }
                }
                event @ (Event::Text(_)
                | Event::CData(_)
                | Event::Comment(_)
                | Event::PI(_)
                | Event::Decl(_)
                | Event::DocType(_)
                | Event::GeneralRef(_)) => table
                    .raw_children
                    .push((table.fonts.len(), capture_event(event)?)),
                Event::End(end)
                    if is_word_element(end.name().as_ref(), b"fonts", &root_prefixes) =>
                {
                    break;
                }
                Event::Eof => return Err(OxmlError::MissingElement("w:fonts end".to_owned())),
                _ => {}
            }
            buffer.clear();
        }
        Ok(table)
    }

    pub fn to_xml(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new(Vec::new());
        writer.write_event(Event::Decl(BytesDecl::new(
            "1.0",
            Some("UTF-8"),
            Some("yes"),
        )))?;
        let mut root = BytesStart::new("w:fonts");
        root.push_attribute(("xmlns:w", W_NS));
        root.push_attribute(("xmlns:r", R_NS));
        root.push_attribute(("xmlns:rdocx", RDOCX_FONT_NS));
        root.push_attribute(("xmlns:mc", MC_NS));
        let mut ignorable_prefixes = self.ignorable_prefixes.clone();
        if !ignorable_prefixes.iter().any(|prefix| prefix == "rdocx") {
            ignorable_prefixes.push("rdocx".to_owned());
        }
        push_attribute_escaped(&mut root, "mc:Ignorable", &ignorable_prefixes.join(" "));
        push_attributes(&mut root, &self.raw_attributes);
        writer.write_event(Event::Start(root))?;
        emit_raw(&mut writer, &self.raw_children, 0);
        for (index, font) in self.fonts.iter().enumerate() {
            write_font(&mut writer, font)?;
            emit_raw(&mut writer, &self.raw_children, index + 1);
        }
        writer.write_event(Event::End(BytesEnd::new("w:fonts")))?;
        let xml = writer.into_inner();
        oxoffice_core::xml::validate_strict_xml_1_0(&xml).map_err(|error| {
            OxmlError::InvalidValue(format!("invalid serialized font-table XML: {error:?}"))
        })?;
        Ok(xml)
    }

    pub fn fonts(&self) -> &[FontRecord] {
        &self.fonts
    }

    pub fn set_font(
        &mut self,
        name: String,
        alternate_name: Option<String>,
        family: Option<String>,
        pitch: Option<String>,
    ) {
        if let Some(font) = self.fonts.iter_mut().find(|font| font.name == name) {
            if alternate_name.is_none() {
                font.alternate_name_raw_attributes.clear();
                font.alternate_name_raw_inner = None;
            }
            if family.is_none() {
                font.family_raw_attributes.clear();
                font.family_raw_inner = None;
            }
            if pitch.is_none() {
                font.pitch_raw_attributes.clear();
                font.pitch_raw_inner = None;
            }
            font.alternate_name = alternate_name;
            font.family = family;
            font.pitch = pitch;
            return;
        }
        let mut font = FontRecord::new(name);
        font.alternate_name = alternate_name;
        font.family = family;
        font.pitch = pitch;
        self.fonts.push(font);
    }

    /// Check whether deleting a record leaves nothing behind it. The record
    /// goes whole, so the children it keeps verbatim (the `w:panose1`,
    /// `w:charset`, `w:notTrueType` and `w:sig` Word writes on every record,
    /// or unknown ones) go with it, unless one names a relationship: only the
    /// relationships of modelled embedded faces are cleaned up after it.
    /// Retained attributes and opaque embedded faces are still refused.
    /// `Document::remove_font` also refuses when the kept XML names, in another
    /// namespace, a relationship nothing else would use after the record.
    pub fn can_remove_font(&self, name: &str) -> Result<bool> {
        let mut matches = self.fonts.iter().filter(|font| font.name == name);
        let Some(font) = matches.next() else {
            return Ok(false);
        };
        if matches.next().is_some() {
            return Err(OxmlError::InvalidValue("ambiguous font record".to_owned()));
        }
        if font.raw_children_name_relationship {
            return Err(OxmlError::InvalidValue(
                "cannot remove a font record whose retained XML names a relationship".to_owned(),
            ));
        }
        if !only_namespace_declarations(&font.raw_attributes)
            || !only_namespace_declarations(&font.alternate_name_raw_attributes)
            || !only_namespace_declarations(&font.family_raw_attributes)
            || !only_namespace_declarations(&font.pitch_raw_attributes)
            || [
                &font.alternate_name_raw_inner,
                &font.family_raw_inner,
                &font.pitch_raw_inner,
            ]
            .into_iter()
            .any(holds_content)
        {
            return Err(OxmlError::InvalidValue(
                "cannot remove opaque font record payload".to_owned(),
            ));
        }
        for face in &font.embedded_fonts {
            self.can_remove_embedded_font(name, face.kind)?;
        }
        Ok(true)
    }

    /// Check whether `set_font` can clear these fields of the record named
    /// `name`. A cleared field takes its element's attributes and content
    /// with it, so one that carries more than namespace declarations is
    /// refused. The record's other children, the XML it keeps and its
    /// embedded faces stay, so they are not checked.
    pub fn can_clear_font_fields(
        &self,
        name: &str,
        alternate_name: bool,
        family: bool,
        pitch: bool,
    ) -> Result<()> {
        let mut matches = self.fonts.iter().filter(|font| font.name == name);
        let Some(font) = matches.next() else {
            return Ok(());
        };
        if matches.next().is_some() {
            return Err(OxmlError::InvalidValue("ambiguous font record".to_owned()));
        }
        let fields = [
            (
                alternate_name,
                "alternate name",
                &font.alternate_name_raw_attributes,
                &font.alternate_name_raw_inner,
            ),
            (
                family,
                "family",
                &font.family_raw_attributes,
                &font.family_raw_inner,
            ),
            (
                pitch,
                "pitch",
                &font.pitch_raw_attributes,
                &font.pitch_raw_inner,
            ),
        ];
        for (cleared, field, attributes, inner) in fields {
            if cleared && (!only_namespace_declarations(attributes) || holds_content(inner)) {
                return Err(OxmlError::InvalidValue(format!(
                    "cannot clear the {field} of font {name}: it holds unmodelled XML"
                )));
            }
        }
        Ok(())
    }

    /// Check whether `set_font` can set these fields of the record named
    /// `name`. A field whose element the record keeps verbatim reads as
    /// unset; setting it would write a second element, which a reader then
    /// refuses as out of order, so it is refused.
    pub fn can_set_font_fields(
        &self,
        name: &str,
        alternate_name: bool,
        family: bool,
        pitch: bool,
    ) -> Result<()> {
        let Some(font) = self.fonts.iter().find(|font| font.name == name) else {
            return Ok(());
        };
        for (set, slot, field) in [
            (alternate_name, 0, "alternate name"),
            (family, 3, "family"),
            (pitch, 5, "pitch"),
        ] {
            if set && font.kept_field_slots.contains(&slot) {
                return Err(OxmlError::InvalidValue(format!(
                    "cannot set the {field} of font {name}: the file keeps that element in a form the model does not read"
                )));
            }
        }
        Ok(())
    }

    /// Check whether `embed_font` can give the record named `name` a face of
    /// this kind: a face element of that kind kept as written reads as no
    /// face, so a new one would be a second element, which is refused.
    pub fn can_embed_font(&self, name: &str, kind: FontFaceKind) -> Result<()> {
        if self
            .fonts
            .iter()
            .find(|font| font.name == name)
            .is_some_and(|font| font.kept_field_slots.contains(&kind.slot()))
        {
            return Err(OxmlError::InvalidValue(format!(
                "cannot embed the {} face of font {name}: the file keeps that element in a form the model does not read",
                kind.local_name()
            )));
        }
        Ok(())
    }

    /// Check that one uniquely identified face has no unowned deletion payload.
    pub fn can_remove_embedded_font(&self, name: &str, kind: FontFaceKind) -> Result<bool> {
        let mut fonts = self.fonts.iter().filter(|font| font.name == name);
        let Some(font) = fonts.next() else {
            return Ok(false);
        };
        if fonts.next().is_some() {
            return Err(OxmlError::InvalidValue("ambiguous font record".to_owned()));
        }
        let mut faces = font.embedded_fonts.iter().filter(|face| face.kind == kind);
        let Some(face) = faces.next() else {
            return Ok(false);
        };
        if faces.next().is_some() {
            return Err(OxmlError::InvalidValue(
                "ambiguous embedded font face".to_owned(),
            ));
        }
        if face
            .raw_attributes
            .iter()
            .any(|(key, _)| key != "xmlns" && !key.starts_with("xmlns:"))
            || face
                .raw_inner
                .as_ref()
                .is_some_and(|raw| !raw.iter().all(u8::is_ascii_whitespace))
        {
            return Err(OxmlError::InvalidValue(
                "cannot remove opaque embedded face payload".to_owned(),
            ));
        }
        Ok(true)
    }

    pub fn remove_font(&mut self, name: &str) -> Option<FontRecord> {
        let index = self.fonts.iter().position(|font| font.name == name)?;
        let removed = self.fonts.remove(index);
        for (position, _) in &mut self.raw_children {
            if *position > index {
                *position -= 1;
            }
        }
        Some(removed)
    }

    pub fn set_embedded_font(&mut self, font_name: &str, embedded: EmbeddedFontReference) -> bool {
        let Some(font) = self.fonts.iter_mut().find(|font| font.name == font_name) else {
            return false;
        };
        if let Some(existing) = font
            .embedded_fonts
            .iter_mut()
            .find(|value| value.kind == embedded.kind)
        {
            let raw_attributes = std::mem::take(&mut existing.raw_attributes);
            let raw_inner = existing.raw_inner.take();
            *existing = embedded;
            existing.raw_attributes = raw_attributes;
            existing.raw_inner = raw_inner;
        } else {
            font.embedded_fonts.push(embedded);
            font.embedded_fonts.sort_by_key(|value| value.kind.slot());
        }
        true
    }

    pub fn remove_embedded_font(
        &mut self,
        font_name: &str,
        kind: FontFaceKind,
    ) -> Option<EmbeddedFontReference> {
        let font = self.fonts.iter_mut().find(|font| font.name == font_name)?;
        let index = font
            .embedded_fonts
            .iter()
            .position(|embedded| embedded.kind == kind)?;
        Some(font.embedded_fonts.remove(index))
    }

    pub fn embedded_relationship_reference_count(&self, relationship_id: &str) -> usize {
        self.fonts
            .iter()
            .flat_map(|font| &font.embedded_fonts)
            .filter(|embedded| embedded.relationship_id == relationship_id)
            .count()
    }

    /// Whether the record named `name` keeps an attribute whose value is
    /// `value`, at any depth and in any namespace, besides its modelled faces'
    /// relationship ids: the test the cleanup after a removal applies to keep
    /// a relationship.
    pub fn record_mentions_value(&self, name: &str, value: &str) -> bool {
        self.fonts
            .iter()
            .filter(|font| font.name == name)
            .any(|font| font.mentions_value(value))
    }

    pub fn retained_raw_mentions_relationship(&self, relationship_id: &str) -> bool {
        raw_attributes_mention(&self.raw_attributes, relationship_id)
            || self
                .raw_children
                .iter()
                .any(|(_, raw)| raw_xml_mentions_value(raw, relationship_id))
            || self
                .fonts
                .iter()
                .any(|font| font.mentions_value(relationship_id))
    }
}

fn read_empty_font(start: &BytesStart<'_>, prefixes: &[String]) -> Result<FontRecord> {
    reject_conflicting_writer_prefixes(start)?;
    let name = word_attribute(start, b"name", prefixes)?
        .ok_or_else(|| OxmlError::MissingElement("w:font@w:name".to_owned()))?;
    let mut font = FontRecord::new(name);
    font.raw_attributes = raw_attributes(start, &[b"name"], prefixes, &[])?;
    Ok(font)
}

fn read_font(
    reader: &mut Reader<&[u8]>,
    start: &BytesStart<'_>,
    prefixes: Vec<String>,
    relationship_prefixes: Vec<String>,
    extension_prefixes: Vec<String>,
) -> Result<FontRecord> {
    reject_conflicting_writer_prefixes(start)?;
    let name = word_attribute(start, b"name", &prefixes)?
        .ok_or_else(|| OxmlError::MissingElement("w:font@w:name".to_owned()))?;
    let mut font = FontRecord::new(name);
    font.raw_attributes = raw_attributes(start, &[b"name"], &prefixes, &[])?;
    let mut boundary = 0usize;
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(child) => {
                let child_prefixes = word_prefixes_at(&child, &prefixes)?;
                let slot = font_child_slot(child.name().as_ref(), &child_prefixes);
                if let Some(slot) = slot {
                    require_order(boundary, slot, &child)?;
                    boundary = slot + 1;
                }
                let raw = capture_element(reader, &child)?;
                let Some(raw_inner) = explicit_element_trivia(&raw)? else {
                    font.retain_field_or_child(slot, boundary, raw, &relationship_prefixes);
                    buffer.clear();
                    continue;
                };
                if !apply_empty_font_child(
                    &mut font,
                    &child,
                    slot,
                    &child_prefixes,
                    &namespace_prefixes_at(&child, &relationship_prefixes, R_NS)?,
                    &namespace_prefixes_at(&child, &extension_prefixes, RDOCX_FONT_NS)?,
                    Some(raw_inner),
                )? {
                    font.retain_field_or_child(slot, boundary, raw, &relationship_prefixes);
                }
            }
            Event::Empty(child) => {
                let child_prefixes = word_prefixes_at(&child, &prefixes)?;
                let slot = font_child_slot(child.name().as_ref(), &child_prefixes);
                if let Some(slot) = slot {
                    require_order(boundary, slot, &child)?;
                    boundary = slot + 1;
                }
                if !apply_empty_font_child(
                    &mut font,
                    &child,
                    slot,
                    &child_prefixes,
                    &namespace_prefixes_at(&child, &relationship_prefixes, R_NS)?,
                    &namespace_prefixes_at(&child, &extension_prefixes, RDOCX_FONT_NS)?,
                    None,
                )? {
                    font.retain_field_or_child(
                        slot,
                        boundary,
                        capture_empty_element(&child)?,
                        &relationship_prefixes,
                    );
                }
            }
            event @ (Event::Text(_)
            | Event::CData(_)
            | Event::Comment(_)
            | Event::PI(_)
            | Event::Decl(_)
            | Event::DocType(_)
            | Event::GeneralRef(_)) => font.raw_children.push((boundary, capture_event(event)?)),
            Event::End(end) if is_word_element(end.name().as_ref(), b"font", &prefixes) => break,
            Event::Eof => return Err(OxmlError::MissingElement("w:font end".to_owned())),
            _ => {}
        }
        buffer.clear();
    }
    Ok(font)
}

fn explicit_element_trivia(raw: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut reader = Reader::from_reader(raw);
    let mut writer = Writer::new(Vec::new());
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(_) => break,
            Event::Eof => return Ok(None),
            _ => {}
        }
        buffer.clear();
    }
    loop {
        buffer.clear();
        match reader.read_event_into(&mut buffer)? {
            Event::End(_) => return Ok(Some(writer.into_inner())),
            Event::Text(text) if text.as_ref().iter().all(u8::is_ascii_whitespace) => {
                writer.write_event(Event::Text(text.into_owned()))?;
            }
            Event::Comment(comment) => {
                writer.write_event(Event::Comment(comment.into_owned()))?;
            }
            Event::PI(instruction) => {
                writer.write_event(Event::PI(instruction.into_owned()))?;
            }
            _ => return Ok(None),
        }
    }
}

fn apply_empty_font_child(
    font: &mut FontRecord,
    child: &BytesStart<'_>,
    slot: Option<usize>,
    word_prefixes: &[String],
    relationship_prefixes: &[String],
    extension_prefixes: &[String],
    raw_inner: Option<Vec<u8>>,
) -> Result<bool> {
    if matches!(slot, Some(0 | 3 | 5 | 7..=10)) {
        reject_conflicting_writer_prefixes(child)?;
    }
    match slot {
        Some(0) => {
            let Some(value) = word_attribute(child, b"val", word_prefixes)? else {
                return Ok(false);
            };
            font.alternate_name = Some(value);
            font.alternate_name_raw_attributes =
                raw_attributes(child, &[b"val"], word_prefixes, &[])?;
            font.alternate_name_raw_inner = raw_inner;
        }
        Some(3) => {
            let Some(value) = word_attribute(child, b"val", word_prefixes)? else {
                return Ok(false);
            };
            font.family = Some(value);
            font.family_raw_attributes = raw_attributes(child, &[b"val"], word_prefixes, &[])?;
            font.family_raw_inner = raw_inner;
        }
        Some(5) => {
            let Some(value) = word_attribute(child, b"val", word_prefixes)? else {
                return Ok(false);
            };
            font.pitch = Some(value);
            font.pitch_raw_attributes = raw_attributes(child, &[b"val"], word_prefixes, &[])?;
            font.pitch_raw_inner = raw_inner;
        }
        Some(7..=10) => {
            let mut embedded = read_embedded_reference(
                child,
                slot.expect("embedded slot"),
                word_prefixes,
                relationship_prefixes,
                extension_prefixes,
            )?;
            embedded.raw_inner = raw_inner;
            font.embedded_fonts.push(embedded);
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn read_embedded_reference(
    start: &BytesStart<'_>,
    slot: usize,
    word_prefixes: &[String],
    relationship_prefixes: &[String],
    extension_prefixes: &[String],
) -> Result<EmbeddedFontReference> {
    let relationship_id = qualified_attribute(start, b"id", relationship_prefixes)?
        .ok_or_else(|| OxmlError::MissingElement("embedded font r:id".to_owned()))?;
    let font_key = word_attribute(start, b"fontKey", word_prefixes)?
        .ok_or_else(|| OxmlError::MissingElement("embedded font w:fontKey".to_owned()))?;
    let subsetted = word_attribute(start, b"subsetted", word_prefixes)?
        .map(|value| parse_bool(&value))
        .transpose()?;
    let authorized = qualified_attribute(start, b"authorized", extension_prefixes)?
        .map(|value| parse_bool(&value))
        .transpose()?;
    let license_identity = qualified_attribute(start, b"license", extension_prefixes)?;
    let raw_attributes = raw_attributes(
        start,
        &[b"fontKey", b"subsetted"],
        word_prefixes,
        &[
            (b"id", relationship_prefixes),
            (b"authorized", extension_prefixes),
            (b"license", extension_prefixes),
        ],
    )?;
    Ok(EmbeddedFontReference {
        kind: match slot {
            7 => FontFaceKind::Regular,
            8 => FontFaceKind::Bold,
            9 => FontFaceKind::Italic,
            10 => FontFaceKind::BoldItalic,
            _ => unreachable!(),
        },
        relationship_id,
        font_key,
        subsetted,
        authorized,
        license_identity,
        raw_attributes,
        raw_inner: None,
    })
}

fn write_font(writer: &mut Writer<Vec<u8>>, font: &FontRecord) -> Result<()> {
    let mut start = BytesStart::new("w:font");
    push_attribute_escaped(&mut start, "w:name", &font.name);
    push_attributes(&mut start, &font.raw_attributes);
    writer.write_event(Event::Start(start))?;
    for slot in 0..FONT_CHILD_COUNT {
        emit_raw(writer, &font.raw_children, slot);
        match slot {
            0 => write_value(
                writer,
                "w:altName",
                font.alternate_name.as_deref(),
                &font.alternate_name_raw_attributes,
                font.alternate_name_raw_inner.as_deref(),
            )?,
            3 => write_value(
                writer,
                "w:family",
                font.family.as_deref(),
                &font.family_raw_attributes,
                font.family_raw_inner.as_deref(),
            )?,
            5 => write_value(
                writer,
                "w:pitch",
                font.pitch.as_deref(),
                &font.pitch_raw_attributes,
                font.pitch_raw_inner.as_deref(),
            )?,
            7..=10 => {
                if let Some(embedded) = font
                    .embedded_fonts
                    .iter()
                    .find(|embedded| embedded.kind.slot() == slot)
                {
                    let mut element = BytesStart::new(format!("w:{}", embedded.kind.local_name()));
                    push_attribute_escaped(&mut element, "r:id", &embedded.relationship_id);
                    push_attribute_escaped(&mut element, "w:fontKey", &embedded.font_key);
                    if let Some(subsetted) = embedded.subsetted {
                        element.push_attribute((
                            "w:subsetted",
                            if subsetted { "true" } else { "false" },
                        ));
                    }
                    if let Some(authorized) = embedded.authorized {
                        element.push_attribute((
                            "rdocx:authorized",
                            if authorized { "true" } else { "false" },
                        ));
                    }
                    if let Some(identity) = &embedded.license_identity {
                        push_attribute_escaped(&mut element, "rdocx:license", identity);
                    }
                    push_attributes(&mut element, &embedded.raw_attributes);
                    if let Some(raw_inner) = &embedded.raw_inner {
                        writer.write_event(Event::Start(element))?;
                        writer.get_mut().extend_from_slice(raw_inner);
                        writer.write_event(Event::End(BytesEnd::new(format!(
                            "w:{}",
                            embedded.kind.local_name()
                        ))))?;
                    } else {
                        writer.write_event(Event::Empty(element))?;
                    }
                }
            }
            _ => {}
        }
    }
    emit_raw(writer, &font.raw_children, FONT_CHILD_COUNT);
    writer.write_event(Event::End(BytesEnd::new("w:font")))?;
    Ok(())
}

fn write_value(
    writer: &mut Writer<Vec<u8>>,
    name: &str,
    value: Option<&str>,
    raw_attributes: &[(String, String)],
    raw_inner: Option<&[u8]>,
) -> Result<()> {
    if let Some(value) = value {
        let mut element = BytesStart::new(name);
        push_attribute_escaped(&mut element, "w:val", value);
        push_attributes(&mut element, raw_attributes);
        if let Some(raw_inner) = raw_inner {
            writer.write_event(Event::Start(element))?;
            writer.get_mut().extend_from_slice(raw_inner);
            writer.write_event(Event::End(BytesEnd::new(name)))?;
        } else {
            writer.write_event(Event::Empty(element))?;
        }
    }
    Ok(())
}

fn emit_raw(writer: &mut Writer<Vec<u8>>, raw: &[(usize, Vec<u8>)], position: usize) {
    for (_, bytes) in raw.iter().filter(|(at, _)| *at == position) {
        writer.get_mut().extend_from_slice(bytes);
    }
}

fn raw_attributes_mention(attributes: &[(String, String)], value: &str) -> bool {
    attributes.iter().any(|(_, candidate)| candidate == value)
}

fn raw_xml_mentions_value(raw: &[u8], value: &str) -> bool {
    let mut reader = Reader::from_reader(raw);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element) | Event::Empty(element)) => {
                for attribute in element.attributes() {
                    let Ok(attribute) = attribute else {
                        return true;
                    };
                    let Ok(candidate) = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                    else {
                        return true;
                    };
                    if candidate == value {
                        return true;
                    }
                }
            }
            Ok(Event::Eof) => return false,
            Err(_) => return true,
            _ => {}
        }
        buffer.clear();
    }
}

/// Whether retained XML names a relationship: an attribute in the
/// relationships namespace (`r:id`, `r:embed`...) at any depth, such as the
/// `r:id` of an embedded face inside `mc:AlternateContent`.
/// `relationship_prefixes` are the prefixes bound to that namespace around
/// it. XML that does not read back counts as naming one.
fn raw_xml_names_relationship(raw: &[u8], relationship_prefixes: &[String]) -> bool {
    let mut reader = Reader::from_reader(raw);
    let mut scopes = vec![relationship_prefixes.to_vec()];
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        let (element, opens) = match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) => (element, true),
            Ok(Event::Empty(element)) => (element, false),
            Ok(Event::End(_)) => {
                scopes.pop();
                continue;
            }
            Ok(Event::Eof) => return false,
            Err(_) => return true,
            Ok(_) => continue,
        };
        let inherited = scopes.last().map_or(&[][..], Vec::as_slice);
        let Ok(scope) = namespace_prefixes_at(&element, inherited, R_NS) else {
            return true;
        };
        for attribute in element.attributes() {
            let Ok(attribute) = attribute else {
                return true;
            };
            let key = attribute.key.as_ref();
            if let Some(separator) = key.iter().position(|byte| *byte == b':')
                && scope
                    .iter()
                    .any(|prefix| prefix.as_bytes() == &key[..separator])
            {
                return true;
            }
        }
        if opens {
            scopes.push(scope);
        }
    }
}

fn push_attributes(start: &mut BytesStart<'_>, attributes: &[(String, String)]) {
    for (name, value) in attributes {
        push_attribute_escaped(start, name, value);
    }
}

fn push_attribute_escaped(start: &mut BytesStart<'_>, name: &str, value: &str) {
    start.push_attribute((name, value));
}

fn reject_conflicting_writer_prefixes(start: &BytesStart<'_>) -> Result<()> {
    for attribute in start.attributes() {
        let attribute = attribute?;
        let expected = match attribute.key.as_ref() {
            b"xmlns:w" => Some(W_NS),
            b"xmlns:r" => Some(R_NS),
            b"xmlns:rdocx" => Some(RDOCX_FONT_NS),
            b"xmlns:mc" => Some(MC_NS),
            _ => None,
        };
        let Some(expected) = expected else {
            continue;
        };
        let value =
            attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())?;
        if value != expected {
            return Err(OxmlError::InvalidValue(format!(
                "{} conflicts with the fixed font-table writer namespace",
                String::from_utf8_lossy(attribute.key.as_ref())
            )));
        }
    }
    Ok(())
}

fn raw_attributes(
    start: &BytesStart<'_>,
    word_modelled: &[&[u8]],
    word_prefixes: &[String],
    qualified_modelled: &[(&[u8], &[String])],
) -> Result<Vec<(String, String)>> {
    let mut raw = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())?
            .into_owned();
        let canonical_namespace = (key == b"xmlns:w" && value == W_NS)
            || (key == b"xmlns:r" && value == R_NS)
            || (key == b"xmlns:rdocx" && value == RDOCX_FONT_NS)
            || (key == b"xmlns:mc" && value == MC_NS);
        let modelled = word_modelled
            .iter()
            .any(|local| is_word_attribute(key, local, word_prefixes))
            || qualified_modelled
                .iter()
                .any(|(local, prefixes)| is_qualified_name(key, local, prefixes));
        if !canonical_namespace && !modelled {
            raw.push((std::str::from_utf8(key)?.to_owned(), value));
        }
    }
    Ok(raw)
}

fn word_attribute(
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

fn qualified_attribute(
    start: &BytesStart<'_>,
    local: &[u8],
    prefixes: &[String],
) -> Result<Option<String>> {
    for attribute in start.attributes() {
        let attribute = attribute?;
        if is_qualified_name(attribute.key.as_ref(), local, prefixes) {
            return Ok(Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

fn is_qualified_name(name: &[u8], local: &[u8], prefixes: &[String]) -> bool {
    let Some(separator) = name.iter().position(|byte| *byte == b':') else {
        return false;
    };
    name.get(separator + 1..) == Some(local)
        && prefixes
            .iter()
            .any(|prefix| prefix.as_bytes() == &name[..separator])
}

fn namespace_prefixes_at(
    start: &BytesStart<'_>,
    inherited: &[String],
    namespace: &str,
) -> Result<Vec<String>> {
    let mut prefixes = inherited.to_vec();
    for attribute in start.attributes() {
        let attribute = attribute?;
        let name = attribute.key.as_ref();
        let Some(prefix) = name.strip_prefix(b"xmlns:") else {
            continue;
        };
        let prefix = std::str::from_utf8(prefix)?.to_owned();
        prefixes.retain(|candidate| candidate != &prefix);
        let value =
            attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())?;
        if value == namespace {
            prefixes.push(prefix);
        }
    }
    Ok(prefixes)
}

fn font_child_slot(name: &[u8], prefixes: &[String]) -> Option<usize> {
    let local = name.rsplit(|byte| *byte == b':').next().unwrap_or(name);
    if !is_word_element(name, local, prefixes) {
        return None;
    }
    match local {
        b"altName" => Some(0),
        b"panose1" => Some(1),
        b"charset" => Some(2),
        b"family" => Some(3),
        b"notTrueType" => Some(4),
        b"pitch" => Some(5),
        b"sig" => Some(6),
        b"embedRegular" => Some(7),
        b"embedBold" => Some(8),
        b"embedItalic" => Some(9),
        b"embedBoldItalic" => Some(10),
        _ => None,
    }
}

fn require_order(boundary: usize, slot: usize, element: &BytesStart<'_>) -> Result<()> {
    if slot < boundary {
        return Err(OxmlError::InvalidValue(format!(
            "font-table child {} is out of schema order",
            element_name(element)
        )));
    }
    Ok(())
}

fn parse_bool(value: &str) -> Result<bool> {
    crate::shared::ST_OnOff::parse(value)
        .ok_or_else(|| OxmlError::InvalidValue(format!("invalid OOXML boolean {value}")))
}

fn element_name(start: &BytesStart<'_>) -> String {
    String::from_utf8_lossy(start.name().as_ref()).into_owned()
}

/// Whether these attributes only declare namespaces.
fn only_namespace_declarations(attributes: &[(String, String)]) -> bool {
    attributes
        .iter()
        .all(|(key, _)| key == "xmlns" || key.starts_with("xmlns:"))
}

/// Whether kept element content holds more than whitespace.
fn holds_content(inner: &Option<Vec<u8>>) -> bool {
    inner
        .as_ref()
        .is_some_and(|raw| !raw.iter().all(u8::is_ascii_whitespace))
}
