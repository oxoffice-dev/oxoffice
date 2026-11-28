//! Comment snapshots: the comments and their related parts a comparison keeps.

use super::*;

pub(crate) const COMMENT_COMPARISON_PART: &str = "/customXml/rdocxComparisonComments.xml";
pub(crate) const COMMENT_COMPARISON_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml";
pub(crate) const COMMENT_COMPARISON_OPEN: &str =
    "<rdocx:comparisonComments xmlns:rdocx=\"urn:rdocx:comparison:comments:1\">";
pub(crate) const COMMENT_COMPARISON_CLOSE: &str = "</rdocx:comparisonComments>";

/// `document` without the comparison's private record of its comments.
pub(super) fn remove_comment_record(document: &mut Document) {
    document.package.remove_part(COMMENT_COMPARISON_PART);
    document
        .package
        .content_types
        .remove_override(COMMENT_COMPARISON_PART);
    let owner = document.doc_part_name.clone();
    if let Some(relationships) = document.package.get_part_rels_mut(&owner) {
        relationships.items.retain(|relationship| {
            relationship.rel_type != COMMENT_COMPARISON_REL
                || OpcPackage::resolve_rel_target(&owner, &relationship.target)
                    != COMMENT_COMPARISON_PART
        });
    }
}

/// A part a comment owns, which a comparison carries with the comments: the
/// durable ids and UTC dates go with the comments they name.
fn comment_relationship(relationship: &Relationship) -> bool {
    matches!(
        relationship.rel_type.as_str(),
        rel_types::COMMENTS
            | crate::comments::COMMENTS_EXTENDED_REL_TYPE
            | crate::comments::COMMENTS_IDS_REL_TYPE
            | crate::comments::COMMENTS_EXTENSIBLE_REL_TYPE
    )
}

fn related_part_snapshot(
    document: &Document,
    name: &str,
    seen: &mut HashSet<String>,
) -> Result<serde_json::Value> {
    if !seen.insert(name.to_owned()) {
        return Ok(serde_json::Value::Null);
    }
    let xml = document
        .package
        .get_part(name)
        .ok_or_else(|| Error::Other(format!("missing comment relationship target {name}")))?;
    let relationships = document.package.get_part_rels(name);
    let mut children = Vec::new();
    if let Some(relationships) = relationships {
        for relationship in &relationships.items {
            if crate::document::relationship_is_internal(relationship) {
                let target = OpcPackage::resolve_rel_target(name, &relationship.target);
                let child = related_part_snapshot(document, &target, seen)?;
                if !child.is_null() {
                    children.push(child);
                }
            }
        }
    }
    let rels = relationships
        .map(|rels| rels.to_xml())
        .transpose()
        .map_err(|error| Error::Other(error.to_string()))?;
    Ok(serde_json::json!({
        "name": name,
        "content_type": document.package.content_types.content_type_for(name),
        "xml": base64::engine::general_purpose::STANDARD.encode(xml),
        "rels": rels.map(|raw| base64::engine::general_purpose::STANDARD.encode(raw)),
        "related": children,
    }))
}

pub(super) fn comment_snapshot(document: &Document) -> Result<serde_json::Value> {
    let mut parts = Vec::new();
    let mut seen = HashSet::new();
    if let Some(relationships) = document.package.get_part_rels(&document.doc_part_name) {
        for relationship in &relationships.items {
            if !comment_relationship(relationship)
                || !crate::document::relationship_is_internal(relationship)
            {
                continue;
            }
            let name =
                OpcPackage::resolve_rel_target(&document.doc_part_name, &relationship.target);
            let mut part = related_part_snapshot(document, &name, &mut seen)?;
            let object = part
                .as_object_mut()
                .ok_or_else(|| Error::Other("duplicate comment comparison part".to_owned()))?;
            object.extend(
                serde_json::json!({
                    "id": relationship.id,
                    "type": relationship.rel_type,
                    "target": relationship.target,
                })
                .as_object()
                .cloned()
                .unwrap_or_default(),
            );
            parts.push(part);
        }
    }
    Ok(serde_json::Value::Array(parts))
}

fn snapshot_part_index<'a>(
    part: &'a serde_json::Value,
    index: &mut HashMap<String, &'a serde_json::Value>,
) -> Result<()> {
    let name = part
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Other("comparison comment part has no name".to_owned()))?;
    index.insert(name.to_owned(), part);
    for child in part
        .get("related")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| Error::Other("comparison comment part has no related list".to_owned()))?
    {
        snapshot_part_index(child, index)?;
    }
    Ok(())
}

