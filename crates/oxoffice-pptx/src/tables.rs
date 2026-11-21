//! Tables: borrowed and mutable tables and cells, table styles and banding, rows and columns, borders, and merging and splitting cells.

use super::*;

pub(super) fn invalid_table_mutation(operation: &'static str, message: String) -> Error {
    Error::InvalidTableMutation { operation, message }
}

impl<'a> TableRef<'a> {
    /// Returns the number of explicit table rows.
    pub fn row_count(&self) -> usize {
        self.table.rows.len()
    }

    /// Returns the number of grid columns.
    pub fn column_count(&self) -> usize {
        self.table.grid.columns.len()
    }

    /// Returns one explicit grid cell by zero-based row and column.
    pub fn cell(&self, row: usize, column: usize) -> Option<TableCellRef<'a>> {
        table_cell(self.table, row, column).map(|cell| TableCellRef { cell })
    }

    /// Returns one grid-column width in EMU.
    pub fn column_width(&self, column: usize) -> Option<Emu> {
        self.table.grid.columns.get(column).copied()
    }

    /// Returns one stored row height in EMU.
    pub fn row_height(&self, row: usize) -> Option<Emu> {
        self.table.rows.get(row).map(|row| row.height)
    }

    /// Returns whether first-row table styling is enabled.
    pub fn first_row(&self) -> bool {
        self.table
            .properties
            .as_ref()
            .is_some_and(|properties| properties.first_row)
    }

    /// Returns whether last-row table styling is enabled.
    pub fn last_row(&self) -> bool {
        self.table
            .properties
            .as_ref()
            .is_some_and(|properties| properties.last_row)
    }

    /// Returns whether first-column table styling is enabled.
    pub fn first_column(&self) -> bool {
        self.table
            .properties
            .as_ref()
            .is_some_and(|properties| properties.first_column)
    }

    /// Returns whether last-column table styling is enabled.
    pub fn last_column(&self) -> bool {
        self.table
            .properties
            .as_ref()
            .is_some_and(|properties| properties.last_column)
    }

    /// Returns whether horizontal row banding is enabled.
    pub fn horizontal_banding(&self) -> bool {
        self.table
            .properties
            .as_ref()
            .is_some_and(|properties| properties.band_rows)
    }

    /// Returns whether vertical column banding is enabled.
    pub fn vertical_banding(&self) -> bool {
        self.table
            .properties
            .as_ref()
            .is_some_and(|properties| properties.band_columns)
    }
    /// Returns the retained table style identity without creating properties.
    pub fn style_id(&self) -> Option<&str> {
        self.table.properties.as_ref()?.style_id.as_deref()
    }
}

impl<'a> TableMut<'a> {
    /// Returns the number of explicit table rows.
    pub fn row_count(&self) -> usize {
        self.table().rows.len()
    }

    /// Returns the number of grid columns.
    pub fn column_count(&self) -> usize {
        self.table().grid.columns.len()
    }

