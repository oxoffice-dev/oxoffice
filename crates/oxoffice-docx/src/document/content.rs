//! Inserting, finding and removing body content, and whether the body still saves after an edit.

use std::collections::BTreeSet;

use super::*;

impl Document {
    // ---- Content insertion ----

    /// Record that typed edits of this body can still be saved after the
    /// typed insert of the block at `slot`, when they could before it
    /// (`savable_before`), without computing [`Self::unsavable_body_edit`]
    /// again over the whole main part. That answer depends on the namespace
    /// declarations of the root and the body and on the nested namespace
    /// owners the body keeps from its source. A typed block declares no
    /// namespace, and the owners are told apart by element name and structure
    /// (`replay_nested_namespace_declarations_in`), so a block that holds no
    /// element named like an owner cannot change it. Any other block, or a
    /// main part no longer as it was read, leaves the answer to be computed
    /// again. The caller makes the insert and any edit of the inserted block
    /// before it calls this.
    #[doc(hidden)]
    pub fn keep_body_savable_after_typed_insert(&mut self, slot: usize, savable_before: bool) {
        if !savable_before || self.unsavable_body_edit_cache.get().is_some() {
            return;
        }
        let Some(kept) = self.kept_main_part() else {
            return;
        };
        let Some(owners) = kept.nested_namespace_owners() else {
            return;
        };
        let Some(block) = self.document.body.content.get(slot) else {
            return;
        };
        let Ok(xml) = self.document.body_item_xml(block) else {
            return;
        };
        let names = owners
            .iter()
            .map(|owner| owner.local_name.as_bytes())
            .collect::<HashSet<_>>();
        if !xml_holds_element_named(&xml, &names) {
            self.unsavable_body_edit_cache = OnceLock::from(None);
        }
    }

    /// [`Self::invalidate_layout`] after an edit that leaves the main part
    /// and the body alone (a settings or package part edit): what a save of
    /// the body answers stays.
    pub(crate) fn invalidate_layout_keeping_body_answer(&mut self) {
        let kept = self.unsavable_body_edit_cache.get().cloned();
        self.invalidate_layout();
        if let Some(kept) = kept {
            self.unsavable_body_edit_cache = OnceLock::from(kept);
        }
    }

    /// Record that typed edits of this body can still be saved after a block
    /// swap whose caller proved it changes no namespace declaration and no
    /// namespace owner, when they could before it. The caller asked
    /// [`Self::unsavable_body_edit`] before the swap.
    pub(crate) fn keep_body_savable_after_swap(&mut self) {
        if self.unsavable_body_edit_cache.get().is_none() {
            self.unsavable_body_edit_cache = OnceLock::from(None);
        }
    }

    /// The error a save of this body would return after a typed edit, when the
    /// retained nested owners cannot be replayed or a serializer prefix is
    /// shadowed. `None` is not a general proof that the document is savable.
    #[doc(hidden)]
    pub fn unsavable_body_edit(&self) -> Option<&str> {
        match self.unsavable_body_edit_cache.get() {
            None => {
                let computed = self.compute_unsavable_body_edit();
                let _ = self.unsavable_body_edit_cache.set(computed);
            }
            // A kept answer is the one the whole main part gives now, computed
            // on a cold copy so that no cache of this document is warmed.
            #[cfg(debug_assertions)]
            Some(kept) => {
                let reference = self.clone_for_debug_reference();
                assert_eq!(
                    kept,
                    &reference.compute_unsavable_body_edit(),
                    "kept unsavable body answer"
                );
            }
            #[cfg(not(debug_assertions))]
            Some(_) => {}
        }
        self.unsavable_body_edit_cache
            .get()
            .and_then(Option::as_deref)
    }

    fn compute_unsavable_body_edit(&self) -> Option<String> {
        let xml = self.package.get_part(&self.doc_part_name)?;
        if let Some(prefix) = unsafe_serializer_namespace_prefix(
            &self.root_namespace_declarations,
            &self.body_namespace_declarations,
            Some(xml),
        ) {
            return Some(shadowed_namespace_diagnostic(&prefix));
        }
        if let Some(result) = self.proved_unsavable_body_edit(xml) {
            return result;
        }
        if let Some(reason) = replay_unsavable_body_edit(xml, &self.document) {
            return Some(reason);
        }
        let owners = nested_modeled_namespace_owners(xml).ok()?;
        if owners.is_empty() {
            return None;
        }
        // The untouched model can still replay. A real body insertion is what
        // makes the same owners unsavable, so prove that on a discarded clone.
        let mut probe = self.clone_for_staging();
        probe.add_paragraph("");
        match probe.flush_document_to_package() {
            Err(error) => {
                let message = error.to_string();
                message.contains("namespace").then_some(message)
            }
            Ok(_) => None,
        }
    }

