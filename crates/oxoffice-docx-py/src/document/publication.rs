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

/// The story an owner's root names, if its route is in a story.
fn owner_root_story(owners: &[WordOwner], index: usize) -> Option<oxoffice_docx::StoryId> {
    let mut root = index;
    while let Some(parent) = owners[root].parent {
        root = parent;
    }
    match owners[root].route_shape() {
        WordOwnerLocation::Paragraph(location)
        | WordOwnerLocation::Table(location)
        | WordOwnerLocation::Run(location, _)
        | WordOwnerLocation::Hyperlink(location, _) => Some(location.story().clone()),
        WordOwnerLocation::Cell { story, .. } => Some(story.clone()),
        WordOwnerLocation::Section(_) => None,
    }
}

impl PyDocument {
    /// The publication of an in-place edit of one related story (a note):
    /// the owners of that story retire, their successors are born, and
    /// every other owner keeps its states and the document epoch, as a
    /// paragraph edit does: a run handle outside the story stays current.
    ///
    /// The caller ensured the Word authority before the edit, and `before`
    /// (debug builds) is the document as it was then. The story is read
    /// from the pieces, which reuse every block and part the edit left.
    pub(crate) fn prepare_story_revision(
        &mut self,
        py: Python<'_>,
        operation: &'static str,
        story: &oxoffice_docx::StoryId,
        #[cfg(debug_assertions)] before: &oxoffice_docx::Document,
    ) -> PyResult<PreparedWordRevision> {
        debug_assert!(
            self.snapshot_scope.is_some(),
            "Word authority before the edit"
        );
        let (pieces, inventory) = self
            .pieces
            .as_ref()
            .expect("active pieces")
            .assemble(&self.inner);
        let (locations, projections) = match inventory {
            Some((locations, projections)) => {
                #[cfg(debug_assertions)]
                assert_word_pieces_rebuild(&self.inner, &locations, &projections);
                (locations, Some(projections))
            }
            None => (
                word_owner_locations(&self.inner, None)
                    .map_err(|error| docx_to_pyerr(py, error))?,
                None,
            ),
        };
        let matches =
            indexed_inventory_matches(&self.owners, &locations, &WordPublication::AppendBody);
        let mut births = Vec::new();
        let mut retained = Vec::with_capacity(locations.len());
        for (index, address) in locations.iter().enumerate() {
            let kept = matches[index].filter(|&before| {
                !owner_root_story(&self.owners, before)
                    .is_some_and(|owned| same_story_owner(&owned, story))
            });
            let states = address
                .location
                .scopes()
                .iter()
                .enumerate()
                .map(|(slot, scope)| {
                    let state = kept.map(|before| self.owners[before].states()[slot].clone());
                    if state.is_none() {
                        births.push(*scope);
                    }
                    state
                })
                .collect::<Vec<_>>();
            retained.push(states);
        }
        let surviving = retained
            .iter()
            .flatten()
            .filter_map(Option::as_ref)
            .map(RevisionState::generation)
            .collect::<std::collections::HashSet<_>>();
        let mut retired = self
            .owners
            .iter()
            .flat_map(WordOwner::states)
            .filter(|state| !surviving.contains(&state.generation()))
            .collect::<Vec<_>>();
        retired.push(
            self.snapshot_scope
                .as_ref()
                .expect("active Word authority")
                .clone(),
        );
        births.push(RevisionScope::Text);
        let revision = self
            .revisions
            .prepare_local_scopes(operation, &retired, &births)
            .map_err(revision_to_pyerr)?;
        let mut born = 0;
        let mut states = Vec::new();
        for record in retained {
            for state in record {
                states.push(state.unwrap_or_else(|| {
                    let state = revision.born(born).expect("prepared owner birth");
                    born += 1;
                    state
                }));
            }
        }
        let snapshot_scope = revision.born(born).expect("prepared snapshot birth");
        let projections = projections.unwrap_or_else(|| WordIndexProjections::of(&self.inner));
        let items = WordBodyItems::derive(&locations, &projections);
        let owners = word_owners_with_states(locations, &states, &items);
        #[cfg(debug_assertions)]
        assert_kept_owners_name_their_elements(
            (before, &self.owners),
            (&self.inner, &owners),
            None,
        );
        let indexes = WordOwnerIndexes::prepare(&owners, items, projections)?;
        Ok(PreparedWordRevision {
            revision,
            owners,
            indexes,
            snapshot_scope,
            pieces: Some(pieces),
        })
    }