    /// Returns one explicit grid cell by zero-based row and column.
    pub fn cell(&self, row: usize, column: usize) -> Option<TableCellRef<'_>> {
        table_cell(self.table(), row, column).map(|cell| TableCellRef { cell })
    }

    /// Returns one explicit grid cell for in-place mutation.
    pub fn cell_mut(&mut self, row: usize, column: usize) -> Option<TableCellMut<'_>> {
        table_cell(self.table_mut_raw(), row, column)?;
        Some(TableCellMut {
            table: self.table_mut_raw(),
            row,
            column,
        })
    }

    /// Consumes this handle and returns one explicit cell for mutation.
    pub fn into_cell_mut(self, row: usize, column: usize) -> Option<TableCellMut<'a>> {
        let ShapeTreeChild::GraphicFrame(frame) = self.shape.into_leaf() else {
            return None;
        };
        let table = frame.graphic_data.table_mut()?;
        table_cell(table, row, column)?;
        Some(TableCellMut { table, row, column })
    }

    /// Returns one grid-column width in EMU.
    pub fn column_width(&self, column: usize) -> Option<Emu> {
        self.table().grid.columns.get(column).copied()
    }

    /// Returns the retained table style identity.
    pub fn style_id(&self) -> Option<&str> {
        self.table().properties.as_ref()?.style_id.as_deref()
    }

    /// Changes only the table style reference, preserving grid and source metadata.
    pub fn set_style_id(&mut self, value: Option<String>) -> Result<()> {
        if let Some(value) = value.as_deref() {
            let bytes = value.as_bytes();
            if bytes.len() != 38
                || bytes[0] != b'{'
                || bytes[37] != b'}'
                || !bytes[1..37].iter().enumerate().all(|(index, byte)| {
                    if [8, 13, 18, 23].contains(&index) {
                        *byte == b'-'
                    } else {
                        byte.is_ascii_hexdigit()
                    }
                })
            {
                return Err(invalid_table_mutation(
                    "set table style",
                    "style id must be a braced GUID".to_owned(),
                ));
            }
        }
        if self.style_id() == value.as_deref() {
            return Ok(());
        }
        if let Some(id) = value.as_deref() {
            let canonical = id.to_ascii_uppercase();
            let mut available =
                oxoffice_drawing::table::CT_TableStyle::builtin(&canonical).is_some();
            if let ShapeMutOwner::Presentation { presentation, .. } = &self.shape.owner
                && let Some(part) =
                    import_table_style_part(&presentation.package, &presentation.presentation_part)?
            {
                if presentation.package.content_types.content_type_for(&part)
                    != Some(oxoffice_opc::content_types::TABLE_STYLES)
                {
                    return Err(invalid_table_mutation(
                        "set table style",
                        "table style part has wrong content type".to_owned(),
                    ));
                }
                let styles = import_table_styles(required_part(&presentation.package, &part)?)
                    .map_err(|error| {
                        invalid_table_mutation("set table style", error.to_string())
                    })?;
                let matches = styles
                    .styles
                    .iter()
                    .filter(|(style, _)| style.eq_ignore_ascii_case(id))
                    .count();
                if matches > 1 {
                    return Err(invalid_table_mutation(
                        "set table style",
                        "ambiguous table style definitions".to_owned(),
                    ));
                }
                available |= matches == 1;
            }
            if !available {
                return Err(invalid_table_mutation(
                    "set table style",
                    "style is neither built in nor defined in this presentation".to_owned(),
                ));
            }
        }
        table_properties_mut(self.table_mut_raw()).style_id = value;
        Ok(())
    }

    /// Changes one grid width and synchronizes the containing frame width.
    pub(super) fn set_column_width_leaf(&mut self, column: usize, width: Emu) -> Result<()> {
        const OPERATION: &str = "set column width";
        if width.0 <= 0 {
            return Err(invalid_table_mutation(
                OPERATION,
                "column width must be positive".to_owned(),
            ));
        }
        if column >= self.table_mut_raw().grid.columns.len() {
            return Err(invalid_table_mutation(
                OPERATION,
                format!("column index {column} is out of range"),
            ));
        }
        let mut staged = self.table_mut_raw().clone();
        staged
            .set_column_width(column, width)
            .map_err(|error| invalid_table_mutation(OPERATION, error.to_string()))?;
        let (total_width, total_height) = table_extent(&staged, OPERATION)?;
        staged
            .to_xml()
            .map_err(|error| invalid_table_mutation(OPERATION, error.to_string()))?;
        *self.table_mut_raw() = staged;
        let height = self
            .transform_mut_raw()
            .extent
            .map_or(total_height, |extent| extent.cy);
        self.transform_mut_raw().extent = Some(CT_PositiveSize2D {
            cx: total_width,
            cy: height,
        });
        Ok(())
    }

    /// Returns one stored row height in EMU.
    pub fn row_height(&self, row: usize) -> Option<Emu> {
        self.table().rows.get(row).map(|row| row.height)
    }

    /// Changes one row height and synchronizes the containing frame height.
    ///
    /// The stored height is a minimum, as in PowerPoint, which grows a row to
    /// fit its text when it lays the table out.
    pub(super) fn set_row_height_leaf(&mut self, row: usize, height: Emu) -> Result<()> {
        const OPERATION: &str = "set row height";
        if height.0 <= 0 {
            return Err(invalid_table_mutation(
                OPERATION,
                "row height must be positive".to_owned(),
            ));
        }
        if row >= self.table_mut_raw().rows.len() {
            return Err(invalid_table_mutation(
                OPERATION,
                format!("row index {row} is out of range"),
            ));
        }
        let mut staged = self.table_mut_raw().clone();
        staged.rows[row].height = height;
        let (total_width, total_height) = table_extent(&staged, OPERATION)?;
        staged
            .to_xml()
            .map_err(|error| invalid_table_mutation(OPERATION, error.to_string()))?;
        self.table_mut_raw().rows[row].height = height;
        let width = self
            .transform_mut_raw()
            .extent
            .map_or(total_width, |extent| extent.cx);
        self.transform_mut_raw().extent = Some(CT_PositiveSize2D {
            cx: width,
            cy: total_height,
        });
        Ok(())
    }

    /// Inserts a row before `index`, or appends one when `index` equals the
    /// row count, and grows the frame height by the new row's height.
    ///
    /// The new row copies the height of the row above it, or of the first row
    /// when it becomes the first, and each new cell copies that row's cell
    /// properties and first-paragraph formatting without its text. A row
    /// inserted inside a merged cell extends the merge.
    pub(super) fn insert_row_leaf(&mut self, index: usize) -> Result<()> {
        self.restructure("insert row", |table| table.insert_row(index))
    }

    /// Removes one row, which must not be the only one, and shrinks the frame
    /// height by its height (a frame shorter than its rows first comes up to
    /// them).
    ///
    /// A merged cell across the row loses one row, and when the row holds the
    /// merge origin the row below takes it over with the origin's text.
    pub(super) fn remove_row_leaf(&mut self, index: usize) -> Result<()> {
        self.restructure("remove row", |table| table.remove_row(index))
    }

    /// Inserts a grid column before `index`, or appends one when `index`
    /// equals the column count, and grows the frame width by its width (a
    /// frame narrower than its grid first comes up to it).
    ///
    /// The new column copies the width of the column left of it, or of the
    /// first column when it becomes the first, and its cells copy that
    /// column's cells as `insert_row` does. A column inserted inside a merged
    /// cell extends the merge.
    pub(super) fn insert_column_leaf(&mut self, index: usize) -> Result<()> {
        self.restructure("insert column", |table| table.insert_column(index))
    }

    /// Removes one grid column, which must not be the only one, and shrinks
    /// the frame width by its width (a frame narrower than its grid first
    /// comes up to it).
    ///
    /// A merged cell across the column loses one column, and when the column
    /// holds the merge origin the column to its right takes it over with the
    /// origin's text.
    pub(super) fn remove_column_leaf(&mut self, index: usize) -> Result<()> {
        self.restructure("remove column", |table| table.remove_column(index))
    }

    /// Applies one row or column edit to a staged table and commits it.
    ///
    /// Each frame edge moves by the change of its grid sum, so the frame grows
    /// or shrinks by the inserted or removed row or column. A frame that
    /// PowerPoint measured taller than its stored rows, because it grew rows
    /// to fit their text, keeps that excess; a frame smaller than its grid
    /// first comes up to it.
    fn restructure(
        &mut self,
        operation: &'static str,
        edit: impl FnOnce(&mut CT_Table) -> std::result::Result<(), OxmlError>,
    ) -> Result<()> {
        let (old_width, old_height) = table_extent(self.table_mut_raw(), operation)?;
        let mut staged = self.table_mut_raw().clone();
        edit(&mut staged).map_err(|error| invalid_table_mutation(operation, error.to_string()))?;
        let (new_width, new_height) = table_extent(&staged, operation)?;
        staged
            .to_xml()
            .map_err(|error| invalid_table_mutation(operation, error.to_string()))?;
        let frame = self
            .transform_mut_raw()
            .extent
            .unwrap_or(CT_PositiveSize2D {
                cx: old_width,
                cy: old_height,
            });
        let extent = CT_PositiveSize2D {
            cx: moved_frame_edge(frame.cx, old_width, new_width, operation)?,
            cy: moved_frame_edge(frame.cy, old_height, new_height, operation)?,
        };
        *self.table_mut_raw() = staged;
        self.transform_mut_raw().extent = Some(extent);
        Ok(())
    }

    /// Returns whether first-row table styling is enabled.
    pub fn first_row(&self) -> bool {
        self.table()
            .properties
            .as_ref()
            .is_some_and(|properties| properties.first_row)
    }

    /// Enables or disables first-row table styling.
    pub fn set_first_row(&mut self, value: bool) {
        table_properties_mut(self.table_mut_raw()).first_row = value;
    }

    /// Returns whether last-row table styling is enabled.
    pub fn last_row(&self) -> bool {
        self.table()
            .properties
            .as_ref()
            .is_some_and(|properties| properties.last_row)
    }

    /// Enables or disables last-row table styling.
    pub fn set_last_row(&mut self, value: bool) {
        table_properties_mut(self.table_mut_raw()).last_row = value;
    }

    /// Returns whether first-column table styling is enabled.
    pub fn first_column(&self) -> bool {
        self.table()
            .properties
            .as_ref()
            .is_some_and(|properties| properties.first_column)
    }

    /// Enables or disables first-column table styling.
    pub fn set_first_column(&mut self, value: bool) {
        table_properties_mut(self.table_mut_raw()).first_column = value;
    }

    /// Returns whether last-column table styling is enabled.
    pub fn last_column(&self) -> bool {
        self.table()
            .properties
            .as_ref()
            .is_some_and(|properties| properties.last_column)
    }

    /// Enables or disables last-column table styling.
    pub fn set_last_column(&mut self, value: bool) {
        table_properties_mut(self.table_mut_raw()).last_column = value;
    }

    /// Returns whether horizontal row banding is enabled.
    pub fn horizontal_banding(&self) -> bool {
        self.table()
            .properties
            .as_ref()
            .is_some_and(|properties| properties.band_rows)
    }

    /// Enables or disables horizontal row banding.
    pub fn set_horizontal_banding(&mut self, value: bool) {
        table_properties_mut(self.table_mut_raw()).band_rows = value;
    }

    /// Returns whether vertical column banding is enabled.
    pub fn vertical_banding(&self) -> bool {
        self.table()
            .properties
            .as_ref()
            .is_some_and(|properties| properties.band_columns)
    }

    /// Enables or disables vertical column banding.
    pub fn set_vertical_banding(&mut self, value: bool) {
        table_properties_mut(self.table_mut_raw()).band_columns = value;
    }
    fn table(&self) -> &CT_Table {
        let ShapeTreeChild::GraphicFrame(frame) = self.shape.leaf() else {
            unreachable!("checked table owner")
        };
        match frame.graphic_data.payload() {
            GraphicDataPayload::Table(table) => table,
            _ => unreachable!("checked table payload"),
        }
    }
    fn table_mut_raw(&mut self) -> &mut CT_Table {
        let ShapeTreeChild::GraphicFrame(frame) = self.shape.leaf_mut() else {
            unreachable!("checked table owner")
        };
        frame
            .graphic_data
            .table_mut()
            .expect("checked table payload")
    }
    fn transform_mut_raw(&mut self) -> &mut CT_Transform2D {
        let ShapeTreeChild::GraphicFrame(frame) = self.shape.leaf_mut() else {
            unreachable!("checked table owner")
        };
        &mut frame.transform
    }
    pub fn set_column_width(&mut self, column: usize, width: Emu) -> Result<()> {
        self.shape
            .apply_geometry(GeometryEdit::TableColumn(column, width))
    }
    pub fn set_row_height(&mut self, row: usize, height: Emu) -> Result<()> {
        self.shape
            .apply_geometry(GeometryEdit::TableRow(row, height))
    }
    pub fn insert_row(&mut self, index: usize) -> Result<()> {
        self.shape
            .apply_geometry(GeometryEdit::TableInsertRow(index))
    }
    pub fn remove_row(&mut self, index: usize) -> Result<()> {
        self.shape
            .apply_geometry(GeometryEdit::TableRemoveRow(index))
    }
    pub fn insert_column(&mut self, index: usize) -> Result<()> {
        self.shape
            .apply_geometry(GeometryEdit::TableInsertColumn(index))
    }
    pub fn remove_column(&mut self, index: usize) -> Result<()> {
        self.shape
            .apply_geometry(GeometryEdit::TableRemoveColumn(index))
    }
}