    /// Reuse source-owner facts only for the exact retained part. Current
    /// replay and the discarded insertion still get their own writer proof.
    fn proved_unsavable_body_edit(&self, source: &[u8]) -> Option<Option<String>> {
        let kept = self.kept_main_part();
        let owners = if let Some(kept) = &kept {
            if kept.root_namespace_declarations != self.root_namespace_declarations
                || kept.body_namespace_declarations != self.body_namespace_declarations
                || kept.body_namespace_bindings != self.body_namespace_bindings
            {
                return None;
            }
            kept.nested_namespace_owners()?.to_vec()
        } else {
            self.main_part_blocks
                .as_ref()?
                .retained_nested_owners(source)?
        };
        if owners.is_empty() {
            return Some(None);
        }
        if let Some(prefix) = unsafe_nested_namespace_prefix(&owners) {
            return Some(Some(shadowed_namespace_diagnostic(&prefix)));
        }
        let (xml, record) = self.canonical_source().ok()?;
        let record = record?;
        let spans = record.proved_namespace_spans(&xml)?;
        if let Err(error) =
            replay_nested_namespace_declarations_with_spans(&xml, &owners, Some(&spans))
        {
            return Some(Some(error.to_string()));
        }
        let item = BodyContent::Paragraph(Box::new(Self::detached_text_paragraph("")));
        let mut block = BodyBlockScan::from_source(self.document.body_item_xml(&item).ok()?);
        block.staged = Some(Arc::new(item));
        let mut blocks = record.blocks.as_ref().clone();
        blocks.push(Arc::new(block));
        let (probe, mut proof) = MainPartBlocks::assemble(
            &self.document,
            blocks,
            None,
            Some((&record.head, &record.tail)),
        )
        .ok()??;
        proof.part = Some(Arc::new(probe.clone()));
        let spans = proof.proved_namespace_spans(&probe)?;
        Some(
            replay_nested_namespace_declarations_with_spans(&probe, &owners, Some(&spans))
                .err()
                .map(|error| error.to_string())
                .filter(|message| message.contains("namespace")),
        )
    }

    pub(super) fn refuse_unsavable_body_edit(&self) -> Result<()> {
        match self.unsavable_body_edit() {
            Some(reason) => Err(Error::Other(reason.to_owned())),
            None => Ok(()),
        }
    }

    #[doc(hidden)]
    pub fn preflight_checked_paragraph(&self, index: Option<usize>, text: &str) -> Result<()> {
        if let Some(index) = index
            && index > self.content_count()
        {
            return Err(Error::Other("content index out of range".to_owned()));
        }
        oxoffice_core::xml::reject_non_xml_characters("paragraph text", text)?;
        self.refuse_unsavable_body_edit()
    }

    fn preflight_checked_table(&self, index: Option<usize>) -> Result<()> {
        if let Some(index) = index
            && index > self.content_count()
        {
            return Err(Error::Other("content index out of range".to_owned()));
        }
        self.refuse_unsavable_body_edit()
    }

