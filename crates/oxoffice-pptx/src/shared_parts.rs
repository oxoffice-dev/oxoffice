//! Parts that belong to one slide but that several relationships may reach.
//!
//! A slide's notes, comments, SmartArt data and embedded objects are its own,
//! yet a deck can relate two slides, or two shapes, to one of them (oxoffice
//! 0.1.0 left such decks after `duplicate_slide`). An edit that writes such a
//! part first gives the slide it edits a copy of its own, so the other owners
//! keep what they showed; a part nobody edits stays shared.

use std::collections::HashSet;

use oxoffice_opc::OpcPackage;
use oxoffice_opc::relationship::rel_types;

use crate::{Error, Result, relationship_is_external, relative_part_target};

/// How many internal relationships of the package reach `target`.
pub(super) fn part_users(package: &OpcPackage, target: &str) -> usize {
    std::iter::once(("/", &package.package_rels))
        .chain(
            package
                .part_rels
                .iter()
                .map(|(source, items)| (source.as_str(), items)),
        )
        .flat_map(|(source, items)| items.items.iter().map(move |item| (source, item)))
        .filter(|(source, item)| {
            !relationship_is_external(item)
                && OpcPackage::resolve_rel_target(source, &item.target).eq_ignore_ascii_case(target)
        })
        .count()
}

/// A numbered part name `{prefix}{n}{suffix}` that no part, relationship part
/// or content type override uses, ignoring case.
pub(super) fn unused_part_name(package: &OpcPackage, prefix: &str, suffix: &str) -> String {
    let mut number = next_numbered_part_number(package, prefix, suffix);
    loop {
        let candidate = format!("{prefix}{number}{suffix}");
        let occupied = package
            .parts
            .keys()
            .chain(package.part_rels.keys())
            .chain(package.content_types.overrides.keys())
            .any(|part| part.eq_ignore_ascii_case(&candidate));
        if !occupied {
            return candidate;
        }
        number += 1;
    }
}

fn next_numbered_part_number(package: &OpcPackage, prefix: &str, suffix: &str) -> usize {
    let occupied = package
        .parts
        .keys()
        .filter_map(|part| numbered_part_suffix(part, prefix, suffix))
        .collect::<HashSet<_>>();
    let mut candidate = occupied
        .iter()
        .copied()
        .max()
        .and_then(|value| value.checked_add(1))
        .unwrap_or(1);
    while occupied.contains(&candidate) {
        candidate = candidate.checked_add(1).unwrap_or(1);
    }
    candidate
}

fn numbered_part_suffix(part: &str, prefix: &str, suffix: &str) -> Option<usize> {
    let value = part.strip_prefix(prefix)?.strip_suffix(suffix)?;
    let parsed = value.parse::<usize>().ok()?;
    (parsed != 0).then_some(parsed)
}

/// Gives `slide_part` a copy of its own of the part its relationship
/// `relationship_id` reaches, holding `bytes`, and returns the copy's name.
///
/// The copy takes the original's content type and relationships; one that
/// pointed back at a slide (a notes slide's) points at `slide_part`. The
/// copy is named after the original, numbered past every part of that name.
pub(super) fn give_slide_its_own_part(
    package: &mut OpcPackage,
    slide_part: &str,
    relationship_id: &str,
    bytes: Vec<u8>,
) -> Result<String> {
    let relationship = package
        .get_part_rels(slide_part)
        .and_then(|relationships| relationships.get_by_id(relationship_id))
        .ok_or_else(|| Error::MissingRelationship {
            source_part: slide_part.to_owned(),
            relationship_id: relationship_id.to_owned(),
        })?;
    let original = OpcPackage::resolve_rel_target(slide_part, &relationship.target);
    let (stem, extension) = original
        .rsplit_once('.')
        .map_or((original.as_str(), String::new()), |(stem, extension)| {
            (stem, format!(".{extension}"))
        });
    let prefix = stem.trim_end_matches(|c: char| c.is_ascii_digit());
    let copy = unused_part_name(package, prefix, &extension);

    package.set_part(&copy, bytes);
    if let Some(content_type) = package.content_types.override_for(&original) {
        let content_type = content_type.to_owned();
        package.content_types.add_override(&copy, &content_type);
    }
    if let Some(mut relationships) = package.get_part_rels(&original).cloned() {
        for item in &mut relationships.items {
            if item.rel_type == rel_types::SLIDE && !relationship_is_external(item) {
                item.target = relative_part_target(&copy, slide_part);
            } else if !relationship_is_external(item) {
                let target = OpcPackage::resolve_rel_target(&original, &item.target);
                item.target = relative_part_target(&copy, &target);
            }
        }
        package.set_part_rels(&copy, relationships);
    }
    let target = relative_part_target(slide_part, &copy);
    if let Some(relationship) = package
        .get_part_rels_mut(slide_part)
        .and_then(|relationships| {
            relationships
                .items
                .iter_mut()
                .find(|item| item.id == relationship_id)
        })
    {
        relationship.target = target;
    }
    Ok(copy)
}

/// Makes the relationships of `part` back to slide `from` reach slide `to`.
pub(super) fn point_back_at(package: &mut OpcPackage, part: &str, from: &str, to: &str) {
    let target = relative_part_target(part, to);
    if let Some(relationships) = package.get_part_rels_mut(part) {
        for item in &mut relationships.items {
            if item.rel_type == rel_types::SLIDE
                && !relationship_is_external(item)
                && OpcPackage::resolve_rel_target(part, &item.target).eq_ignore_ascii_case(from)
            {
                item.target = target.clone();
            }
        }
    }
}