/// One edge of a table cell, drawn by the `a:lnL`, `a:lnR`, `a:lnT`, or `a:lnB`
/// line of its `a:tcPr`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CellBorder {
    Left,
    Right,
    Top,
    Bottom,
}

/// A borrowed explicit table-grid cell.
#[derive(Clone, Copy)]
pub struct TableCellRef<'a> {
    cell: &'a CT_TableCell,
}

impl<'a> TableCellRef<'a> {
    /// Returns only the direct tcPr anchor, without creating properties.
    pub fn vertical_anchor(&self) -> Option<TextAnchor> {
        self.cell
            .properties
            .as_ref()
            .and_then(|properties| properties.anchor)
    }

    /// Returns the cell's visible plain text.
    pub fn text(&self) -> String {
        self.cell
            .text_body
            .as_ref()
            .map_or_else(String::new, CT_TextBody::plain_text)
    }

    /// Returns the cell text body when present.
    pub fn text_frame(&self) -> Option<TextFrameRef<'a>> {
        self.cell
            .text_body
            .as_ref()
            .map(|body| TextFrameRef { body })
    }

    /// Returns whether this cell is the top-left origin of a merge.
    pub fn is_merge_origin(&self) -> bool {
        !self.is_spanned() && (self.cell.row_span > 1 || self.cell.grid_span > 1)
    }

    /// Returns whether this cell continues a merge from another grid cell.
    pub fn is_spanned(&self) -> bool {
        self.cell.horizontal_merge || self.cell.vertical_merge
    }

    /// Returns the cell's own `rowSpan`: the merge height on a merge origin,
    /// one on an unmerged cell, and on a covered cell what its producer wrote
    /// (one, or its origin's).
    pub fn span_height(&self) -> u32 {
        self.cell.row_span
    }

    /// Returns the cell's own `gridSpan`: the merge width on a merge origin,
    /// one on an unmerged cell, and on a covered cell what its producer wrote
    /// (one, or its origin's).
    pub fn span_width(&self) -> u32 {
        self.cell.grid_span
    }

    /// Returns the direct cell fill, when present.
    pub fn fill(&self) -> Option<&Fill> {
        self.cell
            .properties
            .as_ref()
            .and_then(|properties| properties.fill.as_ref())
    }

    /// Returns the optional left, right, top, and bottom cell margins.
    pub fn margins(&self) -> (Option<Emu>, Option<Emu>, Option<Emu>, Option<Emu>) {
        self.cell
            .properties
            .as_ref()
            .map_or((None, None, None, None), |properties| {
                (
                    properties.margin_left,
                    properties.margin_right,
                    properties.margin_top,
                    properties.margin_bottom,
                )
            })
    }

    /// Returns the direct line of one cell edge, when present.
    pub fn border(&self, edge: CellBorder) -> Option<&CT_LineProperties> {
        let properties = self.cell.properties.as_ref()?;
        match edge {
            CellBorder::Left => properties.left.as_ref(),
            CellBorder::Right => properties.right.as_ref(),
            CellBorder::Top => properties.top.as_ref(),
            CellBorder::Bottom => properties.bottom.as_ref(),
        }
    }
}

