//! One direct body paragraph edited in place: the facade edits a copy of
//! the paragraph alone, and a block replacement splice publishes its owners,
//! touching the paragraph's owners and the records of the body items after
//! it, never the rest of the inventory.

use super::*;

/// The old paragraph of a swap the facade made, held until the publication
/// commits: any other exit (a refusal, an error, a panic) puts it back.
struct SwappedParagraph<'a> {
    document: &'a mut PyDocument,
    index: usize,
    old: Option<oxoffice_docx_oxml::CT_P>,
}

impl SwappedParagraph<'_> {
    /// The publication committed: the edited paragraph stays.
    fn keep(&mut self) -> oxoffice_docx_oxml::CT_P {
        self.old.take().expect("a swapped paragraph is kept once")
    }
}

impl Drop for SwappedParagraph<'_> {
    fn drop(&mut self) {
        if let Some(old) = self.old.take() {
            self.document
                .inner
                .restore_body_paragraph(self.index, old)
                .expect("the swapped paragraph's index still names it");
        }
    }
}

/// What the block replacement needs of the edited paragraph before the
/// edit runs, read from the inventory and the pieces.
struct ParagraphSlot {
    slot: usize,
    /// The paragraph's owner and its body story item.
    selected: usize,
    location: oxoffice_docx::ContentLocation,
    /// The owners of its runs, and the body story items it holds.
    runs: Vec<usize>,
    items: usize,
}