    /// Checked [`Self::add_paragraph`]. An out of range insert index, a
    /// character XML cannot carry, or a namespace owner that would make the
    /// save fail returns before the model changes.
    pub fn try_add_paragraph(&mut self, text: &str) -> Result<Paragraph<'_>> {
        self.preflight_checked_paragraph(None, text)?;
        Ok(self.add_paragraph(text))
    }

    /// Checked [`Self::insert_paragraph`]. The index is tested first.
    pub fn try_insert_paragraph(&mut self, index: usize, text: &str) -> Result<Paragraph<'_>> {
        self.preflight_checked_paragraph(Some(index), text)?;
        Ok(self.insert_paragraph(index, text))
    }

    /// Checked [`Self::add_table`]. Zero rows and zero columns stay accepted.
    pub fn try_add_table(&mut self, rows: usize, cols: usize) -> Result<Table<'_>> {
        self.preflight_checked_table(None)?;
        Ok(self.add_table(rows, cols))
    }

    /// Checked [`Self::insert_table`]. The index is tested first.
    pub fn try_insert_table(
        &mut self,
        index: usize,
        rows: usize,
        cols: usize,
    ) -> Result<Table<'_>> {
        self.preflight_checked_table(Some(index))?;
        Ok(self.insert_table(index, rows, cols))
    }

    /// Prepare one owner-free direct-body cell edit without changing this document.
    ///
    /// Returns the edited cell alone in a frame of its table: the table's
    /// properties and grid, and one row with the row's properties and this
    /// cell. The frame is validated as a body holding it alone. The model
    /// limits bound the nesting along each path, and the paths through this
    /// cell are the same in the frame and in the table.
    #[doc(hidden)]
    pub fn direct_body_table_cell_candidate(
        &self,
        body_slot: usize,
        row: usize,
        cell: usize,
        edit: impl FnOnce(&mut crate::table::Cell<'_>) -> Result<()>,
    ) -> Result<Option<CT_Tbl>> {
        let source = self
            .document
            .body
            .content
            .get(body_slot)
            .and_then(|item| match item {
                BodyContent::Table(table) => Some(table),
                _ => None,
            })
            .ok_or_else(|| Error::Other(format!("table index {body_slot} is out of range")))?;
        if !self
            .local_cell_source
            .as_ref()
            .is_some_and(|proof| proof.applies(self))
        {
            return Ok(None);
        }
        // A row or cell content control numbers the rows or cells apart
        // from the table's own; the checked route edits such a table.
        if crate::table::table_holds_controls(source) {
            return Ok(None);
        }
        let Some(source_row) = source.rows.get(row) else {
            return Ok(None);
        };
        let Some(selected) = source_row.cells.get(cell) else {
            return Ok(None);
        };
        if selected.content.iter().any(|item| !matches!(item, oxoffice_docx_oxml::table::CellContent::Paragraph(paragraph) if paragraph.content_controls.is_empty())) {
            return Ok(None);
        }
        let mut edited = selected.clone();
        edit(&mut crate::table::Cell { inner: &mut edited })?;
        let frame = CT_Tbl {
            properties: source.properties.clone(),
            grid: source.grid.clone(),
            rows: vec![oxoffice_docx_oxml::table::CT_Row {
                table_property_exception: source_row.table_property_exception.clone(),
                properties: source_row.properties.clone(),
                cells: vec![edited],
                // The start tag attributes, not the raw children between cells.
                extra_xml: source_row
                    .extra_xml
                    .iter()
                    .filter(|(at, _)| *at == usize::MAX)
                    .cloned()
                    .collect(),
                content_controls: Vec::new(),
            }],
            extra_xml: Vec::new(),
            content_controls: Vec::new(),
        };
        let mut body = CT_Body {
            content: vec![BodyContent::Table(Box::new(frame))],
            sect_pr: None,
        };
        body.validate_resource_limits()?;
        let BodyContent::Table(table) = body.content.pop().expect("singleton table") else {
            unreachable!()
        };
        Ok(Some(*table))
    }

    /// Install the cell of a prepared frame at `row` and `cell` of the table,
    /// under the same exclusive source qualification.
    #[doc(hidden)]
    pub fn commit_direct_body_table_cell(
        &mut self,
        body_slot: usize,
        row: usize,
        cell: usize,
        frame: CT_Tbl,
    ) {
        assert!(
            self.local_cell_source
                .as_ref()
                .is_some_and(|proof| proof.applies(self))
        );
        let BodyContent::Table(existing) = &mut self.document.body.content[body_slot] else {
            panic!("qualified direct table changed kind")
        };
        let edited = frame
            .rows
            .into_iter()
            .next()
            .and_then(|row| row.cells.into_iter().next())
            .expect("a prepared cell frame");
        existing.rows[row].cells[cell] = edited;
        self.invalidate_layout();
        self.unsavable_body_edit_cache = OnceLock::from(None);
    }

    /// Get the number of body content elements (paragraphs + tables).
    pub fn content_count(&self) -> usize {
        self.document.body.content_count()
    }

    /// Insert a paragraph at the given body index.
    ///
    /// Returns a mutable `Paragraph` for further configuration. A character
    /// XML 1.0 cannot carry is dropped; [`Self::try_insert_paragraph`]
    /// refuses it instead.
    ///
    /// A bookmark start written between blocks belongs to the block after
    /// it (Google Docs writes a heading's bookmark so): an insertion before
    /// that block goes before its leading bookmark starts, so the bookmark
    /// still wraps that block alone. The new block then sits at the index of
    /// the first of those starts, before `index`.
    /// # Panics
    ///
    /// Panics if `index > content_count()`. (Unlike [`Self::insert_document`]
    /// and [`Self::insert_toc`], which clamp an out-of-range index to the end.)
    pub fn insert_paragraph(&mut self, index: usize, text: &str) -> Paragraph<'_> {
        self.invalidate_layout();
        let index = self.block_insertion_index(index);
        self.document
            .body
            .insert_paragraph(index, Self::detached_text_paragraph(text));
        match &mut self.document.body.content[index] {
            BodyContent::Paragraph(p) => Paragraph { inner: p },
            _ => unreachable!(),
        }
    }

    /// Insert a table at the given body index.
    ///
    /// Returns a mutable `Table` for further configuration.
    ///
    /// A bookmark start written between blocks belongs to the block after
    /// it (Google Docs writes a heading's bookmark so): an insertion before
    /// that block goes before its leading bookmark starts, so the bookmark
    /// still wraps that block alone. The new block then sits at the index of
    /// the first of those starts, before `index`.
    /// A `cols` of 0 produces a table with no columns rather than panicking.
    ///
    /// # Panics
    ///
    /// Panics if `index > content_count()`. (Unlike [`Self::insert_document`]
    /// and [`Self::insert_toc`], which clamp an out-of-range index to the end.)
    pub fn insert_table(&mut self, index: usize, rows: usize, cols: usize) -> Table<'_> {
        self.invalidate_layout();
        let index = self.block_insertion_index(index);
        self.document
            .body
            .insert_table(index, Self::detached_table(rows, cols));
        match &mut self.document.body.content[index] {
            BodyContent::Table(t) => Table { inner: t },
            _ => unreachable!(),
        }
    }

    /// The table [`Self::insert_table`] and [`Self::add_table`] place for
    /// `rows` and `cols`, built before it is placed.
    #[doc(hidden)]
    pub fn detached_table(rows: usize, cols: usize) -> CT_Tbl {
        use oxoffice_docx_oxml::table::{
            CT_Row, CT_TblGrid, CT_TblGridCol, CT_TblPr, CT_TblWidth, CT_Tc,
        };
        use oxoffice_docx_oxml::units::Twips;

        let col_width = Twips(9360 / cols.max(1) as i32);
        let grid = CT_TblGrid {
            columns: (0..cols)
                .map(|_| CT_TblGridCol { width: col_width })
                .collect(),
            ..Default::default()
        };

        let mut tbl = CT_Tbl::new();
        tbl.properties = Some(CT_TblPr {
            width: Some(CT_TblWidth::dxa(col_width.0 * cols as i32)),
            ..Default::default()
        });
        tbl.grid = Some(grid);

        for _ in 0..rows {
            let mut row = CT_Row::new();
            for _ in 0..cols {
                row.cells.push(CT_Tc::new());
            }
            tbl.rows.push(row);
        }
        tbl
    }

    /// Find the body content index of the first paragraph containing the given text.
    pub fn find_content_index(&self, text: &str) -> Option<usize> {
        self.find_content_indices(text).into_iter().next()
    }

    /// Find every direct body location whose paragraph content contains `text`.
    ///
    /// Direct paragraphs precede enclosing content controls so a heading wins
    /// over a table-of-contents control that happens to repeat its text.
    pub fn find_content_indices(&self, text: &str) -> Vec<usize> {
        self.document.body.find_paragraph_indices(text)
    }

    /// Return the paragraph index of the direct body child at `content_index`.
    ///
    /// The result addresses that paragraph through [`Self::paragraph`] and
    /// [`Self::paragraph_mut`], whose index also counts paragraphs inside block
    /// content controls. Returns `None` when the index is out of bounds or the
    /// direct child is not a paragraph.
    pub fn paragraph_index_of_content(&self, content_index: usize) -> Option<usize> {
        let BodyContent::Paragraph(target) = self.document.body.content.get(content_index)? else {
            return None;
        };
        self.document
            .body
            .paragraphs()
            .position(|paragraph| std::ptr::eq(paragraph, target.as_ref()))
    }

    /// Return the direct body index of the paragraph addressed by `paragraph_index`.
    ///
    /// A paragraph nested in a block content control is not a direct body child
    /// and therefore returns `None`.
    pub fn content_index_of_paragraph(&self, paragraph_index: usize) -> Option<usize> {
        let target = self.paragraph(paragraph_index)?.inner;
        self.document
            .body
            .content
            .iter()
            .position(|content| matches!(content, BodyContent::Paragraph(paragraph) if std::ptr::eq(paragraph.as_ref(), target)))
    }

    /// Resolve a modeled body cell paragraph to its checked physical story owner.
    /// `row` and `cell` count as [`crate::TableRef::cell`] does.
    #[doc(hidden)]
    pub fn cell_paragraph_story_location(
        &self,
        table: usize,
        row: usize,
        cell: usize,
        paragraph: usize,
    ) -> Result<ContentLocation> {
        let mut candidate = self.clone_for_staging();
        let selected = candidate
            .table(table)
            .and_then(|table| table.cell(row, cell).map(|cell| cell.inner as *const CT_Tc))
            .ok_or_else(|| Error::Other("cell index is out of range".to_owned()))?;
        let xml = candidate.document.to_xml()?;
        for owner in scan_story_owners(&xml, StoryKind::Body)?
            .into_iter()
            .filter(|owner| owner.kind == StoryKind::TableCell)
        {
            let (content, mut ordinal) = modeled_main_cell_route(&xml, &owner)?;
            let modeled = match candidate.document.body.content.get_mut(content) {
                Some(BodyContent::Table(table)) => nth_cell_in_table(table, &mut ordinal),
                Some(BodyContent::ContentControl(control)) => {
                    nth_cell_in_control(control, &mut ordinal)
                }
                _ => None,
            };
            if let Some(modeled) = modeled.filter(|cell| std::ptr::eq(selected, *cell)) {
                let count = modeled.paragraphs().len();
                let story = StoryId {
                    kind: owner.kind,
                    part_name: candidate.doc_part_name.clone(),
                    owner_index: owner.owner_index,
                    fingerprint: owner.fingerprint,
                };
                let mut paragraphs = story_range_paragraph_spans(&xml, &[(story, owner)])?
                    .into_iter()
                    .next()
                    .expect("one checked cell inventory");
                // Controls contribute their direct paragraphs separately. Restore
                // physical source order before applying the facade paragraph axis.
                paragraphs.sort_by_key(|(_, span)| span.start);
                if paragraphs.len() != count {
                    return Err(Error::Other(
                        "cell paragraph modeled/source correspondence is unsupported".into(),
                    ));
                }
                let (location, _) = paragraphs
                    .into_iter()
                    .nth(paragraph)
                    .ok_or_else(|| Error::Other("cell paragraph index is out of range".into()))?;
                candidate.story_range_paragraph_snapshot(&location)?;
                return Ok(location);
            }
        }
        Err(Error::Other(
            "cell has no checked physical story owner".to_owned(),
        ))
    }

    /// Resolve the paragraphs of every body table cell at once, keyed by
    /// (table, row, cell) as [`Self::cell_paragraph_story_location`] takes
    /// them.
    ///
    /// A cell is present when the single lookup resolves its paragraphs; a
    /// cell it refuses is absent, and callers that cache this inventory ask
    /// the single lookup for it. The single lookup also reads each returned
    /// paragraph back; this inventory leaves that check to the caller. One
    /// call costs one serialization and one owner scan of the body, where a
    /// single lookup costs them for each paragraph and rescans the body for
    /// each cell owner before its target.
    #[doc(hidden)]
    pub fn cell_paragraph_story_locations(
        &self,
    ) -> Result<HashMap<(usize, usize, usize), Vec<ContentLocation>>> {
        let mut candidate = self.clone_for_staging();
        let addresses =
            crate::table::cell_addresses(candidate.tables().into_iter().map(|table| table.inner));
        let xml = candidate.document.to_xml()?;
        let owners = scan_story_owners(&xml, StoryKind::Body)?;
        let body = owners
            .iter()
            .find(|owner| owner.kind == StoryKind::Body)
            .ok_or_else(|| Error::Other("main document body owner was not found".to_owned()))?;
        let direct_items = direct_story_content_items(&xml, body)?;
        let routes = modeled_main_cell_routes(&owners, &direct_items);
        let mut ordinals = HashMap::<usize, usize>::new();
        let mut selected = Vec::new();
        let mut seen = HashSet::new();
        for owner in owners
            .iter()
            .filter(|owner| owner.kind == StoryKind::TableCell)
        {
            // The single lookup fails on the first owner it cannot route,
            // whatever its target, so the inventory stops there too.
            let Some((content, direct_item)) = routes
                .as_ref()
                .and_then(|routes| routes.get(&owner.owner_index))
                .and_then(|(content, _)| direct_items.get(*content).map(|item| (*content, item)))
                .or_else(|| {
                    direct_items.iter().enumerate().find(|(_, item)| {
                        item.full.start <= owner.full.start && owner.full.end <= item.full.end
                    })
                })
            else {
                break;
            };
            if !matches!(
                direct_item.kind,
                StoryItemKind::Table | StoryItemKind::ContentControl
            ) {
                break;
            }
            let counter = ordinals.entry(content).or_insert(0);
            let mut ordinal = *counter;
            *counter += 1;
            let modeled = match candidate.document.body.content.get_mut(content) {
                Some(BodyContent::Table(table)) => nth_cell_in_table(table, &mut ordinal),
                Some(BodyContent::ContentControl(control)) => {
                    nth_cell_in_control(control, &mut ordinal)
                }
                _ => None,
            };
            let Some((pointer, count)) =
                modeled.map(|cell| (&*cell as *const CT_Tc, cell.paragraphs().len()))
            else {
                continue;
            };
            let Some(address) = addresses.get(&pointer) else {
                continue;
            };
            if !seen.insert(*address) {
                continue;
            }
            let story = StoryId {
                kind: owner.kind,
                part_name: candidate.doc_part_name.clone(),
                owner_index: owner.owner_index,
                fingerprint: owner.fingerprint,
            };
            selected.push((*address, count, (story, owner.clone())));
        }
        let pairs = selected
            .iter()
            .map(|(_, _, pair)| pair.clone())
            .collect::<Vec<_>>();
        let spans = story_range_paragraph_spans(&xml, &pairs)?;
        let mut result = HashMap::with_capacity(selected.len());
        for ((address, count, _), mut paragraphs) in selected.into_iter().zip(spans) {
            paragraphs.sort_by_key(|(_, span)| span.start);
            if paragraphs.len() == count {
                result.insert(
                    address,
                    paragraphs
                        .into_iter()
                        .map(|(location, _)| location)
                        .collect(),
                );
            }
        }
        Ok(result)
    }

    /// Return the direct body index of the table addressed by `table_index`.
    pub fn content_index_of_table(&self, table_index: usize) -> Option<usize> {
        let target = self.table(table_index)?.inner;
        self.document.body.content.iter().position(
            |content| matches!(content, BodyContent::Table(table) if std::ptr::eq(table.as_ref(), target)),
        )
    }

    /// Remove the content at the given body index.
    ///
    /// A direct body paragraph with no comments, drawings, fields, or other
    /// owned companions. Local removal and replacement read only that child.
    /// A package that retains a shadowed serializer prefix is not local.
    #[doc(hidden)]
    pub fn plain_direct_paragraph(&self, index: usize) -> bool {
        if self.plain_local_edit_blocked {
            return false;
        }
        self.comments.is_none()
            && self.comments_extended.is_none()
            && self
                .document
                .body
                .content
                .get(index)
                .is_some_and(|content| match content {
                    BodyContent::Paragraph(paragraph) => paragraph_is_plain(paragraph),
                    _ => false,
                })
    }

    /// Replace one plain direct body paragraph without scanning the other
    /// body children. `None` leaves the document unchanged so the caller can
    /// keep the package transaction. `Some(false)` is an equal paragraph and
    /// `Some(true)` means the stored paragraph was replaced.
    #[doc(hidden)]
    pub fn try_replace_plain_body_paragraph(
        &mut self,
        slot: usize,
        xml: &str,
    ) -> Result<Option<bool>> {
        if !self.plain_direct_paragraph(slot) {
            return Ok(None);
        }
        let stored = {
            let BodyContent::Paragraph(paragraph) = &self.document.body.content[slot] else {
                return Ok(None);
            };
            paragraph.clone()
        };
        let mut stored_xml = Vec::new();
        stored.to_xml(&mut Writer::new(&mut stored_xml))?;
        let incoming = CT_P::from_xml_fragment_checked(xml.as_bytes())?;
        if stored_xml == xml.as_bytes() || *stored == incoming {
            return Ok(Some(false));
        }
        if !paragraph_is_plain(&incoming)
            || (!stored.accepted_run_paths().is_empty()
                && stored.accepted_run_paths() != incoming.accepted_run_paths())
        {
            return Ok(None);
        }
        let stored_section = stored
            .properties
            .as_ref()
            .and_then(|properties| properties.sect_pr.clone());
        let incoming_section = incoming
            .properties
            .as_ref()
            .and_then(|properties| properties.sect_pr.clone());
        if stored_section != incoming_section {
            return Err(Error::Other(
                "element XML changed or introduced paragraph-owned section properties are unsupported, use the checked section owner".into(),
            ));
        }
        let context = direct_body_paragraph_context(self);
        admit_edited_paragraph(&stored_xml, xml.as_bytes(), &context)?;
        let story = StoryId::new(StoryKind::Body, self.doc_part_name.clone(), 0);
        validate_element_owner_relationships(self, &story, WordXmlOwner::Paragraph(&incoming))?;
        validate_element_xml_identity_change(self, &story, &stored_xml, xml.as_bytes())?;
        if let BodyContent::Paragraph(paragraph) = &mut self.document.body.content[slot] {
            **paragraph = incoming;
        }
        self.invalidate_layout();
        Ok(Some(true))
    }

    /// Returns false when the index is absent or safe removal is refused.
    /// Use [`Self::try_remove_content`] to distinguish a refusal from an absent index.
    pub fn remove_content(&mut self, index: usize) -> bool {
        self.try_remove_content(index).unwrap_or(false)
    }

    /// Remove a direct body child and its wholly owned comment threads, and
    /// the footnotes and endnotes nothing else references after it goes.
    /// Partial comment cuts and unprovable companion ownership fail atomically.
    ///
    /// The bookmark markers the child owns go too: the bookmark starts
    /// written just before it whose bookmark ends inside it or in the
    /// bookmark ends written just after it, those ends, and the ends written
    /// just after it that close a bookmark started inside it; a start whose
    /// bookmark ends further on stays. Each start that goes moves the later
    /// body items back by one more index.
    pub fn try_remove_content(&mut self, index: usize) -> Result<bool> {
        if index >= self.document.body.content.len() {
            return Ok(false);
        }
        // A plain paragraph holds no bookmark end, so only a bookmark around
        // it, its start written just before it, could go with it.
        if (self.plain_direct_paragraph(index) || self.plain_removable_paragraph(index))
            && self.block_insertion_index(index) == index
        {
            self.document.body.remove(index);
            self.invalidate_layout();
            return Ok(true);
        }
        let owned = self.owned_bookmark_markers(index, index..index + 1)?;
        let mut candidate = self.clone_for_staging();
        let before = candidate.document.to_xml()?;
        let referenced_before = xml_attribute_values(&before)?;
        let mut removed = owned.before;
        removed.push(index);
        removed.extend(owned.after);
        for index in removed.into_iter().rev() {
            candidate.document.body.remove(index);
        }
        let after = candidate.document.to_xml()?;
        let referenced_after = xml_attribute_values(&after)?;
        let owner = candidate.doc_part_name.clone();
        candidate.release_unreferenced_relationships(&owner, &referenced_before, &referenced_after);
        candidate.remove_unreferenced_notes(&before, &after)?;
        candidate.reconcile_comment_removal(self)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(true)
    }

    /// The main part as a save would write it now: the model flushed first.
    pub(crate) fn main_part_snapshot(&mut self) -> Result<Vec<u8>> {
        self.flush_to_package()?;
        Ok(self
            .package
            .get_part(&self.doc_part_name)
            .map(<[u8]>::to_vec)
            .unwrap_or_default())
    }

    /// Remove the footnotes and endnotes the main part referenced `before`
    /// a cut and nothing references `after` it, neither the main part nor a
    /// header or footer, as Word does when it deletes the text holding their
    /// marks. No reference to them is left, so the main part keeps its
    /// cut: a notes part that loses notes is rewritten once, and the
    /// pictures only those notes showed go with them. A note whose id its
    /// part holds twice stays, a reference to an id the part does not hold
    /// is passed over, and a part whose notes cannot be matched one to one
    /// with its owners keeps all of them.
    pub(crate) fn remove_unreferenced_notes(&mut self, before: &[u8], after: &[u8]) -> Result<()> {
        self.remove_unreferenced_notes_or_copies(before, &[], after)
    }

    /// [`Self::remove_unreferenced_notes`], where the notes made during the
    /// edit (`copies`, by kind and id, such as the notes a template loop
    /// copied) are candidates too: a copy nothing references `after` goes.
    pub(crate) fn remove_unreferenced_notes_or_copies(
        &mut self,
        before: &[u8],
        copies: &[(StoryKind, i32)],
        after: &[u8],
    ) -> Result<()> {
        let mut referenced = note_reference_ids_in(before)?;
        for (kind, id) in copies {
            match kind {
                StoryKind::Footnote => referenced.footnotes.insert(*id),
                StoryKind::Endnote => referenced.endnotes.insert(*id),
                _ => false,
            };
        }
        if referenced.is_empty() {
            return Ok(());
        }
        let mut kept = note_reference_ids_in(after)?;
        for (rel_id, is_header) in self.header_footer_rel_ids() {
            if let Some(part) = self.header_footer_part_name(&rel_id, is_header)
                && let Some(xml) = self.package.get_part(&part)
            {
                let story = note_reference_ids_in(xml)?;
                kept.footnotes.extend(story.footnotes);
                kept.endnotes.extend(story.endnotes);
            }
        }
        let mut gone = NoteReferenceIds {
            footnotes: &referenced.footnotes - &kept.footnotes,
            endnotes: &referenced.endnotes - &kept.endnotes,
        };
        if gone.is_empty() {
            return Ok(());
        }
        // A note a note or a comment references stays too: a template loop
        // copies a note together with the notes it references.
        self.flush_to_package()?;
        let mut scanned = BTreeSet::new();
        for story in self.related_story_sources()? {
            if matches!(
                story.root_kind,
                StoryKind::Footnote | StoryKind::Endnote | StoryKind::Comment
            ) && scanned.insert(story.part_name.clone())
                && let Some(xml) = self.package.get_part(&story.part_name)
            {
                let story = note_reference_ids_in(xml)?;
                gone.footnotes.retain(|id| !story.footnotes.contains(id));
                gone.endnotes.retain(|id| !story.endnotes.contains(id));
            }
        }
        if gone.is_empty() {
            return Ok(());
        }
        for (kind, ids) in [
            (StoryKind::Footnote, &gone.footnotes),
            (StoryKind::Endnote, &gone.endnotes),
        ] {
            if ids.is_empty() {
                continue;
            }
            let Some(part_name) = self.note_part_name(kind)? else {
                continue;
            };
            let Some(xml) = self.package.get_part(&part_name) else {
                continue;
            };
            // Normal notes in part order, matched to their owners in the
            // same order, as `note_story` matches them.
            let normal = oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(xml)?
                .footnotes
                .iter()
                .filter(|note| note.note_type == oxoffice_docx_oxml::footnotes::NoteType::Normal)
                .map(|note| note.id)
                .collect::<Vec<_>>();
            let owners = scan_story_owners(xml, kind)?
                .into_iter()
                .filter(|owner| owner.kind == kind)
                .collect::<Vec<_>>();
            if owners.len() != normal.len() {
                continue;
            }
            let mut ranges = Vec::new();
            for &id in ids {
                let mut matching = normal.iter().enumerate().filter(|(_, note)| **note == id);
                if let (Some((index, _)), None) = (matching.next(), matching.next()) {
                    ranges.push(owners[index].full.clone());
                }
            }
            if ranges.is_empty() {
                continue;
            }
            ranges.sort_by_key(|range| range.start);
            let mut notes_xml = xml.to_vec();
            for range in ranges.into_iter().rev() {
                notes_xml.drain(range);
            }
            // Pictures only the removed notes showed go with them.
            let referenced_before = xml_attribute_values(xml)?;
            let referenced_after = xml_attribute_values(&notes_xml)?;
            self.release_unreferenced_relationships(
                &part_name,
                &referenced_before,
                &referenced_after,
            );
            set_story_source_xml(self, &part_name, notes_xml)?;
        }
        Ok(())
    }

    /// Remove the half-open direct body range `[start, end)`.
    ///
    /// The footnotes and endnotes nothing else references afterwards go too,
    /// and so do the bookmark markers the range owns: the bookmark starts
    /// written just before `start` whose bookmark ends inside the range or
    /// in the bookmark ends written just after it, those ends, and the ends
    /// written just after it that close a bookmark started inside it. Returns
    /// the number of body items removed, those markers included. An empty range removes nothing and
    /// returns 0. A reversed or out-of-bounds range returns an error and
    /// leaves the document unchanged.
    /// The call does not walk into tables.
    pub fn remove_content_range(&mut self, start: usize, end: usize) -> Result<usize> {
        let len = self.document.body.content.len();
        if start > end || end > len {
            return Err(Error::Other(
                "content range is reversed or out of range".to_owned(),
            ));
        }
        if start == end {
            return Ok(0);
        }
        // Plain paragraphs own no companion, as in `try_remove_content`.
        if (start..end).all(|index| self.plain_removable_paragraph(index)) {
            self.document.body.content.drain(start..end);
            self.invalidate_layout();
            return Ok(end - start);
        }
        let owned = self.owned_bookmark_markers(start, start..end)?;
        let markers = owned.before.len() + owned.after.len();
        let mut candidate = self.clone_for_staging();
        let before = candidate.document.to_xml()?;
        let referenced_before = xml_attribute_values(&before)?;
        let mut removed = owned.before;
        removed.extend(start..end);
        removed.extend(owned.after);
        for index in removed.into_iter().rev() {
            candidate.document.body.remove(index);
        }
        let after = candidate.document.to_xml()?;
        let referenced_after = xml_attribute_values(&after)?;
        let owner = candidate.doc_part_name.clone();
        candidate.release_unreferenced_relationships(&owner, &referenced_before, &referenced_after);
        candidate.remove_unreferenced_notes(&before, &after)?;
        candidate.reconcile_comment_removal(self)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(end - start + markers)
    }
}

