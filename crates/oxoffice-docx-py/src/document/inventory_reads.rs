//! Reading through the owner inventory: scopes, indexed locations, routes, direct slots and cached reads.

use super::*;

impl PyDocument {
    pub(super) fn section_child_scope(&self, index: usize) -> PyResult<RevisionState> {
        self.owners
            .iter()
            .find(
                |owner| matches!(owner.location,WordOwnerLocation::Section(value) if value==index),
            )
            .and_then(|owner| owner.content.clone())
            .ok_or_else(|| PyValueError::new_err("section furniture owner is absent"))
    }

    fn prepared_section_child_scope(
        &self,
        prepared: &PreparedWordRevision,
        index: usize,
    ) -> PyResult<RevisionState> {
        self.revisions
            .capture_prepared(&prepared.revision, smallvec![])
            .map_err(revision_to_pyerr)?;
        prepared
            .owners
            .iter()
            .find(
                |owner| matches!(owner.location,WordOwnerLocation::Section(value) if value==index),
            )
            .and_then(|owner| owner.content.clone())
            .ok_or_else(|| PyValueError::new_err("prepared section furniture owner is absent"))
    }

    pub(crate) fn prepared_furniture_scope(
        &self,
        prepared: &PreparedWordRevision,
        scope: Option<&RevisionState>,
    ) -> PyResult<Option<RevisionState>> {
        let Some(scope) = scope else {
            return Ok(None);
        };
        self.revisions
            .capture_prepared(&prepared.revision, smallvec![])
            .map_err(revision_to_pyerr)?;
        let index = self
            .owners
            .iter()
            .find_map(|owner| match owner.location {
                WordOwnerLocation::Section(index)
                    if owner
                        .content
                        .as_ref()
                        .is_some_and(|content| content.same_state(scope)) =>
                {
                    Some(index)
                }
                _ => None,
            })
            .ok_or_else(|| {
                PyValueError::new_err("furniture authority has no current section owner")
            })?;
        self.prepared_section_child_scope(prepared, index).map(Some)
    }

    pub(super) fn indexed_owner(&self, state: &RevisionState) -> Option<(usize, &WordOwner)> {
        let index = *self.indexes.by_generation.get(&state.generation())?;
        let owner = self.owners.get(index)?;
        owner.owner.same_state(state).then_some((index, owner))
    }

    /// The route of the physical owner of a paragraph handle's location,
    /// from the inventory of `prepared` when given, or the current one.
    pub(crate) fn indexed_paragraph_location(
        &self,
        location: &ParagraphLocation,
        prepared: Option<&PreparedWordRevision>,
    ) -> Option<oxoffice_docx::ContentLocation> {
        let (owners, indexes) = prepared.map_or((&self.owners, &self.indexes), |prepared| {
            (&prepared.owners, &prepared.indexes)
        });
        let native = match location {
            ParagraphLocation::Body(index) => {
                indexes.body_paragraphs.as_ref()?.get(*index)?.as_ref()?
            }
            ParagraphLocation::Cell {
                table,
                row,
                cell,
                paragraph,
            } => indexes
                .cell_paragraphs
                .get(&(*table, *row, *cell))?
                .get(*paragraph)?,
            ParagraphLocation::Related(location) => location,
        };
        let index = indexes.find(owners, &WordOwnerLocation::Paragraph(native.clone()))?;
        let WordOwnerLocation::Paragraph(location) = &owners[index].location else {
            return None;
        };
        Some(location.clone())
    }

    /// The route of the physical owner of a paragraph handle's location in
    /// the current document, or in `candidate` as `prepared` publishes it:
    /// from that inventory, which holds the route the document resolves, or
    /// as the document resolves it.
    pub(crate) fn paragraph_route(
        &self,
        location: &ParagraphLocation,
        prepared: Option<(&PreparedWordRevision, &oxoffice_docx::Document)>,
    ) -> oxoffice_docx::Result<oxoffice_docx::ContentLocation> {
        let document = prepared.map_or(&self.inner, |(_, candidate)| candidate);
        let indexed =
            self.indexed_paragraph_location(location, prepared.map(|(prepared, _)| prepared));
        match indexed {
            Some(indexed) => {
                #[cfg(debug_assertions)]
                assert_indexed_route(&indexed, || location.native(document), "paragraph");
                Ok(indexed)
            }
            None => location.native(document),
        }
    }

    /// The route of the physical owner of a table handle's location in the
    /// current document: from the inventory, which holds the route the
    /// document resolves, or as the document resolves it.
    pub(crate) fn table_route(
        &self,
        index: usize,
        related: &Option<oxoffice_docx::ContentLocation>,
    ) -> oxoffice_docx::Result<oxoffice_docx::ContentLocation> {
        let native = || crate::table::TableLocation::new(index, related).native(&self.inner);
        let indexed = self.indexed_table_location(index, related, None);
        match indexed {
            Some(indexed) => {
                // The walk of a related table only reads its story again,
                // so the walk checks a body table alone.
                #[cfg(debug_assertions)]
                if related.is_none() {
                    assert_indexed_route(&indexed, native, "table");
                }
                Ok(indexed)
            }
            None => native(),
        }
    }