impl PyDocument {
    /// Apply `edit` to the direct body paragraph at facade index `index` in
    /// place and publish its owners alone (decision 2 of the in-place spec):
    /// the paragraph keeps its owner and properties states, its runs retire
    /// and the new ones are born, and every other owner keeps its states.
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
        let Some(target) = self.paragraph_slot(index) else {
            return Ok(None);
        };
        #[cfg(debug_assertions)]
        let before = (self.inner.clone_for_staging(), self.owners.clone());
        let Some((value, old)) = self
            .inner
            .edit_body_paragraph_in_place(index, edit)
            .map_err(|error| docx_to_pyerr(py, error))?
        else {
            return Ok(None);
        };
        let mut swapped = SwappedParagraph {
            document: self,
            index,
            old: Some(old),
        };
        let published = swapped.document.replace_paragraph_owners(
            &target,
            swapped.old.as_ref().expect("held old paragraph"),
            operation,
        )?;
        if published.is_none() {
            // Dropping the guard puts the old paragraph back.
            return Ok(None);
        }
        swapped.keep();
        drop(swapped);
        #[cfg(debug_assertions)]
        if published == Some(true) {
            self.assert_paragraph_replacement(&before, &target);
        }
        Ok(Some(value))
    }

    /// The inventory facts of the direct body paragraph at facade index
    /// `index`, when a block replacement can publish an edit of it: its
    /// body item and owners are anchored and indexed, its piece is aligned,
    /// and it holds no text box, table, cell story or hyperlink.
    fn paragraph_slot(&self, index: usize) -> Option<ParagraphSlot> {
        let indexes = &self.indexes;
        let pieces = self.pieces.as_ref()?;
        let blocks = self.inner.body_children();
        if !indexes.items.ready()
            || pieces.context.as_ref() != Some(&self.inner.body_item_context())
            || pieces.blocks.len() != blocks.len()
        {
            return None;
        }
        let slot = self.inner.content_index_of_paragraph(index)?;
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
        if !aligned_block(piece, &blocks[slot]) {
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
        Some(ParagraphSlot {
            slot,
            selected,
            location: location.clone(),
            runs,
            items: facts.body_items,
        })
    }

    /// Publish the paragraph now at `target`'s slot, which replaced `old`:
    /// `Some(false)` when it is what it was (nothing is published, every
    /// handle stays), `Some(true)` when its owners were replaced, `None`
    /// when this edit needs the whole route; then nothing changed here.
    fn replace_paragraph_owners(
        &mut self,
        target: &ParagraphSlot,
        old: &oxoffice_docx_oxml::CT_P,
        operation: &'static str,
    ) -> PyResult<Option<bool>> {
        let block = &self.inner.body_children()[target.slot];
        let oxoffice_docx_oxml::BodyContent::Paragraph(new) = block else {
            return Ok(None);
        };
        if new.as_ref() == old {
            return Ok(Some(false));
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
        // The paragraph's content retires; its runs retire, the new ones are
        // born, and the snapshot scope is born again.
        let selected = &self.owners[target.selected];
        let fail =
            |_| pyo3::exceptions::PyMemoryError::new_err("Word paragraph splice allocation failed");
        let mut retired = Vec::new();
        retired
            .try_reserve(target.runs.len() * 2 + 2)
            .map_err(fail)?;
        retired.push(selected.content.clone().expect("a paragraph has content"));
        for run in &target.runs {
            retired.extend(self.owners[*run].scope_states().cloned());
        }
        retired.push(
            self.snapshot_scope
                .as_ref()
                .expect("active Word authority")
                .clone(),
        );
        let mut births = Vec::new();
        births.try_reserve(runs.len() * 2 + 2).map_err(fail)?;
        births.push(RevisionScope::Text);
        for _ in &runs {
            births.extend_from_slice(&[RevisionScope::Owner, RevisionScope::Text]);
        }
        births.push(RevisionScope::Text);
        let revision = self
            .revisions
            .prepare_local_scopes(operation, &retired, &births)
            .map_err(revision_to_pyerr)?;
        self.owners.try_reserve(runs.len()).map_err(fail)?;
        self.indexes
            .by_generation
            .try_reserve(runs.len())
            .map_err(fail)?;
        self.indexes
            .by_route
            .try_reserve(runs.len())
            .map_err(fail)?;
        self.indexes.runs_by_parent.try_reserve(1).map_err(fail)?;
        let born = |index: usize| revision.born(index).expect("prepared birth");
        let content = born(0);
        let run_states = (0..runs.len())
            .map(|run| [born(1 + 2 * run), born(2 + 2 * run)])
            .collect::<Vec<_>>();
        let snapshot_scope = born(births.len() - 1);
        let piece = std::sync::Arc::new(WordBlockPiece {
            block: std::sync::Arc::new(block.clone()),
            owners: Some(owners),
        });
        let generation = self.owners[target.selected].owner.generation();
        // Debug builds can fail or panic here on request, so that tests see
        // the old paragraph put back.
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
        self.revisions
            .commit_scopes(revision)
            .map_err(revision_to_pyerr)?;
        // The commit: nothing below fails.
        let paragraph = &mut self.owners[target.selected];
        paragraph.content = Some(content);
        paragraph.read_facts = WordReadFacts::Paragraph(text);
        self.remove_owners(&target.runs);
        // The removal may have moved the paragraph's owner to a freed key.
        let selected = self.indexes.by_generation[&generation];
        for (path, states) in runs.into_iter().zip(run_states) {
            let owner = WordOwner::new(
                WordOwnerAddress {
                    location: WordOwnerLocation::Run(target.location.clone(), path),
                    parent: Some(selected),
                    body_slot: None,
                    projection_ordinal: None,
                    read_facts: WordReadFacts::None,
                },
                &states,
                &self.indexes.items,
            );
            self.indexes.index_owner(self.owners.len(), &owner);
            self.owners.push(owner);
        }
        self.indexes.items.shift_after(first, items);
        self.snapshot_scope = Some(snapshot_scope);
        self.pieces.as_mut().expect("active pieces").blocks[target.slot] = piece;
        Ok(Some(true))
    }

    /// Under debug assertions, after a block replacement: the inventory
    /// equals its rebuild from the whole document, the block's read equals
    /// the whole part's, every kept owner names its own element, and the
    /// owners kept and retired are those the whole route keeps and retires.
    #[cfg(debug_assertions)]
    fn assert_paragraph_replacement(
        &self,
        (document, owners): &(oxoffice_docx::Document, Vec<WordOwner>),
        target: &ParagraphSlot,
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
