//! Edits through the owner inventory: capturing owners and snapshots, preparing and committing edits, and publishing the new inventory.

use super::*;

impl PyDocument {
    pub(crate) fn publish_run_content_candidate(
        &mut self,
        candidate: oxoffice_docx::Document,
        owner: Option<&RevisionState>,
        text: String,
    ) -> PyResult<()> {
        let paragraph = owner
            .map(|state| {
                let (_, run) = self.indexed_owner(state).ok_or_else(|| {
                    PyValueError::new_err("run physical owner is no longer current")
                })?;
                if !matches!(run.route_shape(), WordOwnerLocation::Run(..)) {
                    return Err(PyValueError::new_err("run owner has the wrong kind"));
                }
                run.parent
                    .filter(|parent| {
                        self.owners.get(*parent).is_some_and(|owner| {
                            matches!(owner.route_shape(), WordOwnerLocation::Paragraph(_))
                        })
                    })
                    .ok_or_else(|| PyValueError::new_err("run has no current paragraph owner"))
            })
            .transpose()?;
        // These publishers retain physical topology and all existing scopes.
        // Candidate text and parent authority are ready before either value moves.
        if let Some(paragraph) = paragraph {
            self.owners[paragraph].read_facts = WordReadFacts::Paragraph(text);
        }
        self.install_inner(candidate);
        Ok(())
    }
}

impl PyDocument {
    pub(crate) fn capture_owner(
        &self,
        location: &WordOwnerLocation,
        segments: smallvec::SmallVec<[PathSeg; 5]>,
        view: WordView,
        furniture_scope: Option<&RevisionState>,
    ) -> PyResult<(ContentPath, RevisionState)> {
        word_owner_capture(
            &self.revisions,
            &self.owners,
            self.revisions.current_document_scope(),
            self.indexes.find(&self.owners, location).ok_or_else(|| {
                PyValueError::new_err("element has no current checked physical owner")
            })?,
            segments,
            view,
            furniture_scope,
        )
    }

    pub(crate) fn capture_prepared_owner(
        &self,
        prepared: &PreparedWordRevision,
        location: &WordOwnerLocation,
        segments: smallvec::SmallVec<[PathSeg; 5]>,
        view: WordView,
        furniture_scope: Option<&RevisionState>,
    ) -> PyResult<(ContentPath, RevisionState)> {
        // Validate origin and expected serial before using any prepared state.
        self.revisions
            .capture_prepared(&prepared.revision, smallvec![])
            .map_err(revision_to_pyerr)?;
        let (mut path, owner) = word_owner_capture(
            &self.revisions,
            &prepared.owners,
            prepared.revision.document_scope(),
            prepared
                .indexes
                .find(&prepared.owners, location)
                .ok_or_else(|| {
                    PyValueError::new_err("element has no current checked physical owner")
                })?,
            segments,
            view,
            furniture_scope,
        )?;
        path.revision = prepared.revision.committed_serial();
        Ok((path, owner))
    }

    pub(crate) fn capture_snapshot(
        &self,
        segments: smallvec::SmallVec<[PathSeg; 5]>,
    ) -> PyResult<ContentPath> {
        self.revisions
            .capture_scopes(
                segments,
                &[
                    self.revisions.current_document_scope(),
                    self.snapshot_scope
                        .as_ref()
                        .expect("active Word authority")
                        .clone(),
                ],
            )
            .map_err(revision_to_pyerr)
    }

    pub(crate) fn capture_prepared_snapshot(
        &self,
        prepared: &PreparedWordRevision,
        segments: smallvec::SmallVec<[PathSeg; 5]>,
    ) -> PyResult<ContentPath> {
        self.revisions
            .capture_prepared(&prepared.revision, smallvec![])
            .map_err(revision_to_pyerr)?;
        let mut path = self
            .revisions
            .capture_scopes(
                segments,
                &[
                    prepared.revision.document_scope(),
                    prepared.snapshot_scope.clone(),
                ],
            )
            .map_err(revision_to_pyerr)?;
        path.revision = prepared.revision.committed_serial();
        Ok(path)
    }

    pub(crate) fn plain_paragraph_owner(&self, state: &RevisionState) -> Option<(usize, usize)> {
        let selected = *self.indexes.by_generation.get(&state.generation())?;
        let owner = self.owners.get(selected)?;
        if !owner.owner.same_state(state) {
            return None;
        }
        let slot = owner.body_slot()?;
        (self.indexes.direct_body_paragraph(slot) == Some(selected)).then_some((selected, slot))
    }

    pub(super) fn prepare_tail_body_removal(
        &self,
        slot: usize,
    ) -> PyResult<Option<(std::ops::Range<usize>, PreparedRevision)>> {
        if slot.checked_add(1) != Some(self.inner.content_count()) {
            return Ok(None);
        }
        let Some(selected) = self.indexes.direct_body_paragraph(slot) else {
            return Ok(None);
        };
        let mut end = selected + 1;
        while end < self.owners.len() && self.owners[end].parent == Some(selected) {
            end += 1;
        }
        // A removal by swap may have moved an owner of the paragraph's item
        // before it: its runs must all follow it, and its item holds no link.
        let runs = self
            .indexes
            .runs_by_parent
            .get(&selected)
            .map_or(0, Vec::len);
        let item = match &*self.owners[selected].location() {
            WordOwnerLocation::Paragraph(location) => location.index_path().first().copied(),
            _ => None,
        };
        if runs != end - selected - 1
            || item
                .and_then(|item| self.indexes.items.at(item))
                .is_some_and(|item| item.links() != 0)
        {
            return Ok(None);
        }
        for owner in &self.owners[end..] {
            let independent_run = matches!(owner.route_shape(), WordOwnerLocation::Run(..))
                && owner.parent.is_some_and(|parent| parent < selected);
            if !matches!(owner.route_shape(), WordOwnerLocation::Section(_)) && !independent_run {
                return Ok(None);
            }
        }
        // The removed paragraph and its runs retire; every other owner keeps
        // its states, as a splice inside the body keeps them.
        let mut retired = self.owners[selected..end]
            .iter()
            .flat_map(WordOwner::scope_states)
            .cloned()
            .collect::<Vec<_>>();
        retired.push(
            self.snapshot_scope
                .as_ref()
                .expect("active Word authority")
                .clone(),
        );
        let revision = self
            .revisions
            .prepare_scopes("Document.remove_content", &retired, &[RevisionScope::Text])
            .map_err(revision_to_pyerr)?;
        Ok(Some((selected..end, revision)))
    }

    pub(super) fn commit_tail_body_removal(
        &mut self,
        slot: usize,
        range: std::ops::Range<usize>,
        revision: PreparedRevision,
    ) -> PyResult<()> {
        let snapshot_scope = revision.born(0).expect("removal snapshot birth");
        self.revisions
            .commit_scopes(revision)
            .map_err(revision_to_pyerr)?;
        let start = range.start;
        let end = range.end;
        // The removed paragraph's body item goes, the last one.
        if let WordOwnerLocation::Paragraph(location) = &*self.owners[start].location()
            && let Some(item) = location.index_path().first()
        {
            self.indexes.items.remove(*item, 1);
        }
        for (offset, owner) in self.owners.drain(range).enumerate() {
            self.indexes.by_generation.remove(&owner.owner.generation());
            let hash = owner.route_key();
            let bucket = self
                .indexes
                .by_route
                .get_mut(&hash)
                .expect("removed owner route is indexed");
            bucket.retain(|index| *index != start + offset);
            if bucket.is_empty() {
                self.indexes.by_route.remove(&hash);
            }
            self.indexes.paragraphs_by_parent.remove(&(start + offset));
            self.indexes.tables_by_parent.remove(&(start + offset));
            self.indexes.cells_by_parent.remove(&(start + offset));
            self.indexes.runs_by_parent.remove(&(start + offset));
        }
        if let Some(paragraphs) = &mut self.indexes.body_paragraphs {
            paragraphs.pop();
        }
        // Suffix sections and locally born runs of earlier paragraphs have no
        // direct-body route or parent index to shift. Earlier owners stay put.
        for (offset, owner) in self.owners[start..].iter().enumerate() {
            for index in self
                .indexes
                .by_route
                .get_mut(&owner.route_key())
                .expect("surviving suffix route is indexed")
            {
                if *index == end + offset {
                    *index = start + offset;
                }
            }
            *self
                .indexes
                .by_generation
                .get_mut(&owner.owner.generation())
                .expect("surviving suffix owner is indexed") = start + offset;
            if matches!(owner.route_shape(), WordOwnerLocation::Run(..))
                && let Some(parent) = owner.parent
                && let Some(runs) = self.indexes.runs_by_parent.get_mut(&parent)
                && let Some(index) = runs.iter_mut().find(|index| **index == end + offset)
            {
                *index = start + offset;
            }
        }
        self.indexes.direct_body_paragraphs.truncate(slot);
        self.snapshot_scope = Some(snapshot_scope);
        self.pieces
            .as_mut()
            .expect("active pieces")
            .splice(&self.inner, slot, 1, None);
        Ok(())
    }

    pub(crate) fn current_paragraph_owner(
        &self,
        state: &RevisionState,
    ) -> PyResult<oxoffice_docx::ContentLocation> {
        self.owners
            .iter()
            .find_map(|owner| {
                if !owner.owner.same_state(state) {
                    return None;
                }
                match &*owner.location() {
                    WordOwnerLocation::Paragraph(location) => Some(location.clone()),
                    _ => None,
                }
            })
            .ok_or_else(|| PyValueError::new_err("paragraph physical owner is no longer current"))
            .and_then(|location| {
                ParagraphLocation::Related(location)
                    .native(&self.inner)
                    .map_err(|error| PyValueError::new_err(error.to_string()))
            })
    }

    pub(crate) fn current_run_owner(
        &self,
        state: &RevisionState,
    ) -> PyResult<(
        oxoffice_docx::ContentLocation,
        oxoffice_docx_oxml::text::AcceptedRunPath,
    )> {
        self.owners
            .iter()
            .find_map(|owner| {
                if !owner.owner.same_state(state) {
                    return None;
                }
                match &*owner.location() {
                    WordOwnerLocation::Run(location, path) => {
                        Some((location.clone(), path.clone()))
                    }
                    _ => None,
                }
            })
            .ok_or_else(|| PyValueError::new_err("run physical owner is no longer current"))
            .and_then(|(location, path)| {
                ParagraphLocation::Related(location)
                    .native(&self.inner)
                    .map(|location| (location, path))
                    .map_err(|error| PyValueError::new_err(error.to_string()))
            })
    }

    pub(crate) fn current_table_owner(
        &self,
        state: &RevisionState,
    ) -> PyResult<oxoffice_docx::ContentLocation> {
        self.owners
            .iter()
            .find_map(|owner| {
                if !owner.owner.same_state(state) {
                    return None;
                }
                match &*owner.location() {
                    WordOwnerLocation::Table(location) => Some(location.clone()),
                    _ => None,
                }
            })
            .ok_or_else(|| PyValueError::new_err("table physical owner is no longer current"))
            .and_then(|location| {
                crate::table::TableLocation::Related(location)
                    .native(&self.inner)
                    .map_err(|error| PyValueError::new_err(error.to_string()))
            })
    }

