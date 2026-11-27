//! A worksheet part: the index one pass over it builds, and the writer that
//! splices edited rows and children into its source bytes.
//!
//! The first read or edit of a sheet makes one pass over its XML
//! ([`SheetIndex::build`]): the children of the root and their ranges, each
//! row's number and range, each shared formula master and each array and
//! data table range. A row becomes a [`CT_Row`] only when it is read or
//! edited. An edited sheet is written as its source bytes with each edited
//! row written again in its range, new rows at their place and the children
//! an edit changed at their schema position ([`splice`]): the root start tag,
//! every other child and every row not edited stay as they were. An edit
//! costs its rows, not the sheet.

mod page_setup;
mod row;

use std::collections::BTreeMap;
use std::ops::Range;

use oxoffice_core::OxmlError;
use oxoffice_core::xml_element::ElementSpan;

pub use page_setup::{PAGE_SETUP_CHILDREN, PageSetupElements};
pub use row::{
    CELL_ATTRIBUTES, CT_Cell, CT_CellFormula, CT_Row, CellChild, FORMULA_ATTRIBUTES,
    ROW_ATTRIBUTES, RowChild, TextElement, fixed_attributes,
};

use crate::namespaces::MAIN_NAMESPACES;
use crate::reference::{CellRange, CellRef, MAX_COLUMNS, MAX_ROWS};
use crate::xml::{
    Child, Opened, Result, Splice, Tag, XmlSource, append_to, insertion_point, local_name_of,
    prefix_of, qualified,
};

/// The children of a worksheet (`CT_Worksheet`), in schema order.
pub const WORKSHEET_CHILDREN: [&str; 39] = [
    "sheetPr",
    "dimension",
    "sheetViews",
    "sheetFormatPr",
    "cols",
    "sheetData",
    "sheetCalcPr",
    "sheetProtection",
    "protectedRanges",
    "scenarios",
    "autoFilter",
    "sortState",
    "dataConsolidate",
    "customSheetViews",
    "mergeCells",
    "phoneticPr",
    "conditionalFormatting",
    "dataValidations",
    "hyperlinks",
    "printOptions",
    "pageMargins",
    "pageSetup",
    "headerFooter",
    "rowBreaks",
    "colBreaks",
    "customProperties",
    "cellWatches",
    "ignoredErrors",
    "smartTags",
    "drawing",
    "legacyDrawing",
    "legacyDrawingHF",
    "drawingHF",
    "picture",
    "oleObjects",
    "controls",
    "webPublishItems",
    "tableParts",
    "extLst",
];

/// A child of the root: its local name and its range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootChild {
    pub name: String,
    pub range: Range<usize>,
}

/// A row of `sheetData`: its 0-based index and its range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowSpan {
    pub row: u32,
    pub range: Range<usize>,
}

/// The master of a shared formula group: its cell, its text and the range
/// it gives the group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedFormula {
    pub cell: CellRef,
    pub text: String,
    pub range: Option<CellRange>,
}

/// What a formula range holds: an array formula or a data table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeFormulaKind {
    Array,
    DataTable,
}

/// An array formula or a data table: the cell that holds it and its range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RangeFormula {
    pub cell: CellRef,
    pub range: CellRange,
    pub kind: RangeFormulaKind,
}

/// What one pass over a worksheet (or any sheet part) records. See the
/// [module](self).
#[derive(Clone, Debug)]
pub struct SheetIndex {
    /// The root element's ranges.
    pub root: ElementSpan,
    /// The root start tag, as read.
    pub root_tag: Tag,
    /// The prefix of the SpreadsheetML namespace on the root, `""` for the
    /// default namespace: what new elements are written with. `None` when
    /// the root binds neither writing of the namespace.
    pub prefix: Option<String>,
    /// The children of the root, in order.
    pub children: Vec<RootChild>,
    /// The ranges of `sheetData`, when the sheet has one.
    pub sheet_data: Option<ElementSpan>,
    /// The tag of `sheetData`, as read.
    pub sheet_data_tag: Option<Tag>,
    /// The rows, in the order written.
    pub rows: Vec<RowSpan>,
    /// The masters of the shared formula groups, by group (`si`).
    pub shared_formulas: BTreeMap<u32, SharedFormula>,
    /// The array formulas and data tables, in the order written.
    pub range_formulas: Vec<RangeFormula>,
}

