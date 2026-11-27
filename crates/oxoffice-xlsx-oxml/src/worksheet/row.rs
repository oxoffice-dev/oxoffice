//! A row of `sheetData` and its cells (`CT_Row`, `CT_Cell`), read when a row
//! is read or edited. Their start tags are kept as read, attributes another
//! producer added (`x14ac:dyDescent`, `xr:uid`) included, but the attributes
//! of the schema with one fixed writing, which take it as they are read
//! (LibreOffice's `customHeight="false"` reads `customHeight="0"`): a row an
//! edit writes again holds them in that writing. Children the model does not
//! know are kept as written.

use oxoffice_core::OxmlError;
use oxoffice_core::xml::escape_text;

use crate::reference::{CellRange, CellRef};
use crate::shared_strings::CT_Rst;
use crate::xml::{Child, Element, Lexical, Opened, Result, Tag, XmlSource};

/// The attributes of a row with one fixed writing.
pub const ROW_ATTRIBUTES: [(&str, Lexical); 11] = [
    ("r", Lexical::UInt),
    ("s", Lexical::UInt),
    ("customFormat", Lexical::Bool),
    ("ht", Lexical::Double),
    ("hidden", Lexical::Bool),
    ("customHeight", Lexical::Bool),
    ("outlineLevel", Lexical::UInt),
    ("collapsed", Lexical::Bool),
    ("thickTop", Lexical::Bool),
    ("thickBot", Lexical::Bool),
    ("ph", Lexical::Bool),
];

/// The attributes of a cell with one fixed writing.
pub const CELL_ATTRIBUTES: [(&str, Lexical); 4] = [
    ("s", Lexical::UInt),
    ("cm", Lexical::UInt),
    ("vm", Lexical::UInt),
    ("ph", Lexical::Bool),
];

/// The attributes of a formula with one fixed writing.
pub const FORMULA_ATTRIBUTES: [(&str, Lexical); 8] = [
    ("aca", Lexical::Bool),
    ("dt2D", Lexical::Bool),
    ("dtr", Lexical::Bool),
    ("del1", Lexical::Bool),
    ("del2", Lexical::Bool),
    ("ca", Lexical::Bool),
    ("si", Lexical::UInt),
    ("bx", Lexical::Bool),
];

/// The attributes of an element of a row (`row`, `c`, `f`) that take their
/// fixed writing when read.
pub fn fixed_attributes(element: &str) -> &'static [(&'static str, Lexical)] {
    match element {
        "row" => &ROW_ATTRIBUTES,
        "c" => &CELL_ATTRIBUTES,
        "f" => &FORMULA_ATTRIBUTES,
        _ => &[],
    }
}

/// The start tag `opened`, its attributes of `fixed` in their fixed writing.
fn read_tag(opened: &Opened<'_>, fixed: &[(&str, Lexical)]) -> Tag {
    let mut tag = Tag::read(&opened.start);
    tag.normalize(fixed);
    tag
}

/// A row: its start tag and its cells, in the order written.
#[derive(Clone, Debug, PartialEq)]
pub struct CT_Row {
    pub tag: Tag,
    pub children: Vec<RowChild>,
}

/// A child of a row.
#[derive(Clone, Debug, PartialEq)]
pub enum RowChild {
    Cell(CT_Cell),
    /// Kept as written: an `extLst`, a comment, an element the schema does
    /// not name.
    Other(Vec<u8>),
}

/// A cell: its start tag and its children, in the order written.
#[derive(Clone, Debug, PartialEq)]
pub struct CT_Cell {
    pub tag: Tag,
    pub children: Vec<CellChild>,
}

/// A child of a cell.
#[derive(Clone, Debug, PartialEq)]
pub enum CellChild {
    /// `f`
    Formula(CT_CellFormula),
    /// `v`, the value as written.
    Value(TextElement),
    /// `is`, an inline string.
    InlineString(CT_Rst),
    /// Kept as written: an `extLst`, a comment, an element the schema does
    /// not name.
    Other(Vec<u8>),
}

/// `f`: a formula, its start tag (kind, range, group, flags) and its text as
/// stored.
#[derive(Clone, Debug, PartialEq)]
pub struct CT_CellFormula {
    pub tag: Tag,
    pub text: String,
}

/// An element that holds text: `v`, `t`.
#[derive(Clone, Debug, PartialEq)]
pub struct TextElement {
    pub tag: Tag,
    pub text: String,
}

