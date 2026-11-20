#![doc = include_str!("../README.md")]

mod css;
mod emitter;
mod markdown;
mod sanitize;

use std::borrow::Cow;
use std::collections::HashMap;

use rdocx_oxml::content_control::{CT_Sdt, SdtContent};
use rdocx_oxml::document::{BodyContent, CT_Body, CT_Document};
use rdocx_oxml::numbering::CT_Numbering;
use rdocx_oxml::styles::CT_Styles;
use rdocx_oxml::table::{CT_Tbl, CT_Tc, CellContent};
use rdocx_oxml::text::CT_P;

/// Options for HTML conversion.
#[derive(Debug, Clone)]
pub struct HtmlOptions {
    /// Whether to inline images as base64 data URIs (default: true).
    pub inline_images: bool,
}

impl Default for HtmlOptions {
    fn default() -> Self {
        Self {
            inline_images: true,
        }
    }
}

/// Input for HTML conversion.
pub struct HtmlInput {
    pub document: CT_Document,
    pub styles: CT_Styles,
    pub numbering: Option<CT_Numbering>,
    /// Images keyed by embed/relationship ID.
    pub images: HashMap<String, ImageData>,
    /// Hyperlink URLs keyed by relationship ID.
    pub hyperlink_urls: HashMap<String, String>,
}

/// Image data for HTML embedding.
pub struct ImageData {
    pub data: Vec<u8>,
    pub content_type: String,
}

/// A semantic chart projection from the current chart part.
pub struct ChartTable {
    pub title: Option<String>,
    pub categories: Vec<String>,
    pub series: Vec<(String, Vec<f64>)>,
}

impl ChartTable {
    /// Header followed by category rows, with absent or non-finite values blank.
    pub fn rows(&self) -> Vec<Vec<String>> {
        let mut header = vec![String::new()];
        header.extend(self.series.iter().map(|(name, _)| name.clone()));
        let count = self
            .series
            .iter()
            .map(|(_, values)| values.len())
            .chain(std::iter::once(self.categories.len()))
            .max()
            .unwrap_or(0);
        let mut rows = vec![header];
        for index in 0..count {
            let mut row = vec![self.categories.get(index).cloned().unwrap_or_default()];
            row.extend(self.series.iter().map(|(_, values)| {
                values
                    .get(index)
                    .filter(|value| value.is_finite())
                    .map(ToString::to_string)
                    .unwrap_or_default()
            }));
            rows.push(row);
        }
        rows
    }
}

/// Convert a DOCX document to a complete HTML document string.
pub fn to_html_document(input: &HtmlInput, options: &HtmlOptions) -> String {
    to_html_document_with_charts(input, options, &HashMap::new())
}

/// Convert a DOCX document with current chart tables to complete HTML.
pub fn to_html_document_with_charts(
    input: &HtmlInput,
    options: &HtmlOptions,
    charts: &HashMap<String, ChartTable>,
) -> String {
    let body = to_html_fragment_with_charts(input, options, charts);
    let css = css::generate_base_css();
    format!(
        "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"UTF-8\">\n<style>\n{css}\n</style>\n</head>\n<body>\n{body}\n</body>\n</html>"
    )
}

/// Convert a DOCX document to an HTML fragment (body content only).
pub fn to_html_fragment(input: &HtmlInput, options: &HtmlOptions) -> String {
    to_html_fragment_with_charts(input, options, &HashMap::new())
}

/// Convert body content with current chart tables to an HTML fragment.
pub fn to_html_fragment_with_charts(
    input: &HtmlInput,
    options: &HtmlOptions,
    charts: &HashMap<String, ChartTable>,
) -> String {
    emitter::emit_body(
        &input.document.body,
        &input.styles,
        input.numbering.as_ref(),
        &input.images,
        &input.hyperlink_urls,
        options,
        charts,
    )
}

/// Convert a DOCX document to Markdown.
pub fn to_markdown(input: &HtmlInput) -> String {
    to_markdown_with_charts(input, &HashMap::new())
}

/// Convert body content with current chart tables to Markdown.
pub fn to_markdown_with_charts(input: &HtmlInput, charts: &HashMap<String, ChartTable>) -> String {
    markdown::emit_markdown(input, charts)
}

fn paragraph_chart_ids(paragraph: &CT_P) -> Vec<String> {
    let paragraph = paragraph.accepted_view();
    paragraph
        .runs
        .iter()
        .flat_map(|run| &run.content)
        .filter_map(|content| {
            if let rdocx_oxml::text::RunContent::Drawing(drawing) = content {
                Some(drawing)
            } else {
                None
            }
        })
        .flat_map(|drawing| {
            drawing
                .inline
                .as_ref()
                .and_then(|inline| inline.chart_rel_id.as_ref())
                .into_iter()
                .chain(
                    drawing
                        .anchor
                        .as_ref()
                        .and_then(|anchor| anchor.chart_rel_id.as_ref()),
                )
        })
        .cloned()
        .collect()
}

