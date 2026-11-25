//! Direct body blocks replaced in place: the facade changes the live
//! document, and a block replacement splice publishes the owners of the new
//! blocks, touching the owners of the replaced ones and the records of the
//! body items after them, never the rest of the inventory.

use super::blocks_spliced::BlocksSplice;
use super::*;

/// Which owners of a replaced block keep their states.
#[derive(Clone, Copy)]
pub(crate) enum BlockRetention {
    /// One paragraph edited where it stands: its owner keeps its owner and
    /// properties states, and what it holds is born again (decision 1 of
    /// the one-block publication spec).
    EditedParagraphKeepsItsOwner,
    /// Blocks removed or inserted: their owners retire or are born, every
    /// other owner keeps its states, and the document epoch and the
    /// snapshot retire ("body handles follow their element", decision 2).
    Spliced,
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

/// A block replacement planned with no state changed.
pub(crate) enum BlocksPlan {
    /// The new blocks are the old ones.
    Unchanged,
    /// One paragraph edited where it stands.
    Paragraph(ReplacedBlock, Box<ParagraphPlan>),
    /// Blocks removed or inserted.
    Spliced(Box<BlocksSplice>),
}

impl BlocksPlan {
    /// The states the publication retires, the snapshot scope aside.
    pub(crate) fn retired(&self) -> &[RevisionState] {
        match self {
            Self::Unchanged => &[],
            Self::Paragraph(_, plan) => &plan.retired,
            Self::Spliced(plan) => &plan.retired,
        }
    }

    /// The scopes the publication gives birth to, the snapshot scope aside.
    pub(crate) fn births(&self) -> &[RevisionScope] {
        match self {
            Self::Unchanged => &[],
            Self::Paragraph(_, plan) => &plan.births,
            Self::Spliced(plan) => &plan.births,
        }
    }

    /// Whether the document epoch survives (a revision prepared local): an
    /// edit keeps it, a splice that removes or inserts blocks retires it.
    pub(crate) fn keeps_epoch(&self) -> bool {
        !matches!(self, Self::Spliced(_))
    }

