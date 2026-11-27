//! The styles part (`CT_Stylesheet`): number formats, fonts, fills, borders,
//! the cell style formats, the cell formats, the cell styles and the colour
//! palette; `dxfs`, `tableStyles` and `extLst` kept as written. Records are
//! appended at the end of their list, its `count` updated, and never removed
//! or reordered: the `s` of every cell stays valid.

use std::sync::Arc;

use crate::part::{RecordList, Shape, TypedPart};
use crate::xml::{Element, Result};

/// The children of a stylesheet, in schema order.
pub const STYLESHEET_CHILDREN: [&str; 11] = [
    "numFmts",
    "fonts",
    "fills",
    "borders",
    "cellStyleXfs",
    "cellXfs",
    "cellStyles",
    "dxfs",
    "tableStyles",
    "colors",
    "extLst",
];

const SHAPE: Shape = Shape {
    root: "styleSheet",
    order: &STYLESHEET_CHILDREN,
    lists: &[
        ("numFmts", "numFmt", true),
        ("fonts", "font", true),
        ("fills", "fill", true),
        ("borders", "border", true),
        ("cellStyleXfs", "xf", true),
        ("cellXfs", "xf", true),
        ("cellStyles", "cellStyle", true),
    ],
    singles: &["colors"],
};

/// A list of the styles part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleList {
    NumFmts,
    Fonts,
    Fills,
    Borders,
    CellStyleXfs,
    CellXfs,
    CellStyles,
}

impl StyleList {
    fn name(self) -> &'static str {
        match self {
            StyleList::NumFmts => "numFmts",
            StyleList::Fonts => "fonts",
            StyleList::Fills => "fills",
            StyleList::Borders => "borders",
            StyleList::CellStyleXfs => "cellStyleXfs",
            StyleList::CellXfs => "cellXfs",
            StyleList::CellStyles => "cellStyles",
        }
    }
}

/// The styles part. See the [module](self).
#[derive(Clone, Debug)]
pub struct StylesXml {
    part: TypedPart,
}

/// A `numFmt`: a custom number format.
#[derive(Clone, Copy, Debug)]
pub struct NumFmt<'a>(pub &'a Element);

impl NumFmt<'_> {
    pub fn id(&self) -> Option<u32> {
        self.0.tag.u32("numFmtId")
    }

    /// The format code, as stored.
    pub fn code(&self) -> String {
        self.0.tag.get("formatCode").unwrap_or_default()
    }
}

/// An `xf`: a cell format or a cell style format.
#[derive(Clone, Copy, Debug)]
pub struct Xf<'a>(pub &'a Element);

impl Xf<'_> {
    pub fn num_fmt_id(&self) -> u32 {
        self.0.tag.u32("numFmtId").unwrap_or(0)
    }

    pub fn font_id(&self) -> u32 {
        self.0.tag.u32("fontId").unwrap_or(0)
    }

    pub fn fill_id(&self) -> u32 {
        self.0.tag.u32("fillId").unwrap_or(0)
    }

    pub fn border_id(&self) -> u32 {
        self.0.tag.u32("borderId").unwrap_or(0)
    }

    /// The cell style format the cell format follows.
    pub fn xf_id(&self) -> Option<u32> {
        self.0.tag.u32("xfId")
    }

    /// `alignment`.
    pub fn alignment(&self) -> Option<&Element> {
        self.0.child("alignment")
    }

    /// `protection`.
    pub fn protection(&self) -> Option<&Element> {
        self.0.child("protection")
    }
}

/// A `cellStyle`: a named cell style.
#[derive(Clone, Copy, Debug)]
pub struct CellStyle<'a>(pub &'a Element);

impl CellStyle<'_> {
    pub fn name(&self) -> String {
        self.0.tag.get("name").unwrap_or_default()
    }

    pub fn xf_id(&self) -> u32 {
        self.0.tag.u32("xfId").unwrap_or(0)
    }

    pub fn builtin_id(&self) -> Option<u32> {
        self.0.tag.u32("builtinId")
    }
}

impl StylesXml {
    pub fn parse(source: Arc<Vec<u8>>) -> Result<Self> {
        Ok(StylesXml {
            part: TypedPart::parse(source, &SHAPE)?,
        })
    }

    /// The part's lists, single elements and source.
    pub fn part(&self) -> &TypedPart {
        &self.part
    }

    /// A list: an empty one when the part has none (Google writes no
    /// `numFmts`).
    pub fn list(&self, list: StyleList) -> &RecordList {
        self.part.list(list.name())
    }

    /// Appends `record` to `list`, written at the end of the list with its
    /// `count`, and gives its index.
    pub fn push(&mut self, list: StyleList, record: Element) -> usize {
        self.part.list_mut(list.name()).push(record)
    }

    pub fn num_fmts(&self) -> impl Iterator<Item = NumFmt<'_>> {
        self.list(StyleList::NumFmts).iter().map(NumFmt)
    }

    pub fn cell_xfs(&self) -> impl Iterator<Item = Xf<'_>> {
        self.list(StyleList::CellXfs).iter().map(Xf)
    }

    pub fn cell_style_xfs(&self) -> impl Iterator<Item = Xf<'_>> {
        self.list(StyleList::CellStyleXfs).iter().map(Xf)
    }

    pub fn cell_styles(&self) -> impl Iterator<Item = CellStyle<'_>> {
        self.list(StyleList::CellStyles).iter().map(CellStyle)
    }

    /// `colors`: the file's own `indexedColors` and `mruColors`.
    pub fn colors(&self) -> Option<&Element> {
        self.part.single("colors")
    }

    /// The part as it stands. See [`TypedPart::to_xml`].
    pub fn to_xml(&self) -> Vec<u8> {
        self.part.to_xml()
    }

    /// The prefix new elements are written with.
    pub fn prefix(&self) -> &str {
        self.part.prefix()
    }
}