    pub(crate) fn current_cell_owner(
        &self,
        state: &RevisionState,
    ) -> PyResult<(oxoffice_docx::ContentLocation, usize, usize)> {
        self.owners
            .iter()
            .find_map(|owner| {
                if !owner.owner.same_state(state) {
                    return None;
                }
                match &*owner.location() {
                    WordOwnerLocation::Cell {
                        table, row, cell, ..
                    } => Some((table.clone(), *row, *cell)),
                    _ => None,
                }
            })
            .ok_or_else(|| PyValueError::new_err("cell physical owner is no longer current"))
            .and_then(|(table, row, cell)| {
                crate::table::TableLocation::Related(table)
                    .native(&self.inner)
                    .map(|table| (table, row, cell))
                    .map_err(|error| PyValueError::new_err(error.to_string()))
            })
    }

    pub(crate) fn prepare_plain_paragraph_xml(
        &mut self,
        selected: usize,
        paragraph: &oxoffice_docx_oxml::CT_P,
    ) -> PyResult<PreparedPlainParagraphXml> {
        let WordOwnerLocation::Paragraph(location) = self.owners[selected].location().into_owned()
        else {
            return Err(PyValueError::new_err("local XML owner is not a paragraph"));
        };
        let mut runs = Vec::new();
        let mut addresses = Vec::new();
        for path in paragraph.accepted_run_paths() {
            let location = WordOwnerLocation::Run(location.clone(), path);
            if let Some(index) = self.indexes.find(&self.owners, &location) {
                runs.push(index);
            } else {
                addresses.push(WordOwnerAddress {
                    location,
                    parent: Some(selected),
                    body_slot: None,
                    projection_ordinal: None,
                    read_facts: WordReadFacts::None,
                });
            }
        }
        let fail =
            |_| pyo3::exceptions::PyMemoryError::new_err("Word local owner allocation failed");
        self.owners.try_reserve(addresses.len()).map_err(fail)?;
        self.indexes
            .by_generation
            .try_reserve(runs.len() + addresses.len())
            .map_err(fail)?;
        self.indexes
            .by_route
            .try_reserve(addresses.len())
            .map_err(fail)?;
        self.indexes.runs_by_parent.try_reserve(1).map_err(fail)?;
        self.indexes
            .runs_by_parent
            .entry(selected)
            .or_default()
            .try_reserve(addresses.len())
            .map_err(fail)?;
        let mut retired = Vec::new();
        if let Some(content) = &self.owners[selected].content {
            retired.push(content.clone());
        }
        if let Some(properties) = &self.owners[selected].properties {
            retired.push(properties.clone());
        }
        for &index in &runs {
            retired.extend(self.owners[index].states());
        }
        retired.push(
            self.snapshot_scope
                .as_ref()
                .expect("active Word authority")
                .clone(),
        );
        let mut scopes = retired.iter().map(RevisionState::scope).collect::<Vec<_>>();
        for address in &addresses {
            scopes.extend_from_slice(address.location.scopes());
        }
        let revision = self
            .revisions
            .prepare_local_scopes("Paragraph.replace_xml", &retired, &scopes)
            .map_err(revision_to_pyerr)?;
        let states = (0..scopes.len())
            .map(|index| revision.born(index).expect("prepared paragraph scope"))
            .collect::<Vec<_>>();
        let newborn =
            word_owners_with_states(addresses, &states[retired.len()..], &self.indexes.items);
        let mut route_buckets = HashMap::<u64, Vec<usize>>::new();
        for (offset, owner) in newborn.iter().enumerate() {
            let hash = owner.route_key();
            route_buckets
                .entry(hash)
                .or_insert_with(|| {
                    self.indexes
                        .by_route
                        .get(&hash)
                        .cloned()
                        .unwrap_or_default()
                })
                .push(self.owners.len() + offset);
        }
        Ok(PreparedPlainParagraphXml {
            revision,
            states,
            runs,
            newborn,
            route_buckets,
        })
    }

    pub(crate) fn commit_plain_paragraph_xml(
        &mut self,
        selected: usize,
        prepared: PreparedPlainParagraphXml,
        text: String,
    ) -> PyResult<()> {
        let PreparedPlainParagraphXml {
            revision,
            states,
            runs,
            newborn,
            route_buckets,
        } = prepared;
        self.revisions
            .commit_scopes(revision)
            .map_err(revision_to_pyerr)?;
        let mut born = 0;
        if self.owners[selected].content.is_some() {
            self.owners[selected].content = Some(states[born].clone());
            born += 1;
        }
        if self.owners[selected].properties.is_some() {
            self.owners[selected].properties = Some(states[born].clone());
            born += 1;
        }
        for index in runs {
            let owner = &mut self.owners[index];
            self.indexes.by_generation.remove(&owner.owner.generation());
            owner.owner = states[born].clone();
            born += 1;
            if owner.content.is_some() {
                owner.content = Some(states[born].clone());
                born += 1;
            }
            if owner.properties.is_some() {
                owner.properties = Some(states[born].clone());
                born += 1;
            }
            if owner.grid.is_some() {
                owner.grid = Some(states[born].clone());
                born += 1;
            }
            self.indexes
                .by_generation
                .insert(owner.owner.generation(), index);
        }
        self.owners[selected].read_facts = WordReadFacts::Paragraph(text);
        self.snapshot_scope = Some(states[born].clone());
        for owner in newborn {
            self.indexes
                .by_generation
                .insert(owner.owner.generation(), self.owners.len());
            self.indexes
                .runs_by_parent
                .get_mut(&selected)
                .expect("reserved run bucket")
                .push(self.owners.len());
            self.owners.push(owner);
        }
        for (hash, bucket) in route_buckets {
            self.indexes.by_route.insert(hash, bucket);
        }
        Ok(())
    }

    pub(crate) fn prepare_word_revision(
        &mut self,
        py: Python<'_>,
        candidate: &oxoffice_docx::Document,
        operation: &'static str,
        publication: WordPublication,
    ) -> PyResult<PreparedWordRevision> {
        let shift = self.block_shift(py, candidate, &publication)?;
        self.prepare_shifted_revision(py, candidate, operation, publication, shift)
    }

