//! The page setup of a sheet, resolved: what the sheet writes, the schema
//! defaults (ECMA-376 Part 1, 18.3.1) for what it leaves out. Read only in
//! this version; the page setup elements are written back as read.

use oxoffice_xlsx_oxml::Element;
use oxoffice_xlsx_oxml::workbook::WorkbookXml;
use oxoffice_xlsx_oxml::worksheet::PageSetupElements;
use oxoffice_xlsx_oxml::xstring;

/// The margins of the printed page, in inches (`pageMargins`). An attribute
/// the schema requires and the sheet leaves out reads as 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageMargins {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
    pub header: f64,
    pub footer: f64,
}

/// `pageSetup orientation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Orientation {
    /// The printer's own.
    Default,
    Portrait,
    Landscape,
}

/// `pageSetup pageOrder`: how pages follow each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PageOrder {
    DownThenOver,
    OverThenDown,
}

/// `pageSetup cellComments`: how comments print.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CellComments {
    None,
    AsDisplayed,
    AtEnd,
}

/// `pageSetup errors`: how cell errors print.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PrintErrors {
    Displayed,
    Blank,
    Dash,
    NotAvailable,
}

/// The headers and footers (`headerFooter`): each string with its `&`
/// codes, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderFooter {
    pub odd_header: Option<String>,
    pub odd_footer: Option<String>,
    pub even_header: Option<String>,
    pub even_footer: Option<String>,
    pub first_header: Option<String>,
    pub first_footer: Option<String>,
    pub different_odd_even: bool,
    pub different_first: bool,
    pub scale_with_doc: bool,
    pub align_with_margins: bool,
}

/// A page break (`brk` of `rowBreaks` or `colBreaks`), as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PageBreak {
    /// The row or column number (1-based) the break follows.
    pub id: u32,
    /// The first and last column (or row) the break spans, 0 when unset.
    pub min: u32,
    pub max: u32,
    /// Set by the user, not by the page layout.
    pub manual: bool,
}

/// The page setup of a sheet. See the [module](self).
#[derive(Clone, Debug, PartialEq)]
pub struct PageSetup {
    /// `None` when the sheet has no `pageMargins`.
    pub margins: Option<PageMargins>,
    /// The paper (`paperSize`), 1 (Letter) by default.
    pub paper_size: u32,
    /// The paper's width and height with their unit (`210mm`), when the
    /// sheet gives them instead of a size.
    pub paper_width: Option<String>,
    pub paper_height: Option<String>,
    pub orientation: Orientation,
    /// In percent, 100 by default.
    pub scale: u32,
    /// Whether the sheet is fitted to pages (`sheetPr/pageSetUpPr
    /// fitToPage`) rather than scaled.
    pub fit_to_page: bool,
    /// The pages the sheet is fitted to across and down, 0 for no limit.
    pub fit_to_width: u32,
    pub fit_to_height: u32,
    pub first_page_number: u32,
    pub use_first_page_number: bool,
    pub page_order: PageOrder,
    pub black_and_white: bool,
    pub draft: bool,
    pub cell_comments: CellComments,
    pub errors: PrintErrors,
    pub horizontal_dpi: u32,
    pub vertical_dpi: u32,
    pub copies: u32,
    /// `printOptions`.
    pub horizontal_centered: bool,
    pub vertical_centered: bool,
    pub headings: bool,
    pub grid_lines: bool,
    pub header_footer: HeaderFooter,
    pub row_breaks: Vec<PageBreak>,
    pub column_breaks: Vec<PageBreak>,
    /// What the sheet's `_xlnm.Print_Area` name refers to, as stored
    /// (`'Q1 data'!$A$1:$F$20`).
    pub print_area: Option<String>,
    /// What the sheet's `_xlnm.Print_Titles` name refers to, as stored.
    pub print_titles: Option<String>,
}

fn flag(element: Option<&Element>, name: &str, default: bool) -> bool {
    element
        .and_then(|element| element.tag.bool(name))
        .unwrap_or(default)
}

fn number(element: Option<&Element>, name: &str, default: u32) -> u32 {
    element
        .and_then(|element| element.tag.u32(name))
        .unwrap_or(default)
}