    /// Debug builds fail or panic before the commit on request, so that tests
    /// see the old blocks put back.
    pub(crate) fn debug_fail(&self) -> PyResult<()> {
        match self {
            Self::Spliced(_) => debug_fail_block_splice(),
            _ => debug_fail_paragraph_splice(),
        }
    }
}

/// What the debug oracle of a block replacement needs of its plan.
#[cfg(debug_assertions)]
#[derive(Clone)]
pub(crate) enum BlocksSummary {
    Unchanged,
    Paragraph(ReplacedBlock),
    /// The owners retired and born.
    Spliced {
        retired: usize,
        born: usize,
    },
}

#[cfg(debug_assertions)]
impl BlocksPlan {
    /// What the debug oracle needs, taken before the commit consumes the plan.
    pub(crate) fn summary(&self) -> BlocksSummary {
        match self {
            Self::Unchanged => BlocksSummary::Unchanged,
            Self::Paragraph(target, _) => BlocksSummary::Paragraph(target.clone()),
            Self::Spliced(plan) => {
                let (retired, born) = plan.counts();
                BlocksSummary::Spliced { retired, born }
            }
        }
    }
}

/// The document and the inventory before a block replacement.
#[cfg(debug_assertions)]
pub(crate) type BlocksDebugCopy = (oxoffice_docx::Document, Vec<WordOwner>);

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
#[derive(Clone)]
pub(crate) struct ReplacedBlock {
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
    /// Shapes taken: with [`BlockRetention::EditedParagraphKeepsItsOwner`],
    /// one block replaced by one (`previous.len() == 1`, `inserted == 1`),
    /// both direct body paragraphs holding no table, cell story, text box or
    /// hyperlink; with [`BlockRetention::Spliced`], any number of direct body
    /// paragraphs, tables and block content controls replaced by any number
    /// of them, none holding a text box or a section break. Any other shape, an
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
        self.ensure_word_authority(py)?;
        let plan = match self.plan_blocks_replaced(slot, previous, inserted, retention)? {
            None => return Ok(BlocksPublication::WholeRoute),
            Some(BlocksPlan::Unchanged) => return Ok(BlocksPublication::Unchanged),
            Some(plan) => plan,
        };
        #[cfg(debug_assertions)]
        let (before, summary) = (
            self.blocks_replaced_debug_copy(slot, previous, inserted),
            plan.summary(),
        );
        // One revision retires and gives birth for the blocks and the
        // snapshot; a caller composing another splice adds its own scopes
        // to the same revision.
        let mut retired = plan.retired().to_vec();
        retired.push(
            self.snapshot_scope
                .as_ref()
                .expect("active Word authority")
                .clone(),
        );
        let mut births = plan.births().to_vec();
        births.push(RevisionScope::Text);
        let revision = if plan.keeps_epoch() {
            self.revisions
                .prepare_local_scopes(operation, &retired, &births)
        } else {
            self.revisions.prepare_scopes(operation, &retired, &births)
        }
        .map_err(revision_to_pyerr)?;
        let states = (0..births.len())
            .map(|index| revision.born(index).expect("prepared birth"))
            .collect::<Vec<_>>();
        self.reserve_blocks_replaced(&plan)?;
        plan.debug_fail()?;
        self.revisions
            .commit_scopes(revision)
            .map_err(revision_to_pyerr)?;
        // The commit: nothing below fails.
        let (snapshot, states) = states.split_last().expect("snapshot birth");
        self.apply_blocks_replaced(plan, states);
        self.snapshot_scope = Some(snapshot.clone());
        #[cfg(debug_assertions)]
        self.assert_blocks_replaced(&before, &summary);
        Ok(BlocksPublication::Published)
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
        // A part that does not read alone takes the whole route, as one
        // that reads otherwise does.
        let Some(splice) = self.read_story_splice(py, story).ok().flatten() else {
            return Ok(BlocksPublication::WholeRoute);
        };
        let Some(plan @ BlocksPlan::Paragraph(..)) = self.plan_blocks_replaced(
            slot,
            std::slice::from_ref(previous),
            1,
            BlockRetention::EditedParagraphKeepsItsOwner,
        )?
        else {
            return Ok(BlocksPublication::WholeRoute);
        };
        #[cfg(debug_assertions)]
        let (before, summary) = (
            self.blocks_replaced_debug_copy(slot, std::slice::from_ref(previous), 1),
            plan.summary(),
        );
        // One revision retires and gives birth for both halves: whatever can
        // fail comes before its commit, so both halves publish or neither.
        let (story_retired, story_births) = self.story_splice_scopes(story, &splice);
        let mut retired = plan.retired().to_vec();
        retired.extend(story_retired);
        retired.push(
            self.snapshot_scope
                .as_ref()
                .expect("active Word authority")
                .clone(),
        );
        let paragraph_births = plan.births().len();
        let mut births = plan.births().to_vec();
        births.extend(story_births);
        births.push(RevisionScope::Text);
        let revision = self
            .revisions
            .prepare_local_scopes(operation, &retired, &births)
            .map_err(revision_to_pyerr)?;
        let states = (0..births.len())
            .map(|index| revision.born(index).expect("prepared birth"))
            .collect::<Vec<_>>();
        self.reserve_blocks_replaced(&plan)?;
        self.reserve_story_splice(&splice)?;
        plan.debug_fail()?;
        self.revisions
            .commit_scopes(revision)
            .map_err(revision_to_pyerr)?;
        // The commit: nothing below fails.
        let (snapshot, states) = states.split_last().expect("snapshot birth");
        let (paragraph_states, story_states) = states.split_at(paragraph_births);
        self.apply_blocks_replaced(plan, paragraph_states);
        self.apply_story_splice(story, splice, story_states);
        self.snapshot_scope = Some(snapshot.clone());
        #[cfg(debug_assertions)]
        self.assert_blocks_replaced(&before, &summary);
        Ok(BlocksPublication::Published)
    }