/// The footnote and endnote ids some references name.
#[derive(Default)]
pub(super) struct NoteReferenceIds {
    pub(super) footnotes: BTreeSet<i32>,
    pub(super) endnotes: BTreeSet<i32>,
}

impl NoteReferenceIds {
    fn is_empty(&self) -> bool {
        self.footnotes.is_empty() && self.endnotes.is_empty()
    }
}

/// The ids every `w:footnoteReference` and `w:endnoteReference` of a part
/// names, however deep it sits: in a tracked change, a content control, a
/// text box or unmodelled XML.
pub(super) fn note_reference_ids_in(xml: &[u8]) -> Result<NoteReferenceIds> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut ids = NoteReferenceIds::default();
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("note reference scan failed: {error}")))?;
        let (start, footnote) = match &event {
            Event::Start(start) | Event::Empty(start) if word_element(&namespace) => {
                match start.local_name().as_ref() {
                    b"footnoteReference" => (start, true),
                    b"endnoteReference" => (start, false),
                    _ => {
                        buffer.clear();
                        continue;
                    }
                }
            }
            Event::Eof => break,
            _ => {
                buffer.clear();
                continue;
            }
        };
        for attribute in start.attributes() {
            let attribute = attribute.map_err(|error| {
                Error::Other(format!("note reference attribute scan failed: {error}"))
            })?;
            let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
            if word_element(&namespace) && local.as_ref() == b"id" {
                let value = attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                    .map_err(|error| {
                        Error::Other(format!("note reference ID is invalid: {error}"))
                    })?;
                if let Ok(id) = value.parse::<i32>() {
                    if footnote {
                        ids.footnotes.insert(id);
                    } else {
                        ids.endnotes.insert(id);
                    }
                }
            }
        }
        buffer.clear();
    }
    Ok(ids)
}