/// A behavior-bearing mutable table-grid cell addressed within its table.
pub struct TableCellMut<'a> {
    table: &'a mut CT_Table,
    row: usize,
    column: usize,
}

impl<'a> TableCellMut<'a> {
    /// Returns only the direct cell anchor.
    pub fn vertical_anchor(&self) -> Option<TextAnchor> {
        self.cell_ref().vertical_anchor()
    }

    /// Updates ordinary cell formatting without replacing text or its owners.
    /// Clearing an absent anchor never materializes tcPr.
    pub fn set_vertical_anchor(&mut self, anchor: Option<TextAnchor>) {
        if self.vertical_anchor() == anchor {
            return;
        }
        if let Some(properties) = self.cell_mut().properties.as_mut() {
            properties.anchor = anchor;
        } else if anchor.is_some() {
            self.properties_mut().anchor = anchor;
        }
    }

    /// Returns the cell's visible plain text.
    pub fn text(&self) -> String {
        self.cell_ref().text()
    }

    /// Replaces the cell text with one paragraph per line, as
    /// [`CT_TextBody::set_text`] describes.
    pub fn set_text(&mut self, text: &str) {
        self.cell_mut()
            .text_body
            .get_or_insert_with(CT_TextBody::new)
            .set_text(text);
    }

