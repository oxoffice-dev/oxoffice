//! Splicing the body: splice plans, prepared splices, and the owners they
//! insert or remove. A splice moves the records of the body items after it
//! (`WordBodyItems`), never the owners of other blocks.

use super::*;

/// One direct body item that a splice inserts, with what the owner
/// inventory reads for it.
pub(crate) enum SplicedBodyItem {
    /// A plain paragraph's accepted text and run paths.
    Paragraph(String, Vec<oxoffice_docx_oxml::text::AcceptedRunPath>),
    /// A table's namespace-complete XML and its owner snapshots in a body
    /// that holds only that table.
    Table(Vec<u8>, WordOwnerSnapshots),
}

/// An appended paragraph whose publication is ready: what remains cannot
/// fail before the paragraph reaches the document.
pub(crate) enum StagedAppend {
    /// A prepared body splice and the handle of the paragraph it appends.
    Splice(Box<PreparedBodySplice>, Py<PyParagraph>),
    /// A candidate that holds the paragraph, its revision committed, and the
    /// paragraph's handle.
    Revision(Box<oxoffice_docx::Document>, Py<PyParagraph>),
}

/// Plain direct body paragraphs removed from a body slot, or one item
/// inserted at it.
pub(crate) struct BodySplice {
    pub(crate) slot: usize,
    pub(crate) removed: usize,
    pub(crate) inserted: Option<SplicedBodyItem>,
}

/// A splice of the current owner inventory for one direct body edit.
///
/// The inserted item is one body story item, a plain paragraph or a table
/// with no nested table, text box or link, and the removed items are plain
/// paragraphs. So only the body items after the slot move, by the items
/// inserted less those removed, and only the cell stories after an inserted
/// table, by its cells: their records move, and their owners stay as they
/// are.
pub(crate) struct BodySplicePlan {
    slot: usize,
    removed: usize,
    // The body story item index of the splice slot.
    first: usize,
    // The inserted owners, routed as the document reads them after the
    // splice, with parents among themselves.
    inserted: Vec<WordOwnerAddress>,
    // The body paragraphs, body tables and cell stories the inserted item
    // holds.
    counts: (usize, usize, usize),
    // The keys of the removed paragraphs and their runs.
    removed_owners: Vec<usize>,
    // The facade paragraph and table indexes of the splice slot.
    pub(crate) paragraph: usize,
    pub(crate) table: usize,
    // The inserted table's route and the routes of its cell paragraphs.
    table_entry: Option<(oxoffice_docx::ContentLocation, Vec<CellParagraphs>)>,
    piece: Option<WordBlockOwners>,
}

/// The facade coordinates of a cell and the routes of its paragraphs.
type CellParagraphs = ((usize, usize), Vec<oxoffice_docx::ContentLocation>);

/// A prepared body splice: its revision, and the scope states of the
/// inserted owners, in order.
pub(crate) struct PreparedBodySplice {
    pub(crate) revision: PreparedRevision,
    pub(crate) plan: BodySplicePlan,
    states: Vec<RevisionState>,
    snapshot_scope: RevisionState,
    // The document and the inventory before the native edit, for the
    // identity oracle of the kept owners.
    #[cfg(debug_assertions)]
    before: Box<(oxoffice_docx::Document, Vec<WordOwner>)>,
}

/// Binary search over projection records: the index of the first one `pred`
/// rejects, as `slice::partition_point` finds it.
fn location_partition_point(
    items: &[Option<AnchoredRoute>],
    mut pred: impl FnMut(&Option<AnchoredRoute>) -> bool,
) -> usize {
    let mut left = 0;
    let mut right = items.len();
    while left < right {
        let mid = left + (right - left) / 2;
        if pred(&items[mid]) {
            left = mid + 1;
        } else {
            right = mid;
        }
    }
    left
}

/// The body story item index at the head of a projection route.
fn projected_head(route: &Option<AnchoredRoute>) -> Option<usize> {
    route.as_ref().and_then(AnchoredRoute::head)
}

