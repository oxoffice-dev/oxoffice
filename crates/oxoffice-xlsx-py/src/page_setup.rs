//! The page setup of a sheet, as a frozen snapshot.

use pyo3::prelude::*;

use oxoffice_xlsx::{PageBreak, PageSetup};

/// The margins of the printed page, in inches.
#[pyclass(
    name = "PageMargins",
    module = "oxoffice.xlsx",
    frozen,
    get_all,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq)]
pub struct PyPageMargins {
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
    header: f64,
    footer: f64,
}

/// The headers and footers, each with its `&` codes.
#[pyclass(
    name = "HeaderFooter",
    module = "oxoffice.xlsx",
    frozen,
    get_all,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq)]
pub struct PyHeaderFooter {
    odd_header: Option<String>,
    odd_footer: Option<String>,
    even_header: Option<String>,
    even_footer: Option<String>,
    first_header: Option<String>,
    first_footer: Option<String>,
    different_odd_even: bool,
    different_first: bool,
    scale_with_doc: bool,
    align_with_margins: bool,
}

/// A manual or automatic page break.
#[pyclass(
    name = "PageBreak",
    module = "oxoffice.xlsx",
    frozen,
    get_all,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq)]
pub struct PyPageBreak {
    id: u32,
    min: u32,
    max: u32,
    manual: bool,
}

fn breaks(breaks: &[PageBreak]) -> Vec<PyPageBreak> {
    breaks
        .iter()
        .map(|brk| PyPageBreak {
            id: brk.id,
            min: brk.min,
            max: brk.max,
            manual: brk.manual,
        })
        .collect()
}

/// The page setup of a sheet, schema defaults filled.
#[pyclass(
    name = "PageSetup",
    module = "oxoffice.xlsx",
    frozen,
    get_all,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq)]
pub struct PyPageSetup {
    margins: Option<PyPageMargins>,
    paper_size: u32,
    paper_width: Option<String>,
    paper_height: Option<String>,
    orientation: &'static str,
    scale: u32,
    fit_to_page: bool,
    fit_to_width: u32,
    fit_to_height: u32,
    first_page_number: u32,
    use_first_page_number: bool,
    page_order: &'static str,
    black_and_white: bool,
    draft: bool,
    cell_comments: &'static str,
    errors: &'static str,
    horizontal_dpi: u32,
    vertical_dpi: u32,
    copies: u32,
    horizontal_centered: bool,
    vertical_centered: bool,
    headings: bool,
    grid_lines: bool,
    header_footer: PyHeaderFooter,
    row_breaks: Vec<PyPageBreak>,
    column_breaks: Vec<PyPageBreak>,
    print_area: Option<String>,
    print_titles: Option<String>,
}

#[pymethods]
impl PyPageSetup {
    fn __repr__(&self) -> String {
        format!("<PageSetup paper {} {}>", self.paper_size, self.orientation)
    }
}

impl From<&PageSetup> for PyPageSetup {
    fn from(setup: &PageSetup) -> Self {
        let header_footer = &setup.header_footer;
        PyPageSetup {
            margins: setup.margins.map(|margins| PyPageMargins {
                left: margins.left,
                right: margins.right,
                top: margins.top,
                bottom: margins.bottom,
                header: margins.header,
                footer: margins.footer,
            }),
            paper_size: setup.paper_size,
            paper_width: setup.paper_width.clone(),
            paper_height: setup.paper_height.clone(),
            orientation: setup.orientation.as_str(),
            scale: setup.scale,
            fit_to_page: setup.fit_to_page,
            fit_to_width: setup.fit_to_width,
            fit_to_height: setup.fit_to_height,
            first_page_number: setup.first_page_number,
            use_first_page_number: setup.use_first_page_number,
            page_order: setup.page_order.as_str(),
            black_and_white: setup.black_and_white,
            draft: setup.draft,
            cell_comments: setup.cell_comments.as_str(),
            errors: setup.errors.as_str(),
            horizontal_dpi: setup.horizontal_dpi,
            vertical_dpi: setup.vertical_dpi,
            copies: setup.copies,
            horizontal_centered: setup.horizontal_centered,
            vertical_centered: setup.vertical_centered,
            headings: setup.headings,
            grid_lines: setup.grid_lines,
            header_footer: PyHeaderFooter {
                odd_header: header_footer.odd_header.clone(),
                odd_footer: header_footer.odd_footer.clone(),
                even_header: header_footer.even_header.clone(),
                even_footer: header_footer.even_footer.clone(),
                first_header: header_footer.first_header.clone(),
                first_footer: header_footer.first_footer.clone(),
                different_odd_even: header_footer.different_odd_even,
                different_first: header_footer.different_first,
                scale_with_doc: header_footer.scale_with_doc,
                align_with_margins: header_footer.align_with_margins,
            },
            row_breaks: breaks(&setup.row_breaks),
            column_breaks: breaks(&setup.column_breaks),
            print_area: setup.print_area.clone(),
            print_titles: setup.print_titles.clone(),
        }
    }
}

pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyPageMargins>()?;
    module.add_class::<PyHeaderFooter>()?;
    module.add_class::<PyPageBreak>()?;
    module.add_class::<PyPageSetup>()?;
    Ok(())
}
