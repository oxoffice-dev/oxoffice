//! The workbook part (`CT_Workbook`): its date system (`workbookPr`), its
//! sheets, its defined names and its calculation properties (`calcPr`);
//! every other child kept as written. Written by splicing: a new `calcPr`, a
//! new `sheet`.

use std::sync::Arc;

use crate::namespaces::STRICT_MAIN;
use crate::part::{Shape, TypedPart};
use crate::xml::{Element, Result};

/// The children of a workbook, in schema order.
pub const WORKBOOK_CHILDREN: [&str; 19] = [
    "fileVersion",
    "fileSharing",
    "workbookPr",
    "workbookProtection",
    "bookViews",
    "sheets",
    "functionGroups",
    "externalReferences",
    "definedNames",
    "calcPr",
    "oleSize",
    "customWorkbookViews",
    "pivotCaches",
    "smartTagPr",
    "smartTagTypes",
    "webPublishing",
    "fileRecoveryPr",
    "webPublishObjects",
    "extLst",
];

const SHAPE: Shape = Shape {
    root: "workbook",
    order: &WORKBOOK_CHILDREN,
    lists: &[
        ("sheets", "sheet", false),
        ("definedNames", "definedName", false),
    ],
    singles: &["workbookPr", "calcPr"],
};

/// The workbook part. See the [module](self).
#[derive(Clone, Debug)]
pub struct WorkbookXml {
    part: TypedPart,
}

/// A `sheet` of `sheets`.
#[derive(Clone, Copy, Debug)]
pub struct SheetRecord<'a>(pub &'a Element);

impl SheetRecord<'_> {
    pub fn name(&self) -> String {
        self.0.tag.get("name").unwrap_or_default()
    }

    pub fn sheet_id(&self) -> Option<u32> {
        self.0.tag.u32("sheetId")
    }

    /// `visible` (the default), `hidden` or `veryHidden`, as written.
    pub fn state(&self) -> String {
        self.0
            .tag
            .get("state")
            .unwrap_or_else(|| "visible".to_owned())
    }

    /// The relationship id of the sheet part (`r:id`).
    pub fn relationship_id(&self) -> Option<String> {
        self.0.tag.get_prefixed("id")
    }
}

/// A `definedName` of `definedNames`.
#[derive(Clone, Copy, Debug)]
pub struct DefinedName<'a>(pub &'a Element);

impl DefinedName<'_> {
    pub fn name(&self) -> String {
        self.0.tag.get("name").unwrap_or_default()
    }

    /// The 0-based position of the sheet the name belongs to, `None` for a
    /// workbook name.
    pub fn local_sheet_id(&self) -> Option<u32> {
        self.0.tag.u32("localSheetId")
    }

    pub fn hidden(&self) -> bool {
        self.0.tag.bool("hidden").unwrap_or(false)
    }

    /// What the name refers to, as stored.
    pub fn text(&self) -> String {
        self.0.text()
    }
}

/// `calcPr`, the calculation properties.
#[derive(Clone, Copy, Debug)]
pub struct CalcPr<'a>(pub &'a Element);

impl CalcPr<'_> {
    pub fn calc_id(&self) -> Option<u32> {
        self.0.tag.u32("calcId")
    }

    /// `auto` (the default), `autoNoTable` or `manual`.
    pub fn calc_mode(&self) -> String {
        self.0
            .tag
            .get("calcMode")
            .unwrap_or_else(|| "auto".to_owned())
    }

    pub fn full_calc_on_load(&self) -> bool {
        self.0.tag.bool("fullCalcOnLoad").unwrap_or(false)
    }

    pub fn calc_on_save(&self) -> bool {
        self.0.tag.bool("calcOnSave").unwrap_or(true)
    }
}

impl WorkbookXml {
    pub fn parse(source: Arc<Vec<u8>>) -> Result<Self> {
        Ok(WorkbookXml {
            part: TypedPart::parse(source, &SHAPE)?,
        })
    }

    /// The part's lists, single elements and source.
    pub fn part(&self) -> &TypedPart {
        &self.part
    }

    /// Whether the root is in the Strict namespace.
    pub fn is_strict(&self) -> bool {
        let tag = self.part.root_tag();
        let name = tag.name();
        let prefix = name.split_once(':').map_or("", |(prefix, _)| prefix);
        tag.namespace_declarations()
            .iter()
            .any(|(bound, uri)| bound == prefix && uri == STRICT_MAIN)
    }

    /// Whether dates count from 1904 (`workbookPr date1904`).
    pub fn date1904(&self) -> bool {
        self.part
            .single("workbookPr")
            .and_then(|element| element.tag.bool("date1904"))
            .unwrap_or(false)
    }

    pub fn sheets(&self) -> impl Iterator<Item = SheetRecord<'_>> {
        self.part.list("sheets").iter().map(SheetRecord)
    }

    pub fn defined_names(&self) -> impl Iterator<Item = DefinedName<'_>> {
        self.part.list("definedNames").iter().map(DefinedName)
    }

    pub fn calc_pr(&self) -> Option<CalcPr<'_>> {
        self.part.single("calcPr").map(CalcPr)
    }

    /// Replaces `calcPr`, or writes it at its place when the part has none.
    pub fn set_calc_pr(&mut self, calc_pr: Element) {
        self.part.set_single("calcPr", calc_pr);
    }

    /// Appends a `sheet` to `sheets` and gives its position.
    pub fn push_sheet(&mut self, sheet: Element) -> usize {
        self.part.list_mut("sheets").push(sheet)
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