impl PyDocument {
    /// Plan `splice` from the current inventory alone, reading the slot's
    /// body item and the projection entries around it. `None` keeps the
    /// whole route for every case this does not prove, and debug builds
    /// check each committed splice against a full walk.
    pub(crate) fn plan_body_splice(
        &mut self,
        py: Python<'_>,
        splice: BodySplice,
    ) -> PyResult<Option<BodySplicePlan>> {
        self.ensure_word_authority(py)?;
        let indexes = &self.indexes;
        let items = &indexes.items;
        let Some(body_paragraphs) = &indexes.body_paragraphs else {
            return Ok(None);
        };
        let content_count = self.inner.content_count();
        let Some(end_slot) = splice.slot.checked_add(splice.removed) else {
            return Ok(None);
        };
        let inserted_table = matches!(splice.inserted, Some(SplicedBodyItem::Table(..)));
        if end_slot > content_count
            || (splice.removed == 0) == splice.inserted.is_none()
            || !items.ready()
        {
            return Ok(None);
        }
        // An inserted table moves the cell stories after it: their owners
        // must follow their items, and every body table must be indexed to
        // place it among them.
        if inserted_table
            && (!items.cells_anchored() || indexes.body_tables.iter().any(Option::is_none))
        {
            return Ok(None);
        }
        if !(splice.slot..end_slot).all(|slot| self.inner.plain_removable_paragraph(slot)) {
            return Ok(None);
        }
        let Some(story) = items.story().cloned() else {
            return Ok(None);
        };
        let first = if let Some(item) = items.at_slot(splice.slot) {
            item.index()
        } else if splice.removed == 0 && splice.slot > 0 {
            let Some(previous) = items.at_slot(splice.slot - 1) else {
                return Ok(None);
            };
            let Some(count) = self
                .inner
                .direct_body_story_item_count(splice.slot - 1)
                .ok()
                .flatten()
            else {
                return Ok(None);
            };
            previous.index() + count
        } else {
            return Ok(None);
        };
        // Each removed slot is the direct paragraph owner at its own item,
        // which holds that paragraph and its runs alone.
        let mut removed_owners = Vec::new();
        for offset in 0..splice.removed {
            let Some(key) = indexes.direct_body_paragraph(splice.slot + offset) else {
                return Ok(None);
            };
            match &*self.owners[key].location() {
                WordOwnerLocation::Paragraph(location)
                    if location.index_path() == [first + offset] => {}
                _ => return Ok(None),
            }
            let holds = |map: &IndexMap<usize, Vec<usize>>| {
                map.get(&key).is_some_and(|children| !children.is_empty())
            };
            if holds(&indexes.paragraphs_by_parent)
                || holds(&indexes.tables_by_parent)
                || holds(&indexes.cells_by_parent)
                || items.at(first + offset).map(|item| item.counts()) != Some((1, 0, 0))
            {
                return Ok(None);
            }
            removed_owners.push(key);
            removed_owners.extend(indexes.runs_by_parent.get(&key).into_iter().flatten());
        }
        // Facade paragraphs and body tables keep source order, so the splice
        // is one contiguous run of their entries.
        let paragraph = self.inner.paragraph_count_before_content(splice.slot);
        if paragraph + splice.removed > body_paragraphs.len() {
            return Ok(None);
        }
        for offset in 0..splice.removed {
            let fits = body_paragraphs[paragraph + offset]
                .as_ref()
                .is_some_and(|route| route.location().index_path() == [first + offset]);
            if !fits {
                return Ok(None);
            }
        }
        let end_item = first + splice.removed;
        if body_paragraphs
            .get(paragraph + splice.removed)
            .is_some_and(|next| next.is_some() && projected_head(next) < Some(end_item))
            || paragraph
                .checked_sub(1)
                .is_some_and(|previous| projected_head(&body_paragraphs[previous]) >= Some(first))
        {
            return Ok(None);
        }
        let table = location_partition_point(&indexes.body_tables, |route| {
            projected_head(route) < Some(first)
        });
        if indexes
            .body_tables
            .get(table)
            .is_some_and(|route| projected_head(route).is_some_and(|head| head < end_item))
        {
            return Ok(None);
        }
        let (paragraph_ordinal, table_ordinal, cells_before) = items.before(first);
        let mut plan = BodySplicePlan {
            slot: splice.slot,
            removed: splice.removed,
            first,
            inserted: Vec::new(),
            counts: (0, 0, 0),
            removed_owners,
            paragraph,
            table,
            table_entry: None,
            piece: None,
        };
        match splice.inserted {
            None => {}
            Some(SplicedBodyItem::Paragraph(text, runs)) => {
                plan.piece = Some(plain_paragraph_owners(&story, &text, &runs));
                let location = oxoffice_docx::ContentLocation::new(
                    story,
                    oxoffice_docx::StoryItemKind::Paragraph,
                    vec![first],
                );
                plan.inserted.push(WordOwnerAddress {
                    location: WordOwnerLocation::Paragraph(location.clone()),
                    parent: None,
                    body_slot: Some(splice.slot),
                    projection_ordinal: Some(paragraph_ordinal),
                    read_facts: WordReadFacts::Paragraph(text),
                });
                plan.inserted
                    .extend(runs.into_iter().map(|path| WordOwnerAddress {
                        location: WordOwnerLocation::Run(location.clone(), path),
                        parent: Some(0),
                        body_slot: None,
                        projection_ordinal: None,
                        read_facts: WordReadFacts::None,
                    }));
                plan.counts = (1, 0, 0);
            }
            Some(SplicedBodyItem::Table(xml, snapshots)) => {
                let Some((owners, cells, entry, piece)) = spliced_table(
                    xml,
                    snapshots,
                    &story,
                    (splice.slot, first, table_ordinal),
                    cells_before,
                ) else {
                    return Ok(None);
                };
                plan.inserted = owners;
                plan.counts = (0, 1, cells);
                plan.table_entry = Some(entry);
                plan.piece = piece;
            }
        }
        Ok(Some(plan))
    }