/// A paragraph or a table that the emitters write as one block.
enum Block<'a> {
    Paragraph(Box<Cow<'a, CT_P>>, Vec<usize>),
    Table(&'a CT_Tbl),
}

/// The paragraphs and tables of a body in document order. A block content
/// control is transparent: what it wraps, nested controls included, is
/// written as if the control were not there, which is what `Paragraph.text`
/// and `Document::text` read.
fn body_blocks(body: &CT_Body, preserve_field_owners: bool) -> Vec<Block<'_>> {
    let mut blocks = Vec::new();
    let mut carried: Option<(CT_P, Vec<usize>)> = None;
    for (index, item) in body.content.iter().enumerate() {
        match item {
            BodyContent::Paragraph(paragraph) => {
                let (paragraph, boundaries) = match carried.take() {
                    Some(prefix) => {
                        let (joined, boundaries) =
                            join_accepted_paragraphs(prefix, paragraph, preserve_field_owners);
                        (Cow::Owned(joined), boundaries)
                    }
                    None => (Cow::Borrowed(paragraph.as_ref()), Vec::new()),
                };
                if body.accepted_paragraph_joins_next(index) {
                    carried = Some(if preserve_field_owners {
                        let (view, mut projected) = paragraph.accepted_view_with_field_owners();
                        projected.extend(boundaries);
                        (view.into_owned(), projected)
                    } else {
                        (paragraph.accepted_view().into_owned(), Vec::new())
                    });
                } else {
                    blocks.push(Block::Paragraph(Box::new(paragraph), boundaries));
                }
            }
            BodyContent::Table(table) => blocks.push(Block::Table(table.as_ref())),
            BodyContent::ContentControl(control) => push_control_blocks(control, &mut blocks),
            BodyContent::RawXml(_) => {}
        }
    }
    blocks
}

fn join_accepted_paragraphs(
    prefix: (CT_P, Vec<usize>),
    paragraph: &CT_P,
    preserve_field_owners: bool,
) -> (CT_P, Vec<usize>) {
    let (prefix, mut boundaries) = prefix;
    let (mut result, projected) = if preserve_field_owners {
        let (view, boundaries) = paragraph.accepted_view_with_field_owners();
        (view.into_owned(), boundaries)
    } else {
        (paragraph.accepted_view().into_owned(), Vec::new())
    };
    let offset = prefix.runs.len();
    boundaries.extend(projected.into_iter().map(|at| offset + at));
    for hyperlink in &mut result.hyperlinks {
        hyperlink.run_start += offset;
        hyperlink.run_end += offset;
    }
    let mut hyperlinks = prefix.hyperlinks;
    hyperlinks.extend(result.hyperlinks);
    result.hyperlinks = hyperlinks;
    let mut runs = prefix.runs;
    runs.extend(result.runs);
    result.runs = runs;
    (result, boundaries)
}

/// The paragraphs and nested tables of a table cell, as [`body_blocks`]
/// reads a body.
fn cell_blocks(cell: &CT_Tc) -> Vec<Block<'_>> {
    let mut blocks = Vec::new();
    for item in &cell.content {
        match item {
            CellContent::Paragraph(paragraph) => blocks.push(Block::Paragraph(
                Box::new(Cow::Borrowed(paragraph)),
                Vec::new(),
            )),
            CellContent::Table(table) => blocks.push(Block::Table(table)),
            CellContent::ContentControl(control) => push_control_blocks(control, &mut blocks),
        }
    }
    blocks
}

fn push_control_blocks<'a>(control: &'a CT_Sdt, blocks: &mut Vec<Block<'a>>) {
    for item in &control.content {
        match item {
            SdtContent::Paragraph(paragraph) => blocks.push(Block::Paragraph(
                Box::new(Cow::Borrowed(paragraph)),
                Vec::new(),
            )),
            SdtContent::Table(table) => blocks.push(Block::Table(table)),
            SdtContent::ContentControl(nested) => push_control_blocks(nested, blocks),
            // A control around a paragraph or a table holds no rows, cells or
            // runs of its own. Those around rows, cells and runs are read
            // through `CT_Tbl::rows`, `CT_Row::cells` and `CT_P::accepted_view`.
            SdtContent::Row(_) | SdtContent::Cell(_) | SdtContent::Run(_) => {}
            SdtContent::RawXml(_) => {}
        }
    }
}