impl SheetIndex {
    /// Indexes `xml`, a sheet part. A row or a cell without `r` takes the
    /// number after the one before it.
    pub fn build(xml: &[u8]) -> Result<Self> {
        let mut source = XmlSource::new(xml);
        let root = source.root()?;
        let root_tag = Tag::read(&root.start);
        let mut index = SheetIndex {
            root: ElementSpan {
                name: root_tag.name(),
                start_tag: root.tag.clone(),
                content: root.tag.end..root.tag.end,
                end_tag: root.tag.end..root.tag.end,
            },
            prefix: prefix_of(&root_tag, &MAIN_NAMESPACES),
            root_tag,
            children: Vec::new(),
            sheet_data: None,
            sheet_data_tag: None,
            rows: Vec::new(),
            shared_formulas: BTreeMap::new(),
            range_formulas: Vec::new(),
        };
        if root.empty {
            return Ok(index);
        }
        loop {
            match source.next_child()? {
                Child::Element(child) => {
                    let name =
                        String::from_utf8_lossy(child.start.local_name().as_ref()).into_owned();
                    let span = if child.is("sheetData") && index.sheet_data.is_none() {
                        index.sheet_data_tag = Some(Tag::read(&child.start));
                        let span = index.read_sheet_data(&mut source, &child)?;
                        index.sheet_data = Some(span.clone());
                        span
                    } else {
                        source.finish(&child)?
                    };
                    index.children.push(RootChild {
                        name,
                        range: span.range(),
                    });
                }
                Child::Raw(_) => {}
                Child::End(end) => {
                    index.root.content = root.tag.end..end.start;
                    index.root.end_tag = end;
                    return Ok(index);
                }
            }
        }
    }

    fn read_sheet_data(
        &mut self,
        source: &mut XmlSource<'_>,
        opened: &Opened<'_>,
    ) -> Result<ElementSpan> {
        let mut span = ElementSpan {
            name: String::from_utf8_lossy(opened.start.name().as_ref()).into_owned(),
            start_tag: opened.tag.clone(),
            content: opened.tag.end..opened.tag.end,
            end_tag: opened.tag.end..opened.tag.end,
        };
        if opened.empty {
            return Ok(span);
        }
        let mut next_row = 0u32;
        loop {
            match source.next_child()? {
                Child::Element(row) if row.is("row") => {
                    let number = match Tag::read(&row.start).u32("r") {
                        Some(r) if (1..=MAX_ROWS).contains(&r) => r - 1,
                        Some(r) => {
                            return Err(OxmlError::InvalidValue(format!(
                                "row number {r} is outside the sheet"
                            )));
                        }
                        None => next_row,
                    };
                    next_row = number + 1;
                    self.read_row_formulas(source, &row, number)?;
                    self.rows.push(RowSpan {
                        row: number,
                        range: row.tag.start..source.position(),
                    });
                }
                Child::Element(other) => {
                    source.finish(&other)?;
                }
                Child::Raw(_) => {}
                Child::End(end) => {
                    span.content = opened.tag.end..end.start;
                    span.end_tag = end;
                    return Ok(span);
                }
            }
        }
    }

    /// Reads a row to its end, recording its shared formula masters and its
    /// array and data table ranges.
    fn read_row_formulas(
        &mut self,
        source: &mut XmlSource<'_>,
        row: &Opened<'_>,
        number: u32,
    ) -> Result<()> {
        if row.empty {
            return Ok(());
        }
        let mut next_column = 0u32;
        loop {
            match source.next_child()? {
                Child::Element(cell) if cell.is("c") => {
                    let column = match Tag::read(&cell.start).get("r") {
                        Some(r) => {
                            CellRef::parse(&r)
                                .ok_or_else(|| {
                                    OxmlError::InvalidValue(format!("{r} is not a cell reference"))
                                })?
                                .column
                        }
                        None if next_column < MAX_COLUMNS => next_column,
                        None => {
                            return Err(OxmlError::InvalidValue(
                                "a row holds more cells than a sheet has columns".to_owned(),
                            ));
                        }
                    };
                    next_column = column + 1;
                    self.read_cell_formula(source, &cell, CellRef::new(number, column))?;
                }
                Child::Element(other) => {
                    source.finish(&other)?;
                }
                Child::Raw(_) => {}
                Child::End(_) => return Ok(()),
            }
        }
    }

