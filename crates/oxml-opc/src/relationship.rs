//! Parsing and writing of `.rels` relationship files.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use quick_xml::encoding::Decoder;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;
use quick_xml::{Writer, XmlVersion};

use crate::error::{OpcError, Result};

const RELATIONSHIPS_NAMESPACE: &[u8] =
    b"http://schemas.openxmlformats.org/package/2006/relationships";

/// Well-known OOXML relationship types.
pub mod rel_types {
    // Package-level relationships.
    pub const CORE_PROPERTIES: &str =
        "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
    pub const THUMBNAIL: &str =
        "http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail";
    pub const DIGITAL_SIGNATURE_ORIGIN: &str =
        "http://schemas.openxmlformats.org/package/2006/relationships/digital-signature/origin";
    pub const DIGITAL_SIGNATURE: &str =
        "http://schemas.openxmlformats.org/package/2006/relationships/digital-signature/signature";

    // Shared officeDocument relationships.
    pub const DOCUMENT: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
    pub const EXTENDED_PROPERTIES: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties";
    pub const CUSTOM_PROPERTIES: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties";
    pub const STYLES: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";
    pub const NUMBERING: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering";
    pub const HEADER: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";
    pub const FOOTER: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer";
    pub const IMAGE: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
    pub const AUDIO: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/audio";
    pub const VIDEO: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/video";
    pub const POWERPOINT_MEDIA: &str =
        "http://schemas.microsoft.com/office/2007/relationships/media";
    pub const SETTINGS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings";
    pub const WEB_SETTINGS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/webSettings";
    pub const FONT_TABLE: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/fontTable";
    pub const FONT: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/font";
    pub const THEME: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme";
    pub const HYPERLINK: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";
    pub const FOOTNOTES: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes";
    pub const ENDNOTES: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/endnotes";
    pub const COMMENTS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments";
    pub const GLOSSARY_DOCUMENT: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/glossaryDocument";
    pub const CHART: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart";
    pub const DIAGRAM_DATA: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramData";
    pub const DIAGRAM_LAYOUT: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramLayout";
    pub const DIAGRAM_QUICK_STYLE: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramQuickStyle";
    pub const DIAGRAM_COLORS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramColors";
    pub const DIAGRAM_DRAWING: &str =
        "http://schemas.microsoft.com/office/2007/relationships/diagramDrawing";
    pub const PACKAGE: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/package";
    pub const OLE_OBJECT: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/oleObject";
    pub const CONTROL: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/control";
    pub const STRICT_OLE_OBJECT: &str =
        "http://purl.oclc.org/ooxml/officeDocument/relationships/oleObject";
    pub const STRICT_CONTROL: &str =
        "http://purl.oclc.org/ooxml/officeDocument/relationships/control";
    pub const ACTIVEX_CONTROL_BINARY: &str =
        "http://schemas.microsoft.com/office/2006/relationships/activeXControlBinary";
    pub const VBA_PROJECT: &str =
        "http://schemas.microsoft.com/office/2006/relationships/vbaProject";
    pub const VBA_PROJECT_SIGNATURE: &str =
        "http://schemas.microsoft.com/office/2006/relationships/vbaProjectSignature";
    pub const VBA_PROJECT_SIGNATURE_AGILE: &str =
        "http://schemas.microsoft.com/office/2006/relationships/vbaProjectSignatureAgile";

    // SpreadsheetML relationships.
    pub const WORKSHEET: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet";
    pub const SHARED_STRINGS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings";

    // PresentationML relationships.
    pub const SLIDE: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide";
    pub const SLIDE_LAYOUT: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout";
    pub const SLIDE_MASTER: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster";
    pub const NOTES_SLIDE: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide";
    pub const NOTES_MASTER: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesMaster";
    pub const PRES_PROPS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/presProps";
    pub const VIEW_PROPS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/viewProps";
    pub const TABLE_STYLES: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/tableStyles";
    pub const HANDOUT_MASTER: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/handoutMaster";
    pub const POWERPOINT_COMMENTS: &str =
        "http://schemas.microsoft.com/office/2018/10/relationships/comments";
    pub const POWERPOINT_AUTHORS: &str =
        "http://schemas.microsoft.com/office/2018/10/relationships/authors";
}

