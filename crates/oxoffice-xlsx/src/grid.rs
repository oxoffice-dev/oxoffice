//! The grid of a sheet: column widths, row heights and frozen panes.

use oxoffice_xlsx_oxml::reference::{parse_column_span, parse_row_span};
use oxoffice_xlsx_oxml::worksheet::{CT_Row, set_column_width, set_frozen_panes};
use oxoffice_xlsx_oxml::{CellRef, Tag, qualified};

use crate::{Error, Result, WorksheetMut};

/// The widest column Excel makes, in characters.
const MAX_COLUMN_WIDTH: f64 = 255.0;
/// The tallest row Excel makes, in points.
const MAX_ROW_HEIGHT: f64 = 409.5;
/// The most rows a call writes: a row an edit names is written even when it
/// holds nothing, so a span of the whole sheet is refused.
const MAX_ROWS_AT_ONCE: u32 = 100_000;

/// The width of a digit of the default font, in pixels: Calibri 11 and
/// Arial 10 at 96 dpi, Excel's usual. A column width counts characters of
/// that digit, and the file stores them with the padding of five pixels.
const MAX_DIGIT_WIDTH: f64 = 7.0;
const PADDING: f64 = 5.0;

/// The `width` of a `col` that shows `characters` of the default font, as
/// Excel stores it: whole pixels, five of padding, in 256ths of a digit.
/// 20 characters are `20.7109375`, the 8.43 of a new sheet `9.140625`.
pub(crate) fn stored_column_width(characters: f64) -> f64 {
    let pixels = if characters < 1.0 {
        (characters * (MAX_DIGIT_WIDTH + PADDING) + 0.5).trunc()
    } else {
        (characters * MAX_DIGIT_WIDTH + 0.5).trunc() + PADDING
    };
    (pixels / MAX_DIGIT_WIDTH * 256.0).trunc() / 256.0
}

fn invalid(message: String) -> Error {
    Error::InvalidArgument(message)
}

impl WorksheetMut<'_> {
    /// Sets the width of columns, in characters: `"A"`, or `"A:C"` for
    /// several.
    ///
    /// The width is what Excel's Column Width box shows for the default
    /// font (a digit of Calibri 11 or Arial 10 is 7 pixels wide), from just
    /// above 0 to 255. The file stores it with the padding Excel adds
    /// (`20` is written `20.7109375`) and `customWidth`; the `col` spans the
    /// columns cross are cut and keep their style and what else they say,
    /// except `bestFit`, which a width set by hand ends. A width of 0 hides
    /// a column, which is not written yet, and is refused.
    pub fn set_column_width(&mut self, columns: &str, width: f64) -> Result<()> {
        let (first, last) = parse_column_span(columns).ok_or_else(|| {
            invalid(format!(
                "{columns:?} is not a column or columns such as B or A:C"
            ))
        })?;
        if !width.is_finite() || !(0.0..=MAX_COLUMN_WIDTH).contains(&width) {
            return Err(invalid(format!(
                "a column width is a number of characters from 0 to {MAX_COLUMN_WIDTH}, not {width}"
            )));
        }
        if width == 0.0 {
            return Err(invalid(
                "a width of 0 would hide the columns, which is not written yet: give a width above 0"
                    .to_owned(),
            ));
        }
        self.require_worksheet("columns")?;
        let prefix = self.prefix()?;
        let before = self.child("cols")?;
        let after = set_column_width(
            before.clone(),
            &prefix,
            first,
            last,
            stored_column_width(width),
        );
        self.stage_child("cols", &before, Some(after));
        Ok(())
    }

    /// Sets the height of rows, in points: `"3"`, or `"2:5"` for several.
    ///
    /// From just above 0 to 409.5, with `customHeight`; a row the sheet
    /// does not hold is written, with nothing in it. At most 100,000 rows
    /// at once. A height of 0 hides a row, which is not written yet, and is
    /// refused.
    pub fn set_row_height(&mut self, rows: &str, height: f64) -> Result<()> {
        let (first, last) = parse_row_span(rows)
            .ok_or_else(|| invalid(format!("{rows:?} is not a row or rows such as 3 or 2:5")))?;
        if !height.is_finite() || !(0.0..=MAX_ROW_HEIGHT).contains(&height) {
            return Err(invalid(format!(
                "a row height is a number of points from 0 to {MAX_ROW_HEIGHT}, not {height}"
            )));
        }
        if height == 0.0 {
            return Err(invalid(
                "a height of 0 would hide the rows, which is not written yet: give a height above 0"
                    .to_owned(),
            ));
        }
        if last - first >= MAX_ROWS_AT_ONCE {
            return Err(Error::Refused(format!(
                "{rows:?} names {} rows, and at most {MAX_ROWS_AT_ONCE} are written at once",
                last - first + 1
            )));
        }
        self.require_worksheet("rows")?;
        let prefix = self.prefix()?;
        let existing = self.rows_in(first, last)?;
        let changes = (first..=last)
            .map(|number| {
                let before = existing.get(&number).cloned();
                let mut after = before.clone().unwrap_or_else(|| {
                    let mut tag = Tag::new(&qualified(&prefix, "row"));
                    tag.set("r", Some(&(number + 1).to_string()));
                    CT_Row {
                        tag,
                        children: Vec::new(),
                    }
                });
                after.tag.set_f64("ht", Some(height));
                after.tag.set_bool("customHeight", Some(true));
                (number, before, after)
            })
            .collect();
        self.stage_rows(changes);
        Ok(())
    }

    /// Freezes the panes above and left of `cell` (`"B2"` keeps row 1 and
    /// column A in view), or removes the frozen panes (`None`).
    ///
    /// Written as Excel writes a freeze at the cursor: the split, the first
    /// cell that scrolls, the active pane, and the cursor in it. Any pane
    /// and selection the view had is replaced; a split that is not frozen
    /// is left alone when removing. `A1` has nothing above or left of it
    /// and is refused.
    pub fn freeze_panes(&mut self, cell: Option<&str>) -> Result<()> {
        let at = cell
            .map(|text| {
                let at = CellRef::parse(text)
                    .ok_or_else(|| invalid(format!("{text:?} is not a cell such as B2")))?;
                if at == CellRef::new(0, 0) {
                    return Err(invalid(
                        "A1 has no rows above it and no columns left of it to freeze: give a cell below or right of it, or None to remove the panes"
                            .to_owned(),
                    ));
                }
                Ok(at)
            })
            .transpose()?;
        self.require_worksheet("panes")?;
        let prefix = self.prefix()?;
        let before = self.child("sheetViews")?;
        let after = set_frozen_panes(before.clone(), &prefix, at);
        self.stage_child("sheetViews", &before, after);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_width_in_characters_is_stored_as_excel_stores_it() {
        for (characters, stored) in [
            (8.43, 9.140625),
            (10.0, 10.7109375),
            (20.0, 20.7109375),
            (12.3, 13.0),
            (255.0, 255.7109375),
            (0.5, 0.85546875),
        ] {
            assert_eq!(stored_column_width(characters), stored, "{characters}");
        }
    }
}