    /// Returns the cell text body for in-place mutation.
    pub fn text_frame(&mut self) -> TextFrame<'_> {
        let body = self
            .cell_mut()
            .text_body
            .get_or_insert_with(CT_TextBody::new);
        TextFrame { body }
    }

    /// Consumes this cell handle and returns its text body for mutation.
    pub fn into_text_frame(self) -> TextFrame<'a> {
        let body = self.table.rows[self.row].cells[self.column]
            .text_body
            .get_or_insert_with(CT_TextBody::new);
        TextFrame { body }
    }

    /// Replaces or clears the direct cell fill.
    pub fn set_fill(&mut self, fill: Option<Fill>) {
        self.properties_mut().fill = fill;
    }

    /// Returns the direct cell fill, when present.
    pub fn fill(&self) -> Option<&Fill> {
        self.cell_ref().cell.properties.as_ref()?.fill.as_ref()
    }

    /// Replaces all four optional cell margins.
    pub fn set_margins(
        &mut self,
        left: Option<Emu>,
        right: Option<Emu>,
        top: Option<Emu>,
        bottom: Option<Emu>,
    ) {
        let properties = self.properties_mut();
        properties.margin_left = left;
        properties.margin_right = right;
        properties.margin_top = top;
        properties.margin_bottom = bottom;
    }

    /// Returns the optional left, right, top, and bottom cell margins.
    pub fn margins(&self) -> (Option<Emu>, Option<Emu>, Option<Emu>, Option<Emu>) {
        let Some(properties) = self.cell_ref().cell.properties.as_ref() else {
            return (None, None, None, None);
        };
        (
            properties.margin_left,
            properties.margin_right,
            properties.margin_top,
            properties.margin_bottom,
        )
    }

    /// Returns the direct line of one cell edge, when present.
    pub fn border(&self, edge: CellBorder) -> Option<&CT_LineProperties> {
        let properties = self.cell_ref().cell.properties.as_ref()?;
        match edge {
            CellBorder::Left => properties.left.as_ref(),
            CellBorder::Right => properties.right.as_ref(),
            CellBorder::Top => properties.top.as_ref(),
            CellBorder::Bottom => properties.bottom.as_ref(),
        }
    }

    /// Replaces or clears the direct line of one cell edge.
    pub fn set_border(&mut self, edge: CellBorder, line: Option<CT_LineProperties>) {
        let properties = self.properties_mut();
        let slot = match edge {
            CellBorder::Left => &mut properties.left,
            CellBorder::Right => &mut properties.right,
            CellBorder::Top => &mut properties.top,
            CellBorder::Bottom => &mut properties.bottom,
        };
        *slot = line;
    }

    /// Returns whether this cell is the top-left origin of a merge.
    pub fn is_merge_origin(&self) -> bool {
        self.cell_ref().is_merge_origin()
    }

    /// Returns whether this cell continues a merge from another grid cell.
    pub fn is_spanned(&self) -> bool {
        self.cell_ref().is_spanned()
    }

    /// Returns the cell's own `rowSpan`: the merge height on a merge origin,
    /// one on an unmerged cell, and on a covered cell what its producer wrote
    /// (one, or its origin's).
    pub fn span_height(&self) -> u32 {
        self.cell_ref().span_height()
    }

    /// Returns the cell's own `gridSpan`: the merge width on a merge origin,
    /// one on an unmerged cell, and on a covered cell what its producer wrote
    /// (one, or its origin's).
    pub fn span_width(&self) -> u32 {
        self.cell_ref().span_width()
    }

    /// Merges the rectangle between this cell and the other grid coordinate.
    pub fn merge_to(&mut self, row: usize, column: usize) -> Result<()> {
        let mut staged = self.table.clone();
        merge_cells(&mut staged, self.row, self.column, row, column)
            .map_err(|message| invalid_table_mutation("merge cells", message))?;
        staged
            .to_xml()
            .map_err(|error| invalid_table_mutation("merge cells", error.to_string()))?;
        *self.table = staged;
        Ok(())
    }

    /// Splits this merge origin back into its explicit grid cells.
    pub fn split(&mut self) -> Result<()> {
        let mut staged = self.table.clone();
        split_cell(&mut staged, self.row, self.column)
            .map_err(|message| invalid_table_mutation("split cell", message))?;
        staged
            .to_xml()
            .map_err(|error| invalid_table_mutation("split cell", error.to_string()))?;
        *self.table = staged;
        Ok(())
    }

    fn cell_ref(&self) -> TableCellRef<'_> {
        TableCellRef {
            cell: table_cell(self.table, self.row, self.column)
                .expect("TableCellMut coordinates were validated at construction"),
        }
    }

    fn cell_mut(&mut self) -> &mut CT_TableCell {
        &mut self.table.rows[self.row].cells[self.column]
    }

    fn properties_mut(&mut self) -> &mut CT_TableCellProperties {
        self.cell_mut()
            .properties
            .get_or_insert_with(CT_TableCellProperties::default)
    }
}