fn snapshot_part_signature(
    name: &str,
    index: &HashMap<String, &serde_json::Value>,
    visited: &mut HashSet<String>,
) -> Result<String> {
    if !visited.insert(name.to_owned()) {
        return Ok("shared".to_owned());
    }
    let part = index
        .get(name)
        .ok_or_else(|| Error::Other(format!("comparison comment target {name} is absent")))?;
    let xml = part
        .get("xml")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let content_type = part.get("content_type").and_then(serde_json::Value::as_str);
    let mut rel_signatures = Vec::new();
    if let Some(raw) = part.get("rels").and_then(serde_json::Value::as_str) {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(raw)
            .map_err(|error| Error::Other(error.to_string()))?;
        let rels = oxoffice_opc::relationship::Relationships::from_xml(&decoded)
            .map_err(|error| Error::Other(error.to_string()))?;
        for relationship in rels.items {
            let target = if crate::document::relationship_is_internal(&relationship) {
                let child = OpcPackage::resolve_rel_target(name, &relationship.target);
                snapshot_part_signature(&child, index, visited)?
            } else {
                relationship.target.clone()
            };
            rel_signatures.push(format!(
                "{:?}:{:?}:{:?}:{target}",
                relationship.id, relationship.rel_type, relationship.target_mode
            ));
        }
    }
    rel_signatures.sort();
    visited.remove(name);
    Ok(format!("{content_type:?}:{xml}:{rel_signatures:?}"))
}

pub(super) fn comment_snapshots_match(
    left: &serde_json::Value,
    right: &serde_json::Value,
) -> Result<bool> {
    let signature = |value: &serde_json::Value| -> Result<Vec<String>> {
        let roots = value
            .as_array()
            .ok_or_else(|| Error::Other("invalid comparison comment snapshot".to_owned()))?;
        let mut index = HashMap::new();
        for root in roots {
            snapshot_part_index(root, &mut index)?;
        }
        let mut signatures = Vec::new();
        for root in roots {
            let name = root
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| Error::Other("comment root has no name".to_owned()))?;
            signatures.push(format!(
                "{name}:{}",
                snapshot_part_signature(name, &index, &mut HashSet::new())?
            ));
        }
        signatures.sort();
        Ok(signatures)
    };
    Ok(signature(left)? == signature(right)?)
}

/// Whether two comment snapshots hold the same comments: they match, or one
/// has no comment parts and every part of the other holds nothing.
pub(super) fn comment_snapshots_agree(
    left: &serde_json::Value,
    right: &serde_json::Value,
) -> Result<bool> {
    fn holds_nothing(part: &serde_json::Value) -> Result<bool> {
        let xml = base64::engine::general_purpose::STANDARD
            .decode(
                part.get("xml")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
            )
            .map_err(|error| Error::Other(error.to_string()))?;
        let mut reader = oxoffice_core::xml_reader::Reader::from_reader(xml.as_slice());
        let (mut depth, mut buffer) = (0usize, Vec::new());
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(|error| Error::Other(error.to_string()))?
            {
                Event::Start(_) if depth > 0 => return Ok(false),
                Event::Empty(_) if depth > 0 => return Ok(false),
                Event::Start(_) => depth += 1,
                Event::End(_) => depth = depth.saturating_sub(1),
                Event::Eof => break,
                _ => {}
            }
            buffer.clear();
        }
        for child in part
            .get("related")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            if !holds_nothing(child)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    let none = |value: &serde_json::Value| value.as_array().is_some_and(Vec::is_empty);
    let empty = |value: &serde_json::Value| -> Result<bool> {
        for part in value.as_array().into_iter().flatten() {
            if !holds_nothing(part)? {
                return Ok(false);
            }
        }
        Ok(true)
    };
    Ok(comment_snapshots_match(left, right)?
        || none(left) && empty(right)?
        || none(right) && empty(left)?)
}