    /// Prepare an append or keeping publication of `plan`: the removed
    /// owners retire, and the inserted ones are born, before the native edit.
    /// Every other owner keeps its states, and its handles follow it.
    pub(crate) fn prepare_body_splice(
        &mut self,
        plan: BodySplicePlan,
        operation: &'static str,
        publication: &WordPublication,
    ) -> PyResult<PreparedBodySplice> {
        if !matches!(
            publication,
            WordPublication::AppendBody | WordPublication::BodySplice
        ) {
            return Err(PyValueError::new_err("body splice publication"));
        }
        #[cfg(debug_assertions)]
        let before = Box::new((self.inner.clone_for_staging(), self.owners.clone()));
        let fail =
            |_| pyo3::exceptions::PyMemoryError::new_err("Word owner splice allocation failed");
        let mut retired = Vec::new();
        retired
            .try_reserve(plan.removed_owners.len() * 3 + 1)
            .map_err(fail)?;
        for key in &plan.removed_owners {
            retired.extend(self.owners[*key].scope_states().cloned());
        }
        retired.push(
            self.snapshot_scope
                .as_ref()
                .expect("active Word authority")
                .clone(),
        );
        let mut births = Vec::new();
        births
            .try_reserve(plan.inserted.len() * 3 + 1)
            .map_err(fail)?;
        for address in &plan.inserted {
            births.extend_from_slice(address.location.scopes());
        }
        births.push(RevisionScope::Text);
        let revision = self
            .revisions
            .prepare_scopes(operation, &retired, &births)
            .map_err(revision_to_pyerr)?;
        let mut states = Vec::new();
        states.try_reserve_exact(births.len() - 1).map_err(fail)?;
        for born in 0..births.len() - 1 {
            states.push(revision.born(born).expect("prepared owner birth"));
        }
        let snapshot_scope = revision
            .born(births.len() - 1)
            .expect("prepared snapshot birth");
        // Capacity for the commit, which places and indexes the inserted owners.
        let added = plan.inserted.len();
        self.owners.try_reserve(added).map_err(fail)?;
        self.indexes
            .by_generation
            .try_reserve(added)
            .map_err(fail)?;
        self.indexes.by_route.try_reserve(added).map_err(fail)?;
        self.indexes.items.reserve(1).map_err(fail)?;
        self.indexes
            .direct_body_paragraphs
            .try_reserve(1)
            .map_err(fail)?;
        if let Some(paragraphs) = &mut self.indexes.body_paragraphs {
            paragraphs.try_reserve(1).map_err(fail)?;
        }
        self.indexes.body_tables.try_reserve(1).map_err(fail)?;
        self.indexes.cell_paragraphs.try_reserve(1).map_err(fail)?;
        Ok(PreparedBodySplice {
            revision,
            plan,
            states,
            snapshot_scope,
            #[cfg(debug_assertions)]
            before,
        })
    }