    /// [`Self::prepare_word_revision`] with the shift of a block
    /// publication already read. A block publication without one retires
    /// every handle, as `Conservative` does.
    pub(crate) fn prepare_shifted_revision(
        &mut self,
        py: Python<'_>,
        candidate: &oxoffice_docx::Document,
        operation: &'static str,
        publication: WordPublication,
        shift: Option<BlockShift>,
    ) -> PyResult<PreparedWordRevision> {
        let block = matches!(
            publication,
            WordPublication::ParagraphEdit(_) | WordPublication::CellInsert(..)
        );
        let publication = if block && shift.is_none() {
            WordPublication::Conservative
        } else {
            publication
        };
        let mut shift = shift.filter(|_| block);
        self.ensure_word_authority(py)?;
        let forms = matches!(
            publication,
            WordPublication::FormValues
                | WordPublication::ModernFormValues
                | WordPublication::Signing
        );
        let modern = matches!(publication, WordPublication::ModernFormValues);
        let mut old_sources = FormSources::default();
        let mut new_sources = FormSources::default();
        let (piece_cache, inventory) = if forms {
            (None, None)
        } else {
            let (pieces, inventory) = self
                .pieces
                .as_ref()
                .expect("active pieces")
                .assemble(candidate);
            (Some(pieces), inventory)
        };
        let (locations, projections) = match inventory {
            Some((locations, projections)) => {
                #[cfg(debug_assertions)]
                assert_word_pieces_rebuild(candidate, &locations, &projections);
                (locations, Some(projections))
            }
            None => (
                word_owner_locations(candidate, forms.then_some(&mut new_sources))
                    .map_err(|error| docx_to_pyerr(py, error))?,
                None,
            ),
        };
        if forms {
            word_owner_locations(&self.inner, Some(&mut old_sources))
                .map_err(|error| docx_to_pyerr(py, error))?;
        }
        let selected = match &publication {
            WordPublication::HeaderFooterAppend(index) => Some(WordOwnerLocation::Section(*index)),
            WordPublication::ParagraphText(location)
            | WordPublication::ParagraphAppend(location) => {
                Some(WordOwnerLocation::Paragraph(location.clone()))
            }
            WordPublication::TableGrid(location) => {
                Some(WordOwnerLocation::Table(location.clone()))
            }
            WordPublication::ParagraphEdit(location) => {
                Some(WordOwnerLocation::Paragraph(location.clone()))
            }
            WordPublication::CellText(table, row, cell)
            | WordPublication::CellInsert(table, row, cell) => {
                self.owners
                    .iter()
                    .find_map(|owner| match &*owner.location() {
                        WordOwnerLocation::Cell {
                            table: owned,
                            row: r,
                            cell: c,
                            ..
                        } if same_content_owner(owned, table) && r == row && c == cell => {
                            Some(owner.location().into_owned())
                        }
                        _ => None,
                    })
            }
            WordPublication::ElementXml(location)
            | WordPublication::RunText(location)
            | WordPublication::HyperlinkRemove(location) => Some(location.clone()),
            _ => None,
        };
        let selected_index = selected
            .as_ref()
            .and_then(|selected| self.indexes.find(&self.owners, selected));
        // A block edit whose element has no owner retires every handle, as
        // the run edits and cell insertions did before.
        let publication = if shift.is_some() && selected_index.is_none() {
            shift = None;
            WordPublication::Conservative
        } else {
            publication
        };
        let block = shift.is_some();
        if !matches!(
            publication,
            WordPublication::Conservative
                | WordPublication::FormValues
                | WordPublication::ModernFormValues
                | WordPublication::Signing
                | WordPublication::AppendBody
        ) && selected_index.is_none()
        {
            return Err(PyValueError::new_err(
                "checked XML owner has no current physical authority",
            ));
        }
        // Conservative publication retires every old scope. Matching those
        // owners cannot certify any retention and only clones unused routes.
        let matches = if matches!(publication, WordPublication::Conservative) {
            vec![None; locations.len()]
        } else {
            indexed_inventory_matches(&self.owners, &locations, &publication)
        };
        let (eligible_parts, modern_snapshot) = if modern {
            unchanged_related_owner_parts(&self.inner, candidate, &self.owners, &locations)
                .map_err(|error| docx_to_pyerr(py, error))?
        } else {
            (std::collections::HashSet::new(), false)
        };
        let form_certificates = if matches!(publication, WordPublication::Signing) {
            signing_owner_certificates(
                &self.owners,
                &locations,
                &matches,
                &old_sources,
                &new_sources,
            )
        } else if modern {
            modern_form_owner_certificates(
                &self.owners,
                &locations,
                &matches,
                &old_sources,
                &new_sources,
                &eligible_parts,
            )
        } else if forms {
            form_owner_certificates(
                &self.owners,
                &locations,
                &matches,
                &old_sources,
                &new_sources,
                &std::collections::HashSet::new(),
            )
        } else {
            std::collections::HashSet::new()
        };
        let preserve_snapshot = modern_snapshot
            || matches!(publication, WordPublication::Signing)
                && self
                    .inner
                    .stories()
                    .map_err(|error| docx_to_pyerr(py, error))?
                    == candidate
                        .stories()
                        .map_err(|error| docx_to_pyerr(py, error))?
                && self
                    .inner
                    .story_item_snapshots()
                    .map_err(|error| docx_to_pyerr(py, error))?
                    == candidate
                        .story_item_snapshots()
                        .map_err(|error| docx_to_pyerr(py, error))?;
        let block_keep = shift.as_ref().map(|shift| {
            block_retention(
                &self.owners,
                &locations,
                &matches,
                selected_index.expect("a block publication has a selected owner"),
                shift,
            )
        });
        let mut retained = Vec::new();
        let mut births = Vec::new();
        for (index, address) in locations.iter().enumerate() {
            let location = &address.location;
            let old = matches[index].map(|before| (before, &self.owners[before]));
            let mut states = Vec::new();
            let mut descendant = false;
            if let (Some((before, _)), Some(selected)) = (old, selected_index) {
                let mut parent = self.owners[before].parent;
                while let Some(index) = parent {
                    if index == selected {
                        descendant = true;
                        break;
                    }
                    parent = self.owners[index].parent;
                }
            }
            for (slot, scope) in location.scopes().iter().enumerate() {
                let route_proved = old.is_some_and(|(before, owner)| {
                    if matches!(publication, WordPublication::AppendBody) {
                        return true;
                    }
                    if Some(before) == selected_index {
                        // A text-box paragraph is rewritten through its physical
                        // part. Its owner scope retires with the edit, so the
                        // handle that made the change cannot be reused.
                        // The stored route holds the story kind.
                        if matches!(
                            self.owners[before].route_shape(),
                            WordOwnerLocation::Paragraph(location)
                                if location.story().kind() == oxoffice_docx::StoryKind::TextBox
                        ) {
                            return false;
                        }
                        return true;
                    }
                    let mut root = before;
                    while let Some(parent) = self.owners[root].parent {
                        root = parent;
                    }
                    // The kind and the story kind, as the stored route holds them.
                    match self.owners[root].route_shape() {
                        WordOwnerLocation::Section(_) => true,
                        WordOwnerLocation::Hyperlink(..) => matches!(
                            publication,
                            WordPublication::ParagraphAppend(_)
                                | WordPublication::HyperlinkRemove(_)
                        ),
                        WordOwnerLocation::Paragraph(location)
                        | WordOwnerLocation::Table(location) => match location.story().kind() {
                            oxoffice_docx::StoryKind::Body => {
                                self.owners[root].body_slot().is_some()
                            }
                            oxoffice_docx::StoryKind::TextBox
                            | oxoffice_docx::StoryKind::Comment => false,
                            oxoffice_docx::StoryKind::Header
                            | oxoffice_docx::StoryKind::Footer
                            | oxoffice_docx::StoryKind::Footnote
                            | oxoffice_docx::StoryKind::Endnote => true,
                            _ => false,
                        },
                        _ => {
                            let _ = owner;
                            false
                        }
                    }
                });
                let preserve = if let Some(keep) = &block_keep {
                    old.is_some() && keep[index][slot]
                } else {
                    old.is_some_and(|(before, _)| form_certificates.contains(&(before, index)))
                        || (route_proved
                            && !matches!(
                                publication,
                                WordPublication::Conservative
                                    | WordPublication::FormValues
                                    | WordPublication::ModernFormValues
                                    | WordPublication::Signing
                            )
                            && !descendant
                            && !(matches!(publication, WordPublication::HyperlinkRemove(_))
                                && matches!(location, WordOwnerLocation::Paragraph(_))
                                && slot == 1)
                            && !(old.is_some_and(|(before, _)| Some(before) == selected_index)
                                && slot != 0
                                && !matches!(publication, WordPublication::RunText(_))
                                && (!matches!(
                                    publication,
                                    WordPublication::ParagraphText(_)
                                        | WordPublication::ParagraphAppend(_)
                                ) || slot == 1)))
                };
                let state = if preserve {
                    old.map(|(_, owner)| owner.states()[slot].clone())
                } else {
                    None
                };
                if state.is_none() {
                    births.push(*scope);
                }
                states.push(state);
            }
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
        if !preserve_snapshot {
            retired.push(
                self.snapshot_scope
                    .as_ref()
                    .expect("active Word authority")
                    .clone(),
            );
            births.push(RevisionScope::Text);
        }
        // A run handle also carries the document epoch. Modern publication
        // keeps that epoch with the same local commit Signing uses. A proved
        // run then stays current. An unproved owner state still retires it.
        let revision = if matches!(publication, WordPublication::Conservative) {
            self.revisions.prepare_revision(
                Some(operation),
                &retired,
                &births,
                RevisionPreparation::Replacement,
            )
        } else if block
            || matches!(
                publication,
                WordPublication::ElementXml(_)
                    | WordPublication::RunText(_)
                    | WordPublication::Signing
                    | WordPublication::ModernFormValues
            )
        {
            self.revisions
                .prepare_local_scopes(operation, &retired, &births)
        } else {
            self.revisions.prepare_scopes(operation, &retired, &births)
        }
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
        let snapshot_scope = if preserve_snapshot {
            self.snapshot_scope
                .as_ref()
                .expect("active Word authority")
                .clone()
        } else {
            revision.born(born).expect("prepared snapshot birth")
        };
        let projections = projections.unwrap_or_else(|| WordIndexProjections::of(candidate));
        let items = WordBodyItems::derive(&locations, &projections);
        let owners = word_owners_with_states(locations, &states, &items);
        #[cfg(debug_assertions)]
        if let (Some(shift), Some(selected)) = (&shift, selected_index) {
            assert_kept_owners_name_their_elements(
                (&self.inner, &self.owners),
                (candidate, &owners),
                Some((shift, &self.owners[selected].owner)),
            );
        }
        let indexes = WordOwnerIndexes::prepare(&owners, items, projections)?;
        Ok(PreparedWordRevision {
            revision,
            owners,
            indexes,
            snapshot_scope,
            pieces: piece_cache,
        })
    }

    pub(crate) fn commit_word_revision(&mut self, prepared: PreparedWordRevision) -> PyResult<()> {
        self.revisions
            .commit_scopes(prepared.revision)
            .map_err(revision_to_pyerr)?;
        self.owners = prepared.owners;
        self.indexes = prepared.indexes;
        self.snapshot_scope = Some(prepared.snapshot_scope);
        self.pending_pieces = Some(prepared.pieces.unwrap_or_default());
        Ok(())
    }

    pub(crate) fn install_inner(&mut self, candidate: oxoffice_docx::Document) {
        self.pieces = if self.snapshot_scope.is_some() {
            Some(self.pending_pieces.take().unwrap_or_default())
        } else {
            self.pending_pieces = None;
            None
        };
        self.inner = candidate;
    }

    pub(crate) fn publish_owner_candidate(
        &mut self,
        py: Python<'_>,
        candidate: oxoffice_docx::Document,
        operation: &'static str,
        publication: WordPublication,
    ) -> PyResult<()> {
        if self
            .inner
            .saves_as(&candidate)
            .map_err(|error| docx_to_pyerr(py, error))?
        {
            return Ok(());
        }
        let prepared = self.prepare_word_revision(py, &candidate, operation, publication)?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    /// Publish `candidate`, this document after one typed edit of the
    /// direct body paragraph at `slot`, unless the edit changed nothing. The
    /// answer is read from that block when it tells, and from the staged
    /// save comparison of [`Self::publish_owner_candidate`] when it does
    /// not.
    pub(crate) fn publish_block_candidate(
        &mut self,
        py: Python<'_>,
        candidate: oxoffice_docx::Document,
        operation: &'static str,
        publication: WordPublication,
        slot: usize,
    ) -> PyResult<()> {
        let Some(same) = self.direct_paragraph_saves_as(&candidate, slot) else {
            return self.publish_owner_candidate(py, candidate, operation, publication);
        };
        // The staged comparison saves the whole candidate and refuses when
        // content outside this block cannot be saved. It checks the block
        // answer whenever it gives one.
        #[cfg(debug_assertions)]
        if let Ok(saved) = self.inner.saves_as(&candidate) {
            assert_eq!(saved, same, "direct body paragraph save comparison");
        }
        if same {
            return Ok(());
        }
        let prepared = self.prepare_word_revision(py, &candidate, operation, publication)?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    /// Whether `candidate`, this document after one typed edit of the
    /// direct body paragraph at `slot`, saves as this document, read from
    /// that block alone. Equal paragraph models save the same bytes. Two
    /// plain paragraphs whose checked reader facts differ, the text and the
    /// run paths of [`oxoffice_docx::Document::direct_body_paragraph_facts`], save
    /// different bytes, as a plain paragraph holds nothing a save
    /// renumbers. `None` leaves the answer to the staged save: another
    /// paragraph shape, a body whose save is known to fail, text a save
    /// refuses, or facts that do not tell. The block answer does not save
    /// the rest of the document, so content elsewhere that no save accepts
    /// is reported by the save itself, as after `Paragraph.text`.
    fn direct_paragraph_saves_as(
        &self,
        candidate: &oxoffice_docx::Document,
        slot: usize,
    ) -> Option<bool> {
        if self.inner.unsavable_body_edit().is_some() {
            return None;
        }
        let (
            oxoffice_docx_oxml::BodyContent::Paragraph(current),
            oxoffice_docx_oxml::BodyContent::Paragraph(edited),
        ) = (
            self.inner.body_children().get(slot)?,
            candidate.body_children().get(slot)?,
        )
        else {
            return None;
        };
        if current == edited {
            return Some(true);
        }
        let before = self.inner.direct_body_paragraph_facts(current).ok()??;
        let after = candidate.direct_body_paragraph_facts(edited).ok()??;
        // A save refuses a character XML 1.0 cannot carry, with its place in
        // the part. The block reader does not, so the staged save answers.
        if !after
            .0
            .chars()
            .all(oxoffice_core::xml::is_xml_1_0_character)
        {
            return None;
        }
        (before != after).then_some(false)
    }

    pub(crate) fn local_cell_route(
        &self,
        state: &RevisionState,
    ) -> Option<(usize, usize, usize, usize)> {
        let (slot, row, cell) = self.direct_body_cell_slot(state)?;
        let (_, owner) = self.indexed_owner(state)?;
        let parent = self.owners.get(owner.parent?)?;
        Some((slot, parent.projection_ordinal()?, row, cell))
    }

    pub(crate) fn try_local_cell_paragraph_text(
        &mut self,
        py: Python<'_>,
        location: &oxoffice_docx::ContentLocation,
        text: &str,
    ) -> PyResult<bool> {
        if location.index_path().len() != 1 {
            return Ok(false);
        }
        let Some(index) = self.indexes.find(
            &self.owners,
            &WordOwnerLocation::Paragraph(location.clone()),
        ) else {
            return Ok(false);
        };
        let Some(parent) = self.owners[index].parent else {
            return Ok(false);
        };
        let Some((slot, table, row, cell)) = self.local_cell_route(&self.owners[parent].owner)
        else {
            return Ok(false);
        };
        let Some(paragraphs) = self
            .indexes
            .cell_paragraphs
            .get(table)
            .and_then(|cells| cells.get(&(row, cell)))
        else {
            return Ok(false);
        };
        let paragraphs = paragraphs
            .iter()
            .map(AnchoredRoute::location)
            .collect::<Vec<_>>();
        let Some(position) = paragraphs
            .iter()
            .position(|candidate| same_content_owner(candidate, location))
        else {
            return Ok(false);
        };
        let ordinal = paragraphs[..position]
            .iter()
            .filter(|location| location.index_path().len() == 1)
            .count();
        let Some(candidate) = self
            .inner
            .direct_body_table_cell_candidate(slot, row, cell, |cell| {
                cell.paragraph_mut(ordinal)
                    .ok_or_else(|| {
                        oxoffice_docx::Error::Other("paragraph index out of range".into())
                    })?
                    .set_text(text)
            })
            .map_err(|error| docx_to_pyerr(py, error))?
        else {
            return Ok(false);
        };
        // Text the cell already holds changes nothing: no publication.
        if let Some(oxoffice_docx_oxml::BodyContent::Table(current)) =
            self.inner.body_children().get(slot)
            && let (Some(before), Some(after)) = (
                current
                    .rows
                    .get(row)
                    .and_then(|current| current.cells.get(cell)),
                candidate.rows.first().and_then(|row| row.cells.first()),
            )
            && before == after
        {
            return Ok(true);
        }
        self.apply_local_cell_edit(
            py,
            slot,
            table,
            row,
            cell,
            candidate,
            "Paragraph.text",
            Some(WordPublication::ParagraphEdit(location.clone())),
        )
    }

    /// Publish one prepared direct-body cell edit, touching only the owners of
    /// that cell. `candidate` is the edited cell alone in its table frame, as
    /// `direct_body_table_cell_candidate` prepares it, and the checked reader
    /// reads that frame for the facts of the cell. The other cells keep the
    /// facts the checked reader gave the aligned piece, as their models are
    /// the ones it read.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn apply_local_cell_edit(
        &mut self,
        py: Python<'_>,
        body_slot: usize,
        table_ordinal: usize,
        row: usize,
        cell: usize,
        candidate: oxoffice_docx_oxml::CT_Tbl,
        operation: &'static str,
        publication: Option<WordPublication>,
    ) -> PyResult<bool> {
        let Some(pieces) = &self.pieces else {
            return Ok(false);
        };
        if pieces.context.as_ref() != Some(&self.inner.body_item_context())
            || pieces.blocks.len() != self.inner.body_children().len()
        {
            return Ok(false);
        }
        let Some(piece) = pieces.blocks.get(body_slot) else {
            return Ok(false);
        };
        // The piece is spliced in place after the revision commit.
        if std::sync::Arc::strong_count(piece) != 1 || std::sync::Arc::weak_count(piece) != 0 {
            return Ok(false);
        }
        if !aligned_block(piece, &self.inner.body_children()[body_slot]) {
            return Ok(false);
        }
        let Some(before) = &piece.owners else {
            return Ok(false);
        };
        // Every physical cell must belong to this indexed table subtree, with
        // its standalone physical ordinal intact. Otherwise use the checked
        // publisher before preparing any revision or owner/index delta.
        if !before.local_table_membership
            || before.body_items != 1
            || !matches!(before.cell_paragraphs, Some((_, true)))
        {
            return Ok(false);
        }
        // A text-box count needs its own first-story base. Keep its checked fallback.
        if before.text_boxes != 0 || !before.facts.links.is_empty() {
            return Ok(false);
        }
        // Complete membership lists the cells in story order, so the cell's
        // story ordinal in the table alone is its position there.
        let Ok(local) = before
            .facts
            .cells
            .binary_search_by_key(&(row, cell), |(_, row, cell, _)| (*row, *cell))
        else {
            return Ok(false);
        };
        let Some(table_location) = self
            .indexes
            .body_tables
            .get(table_ordinal)
            .and_then(Option::as_ref)
            .map(AnchoredRoute::location)
        else {
            return Ok(false);
        };
        let Some(table_index) = self.indexes.find(
            &self.owners,
            &WordOwnerLocation::Table(table_location.clone()),
        ) else {
            return Ok(false);
        };
        if self.owners[table_index].body_slot() != Some(body_slot)
            || table_location.index_path().len() != 1
            || !matches!(&self.owners[table_index].read_facts, WordReadFacts::Table(rows)
                if rows.get(row).and_then(|cells| cells.get(cell)).is_some())
        {
            return Ok(false);
        }
        let Some(cell_indices) = self.indexes.cells_by_parent.get(&table_index) else {
            return Ok(false);
        };
        let Some(selected) = cell_indices.iter().copied().find(|index| {
            matches!(&*self.owners[*index].location(),
            WordOwnerLocation::Cell { row: r, cell: c, .. } if *r == row && *c == cell)
        }) else {
            return Ok(false);
        };
        let WordOwnerLocation::Cell {
            story: cell_story, ..
        } = &*self.owners[selected].location()
        else {
            return Ok(false);
        };
        if self
            .indexes
            .tables_by_parent
            .get(&selected)
            .is_some_and(|tables| !tables.is_empty())
        {
            return Ok(false);
        }
        let retire = publication.is_some();
        let paragraph_text = matches!(
            publication,
            Some(WordPublication::ParagraphText(_) | WordPublication::ParagraphEdit(_))
        );
        // A paragraph edit keeps the document epoch: run handles elsewhere stay.
        let keeps_epoch = matches!(publication, Some(WordPublication::ParagraphEdit(_)));
        let text_selected = match &publication {
            None => selected,
            Some(WordPublication::CellText(table, r, c))
                if same_content_owner(table, &table_location) && *r == row && *c == cell =>
            {
                selected
            }
            Some(
                WordPublication::ParagraphText(location) | WordPublication::ParagraphEdit(location),
            ) if location.index_path().len() == 1 => {
                let Some(index) = self.indexes.find(
                    &self.owners,
                    &WordOwnerLocation::Paragraph(location.clone()),
                ) else {
                    return Ok(false);
                };
                if self.owners[index].parent != Some(selected)
                    || !self
                        .indexes
                        .cell_paragraphs
                        .get(table_ordinal)
                        .and_then(|cells| cells.get(&(row, cell)))
                        .is_some_and(|paragraphs| {
                            paragraphs.iter().any(|current| current.names(location))
                        })
                {
                    return Ok(false);
                }
                index
            }
            _ => return Ok(false),
        };
        // The document's ordinal of the cell story, from the cell's own record.
        let stories = (cell_story.owner_index(), 0);
        let block = oxoffice_docx_oxml::BodyContent::Table(Box::new(candidate));
        let Some(after) =
            word_block_owners(&self.inner, &block).map_err(|error| docx_to_pyerr(py, error))?
        else {
            return Ok(false);
        };
        let Some((frame_routes, true)) = &after.cell_paragraphs else {
            return Ok(false);
        };
        let (Some(routes), Some((_, _, WordReadFacts::Table(frame_rows)))) =
            (frame_routes.get(&(0, 0, 0)), after.facts.items.first())
        else {
            return Ok(false);
        };
        // The frame holds this cell alone, with no nested table, text box or link.
        if !after.local_table_membership
            || after.facts.cells.len() != 1
            || after.body_items != 1
            || after.cell_stories != 1
            || after.text_boxes != 0
            || after.tables != 1
            || !after.facts.links.is_empty()
            || frame_rows.len() != 1
            || frame_rows[0].len() != 1
        {
            return Ok(false);
        }
        let paragraph_count = frame_rows[0][0];
        let item_base = table_location.index_path()[0];
        let body = table_location.story();
        let place = |location: &oxoffice_docx::ContentLocation| {
            placed_route(location, body, (item_base, stories))
        };
        let mut facts = after.facts.clone();
        for (location, slot, _) in &mut facts.items {
            *location = place(location);
            *slot = slot.map(|_| body_slot);
        }
        for (table, r, c, story) in &mut facts.cells {
            *table = place(table);
            (*r, *c) = (row, cell);
            *story = placed_story(story, stories);
        }
        for (location, _, _) in &mut facts.paragraphs {
            *location = place(location);
        }
        let mut addresses = word_piece_owner_addresses(facts, 0);
        // The frame's table owner is the table's own record. Keep the cell and
        // what it holds, with the cell first and parents among them.
        if !matches!(
            addresses.first().map(|address| &address.location),
            Some(WordOwnerLocation::Table(_))
        ) || !matches!(
            addresses.get(1),
            Some(WordOwnerAddress {
                location: WordOwnerLocation::Cell { .. },
                parent: Some(0),
                ..
            })
        ) || addresses[2..]
            .iter()
            .any(|address| !matches!(address.parent, Some(parent) if parent >= 1))
        {
            return Ok(false);
        }
        addresses.remove(0);
        for address in &mut addresses {
            address.parent = address.parent.and_then(|parent| parent.checked_sub(1));
        }
        // Walk indexed children of the cell, never the table's or the document's.
        let mut previous = Vec::new();
        let mut pending = vec![selected];
        while let Some(index) = pending.pop() {
            previous.push(index);
            for children in [
                self.indexes.cells_by_parent.get(&index),
                self.indexes.paragraphs_by_parent.get(&index),
                self.indexes.runs_by_parent.get(&index),
                self.indexes.tables_by_parent.get(&index),
            ]
            .into_iter()
            .flatten()
            {
                pending.extend(children);
            }
        }
        let local_positions = previous
            .iter()
            .enumerate()
            .map(|(local, global)| (*global, local))
            .collect::<HashMap<_, _>>();
        let mut old_addresses = Vec::with_capacity(previous.len());
        for index in &previous {
            let old = &self.owners[*index];
            old_addresses.push(WordOwnerAddress {
                location: old.location().into_owned(),
                parent: old
                    .parent
                    .and_then(|parent| local_positions.get(&parent).copied()),
                body_slot: old.body_slot(),
                projection_ordinal: old.projection_ordinal(),
                read_facts: old.read_facts.clone(),
            });
        }
        // Ordinal joins survive a first serialized tcPr shifting raw item indices.
        let key = |addresses: &[WordOwnerAddress], index: usize| {
            let address = &addresses[index];
            let paragraph = match address.location {
                WordOwnerLocation::Paragraph(_) => address.projection_ordinal,
                WordOwnerLocation::Run(..) => address
                    .parent
                    .and_then(|parent| addresses[parent].projection_ordinal),
                _ => None,
            };
            (property_owner_key(addresses, index), paragraph)
        };
        let mut buckets = HashMap::<_, Vec<usize>>::new();
        for index in 0..old_addresses.len() {
            buckets
                .entry(key(&old_addresses, index))
                .or_default()
                .push(index);
        }
        let mut cursors = HashMap::<_, usize>::new();
        let mut matched = Vec::with_capacity(addresses.len());
        let mut targets = Vec::with_capacity(addresses.len());
        let mut newborn = 0;
        for index in 0..addresses.len() {
            let key = key(&addresses, index);
            let cursor = cursors.entry(key.clone()).or_default();
            let old = buckets
                .get(&key)
                .and_then(|bucket| bucket.get(*cursor))
                .copied();
            *cursor += 1;
            matched.push(old.map(|local| previous[local]));
            targets.push(old.map(|local| previous[local]).unwrap_or_else(|| {
                let target = self.owners.len() + newborn;
                newborn += 1;
                target
            }));
        }
        if targets.first() != Some(&selected)
            || buckets
                .iter()
                .any(|(key, old)| cursors.get(key).copied().unwrap_or(0) < old.len())
        {
            return Ok(false);
        }
        if !retire && newborn != 0 {
            return Ok(false);
        }
        let mut memberships = Vec::new();
        let mut route_buckets = HashMap::<u64, Vec<usize>>::new();
        for index in &previous {
            let hash = self.owners[*index].route_key();
            route_buckets.entry(hash).or_insert_with(|| {
                self.indexes
                    .by_route
                    .get(&hash)
                    .cloned()
                    .unwrap_or_default()
            });
        }
        for (index, address) in addresses.iter_mut().enumerate() {
            // The cell's parent is the table, outside the replaced owners.
            address.parent = Some(address.parent.map_or(table_index, |parent| targets[parent]));
            let (route, anchor) = self.indexes.items.stored(&address.location);
            let hash = route_key(&route, anchor);
            route_buckets.entry(hash).or_insert_with(|| {
                self.indexes
                    .by_route
                    .get(&hash)
                    .cloned()
                    .unwrap_or_default()
            });
            memberships.push((hash, targets[index]));
        }
        for bucket in route_buckets.values_mut() {
            bucket.retain(|index| !local_positions.contains_key(index));
        }
        for (hash, index) in memberships {
            route_buckets
                .get_mut(&hash)
                .expect("prepared bucket")
                .push(index);
        }
        let mut parent_buckets = [
            HashMap::<usize, Vec<usize>>::new(),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        ];
        // The table keeps its list of cells, and the cell keeps its record.
        for (index, address) in addresses.iter().enumerate().skip(1) {
            if let Some(parent) = address.parent {
                let family = match address.location {
                    WordOwnerLocation::Cell { .. } => 0,
                    WordOwnerLocation::Paragraph(_) => 1,
                    WordOwnerLocation::Run(..) => 2,
                    WordOwnerLocation::Table(_) => 3,
                    _ => return Ok(false),
                };
                parent_buckets[family]
                    .entry(parent)
                    .or_default()
                    .push(targets[index]);
            }
        }
        let cell_routes = routes.iter().map(&place).collect::<Vec<_>>();
        let mut births = Vec::new();
        let mut retained = Vec::new();
        let mut retired = Vec::new();
        for (index, address) in addresses.iter().enumerate() {
            let old = matched[index].map(|index| &self.owners[index]);
            let changed = retire
                && matched[index].is_some_and(|index| {
                    let mut parent = self.owners[index].parent;
                    while let Some(index) = parent {
                        if index == text_selected {
                            return true;
                        }
                        parent = self.owners[index].parent;
                    }
                    false
                });
            let old_states = old.map(|owner| owner.states());
            let mut states = Vec::new();
            for (slot, scope) in address.location.scopes().iter().enumerate() {
                let replace = changed
                    || retire
                        && matched[index] == Some(text_selected)
                        && if paragraph_text { slot == 1 } else { slot != 0 };
                let state = if replace {
                    None
                } else {
                    old_states.as_ref().map(|states| states[slot].clone())
                };
                if state.is_none() {
                    births.push(*scope);
                    if let Some(states) = &old_states {
                        retired.push(states[slot].clone());
                    }
                }
                states.push(state);
            }
            retained.push(states);
        }
        if retire {
            retired.push(
                self.snapshot_scope
                    .as_ref()
                    .expect("active authority")
                    .clone(),
            );
            births.push(RevisionScope::Text);
        }
        // The piece facts of the cell, in the numbering of the table alone.
        let piece_place = |location: &oxoffice_docx::ContentLocation| {
            placed_route(location, location.story(), (0, (local, 0)))
        };
        let piece_paragraphs = after
            .facts
            .paragraphs
            .iter()
            .map(|(location, text, runs)| (piece_place(location), text.clone(), runs.clone()))
            .collect::<Vec<_>>();
        let piece_story = placed_story(&after.facts.cells[0].3, (local, 0));
        let piece_routes = routes.iter().map(piece_place).collect::<Vec<_>>();
        // The paragraphs are those of the cell stories, in story order.
        let ordinal = |location: &oxoffice_docx::ContentLocation| location.story().owner_index();
        let paragraphs = &before.facts.paragraphs;
        let paragraph_range = paragraphs
            .partition_point(|(location, _, _)| ordinal(location) < local)
            ..paragraphs.partition_point(|(location, _, _)| ordinal(location) <= local);
        let holds_field = after.holds_field;
        let oxoffice_docx_oxml::BodyContent::Table(frame) = block else {
            unreachable!()
        };
        let piece_cell = frame.rows[0].cells[0].clone();
        let fail = |_| {
            pyo3::exceptions::PyMemoryError::new_err("local cell publication allocation failed")
        };
        self.owners.try_reserve(newborn).map_err(fail)?;
        self.indexes
            .by_generation
            .try_reserve(newborn)
            .map_err(fail)?;
        self.indexes
            .by_route
            .try_reserve(route_buckets.len())
            .map_err(fail)?;
        self.indexes
            .paragraphs_by_parent
            .try_reserve(parent_buckets[1].len())
            .map_err(fail)?;
        self.indexes
            .runs_by_parent
            .try_reserve(parent_buckets[2].len())
            .map_err(fail)?;
        {
            let piece = std::sync::Arc::get_mut(
                &mut self.pieces.as_mut().expect("active pieces").blocks[body_slot],
            )
            .expect("a piece checked unique");
            let owners = piece.owners.as_mut().expect("aligned piece owners");
            owners
                .facts
                .paragraphs
                .try_reserve(piece_paragraphs.len())
                .map_err(fail)?;
        }
        let revision = if retire {
            Some(
                if keeps_epoch {
                    self.revisions
                        .prepare_local_scopes(operation, &retired, &births)
                } else {
                    self.revisions.prepare_scopes(operation, &retired, &births)
                }
                .map_err(revision_to_pyerr)?,
            )
        } else {
            None
        };
        #[cfg(debug_assertions)]
        let before = keeps_epoch.then(|| {
            (
                self.inner.clone_for_staging(),
                self.owners.clone(),
                self.owners[text_selected].owner.clone(),
            )
        });
        let mut born = 0;
        let states = retained
            .into_iter()
            .flatten()
            .map(|state| {
                state.unwrap_or_else(|| {
                    let state = revision
                        .as_ref()
                        .expect("text births")
                        .born(born)
                        .expect("prepared owner birth");
                    born += 1;
                    state
                })
            })
            .collect::<Vec<_>>();
        let snapshot = revision
            .as_ref()
            .map(|revision| revision.born(born).expect("prepared snapshot"));
        let owners = word_owners_with_states(addresses, &states, &self.indexes.items);
        if let Some(revision) = revision {
            self.revisions
                .commit_scopes(revision)
                .map_err(revision_to_pyerr)?;
        }
        // No recoverably fallible operation follows the revision commit.
        self.inner
            .commit_direct_body_table_cell(body_slot, row, cell, *frame);
        for index in &previous {
            self.indexes
                .by_generation
                .remove(&self.owners[*index].owner.generation());
        }
        for (owner, target) in owners.into_iter().zip(targets) {
            self.indexes
                .by_generation
                .insert(owner.owner.generation(), target);
            if target == self.owners.len() {
                self.owners.push(owner);
            } else {
                self.owners[target] = owner;
            }
        }
        if let WordReadFacts::Table(rows) = &mut self.owners[table_index].read_facts {
            rows[row][cell] = paragraph_count;
        }
        for (hash, bucket) in route_buckets {
            if bucket.is_empty() {
                self.indexes.by_route.remove(&hash);
            } else {
                self.indexes.by_route.insert(hash, bucket);
            }
        }
        for (map, prepared) in [
            &mut self.indexes.cells_by_parent,
            &mut self.indexes.paragraphs_by_parent,
            &mut self.indexes.runs_by_parent,
            &mut self.indexes.tables_by_parent,
        ]
        .into_iter()
        .zip(parent_buckets)
        {
            for index in &previous {
                map.remove(index);
            }
            map.extend(prepared);
        }
        let cell_routes = cell_routes
            .into_iter()
            .map(|location| self.indexes.items.route(location))
            .collect();
        self.indexes.cell_paragraphs[table_ordinal].insert((row, cell), cell_routes);
        if let Some(snapshot) = snapshot {
            self.snapshot_scope = Some(snapshot);
        }
        let piece = std::sync::Arc::get_mut(
            &mut self.pieces.as_mut().expect("active pieces").blocks[body_slot],
        )
        .expect("a piece checked unique");
        if let oxoffice_docx_oxml::BodyContent::Table(table) =
            std::sync::Arc::make_mut(&mut piece.block)
        {
            table.rows[row].cells[cell] = piece_cell;
        }
        let owners = piece.owners.as_mut().expect("aligned piece owners");
        // The inventory reads the table and body items of a piece, here the
        // table's own item. A full read also lists the paragraphs of the cell
        // stories, which a body splice leaves out. Keep no stale ones.
        owners.facts.items.truncate(1);
        owners
            .facts
            .paragraphs
            .splice(paragraph_range, piece_paragraphs);
        owners.facts.cells[local].3 = piece_story;
        if let Some((_, _, WordReadFacts::Table(rows))) = owners.facts.items.first_mut() {
            rows[row][cell] = paragraph_count;
        }
        if let Some((routes, _)) = &mut owners.cell_paragraphs {
            routes.insert((0, row, cell), piece_routes);
        }
        // A field elsewhere in the table may remain. The flag may stay set
        // until the next full read of the table, which a set flag asks for.
        owners.holds_field |= holds_field;
        #[cfg(debug_assertions)]
        self.assert_local_cell_publication(body_slot, table_index, table_ordinal);
        #[cfg(debug_assertions)]
        if let (
            Some((document, owners, selected)),
            Some(WordPublication::ParagraphEdit(location)),
        ) = (before, &publication)
        {
            self.assert_word_inventory_rebuilds(false);
            let read = |document: &oxoffice_docx::Document| {
                document
                    .element_extent(location.story(), Some(location.index_path()))
                    .expect("an edited cell paragraph reads")
                    .expect("an edited cell paragraph has an extent")
            };
            let shift = BlockShift::of(location.story(), read(&document), read(&self.inner))
                .expect("a local cell edit changes its paragraph alone");
            assert_kept_owners_name_their_elements(
                (&document, &owners),
                (&self.inner, &self.owners),
                Some((&shift, &selected)),
            );
        }
        Ok(true)
    }

    /// After an in-place property edit of `scope` of the direct body table
    /// at `body_slot`, copy the table and row properties of that scope into
    /// the table's piece when the piece holds everything else of that scope
    /// as the live table does. The piece's owner facts read the rows, cells
    /// and content of the table (a cell story's fingerprint is its bytes),
    /// never these properties, so they stay the facts of the piece, and a
    /// piece aligned before the edit stays aligned: the next cell edit of the
    /// table keeps the local route, as after a publication. An edit that
    /// changed a cell leaves the piece behind, and the next cell edit reads
    /// the table again. Tables with row or cell controls, and pieces without
    /// owner facts, are left behind too.
    pub(crate) fn follow_table_property_edit(
        &mut self,
        body_slot: usize,
        scope: TablePropertyScope,
    ) {
        let Some(pieces) = &mut self.pieces else {
            return;
        };
        if pieces.context.as_ref() != Some(&self.inner.body_item_context())
            || pieces.blocks.len() != self.inner.body_children().len()
        {
            return;
        }
        let Some(oxoffice_docx_oxml::BodyContent::Table(live)) =
            self.inner.body_children().get(body_slot)
        else {
            return;
        };
        let Some(piece) = pieces
            .blocks
            .get_mut(body_slot)
            .and_then(std::sync::Arc::get_mut)
        else {
            return;
        };
        // Properties the checked reader refuses (out of order, repeated) can
        // leave a table without owner facts; a rewrite may order them.
        if piece.owners.is_none() {
            return;
        }
        let oxoffice_docx_oxml::BodyContent::Table(held) = piece.block.as_ref() else {
            return;
        };
        let same_row = |held: &oxoffice_docx_oxml::CT_Row, live: &oxoffice_docx_oxml::CT_Row| {
            held.cells == live.cells
                && held.extra_xml == live.extra_xml
                && held.content_controls.is_empty()
                && live.content_controls.is_empty()
        };
        if !held.content_controls.is_empty()
            || !live.content_controls.is_empty()
            || held.rows.len() != live.rows.len()
        {
            return;
        }
        let rows = match scope {
            TablePropertyScope::Table => {
                if held.extra_xml != live.extra_xml
                    || held
                        .rows
                        .iter()
                        .zip(&live.rows)
                        .any(|(held, live)| !same_row(held, live))
                {
                    return;
                }
                0..live.rows.len()
            }
            TablePropertyScope::Row(row) => {
                if !held
                    .rows
                    .get(row)
                    .is_some_and(|held| same_row(held, &live.rows[row]))
                {
                    return;
                }
                row..row + 1
            }
        };
        let oxoffice_docx_oxml::BodyContent::Table(held) =
            std::sync::Arc::make_mut(&mut piece.block)
        else {
            unreachable!("checked a table above")
        };
        if matches!(scope, TablePropertyScope::Table) {
            held.properties.clone_from(&live.properties);
            held.grid.clone_from(&live.grid);
        }
        for row in rows {
            held.rows[row]
                .table_property_exception
                .clone_from(&live.rows[row].table_property_exception);
            held.rows[row]
                .properties
                .clone_from(&live.rows[row].properties);
        }
        #[cfg(debug_assertions)]
        if aligned_block(
            &pieces.blocks[body_slot],
            &self.inner.body_children()[body_slot],
        ) {
            self.assert_table_piece_reads_fresh(body_slot);
        }
    }

    /// Debug proof that the piece of the body table at `body_slot` holds the
    /// facts a full read of the table gives, body story fingerprints aside.
    /// Returns that full read.
    #[cfg(debug_assertions)]
    fn assert_table_piece_reads_fresh(&self, body_slot: usize) -> WordBlockOwners {
        let fresh = word_block_owners(&self.inner, &self.inner.body_children()[body_slot])
            .expect("a published table reads")
            .expect("a published table has owner facts");
        let piece = self.pieces.as_ref().expect("active pieces").blocks[body_slot]
            .owners
            .as_ref()
            .expect("piece owners");
        let content = |location: &oxoffice_docx::ContentLocation| {
            if location.story().kind() == oxoffice_docx::StoryKind::Body {
                unfingerprinted_location(location)
            } else {
                location.clone()
            }
        };
        let facts = |facts: &WordOwnerFacts| {
            (
                facts
                    .items
                    .iter()
                    .filter(|(location, _, _)| {
                        location.story().kind() == oxoffice_docx::StoryKind::Body
                            || location.item_kind() == oxoffice_docx::StoryItemKind::Table
                    })
                    .map(|(location, slot, read)| (content(location), *slot, read.clone()))
                    .collect::<Vec<_>>(),
                facts
                    .cells
                    .iter()
                    .map(|(table, row, cell, story)| (content(table), *row, *cell, story.clone()))
                    .collect::<Vec<_>>(),
                facts.paragraphs.clone(),
                facts.links.clone(),
            )
        };
        assert_eq!(
            facts(&piece.facts),
            facts(&fresh.facts),
            "table piece facts"
        );
        assert_eq!(
            (
                &piece.relationships,
                piece.body_items,
                piece.cell_stories,
                piece.text_boxes,
                &piece.paragraphs,
                piece.tables,
                piece.local_table_membership,
                &piece.cell_paragraphs,
            ),
            (
                &fresh.relationships,
                fresh.body_items,
                fresh.cell_stories,
                fresh.text_boxes,
                &fresh.paragraphs,
                fresh.tables,
                fresh.local_table_membership,
                &fresh.cell_paragraphs,
            ),
            "table piece counts"
        );
        assert!(piece.holds_field || !fresh.holds_field);
        fresh
    }

    /// Debug proof that a local cell publication leaves what a full read of
    /// the table gives: the piece facts the inventory reads, body story
    /// fingerprints aside, and the inventory records and cell paragraph
    /// routes of its owners.
    #[cfg(debug_assertions)]
    fn assert_local_cell_publication(
        &self,
        body_slot: usize,
        table_index: usize,
        table_ordinal: usize,
    ) {
        let fresh = self.assert_table_piece_reads_fresh(body_slot);
        let WordOwnerLocation::Table(location) = &*self.owners[table_index].location() else {
            panic!("a local cell table owner")
        };
        let cells = &self.indexes.cells_by_parent[&table_index];
        let base = cells
            .iter()
            .filter_map(|index| match &*self.owners[*index].location() {
                WordOwnerLocation::Cell { story, .. } => Some(story.owner_index()),
                _ => None,
            })
            .min()
            .expect("table cells");
        let stories = (base, 0);
        let place = |route: &oxoffice_docx::ContentLocation| {
            placed_route(route, location.story(), (location.index_path()[0], stories))
        };
        let mut placed = fresh.facts.clone();
        for (route, slot, _) in &mut placed.items {
            *route = place(route);
            *slot = slot.map(|_| body_slot);
        }
        for (table, _, _, story) in &mut placed.cells {
            *table = place(table);
            *story = placed_story(story, stories);
        }
        for (route, _, _) in &mut placed.paragraphs {
            *route = place(route);
        }
        let expected = word_piece_owner_addresses(placed, 0);
        let record = |address: &WordOwnerAddress, parent: Option<&WordOwnerLocation>| {
            format!(
                "{:?}",
                (
                    owner_location_without_body(&address.location),
                    parent.map(owner_location_without_body),
                    address.body_slot,
                    address.projection_ordinal,
                    &address.read_facts,
                )
            )
        };
        let mut wanted = expected
            .iter()
            .map(|address| {
                let mut address = body_unfingerprinted_address(address);
                if address.parent.is_none() {
                    address.projection_ordinal = Some(table_ordinal);
                }
                record(
                    &address,
                    address.parent.map(|parent| &expected[parent].location),
                )
            })
            .collect::<Vec<_>>();
        let mut actual = Vec::new();
        let mut pending = vec![table_index];
        while let Some(index) = pending.pop() {
            let owner = &self.owners[index];
            let address = owner.address();
            let parent = owner
                .parent
                .map(|parent| self.owners[parent].location().into_owned());
            actual.push(record(&address, parent.as_ref()));
            for children in [
                self.indexes.cells_by_parent.get(&index),
                self.indexes.paragraphs_by_parent.get(&index),
                self.indexes.runs_by_parent.get(&index),
                self.indexes.tables_by_parent.get(&index),
            ]
            .into_iter()
            .flatten()
            {
                pending.extend(children);
            }
        }
        wanted.sort();
        actual.sort();
        assert_eq!(actual, wanted, "local cell owner records");
        if let Some((routes, _)) = &fresh.cell_paragraphs {
            for ((_, row, cell), routes) in routes {
                assert_eq!(
                    self.indexes
                        .cell_paragraphs
                        .get(table_ordinal)
                        .and_then(|cells| cells.get(&(*row, *cell)))
                        .map(|routes| routes
                            .iter()
                            .map(AnchoredRoute::location)
                            .collect::<Vec<_>>()),
                    Some(routes.iter().map(place).collect::<Vec<_>>()),
                    "local cell paragraph routes"
                );
            }
        }
    }

    /// Keep live handles when a property edit only moves existing physical
    /// addresses. A real insert, removal or replacement keeps the conservative
    /// publication.
    pub(crate) fn adopt_property_edit(
        &mut self,
        py: Python<'_>,
        candidate: oxoffice_docx::Document,
        operation: &'static str,
    ) -> PyResult<()> {
        let (pieces, inventory) = self
            .pieces
            .as_ref()
            .expect("active pieces")
            .assemble(&candidate);
        let (locations, projections) = match inventory {
            Some((locations, projections)) => {
                #[cfg(debug_assertions)]
                assert_word_pieces_rebuild(&candidate, &locations, &projections);
                (locations, Some(projections))
            }
            None => (
                word_owner_locations(&candidate, None).map_err(|error| docx_to_pyerr(py, error))?,
                None,
            ),
        };
        if self.owners.len() == locations.len() {
            let projections = projections.unwrap_or_else(|| WordIndexProjections::of(&candidate));
            let items = WordBodyItems::derive(&locations, &projections);
            if let Some(owners) = relocated_property_owners(&self.owners, &locations, &items) {
                let indexes = WordOwnerIndexes::prepare(&owners, items, projections)?;
                self.pending_pieces = Some(pieces);
                self.owners = owners;
                self.indexes = indexes;
                self.install_inner(candidate);
                return Ok(());
            }
        }
        self.publish_owner_candidate(py, candidate, operation, WordPublication::Conservative)
    }

    pub(super) fn remove_note_by_id(
        &mut self,
        py: Python<'_>,
        id: &Bound<'_, PyAny>,
        endnote: bool,
    ) -> PyResult<()> {
        if !id.is_instance_of::<PyInt>() || id.is_instance_of::<PyBool>() {
            return Err(PyTypeError::new_err("note id must be a non-bool integer"));
        }
        let id = id.extract::<i32>()?;
        let mut candidate = self.inner.clone_for_staging();
        if endnote {
            candidate.remove_endnote(id)
        } else {
            candidate.remove_footnote(id)
        }
        .map_err(|e| docx_to_pyerr(py, e))?;
        self.publish_api_candidate(py, candidate, "Document.remove_note")
    }

    pub(super) fn edit_content_control(
        &mut self,
        py: Python<'_>,
        location: &PyContentLocation,
        operation: &'static str,
        edit: impl FnOnce(
            &mut oxoffice_docx::Document,
            &oxoffice_docx::ContentLocation,
        ) -> oxoffice_docx::Result<()>
        + Send,
    ) -> PyResult<()> {
        let location = location.native(py, self)?;
        let mut candidate = self.inner.clone_for_staging();
        py.detach(|| edit(&mut candidate, &location))
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, operation)
    }

    /// Make a property edit of a content control (tag, alias, lock,
    /// appearance, placeholder, type, data binding) on the direct body block
    /// that holds it, when the inventory places the control in one, and
    /// publish that block alone: the control and every element around it
    /// stay, so every handle stays. Any other control, and an edit the facade
    /// leaves to its staged route, takes [`Self::edit_content_control`].
    pub(super) fn edit_content_control_property(
        &mut self,
        py: Python<'_>,
        location: &PyContentLocation,
        operation: &'static str,
        edit: oxoffice_docx::ContentControlEdit,
    ) -> PyResult<()> {
        /// Stops holding the edit undo however the edit ends; an edit that
        /// panics leaves the document as it was.
        struct Held<'a> {
            document: &'a mut oxoffice_docx::Document,
            armed: bool,
        }
        impl Drop for Held<'_> {
            fn drop(&mut self) {
                let undo = self.document.take_edit_undo();
                if self.armed
                    && let Some(undo) = undo
                {
                    self.document.undo_edit(undo);
                }
            }
        }
        impl Held<'_> {
            fn take(mut self) -> Option<oxoffice_docx::EditUndo> {
                self.armed = false;
                self.document.take_edit_undo()
            }
        }
        if let Some((native, block)) = self.control_block(py, location)? {
            self.inner.hold_edit_undo();
            let held = Held {
                document: &mut self.inner,
                armed: true,
            };
            let edit = &edit;
            let (result, held) = py.detach(move || {
                let result = held
                    .document
                    .edit_content_control_in_block(&native, block, edit);
                (result, held)
            });
            let undo = held.take();
            match result.map_err(|error| docx_to_pyerr(py, error))? {
                None => {}
                Some(changed) => {
                    if !changed || self.follow_control_property_edit(block.slot) {
                        #[cfg(debug_assertions)]
                        if self.snapshot_scope.is_some() {
                            self.assert_word_inventory_rebuilds(false);
                        }
                        return Ok(());
                    }
                    // The block reads other owners than it held: put it back
                    // and publish the staged route's document.
                    if let Some(undo) = undo {
                        self.inner.undo_edit(undo);
                    }
                }
            }
        }
        self.edit_content_control(py, location, operation, move |document, location| {
            document.edit_content_control(location, edit)
        })
    }

    /// The facade location of the content control at `location` and the
    /// place of the control in its direct body block, from the pieces of
    /// the inventory, without reading the stories; `None` when the control
    /// is not in the main part's body, its table cells or text boxes, or the
    /// pieces do not tell.
    fn control_block(
        &mut self,
        py: Python<'_>,
        location: &PyContentLocation,
    ) -> PyResult<Option<(oxoffice_docx::ContentLocation, oxoffice_docx::ControlBlock)>> {
        use oxoffice_docx::StoryKind;
        if location.is_end
            || location.kind != story_item_kind_name(oxoffice_docx::StoryItemKind::ContentControl)
            || location.index_path.len() != 1
        {
            return Ok(None);
        }
        let story = &location.story;
        let Some(kind) = [StoryKind::Body, StoryKind::TableCell, StoryKind::TextBox]
            .into_iter()
            .find(|kind| story_kind_name(*kind) == story.kind)
        else {
            return Ok(None);
        };
        if kind == StoryKind::Body && story.owner_index != 0 {
            return Ok(None);
        }
        self.validate_owned_story(py, story)?;
        self.ensure_word_authority(py)?;
        let Some(pieces) = self.pieces.as_ref() else {
            return Ok(None);
        };
        let context = self.inner.body_item_context();
        if pieces.context.as_ref() != Some(&context)
            || pieces.blocks.len() != self.inner.body_children().len()
            || story.part_name != context.part_name()
        {
            return Ok(None);
        }
        // A body control is counted among the body's items, a control of a
        // cell or a text box by its story among the block's stories.
        let index = location.index_path[0];
        let target = if kind == StoryKind::Body {
            index
        } else {
            story.owner_index
        };
        let mut base = 0;
        for (slot, piece) in pieces.blocks.iter().enumerate() {
            let Some(owners) = piece.owners.as_ref() else {
                return Ok(None);
            };
            let count = match kind {
                StoryKind::Body => owners.body_items,
                StoryKind::TableCell => owners.cell_stories,
                _ => owners.text_boxes,
            };
            if target < base + count {
                let local = target - base;
                let block = if kind == StoryKind::Body {
                    oxoffice_docx::ControlBlock {
                        slot,
                        owner_index: 0,
                        item_index: local,
                    }
                } else {
                    oxoffice_docx::ControlBlock {
                        slot,
                        owner_index: local,
                        item_index: index,
                    }
                };
                let native = oxoffice_docx::ContentLocation::new(
                    oxoffice_docx::StoryId::new(kind, story.part_name.clone(), story.owner_index),
                    oxoffice_docx::StoryItemKind::ContentControl,
                    location.index_path.clone(),
                );
                return Ok(Some((native, block)));
            }
            base += count;
        }
        Ok(None)
    }

    /// Read the owners of the direct body block at `slot` again after a
    /// property edit of a control inside it, and keep them in its piece;
    /// false, changing nothing, when the block now holds other stories,
    /// items, paragraphs, tables or links than it held.
    fn follow_control_property_edit(&mut self, slot: usize) -> bool {
        let Some(live) = self.inner.body_children().get(slot) else {
            return false;
        };
        let Ok(Some(owners)) = word_block_owners(&self.inner, live) else {
            return false;
        };
        let Some(pieces) = &mut self.pieces else {
            return false;
        };
        if pieces.context.as_ref() != Some(&self.inner.body_item_context())
            || pieces.blocks.len() != self.inner.body_children().len()
        {
            return false;
        }
        let Some(held) = pieces.blocks[slot].owners.as_ref() else {
            return false;
        };
        let shape = |owners: &WordBlockOwners| {
            (
                owners.body_items,
                owners.cell_stories,
                owners.text_boxes,
                owners.tables,
                owners.paragraphs.clone(),
                owners.relationships.clone(),
                owners.facts.links.len(),
            )
        };
        if shape(held) != shape(&owners) {
            return false;
        }
        pieces.blocks[slot] = std::sync::Arc::new(WordBlockPiece {
            block: std::sync::Arc::new(live.clone()),
            owners: Some(owners),
        });
        true
    }

    pub(super) fn publish_api_candidate(
        &mut self,
        py: Python<'_>,
        candidate: oxoffice_docx::Document,
        operation: &'static str,
    ) -> PyResult<()> {
        if self
            .inner
            .saves_as(&candidate)
            .map_err(|e| docx_to_pyerr(py, e))?
        {
            return Ok(());
        }
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            operation,
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }
    pub(super) fn publish_form_candidate(
        &mut self,
        py: Python<'_>,
        candidate: oxoffice_docx::Document,
        operation: &'static str,
        publication: WordPublication,
    ) -> PyResult<()> {
        if self
            .inner
            .saves_as(&candidate)
            .map_err(|e| docx_to_pyerr(py, e))?
        {
            return Ok(());
        }
        let prepared = self.prepare_word_revision(py, &candidate, operation, publication)?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }
    pub(super) fn set_control_value(
        &mut self,
        py: Python<'_>,
        name: &str,
        value: &str,
        by_tag: bool,
    ) -> PyResult<usize> {
        let missing = if by_tag {
            self.inner.content_controls_by_tag(name).is_empty()
        } else {
            self.inner.content_controls_by_alias(name).is_empty()
        };
        if missing {
            return Err(PyKeyError::new_err(name.to_owned()));
        }
        let mut candidate = self.inner.clone_for_staging();
        let count = if by_tag {
            candidate.set_content_control_value_by_tag(name, value)
        } else {
            candidate.set_content_control_value_by_alias(name, value)
        }
        .map_err(|error| match error {
            oxoffice_docx::Error::Oxml(oxoffice_docx_oxml::OxmlError::InvalidValue(message)) => {
                PyValueError::new_err(message)
            }
            other => docx_to_pyerr(py, other),
        })?;
        self.publish_api_candidate(py, candidate, "Document.set_content_control_value")?;
        Ok(count)
    }

    pub(super) fn create_note(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        reference: &Bound<'_, PyAny>,
        text: &str,
        endnote: bool,
    ) -> PyResult<Py<PyNote>> {
        let owner = slf.clone().unbind();
        let (location, run_index) = if let Ok(reference) = reference.cast::<PyParagraph>() {
            let paragraph = reference.borrow();
            if !paragraph.belongs_to(py, &owner) {
                return Err(PyValueError::new_err(
                    "note reference belongs to a different document",
                ));
            }
            let location = paragraph.validate(py)?;
            let document = slf.borrow();
            let run_index = location
                .read(&document.inner, |paragraph| paragraph.run_count())
                .map_err(|error| docx_to_pyerr(py, error))?;
            (
                location
                    .native(&document.inner)
                    .map_err(|error| docx_to_pyerr(py, error))?,
                run_index,
            )
        } else if let Ok(reference) = reference.cast::<crate::run::PyRun>() {
            let run = reference.borrow();
            if !run.belongs_to(py, &owner) {
                return Err(PyValueError::new_err(
                    "note reference belongs to a different document",
                ));
            }
            let (location, index) = run.validate(py)?;
            let document = slf.borrow();
            (
                location
                    .native(&document.inner)
                    .map_err(|error| docx_to_pyerr(py, error))?,
                index + 1,
            )
        } else {
            return Err(PyTypeError::new_err(
                "note reference must be a Paragraph or Run",
            ));
        };
        let mut document = slf.borrow_mut();
        let mut candidate = document.inner.clone_for_staging();
        let position = oxoffice_docx::StoryRunPosition {
            location,
            run_index,
        };
        let id = if endnote {
            candidate.create_endnote_at(&position, text)
        } else {
            candidate.create_footnote_at(&position, text)
        }
        .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.add_note",
            crate::document::WordPublication::ParagraphAppend(position.location.clone()),
        )?;
        let path = document
            .revisions
            .capture_prepared(&prepared.revision, smallvec![])
            .map_err(revision_to_pyerr)?;
        let result = Py::new(
            py,
            PyNote {
                document: owner,
                id,
                endnote,
                path,
            },
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(result)
    }
}