    /// The plan of [`Self::publish_body_blocks_replaced`], read from the
    /// inventory, the pieces and the new blocks with no state changed:
    /// `None` for the whole route, [`BlocksPlan::Unchanged`] for new blocks
    /// equal to the old ones (nothing to publish, every handle stays). The
    /// caller has ensured the Word authority. Debug builds check here that
    /// the old owners found by route are the blocks' closure.
    ///
    /// A caller composing this publication with its own: put
    /// [`BlocksPlan::retired`] and [`BlocksPlan::births`] in its revision
    /// (prepared local when [`BlocksPlan::keeps_epoch`], else not), call
    /// [`Self::reserve_blocks_replaced`] and its own reservations, commit the
    /// revision once, then [`Self::apply_blocks_replaced`] with the states
    /// born for [`BlocksPlan::births`], in order, its own application and
    /// the snapshot scope; under debug assertions, take
    /// [`Self::blocks_replaced_debug_copy`] before and call
    /// [`Self::assert_blocks_replaced`] after every application.
    pub(crate) fn plan_blocks_replaced(
        &self,
        slot: usize,
        previous: &[oxoffice_docx_oxml::BodyContent],
        inserted: usize,
        retention: BlockRetention,
    ) -> PyResult<Option<BlocksPlan>> {
        if let BlockRetention::Spliced = retention {
            let Some(plan) = self.plan_blocks_splice(slot, previous, inserted) else {
                return Ok(None);
            };
            #[cfg(debug_assertions)]
            self.assert_old_owners_are_the_blocks_closure(&plan);
            return Ok(Some(BlocksPlan::Spliced(Box::new(plan))));
        }
        let ([old], 1) = (previous, inserted) else {
            return Ok(None);
        };
        let Some(new) = self.inner.body_children().get(slot) else {
            return Ok(None);
        };
        if new == old {
            return Ok(Some(BlocksPlan::Unchanged));
        }
        let Some(target) = self.replaced_block(slot, old) else {
            return Ok(None);
        };
        Ok(self
            .plan_paragraph_owners(&target)?
            .map(|plan| BlocksPlan::Paragraph(target, Box::new(plan))))
    }

    /// Reserve what [`Self::apply_blocks_replaced`] adds, so that nothing
    /// fails once the revision commits.
    pub(crate) fn reserve_blocks_replaced(&mut self, plan: &BlocksPlan) -> PyResult<()> {
        match plan {
            BlocksPlan::Unchanged => Ok(()),
            BlocksPlan::Paragraph(_, plan) => self.reserve_paragraph_owners(plan),
            BlocksPlan::Spliced(plan) => self.reserve_blocks_splice(plan),
        }
    }

    /// Install `plan` after its revision committed, under `states`: the
    /// states born for [`BlocksPlan::births`], in order. Nothing here fails;
    /// the caller sets the snapshot scope. No debug oracle runs here.
    pub(crate) fn apply_blocks_replaced(&mut self, plan: BlocksPlan, states: &[RevisionState]) {
        match plan {
            BlocksPlan::Unchanged => {}
            BlocksPlan::Paragraph(target, plan) => {
                self.apply_paragraph_owners(&target, *plan, states);
            }
            BlocksPlan::Spliced(plan) => self.apply_blocks_splice(*plan, states),
        }
    }

    /// Under debug assertions, the document and the inventory as they were
    /// before the blocks at `slot..slot + inserted` replaced `previous`, for
    /// [`Self::assert_blocks_replaced`]. Take it before the commit.
    #[cfg(debug_assertions)]
    pub(crate) fn blocks_replaced_debug_copy(
        &self,
        slot: usize,
        previous: &[oxoffice_docx_oxml::BodyContent],
        inserted: usize,
    ) -> BlocksDebugCopy {
        let mut document = self.inner.clone_for_staging();
        document
            .splice_body_blocks(slot, inserted, previous.to_vec())
            .expect("the replaced slots are in the body");
        (document, self.owners.clone())
    }

    /// Under debug assertions, after a block replacement was applied and any
    /// application that goes with it: the oracles of the plan's shape (the
    /// rebuild, kept owners naming their elements, and the retention of the
    /// whole route or of a splice).
    #[cfg(debug_assertions)]
    pub(crate) fn assert_blocks_replaced(&self, before: &BlocksDebugCopy, summary: &BlocksSummary) {
        match summary {
            BlocksSummary::Unchanged => {}
            BlocksSummary::Paragraph(target) => self.assert_paragraph_replacement(before, target),
            BlocksSummary::Spliced { retired, born } => {
                self.assert_blocks_splice(before, *retired, *born);
            }
        }
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

    /// Reserve what [`Self::apply_paragraph_owners`] adds, so that nothing
    /// fails once the revision commits.
    fn reserve_paragraph_owners(&mut self, plan: &ParagraphPlan) -> PyResult<()> {
        let runs = plan.runs.len();
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
            .try_reserve(1)
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
pub(crate) struct ParagraphPlan {
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
fn debug_fail_paragraph_splice() -> PyResult<()> {
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

/// Debug builds can fail or panic before a block splice commits, on
/// request, so that tests see the old blocks put back.
fn debug_fail_block_splice() -> PyResult<()> {
    #[cfg(debug_assertions)]
    match std::env::var("OXOFFICE_DEBUG_FAIL_BLOCK_SPLICE").as_deref() {
        Ok("error") => {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                "block splice failed on request",
            ));
        }
        Ok("panic") => panic!("block splice panicked on request"),
        _ => {}
    }
    Ok(())
}