    /// Commit a story publication of the live document, whose pieces it
    /// installs: no candidate replaces the document.
    pub(crate) fn commit_story_revision(&mut self, prepared: PreparedWordRevision) -> PyResult<()> {
        self.commit_word_revision(prepared)?;
        self.pieces = Some(self.pending_pieces.take().unwrap_or_default());
        Ok(())
    }
}

/// For each address of a part read alone, whether its root names `story`.
fn in_story(addresses: &[WordOwnerAddress], story: &oxoffice_docx::StoryId) -> Vec<bool> {
    addresses
        .iter()
        .map(|address| {
            let mut root = address;
            while let Some(parent) = root.parent {
                root = &addresses[parent];
            }
            match &root.location {
                WordOwnerLocation::Paragraph(location)
                | WordOwnerLocation::Table(location)
                | WordOwnerLocation::Run(location, _)
                | WordOwnerLocation::Hyperlink(location, _) => {
                    same_story_owner(location.story(), story)
                }
                WordOwnerLocation::Cell { story: owned, .. } => same_story_owner(owned, story),
                WordOwnerLocation::Section(_) => false,
            }
        })
        .collect()
}

/// The routes of the addresses outside the story, each with its ordinal
/// and its parent counted among them: equal when the edit left them.
fn routes_outside(
    addresses: &[WordOwnerAddress],
    inside: &[bool],
) -> Vec<(u64, Option<usize>, Option<usize>)> {
    let mut rank = vec![None; addresses.len()];
    let mut next = 0;
    for (index, inside) in inside.iter().enumerate() {
        if !inside {
            rank[index] = Some(next);
            next += 1;
        }
    }
    addresses
        .iter()
        .zip(inside)
        .filter(|(_, inside)| !**inside)
        .map(|(address, _)| {
            (
                route_key(&address.location, None),
                address.projection_ordinal,
                address.parent.and_then(|parent| rank[parent]),
            )
        })
        .collect()
}

impl PyDocument {
    /// Publish an in-place edit of one note story at the cost of its part:
    /// the owners of that story retire, the owners read from the part for
    /// that story are born, every other owner and the document epoch stay
    /// (as [`Self::prepare_story_revision`] decides, without reading the
    /// whole inventory).
    ///
    /// Answers `false`, publishing nothing, when the part cannot be read
    /// alone or its other stories read otherwise than before (a table or
    /// text box that renumbers the part's cell or text-box stories): the
    /// caller then publishes the whole inventory.
    pub(crate) fn splice_story_owners(
        &mut self,
        py: Python<'_>,
        operation: &'static str,
        story: &oxoffice_docx::StoryId,
        #[cfg(debug_assertions)] before: &oxoffice_docx::Document,
    ) -> PyResult<bool> {
        let Some(splice) = self.read_story_splice(py, story)? else {
            return Ok(false);
        };
        #[cfg(debug_assertions)]
        let old_owners = self.owners.clone();
        self.commit_story_splice(operation, story, splice)?;
        #[cfg(debug_assertions)]
        {
            self.assert_word_inventory_rebuilds(false);
            assert_kept_owners_name_their_elements(
                (before, &old_owners),
                (&self.inner, &self.owners),
                None,
            );
        }
        Ok(true)
    }

    /// The read half of [`Self::splice_story_owners`]: the story's part read
    /// alone and checked, `None` when the splice cannot publish it. It reads
    /// no body owner, so a body publication may come between it and
    /// [`Self::commit_story_splice`].
    pub(crate) fn read_story_splice(
        &self,
        py: Python<'_>,
        story: &oxoffice_docx::StoryId,
    ) -> PyResult<Option<StorySplice>> {
        let part_name = story.part_name();
        let Some((part, old)) = self
            .pieces
            .as_ref()
            .and_then(|pieces| pieces.part_owner_addresses(part_name))
        else {
            return Ok(None);
        };
        let read = {
            let sources = self
                .inner
                .story_part_sources()
                .map_err(|error| docx_to_pyerr(py, error))?;
            sources
                .iter()
                .find(|(_, name, _)| name == part_name)
                .and_then(|(kind, name, xml)| WordPieces::read_part(&self.inner, *kind, name, xml))
        };
        let Some((piece, new)) = read else {
            return Ok(None);
        };
        let (old_inside, new_inside) = (in_story(&old, story), in_story(&new, story));
        if routes_outside(&old, &old_inside) != routes_outside(&new, &new_inside) {
            return Ok(None);
        }
        if self.story_owner_keys(story).len() != old_inside.iter().filter(|inside| **inside).count()
        {
            return Ok(None);
        }
        Ok(Some(StorySplice {
            part,
            piece,
            new,
            new_inside,
        }))
    }