impl PyDocument {
    pub(super) fn ensure_word_authority(&mut self, py: Python<'_>) -> PyResult<()> {
        if self.snapshot_scope.is_some() {
            return Ok(());
        }
        let (locations, projections, pieces) = validate_word_facts(&self.inner, py)?;
        let mut scopes = Vec::new();
        scopes
            .try_reserve(
                locations
                    .iter()
                    .map(|address| address.location.scopes().len())
                    .sum::<usize>()
                    + 1,
            )
            .map_err(|_| {
                pyo3::exceptions::PyMemoryError::new_err("Word scope allocation failed")
            })?;
        for address in &locations {
            scopes.extend_from_slice(address.location.scopes());
        }
        scopes.push(RevisionScope::Text);
        let mut owners = Vec::new();
        owners.try_reserve(locations.len()).map_err(|_| {
            pyo3::exceptions::PyMemoryError::new_err("Word owner allocation failed")
        })?;
        let items = WordBodyItems::derive(&locations, &projections);
        let mut indexes = WordOwnerIndexes::prepare_addresses(&locations, items, projections)?;
        // This is the last recoverably fallible step. All packet storage is reserved.
        let states = self
            .revisions
            .initialize_scopes(&scopes)
            .map_err(revision_to_pyerr)?;
        let snapshot_scope = states.last().expect("snapshot scope allocated").clone();
        append_word_owners_with_states(
            &mut owners,
            locations,
            &states[..states.len() - 1],
            &indexes.items,
        );
        for (index, owner) in owners.iter().enumerate() {
            indexes
                .by_generation
                .insert(owner.owner.generation(), index);
        }
        self.owners = owners;
        self.indexes = indexes;
        self.snapshot_scope = Some(snapshot_scope);
        self.pieces = Some(pieces);
        Ok(())
    }
}