    fn read_cell_formula(
        &mut self,
        source: &mut XmlSource<'_>,
        opened: &Opened<'_>,
        cell: CellRef,
    ) -> Result<()> {
        if opened.empty {
            return Ok(());
        }
        loop {
            match source.next_child()? {
                Child::Element(formula) if formula.is("f") => {
                    let tag = Tag::read(&formula.start);
                    let text = source.text(&formula)?;
                    let range = tag.get("ref").as_deref().and_then(CellRange::parse);
                    match tag.get("t").as_deref() {
                        Some("shared") if !text.is_empty() => {
                            if let Some(si) = tag.u32("si") {
                                self.shared_formulas.entry(si).or_insert(SharedFormula {
                                    cell,
                                    text,
                                    range,
                                });
                            }
                        }
                        Some(kind @ ("array" | "dataTable")) => {
                            let kind = if kind == "array" {
                                RangeFormulaKind::Array
                            } else {
                                RangeFormulaKind::DataTable
                            };
                            self.range_formulas.push(RangeFormula {
                                cell,
                                range: range.unwrap_or(CellRange::new(cell, cell)),
                                kind,
                            });
                        }
                        _ => {}
                    }
                }
                Child::Element(other) => {
                    source.finish(&other)?;
                }
                Child::Raw(_) => {}
                Child::End(_) => return Ok(()),
            }
        }
    }

    /// The row with this 0-based index, the first one written when two say
    /// the same.
    pub fn row(&self, row: u32) -> Option<&RowSpan> {
        self.rows.iter().find(|span| span.row == row)
    }

    /// The first child of the root named `local`.
    pub fn child(&self, local: &str) -> Option<&RootChild> {
        self.children.iter().find(|child| child.name == local)
    }

    /// Where a new child named `local` goes: before the first child the
    /// schema puts after it, else at the end of the root.
    fn insertion_point(&self, local: &str) -> usize {
        let children = self
            .children
            .iter()
            .map(|child| (child.name.as_str(), child.range.start));
        insertion_point(children, self.root.content.end, &WORKSHEET_CHILDREN, local)
    }
}

/// The edits of one sheet, made at once by [`splice`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetEdits {
    /// Rows by 0-based index: written again in place of the row the sheet
    /// has, written at their place for a row it does not have, or removed
    /// (`None`).
    pub rows: BTreeMap<u32, Option<CT_Row>>,
    /// Children of the root by local name, written as given: in place of the
    /// first child of that name, at its schema position when the sheet has
    /// none, or removed (`None`).
    pub children: BTreeMap<String, Option<Vec<u8>>>,
}

impl SheetEdits {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.children.is_empty()
    }
}

