//! Relationships, media and chart parts carried from the edited package.

use super::*;

const MAX_COMPARISON_CHART_PARTS: usize = 4096;
const MAX_COMPARISON_CHART_BYTES: usize = 256 * 1024 * 1024;

/// Give equivalent edited image, chart and hyperlink relationships their original ids.
/// Changed image/chart payloads and new hyperlinks get distinct ids and are carried
/// into the redline when its tracked content references them.
pub(super) fn remap_equivalent_story_relationships(
    original: &Document,
    edited: &mut Document,
    original_stories: &[StoryPart],
    edited_stories: &[StoryPart],
) -> Result<(Vec<CarriedRelationship>, CarriedCharts)> {
    let mut reserved_media = HashSet::new();
    let mut charts = CarriedCharts::default();
    let mut carried = remap_equivalent_owner_relationships(
        original,
        edited,
        &original.doc_part_name,
        &edited.doc_part_name.clone(),
        &mut reserved_media,
        &mut charts,
    )?;
    for (left, right) in original_stories.iter().zip(edited_stories) {
        carried.extend(remap_equivalent_owner_relationships(
            original,
            edited,
            &left.part_name,
            &right.part_name,
            &mut reserved_media,
            &mut charts,
        )?);
    }
    if let Some(right) = NotePart::of(edited, NoteKind::Footnote)? {
        let left = NotePart::of(original, NoteKind::Footnote)?;
        let owner = left
            .as_ref()
            .map_or(right.name.as_str(), |part| part.name.as_str());
        carried.extend(remap_equivalent_owner_relationships(
            original,
            edited,
            owner,
            &right.name,
            &mut reserved_media,
            &mut charts,
        )?);
    }
    Ok((carried, charts))
}