    /// The keys of the owners whose root is in `story`.
    fn story_owner_keys(&self, story: &oxoffice_docx::StoryId) -> Vec<usize> {
        (0..self.owners.len())
            .filter(|&key| {
                owner_root_story(&self.owners, key)
                    .is_some_and(|owned| same_story_owner(&owned, story))
            })
            .collect()
    }

    /// The commit half of [`Self::splice_story_owners`], without its debug
    /// checks: the caller runs them once the whole edit is published.
    pub(crate) fn commit_story_splice(
        &mut self,
        operation: &'static str,
        story: &oxoffice_docx::StoryId,
        splice: StorySplice,
    ) -> PyResult<()> {
        let (mut retired, mut births) = self.story_splice_scopes(story, &splice);
        retired.push(
            self.snapshot_scope
                .as_ref()
                .expect("active Word authority")
                .clone(),
        );
        births.push(RevisionScope::Text);
        let revision = self
            .revisions
            .prepare_local_scopes(operation, &retired, &births)
            .map_err(revision_to_pyerr)?;
        let states = (0..births.len())
            .map(|index| revision.born(index).expect("prepared owner birth"))
            .collect::<Vec<_>>();
        self.reserve_story_splice(&splice)?;
        self.revisions
            .commit_scopes(revision)
            .map_err(revision_to_pyerr)?;
        let (snapshot, states) = states.split_last().expect("snapshot birth");
        self.apply_story_splice(story, splice, states);
        self.snapshot_scope = Some(snapshot.clone());
        Ok(())
    }

    /// The states the owners of `story` retire and the scopes its new
    /// owners are born with, the snapshot scope aside.
    pub(crate) fn story_splice_scopes(
        &self,
        story: &oxoffice_docx::StoryId,
        splice: &StorySplice,
    ) -> (Vec<RevisionState>, Vec<RevisionScope>) {
        let retired = self
            .story_owner_keys(story)
            .iter()
            .flat_map(|&key| self.owners[key].states())
            .collect::<Vec<_>>();
        let births = splice
            .new
            .iter()
            .zip(&splice.new_inside)
            .filter(|(_, inside)| **inside)
            .flat_map(|(address, _)| address.location.scopes().iter().copied())
            .collect::<Vec<_>>();
        (retired, births)
    }

    /// Reserve the owner slots [`Self::apply_story_splice`] may push, so
    /// that nothing fails once the revision commits.
    pub(crate) fn reserve_story_splice(&mut self, splice: &StorySplice) -> PyResult<()> {
        let count = splice.new_inside.iter().filter(|inside| **inside).count();
        self.owners.try_reserve(count).map_err(|_| {
            pyo3::exceptions::PyMemoryError::new_err("Word story splice allocation failed")
        })
    }

    /// Install the owners of `story` that `splice` read, under `states`
    /// (the births of [`Self::story_splice_scopes`], in order), after the
    /// revision committed: the story's owners take the keys of its retired
    /// ones, then keys after every current one; a parent is in the story
    /// with its child. A story that lost owners removes them by swap
    /// (`remove_owners`).
    pub(crate) fn apply_story_splice(
        &mut self,
        story: &oxoffice_docx::StoryId,
        splice: StorySplice,
        states: &[RevisionState],
    ) {
        let StorySplice {
            part,
            piece,
            new,
            new_inside,
        } = splice;
        let keys = self.story_owner_keys(story);
        let count = new_inside.iter().filter(|inside| **inside).count();
        let mut free = keys.clone();
        if count < keys.len() {
            self.remove_owners(&keys);
            free.clear();
        } else {
            for &key in &keys {
                self.indexes.unindex_owner(key, &self.owners[key]);
            }
        }
        let mut slots = free.into_iter().chain(self.owners.len()..);
        let key_of = new_inside
            .iter()
            .map(|inside| inside.then(|| slots.next().expect("a key")))
            .collect::<Vec<_>>();
        let mut offset = 0;
        for (index, inside) in new_inside.iter().enumerate() {
            if !inside {
                continue;
            }
            let mut address = new[index].clone();
            let scopes = &states[offset..offset + address.location.scopes().len()];
            offset += scopes.len();
            address.parent = address
                .parent
                .map(|parent| key_of[parent].expect("a story owner's parent is in the story"));
            let owner = WordOwner::new(address, scopes, &self.indexes.items);
            let key = key_of[index].expect("a story owner has a key");
            self.indexes.index_owner(key, &owner);
            if key < self.owners.len() {
                self.owners[key] = owner;
            } else {
                self.owners.push(owner);
            }
        }
        self.pieces
            .as_mut()
            .expect("active pieces")
            .replace_part(part, piece);
    }
}