impl PyDocument {
    /// Split a run of the direct body paragraph at `body_index`. The revision
    /// advances only when a continuation run is created.
    pub(super) fn split_body_run(
        &mut self,
        py: Python<'_>,
        body_index: usize,
        run_index: usize,
        character_offset: usize,
    ) -> PyResult<usize> {
        let paragraph_index = self.inner.paragraph_index_of_content(body_index);
        let run_count = |document: &oxoffice_docx::Document| {
            paragraph_index
                .and_then(|index| document.paragraph(index))
                .map(|paragraph| paragraph.run_count())
        };
        let before = run_count(&self.inner);
        let mut candidate = self.inner.clone_for_staging();
        let boundary = candidate
            .split_run(body_index, run_index, character_offset)
            .map_err(|error| docx_to_pyerr(py, error))?;
        if run_count(&candidate) != before {
            // The route comes from the inventory; a paragraph without one
            // retires every handle.
            let publication = paragraph_index
                .and_then(|index| {
                    self.paragraph_route(&ParagraphLocation::Body(index), None)
                        .ok()
                })
                .map_or(
                    WordPublication::Conservative,
                    WordPublication::ParagraphEdit,
                );
            self.publish_block_edit(py, candidate, "Document.split_run", publication)?;
        }
        Ok(boundary)
    }