fn breaks(element: Option<&Element>) -> Vec<PageBreak> {
    let Some(element) = element else {
        return Vec::new();
    };
    element
        .elements_named("brk")
        .map(|brk| PageBreak {
            id: brk.tag.u32("id").unwrap_or(0),
            min: brk.tag.u32("min").unwrap_or(0),
            max: brk.tag.u32("max").unwrap_or(0),
            manual: brk.tag.bool("man").unwrap_or(false),
        })
        .collect()
}

impl PageSetup {
    /// The page setup of the sheet at `position`, from its elements and the
    /// workbook's print names.
    pub(crate) fn resolve(
        elements: &PageSetupElements,
        workbook: &WorkbookXml,
        position: usize,
    ) -> Self {
        let setup = elements.page_setup.as_ref();
        let options = elements.print_options.as_ref();
        let header_footer = elements.header_footer.as_ref();
        let text = |local: &str| {
            header_footer
                .and_then(|element| element.child(local))
                .map(|element| xstring::decode(&element.text()).into_owned())
        };
        let print_name = |name: &str| {
            workbook
                .defined_names()
                .find(|defined| {
                    defined.local_sheet_id() == u32::try_from(position).ok()
                        && defined.name().eq_ignore_ascii_case(name)
                })
                .map(|defined| defined.text())
        };
        let attribute = |name: &str| setup.and_then(|element| element.tag.get(name));
        PageSetup {
            margins: elements.page_margins.as_ref().map(|margins| {
                let side = |name: &str| margins.tag.f64(name).unwrap_or(0.0);
                PageMargins {
                    left: side("left"),
                    right: side("right"),
                    top: side("top"),
                    bottom: side("bottom"),
                    header: side("header"),
                    footer: side("footer"),
                }
            }),
            paper_size: number(setup, "paperSize", 1),
            paper_width: attribute("paperWidth"),
            paper_height: attribute("paperHeight"),
            orientation: match attribute("orientation").as_deref() {
                Some("portrait") => Orientation::Portrait,
                Some("landscape") => Orientation::Landscape,
                _ => Orientation::Default,
            },
            scale: number(setup, "scale", 100),
            fit_to_page: flag(
                elements
                    .sheet_pr
                    .as_ref()
                    .and_then(|sheet_pr| sheet_pr.child("pageSetUpPr")),
                "fitToPage",
                false,
            ),
            fit_to_width: number(setup, "fitToWidth", 1),
            fit_to_height: number(setup, "fitToHeight", 1),
            first_page_number: number(setup, "firstPageNumber", 1),
            use_first_page_number: flag(setup, "useFirstPageNumber", false),
            page_order: match attribute("pageOrder").as_deref() {
                Some("overThenDown") => PageOrder::OverThenDown,
                _ => PageOrder::DownThenOver,
            },
            black_and_white: flag(setup, "blackAndWhite", false),
            draft: flag(setup, "draft", false),
            cell_comments: match attribute("cellComments").as_deref() {
                Some("asDisplayed") => CellComments::AsDisplayed,
                Some("atEnd") => CellComments::AtEnd,
                _ => CellComments::None,
            },
            errors: match attribute("errors").as_deref() {
                Some("blank") => PrintErrors::Blank,
                Some("dash") => PrintErrors::Dash,
                Some("NA") => PrintErrors::NotAvailable,
                _ => PrintErrors::Displayed,
            },
            horizontal_dpi: number(setup, "horizontalDpi", 600),
            vertical_dpi: number(setup, "verticalDpi", 600),
            copies: number(setup, "copies", 1),
            horizontal_centered: flag(options, "horizontalCentered", false),
            vertical_centered: flag(options, "verticalCentered", false),
            headings: flag(options, "headings", false),
            grid_lines: flag(options, "gridLines", false),
            header_footer: HeaderFooter {
                odd_header: text("oddHeader"),
                odd_footer: text("oddFooter"),
                even_header: text("evenHeader"),
                even_footer: text("evenFooter"),
                first_header: text("firstHeader"),
                first_footer: text("firstFooter"),
                different_odd_even: flag(header_footer, "differentOddEven", false),
                different_first: flag(header_footer, "differentFirst", false),
                scale_with_doc: flag(header_footer, "scaleWithDoc", true),
                align_with_margins: flag(header_footer, "alignWithMargins", true),
            },
            row_breaks: breaks(elements.row_breaks.as_ref()),
            column_breaks: breaks(elements.col_breaks.as_ref()),
            print_area: print_name("_xlnm.Print_Area"),
            print_titles: print_name("_xlnm.Print_Titles"),
        }
    }
}
