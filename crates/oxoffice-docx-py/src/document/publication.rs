//! Publishing a new inventory: the certificates of unchanged owners and their related parts.

use super::*;

#[derive(Clone)]
pub(crate) enum WordPublication {
    Conservative,
    FormValues,
    /// Metadata and form publication that may keep comment and text-box
    /// owners when their physical part is proved unchanged.
    ModernFormValues,
    Signing,
    AppendBody,
    /// A body splice that inserts or removes blocks before others: the
    /// removed owners retire, the inserted ones are born, and every other
    /// owner keeps its states at its moved route, which its handles follow.
    BodySplice,
    HeaderFooterAppend(usize),
    ParagraphText(oxoffice_docx::ContentLocation),
    ParagraphAppend(oxoffice_docx::ContentLocation),
    TableGrid(oxoffice_docx::ContentLocation),
    HyperlinkRemove(WordOwnerLocation),
    CellText(oxoffice_docx::ContentLocation, usize, usize),
    ElementXml(WordOwnerLocation),
    RunText(WordOwnerLocation),
    /// An edit of the paragraph at this route alone (for a run edit, the
    /// run's paragraph). It retires the paragraph's content and what it
    /// holds, and what the edit renumbers, and keeps the document epoch.
    ParagraphEdit(oxoffice_docx::ContentLocation),
    /// An insertion in one table cell: the table's route, the row and the
    /// cell. It retires the cell's content and what the edit renumbers, and
    /// keeps the document epoch.
    CellInsert(oxoffice_docx::ContentLocation, usize, usize),
}

pub(super) fn form_owner_certificates(
    old: &[WordOwner],
    new: &[WordOwnerAddress],
    matched: &[Option<usize>],
    before: &FormSources,
    after: &FormSources,
    eligible_parts: &std::collections::HashSet<String>,
) -> std::collections::HashSet<(usize, usize)> {
    let mut old_routes = HashMap::<FormOwnerRoute, Vec<usize>>::new();
    let mut new_routes = HashMap::<FormOwnerRoute, Vec<usize>>::new();
    for (index, owner) in old.iter().enumerate() {
        if let Some(route) = form_owner_route(&owner.location()) {
            old_routes.entry(route).or_default().push(index);
        }
    }
    for (index, owner) in new.iter().enumerate() {
        if let Some(route) = form_owner_route(&owner.location) {
            new_routes.entry(route).or_default().push(index);
        }
    }
    let mut proved = std::collections::HashSet::new();
    let mut ready = vec![false; new.len()];
    for target in 0..new.len() {
        let mut stack = vec![(target, false)];
        while let Some((index, parent_ready)) = stack.pop() {
            if ready[index] {
                continue;
            }
            if !parent_ready && let Some(parent) = new[index].parent {
                stack.push((index, true));
                if !ready[parent] {
                    stack.push((parent, false));
                }
                continue;
            }
            ready[index] = true;
            let Some(route) = form_owner_route(&new[index].location) else {
                continue;
            };
            let Some(indices) = old_routes.get(&route) else {
                continue;
            };
            let [previous] = indices.as_slice() else {
                continue;
            };
            if new_routes.get(&route).map(Vec::as_slice) != Some(&[index])
                || matched[index] != Some(*previous)
            {
                continue;
            }
            let source_proved = match &route {
                FormOwnerRoute::Cell(_, _, _, story) => {
                    before.cells.get(&route) == Some(&1)
                        && after.cells.get(&route) == Some(&1)
                        && before.cell_stories.get(story) == Some(&1)
                        && after.cell_stories.get(story) == Some(&1)
                }
                FormOwnerRoute::Paragraph(key) | FormOwnerRoute::Table(key)
                    if matches!(
                        key.0,
                        oxoffice_docx::StoryKind::Comment | oxoffice_docx::StoryKind::TextBox
                    ) =>
                {
                    eligible_parts.contains(&key.1)
                        && matches!(
                            (before.xml.get(&route), after.xml.get(&route)),
                            (Some(Some(a)), Some(Some(b))) if a == b
                        )
                }
                _ => {
                    matches!((before.xml.get(&route), after.xml.get(&route)), (Some(Some(a)), Some(Some(b))) if a == b)
                }
            };
            if !source_proved {
                continue;
            }
            let ancestors_proved = match (old[*previous].parent, new[index].parent) {
                (Some(a), Some(b)) => proved.contains(&(a, b)),
                (None, None) => !matches!(
                    &route,
                    FormOwnerRoute::Cell(..)
                        | FormOwnerRoute::Paragraph((oxoffice_docx::StoryKind::TableCell, ..))
                        | FormOwnerRoute::Table((oxoffice_docx::StoryKind::TableCell, ..))
                ),
                _ => false,
            };
            if ancestors_proved {
                proved.insert((*previous, index));
            }
        }
    }
    proved
}