    /// Run a native mutation that reports how many things it changed.
    ///
    /// The GIL is released while it runs, and live handles are staled only
    /// when the count is nonzero.
    pub(super) fn counted_mutation<F>(
        &mut self,
        py: Python<'_>,
        operation: &'static str,
        mutation: F,
    ) -> PyResult<usize>
    where
        F: FnOnce(&mut oxoffice_docx::Document) -> oxoffice_docx::Result<usize> + Send,
    {
        let mut candidate = self.inner.clone_for_staging();
        let count = py
            .detach(|| mutation(&mut candidate))
            .map_err(|error| docx_to_pyerr(py, error))?;
        if count > 0 {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                operation,
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(count)
    }
}

impl PyDocument {
    pub(super) fn refuse_unsavable_body_edit(&self, py: Python<'_>) -> PyResult<()> {
        match self.inner.unsavable_body_edit() {
            Some(error) => Err(docx_to_pyerr(
                py,
                oxoffice_docx::Error::Other(error.to_owned()),
            )),
            None => Ok(()),
        }
    }

    /// XML 1.0 text before the namespace refusal. Index stays a Python
    /// `IndexError` at the insert callers. A namespace `Other` is left for
    /// [`Self::refuse_unsavable_body_edit`] so the diagnostic stays one path.
    pub(super) fn refuse_non_xml_paragraph_text(&self, py: Python<'_>, text: &str) -> PyResult<()> {
        match self.inner.preflight_checked_paragraph(None, text) {
            Err(oxoffice_docx::Error::Oxml(error)) => {
                Err(docx_to_pyerr(py, oxoffice_docx::Error::Oxml(error)))
            }
            Err(oxoffice_docx::Error::Other(_)) => Ok(()),
            Err(error) => Err(docx_to_pyerr(py, error)),
            Ok(()) => Ok(()),
        }
    }

