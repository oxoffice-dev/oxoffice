//! Direct body blocks replaced in place: the facade changes the live
//! document, and a block replacement splice publishes the owners of the new
//! blocks, touching the owners of the replaced ones and the records of the
//! body items after them, never the rest of the inventory.

use super::*;

/// Which owners of a replaced block keep their states.
#[derive(Clone, Copy)]
pub(crate) enum BlockRetention {
    /// One paragraph edited where it stands: its owner keeps its owner and
    /// properties states, and what it holds is born again (decision 1 of
    /// the one-block publication spec).
    EditedParagraphKeepsItsOwner,
}

/// What [`PyDocument::publish_body_blocks_replaced`] did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BlocksPublication {
    /// The new blocks are the old ones: nothing published, every handle stays.
    Unchanged,
    /// The owners of the new blocks are published.
    Published,
    /// A shape this splice does not take: nothing changed in the inventory,
    /// and the caller puts the old blocks back and takes the whole route.
    WholeRoute,
}

/// The old block of a swap the facade made, held until the publication
/// commits: any other exit (a refusal, an error, a panic) puts it back.
struct SwappedBlock<'a> {
    document: &'a mut PyDocument,
    slot: usize,
    old: Option<oxoffice_docx_oxml::BodyContent>,
}

impl SwappedBlock<'_> {
    /// The publication committed: the edited block stays.
    fn keep(&mut self) {
        self.old = None;
    }
}

impl Drop for SwappedBlock<'_> {
    fn drop(&mut self) {
        if let Some(old) = self.old.take() {
            self.document
                .inner
                .swap_body_block(self.slot, old)
                .expect("the swapped slot is still in the body");
        }
    }
}

/// What the block replacement needs of the replaced block, read from the
/// inventory and the pieces.
struct ReplacedBlock {
    slot: usize,
    /// The block's root owner and its body story item.
    selected: usize,
    location: oxoffice_docx::ContentLocation,
    /// The owners the root owner holds, and the body story items of the block.
    runs: Vec<usize>,
    items: usize,
}