fn remap_equivalent_owner_relationships(
    original: &Document,
    edited: &mut Document,
    original_owner: &str,
    edited_owner: &str,
    reserved_media: &mut HashSet<String>,
    charts: &mut CarriedCharts,
) -> Result<Vec<CarriedRelationship>> {
    let original_relationships = original
        .package
        .get_part_rels(original_owner)
        .cloned()
        .unwrap_or_default();
    let edited_relationships = edited
        .package
        .get_part_rels(edited_owner)
        .cloned()
        .unwrap_or_default();
    let mut used = HashSet::new();
    let mut remap = HashMap::new();
    let mut kept = HashSet::new();
    let mut unmatched_links = Vec::new();
    let mut unmatched_images = Vec::new();
    let mut unmatched_charts = Vec::new();
    let original_keys = original_relationships
        .items
        .iter()
        .map(|left| relationship_key(original, original_owner, left))
        .collect::<Result<Vec<_>>>()?;
    let mut chart_parts = HashMap::new();
    let mut edited_keys = Vec::with_capacity(edited_relationships.items.len());
    for right in &edited_relationships.items {
        if right.rel_type == rel_types::CHART {
            let (signature, parts) = chart_relationship_signature(edited, edited_owner, right)?;
            chart_parts.insert(right.id.clone(), parts);
            edited_keys.push(Some(RelationshipKey::of(right, Some(signature.as_bytes()))));
        } else {
            edited_keys.push(relationship_key(edited, edited_owner, right)?);
        }
    }
    // An edited relationship takes an unused original relationship with the
    // same payload, or else a used one: several edited pictures or links can
    // name what one original relationship names. A chart takes only an
    // unused one: two drawings on one chart part would edit together.
    // A relationship both versions hold under one id keeps it first, so the
    // order of the edited relationships never decides which chart is the copy.
    let mut matches = vec![None; edited_relationships.items.len()];
    for (slot, right) in edited_relationships.items.iter().enumerate() {
        if let Some(key) = &edited_keys[slot]
            && let Some(index) = original_relationships
                .items
                .iter()
                .position(|left| left.id == right.id)
            && original_keys[index].as_ref() == Some(key)
            && used.insert(index)
        {
            matches[slot] = Some(index);
        }
    }
    for (slot, key) in edited_keys.iter().enumerate() {
        let Some(key) = key else {
            continue;
        };
        if matches[slot].is_some() {
            continue;
        }
        let same = |index: &usize| original_keys[*index].as_ref() == Some(key);
        let unused = (0..original_keys.len()).find(|index| !used.contains(index) && same(index));
        matches[slot] = unused.or_else(|| {
            (key.rel_type != rel_types::CHART)
                .then(|| (0..original_keys.len()).find(same))
                .flatten()
        });
        if let Some(index) = unused {
            used.insert(index);
        }
    }
    for (slot, right) in edited_relationships.items.iter().enumerate() {
        if edited_keys[slot].is_none() {
            continue;
        }
        if let Some(index) = matches[slot] {
            let left = &original_relationships.items[index];
            if left.id == right.id {
                kept.insert(right.id.clone());
            } else {
                remap.insert(right.id.clone(), left.id.clone());
            }
            continue;
        }
        if right.rel_type == rel_types::CHART {
            let source_part =
                OpcPackage::resolve_rel_target(edited_owner, &right.target).to_ascii_lowercase();
            let parts = chart_parts.remove(&right.id).unwrap_or_default();
            reserve_comparison_chart_parts(original, edited, parts, reserved_media, charts)?;
            unmatched_charts.push((right.clone(), source_part));
        } else if right.rel_type == rel_types::HYPERLINK
            || !crate::document::relationship_is_internal(right)
        {
            // A linked picture's image relationship is an address, carried
            // as a hyperlink's is.
            unmatched_links.push(right.clone());
        } else {
            let target = OpcPackage::resolve_rel_target(edited_owner, &right.target);
            let bytes = edited.package.get_part(&target).ok_or_else(|| {
                Error::Other(format!("comparison image target is missing: {target}"))
            })?;
            let content_type = edited
                .package
                .content_types
                .content_type_for(&target)
                .ok_or_else(|| {
                    Error::Other(format!("comparison image has no content type: {target}"))
                })?;
            unmatched_images.push((
                right.clone(),
                target,
                bytes.to_vec(),
                content_type.to_owned(),
            ));
        }
    }
    let mut occupied = edited_relationships
        .items
        .iter()
        .chain(&original_relationships.items)
        .map(|relationship| relationship.id.clone())
        .chain(remap.values().cloned())
        .collect::<HashSet<_>>();
    // The redline allocates a carried address's id the way the original's
    // relationships do, which is the id staging keeps when it reopens it.
    let mut allocator = original_relationships.clone();
    let mut carried = Vec::with_capacity(unmatched_links.len() + unmatched_images.len());
    for mut link in unmatched_links {
        let fresh = allocator.add_external(&link.rel_type, &link.target);
        occupied.insert(fresh.clone());
        remap.insert(link.id.clone(), fresh.clone());
        link.id = fresh;
        carried.push(CarriedRelationship {
            owner: original_owner.to_owned(),
            relationship: link,
            media: None,
            chart: None,
        });
    }
    for (mut image, source_part, bytes, content_type) in unmatched_images {
        let existing = original_relationships
            .items
            .iter()
            .find(|relationship| {
                if relationship.rel_type != rel_types::IMAGE
                    || !crate::document::relationship_is_internal(relationship)
                {
                    return false;
                }
                let target = OpcPackage::resolve_rel_target(original_owner, &relationship.target);
                original.package.content_types.content_type_for(&target)
                    == Some(content_type.as_str())
                    && relationship_payload(original, original_owner, relationship).as_deref()
                        == Some(bytes.as_slice())
            })
            .map(|relationship| (relationship.target.clone(), None))
            .or_else(|| {
                carried.iter().find_map(|existing: &CarriedRelationship| {
                    let media = existing.media.as_ref()?;
                    (media.bytes == bytes && media.content_type == content_type)
                        .then(|| (existing.relationship.target.clone(), Some(media.clone())))
                })
            });
        if let Some((target, media)) = existing {
            let mut id_ordinal = 1;
            let fresh = loop {
                let candidate = format!("rdocxComparisonImage{id_ordinal}");
                if occupied.insert(candidate.clone()) {
                    break candidate;
                }
                id_ordinal += 1;
            };
            remap.insert(image.id.clone(), fresh.clone());
            image.id = fresh;
            image.target = target;
            carried.push(CarriedRelationship {
                owner: original_owner.to_owned(),
                relationship: image,
                media,
                chart: None,
            });
            continue;
        }
        let extension = source_part.rsplit('.').next().unwrap_or("bin");
        let owner_dir = original_owner.rsplit_once('/').map_or("", |(dir, _)| dir);
        let mut ordinal = 1;
        let part_name = loop {
            let part_name = format!("{owner_dir}/media/rdocxComparison{ordinal}.{extension}");
            if !original.package.contains_part(&part_name)
                && !edited.package.contains_part(&part_name)
                && reserved_media.insert(part_name.clone())
            {
                break part_name;
            }
            ordinal += 1;
        };
        let target = format!("media/rdocxComparison{ordinal}.{extension}");
        let mut id_ordinal = 1;
        let fresh = loop {
            let candidate = format!("rdocxComparisonImage{id_ordinal}");
            if occupied.insert(candidate.clone()) {
                break candidate;
            }
            id_ordinal += 1;
        };
        remap.insert(image.id.clone(), fresh.clone());
        image.id = fresh;
        image.target = target;
        carried.push(CarriedRelationship {
            owner: original_owner.to_owned(),
            relationship: image,
            media: Some(CarriedMedia {
                part_name,
                bytes,
                content_type,
            }),
            chart: None,
        });
    }
    for (mut chart, source_part) in unmatched_charts {
        let mut ordinal = 1;
        let fresh = loop {
            let id = format!("rdocxComparisonChart{ordinal}");
            if occupied.insert(id.clone()) {
                break id;
            }
            ordinal += 1;
        };
        remap.insert(chart.id.clone(), fresh.clone());
        chart.id = fresh;
        chart.target = charts.part_map[&source_part].clone();
        carried.push(CarriedRelationship {
            owner: original_owner.to_owned(),
            relationship: chart,
            media: None,
            chart: Some(source_part),
        });
    }
    if remap.is_empty() {
        return Ok(carried);
    }
    let mut collision_remap = HashMap::new();
    let remapped_sources = remap.keys().cloned().collect::<HashSet<_>>();
    for target in remap.values() {
        // An edited relationship kept under this id names what the remapped
        // one names: they share it.
        if remapped_sources.contains(target)
            || kept.contains(target)
            || collision_remap.contains_key(target)
            || edited_relationships
                .items
                .iter()
                .all(|relationship| relationship.id != *target)
        {
            continue;
        }
        let mut ordinal = collision_remap.len() + 1;
        let replacement = loop {
            let candidate = format!("rdocxComparison{ordinal}");
            if occupied.insert(candidate.clone()) {
                break candidate;
            }
            ordinal += 1;
        };
        collision_remap.insert(target.clone(), replacement);
    }
    collision_remap.extend(remap);
    let source = edited
        .package
        .get_part(edited_owner)
        .ok_or_else(|| Error::Other(format!("missing comparison story {edited_owner}")))?;
    let updated = crate::document::remap_xml_relationship_ids(source, &collision_remap)?;
    if edited_owner == edited.doc_part_name {
        edited.document =
            crate::document::DocumentModel::unproved(CT_Document::from_xml(&updated)?);
    }
    edited.package.set_part(edited_owner, updated);
    if let Some(relationships) = edited.package.get_part_rels_mut(edited_owner) {
        for relationship in &mut relationships.items {
            if let Some(updated) = collision_remap.get(&relationship.id) {
                relationship.id.clone_from(updated);
            }
        }
        // Relationships that now share an id name the same payload.
        let mut ids = HashSet::new();
        relationships
            .items
            .retain(|relationship| ids.insert(relationship.id.clone()));
        relationships.to_xml()?;
    }
    Ok(carried)
}