fn apply_related_part(document: &mut Document, part: &serde_json::Value) -> Result<()> {
    let field = |key: &str| -> Result<&str> {
        part.get(key)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Other(format!("invalid comparison comment field {key}")))
    };
    let name = field("name")?;
    let xml = base64::engine::general_purpose::STANDARD
        .decode(field("xml")?)
        .map_err(|error| Error::Other(error.to_string()))?;
    document.package.set_part(name, xml);
    if let Some(content_type) = part.get("content_type").and_then(serde_json::Value::as_str) {
        document
            .package
            .content_types
            .add_override(name, content_type);
    }
    if let Some(encoded) = part.get("rels").and_then(serde_json::Value::as_str) {
        let xml = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| Error::Other(error.to_string()))?;
        let rels = oxoffice_opc::relationship::Relationships::from_xml(&xml)
            .map_err(|error| Error::Other(error.to_string()))?;
        document.package.set_part_rels(name, rels);
    } else {
        document.package.remove_part_rels(name);
    }
    for child in part
        .get("related")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| Error::Other("invalid related comment parts".to_owned()))?
    {
        apply_related_part(document, child)?;
    }
    Ok(())
}

fn comment_part_has_inbound(document: &Document, child: &str) -> bool {
    document
        .package
        .package_rels
        .items
        .iter()
        .any(|relationship| {
            crate::document::relationship_is_internal(relationship)
                && OpcPackage::resolve_rel_target("/", &relationship.target) == child
        })
        || document.package.part_rels.iter().any(|(owner, rels)| {
            rels.items.iter().any(|relationship| {
                crate::document::relationship_is_internal(relationship)
                    && OpcPackage::resolve_rel_target(owner, &relationship.target) == child
            })
        })
}

fn remove_orphan_comment_part(document: &mut Document, name: &str, seen: &mut HashSet<String>) {
    if !seen.insert(name.to_owned()) {
        return;
    }
    let children = document
        .package
        .remove_part_rels(name)
        .into_iter()
        .flat_map(|rels| rels.items)
        .filter(crate::document::relationship_is_internal)
        .map(|relationship| OpcPackage::resolve_rel_target(name, &relationship.target))
        .collect::<Vec<_>>();
    document.package.remove_part(name);
    document.package.content_types.remove_override(name);
    for child in children {
        if !comment_part_has_inbound(document, &child) {
            remove_orphan_comment_part(document, &child, seen);
        }
    }
}

pub(super) fn extended_comment_snapshots_match(
    left: &serde_json::Value,
    right: &serde_json::Value,
) -> bool {
    let extended = |value: &serde_json::Value| {
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter(|part| {
                part.get("type").and_then(serde_json::Value::as_str)
                    == Some(crate::comments::COMMENTS_EXTENDED_REL_TYPE)
            })
            .map(|part| {
                part.get("xml")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect::<Vec<_>>()
    };
    extended(left) == extended(right)
}

pub(super) fn comment_package_links_match(
    left: &serde_json::Value,
    right: &serde_json::Value,
) -> bool {
    let links = |value: &serde_json::Value| {
        let mut parts = value
            .as_array()
            .into_iter()
            .flatten()
            .map(|part| {
                (
                    part.get("name").cloned().unwrap_or_default().to_string(),
                    part.get("rels").cloned().unwrap_or_default().to_string(),
                    part.get("related").cloned().unwrap_or_default().to_string(),
                )
            })
            .collect::<Vec<_>>();
        parts.sort();
        parts
    };
    links(left) == links(right)
}

pub(super) fn remap_comment_targets(
    document: &Document,
    snapshot: &mut serde_json::Value,
) -> Result<()> {
    fn plan(
        document: &Document,
        part: &serde_json::Value,
        child: bool,
        names: &mut HashMap<String, String>,
        occupied: &mut HashSet<String>,
    ) -> Result<()> {
        let name = part
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Other("comment snapshot target has no name".to_owned()))?;
        let encoded = part
            .get("xml")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Other("comment snapshot target has no bytes".to_owned()))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| Error::Other(error.to_string()))?;
        let desired_rels = part
            .get("rels")
            .and_then(serde_json::Value::as_str)
            .map(|value| base64::engine::general_purpose::STANDARD.decode(value))
            .transpose()
            .map_err(|error| Error::Other(error.to_string()))?;
        let existing_rels = document
            .package
            .get_part_rels(name)
            .map(|rels| rels.to_xml())
            .transpose()
            .map_err(|error| Error::Other(error.to_string()))?;
        let desired_content_type = part.get("content_type").and_then(serde_json::Value::as_str);
        let collision = document
            .package
            .get_part(name)
            .is_some_and(|old| old != bytes)
            || existing_rels != desired_rels
            || (document.package.contains_part(name)
                && document.package.content_types.content_type_for(name) != desired_content_type);
        let target = if child && collision {
            let (stem, extension) = name.rsplit_once('.').unwrap_or((name, ""));
            let mut ordinal = 1usize;
            loop {
                let candidate = if extension.is_empty() {
                    format!("{stem}-rdocx-comment-{ordinal}")
                } else {
                    format!("{stem}-rdocx-comment-{ordinal}.{extension}")
                };
                if !document.package.contains_part(&candidate)
                    && occupied.insert(candidate.to_ascii_lowercase())
                {
                    break candidate;
                }
                ordinal = ordinal
                    .checked_add(1)
                    .ok_or_else(|| Error::Other("comment target names exhausted".to_owned()))?;
            }
        } else {
            name.to_owned()
        };
        names.insert(name.to_owned(), target);
        for related in part
            .get("related")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| Error::Other("comment snapshot lacks related parts".to_owned()))?
        {
            plan(document, related, true, names, occupied)?;
        }
        Ok(())
    }

    fn rewrite(part: &mut serde_json::Value, names: &HashMap<String, String>) -> Result<()> {
        let old_name = part
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Other("comment snapshot target has no name".to_owned()))?
            .to_owned();
        if let Some(encoded) = part.get("rels").and_then(serde_json::Value::as_str) {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|error| Error::Other(error.to_string()))?;
            let mut rels = oxoffice_opc::relationship::Relationships::from_xml(&bytes)
                .map_err(|error| Error::Other(error.to_string()))?;
            for relationship in &mut rels.items {
                if crate::document::relationship_is_internal(relationship) {
                    let target = OpcPackage::resolve_rel_target(&old_name, &relationship.target);
                    if let Some(mapped) = names.get(&target) {
                        relationship.target = mapped.clone();
                    }
                }
            }
            let xml = rels
                .to_xml()
                .map_err(|error| Error::Other(error.to_string()))?;
            part["rels"] =
                serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(xml));
        }
        part["name"] = serde_json::Value::String(names.get(&old_name).cloned().unwrap_or(old_name));
        for related in part
            .get_mut("related")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or_else(|| Error::Other("comment snapshot lacks related parts".to_owned()))?
        {
            rewrite(related, names)?;
        }
        Ok(())
    }

    let roots = snapshot
        .as_array()
        .ok_or_else(|| Error::Other("invalid comparison comment snapshot".to_owned()))?;
    let mut occupied = document
        .package
        .parts
        .keys()
        .map(|name| name.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let mut names = HashMap::new();
    for root in roots {
        plan(document, root, false, &mut names, &mut occupied)?;
    }
    for root in snapshot
        .as_array_mut()
        .expect("snapshot roots were checked")
    {
        rewrite(root, &names)?;
    }
    Ok(())
}