// Signing changes package infrastructure. Preserve content authority only when
// the physical source and every required ancestor survived serialization unchanged.
pub(super) fn signing_owner_certificates(
    old: &[WordOwner],
    new: &[WordOwnerAddress],
    matched: &[Option<usize>],
    before: &FormSources,
    after: &FormSources,
) -> std::collections::HashSet<(usize, usize)> {
    let mut proved = form_owner_certificates(
        old,
        new,
        matched,
        before,
        after,
        &std::collections::HashSet::new(),
    );
    let mut paragraphs = HashMap::<FormSourceKey, Vec<(usize, usize)>>::new();
    for &(previous, next) in &proved {
        if let WordOwnerLocation::Paragraph(location) = &new[next].location {
            paragraphs
                .entry(form_source_key(location))
                .or_default()
                .push((previous, next));
        }
    }
    for (next, address) in new.iter().enumerate() {
        let Some(previous) = matched[next] else {
            continue;
        };
        let survives = match (&*old[previous].location(), &address.location) {
            (WordOwnerLocation::Run(a, left), WordOwnerLocation::Run(b, right)) => {
                left == right
                    && form_source_key(a) == form_source_key(b)
                    && matches!((old[previous].parent, address.parent), (Some(a), Some(b)) if proved.contains(&(a, b)))
            }
            (WordOwnerLocation::Hyperlink(a, left), WordOwnerLocation::Hyperlink(b, right)) => {
                left == right
                    && form_source_key(a) == form_source_key(b)
                    && paragraphs
                        .get(&form_source_key(b))
                        .is_some_and(|pairs| pairs.len() == 1)
            }
            _ => false,
        };
        if survives {
            proved.insert((previous, next));
        }
    }
    proved
}

/// Comment and text-box certificates still need the ordinary owner proof.
/// A run survives only when that proof already covers its paragraph.
pub(super) fn modern_form_owner_certificates(
    old: &[WordOwner],
    new: &[WordOwnerAddress],
    matched: &[Option<usize>],
    before: &FormSources,
    after: &FormSources,
    eligible_parts: &std::collections::HashSet<String>,
) -> std::collections::HashSet<(usize, usize)> {
    let mut proved = form_owner_certificates(old, new, matched, before, after, eligible_parts);
    for (next, address) in new.iter().enumerate() {
        let Some(previous) = matched[next] else {
            continue;
        };
        let survives = match (&*old[previous].location(), &address.location) {
            (WordOwnerLocation::Run(a, left), WordOwnerLocation::Run(b, right)) => {
                left == right
                    && form_source_key(a) == form_source_key(b)
                    && matches!(
                        (old[previous].parent, address.parent),
                        (Some(a), Some(b)) if proved.contains(&(a, b))
                    )
            }
            _ => false,
        };
        if survives {
            proved.insert((previous, next));
        }
    }
    proved
}

fn related_owner_part(location: &WordOwnerLocation) -> Option<String> {
    let content = match location {
        WordOwnerLocation::Paragraph(location) | WordOwnerLocation::Table(location) => location,
        _ => return None,
    };
    if !matches!(
        content.story().kind(),
        oxoffice_docx::StoryKind::Comment | oxoffice_docx::StoryKind::TextBox
    ) {
        return None;
    }
    Some(content.story().part_name().to_owned())
}

/// One inventory name, or none when case-insensitive lookup is missing or repeated.
fn unique_part_name<'a>(names: &'a [String], route: &str) -> Option<&'a str> {
    let mut found = names.iter().filter(|name| name.eq_ignore_ascii_case(route));
    match (found.next(), found.next()) {
        (Some(name), None) => Some(name.as_str()),
        _ => None,
    }
}

/// Unchanged comment and text-box parts, plus whether content snapshots match.
///
/// `part_xml` is the save-equivalent XML text. A UTF-8 BOM is stripped, so
/// this is not a ZIP byte proof. `part_names`, `stories` and `part_xml` each
/// stage the package. One part is proved once. That cost is whole-package
/// validation, not a constant-time claim. A helper error is not equality.
pub(super) fn unchanged_related_owner_parts(
    old: &oxoffice_docx::Document,
    new: &oxoffice_docx::Document,
    old_owners: &[WordOwner],
    new_addresses: &[WordOwnerAddress],
) -> oxoffice_docx::Result<(std::collections::HashSet<String>, bool)> {
    let mut requested = Vec::new();
    for part in old_owners
        .iter()
        .filter_map(|owner| related_owner_part(&owner.location()))
        .chain(
            new_addresses
                .iter()
                .filter_map(|address| related_owner_part(&address.location)),
        )
    {
        if !requested.iter().any(|seen: &String| seen == &part) {
            requested.push(part);
        }
    }
    let old_names = old.part_names()?;
    let new_names = new.part_names()?;
    let old_stories = old.stories()?;
    let new_stories = new.stories()?;
    let mut eligible = std::collections::HashSet::new();
    for part in &requested {
        let distinct = requested
            .iter()
            .filter(|candidate| candidate.eq_ignore_ascii_case(part))
            .count();
        if distinct != 1 {
            continue;
        }
        let Some(actual_old) = unique_part_name(&old_names, part) else {
            continue;
        };
        let Some(actual_new) = unique_part_name(&new_names, part) else {
            continue;
        };
        if actual_old != actual_new || actual_old != part {
            continue;
        }
        let old_part = old_stories
            .iter()
            .filter(|story| story.part_name() == actual_old)
            .cloned()
            .collect::<Vec<_>>();
        let new_part = new_stories
            .iter()
            .filter(|story| story.part_name() == actual_new)
            .cloned()
            .collect::<Vec<_>>();
        if old_part != new_part {
            continue;
        }
        match (old.part_xml(actual_old), new.part_xml(actual_new)) {
            (Ok(before), Ok(after)) if before == after => {
                eligible.insert(actual_old.to_owned());
            }
            (Ok(_), Ok(_)) => {}
            (Err(_), _) | (_, Err(_)) => {}
        }
    }
    let preserve =
        old_stories == new_stories && old.story_item_snapshots()? == new.story_item_snapshots()?;
    Ok((eligible, preserve))
}
