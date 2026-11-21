//! Tables of the body and of stories: rows, columns, cells and their text.

use super::*;

impl Document {
    // ---- Table access ----

    /// Get immutable references to all tables.
    pub fn tables(&self) -> Vec<TableRef<'_>> {
        self.document
            .body
            .tables()
            .map(|t| TableRef { inner: t })
            .collect()
    }

    /// Get an immutable table by index among tables only.
    pub fn table(&self, index: usize) -> Option<TableRef<'_>> {
        self.document
            .body
            .tables()
            .nth(index)
            .map(|inner| TableRef { inner })
    }

    /// Get a mutable table by index among tables only.
    pub fn table_mut(&mut self, index: usize) -> Option<Table<'_>> {
        self.invalidate_layout();
        let mut remaining = index;
        nth_table_in_body(&mut self.document.body.content, &mut remaining)
            .map(|inner| Table { inner })
    }

    /// Apply `edit`, an edit of the properties of the body table at `index`
    /// (among tables only), of its rows or of its cells, without a copy of
    /// the document: `edit` runs on a copy of that table, which replaces it
    /// when the edit succeeds and changed properties only. An edit that fails
    /// or panics leaves the document untouched. So does one that changed
    /// anything besides properties (rows, cells, their content, unmodelled
    /// children) or gave a cell its first property or took its last, which
    /// returns `Ok(None)`: such an edit moves content and belongs to a route
    /// that publishes content changes.
    #[doc(hidden)]
    pub fn edit_table_properties_in_place<T>(
        &mut self,
        index: usize,
        edit: impl FnOnce(&mut Table<'_>) -> Result<T>,
    ) -> Result<Option<T>> {
        let current = nth_table_in_body_ref(&self.document.body.content, &mut index.clone())
            .ok_or_else(|| Error::Other("table index out of range".to_owned()))?;
        let mut edited = current.clone();
        let value = edit(&mut Table { inner: &mut edited })?;
        if !same_table_content(current, &edited) {
            return Ok(None);
        }
        *self.table_mut(index).expect("the table read above").inner = edited;
        Ok(Some(value))
    }

    /// Append a formatted empty row through one checked package transaction.
    pub fn add_empty_table_row(&mut self, table_index: usize) -> Result<usize> {
        let mut candidate = self.clone_for_staging();
        let index = {
            let mut table = candidate.table_mut(table_index).ok_or_else(|| {
                Error::Other(format!("table index {table_index} is out of range"))
            })?;
            let index = table.row_count();
            table.add_row()?;
            index
        };
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(index)
    }

    /// Insert a logical grid column before or after an existing grid column.
    pub fn insert_table_column(
        &mut self,
        table_index: usize,
        index: usize,
        before: bool,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        let mut table = candidate
            .table_mut(table_index)
            .ok_or_else(|| Error::Other(format!("table index {table_index} is out of range")))?;
        let columns = table
            .inner
            .grid
            .as_ref()
            .map_or(0, |grid| grid.columns.len());
        if index >= columns {
            return Err(Error::Other(
                "column insertion index is out of range".into(),
            ));
        }
        table.insert_column(index, before, None)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    fn reconcile_cut_table_bookmarks(&mut self, original: &Self, part_name: &str) -> Result<()> {
        let source_xml = |document: &Self| -> Result<Vec<u8>> {
            document
                .story_sources()?
                .into_iter()
                .find(|source| source.part_name == part_name)
                .map(|source| source.xml.into_owned())
                .ok_or_else(|| Error::Other("table bookmark source part disappeared".to_owned()))
        };
        let paired = |markers: &[BookmarkMarkerSpan]| {
            let starts = markers
                .iter()
                .filter(|marker| marker.start)
                .map(|marker| marker.id.clone())
                .collect::<HashSet<_>>();
            markers
                .iter()
                .filter(|marker| !marker.start && starts.contains(&marker.id))
                .map(|marker| marker.id.clone())
                .collect::<HashSet<_>>()
        };
        let paired_before = paired(&bookmark_marker_spans(&source_xml(original)?)?);
        let mut xml = source_xml(self)?;
        let markers = bookmark_marker_spans(&xml)?;
        let paired_after = paired(&markers);
        let mut orphans = markers
            .into_iter()
            .filter(|marker| {
                paired_before.contains(&marker.id) && !paired_after.contains(&marker.id)
            })
            .collect::<Vec<_>>();
        orphans.sort_by_key(|marker| marker.span.start);
        if !orphans.is_empty() {
            for marker in orphans.into_iter().rev() {
                xml.splice(marker.span, std::iter::empty());
            }
            set_story_source_xml(self, part_name, xml)?;
        }
        Ok(())
    }

    /// Delete one logical grid column without publishing partial topology.
    /// The footnotes and endnotes only the removed cells referenced go too.
    pub fn delete_table_column(&mut self, table_index: usize, index: usize) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        let before = candidate.document.to_xml()?;
        let table = candidate
            .table_mut(table_index)
            .ok_or_else(|| Error::Other(format!("table index {table_index} is out of range")))?
            .inner;
        crate::table::remove_grid_column(table, index)?;
        let after = candidate.document.to_xml()?;
        candidate.remove_unreferenced_notes(&before, &after)?;
        let part_name = candidate.doc_part_name.clone();
        candidate.reconcile_cut_table_bookmarks(self, &part_name)?;
        candidate.reconcile_comment_removal(self)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Split the complete merge containing a logical column, or preserve an unmerged cell.
    pub fn split_table_cell(
        &mut self,
        table_index: usize,
        row: usize,
        logical_column: usize,
    ) -> Result<bool> {
        let mut candidate = self.clone_for_staging();
        let mut table = candidate
            .table_mut(table_index)
            .ok_or_else(|| Error::Other(format!("table index {table_index} is out of range")))?;
        let columns = table
            .inner
            .grid
            .as_ref()
            .map_or(0, |grid| grid.columns.len());
        if crate::table::table_holds_controls(table.inner) {
            return Err(Error::Other(
                "a table cell cannot be split while rows or cells sit in content controls"
                    .to_owned(),
            ));
        }
        let source_row = table
            .inner
            .rows
            .get(row)
            .ok_or_else(|| Error::Other("table row index is out of range".to_owned()))?;
        let cell = crate::table::row_cell_ranges(source_row, columns)?
            .iter()
            .position(|(start, end)| (*start..*end).contains(&logical_column))
            .ok_or_else(|| Error::Other("logical column has no physical cell".to_owned()))?;
        let before = table.inner.clone();
        table.split_cell(row, cell)?;
        if table.inner == &before {
            return Ok(false);
        }
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(true)
    }

    /// Insert a copy of one table row before row `insert_at` in the same table.
    ///
    /// Rows count as [`TableRef::row`] counts them, the rows of row content
    /// controls included; a row of a row control cannot be the copied one,
    /// and the copy cannot go inside a control.
    ///
    /// `table_index` counts the body tables in document order, as
    /// [`Self::table`] does; nested tables are not counted. `insert_at` may
    /// equal the row count to append. The copy keeps
    /// row properties, cell formatting, nested content, relationships, and
    /// preserved producer XML. Document-wide identities are made unique, the
    /// row and its paragraphs drop `w14:paraId` and `w14:textId`, and comment
    /// anchors are omitted. The document is unchanged on error.
    pub fn clone_table_row(
        &mut self,
        table_index: usize,
        source: usize,
        insert_at: usize,
    ) -> Result<usize> {
        let mut candidate = self.clone_for_staging();
        let source_row = {
            let table = candidate
                .table_mut(table_index)
                .ok_or_else(|| Error::Other(format!("table index {table_index} is out of range")))?
                .inner;
            let row_count = TableRef { inner: table }.row_count();
            if source >= row_count {
                return Err(Error::Other(format!(
                    "row index {source} is out of range for a table with {row_count} rows"
                )));
            }
            if insert_at > row_count {
                return Err(Error::Other(format!(
                    "row insertion index {insert_at} is out of range for a table with {row_count} rows"
                )));
            }
            table.rows[own_copied_row(table, source)?].clone()
        };
        let copied_row = candidate.fresh_table_row_copy(source_row)?;
        let table = candidate
            .table_mut(table_index)
            .expect("the checked table remains available")
            .inner;
        insert_copied_row(table, insert_at, copied_row)?;
        validate_table_topology(table)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(insert_at)
    }

    /// Remove one row from a table and report whether it was removed.
    ///
    /// `row_index` counts rows as [`TableRef::row`] counts them; a row of a
    /// row content control is refused, as removing its control removes it.
    ///
    /// A table retains at least one direct row. If the removed row starts a
    /// vertical merge that continues below, the next row becomes its start.
    /// Preserved table children stay at their original logical boundaries.
    /// The document is unchanged on error.
    /// The footnotes and endnotes only the removed cells referenced go too.
    pub fn remove_table_row(&mut self, table_index: usize, row_index: usize) -> Result<bool> {
        let mut candidate = self.clone_for_staging();
        let before = candidate.document.to_xml()?;
        let table = candidate
            .table_mut(table_index)
            .ok_or_else(|| Error::Other(format!("table index {table_index} is out of range")))?
            .inner;
        let row_count = TableRef { inner: table }.row_count();
        if row_index >= row_count {
            return Err(Error::Other(format!(
                "row index {row_index} is out of range for a table with {row_count} rows"
            )));
        }
        let row_index = own_removed_row(table, row_index)?;
        if table.rows.len() == 1 {
            return Err(Error::Other(
                "a table keeps at least one row, so its only row cannot be removed".to_owned(),
            ));
        }
        crate::table::hand_on_vertical_restarts(table, row_index)?;
        let raw_before_removed = table
            .extra_xml
            .iter()
            .filter(|(position, _)| *position == row_index)
            .count();
        for (position, _) in &mut table.extra_xml {
            if *position > row_index {
                *position -= 1;
            }
        }
        for (position, raw_before, _) in &mut table.content_controls {
            if *position > row_index {
                if *position == row_index + 1 {
                    *raw_before += raw_before_removed;
                }
                *position -= 1;
            }
        }
        table.rows.remove(row_index);
        validate_table_topology(table)?;
        let after = candidate.document.to_xml()?;
        candidate.remove_unreferenced_notes(&before, &after)?;
        candidate.reconcile_comment_removal(self)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(true)
    }

    /// Delete a logical column in one checked related table and publish once.
    /// The footnotes and endnotes only the removed cells referenced go too.
    #[doc(hidden)]
    pub fn delete_story_table_column(
        &mut self,
        location: &ContentLocation,
        column: usize,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        let mut table = candidate.story_table_model(location)?;
        crate::table::remove_grid_column(&mut table, column)?;
        // The replacement takes the notes only the removed cells referenced.
        candidate.replace_story_content(location, ContentFragment::table(table)?)?;
        candidate.reconcile_cut_table_bookmarks(self, location.story().part_name())?;
        candidate.reconcile_comment_removal(self)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Copy one row within a checked related table, freshening its identities once.
    #[doc(hidden)]
    pub fn clone_story_table_row(
        &mut self,
        location: &ContentLocation,
        source: usize,
        insert_at: usize,
    ) -> Result<usize> {
        let mut candidate = self.clone_for_staging();
        let mut table = candidate.story_table_model(location)?;
        let row_count = TableRef { inner: &table }.row_count();
        if source >= row_count || insert_at > row_count {
            return Err(Error::Other(
                "rich table row copy index is out of range".to_owned(),
            ));
        }
        let copied =
            candidate.fresh_table_row_copy(table.rows[own_copied_row(&table, source)?].clone())?;
        insert_copied_row(&mut table, insert_at, copied)?;
        validate_table_topology(&table)?;
        candidate.replace_story_content(location, ContentFragment::table(table)?)?;
        self.commit_staged_mutation(candidate);
        Ok(insert_at)
    }

    /// Remove one row from a checked related table while preserving owned boundaries.
    /// The footnotes and endnotes only the removed cells referenced go too.
    #[doc(hidden)]
    pub fn remove_story_table_row(
        &mut self,
        location: &ContentLocation,
        row_index: usize,
    ) -> Result<bool> {
        let mut candidate = self.clone_for_staging();
        let mut table = candidate.story_table_model(location)?;
        if row_index >= (TableRef { inner: &table }).row_count() {
            return Err(Error::Other(
                "rich table row index is out of range".to_owned(),
            ));
        }
        let row_index = own_removed_row(&table, row_index)?;
        if table.rows.len() == 1 {
            return Err(Error::Other(
                "a table keeps at least one row, so its only row cannot be removed".to_owned(),
            ));
        }
        crate::table::hand_on_vertical_restarts(&mut table, row_index)?;
        let raw_before = table
            .extra_xml
            .iter()
            .filter(|(position, _)| *position == row_index)
            .count();
        for (position, _) in &mut table.extra_xml {
            if *position > row_index {
                *position -= 1;
            }
        }
        for (position, raw, _) in &mut table.content_controls {
            if *position > row_index {
                if *position == row_index + 1 {
                    *raw += raw_before;
                }
                *position -= 1;
            }
        }
        table.rows.remove(row_index);
        validate_table_topology(&table)?;
        // The replacement takes the notes only the removed cells referenced.
        candidate.replace_story_content(location, ContentFragment::table(table)?)?;
        candidate.reconcile_comment_removal(self)?;
        self.commit_staged_mutation(candidate);
        Ok(true)
    }

    /// Set one body's table cell text through a checked package transaction.
    #[doc(hidden)]
    pub fn try_set_cell_text(
        &mut self,
        table: usize,
        row: usize,
        cell: usize,
        text: &str,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate
            .table_mut(table)
            .ok_or_else(|| Error::Other(format!("table index {table} is out of range")))?
            .cell(row, cell)
            .ok_or_else(|| Error::Other(format!("cell {row}:{cell} is out of range")))?
            .try_set_text(text)?;
        candidate.reconcile_comment_removal(self)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Freshen the document-wide identities carried by a copied table row.
    fn fresh_table_row_copy(&mut self, row: CT_Row) -> Result<CT_Row> {
        let mut xml = Vec::new();
        CT_Tbl {
            properties: None,
            grid: None,
            rows: vec![row],
            extra_xml: Vec::new(),
            content_controls: Vec::new(),
        }
        .to_xml(&mut Writer::new(&mut xml))?;
        let mut namespace_scope = self
            .body_namespace_bindings
            .iter()
            .map(|(name, value)| (namespace_prefix(name), value.clone()))
            .collect::<BTreeMap<_, _>>();
        namespace_scope.insert("w".to_owned(), WORD_NAMESPACE.to_owned());
        let fragment = ContentFragment {
            kind: StoryItemKind::Table,
            xml,
            source_part_name: None,
            source_story_kind: None,
            source_owner_index: None,
            namespace_scope,
            relationships: Vec::new(),
        };
        let freshened = freshen_content_fragment_identities(self, &fragment)?;
        let closed = close_content_fragment_namespaces(&freshened, &fragment.namespace_scope)?;
        let mut wrapped =
            format!(r#"<w:document xmlns:w="{WORD_NAMESPACE}"><w:body>"#).into_bytes();
        wrapped.extend(closed);
        wrapped.extend_from_slice(b"</w:body></w:document>");
        let mut content = CT_Document::from_xml(&wrapped)?.body.content.into_iter();
        match (content.next(), content.next()) {
            (Some(BodyContent::Table(mut table)), None) if table.rows.len() == 1 => {
                Ok(table.rows.remove(0))
            }
            _ => Err(Error::Other(
                "the copied table row did not parse back as one row".to_owned(),
            )),
        }
    }

    /// Add a table with the specified number of rows and columns.
    /// Returns a mutable reference for further configuration.
    pub fn add_table(&mut self, rows: usize, cols: usize) -> Table<'_> {
        self.invalidate_layout();
        self.document
            .body
            .content
            .push(BodyContent::Table(Box::new(Self::detached_table(
                rows, cols,
            ))));
        match self.document.body.content.last_mut().unwrap() {
            BodyContent::Table(t) => Table { inner: t },
            _ => unreachable!(),
        }
    }

    /// Get the number of tables.
    pub fn table_count(&self) -> usize {
        self.document.body.tables().count()
    }
}

/// The table's own row that row `index` names, for a copy: a row of a row
/// content control is refused, as repeating the control copies it.
fn own_copied_row(table: &CT_Tbl, index: usize) -> Result<usize> {
    crate::table::own_row_index(table, index).ok_or_else(|| {
        Error::Other(
            "a row inside a row content control cannot be copied; repeat its control".to_owned(),
        )
    })
}

/// The table's own row that row `index` names, for a removal: a row of a
/// row content control is refused, as removing the control removes it.
fn own_removed_row(table: &CT_Tbl, index: usize) -> Result<usize> {
    crate::table::own_row_index(table, index).ok_or_else(|| {
        Error::Other(
            "a row inside a row content control cannot be removed; remove its control".to_owned(),
        )
    })
}

/// Insert `row` among the table's own rows before row `insert_at`, rows
/// counted as [`TableRef::row`] counts them, or after the last one. The
/// preserved children and row controls of the boundary written before the
/// new row stay at it, and those written after move with the rows after it,
/// so that each control keeps the children written before it.
fn insert_copied_row(table: &mut CT_Tbl, insert_at: usize, row: CT_Row) -> Result<()> {
    let (position, follows) =
        crate::table::own_row_insertion(table, insert_at).ok_or_else(|| {
            Error::Other("a row cannot be inserted inside a row content control".to_owned())
        })?;
    let mut raw = 0;
    for (at, _) in &mut table.extra_xml {
        if *at == position {
            if follows.is_some_and(|(first, _)| raw >= first) {
                *at += 1;
            }
            raw += 1;
        } else if *at > position {
            *at += 1;
        }
    }
    for (index, (at, raw_before, _)) in table.content_controls.iter_mut().enumerate() {
        if *at > position {
            *at += 1;
        } else if *at == position
            && let Some((first, control)) = follows
            && (*raw_before > first || *raw_before == first && index >= control)
        {
            *at += 1;
            *raw_before -= first;
        }
    }
    table.rows.insert(position, row);
    Ok(())
}