    /// Append a list item. Its numbering half runs in place first, and the
    /// facade keeps a journal of it; the paragraph is then published by the
    /// body splice when the body allows one, as `add_paragraph` publishes
    /// its paragraph, and by a checked revision otherwise. Either way it
    /// keeps the handles `add_paragraph` keeps. A step that fails or panics
    /// before the paragraph reaches the document undoes the numbering half,
    /// so the document is left as it was. After it, only the commit of the
    /// splice remains, the window `add_paragraph` has too.
    pub(super) fn append_list_item(
        slf: Py<Self>,
        py: Python<'_>,
        kind: oxoffice_docx::ListItemKind,
        text: &str,
        level: u32,
        operation: &'static str,
    ) -> PyResult<Py<PyParagraph>> {
        let mut document = slf.borrow_mut(py);
        document.refuse_unsavable_body_edit(py)?;
        let index = document.inner.paragraph_count();
        let slot = document.inner.content_count();
        let (num_id, journal) = document
            .inner
            .begin_list_item(kind, text, level)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let paragraph = oxoffice_docx::Document::list_item_paragraph(text, num_id, level);
        let staged = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Self::stage_appended_paragraph(
                &slf,
                py,
                &mut document,
                index,
                slot,
                &paragraph,
                operation,
            )
        }));
        let staged = match staged {
            Ok(Ok(staged)) => staged,
            Ok(Err(error)) => {
                document.inner.undo_numbering_edit(journal);
                return Err(error);
            }
            Err(panic) => {
                document.inner.undo_numbering_edit(journal);
                std::panic::resume_unwind(panic)
            }
        };
        match staged {
            StagedAppend::Splice(prepared, result) => {
                document.inner.append_body_paragraph(paragraph);
                debug_assert_eq!(prepared.plan.paragraph, index);
                document.commit_body_splice(*prepared, &WordPublication::AppendBody)?;
                document
                    .inner
                    .keep_body_savable_after_typed_insert(slot, true);
                Ok(result)
            }
            StagedAppend::Revision(candidate, result) => {
                document.install_inner(*candidate);
                Ok(result)
            }
        }
    }

    /// Every fallible step of publishing `paragraph`, a plain paragraph made
    /// outside the document, appended at body `slot` as paragraph `index`,
    /// before it reaches the document: the prepared splice, or the committed
    /// revision of a candidate that holds it.
    #[allow(clippy::too_many_arguments)]
    fn stage_appended_paragraph(
        slf: &Py<Self>,
        py: Python<'_>,
        document: &mut PyDocument,
        index: usize,
        slot: usize,
        paragraph: &oxoffice_docx_oxml::CT_P,
        operation: &'static str,
    ) -> PyResult<StagedAppend> {
        let publication = WordPublication::AppendBody;
        let path = smallvec![PathSeg::Body(0), PathSeg::Para(index)];
        if let Ok(Some(facts)) = document.inner.direct_body_paragraph_facts(paragraph)
            && let Some(plan) = document.plan_body_splice(
                py,
                BodySplice {
                    slot,
                    removed: 0,
                    inserted: Some(SplicedBodyItem::Paragraph(facts.0, facts.1)),
                },
            )?
        {
            let prepared = document.prepare_body_splice(plan, operation, &publication)?;
            let path = document
                .revisions
                .capture_prepared(&prepared.revision, path)
                .map_err(revision_to_pyerr)?;
            let result = Py::new(
                py,
                PyParagraph::new(slf.clone_ref(py), path)
                    .bind_prepared_splice(document, &prepared)?,
            )?;
            return Ok(StagedAppend::Splice(Box::new(prepared), result));
        }
        let mut candidate = document.inner.clone_for_staging();
        candidate.append_body_paragraph(paragraph.clone());
        let prepared = document.prepare_word_revision(py, &candidate, operation, publication)?;
        let path = document
            .revisions
            .capture_prepared(&prepared.revision, path)
            .map_err(revision_to_pyerr)?;
        let result = Py::new(
            py,
            PyParagraph::new(slf.clone_ref(py), path)
                .bind_prepared(py, document, &prepared, &candidate, None)?,
        )?;
        document.commit_word_revision(prepared)?;
        Ok(StagedAppend::Revision(Box::new(candidate), result))
    }

    pub(super) fn append_paragraph_candidate(
        slf: Py<Self>,
        py: Python<'_>,
        text: &str,
        style: Option<&str>,
        page_break: bool,
    ) -> PyResult<Py<PyParagraph>> {
        let mut document = slf.borrow_mut(py);
        let index = document.inner.paragraph_count();
        let style = style
            .map(|value| {
                style_id_of_type(&document.inner, value, oxoffice_docx::StyleType::Paragraph)
            })
            .transpose()?;
        document.refuse_non_xml_paragraph_text(py, text)?;
        document.refuse_unsavable_body_edit(py)?;
        // The same edit builds the paragraph read for the inventory and the
        // paragraph placed.
        let edit = |paragraph: &mut oxoffice_docx::Paragraph<'_>| {
            if let Some(style) = &style {
                paragraph.set_style(style);
            }
            if page_break {
                paragraph
                    .add_run("")
                    .add_break(oxoffice_docx::BreakKind::Page);
            }
        };
        let mut appended = oxoffice_docx::Document::detached_text_paragraph(text);
        edit(&mut oxoffice_docx::Paragraph::detached(&mut appended));
        let slot = document.inner.content_count();
        let splice = document.inner.direct_body_paragraph_facts(&appended);
        if let Ok(Some(facts)) = splice
            && let Some(plan) = document.plan_body_splice(
                py,
                BodySplice {
                    slot,
                    removed: 0,
                    inserted: Some(SplicedBodyItem::Paragraph(facts.0, facts.1)),
                },
            )?
        {
            // Every fallible step precedes the in-place native append.
            let publication = crate::document::WordPublication::AppendBody;
            let prepared =
                document.prepare_body_splice(plan, "Document.add_paragraph", &publication)?;
            let path = document
                .revisions
                .capture_prepared(
                    &prepared.revision,
                    smallvec![PathSeg::Body(0), PathSeg::Para(index)],
                )
                .map_err(revision_to_pyerr)?;
            let result = Py::new(
                py,
                PyParagraph::new(slf.clone_ref(py), path)
                    .bind_prepared_splice(&document, &prepared)?,
            )?;
            let savable = document.inner.unsavable_body_edit().is_none();
            edit(&mut document.inner.add_paragraph(text));
            debug_assert_eq!(prepared.plan.paragraph, index);
            document.commit_body_splice(prepared, &publication)?;
            document
                .inner
                .keep_body_savable_after_typed_insert(slot, savable);
            return Ok(result);
        }
        let mut candidate = document.inner.clone_for_staging();
        edit(&mut candidate.add_paragraph(text));
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.add_paragraph",
            crate::document::WordPublication::AppendBody,
        )?;
        let path = document
            .revisions
            .capture_prepared(
                &prepared.revision,
                smallvec![PathSeg::Body(0), PathSeg::Para(index)],
            )
            .map_err(revision_to_pyerr)?;
        let result = Py::new(
            py,
            PyParagraph::new(slf.clone_ref(py), path)
                .bind_prepared(py, &document, &prepared, &candidate, None)?,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(result)
    }
}
