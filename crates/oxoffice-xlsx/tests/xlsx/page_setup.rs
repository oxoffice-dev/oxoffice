//! The page setup reads what the sheet writes, the schema defaults for what
//! it leaves out, and the sheet's print area and titles.

use std::io::Cursor;

use oxoffice_opc::OpcPackage;
use oxoffice_xlsx::{
    CellComments, HeaderFooter, Orientation, PageBreak, PageMargins, PageOrder, PageSetup,
    PrintErrors, Workbook,
};

use crate::support::corpus_bytes;

fn page_setup(bytes: &[u8], sheet: &str) -> PageSetup {
    let workbook = Workbook::from_bytes(bytes).unwrap();
    workbook.sheet(sheet).unwrap().page_setup().unwrap()
}

/// What a sheet without page setup elements reads: the schema defaults.
fn defaults() -> PageSetup {
    PageSetup {
        margins: None,
        paper_size: 1,
        paper_width: None,
        paper_height: None,
        orientation: Orientation::Default,
        scale: 100,
        fit_to_page: false,
        fit_to_width: 1,
        fit_to_height: 1,
        first_page_number: 1,
        use_first_page_number: false,
        page_order: PageOrder::DownThenOver,
        black_and_white: false,
        draft: false,
        cell_comments: CellComments::None,
        errors: PrintErrors::Displayed,
        horizontal_dpi: 600,
        vertical_dpi: 600,
        copies: 1,
        horizontal_centered: false,
        vertical_centered: false,
        headings: false,
        grid_lines: false,
        header_footer: HeaderFooter {
            odd_header: None,
            odd_footer: None,
            even_header: None,
            even_footer: None,
            first_header: None,
            first_footer: None,
            different_odd_even: false,
            different_first: false,
            scale_with_doc: true,
            align_with_margins: true,
        },
        row_breaks: Vec::new(),
        column_breaks: Vec::new(),
        print_area: None,
        print_titles: None,
    }
}

/// `budget`'s print setup, as XlsxWriter, Excel and LibreOffice write it.
fn budget() -> PageSetup {
    let mut expected = defaults();
    expected.margins = Some(PageMargins {
        left: 0.5,
        right: 0.5,
        top: 0.75,
        bottom: 0.75,
        header: 0.3,
        footer: 0.3,
    });
    expected.paper_size = 9;
    expected.orientation = Orientation::Landscape;
    expected.fit_to_page = true;
    expected.fit_to_height = 0;
    expected.horizontal_centered = true;
    expected.header_footer.odd_header =
        Some("&L&\"Calibri,Bold\"Operating budget 2027&RConfidential".to_owned());
    expected.header_footer.odd_footer = Some("&LPrepared by Finance&CPage &P of &N".to_owned());
    expected.print_area = Some("Budget!$A$1:$J$36".to_owned());
    expected.print_titles = Some("Budget!$3:$3".to_owned());
    expected
}

/// `bytes` with `xml` written before the end of sheet `part`'s root.
fn with_breaks(bytes: &[u8], part: &str, xml: &str) -> Vec<u8> {
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let sheet = String::from_utf8(package.get_part(part).unwrap().to_vec()).unwrap();
    let at = sheet.find("<drawing").unwrap();
    package.set_part(
        part,
        format!("{}{xml}{}", &sheet[..at], &sheet[at..]).into_bytes(),
    );
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    out.into_inner()
}

#[test]
fn the_page_setup_reads_as_written_with_the_schema_defaults() {
    let mut libreoffice = budget();
    (libreoffice.horizontal_dpi, libreoffice.vertical_dpi) = (300, 300);
    let mut broken = budget();
    broken.row_breaks = vec![PageBreak {
        id: 20,
        min: 0,
        max: 16_383,
        manual: true,
    }];
    let breaks = with_breaks(
        &corpus_bytes("excel/budget.xlsx"),
        "/xl/worksheets/sheet1.xml",
        r#"<rowBreaks count="1" manualBreakCount="1"><brk id="20" max="16383" man="1"/></rowBreaks>"#,
    );
    let cases = [
        (
            "authored/budget.xlsx",
            corpus_bytes("authored/budget.xlsx"),
            "Budget",
            budget(),
        ),
        (
            "excel/budget.xlsx",
            corpus_bytes("excel/budget.xlsx"),
            "budget",
            budget(),
        ),
        (
            "libreoffice/budget.xlsx",
            corpus_bytes("libreoffice/budget.xlsx"),
            "BUDGET",
            libreoffice,
        ),
        ("a manual row break", breaks, "Budget", broken),
        (
            "authored/multi.xlsx",
            corpus_bytes("authored/multi.xlsx"),
            "Q1 Forecast",
            PageSetup {
                margins: Some(PageMargins {
                    left: 0.7,
                    right: 0.7,
                    top: 0.75,
                    bottom: 0.75,
                    header: 0.3,
                    footer: 0.3,
                }),
                ..defaults()
            },
        ),
    ];
    for (case, bytes, sheet, expected) in cases {
        assert_eq!(page_setup(&bytes, sheet), expected, "{case}");
    }
}
