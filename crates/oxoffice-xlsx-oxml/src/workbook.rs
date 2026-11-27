//! The workbook part (`CT_Workbook`): its date system (`workbookPr`), its
//! sheets, its defined names and its calculation properties (`calcPr`);
//! every other child kept as written. Written by splicing: a new `calcPr`, a
//! new `sheet`.

use std::sync::Arc;

use crate::namespaces::{RELATIONSHIPS, STRICT_MAIN, is_relationships_namespace, strict};
use crate::part::{Shape, TypedPart};
use crate::xml::{Element, Result, qualified};
use crate::xstring;

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
    /// The name, as its tab shows it: `_xHHHH_` escapes read.
    pub fn name(&self) -> String {
        xstring::decode(&self.0.tag.get("name").unwrap_or_default()).into_owned()
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

    /// The `sheetId` a new sheet takes: one more than the largest there is.
    pub fn next_sheet_id(&self) -> u32 {
        self.sheets()
            .filter_map(|sheet| sheet.sheet_id())
            .max()
            .map_or(1, |largest| largest.saturating_add(1))
    }

    /// A `sheet` record for `sheets`, written as the part writes its own: the
    /// elements under the prefix the root binds to SpreadsheetML, `r:id`
    /// under the one it binds to the relationships namespace (declared on
    /// the record when the root binds none). `name` is the text as stored,
    /// its `_xHHHH_` escapes written.
    pub fn sheet_record(&self, name: &str, sheet_id: u32, relationship_id: &str) -> Element {
        let mut record = Element::new(&qualified(self.part.prefix(), "sheet"));
        record.tag.set("name", Some(name));
        record.tag.set("sheetId", Some(&sheet_id.to_string()));
        let bound = self
            .part
            .root_tag()
            .namespace_declarations()
            .into_iter()
            .find(|(prefix, uri)| !prefix.is_empty() && is_relationships_namespace(uri))
            .map(|(prefix, _)| prefix);
        let prefix = bound.unwrap_or_else(|| {
            let uri = if self.is_strict() {
                strict(RELATIONSHIPS)
            } else {
                RELATIONSHIPS
            };
            record.tag.set("xmlns:r", Some(uri));
            "r".to_owned()
        });
        record
            .tag
            .set(&qualified(&prefix, "id"), Some(relationship_id));
        record
    }

    /// Asks Excel to calculate the workbook when it opens it
    /// (`calcPr fullCalcOnLoad`): sets it on the `calcPr` there is, else
    /// writes one. Nothing changes when the workbook asks already.
    pub fn set_full_calc_on_load(&mut self) {
        if self.calc_pr().is_some_and(|calc| calc.full_calc_on_load()) {
            return;
        }
        let mut element = match self.part.single("calcPr") {
            Some(element) => element.clone(),
            None => Element::new(&qualified(self.part.prefix(), "calcPr")),
        };
        element.tag.set_bool("fullCalcOnLoad", Some(true));
        self.set_calc_pr(element);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_sheet_takes_the_next_id_and_the_prefixes_of_the_root() {
        const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
        const RELATIONSHIPS: &str =
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
        let cases = [
            (
                "a default namespace and r",
                format!(
                    r#"<workbook xmlns="{MAIN}" xmlns:r="{RELATIONSHIPS}"><sheets><sheet name="A" sheetId="3" r:id="rId1"/><sheet name="B" sheetId="2" r:id="rId2"/></sheets></workbook>"#
                ),
                r#"<sheet name="C_x005F_x0041_" sheetId="4" r:id="rId9"/>"#.to_owned(),
            ),
            (
                "a prefix and no prefix for the relationships: declared on the record",
                format!(r#"<x:workbook xmlns:x="{MAIN}"><x:sheets/></x:workbook>"#),
                format!(
                    r#"<x:sheet name="C_x005F_x0041_" sheetId="1" xmlns:r="{RELATIONSHIPS}" r:id="rId9"/>"#
                ),
            ),
            (
                "Strict, the relationships under another prefix",
                r#"<ns0:workbook xmlns:ns0="http://purl.oclc.org/ooxml/spreadsheetml/main" xmlns:ns1="http://purl.oclc.org/ooxml/officeDocument/relationships"><ns0:sheets><ns0:sheet name="A" sheetId="7" ns1:id="rId1"/></ns0:sheets></ns0:workbook>"#.to_owned(),
                r#"<ns0:sheet name="C_x005F_x0041_" sheetId="8" ns1:id="rId9"/>"#.to_owned(),
            ),
            (
                "Strict, no prefix for the relationships",
                r#"<workbook xmlns="http://purl.oclc.org/ooxml/spreadsheetml/main"><sheets/></workbook>"#.to_owned(),
                r#"<sheet name="C_x005F_x0041_" sheetId="1" xmlns:r="http://purl.oclc.org/ooxml/officeDocument/relationships" r:id="rId9"/>"#.to_owned(),
            ),
        ];
        for (case, xml, expected) in cases {
            let workbook = WorkbookXml::parse(Arc::new(xml.into_bytes())).unwrap();
            let record = workbook.sheet_record("C_x005F_x0041_", workbook.next_sheet_id(), "rId9");
            assert_eq!(
                String::from_utf8(record.to_xml()).unwrap(),
                expected,
                "{case}"
            );
        }
    }

    #[test]
    fn a_workbook_that_asks_for_a_calculation_is_written_with_it_once() {
        let part = |calc: &str| {
            format!(
                r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheets/>{calc}</workbook>"#
            )
        };
        for (case, before, expected) in [
            ("none", part(""), part(r#"<calcPr fullCalcOnLoad="1"/>"#)),
            (
                "empty, as Google writes it",
                part("<calcPr/>"),
                part(r#"<calcPr fullCalcOnLoad="1"/>"#),
            ),
            (
                "Excel's, its other attributes kept",
                part(r#"<calcPr calcId="191029" fullCalcOnLoad="false"/>"#),
                part(r#"<calcPr calcId="191029" fullCalcOnLoad="1"/>"#),
            ),
            (
                "asked already: not written",
                part(r#"<calcPr fullCalcOnLoad="true"/>"#),
                part(r#"<calcPr fullCalcOnLoad="true"/>"#),
            ),
        ] {
            let mut workbook = WorkbookXml::parse(Arc::new(before.into_bytes())).unwrap();
            workbook.set_full_calc_on_load();
            assert_eq!(
                String::from_utf8(workbook.to_xml()).unwrap(),
                expected,
                "{case}"
            );
        }
    }

    #[test]
    fn a_sheet_name_reads_its_escapes() {
        let xml = concat!(
            r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="urn:r">"#,
            r#"<sheets><sheet name="Q_x005F_x0031_ data" sheetId="1" r:id="rId1"/></sheets></workbook>"#
        );
        let workbook = WorkbookXml::parse(Arc::new(xml.as_bytes().to_vec())).unwrap();
        let names: Vec<_> = workbook.sheets().map(|sheet| sheet.name()).collect();
        assert_eq!(names, ["Q_x0031_ data"]);
    }
}