/// One direct body block written alone between the head and the tail of the
/// main part, and the span of one story item inside it.
pub(crate) struct BodyBlockItem {
    /// The bytes before the block, through the body start tag.
    pub(crate) head: Vec<u8>,
    /// The head, the block and the tail.
    pub(crate) xml: Vec<u8>,
    /// The length of the bytes after the block, from the final section on.
    pub(crate) tail: usize,
    /// The item, in `xml`.
    pub(crate) item: Range<usize>,
}

impl BodyBlockItem {
    /// The block of `xml`, an edit of [`Self::xml`] that kept its head and
    /// tail.
    pub(crate) fn block_of<'a>(&self, xml: &'a [u8]) -> &'a [u8] {
        &xml[self.head.len()..xml.len() - self.tail]
    }

    /// The tail of [`Self::xml`].
    pub(crate) fn tail_bytes(&self) -> &[u8] {
        &self.xml[self.xml.len() - self.tail..]
    }
}

impl Document {
    /// The direct body block at `slot` written alone under the root and the
    /// final section of the main part, at the cost of that block, and the
    /// item `index` of the story owner `(kind, owner)` inside it: owners and
    /// items are counted from the start of the block, as
    /// [`Self::block_element_extent`] counts them. `None` when the block has
    /// no such item, the item is not of `item_kind`, or a save of the body is
    /// known to fail, which the staged route reports.
    pub(crate) fn body_block_item(
        &self,
        slot: usize,
        (kind, owner): (StoryKind, usize),
        index: usize,
        item_kind: StoryItemKind,
    ) -> Result<Option<BodyBlockItem>> {
        // Computed once after an invalidation: an empty cache is no answer,
        // and the swap keeps a savable answer
        // (`keep_body_savable_after_typed_insert`).
        if self.unsavable_body_edit().is_some() {
            return Ok(None);
        }
        let Some(block) = self.document.body.content.get(slot) else {
            return Ok(None);
        };
        let child = self.document.body_item_xml(block)?;
        // The frame of the part with no body child, which costs the root and
        // the final section, not the body.
        let frame = CT_Document {
            body: CT_Body {
                content: Vec::new(),
                sect_pr: self.document.body.sect_pr.clone(),
            },
            extra_namespaces: self.document.extra_namespaces.clone(),
            background_xml: self.document.background_xml.clone(),
            background_extra_xml: self.document.background_extra_xml.clone(),
            root_attributes: self.document.root_attributes.clone(),
        };
        let (head, tail) = frame.body_frame_xml()?;
        let mut xml = Vec::with_capacity(head.len() + child.len() + tail.len());
        xml.extend_from_slice(&head);
        xml.extend_from_slice(&child);
        xml.extend_from_slice(&tail);
        let written = xml.len();
        // The declaration a save adds to the root when the block uses w14.
        declare_w14_on_part_root(&mut xml)?;
        let head = xml[..head.len() + xml.len() - written].to_vec();
        let block_span = head.len()..xml.len() - tail.len();
        let owners = scan_story_owners(&xml, StoryKind::Body)?;
        let Some(owner) = owners
            .iter()
            .find(|candidate| candidate.kind == kind && candidate.owner_index == owner)
        else {
            return Ok(None);
        };
        let items = scan_story_items(&xml, owner, None)?;
        let Some(item) = items.get(index) else {
            return Ok(None);
        };
        if item.kind != item_kind
            || item.full.start < block_span.start
            || item.full.end > block_span.end
        {
            return Ok(None);
        }
        let item = item.full.clone();
        Ok(Some(BodyBlockItem {
            head,
            tail: tail.len(),
            xml,
            item,
        }))
    }