/// A single relationship entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Relationship {
    pub id: String,
    pub rel_type: String,
    pub target: String,
    pub target_mode: Option<String>,
}

/// A collection of relationships parsed from a `.rels` file.
#[derive(Debug, Clone, Default)]
pub struct Relationships {
    pub items: Vec<Relationship>,
    next_id: u32,
    // The bytes read and the items they gave, shared between clones.
    source: Option<Arc<(Vec<u8>, Vec<Relationship>)>>,
}

impl Relationships {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            next_id: 1,
            source: None,
        }
    }

    /// Parse from XML bytes.
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        let mut reader = NsReader::from_reader(xml);
        reader.config_mut().trim_text(true);

        let mut items = Vec::new();
        let mut ids = HashSet::new();
        let mut max_id: u32 = 0;
        let mut buf = Vec::new();
        let mut depth = 0usize;
        let mut owned_root = false;

        loop {
            let decoder = reader.decoder();
            match reader.read_resolved_event_into(&mut buf) {
                Ok((namespace, Event::Empty(ref e)))
                    if owned_root
                        && depth == 1
                        && matches!(namespace, ResolveResult::Bound(value) if namespace_uri(value.as_ref(), decoder)? == std::str::from_utf8(RELATIONSHIPS_NAMESPACE).expect("static namespace"))
                        && e.local_name().as_ref() == b"Relationship" =>
                {
                    let relationship = parse_relationship(e, reader.decoder())?;
                    if !ids.insert(relationship.id.clone()) {
                        return Err(OpcError::InvalidRelationship);
                    }
                    update_max_relationship_id(&relationship.id, &mut max_id);
                    items.push(relationship);
                }
                Ok((namespace, Event::Start(ref e)))
                    if owned_root
                        && depth == 1
                        && matches!(namespace, ResolveResult::Bound(value) if namespace_uri(value.as_ref(), decoder)? == std::str::from_utf8(RELATIONSHIPS_NAMESPACE).expect("static namespace"))
                        && e.local_name().as_ref() == b"Relationship" =>
                {
                    let relationship = parse_relationship(e, reader.decoder())?;
                    if !ids.insert(relationship.id.clone()) {
                        return Err(OpcError::InvalidRelationship);
                    }
                    update_max_relationship_id(&relationship.id, &mut max_id);
                    items.push(relationship);
                    reader.read_to_end_into(e.name(), &mut Vec::new())?;
                }
                Ok((namespace, Event::Start(ref start))) => {
                    if depth == 0 {
                        owned_root = matches!(namespace, ResolveResult::Bound(value) if namespace_uri(value.as_ref(), decoder)? == std::str::from_utf8(RELATIONSHIPS_NAMESPACE).expect("static namespace"))
                            && start.local_name().as_ref() == b"Relationships";
                    }
                    depth += 1;
                }
                Ok((_, Event::End(_))) => {
                    depth = depth.checked_sub(1).ok_or(OpcError::InvalidRelationship)?
                }
                Ok((_, Event::Eof)) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        if depth != 0 {
            return Err(OpcError::InvalidRelationship);
        }
        Ok(Relationships {
            source: Some(Arc::new((xml.to_vec(), items.clone()))),
            items,
            next_id: next_relationship_number(max_id),
        })
    }

    /// Rebinds one imported scope, preserving opaque source around owned records.
    /// ID changes are explicit, and absent proposed records are removed atomically.
    pub fn rebind_for_import(
        &self,
        items: Vec<Relationship>,
        ids: &HashMap<String, String>,
    ) -> Result<Self> {
        self.validate_ids()?;
        let mut proposed = Self::new();
        proposed.items = items;
        proposed.validate_ids()?;
        for item in &proposed.items {
            if item.rel_type.is_empty()
                || item.target.is_empty()
                || item.rel_type.chars().any(char::is_control)
                || item.target.chars().any(char::is_control)
                || item
                    .target_mode
                    .as_deref()
                    .is_some_and(|mode| !matches!(mode, "External" | "Internal"))
            {
                return Err(OpcError::InvalidRelationship);
            }
        }
        let source = match &self.source {
            Some(source) => source.0.clone(),
            None => self.to_xml()?,
        };
        let parsed = relationship_xml(&source)?;
        let proposed_by_id = proposed
            .items
            .iter()
            .map(|item| (item.id.as_str(), item))
            .collect::<HashMap<_, _>>();
        let mut edits = Vec::new();
        let mut seen = HashSet::new();
        let mut old = HashSet::new();
        for record in &parsed.records {
            let id = record
                .attributes
                .iter()
                .find(|attribute| attribute.name == b"Id")
                .ok_or(OpcError::InvalidRelationship)?;
            if !old.insert(id.value.as_str()) {
                return Err(OpcError::InvalidRelationship);
            }
            let mapped = ids.get(&id.value).map(String::as_str).unwrap_or(&id.value);
            let Some(item) = proposed_by_id.get(mapped) else {
                edits.push((record.range.clone(), Vec::new()));
                continue;
            };
            if !seen.insert(item.id.as_str()) {
                return Err(OpcError::InvalidRelationship);
            }
            for (name, value) in [
                (b"Id".as_slice(), Some(item.id.as_str())),
                (b"Type", Some(item.rel_type.as_str())),
                (b"Target", Some(item.target.as_str())),
                (b"TargetMode", item.target_mode.as_deref()),
            ] {
                match (
                    record
                        .attributes
                        .iter()
                        .find(|attribute| attribute.name == name),
                    value,
                ) {
                    (Some(attribute), Some(value)) if attribute.value != value => edits.push((
                        attribute.value_range.clone(),
                        quick_xml::escape::escape(value).into_owned().into_bytes(),
                    )),
                    (Some(attribute), None) => {
                        edits.push((attribute.full_range.clone(), Vec::new()))
                    }
                    (None, Some(value)) => {
                        let at = record.head_end - if record.empty { 2 } else { 1 };
                        edits.push((
                            at..at,
                            format!(
                                " {}=\"{}\"",
                                std::str::from_utf8(name)
                                    .map_err(|_| OpcError::InvalidRelationship)?,
                                quick_xml::escape::escape(value)
                            )
                            .into_bytes(),
                        ));
                    }
                    _ => {}
                }
            }
        }
        let current_ids = self
            .items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<HashSet<_>>();
        if ids
            .keys()
            .any(|id| !current_ids.contains(id.as_str()) && !old.contains(id.as_str()))
        {
            return Err(OpcError::InvalidRelationship);
        }
        // A changed public ID does not identify its historical opaque row.
        // An explicit retained-ID map proves that ownership. Otherwise refuse
        // a missing raw owner combined with newly emitted, unowned rows.
        let unclaimed_raw_owner = old
            .iter()
            .any(|id| !current_ids.contains(*id) && !ids.contains_key(*id));
        if unclaimed_raw_owner
            && proposed
                .items
                .iter()
                .any(|item| !seen.contains(item.id.as_str()))
        {
            return Err(OpcError::InvalidRelationship);
        }
        let prefix = parsed
            .name
            .rsplit_once(':')
            .map(|(prefix, _)| format!("{prefix}:"))
            .unwrap_or_default();
        let mut added = Vec::new();
        for item in &proposed.items {
            if !seen.contains(item.id.as_str()) {
                let mut start = BytesStart::new(format!("{prefix}Relationship"));
                start.push_attribute(("Id", item.id.as_str()));
                start.push_attribute(("Type", item.rel_type.as_str()));
                start.push_attribute(("Target", item.target.as_str()));
                if let Some(mode) = &item.target_mode {
                    start.push_attribute(("TargetMode", mode.as_str()));
                }
                let mut writer = Writer::new(Vec::new());
                writer.write_event(Event::Empty(start))?;
                added.extend(writer.into_inner());
            }
        }
        if parsed.empty {
            let replacement = [
                b">".as_slice(),
                &added,
                format!("</{}>", parsed.name).as_bytes(),
            ]
            .concat();
            edits.push((parsed.head_end - 2..parsed.head_end, replacement));
        } else if !added.is_empty() {
            edits.push((parsed.close_start..parsed.close_start, added));
        }
        let xml = apply_relationship_edits(&source, edits)?;
        let result = Self::from_xml(&xml)?;
        if result.items != proposed.items {
            return Err(OpcError::InvalidRelationship);
        }
        Ok(result)
    }

    /// Merges an imported scope without rebuilding its opaque XML.
    /// Conflicting root metadata is refused before either scope changes.
    pub fn merge_for_import(&self, other: &Self) -> Result<(Self, HashMap<String, String>)> {
        let left_xml = self
            .rebind_for_import(self.items.clone(), &HashMap::new())?
            .to_xml()?;
        let left = relationship_xml(&left_xml)?;
        let right_xml = other
            .rebind_for_import(other.items.clone(), &HashMap::new())?
            .to_xml()?;
        let right = relationship_xml(&right_xml)?;
        let mut root_attributes = Vec::new();
        for attribute in &right.attributes {
            if let Some(existing) = left.attributes.iter().find(|item| {
                if attribute.name == b"xmlns" || attribute.name.starts_with(b"xmlns:") {
                    item.name == attribute.name
                } else {
                    item.expanded_name == attribute.expanded_name
                }
            }) {
                if existing.value != attribute.value {
                    return Err(OpcError::InvalidRelationship);
                }
            } else {
                root_attributes.extend_from_slice(&right_xml[attribute.full_range.clone()]);
            }
        }
        let mut occupied = self.clone();
        let mut ids = HashMap::new();
        let mut imported = other.items.clone();
        for item in &mut imported {
            let old = item.id.clone();
            if occupied.get_by_id(&item.id).is_some() {
                item.id = occupied.allocate_id();
            }
            ids.insert(old, item.id.clone());
            occupied.items.push(item.clone());
        }
        let rebound = other.rebind_for_import(imported, &ids)?.to_xml()?;
        let rebound_root = relationship_xml(&rebound)?;
        let interior = if rebound_root.empty {
            Vec::new()
        } else {
            let mut inner = rebound[rebound_root.head_end..rebound_root.close_start].to_vec();
            let mut edits = Vec::new();
            for child in &rebound_root.children {
                let mut reader = quick_xml::Reader::from_reader(&rebound[child.clone()]);
                let start = match reader.read_event()? {
                    Event::Start(start) | Event::Empty(start) => start,
                    _ => return Err(OpcError::InvalidRelationship),
                };
                let empty = rebound[child.start + reader.buffer_position() as usize - 2] == b'/';
                let declared = start
                    .attributes()
                    .map(|attribute| attribute.map(|attribute| attribute.key.as_ref().to_vec()))
                    .collect::<std::result::Result<HashSet<_>, _>>()?;
                let mut closure = Vec::new();
                for attribute in rebound_root.attributes.iter().filter(|attribute| {
                    attribute.name == b"xmlns" || attribute.name.starts_with(b"xmlns:")
                }) {
                    if !declared.contains(&attribute.name) {
                        closure.extend_from_slice(&rebound[attribute.full_range.clone()]);
                    }
                }
                if !declared.contains(b"xmlns".as_slice())
                    && !rebound_root
                        .attributes
                        .iter()
                        .any(|attribute| attribute.name == b"xmlns")
                {
                    closure.extend_from_slice(b" xmlns=\"\"");
                }
                let at = child.start + reader.buffer_position() as usize
                    - if empty { 2 } else { 1 }
                    - rebound_root.head_end;
                edits.push((at..at, closure));
            }
            inner = apply_relationship_edits(&inner, edits)?;
            inner
        };
        let mut edits = Vec::new();
        if left.empty {
            let replacement = [
                root_attributes.as_slice(),
                b">",
                &interior,
                format!("</{}>", left.name).as_bytes(),
            ]
            .concat();
            edits.push((left.head_end - 2..left.head_end, replacement));
        } else {
            edits.push((left.head_end - 1..left.head_end - 1, root_attributes));
            edits.push((left.close_start..left.close_start, interior));
        }
        let xml = apply_relationship_edits(&left_xml, edits)?;
        let result = Self::from_xml(&xml)?;
        if result.items != occupied.items {
            return Err(OpcError::InvalidRelationship);
        }
        Ok((result, ids))
    }

    /// Serialize to XML bytes.
    pub fn to_xml(&self) -> Result<Vec<u8>> {
        self.validate_ids()?;
        if let Some(source) = &self.source
            && source.1 == self.items
        {
            return Ok(source.0.clone());
        }
        let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);

        writer.write_event(Event::Decl(BytesDecl::new(
            "1.0",
            Some("UTF-8"),
            Some("yes"),
        )))?;

        let mut rels_start = BytesStart::new("Relationships");
        rels_start.push_attribute((
            "xmlns",
            "http://schemas.openxmlformats.org/package/2006/relationships",
        ));
        writer.write_event(Event::Start(rels_start))?;

        for rel in &self.items {
            let mut elem = BytesStart::new("Relationship");
            elem.push_attribute(("Id", rel.id.as_str()));
            elem.push_attribute(("Type", rel.rel_type.as_str()));
            elem.push_attribute(("Target", rel.target.as_str()));
            if let Some(ref mode) = rel.target_mode {
                elem.push_attribute(("TargetMode", mode.as_str()));
            }
            writer.write_event(Event::Empty(elem))?;
        }

        writer.write_event(Event::End(BytesEnd::new("Relationships")))?;

        Ok(writer.into_inner())
    }

    /// Compare retained metadata of two qualified records after assigned ids and
    /// targets have been matched by the caller. Opaque values retain the full
    /// decoded namespace context, because their QName semantics are unmodelled.
    pub fn record_metadata_equal_for_import(
        &self,
        id: &str,
        other: &Self,
        other_id: &str,
    ) -> Result<bool> {
        let left = self
            .rebind_for_import(self.items.clone(), &HashMap::new())?
            .to_xml()?;
        let right = other
            .rebind_for_import(other.items.clone(), &HashMap::new())?
            .to_xml()?;
        let a = relationship_xml(&left)?;
        let b = relationship_xml(&right)?;
        let record = |root: &RelationshipXmlRoot, id: &str| {
            root.records.iter().position(|record| {
                record.attributes.iter().any(|attribute| {
                    attribute.expanded_name.0.is_none()
                        && attribute.name == b"Id"
                        && attribute.value == id
                })
            })
        };
        let (Some(ai), Some(bi)) = (record(&a, id), record(&b, other_id)) else {
            return Err(OpcError::InvalidRelationship);
        };
        let a_record = &a.records[ai];
        let b_record = &b.records[bi];
        let metadata = |record: &RelationshipXmlRecord| {
            record
                .attributes
                .iter()
                .enumerate()
                .filter(|(_, attribute)| {
                    attribute.name != b"xmlns"
                        && !attribute.name.starts_with(b"xmlns:")
                        && (attribute.expanded_name.0.is_some()
                            || !matches!(
                                attribute.name.as_slice(),
                                b"Id" | b"Type" | b"Target" | b"TargetMode"
                            ))
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>()
        };
        let a_attributes = metadata(a_record);
        let b_attributes = metadata(b_record);
        let content = |xml: &[u8], record: &RelationshipXmlRecord| -> Vec<u8> {
            if record.empty {
                Vec::new()
            } else {
                let interior = &xml[record.head_end..record.range.end];
                interior[..interior.iter().rposition(|byte| *byte == b'<').unwrap_or(0)].to_vec()
            }
        };
        if a_attributes.is_empty()
            && b_attributes.is_empty()
            && content(&left, a_record).is_empty()
            && content(&right, b_record).is_empty()
        {
            return Ok(true);
        }
        // Namespace closure can add redundant declarations to a copied record.
        // Compare its complete effective authority, not declaration placement.
        let namespaces = |root: &RelationshipXmlRoot, record: &RelationshipXmlRecord| {
            root.attributes
                .iter()
                .chain(&record.attributes)
                .filter(|attribute| {
                    attribute.name == b"xmlns" || attribute.name.starts_with(b"xmlns:")
                })
                .map(|attribute| (attribute.name.clone(), attribute.value.clone()))
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        if namespaces(&a, a_record) != namespaces(&b, b_record)
            || a_attributes.len() != b_attributes.len()
        {
            return Ok(false);
        }
        for (left_index, right_index) in a_attributes.iter().zip(b_attributes) {
            let left_attribute = &a_record.attributes[*left_index];
            let right_attribute = &b_record.attributes[right_index];
            if left_attribute.expanded_name != right_attribute.expanded_name
                || left[left_attribute.full_range.clone()]
                    != right[right_attribute.full_range.clone()]
            {
                return Ok(false);
            }
        }
        Ok(content(&left, a_record) == content(&right, b_record))
    }

    /// Find a relationship by its ID.
    pub fn get_by_id(&self, id: &str) -> Option<&Relationship> {
        self.items.iter().find(|r| r.id == id)
    }

    /// Find the first relationship matching a given type.
    pub fn get_by_type(&self, rel_type: &str) -> Option<&Relationship> {
        self.items.iter().find(|r| r.rel_type == rel_type)
    }

    /// Find all relationships matching a given type.
    pub fn get_all_by_type(&self, rel_type: &str) -> Vec<&Relationship> {
        self.items
            .iter()
            .filter(|r| r.rel_type == rel_type)
            .collect()
    }

    /// Add a new relationship and return its generated ID.
    pub fn add(&mut self, rel_type: &str, target: &str) -> String {
        let id = self.allocate_id();
        self.items.push(Relationship {
            id: id.clone(),
            rel_type: rel_type.to_string(),
            target: target.to_string(),
            target_mode: None,
        });
        id
    }

    /// Add an externally-targeted relationship (e.g. a hyperlink URL) and
    /// return its generated ID.
    pub fn add_external(&mut self, rel_type: &str, target: &str) -> String {
        let id = self.allocate_id();
        self.items.push(Relationship {
            id: id.clone(),
            rel_type: rel_type.to_string(),
            target: target.to_string(),
            target_mode: Some("External".to_string()),
        });
        id
    }

    /// Add a relationship with a specific ID.
    ///
    /// If a relationship with this ID already exists, it is replaced.
    /// The `next_id` counter is updated to avoid future collisions.
    pub fn add_with_id(&mut self, id: &str, rel_type: &str, target: &str) {
        self.items.retain(|r| r.id != id);
        self.items.push(Relationship {
            id: id.to_string(),
            rel_type: rel_type.to_string(),
            target: target.to_string(),
            target_mode: None,
        });
        if let Some(num) = id.strip_prefix("rId").and_then(|s| s.parse::<u32>().ok())
            && num >= self.next_id
        {
            self.next_id = next_relationship_number(num);
        }
    }

    fn allocate_id(&mut self) -> String {
        let mut candidate = self.next_id;
        let numeric_space = usize::try_from(u32::MAX).unwrap_or(usize::MAX);
        let attempts = self.items.len().saturating_add(1).min(numeric_space);
        for _ in 0..attempts {
            let id = format!("rId{candidate}");
            let next = next_relationship_number(candidate);
            if self.items.iter().all(|relationship| relationship.id != id) {
                self.next_id = next;
                return id;
            }
            candidate = next;
        }

        for ordinal in 1u128.. {
            let id = format!("rIdGenerated{ordinal}");
            if self.items.iter().all(|relationship| relationship.id != id) {
                return id;
            }
        }
        unreachable!("the generated relationship id space is unbounded")
    }

    /// Whether two relationship sets hold the same items, identifier counter
    /// and source.
    pub(crate) fn same_as(&self, other: &Relationships) -> bool {
        self.items == other.items && self.next_id == other.next_id && self.source == other.source
    }

    /// Whether these relationships still hold the items their source bytes
    /// were read into, with the identifier counter reading them sets, so
    /// that writing and reading them back gives them unchanged.
    pub(crate) fn is_as_read(&self) -> bool {
        let Some(source) = &self.source else {
            return false;
        };
        let mut maximum = 0;
        for item in &self.items {
            update_max_relationship_id(&item.id, &mut maximum);
        }
        source.1 == self.items && self.next_id == next_relationship_number(maximum)
    }

    pub(crate) fn validate_ids(&self) -> Result<()> {
        let mut ids = HashSet::with_capacity(self.items.len());
        if self
            .items
            .iter()
            .all(|relationship| ids.insert(relationship.id.as_str()))
        {
            Ok(())
        } else {
            Err(OpcError::InvalidRelationship)
        }
    }
}

struct RelationshipXmlAttribute {
    name: Vec<u8>,
    expanded_name: (Option<String>, Vec<u8>),
    value: String,
    value_range: std::ops::Range<usize>,
    full_range: std::ops::Range<usize>,
}

struct RelationshipXmlRecord {
    range: std::ops::Range<usize>,
    head_end: usize,
    empty: bool,
    attributes: Vec<RelationshipXmlAttribute>,
}

struct RelationshipXmlRoot {
    name: String,
    head_end: usize,
    close_start: usize,
    empty: bool,
    attributes: Vec<RelationshipXmlAttribute>,
    records: Vec<RelationshipXmlRecord>,
    children: Vec<std::ops::Range<usize>>,
}

fn namespace_uri(bytes: &[u8], decoder: Decoder) -> Result<String> {
    // NamespaceResolver retains the declaration's lexical attribute value.
    // Namespace identity uses XML attribute normalization, including references.
    let declaration = quick_xml::events::attributes::Attribute {
        key: quick_xml::name::QName(b"xmlns"),
        value: std::borrow::Cow::Borrowed(bytes),
    };
    Ok(declaration
        .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)?
        .into_owned())
}

fn relationship_xml(xml: &[u8]) -> Result<RelationshipXmlRoot> {
    // quick-xml consumes its initial BOM without counting it in buffer_position.
    // Parse the remaining slice explicitly and translate every splice offset.
    let origin = if xml.starts_with(b"\xef\xbb\xbf") {
        3
    } else {
        0
    };
    let mut reader = NsReader::from_reader(&xml[origin..]);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut root = None;
    let mut record = None;
    let mut child = None;
    loop {
        let begin = origin + reader.buffer_position() as usize;
        let decoder = reader.decoder();
        let (namespace, event) = reader.read_resolved_event_into(&mut buffer)?;
        let qualified = matches!(namespace, ResolveResult::Bound(uri) if namespace_uri(uri.as_ref(), decoder)? == std::str::from_utf8(RELATIONSHIPS_NAMESPACE).expect("static namespace"));
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(start) | Event::Empty(start) => {
                let end = origin + reader.buffer_position() as usize;
                let mut attributes = Vec::new();
                let mut expanded_attributes = HashSet::new();
                let raw_start: &[u8] = start.as_ref();
                for attribute in start.attributes() {
                    let attribute = attribute?;
                    let name_offset = (attribute.key.as_ref().as_ptr() as usize)
                        .checked_sub(raw_start.as_ptr() as usize)
                        .ok_or(OpcError::InvalidRelationship)?;
                    let value_offset = (attribute.value.as_ptr() as usize)
                        .checked_sub(raw_start.as_ptr() as usize)
                        .ok_or(OpcError::InvalidRelationship)?;
                    let value_start = begin + 1 + value_offset;
                    let value_end = value_start + attribute.value.len();
                    if value_end >= xml.len() {
                        return Err(OpcError::InvalidRelationship);
                    }
                    let mut full_start = begin + 1 + name_offset;
                    while full_start > begin && xml[full_start - 1].is_ascii_whitespace() {
                        full_start -= 1;
                    }
                    let declaration = attribute.key.as_ref() == b"xmlns"
                        || attribute.key.as_ref().starts_with(b"xmlns:");
                    let expanded_name = if declaration {
                        (
                            Some("http://www.w3.org/2000/xmlns/".to_owned()),
                            attribute.key.as_ref().to_vec(),
                        )
                    } else {
                        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                        let namespace = match namespace {
                            ResolveResult::Bound(uri) => {
                                Some(namespace_uri(uri.as_ref(), decoder)?)
                            }
                            ResolveResult::Unbound => None,
                            ResolveResult::Unknown(_) => return Err(OpcError::InvalidRelationship),
                        };
                        (namespace, local.as_ref().to_vec())
                    };
                    if !expanded_attributes.insert(expanded_name.clone()) {
                        return Err(OpcError::InvalidRelationship);
                    }
                    attributes.push(RelationshipXmlAttribute {
                        name: attribute.key.as_ref().to_vec(),
                        expanded_name,
                        value: attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())?
                            .into_owned(),
                        value_range: value_start..value_end,
                        full_range: full_start..value_end + 1,
                    });
                }
                if depth == 0 {
                    if root.is_some()
                        || !qualified
                        || start.local_name().as_ref() != b"Relationships"
                    {
                        return Err(OpcError::InvalidRelationship);
                    }
                    root = Some(RelationshipXmlRoot {
                        name: std::str::from_utf8(start.name().as_ref())
                            .map_err(|_| OpcError::InvalidRelationship)?
                            .to_owned(),
                        head_end: end,
                        close_start: end,
                        empty,
                        attributes,
                        records: Vec::new(),
                        children: Vec::new(),
                    });
                } else if depth == 1 {
                    child = Some(begin);
                    if qualified && start.local_name().as_ref() == b"Relationship" {
                        let current = RelationshipXmlRecord {
                            range: begin..end,
                            head_end: end,
                            empty,
                            attributes,
                        };
                        if empty {
                            root.as_mut()
                                .ok_or(OpcError::InvalidRelationship)?
                                .records
                                .push(current);
                        } else {
                            record = Some(current);
                        }
                    }
                    if empty {
                        root.as_mut()
                            .ok_or(OpcError::InvalidRelationship)?
                            .children
                            .push(begin..end);
                        child = None;
                    }
                }
                if !empty {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or(OpcError::InvalidRelationship)?;
                let current = root.as_mut().ok_or(OpcError::InvalidRelationship)?;
                if depth == 1 {
                    if let Some(mut record) = record.take() {
                        record.range.end = origin + reader.buffer_position() as usize;
                        current.records.push(record);
                    }
                    if let Some(begin) = child.take() {
                        current
                            .children
                            .push(begin..origin + reader.buffer_position() as usize);
                    }
                } else if depth == 0 {
                    current.close_start = begin;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if depth != 0 || record.is_some() || child.is_some() {
        return Err(OpcError::InvalidRelationship);
    }
    root.ok_or(OpcError::InvalidRelationship)
}

fn apply_relationship_edits(
    xml: &[u8],
    mut edits: Vec<(std::ops::Range<usize>, Vec<u8>)>,
) -> Result<Vec<u8>> {
    edits.sort_by_key(|(range, _)| range.start);
    let mut end = 0;
    for (range, _) in &edits {
        if range.start < end || range.start > range.end || range.end > xml.len() {
            return Err(OpcError::InvalidRelationship);
        }
        end = range.end;
    }
    let mut result = xml.to_vec();
    for (range, bytes) in edits.into_iter().rev() {
        result.splice(range, bytes);
    }
    Ok(result)
}

fn parse_relationship(element: &BytesStart<'_>, decoder: Decoder) -> Result<Relationship> {
    let mut id = None;
    let mut rel_type = None;
    let mut target = None;
    let mut target_mode = None;
    for attribute in element.attributes() {
        let attribute = attribute?;
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)?
            .into_owned();
        match attribute.key.as_ref() {
            b"Id" => id = Some(value),
            b"Type" => rel_type = Some(value),
            b"Target" => target = Some(value),
            b"TargetMode" => target_mode = Some(value),
            _ => {}
        }
    }
    match (id, rel_type, target) {
        (Some(id), Some(rel_type), Some(target)) => Ok(Relationship {
            id,
            rel_type,
            target,
            target_mode,
        }),
        _ => Err(OpcError::InvalidRelationship),
    }
}

fn update_max_relationship_id(id: &str, maximum: &mut u32) {
    if let Some(number) = id.strip_prefix("rId").and_then(|value| value.parse().ok()) {
        *maximum = (*maximum).max(number);
    }
}

fn next_relationship_number(current: u32) -> u32 {
    current.checked_add(1).unwrap_or(1).max(1)
}