    /// The route and run path of a run handle's physical owner: from the
    /// inventory record of that owner, or as the document resolves it.
    pub(crate) fn run_route(
        &self,
        state: &RevisionState,
    ) -> PyResult<(
        oxoffice_docx::ContentLocation,
        oxoffice_docx_oxml::text::AcceptedRunPath,
    )> {
        if let Some((_, owner)) = self.indexed_owner(state)
            && let WordOwnerLocation::Run(location, path) = &owner.location
        {
            // The story walk of a recorded route only reads its story again,
            // so the check is that the paragraph there holds this run path.
            #[cfg(debug_assertions)]
            assert!(
                (|| {
                    let current =
                        ParagraphLocation::Related(location.clone()).native(&self.inner)?;
                    self.inner.read_story_paragraph(&current, |paragraph| {
                        (0..paragraph.run_count())
                            .any(|index| paragraph.run_path(index).as_ref() == Some(path))
                    })
                })()
                .expect("an indexed run route resolves in the document"),
                "indexed run route holds its run path"
            );
            return Ok((location.clone(), path.clone()));
        }
        self.current_run_owner(state)
    }

    /// The direct body slot of the paragraph that a body `location` and the
    /// run owner `state` both name, from the inventory. A typed edit made
    /// through `location` then changes that block and no other.
    pub(crate) fn typed_run_edit_slot(
        &self,
        location: &ParagraphLocation,
        state: &RevisionState,
    ) -> Option<usize> {
        if !matches!(location, ParagraphLocation::Body(_)) {
            return None;
        }
        let (_, run) = self.indexed_owner(state)?;
        let paragraph = self.owners.get(run.parent?)?;
        let WordOwnerLocation::Paragraph(route) = &paragraph.location else {
            return None;
        };
        let indexed = self.indexed_paragraph_location(location, None)?;
        (route.story().kind() == oxoffice_docx::StoryKind::Body
            && route.index_path().len() == 1
            && same_content_owner(route, &indexed))
        .then_some(paragraph.body_slot?)
    }

    pub(crate) fn indexed_table_location(
        &self,
        index: usize,
        related: &Option<oxoffice_docx::ContentLocation>,
        prepared: Option<&PreparedWordRevision>,
    ) -> Option<oxoffice_docx::ContentLocation> {
        let (owners, indexes) = prepared.map_or((&self.owners, &self.indexes), |prepared| {
            (&prepared.owners, &prepared.indexes)
        });
        let native = match related {
            Some(location) => location,
            None => indexes.body_tables.get(index)?.as_ref()?,
        };
        let index = indexes.find(owners, &WordOwnerLocation::Table(native.clone()))?;
        let WordOwnerLocation::Table(location) = &owners[index].location else {
            return None;
        };
        Some(location.clone())
    }

    pub(crate) fn indexed_table_owner_location(
        &self,
        state: &RevisionState,
    ) -> Option<oxoffice_docx::ContentLocation> {
        let (_, owner) = self.indexed_owner(state)?;
        let WordOwnerLocation::Table(location) = &owner.location else {
            return None;
        };
        Some(location.clone())
    }

    pub(crate) fn indexed_cell_parent_location(
        &self,
        state: &RevisionState,
    ) -> Option<oxoffice_docx::ContentLocation> {
        let (_, owner) = self.indexed_owner(state)?;
        let WordOwnerLocation::Cell { table, .. } = &owner.location else {
            return None;
        };
        Some(table.clone())
    }

    pub(crate) fn indexed_cell_location(
        &self,
        table: &oxoffice_docx::ContentLocation,
        row: usize,
        cell: usize,
    ) -> Option<WordOwnerLocation> {
        // Cell route equivalence intentionally ignores the story fingerprint.
        // Return the actual current inventory value, never the lookup placeholder.
        let key = WordOwnerLocation::Cell {
            table: table.clone(),
            row,
            cell,
            story: oxoffice_docx::StoryId::body(),
        };
        let index = self.indexes.find(&self.owners, &key)?;
        Some(self.owners[index].location.clone())
    }

    pub(crate) fn capture_current_view(
        &self,
        state: &RevisionState,
        segments: smallvec::SmallVec<[PathSeg; 5]>,
        view: WordView,
        furniture: Option<&RevisionState>,
    ) -> PyResult<ContentPath> {
        let (index, _) = self
            .indexed_owner(state)
            .ok_or_else(|| PyValueError::new_err("physical owner is no longer current"))?;
        word_owner_capture(
            &self.revisions,
            &self.owners,
            (
                self.revisions.current_document_scope(),
                self.removal_scope.as_ref().expect("active Word authority"),
            ),
            index,
            segments,
            view,
            furniture,
        )
        .map(|(path, _)| path)
    }