fn table_cell(table: &CT_Table, row: usize, column: usize) -> Option<&CT_TableCell> {
    table.rows.get(row)?.cells.get(column)
}

pub(super) fn table_properties_mut(table: &mut CT_Table) -> &mut CT_TableProperties {
    table
        .properties
        .get_or_insert_with(CT_TableProperties::default)
}

/// Returns the checked sums of a table's column widths and row heights.
fn table_extent(table: &CT_Table, operation: &'static str) -> Result<(Emu, Emu)> {
    let total_width = table
        .grid
        .columns
        .iter()
        .try_fold(0i64, |total, width| total.checked_add(width.0))
        .ok_or_else(|| {
            invalid_table_mutation(operation, "table width exceeds the EMU range".to_owned())
        })?;
    let total_height = table
        .rows
        .iter()
        .try_fold(0i64, |total, row| total.checked_add(row.height.0))
        .ok_or_else(|| {
            invalid_table_mutation(operation, "table height exceeds the EMU range".to_owned())
        })?;
    if total_width <= 0 || total_height <= 0 {
        return Err(invalid_table_mutation(
            operation,
            "table width and height must remain positive".to_owned(),
        ));
    }
    Ok((Emu(total_width), Emu(total_height)))
}

/// Moves a table frame edge by the grid's change. A frame larger than its
/// grid (rows grown by their text) keeps its margin; one smaller than its
/// grid (Google writes 3,000,000 EMU whatever the grid) is first brought up
/// to the grid, which is what every reader draws.
fn moved_frame_edge(
    frame: Emu,
    old_total: Emu,
    new_total: Emu,
    operation: &'static str,
) -> Result<Emu> {
    new_total
        .0
        .checked_sub(old_total.0)
        .and_then(|change| frame.0.max(old_total.0).checked_add(change))
        .filter(|size| *size > 0)
        .map(Emu)
        .ok_or_else(|| {
            invalid_table_mutation(
                operation,
                "table frame size must remain positive and within the EMU range".to_owned(),
            )
        })
}