    /// The block `child` reads as between `head` and `tail`, when it writes
    /// back to exactly `child`.
    pub(crate) fn reparse_body_block(
        head: &[u8],
        tail: &[u8],
        child: &[u8],
    ) -> Option<BodyContent> {
        super::main_part::parse_one_child(head, tail, child)
    }

    /// Put `block` in place of the direct body block at `slot`, in place
    /// (`EditUnit::BodyBlocks`), and answer whether the body changed. The
    /// caller made `block` from the block at `slot` and checked that it
    /// reads back as it writes.
    pub(crate) fn replace_body_block_in_place(
        &mut self,
        slot: usize,
        block: BodyContent,
    ) -> Result<bool> {
        if self.document.body.content.get(slot) == Some(&block) {
            return Ok(false);
        }
        if slot >= self.document.body.content.len() {
            return Err(Error::Other(format!("body has no block {slot}")));
        }
        let savable_before = self.unsavable_body_edit_cache.get() == Some(&None);
        self.edit_in_place(&[EditUnit::BodyBlocks { slot, count: 1 }], |document| {
            document.document.body.content[slot] = block;
            document.invalidate_layout();
            Ok(())
        })?;
        let changed = self.take_last_edit_changed();
        if changed {
            self.keep_body_savable_after_typed_insert(slot, savable_before);
        }
        Ok(changed)
    }
}