    /// The direct body slot of a current paragraph owner, from the inventory.
    pub(crate) fn direct_body_paragraph_slot(&self, state: &RevisionState) -> Option<usize> {
        let (_, owner) = self.indexed_owner(state)?;
        match &owner.location {
            WordOwnerLocation::Paragraph(location)
                if location.story().kind() == oxoffice_docx::StoryKind::Body
                    && location.index_path().len() == 1 =>
            {
                owner.body_slot
            }
            _ => None,
        }
    }

    /// The direct body slot of a current run owner's paragraph, and the run
    /// route in it, from the inventory.
    pub(crate) fn direct_body_run_slot(
        &self,
        state: &RevisionState,
    ) -> Option<(usize, oxoffice_docx_oxml::text::AcceptedRunPath)> {
        let (_, owner) = self.indexed_owner(state)?;
        let WordOwnerLocation::Run(location, path) = &owner.location else {
            return None;
        };
        let paragraph = self.owners.get(owner.parent?)?;
        match &paragraph.location {
            WordOwnerLocation::Paragraph(parent)
                if parent.story().kind() == oxoffice_docx::StoryKind::Body
                    && parent.index_path().len() == 1
                    && parent.index_path() == location.index_path() =>
            {
                Some((paragraph.body_slot?, path.clone()))
            }
            _ => None,
        }
    }

    /// The direct body slot of a current cell owner's table, and the cell
    /// coordinates, from the inventory.
    pub(crate) fn direct_body_cell_slot(
        &self,
        state: &RevisionState,
    ) -> Option<(usize, usize, usize)> {
        let (_, owner) = self.indexed_owner(state)?;
        let WordOwnerLocation::Cell {
            table, row, cell, ..
        } = &owner.location
        else {
            return None;
        };
        let parent = self.owners.get(owner.parent?)?;
        match &parent.location {
            WordOwnerLocation::Table(location)
                if location.story().kind() == oxoffice_docx::StoryKind::Body
                    && location.index_path().len() == 1
                    && location.index_path() == table.index_path() =>
            {
                Some((parent.body_slot?, *row, *cell))
            }
            _ => None,
        }
    }

    /// The direct body slot of a current table owner, from the inventory.
    pub(crate) fn direct_body_table_slot(&self, state: &RevisionState) -> Option<usize> {
        let (_, owner) = self.indexed_owner(state)?;
        match &owner.location {
            WordOwnerLocation::Table(location)
                if location.story().kind() == oxoffice_docx::StoryKind::Body
                    && location.index_path().len() == 1 =>
            {
                owner.body_slot
            }
            _ => None,
        }
    }

    pub(crate) fn cached_paragraph_text(&self, state: &RevisionState) -> Option<&str> {
        let (_, owner) = self.indexed_owner(state)?;
        match &owner.read_facts {
            WordReadFacts::Paragraph(text) => Some(text),
            _ => None,
        }
    }
}

impl PyDocument {
    pub(crate) fn cached_body_paragraph_count(&self) -> Option<usize> {
        Some(self.indexes.body_paragraphs.as_ref()?.len())
    }

    pub(crate) fn cached_body_table_count(&self) -> usize {
        self.indexes.body_tables.len()
    }

    pub(crate) fn cached_table_rows(&self, state: &RevisionState) -> Option<&[Vec<usize>]> {
        let (_, owner) = self.indexed_owner(state)?;
        match &owner.read_facts {
            WordReadFacts::Table(rows) => Some(rows),
            _ => None,
        }
    }

    pub(crate) fn cached_cell_paragraph_count(&self, state: &RevisionState) -> Option<usize> {
        let (_, owner) = self.indexed_owner(state)?;
        let WordOwnerLocation::Cell { row, cell, .. } = &owner.location else {
            return None;
        };
        let parent = self.owners.get(owner.parent?)?;
        let WordReadFacts::Table(rows) = &parent.read_facts else {
            return None;
        };
        rows.get(*row)?.get(*cell).copied()
    }

    pub(crate) fn cached_cell_paragraph(
        &self,
        state: &RevisionState,
        paragraph: usize,
    ) -> Option<oxoffice_docx::ContentLocation> {
        let (index, _) = self.indexed_owner(state)?;
        let items = self.indexes.paragraphs_by_parent.get(&index)?;
        // A source/model mismatch stays on the original native admission path.
        if items.len() != self.cached_cell_paragraph_count(state)? {
            return None;
        }
        let WordOwnerLocation::Paragraph(location) = &self.owners[*items.get(paragraph)?].location
        else {
            return None;
        };
        Some(location.clone())
    }

    pub(crate) fn cached_cell_tables(
        &self,
        state: &RevisionState,
    ) -> Option<Vec<oxoffice_docx::ContentLocation>> {
        let (index, owner) = self.indexed_owner(state)?;
        if !matches!(owner.location, WordOwnerLocation::Cell { .. }) {
            return None;
        }
        Some(
            self.indexes
                .tables_by_parent
                .get(&index)
                .into_iter()
                .flatten()
                .map(|index| {
                    let WordOwnerLocation::Table(location) = &self.owners[*index].location else {
                        unreachable!("table index contains tables");
                    };
                    location.clone()
                })
                .collect(),
        )
    }
}