fn reserve_comparison_chart_parts(
    original: &Document,
    edited: &Document,
    parts: Vec<String>,
    reserved: &mut HashSet<String>,
    charts: &mut CarriedCharts,
) -> Result<()> {
    for source in parts {
        if charts.part_map.contains_key(&source) {
            continue;
        }
        if charts.part_map.len() >= MAX_COMPARISON_CHART_PARTS {
            return Err(Error::Other(
                "comparison aggregate chart resources exceed part limit".to_owned(),
            ));
        }
        let size = comparison_chart_part_size(edited, &source)?;
        charts.bytes = charts.bytes.checked_add(size).ok_or_else(|| {
            Error::Other("comparison aggregate chart resources overflow".to_owned())
        })?;
        if charts.bytes > MAX_COMPARISON_CHART_BYTES {
            return Err(Error::Other(
                "comparison aggregate chart resources exceed byte limit".to_owned(),
            ));
        }
        let (directory, filename) = source.rsplit_once('/').unwrap_or(("", &source));
        let mut ordinal = 1;
        let target = loop {
            let target = format!("{directory}/rdocxComparison{ordinal}-{filename}");
            if !original.package.contains_part(&target)
                && !edited.package.contains_part(&target)
                && reserved.insert(target.to_ascii_lowercase())
            {
                break target;
            }
            ordinal += 1;
        };
        charts.part_map.insert(source, target);
    }
    Ok(())
}