/// What each section is and shows, in section order: its properties
/// (`w:sectPr`), and for each header and footer variant the effective story
/// (its kind, part and owner index, not its content fingerprint, which an
/// edit of the part changes) and the section it comes from. `None` when a
/// section's story cannot be resolved. It walks the section properties once
/// per variant and section, without reading any story.
pub(crate) type SectionStories = Vec<(
    Option<oxoffice_docx_oxml::CT_SectPr>,
    Vec<Option<(oxoffice_docx::StoryKind, String, usize, usize)>>,
)>;

pub(crate) fn section_stories(document: &oxoffice_docx::Document) -> Option<SectionStories> {
    use oxoffice_docx::{HdrFtrType, HeaderFooterKind};
    (0..document.section_count())
        .map(|index| {
            [HeaderFooterKind::Header, HeaderFooterKind::Footer]
                .into_iter()
                .flat_map(|kind| {
                    [HdrFtrType::Default, HdrFtrType::First, HdrFtrType::Even]
                        .map(|variant| (kind, variant))
                })
                .map(|(kind, variant)| {
                    let story = document.section_story(index, kind, variant).ok()?;
                    Some(story.map(|story| {
                        let id = story.story();
                        (
                            id.kind(),
                            id.part_name().to_owned(),
                            id.owner_index(),
                            story.source_section(),
                        )
                    }))
                })
                .collect::<Option<Vec<_>>>()
                .map(|stories| {
                    let properties = document
                        .section(index)
                        .map(|section| section.properties().clone());
                    (properties, stories)
                })
        })
        .collect()
}

/// How an edit of header and footer parts was published.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoryPartsPublication {
    /// Nothing a handle names changed: every handle stays.
    Nothing,
    /// The owners of the changed, added and removed story parts, and the
    /// section owners' content views whose stories changed.
    Spliced,
    /// The whole inventory, every handle retired.
    Whole,
}