    /// Capture the first owner a prepared splice inserts, the paragraph or
    /// table it places, as [`Self::capture_prepared_owner`] captures a
    /// prepared owner.
    pub(crate) fn capture_prepared_splice_owner(
        &self,
        prepared: &PreparedBodySplice,
        segments: smallvec::SmallVec<[PathSeg; 5]>,
    ) -> PyResult<(ContentPath, RevisionState)> {
        self.revisions
            .capture_prepared(&prepared.revision, smallvec![])
            .map_err(revision_to_pyerr)?;
        let unbound = || PyValueError::new_err("element has no current checked physical owner");
        let address = prepared
            .plan
            .inserted
            .first()
            .filter(|address| address.parent.is_none())
            .ok_or_else(unbound)?;
        let states = prepared
            .states
            .get(..address.location.scopes().len())
            .ok_or_else(unbound)?;
        let owners = word_owners_with_states(
            vec![WordOwnerAddress {
                read_facts: WordReadFacts::None,
                ..address.clone()
            }],
            states,
            &WordBodyItems::default(),
        );
        let (mut path, owner) = word_owner_capture(
            &self.revisions,
            &owners,
            prepared.revision.document_scope(),
            0,
            segments,
            WordView::Owner,
            None,
        )?;
        path.revision = prepared.revision.committed_serial();
        Ok((path, owner))
    }

    /// Publish a prepared splice after its native edit: move the records of
    /// the body items after it, then append and index the inserted owners,
    /// or remove the removed ones.
    pub(crate) fn commit_body_splice(
        &mut self,
        prepared: PreparedBodySplice,
        publication: &WordPublication,
    ) -> PyResult<()> {
        #[cfg(debug_assertions)]
        if matches!(publication, WordPublication::AppendBody) {
            self.assert_body_append_matches(&prepared.plan);
        }
        #[cfg(not(debug_assertions))]
        let _ = publication;
        let PreparedBodySplice {
            revision,
            mut plan,
            states,
            snapshot_scope,
            #[cfg(debug_assertions)]
            before,
        } = prepared;
        self.revisions
            .commit_scopes(revision)
            .map_err(revision_to_pyerr)?;
        if plan.removed > 0 {
            self.indexes.items.remove(plan.first, plan.removed);
            if let Some(paragraphs) = &mut self.indexes.body_paragraphs {
                paragraphs.drain(plan.paragraph..plan.paragraph + plan.removed);
            }
            self.remove_owners(&plan.removed_owners);
        } else {
            let indexes = &mut self.indexes;
            indexes.items.insert(plan.first, plan.slot, plan.counts);
            if plan.slot <= indexes.direct_body_paragraphs.len() {
                indexes.direct_body_paragraphs.insert(plan.slot, None);
            }
            match &plan.inserted.first().map(|address| &address.location) {
                Some(WordOwnerLocation::Paragraph(location)) => {
                    if let Some(paragraphs) = &mut indexes.body_paragraphs {
                        paragraphs
                            .insert(plan.paragraph, Some(indexes.items.route(location.clone())));
                    }
                }
                Some(WordOwnerLocation::Table(_)) => {
                    let (location, cells) = plan
                        .table_entry
                        .take()
                        .expect("an inserted table has its entry");
                    indexes
                        .body_tables
                        .insert(plan.table, Some(indexes.items.route(location)));
                    let cells = cells
                        .into_iter()
                        .map(|(cell, locations)| {
                            let routes = locations
                                .into_iter()
                                .map(|location| indexes.items.route(location))
                                .collect();
                            (cell, routes)
                        })
                        .collect();
                    indexes.cell_paragraphs.insert(plan.table, cells);
                }
                _ => {}
            }
            // The inserted owners take keys after every current one.
            let base = self.owners.len();
            let mut offset = 0;
            for mut address in plan.inserted.drain(..) {
                let scopes = &states[offset..offset + address.location.scopes().len()];
                offset += scopes.len();
                address.parent = address.parent.map(|parent| base + parent);
                let owner = WordOwner::new(address, scopes, &self.indexes.items);
                self.indexes.index_owner(self.owners.len(), &owner);
                self.owners.push(owner);
            }
        }
        self.snapshot_scope = Some(snapshot_scope);
        let inserted = plan.piece.take();
        self.pieces.as_mut().expect("active pieces").splice(
            &self.inner,
            plan.slot,
            plan.removed,
            inserted,
        );
        #[cfg(debug_assertions)]
        {
            self.assert_word_inventory_rebuilds(false);
            let (document, owners) = &*before;
            assert_kept_owners_name_their_elements(
                (document, owners),
                (&self.inner, &self.owners),
                None,
            );
        }
        Ok(())
    }