pub(crate) fn apply_comment_snapshot(
    document: &mut Document,
    snapshot: &serde_json::Value,
) -> Result<()> {
    let parts = snapshot
        .as_array()
        .ok_or_else(|| Error::Other("invalid comparison comment snapshot".to_owned()))?;
    if let Some(relationships) = document.package.get_part_rels_mut(&document.doc_part_name) {
        let removed = relationships
            .items
            .iter()
            .filter(|item| comment_relationship(item))
            .map(|item| OpcPackage::resolve_rel_target(&document.doc_part_name, &item.target))
            .collect::<Vec<_>>();
        relationships
            .items
            .retain(|item| !comment_relationship(item));
        let mut children = Vec::new();
        for name in removed {
            if let Some(rels) = document.package.remove_part_rels(&name) {
                children.extend(
                    rels.items
                        .into_iter()
                        .filter(crate::document::relationship_is_internal)
                        .map(|relationship| {
                            OpcPackage::resolve_rel_target(&name, &relationship.target)
                        }),
                );
            }
            document.package.remove_part(&name);
            document.package.content_types.remove_override(&name);
        }
        let mut seen = HashSet::new();
        for child in children {
            if !comment_part_has_inbound(document, &child) {
                remove_orphan_comment_part(document, &child, &mut seen);
            }
        }
    }
    for part in parts {
        let field = |key: &str| -> Result<&str> {
            part.get(key)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| Error::Other(format!("invalid comparison comment field {key}")))
        };
        apply_related_part(document, part)?;
        document
            .package
            .get_or_create_part_rels(&document.doc_part_name)
            .items
            .push(Relationship {
                id: field("id")?.to_owned(),
                rel_type: field("type")?.to_owned(),
                target: field("target")?.to_owned(),
                target_mode: None,
            });
    }
    document.comments = None;
    document.comments_part_name = None;
    document.comments_owned = false;
    document.comments_extended = None;
    document.comments_extended_part_name = None;
    document.comments_extended_owned = false;
    Ok(())
}