impl PyDocument {
    /// Run `edit`, a header, footer, watermark or section story edit, on
    /// the live document, and publish what it changed (decision 4 of
    /// `specs/2026-11-24-staged-edits-in-place.md`):
    ///
    /// - no change: nothing, every handle kept;
    /// - a header or footer part changed, added or no longer referenced:
    ///   the owners of that part retire, those read from the part alone
    ///   are born ([`Self::splice_story_parts`]), every other owner and the
    ///   document epoch stay;
    /// - a section that shows another story than before: its section
    ///   owner's content view retires (the header and footer handles of
    ///   that section and what they returned), its owner view stays.
    ///
    /// A refused edit, or a refused publication, leaves the document and
    /// every handle as they were. A document whose header and footer edits
    /// take the staged route, or whose story parts the pieces do not hold,
    /// is edited on a copy and publishes the whole inventory, as before.
    pub(crate) fn edit_story_parts<T: Send>(
        &mut self,
        py: Python<'_>,
        operation: &'static str,
        edit: impl FnOnce(&mut oxoffice_docx::Document) -> oxoffice_docx::Result<T> + Send,
    ) -> PyResult<(T, StoryPartsPublication)> {
        if self.snapshot_scope.is_none() {
            // No Word authority: no owner and no handle to publish to.
            let inner = &mut self.inner;
            let value = py
                .detach(|| edit(inner))
                .map_err(|error| docx_to_pyerr(py, error))?;
            return Ok((value, StoryPartsPublication::Nothing));
        }
        let sections = section_stories(&self.inner);
        let parts_known = self.pieces.as_ref().is_some_and(|pieces| {
            self.inner.story_part_sources().is_ok_and(|sources| {
                sources
                    .iter()
                    .map(|(kind, name, _)| (*kind, name.as_str()))
                    .eq(pieces.part_sources().map(|(kind, name, _)| (kind, name)))
            })
        });
        let (Some(sections), true, false) = (
            sections,
            parts_known,
            self.inner.header_footer_edits_are_staged(),
        ) else {
            let mut candidate = self.inner.clone_for_staging();
            let value = py
                .detach(|| edit(&mut candidate))
                .map_err(|error| docx_to_pyerr(py, error))?;
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                operation,
                WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
            return Ok((value, StoryPartsPublication::Whole));
        };
        #[cfg(debug_assertions)]
        let before = self.inner.clone_for_staging();
        self.inner.hold_edit_undo();
        // An edit that fails or panics leaves the document as it was and
        // stops holding the undo: the guard undoes it as it drops.
        let held = super::block_edits_in_place::HeldUndo {
            document: &mut self.inner,
            armed: true,
        };
        let (result, held) = py.detach(move || {
            let result = edit(held.document);
            (result, held)
        });
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                drop(held);
                return Err(docx_to_pyerr(py, error));
            }
        };
        let undo = held.take();
        // No in-place edit ran (the call found nothing to do), or it
        // changed nothing: no publication.
        let Some(undo) = undo.filter(|undo| undo.report().changed()) else {
            #[cfg(debug_assertions)]
            self.assert_word_inventory_rebuilds(false);
            return Ok((value, StoryPartsPublication::Nothing));
        };
        // A publication that panics leaves the document as it was: undo the
        // edit before the panic goes on.
        let published = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.splice_story_parts(
                py,
                operation,
                &sections,
                #[cfg(debug_assertions)]
                &before,
            )
            .and_then(|spliced| {
                if spliced {
                    return Ok(StoryPartsPublication::Spliced);
                }
                let candidate = self.inner.clone_for_staging();
                let prepared = self.prepare_word_revision(
                    py,
                    &candidate,
                    operation,
                    WordPublication::Conservative,
                )?;
                self.commit_word_revision(prepared)?;
                self.install_inner(candidate);
                Ok(StoryPartsPublication::Whole)
            })
        }));
        let published = match published {
            Ok(published) => published,
            Err(panic) => {
                self.inner.undo_edit(undo);
                std::panic::resume_unwind(panic);
            }
        };
        match published {
            Ok(publication) => Ok((value, publication)),
            Err(error) => {
                self.inner.undo_edit(undo);
                Err(error)
            }
        }
    }

    /// Publish an in-place edit of header and footer parts and of what each
    /// section shows, at the cost of those parts: the owners of each story
    /// part whose bytes changed, or that the document no longer relates,
    /// retire; the owners of each changed or added part, read from that
    /// part alone, are born; each section that shows another story than in
    /// `sections` (taken before the edit) gets a new content view; every
    /// other owner and the document epoch stay.
    ///
    /// Answers `false`, publishing nothing, when a part cannot be read alone
    /// or the inventory does not hold the owners the pieces say a part has:
    /// the caller then publishes the whole inventory.
    pub(crate) fn splice_story_parts(
        &mut self,
        py: Python<'_>,
        operation: &'static str,
        sections: &SectionStories,
        #[cfg(debug_assertions)] before: &oxoffice_docx::Document,
    ) -> PyResult<bool> {
        let Some(after) = section_stories(&self.inner) else {
            return Ok(false);
        };
        if after.len() != sections.len() {
            return Ok(false);
        }
        let Some(pieces) = self.pieces.as_ref() else {
            return Ok(false);
        };
        let sources = self
            .inner
            .story_part_sources()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let old_parts = pieces
            .part_sources()
            .map(|(kind, name, xml)| (kind, name.to_owned(), xml))
            .collect::<Vec<_>>();
        // The parts whose owners retire, with how many the pieces hold.
        let mut retiring = Vec::<String>::new();
        let mut retiring_count = 0;
        let mut next_parts = Vec::with_capacity(sources.len());
        let mut born_parts = Vec::new();
        let mut kept = vec![false; old_parts.len()];
        for (kind, name, xml) in &sources {
            let old = old_parts
                .iter()
                .position(|(old_kind, old_name, _)| old_kind == kind && old_name == name);
            if let Some(index) = old {
                kept[index] = true;
                if old_parts[index].2 == xml.as_ref() {
                    next_parts.push(pieces.part_piece(index));
                    continue;
                }
                let Some((_, addresses)) = pieces.part_owner_addresses(name) else {
                    return Ok(false);
                };
                retiring_count += addresses.len();
                retiring.push(name.clone());
            }
            let Some((piece, addresses)) = WordPieces::read_part(&self.inner, *kind, name, xml)
            else {
                return Ok(false);
            };
            next_parts.push(piece);
            born_parts.push(addresses);
        }
        for (index, (_, name, _)) in old_parts.iter().enumerate() {
            if kept[index] {
                continue;
            }
            let Some((_, addresses)) = pieces.part_owner_addresses(name) else {
                return Ok(false);
            };
            retiring_count += addresses.len();
            retiring.push(name.clone());
        }
        drop(old_parts);
        // A section whose properties changed (a header reference) is another
        // element: its owner retires with its content view. One that shows
        // other stories through an earlier section retires its content view.
        let changed_sections = sections
            .iter()
            .zip(&after)
            .enumerate()
            .filter(|(_, (before, after))| before != after)
            .map(|(index, (before, after))| (index, before.0 != after.0))
            .collect::<Vec<_>>();
        if retiring.is_empty() && born_parts.is_empty() && changed_sections.is_empty() {
            // The edit changed no story part and no section's stories: no
            // owner changed.
            #[cfg(debug_assertions)]
            self.assert_word_inventory_rebuilds(false);
            return Ok(true);
        }
        let keys = (0..self.owners.len())
            .filter(|&key| {
                owner_root_story(&self.owners, key)
                    .is_some_and(|story| retiring.iter().any(|name| name == story.part_name()))
            })
            .collect::<Vec<_>>();
        if keys.len() != retiring_count {
            return Ok(false);
        }
        // The direct body paragraphs that hold the properties of the
        // sections before the last one, in section order: one whose section
        // properties changed is another element too, and retires (its runs
        // keep their states, which only a handle taken through it names).
        let holders = self
            .inner
            .body_children()
            .iter()
            .enumerate()
            .filter(|(_, block)| {
                matches!(
                    block,
                    oxoffice_docx_oxml::BodyContent::Paragraph(paragraph)
                        if paragraph
                            .properties
                            .as_ref()
                            .is_some_and(|properties| properties.sect_pr.is_some())
                )
            })
            .map(|(slot, _)| slot)
            .collect::<Vec<_>>();
        // Each renewed owner: its key, and whether all its states renew
        // (else its content view alone), in birth order.
        let mut renewed = Vec::with_capacity(2 * changed_sections.len());
        let mut held = Vec::new();
        for &(index, properties) in &changed_sections {
            let Some(key) = self
                .indexes
                .find(&self.owners, &WordOwnerLocation::Section(index))
            else {
                return Ok(false);
            };
            if self.owners[key].content.is_none() {
                return Ok(false);
            }
            renewed.push((key, properties));
            if properties && let Some(&slot) = holders.get(index) {
                let Some(key) = self.indexes.direct_body_paragraph(slot) else {
                    return Ok(false);
                };
                renewed.push((key, true));
                held.push(slot);
            }
        }
        // A renewed owner is never a retiring one: a compaction below moves
        // it, never drops it.
        if renewed.iter().any(|(key, _)| keys.contains(key)) {
            return Ok(false);
        }
        // The new owners of the holding paragraphs, read before the revision
        // is prepared: a paragraph read fails before anything is published.
        let mut held_owners = Vec::with_capacity(held.len());
        {
            let blocks = self.inner.body_children();
            for &slot in &held {
                let Ok(Some(owners)) = word_block_owners(&self.inner, &blocks[slot]) else {
                    return Ok(false);
                };
                held_owners.push((slot, owners));
            }
        }
        #[cfg(debug_assertions)]
        let renewed_locations = renewed
            .iter()
            .map(|&(key, _)| self.owners[key].location().into_owned())
            .collect::<Vec<_>>();
        let section_states = renewed
            .iter()
            .flat_map(|&(key, all)| {
                let owner = &self.owners[key];
                if all {
                    owner.states()
                } else {
                    owner.content.iter().cloned().collect()
                }
            })
            .collect::<Vec<_>>();
        let mut retired = keys
            .iter()
            .flat_map(|&key| self.owners[key].states())
            .chain(section_states)
            .collect::<Vec<_>>();
        retired.push(
            self.snapshot_scope
                .as_ref()
                .expect("active Word authority")
                .clone(),
        );
        let mut births = born_parts
            .iter()
            .flatten()
            .flat_map(|address| address.location.scopes().iter().copied())
            .collect::<Vec<_>>();
        for &(key, all) in &renewed {
            if all {
                births.extend_from_slice(self.owners[key].location().scopes());
            } else {
                births.push(RevisionScope::Text);
            }
        }
        births.push(RevisionScope::Text);
        let revision = self
            .revisions
            .prepare_local_scopes(operation, &retired, &births)
            .map_err(revision_to_pyerr)?;
        let states = (0..births.len())
            .map(|index| revision.born(index).expect("prepared owner birth"))
            .collect::<Vec<_>>();
        #[cfg(debug_assertions)]
        let old_owners = self.owners.clone();
        self.revisions
            .commit_scopes(revision)
            .map_err(revision_to_pyerr)?;
        // The born owners take the keys of the retired ones, then keys after
        // every current one; a parent is in its child's part. Parts that
        // lost owners compact the inventory.
        let count = born_parts.iter().map(Vec::len).sum::<usize>();
        let mut free = keys.clone();
        if count < keys.len() {
            // The compaction moves the last owner into each removed key,
            // from the highest removed key down: the renewed owners follow.
            let mut removed = keys.clone();
            removed.sort_unstable_by(|a, b| b.cmp(a));
            let mut len = self.owners.len();
            for key in removed {
                len -= 1;
                for (renewed_key, _) in &mut renewed {
                    if *renewed_key == len {
                        *renewed_key = key;
                    }
                }
            }
            self.remove_owners(&keys);
            free.clear();
        } else {
            for &key in &keys {
                self.indexes.unindex_owner(key, &self.owners[key]);
            }
        }
        let mut slots = free.into_iter().chain(self.owners.len()..);
        let mut offset = 0;
        for addresses in born_parts {
            let key_of = addresses
                .iter()
                .map(|_| slots.next().expect("a key"))
                .collect::<Vec<_>>();
            for (index, mut address) in addresses.into_iter().enumerate() {
                let scopes = &states[offset..offset + address.location.scopes().len()];
                offset += scopes.len();
                address.parent = address.parent.map(|parent| key_of[parent]);
                let owner = WordOwner::new(address, scopes, &self.indexes.items);
                let key = key_of[index];
                self.indexes.index_owner(key, &owner);
                if key < self.owners.len() {
                    self.owners[key] = owner;
                } else {
                    debug_assert_eq!(key, self.owners.len(), "born keys follow the owners");
                    self.owners.push(owner);
                }
            }
        }
        // The renewed owners, at the keys the compaction moved them to.
        #[cfg(debug_assertions)]
        for (&(key, _), location) in renewed.iter().zip(&renewed_locations) {
            debug_assert_eq!(
                self.indexes.find(&self.owners, location),
                Some(key),
                "a renewed owner follows the compaction"
            );
        }
        for (key, all) in renewed {
            let mut next = || {
                let state = states[offset].clone();
                offset += 1;
                state
            };
            let owner = &mut self.owners[key];
            if !all {
                owner.content = Some(next());
                continue;
            }
            // Its route, parent and children stay: only the index by
            // owner state follows.
            let previous = owner.owner.generation();
            owner.owner = next();
            for state in [&mut owner.content, &mut owner.properties, &mut owner.grid] {
                if state.is_some() {
                    *state = Some(next());
                }
            }
            let current = owner.owner.generation();
            self.indexes.by_generation.remove(&previous);
            self.indexes.by_generation.insert(current, key);
        }
        // The pieces of the holding paragraphs follow their new bytes.
        for (slot, owners) in held_owners {
            self.pieces
                .as_mut()
                .expect("active pieces")
                .splice(&self.inner, slot, 1, Some(owners));
        }
        self.snapshot_scope = Some(states.last().expect("snapshot birth").clone());
        self.pieces
            .as_mut()
            .expect("active pieces")
            .replace_parts(next_parts);
        #[cfg(debug_assertions)]
        {
            self.assert_word_inventory_rebuilds(false);
            assert_kept_owners_name_their_elements(
                (before, &old_owners),
                (&self.inner, &self.owners),
                None,
            );
        }
        Ok(true)
    }
}

/// A story's part read and checked by [`PyDocument::read_story_splice`],
/// for [`PyDocument::commit_story_splice`].
pub(crate) struct StorySplice {
    part: usize,
    piece: std::sync::Arc<WordPartPiece>,
    new: Vec<WordOwnerAddress>,
    new_inside: Vec<bool>,
}