    /// Remove the owners at `removed` (keys), and give the others the keys of
    /// their order: the inventory is indexed again, linear in its owners.
    fn remove_owners(&mut self, removed: &[usize]) {
        let mut gone = vec![false; self.owners.len()];
        for key in removed {
            gone[*key] = true;
        }
        let mut next = Vec::with_capacity(gone.len());
        let mut kept = 0;
        for gone in &gone {
            next.push((!gone).then_some(kept));
            kept += usize::from(!gone);
        }
        let mut key = 0;
        self.owners.retain(|_| {
            key += 1;
            !gone[key - 1]
        });
        for owner in &mut self.owners {
            owner.parent = owner
                .parent
                .map(|parent| next[parent].expect("a kept owner's parent is kept"));
        }
        self.indexes.index_owners_again(&self.owners);
    }

    /// Under debug assertions, prove that an append keeps exactly the owners
    /// the rebuild's matcher keeps: each owner of the walk is the current
    /// owner at its route, or one the append inserts. Runs after the native
    /// edit, before the inventory changes.
    #[cfg(debug_assertions)]
    fn assert_body_append_matches(&self, plan: &BodySplicePlan) {
        let rebuilt =
            word_owner_locations(&self.inner, None).expect("a spliced inventory has a rebuild");
        let expected = rebuilt
            .iter()
            .map(|address| self.indexes.find(&self.owners, &address.location))
            .collect::<Vec<_>>();
        assert_eq!(
            expected.iter().filter(|matched| matched.is_none()).count(),
            plan.inserted.len(),
            "an append keeps every owner and adds its own"
        );
        assert_eq!(
            indexed_inventory_matches(&self.owners, &rebuilt, &WordPublication::AppendBody),
            expected,
            "spliced Word owner matches"
        );
    }