impl PyDocument {
    /// Apply `edit` to the direct body paragraph at facade index `index` in
    /// place and publish it with [`Self::publish_body_blocks_replaced`]: the
    /// paragraph keeps its owner and properties states, its runs retire and
    /// the new ones are born, and every other owner keeps its states.
    /// `Ok(None)` before or after the edit leaves the document and every
    /// handle as they were: the caller then takes its own route. An edit
    /// that fails leaves them as they were too, and its error is returned.
    pub(crate) fn edit_body_paragraph_in_place<T>(
        &mut self,
        py: Python<'_>,
        index: usize,
        operation: &'static str,
        edit: impl FnOnce(&mut oxoffice_docx::Paragraph<'_>) -> oxoffice_docx::Result<T>,
    ) -> PyResult<Option<T>> {
        self.ensure_word_authority(py)?;
        // A paragraph the splice would not take keeps its route before the
        // edit runs, so that the edit runs once.
        let Some(slot) = self.inner.content_index_of_paragraph(index) else {
            return Ok(None);
        };
        if self
            .replaced_block(slot, &self.inner.body_children()[slot])
            .is_none()
        {
            return Ok(None);
        }
        let Some((value, old)) = self
            .inner
            .edit_body_paragraph_in_place(index, edit)
            .map_err(|error| docx_to_pyerr(py, error))?
        else {
            return Ok(None);
        };
        let mut swapped = SwappedBlock {
            document: self,
            slot,
            old: Some(oxoffice_docx_oxml::BodyContent::Paragraph(Box::new(old))),
        };
        let SwappedBlock { document, old, .. } = &mut swapped;
        let published = document.publish_body_blocks_replaced(
            py,
            slot,
            std::slice::from_ref(old.as_ref().expect("held old block")),
            1,
            operation,
            BlockRetention::EditedParagraphKeepsItsOwner,
        )?;
        if published == BlocksPublication::WholeRoute {
            // Dropping the guard puts the old paragraph back.
            return Ok(None);
        }
        swapped.keep();
        Ok(Some(value))
    }

    /// Publish the direct body blocks at `slot..slot + inserted`, which the
    /// facade already put in place of `previous` in the live document, with
    /// the owners of the new blocks alone: read from those blocks
    /// (`word_block_owners`), the owners of the old ones found from their
    /// root owner through the parent maps, and the records of the later body
    /// items moved by the change in body story items. `retention` says which
    /// owners keep their states.
    ///
    /// Shapes taken: one block replaced by one (`previous.len() == 1`,
    /// `inserted == 1`), both direct body paragraphs holding no table, cell
    /// story, text box or hyperlink, with
    /// [`BlockRetention::EditedParagraphKeepsItsOwner`]. Any other shape, an
    /// inventory whose body records are not anchored, pieces not aligned with
    /// `previous`, or a change in body story items when a later item holds
    /// a hyperlink answers [`BlocksPublication::WholeRoute`] with nothing
    /// changed: the caller puts `previous` back and takes the whole route.
    /// An error leaves the inventory unchanged too.
    pub(crate) fn publish_body_blocks_replaced(
        &mut self,
        py: Python<'_>,
        slot: usize,
        previous: &[oxoffice_docx_oxml::BodyContent],
        inserted: usize,
        operation: &'static str,
        retention: BlockRetention,
    ) -> PyResult<BlocksPublication> {
        let BlockRetention::EditedParagraphKeepsItsOwner = retention;
        let ([old], 1) = (previous, inserted) else {
            return Ok(BlocksPublication::WholeRoute);
        };
        let Some(new) = self.inner.body_children().get(slot) else {
            return Ok(BlocksPublication::WholeRoute);
        };
        if new == old {
            return Ok(BlocksPublication::Unchanged);
        }
        self.ensure_word_authority(py)?;
        let Some(target) = self.replaced_block(slot, old) else {
            return Ok(BlocksPublication::WholeRoute);
        };
        #[cfg(debug_assertions)]
        let before = {
            let mut document = self.inner.clone_for_staging();
            document
                .swap_body_block(slot, old.clone())
                .expect("the replaced slot is in the body");
            (document, self.owners.clone())
        };
        let published = self.replace_paragraph_owners(&target, operation)?;
        #[cfg(debug_assertions)]
        if published == BlocksPublication::Published {
            self.assert_paragraph_replacement(&before, &target);
        }
        Ok(published)
    }

    /// The body slot of the paragraph at `location`, from the inventory
    /// alone, when it is a direct body paragraph that a block replacement
    /// can publish ([`Self::publishable_body_slot`]). The caller ensured the
    /// Word authority.
    pub(crate) fn publishable_paragraph_slot(&self, location: &ParagraphLocation) -> Option<usize> {
        let ParagraphLocation::Body(index) = location else {
            return None;
        };
        let native = self
            .indexes
            .body_paragraphs
            .as_ref()?
            .get(*index)?
            .as_ref()?
            .location();
        let key = self
            .indexes
            .find(&self.owners, &WordOwnerLocation::Paragraph(native))?;
        let slot = self.owners[key].body_slot()?;
        (self.indexes.direct_body_paragraph(slot) == Some(key) && self.publishable_body_slot(slot))
            .then_some(slot)
    }

    /// Whether the direct body block at `slot` is a paragraph that a block
    /// replacement can publish. The caller ensured the Word authority.
    pub(crate) fn publishable_body_slot(&self, slot: usize) -> bool {
        self.inner
            .body_children()
            .get(slot)
            .is_some_and(|block| self.replaced_block(slot, block).is_some())
    }

    /// Publish an in-place edit that changed the direct body paragraph at
    /// `slot` (it was `previous`) and added `story` in a related part (a
    /// note or a comment, the last entry of its part): the paragraph keeps
    /// its owner and properties states and its runs are born again, as
    /// [`Self::publish_body_blocks_replaced`] publishes them, then the
    /// story's owners are born as [`Self::splice_story_owners`] publishes
    /// them; every other owner keeps its states.
    ///
    /// [`BlocksPublication::WholeRoute`] when either half cannot be taken,
    /// with nothing published. Both halves publish under one revision,
    /// prepared and reserved before it commits: an error leaves the
    /// inventory as it was, and nothing after the commit fails. Under
    /// debug assertions the whole edit is checked once both are published
    /// (each half alone sees the other's change unpublished).
    pub(crate) fn publish_body_paragraph_and_story(
        &mut self,
        py: Python<'_>,
        slot: usize,
        previous: &oxoffice_docx_oxml::BodyContent,
        story: &oxoffice_docx::StoryId,
        operation: &'static str,
    ) -> PyResult<BlocksPublication> {
        match self.inner.body_children().get(slot) {
            Some(new) if new != previous => {}
            _ => return Ok(BlocksPublication::WholeRoute),
        }
        self.ensure_word_authority(py)?;
        let Some(target) = self.replaced_block(slot, previous) else {
            return Ok(BlocksPublication::WholeRoute);
        };
        // A part that does not read alone takes the whole route, as one
        // that reads otherwise does.
        let Some(splice) = self.read_story_splice(py, story).ok().flatten() else {
            return Ok(BlocksPublication::WholeRoute);
        };
        let Some(plan) = self.plan_paragraph_owners(&target)? else {
            return Ok(BlocksPublication::WholeRoute);
        };
        #[cfg(debug_assertions)]
        let before = {
            let mut document = self.inner.clone_for_staging();
            document
                .swap_body_block(slot, previous.clone())
                .expect("the replaced slot is in the body");
            (document, self.owners.clone())
        };
        // One revision retires and gives birth for both halves: whatever can
        // fail comes before its commit, so both halves publish or neither.
        let (story_retired, story_births) = self.story_splice_scopes(story, &splice);
        let mut retired = plan.retired.clone();
        retired.extend(story_retired);
        retired.push(
            self.snapshot_scope
                .as_ref()
                .expect("active Word authority")
                .clone(),
        );
        let paragraph_births = plan.births.len();
        let mut births = plan.births.clone();
        births.extend(story_births);
        births.push(RevisionScope::Text);
        let revision = self
            .revisions
            .prepare_local_scopes(operation, &retired, &births)
            .map_err(revision_to_pyerr)?;
        let states = (0..births.len())
            .map(|index| revision.born(index).expect("prepared birth"))
            .collect::<Vec<_>>();
        self.reserve_paragraph_owners(std::slice::from_ref(&plan))?;
        self.reserve_story_splice(&splice)?;
        debug_fail_paragraph_splice()?;
        self.revisions
            .commit_scopes(revision)
            .map_err(revision_to_pyerr)?;
        // The commit: nothing below fails.
        let (snapshot, states) = states.split_last().expect("snapshot birth");
        let (paragraph_states, story_states) = states.split_at(paragraph_births);
        self.apply_paragraph_owners(&target, plan, paragraph_states);
        self.apply_story_splice(story, splice, story_states);
        self.snapshot_scope = Some(snapshot.clone());
        #[cfg(debug_assertions)]
        self.assert_paragraph_replacement(&before, &target);
        Ok(BlocksPublication::Published)
    }

    /// Publish an in-place removal of `stories` (a note, or the comments of
    /// a thread, all of one part; identities before the removal) that also
    /// changed the direct body paragraphs `blocks` (each a slot and the
    /// paragraph it held before), one paragraph for one: each paragraph
    /// keeps its owner and properties states and its runs are born again,
    /// as [`Self::publish_body_blocks_replaced`] publishes them, the owners
    /// of the removed stories retire, and every other owner keeps its
    /// states, the stories after a removed one followed to their new owner
    /// index ([`Self::read_removed_story_splice`]).
    ///
    /// [`BlocksPublication::WholeRoute`] when either half cannot be taken
    /// (a changed block that is not such a paragraph, a part that does not
    /// tell the removal alone, several paragraphs one of which changes its
    /// body item count), with nothing published. Both halves publish under
    /// one revision, prepared and reserved before it commits: an error
    /// leaves the inventory as it was, and nothing after the commit fails.
    /// Under debug assertions the whole edit is checked once both are
    /// published against `before`, the document before the edit.
    pub(crate) fn publish_body_blocks_and_removed_stories(
        &mut self,
        py: Python<'_>,
        blocks: &[(usize, &oxoffice_docx_oxml::BodyContent)],
        stories: &[oxoffice_docx::StoryId],
        operation: &'static str,
        #[cfg(debug_assertions)] before: &oxoffice_docx::Document,
    ) -> PyResult<BlocksPublication> {
        self.ensure_word_authority(py)?;
        let mut targets = Vec::new();
        targets
            .try_reserve(blocks.len())
            .map_err(allocation_failed)?;
        for &(slot, previous) in blocks {
            match self.inner.body_children().get(slot) {
                Some(new) if new != previous => {}
                _ => return Ok(BlocksPublication::WholeRoute),
            }
            let Some(target) = self.replaced_block(slot, previous) else {
                return Ok(BlocksPublication::WholeRoute);
            };
            targets.push(target);
        }
        let Some(splice) = self.read_removed_story_splice(py, stories).ok().flatten() else {
            return Ok(BlocksPublication::WholeRoute);
        };
        let mut plans = Vec::new();
        plans
            .try_reserve(targets.len())
            .map_err(allocation_failed)?;
        for target in &targets {
            let Some(plan) = self.plan_paragraph_owners(target)? else {
                return Ok(BlocksPublication::WholeRoute);
            };
            plans.push(plan);
        }
        // A paragraph that changes its body item count moves the items of
        // the later ones, which their plans read before it: one at most.
        if plans.len() > 1 && plans.iter().any(|plan| plan.items != 0) {
            return Ok(BlocksPublication::WholeRoute);
        }
        // Each paragraph's owner and runs, by generation: applying one
        // plan removes owners by swap, which moves the keys of the others.
        let generations = targets
            .iter()
            .map(|target| {
                (
                    self.owners[target.selected].owner.generation(),
                    target
                        .runs
                        .iter()
                        .map(|&run| self.owners[run].owner.generation())
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        #[cfg(debug_assertions)]
        let old_owners = self.owners.clone();
        let mut retired = Vec::new();
        let mut births = Vec::new();
        let mut paragraph_births = Vec::new();
        for plan in &plans {
            retired.extend(plan.retired.iter().cloned());
            births.extend_from_slice(&plan.births);
            paragraph_births.push(plan.births.len());
        }
        retired.extend(self.removed_story_splice_scopes(&splice));
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
            .map(|index| revision.born(index).expect("prepared birth"))
            .collect::<Vec<_>>();
        self.reserve_paragraph_owners(&plans)?;
        debug_fail_paragraph_splice()?;
        self.revisions
            .commit_scopes(revision)
            .map_err(revision_to_pyerr)?;
        // The commit: nothing below fails.
        let (snapshot, mut states) = states.split_last().expect("snapshot birth");
        for ((target, plan), (owner, runs)) in targets.iter().zip(plans).zip(&generations) {
            let (plan_states, rest) = states.split_at(plan.births.len());
            states = rest;
            let by_generation = &self.indexes.by_generation;
            let target = ReplacedBlock {
                selected: by_generation[owner],
                runs: runs.iter().map(|run| by_generation[run]).collect(),
                location: target.location.clone(),
                ..*target
            };
            self.apply_paragraph_owners(&target, plan, plan_states);
        }
        self.apply_removed_story_splice(stories, splice);
        self.snapshot_scope = Some(snapshot.clone());
        #[cfg(debug_assertions)]
        self.assert_removal_publication(before, old_owners, &generations);
        Ok(BlocksPublication::Published)
    }

    /// Under debug assertions, after [`Self::publish_body_blocks_and_removed_stories`]:
    /// the inventory equals its rebuild from the whole document, every kept
    /// owner but the edited paragraphs names its own element, and each
    /// edited paragraph kept its owner while its runs were born again.
    #[cfg(debug_assertions)]
    fn assert_removal_publication(
        &self,
        before: &oxoffice_docx::Document,
        mut old_owners: Vec<WordOwner>,
        edited: &[(u64, Vec<u64>)],
    ) {
        self.assert_word_inventory_rebuilds(false);
        for (owner, runs) in edited {
            assert!(
                self.indexes.by_generation.contains_key(owner),
                "an edited paragraph keeps its owner"
            );
            assert!(
                runs.iter()
                    .all(|run| !self.indexes.by_generation.contains_key(run)),
                "an edited paragraph's runs are born again"
            );
        }
        // The edited paragraphs name their new element: the check of the
        // kept owners sets them aside.
        old_owners.retain(|owner| {
            !edited
                .iter()
                .any(|(edited, _)| owner.owner.generation() == *edited)
        });
        assert_kept_owners_name_their_elements(
            (before, &old_owners),
            (&self.inner, &self.owners),
            None,
        );
    }

    /// The inventory facts of the direct body block at `slot`, whose block
    /// is (or was, before an edit in place) `block`, when a block replacement
    /// can publish it: a paragraph whose body item and owners are anchored
    /// and indexed, whose piece is aligned with `block`, and which holds no
    /// text box, table, cell story or hyperlink.
    fn replaced_block(
        &self,
        slot: usize,
        block: &oxoffice_docx_oxml::BodyContent,
    ) -> Option<ReplacedBlock> {
        let indexes = &self.indexes;
        let pieces = self.pieces.as_ref()?;
        let blocks = self.inner.body_children();
        if !indexes.items.ready()
            || pieces.context.as_ref() != Some(&self.inner.body_item_context())
            || pieces.blocks.len() != blocks.len()
            || !matches!(block, oxoffice_docx_oxml::BodyContent::Paragraph(_))
        {
            return None;
        }
        let selected = indexes.direct_body_paragraph(slot)?;
        let owner = &self.owners[selected];
        let WordOwnerLocation::Paragraph(location) = &*owner.location() else {
            return None;
        };
        let [first] = *location.index_path() else {
            return None;
        };
        if owner.body_slot() != Some(slot) {
            return None;
        }
        let item = indexes.items.at(first)?;
        let piece = pieces.blocks.get(slot)?;
        if !aligned_block(piece, block) {
            return None;
        }
        let facts = piece.owners.as_ref()?;
        let runs = indexes
            .runs_by_parent
            .get(&selected)
            .cloned()
            .unwrap_or_default();
        if !plain_block(facts) {
            return None;
        }
        // What an aligned piece of a paragraph without link, text box or
        // table tells of its owners: its runs alone, all on its own item.
        #[cfg(debug_assertions)]
        {
            let holds = |map: &IndexMap<usize, Vec<usize>>| {
                map.get(&selected)
                    .is_some_and(|children| !children.is_empty())
            };
            assert_eq!(facts.facts.paragraphs[0].2.len(), runs.len(), "piece runs");
            assert!(
                !holds(&indexes.paragraphs_by_parent)
                    && !holds(&indexes.tables_by_parent)
                    && !holds(&indexes.cells_by_parent),
                "a paragraph owns runs alone"
            );
            assert_eq!((item.counts(), item.links()), ((1, 0, 0), 0), "item record");
            assert!(
                !indexes.items.any_between(first, first + facts.body_items),
                "a paragraph's later items hold no owner"
            );
        }
        #[cfg(not(debug_assertions))]
        let _ = item;
        Some(ReplacedBlock {
            slot,
            selected,
            location: location.clone(),
            runs,
            items: facts.body_items,
        })
    }

    /// Publish the paragraph now at `target`'s slot, which replaced the
    /// block `target` read, keeping the paragraph's owner.
    fn replace_paragraph_owners(
        &mut self,
        target: &ReplacedBlock,
        operation: &'static str,
    ) -> PyResult<BlocksPublication> {
        let Some(plan) = self.plan_paragraph_owners(target)? else {
            return Ok(BlocksPublication::WholeRoute);
        };
        let mut retired = plan.retired.clone();
        retired.push(
            self.snapshot_scope
                .as_ref()
                .expect("active Word authority")
                .clone(),
        );
        let mut births = plan.births.clone();
        births.push(RevisionScope::Text);
        let revision = self
            .revisions
            .prepare_local_scopes(operation, &retired, &births)
            .map_err(revision_to_pyerr)?;
        let states = (0..births.len())
            .map(|index| revision.born(index).expect("prepared birth"))
            .collect::<Vec<_>>();
        self.reserve_paragraph_owners(std::slice::from_ref(&plan))?;
        debug_fail_paragraph_splice()?;
        self.revisions
            .commit_scopes(revision)
            .map_err(revision_to_pyerr)?;
        // The commit: nothing below fails.
        let (snapshot, states) = states.split_last().expect("snapshot birth");
        self.apply_paragraph_owners(target, plan, states);
        self.snapshot_scope = Some(snapshot.clone());
        Ok(BlocksPublication::Published)
    }

    /// What publishing the paragraph now at `target`'s slot retires and
    /// gives birth to, the snapshot scope aside, read before anything
    /// commits; `None` when a block replacement does not take it. Decision 1
    /// of the one-block publication spec: the paragraph's content retires,
    /// its runs retire and the new ones are born.
    fn plan_paragraph_owners(&self, target: &ReplacedBlock) -> PyResult<Option<ParagraphPlan>> {
        let block = &self.inner.body_children()[target.slot];
        if !matches!(block, oxoffice_docx_oxml::BodyContent::Paragraph(_)) {
            return Ok(None);
        }
        let Some(owners) = word_block_owners(&self.inner, block).ok().flatten() else {
            return Ok(None);
        };
        if !plain_block(&owners) {
            return Ok(None);
        }
        let (text, runs) = {
            let (_, text, runs) = &owners.facts.paragraphs[0];
            (text.clone(), runs.clone())
        };
        let first = target.location.index_path()[0];
        let Some(items) = isize::try_from(owners.body_items)
            .ok()
            .zip(isize::try_from(target.items).ok())
            .map(|(after, before)| after - before)
        else {
            return Ok(None);
        };
        // Decision 1 retires the hyperlinks of later items when the item
        // count changes: the whole route publishes that.
        if items != 0 && self.indexes.items.links_after(first) {
            return Ok(None);
        }
        let selected = &self.owners[target.selected];
        let mut retired = Vec::new();
        retired
            .try_reserve(target.runs.len() * 2 + 1)
            .map_err(allocation_failed)?;
        retired.push(selected.content.clone().expect("a paragraph has content"));
        for run in &target.runs {
            retired.extend(self.owners[*run].scope_states().cloned());
        }
        let mut births = Vec::new();
        births
            .try_reserve(runs.len() * 2 + 1)
            .map_err(allocation_failed)?;
        births.push(RevisionScope::Text);
        for _ in &runs {
            births.extend_from_slice(&[RevisionScope::Owner, RevisionScope::Text]);
        }
        Ok(Some(ParagraphPlan {
            owners,
            text,
            runs,
            first,
            items,
            retired,
            births,
        }))
    }

    /// Reserve what [`Self::apply_paragraph_owners`] adds for `plans`, so
    /// that nothing fails once the revision commits.
    fn reserve_paragraph_owners(&mut self, plans: &[ParagraphPlan]) -> PyResult<()> {
        let runs = plans.iter().map(|plan| plan.runs.len()).sum();
        self.owners.try_reserve(runs).map_err(allocation_failed)?;
        self.indexes
            .by_generation
            .try_reserve(runs)
            .map_err(allocation_failed)?;
        self.indexes
            .by_route
            .try_reserve(runs)
            .map_err(allocation_failed)?;
        self.indexes
            .runs_by_parent
            .try_reserve(plans.len())
            .map_err(allocation_failed)?;
        Ok(())
    }

    /// Install the owners `plan` read, under `states` (the births of
    /// `plan.births`, in order), after the revision committed.
    fn apply_paragraph_owners(
        &mut self,
        target: &ReplacedBlock,
        plan: ParagraphPlan,
        states: &[RevisionState],
    ) {
        let ParagraphPlan {
            owners,
            text,
            runs,
            first,
            items,
            ..
        } = plan;
        let block = &self.inner.body_children()[target.slot];
        let piece = std::sync::Arc::new(WordBlockPiece {
            block: std::sync::Arc::new(block.clone()),
            owners: Some(owners),
        });
        let generation = self.owners[target.selected].owner.generation();
        let paragraph = &mut self.owners[target.selected];
        paragraph.content = Some(states[0].clone());
        paragraph.read_facts = WordReadFacts::Paragraph(text);
        self.remove_owners(&target.runs);
        // The removal may have moved the paragraph's owner to a freed key.
        let selected = self.indexes.by_generation[&generation];
        for (run, path) in runs.into_iter().enumerate() {
            let owner = WordOwner::new(
                WordOwnerAddress {
                    location: WordOwnerLocation::Run(target.location.clone(), path),
                    parent: Some(selected),
                    body_slot: None,
                    projection_ordinal: None,
                    read_facts: WordReadFacts::None,
                },
                &states[1 + 2 * run..3 + 2 * run],
                &self.indexes.items,
            );
            self.indexes.index_owner(self.owners.len(), &owner);
            self.owners.push(owner);
        }
        self.indexes.items.shift_after(first, items);
        self.pieces.as_mut().expect("active pieces").blocks[target.slot] = piece;
    }

    /// Under debug assertions, after a block replacement: the inventory
    /// equals its rebuild from the whole document, the block's read equals
    /// the whole part's, every kept owner names its own element, and the
    /// owners kept and retired are those the whole route keeps and retires.
    #[cfg(debug_assertions)]
    fn assert_paragraph_replacement(
        &self,
        (document, owners): &(oxoffice_docx::Document, Vec<WordOwner>),
        target: &ReplacedBlock,
    ) {
        self.assert_word_inventory_rebuilds(false);
        let story = target.location.story();
        let path = target.location.index_path();
        let pieces = self.pieces.as_ref().expect("active pieces");
        let mut bases = (0, 0, 0);
        for piece in &pieces.blocks[..target.slot] {
            let owners = piece.owners.as_ref().expect("aligned pieces have owners");
            bases.0 += owners.body_items;
            bases.1 += owners.text_boxes;
            bases.2 += owners.cell_stories;
        }
        let read = |document: &oxoffice_docx::Document| {
            let extent = document
                .block_element_extent(target.slot, bases, story, Some(path))
                .expect("a block element extent reads")
                .expect("the block holds the paragraph");
            let mut whole = document
                .element_extent(story, Some(path))
                .expect("an element extent reads")
                .expect("the part holds the paragraph");
            whole
                .links
                .retain(|(route, _, _)| extent.read.contains(&route[0]));
            whole.read = extent.read.clone();
            assert_eq!(extent, whole, "block element extent");
            extent
        };
        let shift = BlockShift::of(story, read(document), read(&self.inner))
            .expect("a paragraph edit changes its paragraph alone");
        let selected = &owners[target.selected];
        assert_kept_owners_name_their_elements(
            (document, owners),
            (&self.inner, &self.owners),
            Some((&shift, &selected.owner)),
        );
        // The retention of the whole route, from a walk of the document.
        let locations =
            word_owner_locations(&self.inner, None).expect("an edited document has a walk");
        let matches = indexed_inventory_matches(
            owners,
            &locations,
            &WordPublication::ParagraphEdit(target.location.clone()),
        );
        let keep = block_retention(owners, &locations, &matches, target.selected, &shift);
        for (index, address) in locations.iter().enumerate() {
            let key = self
                .indexes
                .find(&self.owners, &address.location)
                .expect("every walked owner is indexed");
            let states = self.owners[key].states();
            for (slot, kept) in keep[index].iter().enumerate() {
                let same = matches[index]
                    .is_some_and(|before| owners[before].states()[slot].same_state(&states[slot]));
                assert_eq!(
                    same, *kept,
                    "a paragraph replacement keeps what the whole route keeps"
                );
            }
        }
    }
}

/// Whether the owner facts of a body block are those of one paragraph with
/// runs alone: no table, cell story, text box or hyperlink.
fn plain_block(owners: &WordBlockOwners) -> bool {
    owners.tables == 0
        && owners.cell_stories == 0
        && owners.text_boxes == 0
        && owners.body_items >= 1
        && owners.facts.links.is_empty()
        && owners.facts.cells.is_empty()
        && owners.facts.paragraphs.len() == 1
        && owners.facts.paragraphs[0].0.index_path() == [0]
}

impl PyDocument {
    /// `Document.split_run` on the direct body paragraph at `body_index`: in
    /// place when a block replacement publishes it, else through the staged
    /// route, which also gives the errors for a slot that is no paragraph.
    pub(super) fn split_run_in_place_or_staged(
        &mut self,
        py: Python<'_>,
        body_index: usize,
        run_index: usize,
        character_offset: usize,
    ) -> PyResult<usize> {
        if let Some(index) = self.inner.paragraph_index_of_content(body_index)
            && let Some(boundary) =
                self.edit_body_paragraph_in_place(py, index, "Document.split_run", |paragraph| {
                    paragraph.split_run(run_index, character_offset)
                })?
        {
            return Ok(boundary);
        }
        self.split_body_run(py, body_index, run_index, character_offset)
    }
}

/// What [`PyDocument::plan_paragraph_owners`] read of a replaced paragraph.
struct ParagraphPlan {
    owners: WordBlockOwners,
    text: String,
    runs: Vec<oxoffice_docx::AcceptedRunPath>,
    first: usize,
    items: isize,
    /// The states that retire and the scopes born, the snapshot scope aside.
    retired: Vec<RevisionState>,
    births: Vec<RevisionScope>,
}

fn allocation_failed(_: std::collections::TryReserveError) -> PyErr {
    pyo3::exceptions::PyMemoryError::new_err("Word paragraph splice allocation failed")
}

/// Debug builds can fail or panic before a paragraph publication commits,
/// on request, so that tests see the old paragraph put back.
pub(super) fn debug_fail_paragraph_splice() -> PyResult<()> {
    #[cfg(debug_assertions)]
    match std::env::var("OXOFFICE_DEBUG_FAIL_PARAGRAPH_SPLICE").as_deref() {
        Ok("error") => {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                "paragraph splice failed on request",
            ));
        }
        Ok("panic") => panic!("paragraph splice panicked on request"),
        _ => {}
    }
    Ok(())
}