fn rectangular_dimensions(table: &CT_Table) -> std::result::Result<(usize, usize), String> {
    let rows = table.rows.len();
    let columns = table.grid.columns.len();
    if rows == 0 || columns == 0 || table.rows.iter().any(|row| row.cells.len() != columns) {
        return Err("table does not have a rectangular explicit cell grid".to_owned());
    }
    Ok((rows, columns))
}

fn merge_cells(
    table: &mut CT_Table,
    first_row: usize,
    first_column: usize,
    second_row: usize,
    second_column: usize,
) -> std::result::Result<(), String> {
    let (rows, columns) = rectangular_dimensions(table)?;
    if first_row >= rows
        || second_row >= rows
        || first_column >= columns
        || second_column >= columns
    {
        return Err("merge coordinate is out of range".to_owned());
    }
    let top = first_row.min(second_row);
    let bottom = first_row.max(second_row);
    let left = first_column.min(second_column);
    let right = first_column.max(second_column);
    if top == bottom && left == right {
        return Err("merge requires at least two cells".to_owned());
    }
    if table.rows[top..=bottom].iter().any(|row| {
        row.cells[left..=right].iter().any(|cell| {
            cell.row_span != 1
                || cell.grid_span != 1
                || cell.horizontal_merge
                || cell.vertical_merge
        })
    }) {
        return Err("merge range contains one or more merged cells".to_owned());
    }
    let row_span = u32::try_from(bottom - top + 1)
        .map_err(|_| "merge row span exceeds the DrawingML range".to_owned())?;
    let grid_span = u32::try_from(right - left + 1)
        .map_err(|_| "merge column span exceeds the DrawingML range".to_owned())?;

    let mut moved_bodies = Vec::new();
    for row in top..=bottom {
        for column in left..=right {
            if row == top && column == left {
                continue;
            }
            let body = table.rows[row].cells[column]
                .text_body
                .take()
                .unwrap_or_default();
            moved_bodies.push((row, column, body));
        }
    }
    for (row, column, mut body) in moved_bodies {
        let origin_body = table.rows[top].cells[left]
            .text_body
            .get_or_insert_with(CT_TextBody::new);
        body.move_content_to(origin_body);
        table.rows[row].cells[column].text_body = Some(body);
    }

    for row in top..=bottom {
        for column in left..=right {
            let cell = &mut table.rows[row].cells[column];
            cell.row_span = if row == top { row_span } else { 1 };
            cell.grid_span = if column == left { grid_span } else { 1 };
            cell.horizontal_merge = column != left;
            cell.vertical_merge = row != top;
        }
    }
    Ok(())
}

