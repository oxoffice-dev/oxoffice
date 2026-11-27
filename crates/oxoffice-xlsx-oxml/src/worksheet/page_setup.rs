//! The page setup of a sheet: `sheetPr` (its `pageSetUpPr`), `printOptions`,
//! `pageMargins`, `pageSetup`, `headerFooter`, `rowBreaks` and `colBreaks`,
//! each read as written. The facade resolves them with the schema defaults.

use super::SheetIndex;
use crate::xml::{Element, Result};

/// The children of a sheet that hold its page setup.
pub const PAGE_SETUP_CHILDREN: [&str; 7] = [
    "sheetPr",
    "printOptions",
    "pageMargins",
    "pageSetup",
    "headerFooter",
    "rowBreaks",
    "colBreaks",
];

/// The page setup elements of a sheet, each when the sheet has it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageSetupElements {
    pub sheet_pr: Option<Element>,
    pub print_options: Option<Element>,
    pub page_margins: Option<Element>,
    pub page_setup: Option<Element>,
    pub header_footer: Option<Element>,
    pub row_breaks: Option<Element>,
    pub col_breaks: Option<Element>,
}

impl PageSetupElements {
    /// The page setup elements of `xml`, the sheet `index` was built from.
    pub fn read(xml: &[u8], index: &SheetIndex) -> Result<Self> {
        let read = |local: &str| -> Result<Option<Element>> {
            index
                .child(local)
                .map(|child| Element::parse(&xml[child.range.clone()]))
                .transpose()
        };
        Ok(PageSetupElements {
            sheet_pr: read("sheetPr")?,
            print_options: read("printOptions")?,
            page_margins: read("pageMargins")?,
            page_setup: read("pageSetup")?,
            header_footer: read("headerFooter")?,
            row_breaks: read("rowBreaks")?,
            col_breaks: read("colBreaks")?,
        })
    }

    /// The elements, by the local name of each.
    pub fn by_name(&self) -> [(&'static str, Option<&Element>); 7] {
        [
            ("sheetPr", self.sheet_pr.as_ref()),
            ("printOptions", self.print_options.as_ref()),
            ("pageMargins", self.page_margins.as_ref()),
            ("pageSetup", self.page_setup.as_ref()),
            ("headerFooter", self.header_footer.as_ref()),
            ("rowBreaks", self.row_breaks.as_ref()),
            ("colBreaks", self.col_breaks.as_ref()),
        ]
    }
}