/// `xml`, the sheet `index` was built from, with `edits` made. See the
/// [module](self).
pub fn splice(xml: &[u8], index: &SheetIndex, edits: &SheetEdits) -> Result<Vec<u8>> {
    let mut splice = Splice::default();
    let mut root_insertions: Vec<(usize, Vec<u8>)> = Vec::new();
    for (name, edit) in &edits.children {
        if name == "sheetData" {
            return Err(OxmlError::InvalidValue(
                "sheetData is edited by its rows".to_owned(),
            ));
        }
        match (index.child(name), edit) {
            (Some(child), Some(bytes)) => splice.replace(child.range.clone(), bytes.clone()),
            (Some(child), None) => splice.replace(child.range.clone(), Vec::new()),
            (None, Some(bytes)) => {
                root_insertions.push((index.insertion_point(name), bytes.clone()))
            }
            (None, None) => {}
        }
    }

    // Rows: replaced or removed in place, new ones before the first row
    // written after them.
    let mut new_rows: Vec<(usize, Vec<u8>)> = Vec::new();
    for (&number, edit) in &edits.rows {
        let mut written = Vec::new();
        if let Some(row) = edit {
            row.write(&mut written);
        }
        match index.row(number) {
            Some(span) => splice.replace(span.range.clone(), written),
            None if edit.is_some() => {
                let at = index
                    .rows
                    .iter()
                    .find(|span| span.row > number)
                    .map(|span| span.range.start);
                new_rows.push((at.unwrap_or(usize::MAX), written));
            }
            None => {}
        }
    }
    if !new_rows.is_empty() {
        match (&index.sheet_data, &index.sheet_data_tag) {
            (Some(span), Some(tag)) => {
                let (placed, at_end): (Vec<_>, Vec<_>) =
                    new_rows.into_iter().partition(|(at, _)| *at != usize::MAX);
                for (at, bytes) in placed {
                    splice.insert(at, bytes);
                }
                let tail: Vec<u8> = at_end.into_iter().flat_map(|(_, bytes)| bytes).collect();
                append_to(&mut splice, span, tag, None, tail);
            }
            _ => {
                let prefix = index.prefix.as_deref().unwrap_or("");
                let tag = Tag::new(&qualified(prefix, "sheetData"));
                let mut element = Vec::new();
                tag.write_start(&mut element);
                for (_, bytes) in new_rows {
                    element.extend_from_slice(&bytes);
                }
                tag.write_end(&mut element);
                root_insertions.push((index.insertion_point("sheetData"), element));
            }
        }
    }
    if !root_insertions.is_empty() {
        if index.root.is_self_closing() {
            let content = root_insertions
                .into_iter()
                .flat_map(|(_, bytes)| bytes)
                .collect();
            append_to(&mut splice, &index.root, &index.root_tag, None, content);
        } else {
            // Children inserted at one place keep the schema order.
            let rank = |bytes: &[u8]| {
                local_name_of(bytes)
                    .and_then(|name| WORKSHEET_CHILDREN.iter().position(|known| *known == name))
            };
            root_insertions.sort_by_key(|(at, bytes)| (*at, rank(bytes)));
            for (at, bytes) in root_insertions {
                splice.insert(at, bytes);
            }
        }
    }
    Ok(splice.apply(xml))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHEET: &str = concat!(
        r#"<?xml version="1.0"?>"#,
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
        r#"<dimension ref="A1:B3"/>"#,
        r#"<sheetData><row r="1"><c r="A1"><v>1</v></c></row>"#,
        r#"<row r="3" customHeight="true" ht="20"><c r="B3"><f t="shared" ref="B3:B4" si="0">A1*2</f><v>2</v></c></row>"#,
        r#"<row><c><f t="shared" si="0"/></c><c t="s"><f t="array" ref="B4:C5">A1:B2</f></c></row>"#,
        r#"</sheetData><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/>"#,
        r#"</worksheet>"#
    );

    /// The third row of `SHEET`, as written.
    fn row3() -> &'static str {
        let start = SHEET.find(r#"<row r="3""#).unwrap();
        let end = start + SHEET[start..].find("</row>").unwrap() + "</row>".len();
        &SHEET[start..end]
    }

    fn row(xml: &str) -> CT_Row {
        CT_Row::parse(xml.as_bytes()).unwrap()
    }

    #[test]
    fn one_pass_indexes_rows_children_and_formula_ranges() {
        let index = SheetIndex::build(SHEET.as_bytes()).unwrap();
        let names: Vec<_> = index
            .children
            .iter()
            .map(|child| child.name.as_str())
            .collect();
        assert_eq!(names, ["dimension", "sheetData", "pageMargins"]);
        let rows: Vec<_> = index.rows.iter().map(|row| row.row).collect();
        assert_eq!(rows, [0, 2, 3], "a row without r follows the one before it");
        assert_eq!(index.prefix.as_deref(), Some(""));
        let master = &index.shared_formulas[&0];
        assert_eq!(
            (master.cell, master.text.as_str()),
            (CellRef::new(2, 1), "A1*2")
        );
        let array = &index.range_formulas[0];
        assert_eq!(
            (array.cell, array.range, array.kind),
            (
                CellRef::new(3, 1),
                CellRange::parse("B4:C5").unwrap(),
                RangeFormulaKind::Array
            )
        );
        assert_eq!(
            &SHEET[index.row(2).unwrap().range.clone()][..15],
            r#"<row r="3" cust"#
        );
    }

    #[test]
    fn a_splice_writes_only_what_the_edits_name() {
        let replaced = r#"<row r="3"><c r="B3"><v>9</v></c></row>"#;
        let cases: [(&str, &str, SheetEdits, &str); 7] = [
            ("nothing", SHEET, SheetEdits::default(), SHEET),
            (
                "a row replaced, the fixed writing of what it read",
                SHEET,
                SheetEdits {
                    rows: [(2, Some(row(row3())))].into(),
                    ..SheetEdits::default()
                },
                &SHEET.replace(r#"customHeight="true""#, r#"customHeight="1""#),
            ),
            (
                "rows added before, between and after, one removed",
                SHEET,
                SheetEdits {
                    rows: [
                        (0, None),
                        (1, Some(row(r#"<row r="2"/>"#))),
                        (2, Some(row(replaced))),
                        (9, Some(row(r#"<row r="10"/>"#))),
                    ]
                    .into(),
                    ..SheetEdits::default()
                },
                &SHEET
                    .replace(
                        r#"<row r="1"><c r="A1"><v>1</v></c></row>"#,
                        r#"<row r="2"/>"#,
                    )
                    .replace(row3(), replaced)
                    .replace("</sheetData>", r#"<row r="10"/></sheetData>"#),
            ),
            (
                "children replaced, removed and added at their schema position",
                SHEET,
                SheetEdits {
                    children: [
                        (
                            "dimension".to_owned(),
                            Some(br#"<dimension ref="A1:C9"/>"#.to_vec()),
                        ),
                        ("pageMargins".to_owned(), None),
                        ("sheetViews".to_owned(), Some(b"<sheetViews/>".to_vec())),
                        ("cols".to_owned(), Some(b"<cols/>".to_vec())),
                        ("extLst".to_owned(), Some(b"<extLst/>".to_vec())),
                    ]
                    .into(),
                    ..SheetEdits::default()
                },
                &SHEET
                    .replace(
                        r#"<dimension ref="A1:B3"/>"#,
                        r#"<dimension ref="A1:C9"/><sheetViews/><cols/>"#,
                    )
                    .replace(
                        &SHEET[SHEET.find("<pageMargins").unwrap()
                            ..SHEET.find("</worksheet>").unwrap()],
                        "<extLst/>",
                    ),
            ),
            (
                "a row in an empty sheetData written with end tags",
                r#"<worksheet xmlns="urn:x"><sheetData/></worksheet>"#,
                SheetEdits {
                    rows: [(0, Some(row(r#"<row r="1"/>"#)))].into(),
                    ..SheetEdits::default()
                },
                r#"<worksheet xmlns="urn:x"><sheetData><row r="1"/></sheetData></worksheet>"#,
            ),
            (
                "sheetData written under the prefix the root binds",
                r#"<x:worksheet xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><x:pageMargins/></x:worksheet>"#,
                SheetEdits {
                    rows: [(0, Some(row(r#"<x:row r="1"/>"#)))].into(),
                    ..SheetEdits::default()
                },
                r#"<x:worksheet xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><x:sheetData><x:row r="1"/></x:sheetData><x:pageMargins/></x:worksheet>"#,
            ),
            (
                "children of an empty root",
                r#"<worksheet xmlns="http://purl.oclc.org/ooxml/spreadsheetml/main"/>"#,
                SheetEdits {
                    children: [(
                        "dimension".to_owned(),
                        Some(br#"<dimension ref="A1"/>"#.to_vec()),
                    )]
                    .into(),
                    ..SheetEdits::default()
                },
                r#"<worksheet xmlns="http://purl.oclc.org/ooxml/spreadsheetml/main"><dimension ref="A1"/></worksheet>"#,
            ),
        ];
        for (case, source, edits, expected) in cases {
            let index = SheetIndex::build(source.as_bytes()).unwrap();
            let written = splice(source.as_bytes(), &index, &edits).unwrap();
            assert_eq!(String::from_utf8(written).unwrap(), expected, "{case}");
        }
    }
}