impl TextElement {
    fn write(&self, out: &mut Vec<u8>) {
        if self.text.is_empty() {
            self.tag.write_empty(out);
        } else {
            self.tag.write_start(out);
            out.extend_from_slice(escape_text(&self.text).as_bytes());
            self.tag.write_end(out);
        }
    }
}

fn expect(opened: &Opened<'_>, local: &str) -> Result<()> {
    if opened.is(local) {
        Ok(())
    } else {
        Err(OxmlError::UnexpectedElement(format!(
            "{} where {local} was expected",
            String::from_utf8_lossy(opened.start.name().as_ref())
        )))
    }
}

impl CT_Row {
    /// The row `xml` holds, the `row` element alone.
    pub fn parse(xml: &[u8]) -> Result<Self> {
        let mut source = XmlSource::new(xml);
        let opened = source.root()?;
        expect(&opened, "row")?;
        Self::read(&mut source, &opened)
    }

    pub(crate) fn read(source: &mut XmlSource<'_>, opened: &Opened<'_>) -> Result<Self> {
        let mut row = CT_Row {
            tag: read_tag(opened, &ROW_ATTRIBUTES),
            children: Vec::new(),
        };
        if opened.empty {
            return Ok(row);
        }
        loop {
            let child = match source.next_child()? {
                Child::Element(cell) if cell.is("c") => {
                    RowChild::Cell(CT_Cell::read(source, &cell)?)
                }
                Child::Element(other) => {
                    let span = source.finish(&other)?;
                    RowChild::Other(source.bytes(span.range()).to_vec())
                }
                Child::Raw(range) => RowChild::Other(source.bytes(range).to_vec()),
                Child::End(_) => return Ok(row),
            };
            row.children.push(child);
        }
    }

    /// Writes the row.
    pub fn write(&self, out: &mut Vec<u8>) {
        if self.children.is_empty() {
            self.tag.write_empty(out);
            return;
        }
        self.tag.write_start(out);
        for child in &self.children {
            match child {
                RowChild::Cell(cell) => cell.write(out),
                RowChild::Other(bytes) => out.extend_from_slice(bytes),
            }
        }
        self.tag.write_end(out);
    }

    /// The row number written in `r`, 1-based.
    pub fn r(&self) -> Option<u32> {
        self.tag.u32("r")
    }

    /// The cells, in the order written.
    pub fn cells(&self) -> impl Iterator<Item = &CT_Cell> {
        self.children.iter().filter_map(|child| match child {
            RowChild::Cell(cell) => Some(cell),
            RowChild::Other(_) => None,
        })
    }

    /// The height in points (`ht`).
    pub fn ht(&self) -> Option<f64> {
        self.tag.f64("ht")
    }

    /// `customHeight`, false when absent.
    pub fn custom_height(&self) -> bool {
        self.tag.bool("customHeight").unwrap_or(false)
    }

    /// `hidden`, false when absent.
    pub fn hidden(&self) -> bool {
        self.tag.bool("hidden").unwrap_or(false)
    }

    /// The style of the row (`s`), used when `customFormat` is set.
    pub fn s(&self) -> Option<u32> {
        self.tag.u32("s")
    }

    /// `customFormat`, false when absent.
    pub fn custom_format(&self) -> bool {
        self.tag.bool("customFormat").unwrap_or(false)
    }

    /// `outlineLevel`, 0 when absent.
    pub fn outline_level(&self) -> u32 {
        self.tag.u32("outlineLevel").unwrap_or(0)
    }
}

impl CT_Cell {
    pub(crate) fn read(source: &mut XmlSource<'_>, opened: &Opened<'_>) -> Result<Self> {
        let mut cell = CT_Cell {
            tag: read_tag(opened, &CELL_ATTRIBUTES),
            children: Vec::new(),
        };
        if opened.empty {
            return Ok(cell);
        }
        loop {
            let child = match source.next_child()? {
                Child::Element(formula) if formula.is("f") => CellChild::Formula(CT_CellFormula {
                    tag: read_tag(&formula, &FORMULA_ATTRIBUTES),
                    text: source.text(&formula)?,
                }),
                Child::Element(value) if value.is("v") => CellChild::Value(TextElement {
                    tag: Tag::read(&value.start),
                    text: source.text(&value)?,
                }),
                Child::Element(inline) if inline.is("is") => {
                    CellChild::InlineString(CT_Rst(Element::read(source, &inline)?))
                }
                Child::Element(other) => {
                    let span = source.finish(&other)?;
                    CellChild::Other(source.bytes(span.range()).to_vec())
                }
                Child::Raw(range) => CellChild::Other(source.bytes(range).to_vec()),
                Child::End(_) => return Ok(cell),
            };
            cell.children.push(child);
        }
    }