fn comparison_chart_part_size(document: &Document, name: &str) -> Result<usize> {
    let bytes = document
        .package
        .get_part(name)
        .ok_or_else(|| Error::Other(format!("comparison chart target is missing: {name}")))?;
    let relationship_bytes = document
        .package
        .get_part_rels(name)
        .map(|relationships| relationships.to_xml())
        .transpose()?
        .map_or(0, |xml| xml.len());
    bytes
        .len()
        .checked_add(relationship_bytes)
        .ok_or_else(|| Error::Other("comparison chart closure size overflow".to_owned()))
}

/// Hash a chart's complete owned graph independently of package part names.
/// Sorted relationship ids define stable node ordinals, including shared and cyclic edges.
pub(super) fn chart_relationship_signature(
    document: &Document,
    owner: &str,
    relationship: &Relationship,
) -> Result<(String, Vec<String>)> {
    const MAX_CHART_DEPTH: usize = 128;
    if !crate::document::relationship_is_internal(relationship) {
        return Err(Error::Other(
            "comparison requires an internal chart relationship".to_owned(),
        ));
    }
    fn field(hash: &mut Sha256, bytes: &[u8]) {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    let root = OpcPackage::resolve_rel_target(owner, &relationship.target).to_ascii_lowercase();
    let mut nodes = vec![root.clone()];
    let mut ordinals = HashMap::from([(root.clone(), 0usize)]);
    let mut pending = vec![(root, 0usize)];
    let mut visited = HashSet::new();
    let mut hash = Sha256::new();
    let mut total_bytes = 0usize;
    while let Some((name, depth)) = pending.pop() {
        if !visited.insert(name.clone()) {
            continue;
        }
        if depth > MAX_CHART_DEPTH {
            return Err(Error::Other(
                "comparison chart closure exceeds graph limit".to_owned(),
            ));
        }
        let content_type = document
            .package
            .content_types
            .content_type_for(&name)
            .ok_or_else(|| {
                Error::Other(format!("comparison chart part has no content type: {name}"))
            })?;
        if content_type.starts_with("application/vnd.openxmlformats-package.digital-signature") {
            return Err(Error::Other(
                "comparison cannot remap integrity-bound chart resources".to_owned(),
            ));
        }
        let bytes = document
            .package
            .get_part(&name)
            .ok_or_else(|| Error::Other(format!("comparison chart target is missing: {name}")))?;
        total_bytes = total_bytes
            .checked_add(comparison_chart_part_size(document, &name)?)
            .ok_or_else(|| Error::Other("comparison chart closure size overflow".to_owned()))?;
        if total_bytes > MAX_COMPARISON_CHART_BYTES {
            return Err(Error::Other(
                "comparison chart closure exceeds byte limit".to_owned(),
            ));
        }
        field(&mut hash, content_type.as_bytes());
        field(&mut hash, bytes);
        let mut edges = document
            .package
            .get_part_rels(&name)
            .map(|rels| rels.items.clone())
            .unwrap_or_default();
        edges.sort_by(|left, right| left.id.cmp(&right.id));
        field(&mut hash, &(edges.len() as u64).to_le_bytes());
        let mut ids = HashSet::new();
        let mut children = Vec::new();
        for edge in edges {
            if !ids.insert(edge.id.clone()) {
                return Err(Error::Other(format!(
                    "comparison chart relationship id is ambiguous: {}",
                    edge.id
                )));
            }
            field(&mut hash, edge.id.as_bytes());
            field(&mut hash, edge.rel_type.as_bytes());
            if crate::document::relationship_is_internal(&edge) {
                field(&mut hash, b"internal");
                let target =
                    OpcPackage::resolve_rel_target(&name, &edge.target).to_ascii_lowercase();
                let next = if let Some(ordinal) = ordinals.get(&target) {
                    *ordinal
                } else {
                    if nodes.len() >= MAX_COMPARISON_CHART_PARTS {
                        return Err(Error::Other(
                            "comparison chart closure exceeds graph limit".to_owned(),
                        ));
                    }
                    let ordinal = nodes.len();
                    ordinals.insert(target.clone(), ordinal);
                    nodes.push(target.clone());
                    ordinal
                };
                field(&mut hash, &(next as u64).to_le_bytes());
                children.push((target, depth + 1));
            } else {
                if edge.target_mode.as_deref() != Some("External") {
                    return Err(Error::Other(
                        "comparison chart relationship has an invalid target mode".to_owned(),
                    ));
                }
                field(&mut hash, b"external");
                field(&mut hash, edge.target.as_bytes());
            }
        }
        // Match the existing copier's sorted depth-first recursion, rather than
        // shortest-path depth. Shortcut edges must not bypass its stack bound.
        pending.extend(children.into_iter().rev());
    }
    Ok((
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        nodes,
    ))
}

/// What a relationship names, for the relationships a comparison matches:
/// its type, target mode and payload (an image's bytes, an address, a
/// chart's graph signature).
#[derive(PartialEq, Eq, Hash)]
struct RelationshipKey {
    rel_type: String,
    target_mode: Option<String>,
    /// The payload's SHA-256.
    payload: Option<[u8; 32]>,
}

impl RelationshipKey {
    fn of(relationship: &Relationship, payload: Option<&[u8]>) -> Self {
        Self {
            rel_type: relationship.rel_type.clone(),
            target_mode: relationship.target_mode.clone(),
            payload: payload.map(|bytes| Sha256::digest(bytes).into()),
        }
    }
}

/// The key of an image, hyperlink or chart relationship, `None` for others.
fn relationship_key(
    document: &Document,
    owner: &str,
    relationship: &Relationship,
) -> Result<Option<RelationshipKey>> {
    Ok(if relationship.rel_type == rel_types::CHART {
        let (signature, _) = chart_relationship_signature(document, owner, relationship)?;
        Some(RelationshipKey::of(
            relationship,
            Some(signature.as_bytes()),
        ))
    } else if relationship.rel_type == rel_types::IMAGE
        || relationship.rel_type == rel_types::HYPERLINK
    {
        Some(RelationshipKey::of(
            relationship,
            relationship_payload(document, owner, relationship).as_deref(),
        ))
    } else {
        None
    })
}

/// The image, hyperlink and chart relationships of `owner` that name what
/// an earlier one names, each id to the first id of its class. Two versions
/// that reach one picture, chart or address through other relationships
/// then sign alike (see `RelationshipClassScope`).
pub(super) fn relationship_classes(
    document: &Document,
    owner: &str,
) -> Result<HashMap<String, String>> {
    let mut first = HashMap::new();
    let mut classes = HashMap::new();
    for relationship in document
        .package
        .get_part_rels(owner)
        .into_iter()
        .flat_map(|relationships| &relationships.items)
    {
        let Some(key) = relationship_key(document, owner, relationship)? else {
            continue;
        };
        let representative = first.entry(key).or_insert_with(|| relationship.id.clone());
        if *representative != relationship.id {
            classes.insert(relationship.id.clone(), representative.clone());
        }
    }
    Ok(classes)
}

fn relationship_payload(
    document: &Document,
    owner: &str,
    relationship: &Relationship,
) -> Option<Vec<u8>> {
    if !crate::document::relationship_is_internal(relationship) {
        return Some(relationship.target.as_bytes().to_vec());
    }
    let target = OpcPackage::resolve_rel_target(owner, &relationship.target);
    document.package.get_part(&target).map(<[u8]>::to_vec)
}