fn split_cell(table: &mut CT_Table, row: usize, column: usize) -> std::result::Result<(), String> {
    let (rows, columns) = rectangular_dimensions(table)?;
    let origin = table_cell(table, row, column)
        .ok_or_else(|| "split coordinate is out of range".to_owned())?;
    if origin.horizontal_merge
        || origin.vertical_merge
        || (origin.row_span == 1 && origin.grid_span == 1)
    {
        return Err("only a merge-origin cell can be split".to_owned());
    }
    let bottom = row
        .checked_add(origin.row_span as usize - 1)
        .ok_or_else(|| "merge row span exceeds the table grid".to_owned())?;
    let right = column
        .checked_add(origin.grid_span as usize - 1)
        .ok_or_else(|| "merge column span exceeds the table grid".to_owned())?;
    if bottom >= rows || right >= columns {
        return Err("merge origin span exceeds the table grid".to_owned());
    }
    let row_span = origin.row_span;
    let grid_span = origin.grid_span;
    // The merge flags mark the covered cells. Their own spans are 1, or, in
    // the top row and the left column, repeat the origin's: PowerPoint and
    // python-pptx repeat them, LibreOffice writes 1.
    for current_row in row..=bottom {
        for current_column in column..=right {
            let cell = &table.rows[current_row].cells[current_column];
            let flags = (cell.horizontal_merge, cell.vertical_merge)
                == (current_column != column, current_row != row);
            let row_span_ok =
                cell.row_span == 1 || (current_row == row && cell.row_span == row_span);
            let grid_span_ok =
                cell.grid_span == 1 || (current_column == column && cell.grid_span == grid_span);
            if !(flags && row_span_ok && grid_span_ok) {
                return Err("merge origin does not have a valid continuation rectangle".to_owned());
            }
        }
    }
    for current_row in row..=bottom {
        for current_column in column..=right {
            let cell = &mut table.rows[current_row].cells[current_column];
            cell.row_span = 1;
            cell.grid_span = 1;
            cell.horizontal_merge = false;
            cell.vertical_merge = false;
        }
    }
    Ok(())
}