    /// Writes the cell.
    pub fn write(&self, out: &mut Vec<u8>) {
        if self.children.is_empty() {
            self.tag.write_empty(out);
            return;
        }
        self.tag.write_start(out);
        for child in &self.children {
            match child {
                CellChild::Formula(formula) => formula.write(out),
                CellChild::Value(value) => value.write(out),
                CellChild::InlineString(text) => text.0.write(out),
                CellChild::Other(bytes) => out.extend_from_slice(bytes),
            }
        }
        self.tag.write_end(out);
    }

    /// The cell written in `r`.
    pub fn r(&self) -> Option<CellRef> {
        self.tag.get("r").as_deref().and_then(CellRef::parse)
    }

    /// The style index (`s`), 0 when absent.
    pub fn s(&self) -> u32 {
        self.tag.u32("s").unwrap_or(0)
    }

    /// The type as written (`t`): `n` when absent.
    pub fn t(&self) -> String {
        self.tag.get("t").unwrap_or_else(|| "n".to_owned())
    }

    /// The formula.
    pub fn formula(&self) -> Option<&CT_CellFormula> {
        self.children.iter().find_map(|child| match child {
            CellChild::Formula(formula) => Some(formula),
            _ => None,
        })
    }

    /// The value as written in `v`.
    pub fn value(&self) -> Option<&str> {
        self.children.iter().find_map(|child| match child {
            CellChild::Value(value) => Some(value.text.as_str()),
            _ => None,
        })
    }

    /// The inline string (`is`).
    pub fn inline_string(&self) -> Option<&CT_Rst> {
        self.children.iter().find_map(|child| match child {
            CellChild::InlineString(text) => Some(text),
            _ => None,
        })
    }
}

impl CT_CellFormula {
    fn write(&self, out: &mut Vec<u8>) {
        TextElement {
            tag: self.tag.clone(),
            text: self.text.clone(),
        }
        .write(out);
    }

    /// The kind as written (`t`): `normal` when absent, `shared`, `array`,
    /// `dataTable`.
    pub fn t(&self) -> String {
        self.tag.get("t").unwrap_or_else(|| "normal".to_owned())
    }

    /// The range of a shared group's master, an array or a data table
    /// (`ref`).
    pub fn reference(&self) -> Option<CellRange> {
        self.tag.get("ref").as_deref().and_then(CellRange::parse)
    }

    /// The shared group (`si`).
    pub fn si(&self) -> Option<u32> {
        self.tag.u32("si")
    }

    /// `ca`, calculate the cell on open: false when absent.
    pub fn ca(&self) -> bool {
        self.tag.bool("ca").unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_reads_its_cells_and_writes_what_it_read() {
        let xml = concat!(
            r#"<row r="2" spans="1:3" customHeight="true" ht="15" x14ac:dyDescent="0.25">"#,
            r#"<c r="A2" s="1" t="s"><v>0</v></c>"#,
            r#"<c r="B2" t="inlineStr"><is><t xml:space="preserve"> a &amp; b</t></is></c>"#,
            r#"<c r="C2"><f t="shared" ref="C2:C4" si="0">A2&amp;B2</f><v>1.0</v></c>"#,
            r#"<c r="D2"><f t="shared" si="0"/><v></v><extLst><ext uri="{x}"/></extLst></c>"#,
            r#"</row>"#
        );
        let row = CT_Row::parse(xml.as_bytes()).unwrap();
        assert_eq!(
            (row.r(), row.ht(), row.custom_height()),
            (Some(2), Some(15.0), true)
        );
        let cells: Vec<_> = row.cells().collect();
        assert_eq!(cells[0].r(), CellRef::parse("A2"));
        assert_eq!(
            (cells[0].s(), cells[0].t().as_str(), cells[0].value()),
            (1, "s", Some("0"))
        );
        assert_eq!(cells[1].inline_string().unwrap().plain_text(), " a & b");
        let formula = cells[2].formula().unwrap();
        assert_eq!(
            (formula.t().as_str(), formula.text.as_str()),
            ("shared", "A2&B2")
        );
        assert_eq!(
            (formula.si(), formula.reference()),
            (Some(0), CellRange::parse("C2:C4"))
        );
        assert_eq!(cells[3].formula().unwrap().text, "");
        let mut written = Vec::new();
        row.write(&mut written);
        let expected = xml
            .replace("<v></v>", "<v/>")
            .replace(r#"customHeight="true""#, r#"customHeight="1""#);
        assert_eq!(String::from_utf8(written).unwrap(), expected);
    }
}