    /// Under debug assertions, prove the inventory and its document indexes
    /// equal their rebuild from the current document, story fingerprints
    /// aside, which local edits leave stale in the inventory too: the
    /// owners, their body item records, the route index and the projections.
    /// Unless `in_order`, the owners may come in any order, each known by its
    /// route and its parent's: a splice and a local cell edit append the
    /// owners they add.
    #[cfg(debug_assertions)]
    pub(crate) fn assert_word_inventory_rebuilds(&self, in_order: bool) {
        let address = |address: &WordOwnerAddress| WordOwnerAddress {
            location: unfingerprinted_owner_location(&address.location),
            ..address.clone()
        };
        let current = self
            .owners
            .iter()
            .map(WordOwner::address)
            .collect::<Vec<_>>();
        let actual = current.iter().map(address).collect::<Vec<_>>();
        let walked =
            word_owner_locations(&self.inner, None).expect("a spliced inventory has a rebuild");
        let expected = walked.iter().map(address).collect::<Vec<_>>();
        if in_order {
            assert_eq!(actual, expected, "spliced Word owner inventory");
        } else {
            let records = |addresses: &[WordOwnerAddress]| {
                let mut records = addresses
                    .iter()
                    .map(|address| {
                        format!(
                            "{:?}",
                            (
                                &address.location,
                                address.parent.map(|parent| &addresses[parent].location),
                                address.body_slot,
                                address.projection_ordinal,
                                &address.read_facts,
                            )
                        )
                    })
                    .collect::<Vec<_>>();
                records.sort();
                records
            };
            assert_eq!(records(&actual), records(&expected), "Word owner inventory");
        }
        // The records of the body items, as a walk derives them.
        let projections = WordIndexProjections::of(&self.inner);
        self.indexes
            .items
            .assert_rebuilds(&WordBodyItems::derive(&walked, &projections));
        // Every owner is found at its current route.
        for (key, owner) in self.owners.iter().enumerate() {
            assert_eq!(
                self.indexes.find(&self.owners, &owner.location()),
                Some(key),
                "owner found by its route"
            );
        }
        let rebuilt_items = WordBodyItems::derive(&current, &projections);
        let indexes = WordOwnerIndexes::prepare(&self.owners, rebuilt_items, projections)
            .expect("a spliced inventory has rebuilt indexes");
        let locations = |routes: &[Option<AnchoredRoute>]| {
            routes
                .iter()
                .map(|route| {
                    route
                        .as_ref()
                        .map(|route| unfingerprinted_location(&route.location()))
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            self.indexes.body_paragraphs.as_deref().map(locations),
            indexes.body_paragraphs.as_deref().map(locations),
            "spliced body paragraph index"
        );
        assert_eq!(
            locations(&self.indexes.body_tables),
            locations(&indexes.body_tables),
            "spliced body table index"
        );
        let cells = |cells: &CellParagraphIndex| {
            let mut all = BTreeMap::new();
            for (table, routes) in cells.iter().enumerate() {
                for ((row, cell), routes) in routes {
                    all.insert(
                        (table, *row, *cell),
                        routes
                            .iter()
                            .map(|route| unfingerprinted_location(&route.location()))
                            .collect::<Vec<_>>(),
                    );
                }
            }
            all
        };
        assert_eq!(
            cells(&self.indexes.cell_paragraphs),
            cells(&indexes.cell_paragraphs),
            "spliced cell paragraph index"
        );
        let slots = |slots: &[Option<usize>]| {
            let end = slots
                .iter()
                .rposition(Option::is_some)
                .map_or(0, |last| last + 1);
            slots[..end].to_vec()
        };
        assert_eq!(
            slots(&self.indexes.direct_body_paragraphs),
            slots(&indexes.direct_body_paragraphs),
            "spliced direct body paragraph index"
        );
        // The owner indexes, each list in any order, an empty one as none.
        fn lists<K: Copy + Ord>(map: &IndexMap<K, Vec<usize>>) -> BTreeMap<K, Vec<usize>> {
            map.iter()
                .filter(|(_, list)| !list.is_empty())
                .map(|(key, list)| {
                    let mut list = list.clone();
                    list.sort_unstable();
                    (*key, list)
                })
                .collect()
        }
        assert_eq!(
            self.indexes
                .by_generation
                .iter()
                .collect::<BTreeMap<_, _>>(),
            indexes.by_generation.iter().collect::<BTreeMap<_, _>>(),
            "owner index by generation"
        );
        assert_eq!(
            lists(&self.indexes.by_route),
            lists(&indexes.by_route),
            "owner index by route"
        );
        for (name, actual, expected) in [
            (
                "paragraphs",
                &self.indexes.paragraphs_by_parent,
                &indexes.paragraphs_by_parent,
            ),
            (
                "tables",
                &self.indexes.tables_by_parent,
                &indexes.tables_by_parent,
            ),
            (
                "cells",
                &self.indexes.cells_by_parent,
                &indexes.cells_by_parent,
            ),
            (
                "runs",
                &self.indexes.runs_by_parent,
                &indexes.runs_by_parent,
            ),
        ] {
            assert_eq!(
                lists(actual),
                lists(expected),
                "owner index of {name} by parent"
            );
        }
    }
}

/// The owners of a detached table placed at body story item `first` of slot
/// `slot`, as the projection ordinal `ordinal` among body tables, with cell
/// story owners counted from `cells_before`: the table, its cells, then
/// their paragraphs and runs, with parents among them. Also the number of
/// cells, the table route with the routes of each cell's paragraphs, and the
/// table's piece.
#[allow(clippy::type_complexity)] // The owners, the cell count and the index entry of one table.
fn spliced_table(
    xml: Vec<u8>,
    snapshots: WordOwnerSnapshots,
    story: &oxoffice_docx::StoryId,
    (slot, first, ordinal): (usize, usize, usize),
    cells_before: usize,
) -> Option<(
    Vec<WordOwnerAddress>,
    usize,
    (oxoffice_docx::ContentLocation, Vec<CellParagraphs>),
    Option<WordBlockOwners>,
)> {
    // The detached body holds this table as its only item.
    let table_location = {
        let mut items = snapshots
            .0
            .iter()
            .filter(|item| item.location().story().kind() == oxoffice_docx::StoryKind::Body);
        let item = items.next()?;
        if items.next().is_some()
            || item.location().item_kind() != oxoffice_docx::StoryItemKind::Table
            || item.location().index_path() != [0]
        {
            return None;
        }
        item.location().clone()
    };
    let piece_facts = word_table_facts(&xml).and_then(|rows| {
        let mut facts = WordOwnerFacts {
            cells: snapshots.1.clone(),
            ..WordOwnerFacts::default()
        };
        facts.items.push((table_location.clone(), Some(0), rows));
        for (location, paragraph_xml) in &snapshots.2 {
            let paragraph = oxoffice_docx_oxml::CT_P::from_xml_fragment(paragraph_xml).ok()?;
            facts.paragraphs.push((
                location.clone(),
                paragraph.accepted_text(),
                paragraph.accepted_run_paths(),
            ));
        }
        Some(facts)
    });
    let facts = word_table_facts(&xml)
        .map(|facts| (table_location, facts))
        .into_iter()
        .collect::<HashMap<_, _>>();
    let addresses = word_owner_addresses(snapshots, &facts, None, None).ok()?;
    let cells = addresses
        .iter()
        .skip(1)
        .take_while(|address| matches!(address.location, WordOwnerLocation::Cell { .. }))
        .count();
    // The table, its cells in story order, then only their paragraphs and runs.
    let cell_story = |address: &WordOwnerAddress| match &address.location {
        WordOwnerLocation::Cell { story, .. } => Some(story.owner_index()),
        _ => None,
    };
    let in_cells = |location: &oxoffice_docx::ContentLocation| {
        location.story().kind() == oxoffice_docx::StoryKind::TableCell
            && location.story().owner_index() < cells
    };
    let cell_parent =
        |parent: Option<usize>| parent.is_some_and(|parent| (1..=cells).contains(&parent));
    let shaped = matches!(addresses.first()?.location, WordOwnerLocation::Table(_))
        && addresses[1..=cells]
            .iter()
            .enumerate()
            .all(|(index, address)| {
                cell_story(address) == Some(index) && address.parent == Some(0)
            })
        && addresses[cells + 1..]
            .iter()
            .all(|address| match &address.location {
                WordOwnerLocation::Paragraph(location) => {
                    in_cells(location) && cell_parent(address.parent)
                }
                WordOwnerLocation::Run(location, _) => {
                    in_cells(location) && address.parent.is_some_and(|parent| parent > cells)
                }
                _ => false,
            });
    if !shaped {
        return None;
    }
    let moved = |location: &oxoffice_docx::ContentLocation| {
        let mut story = location.story().clone();
        story.set_owner_index(story.owner_index() + cells_before);
        oxoffice_docx::ContentLocation::new(
            story,
            location.item_kind(),
            location.index_path().to_vec(),
        )
    };
    let table = oxoffice_docx::ContentLocation::new(
        story.clone(),
        oxoffice_docx::StoryItemKind::Table,
        vec![first],
    );
    let mut entries = Vec::with_capacity(cells);
    let mut owners = Vec::with_capacity(addresses.len());
    for (index, address) in addresses.into_iter().enumerate() {
        let location = match &address.location {
            WordOwnerLocation::Table(_) => WordOwnerLocation::Table(table.clone()),
            WordOwnerLocation::Cell {
                row, cell, story, ..
            } => {
                let mut story = story.clone();
                story.set_owner_index(story.owner_index() + cells_before);
                entries.push(((*row, *cell), Vec::new()));
                WordOwnerLocation::Cell {
                    table: table.clone(),
                    row: *row,
                    cell: *cell,
                    story,
                }
            }
            WordOwnerLocation::Paragraph(location) => {
                let location = moved(location);
                entries[address.parent? - 1].1.push(location.clone());
                WordOwnerLocation::Paragraph(location)
            }
            WordOwnerLocation::Run(location, path) => {
                WordOwnerLocation::Run(moved(location), path.clone())
            }
            _ => return None,
        };
        owners.push(if index == 0 {
            WordOwnerAddress {
                location,
                parent: None,
                body_slot: Some(slot),
                projection_ordinal: Some(ordinal),
                read_facts: address.read_facts,
            }
        } else {
            WordOwnerAddress {
                location,
                ..address
            }
        });
    }
    let piece = piece_facts.and_then(|facts| table_owners(facts, cells));
    Some((owners, cells, (table, entries), piece))
}
