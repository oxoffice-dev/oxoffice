#![doc = include_str!("../README.md")]

mod css;
mod emitter;
mod markdown;
mod sanitize;

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use oxoffice_docx_layout::style_resolver::{NumberingState, generate_marker};
use oxoffice_docx_layout::table::effective_table_paragraph_properties;
use oxoffice_docx_oxml::content_control::{CT_Sdt, SdtContent};
use oxoffice_docx_oxml::document::{BodyContent, CT_Body, CT_Document};
use oxoffice_docx_oxml::math::{CT_OMath, OfficeMath};
use oxoffice_docx_oxml::numbering::{CT_Lvl, CT_Numbering, ST_NumberFormat};
use oxoffice_docx_oxml::styles::{CT_Styles, StyleType};
use oxoffice_docx_oxml::table::{CT_Tbl, CT_Tc, CellContent};
use oxoffice_docx_oxml::text::CT_P;

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

/// Writes the equations of an export. The OfficeMath converters live in the
/// facade (`oxoffice-docx`), which supplies one; the functions that take no
/// writer leave equations out.
pub trait EquationWriter {
    /// One equation as a Presentation MathML `<math>` element, or `None`
    /// when it cannot be written.
    fn mathml(&self, equation: &CT_OMath) -> Option<String>;
    /// One equation as LaTeX, without delimiters, or `None` when it cannot
    /// be written.
    fn latex(&self, equation: &CT_OMath) -> Option<String>;
}

/// The writer of the functions that take none: every equation is left out.
struct NoEquations;

impl EquationWriter for NoEquations {
    fn mathml(&self, _: &CT_OMath) -> Option<String> {
        None
    }

    fn latex(&self, _: &CT_OMath) -> Option<String> {
        None
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
    to_html_document_with_equations(input, options, charts, &NoEquations)
}

/// Convert a DOCX document with current chart tables to complete HTML, its
/// equations written as MathML by `equations`.
pub fn to_html_document_with_equations(
    input: &HtmlInput,
    options: &HtmlOptions,
    charts: &HashMap<String, ChartTable>,
    equations: &dyn EquationWriter,
) -> String {
    let body = to_html_fragment_with_equations(input, options, charts, equations);
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
    to_html_fragment_with_equations(input, options, charts, &NoEquations)
}

/// Convert body content with current chart tables to an HTML fragment, its
/// equations written as MathML by `equations`.
pub fn to_html_fragment_with_equations(
    input: &HtmlInput,
    options: &HtmlOptions,
    charts: &HashMap<String, ChartTable>,
    equations: &dyn EquationWriter,
) -> String {
    emitter::emit_body(input, options, charts, equations)
}

/// Convert a DOCX document to Markdown.
pub fn to_markdown(input: &HtmlInput) -> String {
    to_markdown_with_charts(input, &HashMap::new())
}

/// Convert body content with current chart tables to Markdown.
pub fn to_markdown_with_charts(input: &HtmlInput, charts: &HashMap<String, ChartTable>) -> String {
    to_markdown_with_equations(input, charts, &NoEquations)
}

/// Convert body content with current chart tables to Markdown, its
/// equations written as LaTeX by `equations`: `$...$` inline, `$$...$$` for
/// a display equation.
pub fn to_markdown_with_equations(
    input: &HtmlInput,
    charts: &HashMap<String, ChartTable>,
    equations: &dyn EquationWriter,
) -> String {
    markdown::emit_markdown(input, charts, equations)
}

/// The equations of a paragraph at run boundary `boundary` (before run
/// `boundary`, after the last run when it is the run count).
fn equations_at(paragraph: &CT_P, boundary: usize) -> impl Iterator<Item = &OfficeMath> {
    paragraph
        .equations
        .iter()
        .filter(move |(at, _, _)| *at == boundary)
        .map(|(_, _, equation)| equation)
}

/// The list level a paragraph shows, as layout resolves it. The numbering id
/// and the level each come from the paragraph, else from its style and that
/// style's `basedOn` chain (the default paragraph style when it names none),
/// else from the document defaults; a numbering id of 0 means no list. The
/// level is the one the instance swaps in, else its definition's. Returns
/// the instance id, the level index and the level.
fn list_level<'a>(
    paragraph: &CT_P,
    styles: &CT_Styles,
    numbering: &'a CT_Numbering,
) -> Option<(u32, u32, &'a CT_Lvl)> {
    let direct = paragraph.properties.as_ref();
    let mut num_id = direct.and_then(|properties| properties.num_id);
    let mut ilvl = direct.and_then(|properties| properties.num_ilvl);
    let mut style_id = direct
        .and_then(|properties| properties.style_id.as_deref())
        .or_else(|| {
            styles
                .get_default(StyleType::Paragraph)
                .map(|style| style.style_id.as_str())
        });
    let mut seen = HashSet::new();
    while (num_id.is_none() || ilvl.is_none())
        && let Some(id) = style_id
        && seen.insert(id)
        && let Some(style) = styles.get_by_id(id)
    {
        if let Some(properties) = &style.ppr {
            num_id = num_id.or(properties.num_id);
            ilvl = ilvl.or(properties.num_ilvl);
        }
        style_id = style.based_on.as_deref();
    }
    if let Some(properties) = styles
        .doc_defaults
        .as_ref()
        .and_then(|defaults| defaults.ppr.as_ref())
    {
        num_id = num_id.or(properties.num_id);
        ilvl = ilvl.or(properties.num_ilvl);
    }
    let num_id = num_id.filter(|id| *id != 0)?;
    let ilvl = ilvl.unwrap_or(0);
    let instance = numbering.nums.iter().find(|item| item.num_id == num_id)?;
    let definition = numbering
        .abstract_nums
        .iter()
        .find(|item| item.abstract_num_id == instance.abstract_num_id)?;
    let base = definition.levels.iter().find(|item| item.ilvl == ilvl)?;
    let level_override = instance
        .level_overrides
        .iter()
        .find(|item| item.ilvl == ilvl);
    let level = level_override
        .and_then(|item| item.level.as_ref())
        .unwrap_or(base);
    Some((num_id, ilvl, level))
}

/// How a list item's marker shows in HTML: a bullet, or a number in one of
/// the formats layout draws. Legal numbering shows decimal numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListKind {
    Bullet,
    Decimal,
    LowerLetter,
    UpperLetter,
    LowerRoman,
    UpperRoman,
}

impl ListKind {
    /// The kind of a level that draws a marker: `None` for a `none` level
    /// and for the formats the exports do not write as list items.
    fn of(level: &CT_Lvl) -> Option<Self> {
        let legal = level.legal == Some(true);
        Some(match level.num_fmt.as_ref() {
            Some(ST_NumberFormat::Bullet) => Self::Bullet,
            Some(ST_NumberFormat::Decimal | ST_NumberFormat::Ordinal) | None => Self::Decimal,
            Some(ST_NumberFormat::None | ST_NumberFormat::Other(_)) => return None,
            Some(_) if legal => Self::Decimal,
            Some(ST_NumberFormat::LowerLetter) => Self::LowerLetter,
            Some(ST_NumberFormat::UpperLetter) => Self::UpperLetter,
            Some(ST_NumberFormat::LowerRoman) => Self::LowerRoman,
            Some(ST_NumberFormat::UpperRoman) => Self::UpperRoman,
            Some(_) => return None,
        })
    }

    fn ordered(self) -> bool {
        self != Self::Bullet
    }
}

/// A list item as the exports write it: its level, kind, the
/// definition that owns its count, and the number layout gives it.
#[derive(Clone, Copy, Debug)]
struct ListItem {
    level: u32,
    kind: ListKind,
    definition: u32,
    count: u32,
}

/// Counts list items in document order with layout's own counter, so that
/// the exports show the numbers a reader sees: each definition counts per
/// level for all its instances, a start override applies once, an item
/// restarts the deeper levels as their `lvlRestart` says, and a definition
/// that restarts after a section break starts again after one. Every
/// paragraph on a list counts, a heading or a paragraph on a `none` level
/// too, and so do the paragraphs of tables, which the exports write without
/// markers, as [`Self::table`] says.
struct ListCounter<'a> {
    styles: &'a CT_Styles,
    numbering: Option<&'a CT_Numbering>,
    state: NumberingState,
}

/// What [`ListCounter::paragraph`] reads on a paragraph.
enum Counted {
    /// Not on a list.
    None,
    /// On a list level that draws no marker the exports write (`none`
    /// gives `unmarked`): its level and definition.
    Unmarked {
        level: u32,
        definition: u32,
        unmarked: bool,
    },
    /// A list item.
    Item(ListItem),
}

impl<'a> ListCounter<'a> {
    fn new(styles: &'a CT_Styles, numbering: Option<&'a CT_Numbering>) -> Self {
        Self {
            styles,
            numbering,
            state: NumberingState::new(),
        }
    }

    /// Count the next paragraph in document order and say what it is.
    fn paragraph(&mut self, paragraph: &CT_P) -> Counted {
        let counted = self.count(paragraph);
        if let Some(numbering) = self.numbering
            && paragraph
                .properties
                .as_ref()
                .is_some_and(|properties| properties.sect_pr.is_some())
        {
            self.state.restart_after_section_break(numbering);
        }
        counted
    }

    /// Count the paragraphs of a table as layout lays them out: those of a
    /// table or rows that accepting changes removes, and of cells that go on
    /// a vertical merge, left out, the numbering a table style gives read.
    fn table(&mut self, table: &CT_Tbl) {
        let Some(numbering) = self.numbering else {
            return;
        };
        for (_, properties) in effective_table_paragraph_properties(table, self.styles) {
            if let Some(num_id) = properties.num_id.filter(|id| *id != 0) {
                generate_marker(
                    num_id,
                    properties.num_ilvl.unwrap_or(0),
                    numbering,
                    &mut self.state,
                );
            }
        }
    }

    fn count(&mut self, paragraph: &CT_P) -> Counted {
        let Some(numbering) = self.numbering else {
            return Counted::None;
        };
        let Some((num_id, ilvl, level)) = list_level(paragraph, self.styles, numbering) else {
            return Counted::None;
        };
        let Some(definition) = numbering
            .nums
            .iter()
            .find(|item| item.num_id == num_id)
            .map(|item| item.abstract_num_id)
        else {
            return Counted::None;
        };
        generate_marker(num_id, ilvl, numbering, &mut self.state);
        let count = self.state.current(num_id, ilvl);
        match ListKind::of(level) {
            Some(kind) => Counted::Item(ListItem {
                level: ilvl,
                kind,
                definition,
                count,
            }),
            None => Counted::Unmarked {
                level: ilvl,
                definition,
                unmarked: level.num_fmt == Some(ST_NumberFormat::None),
            },
        }
    }
}

/// The level of a paragraph whose own numbering is turned off (`numId` 0)
/// at a level, as an HTML import writes the paragraphs of a list item after
/// its first.
fn numbering_turned_off_level(paragraph: &CT_P) -> Option<u32> {
    let properties = paragraph.properties.as_ref()?;
    properties
        .num_id
        .filter(|id| *id == 0)
        .and(properties.num_ilvl)
}

/// Whether the paragraph sets a numbering id itself, not through its style.
fn has_own_numbering(paragraph: &CT_P) -> bool {
    paragraph
        .properties
        .as_ref()
        .is_some_and(|properties| properties.num_id.is_some())
}

fn paragraph_chart_ids(paragraph: &CT_P) -> Vec<String> {
    let paragraph = paragraph.accepted_view();
    paragraph
        .runs
        .iter()
        .flat_map(|run| &run.content)
        .filter_map(|content| {
            if let oxoffice_docx_oxml::text::RunContent::Drawing(drawing) = content {
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
    for (at, _, _) in &mut result.equations {
        *at += offset;
    }
    let mut equations = prefix.equations;
    equations.extend(std::mem::take(&mut result.equations));
    result.equations = equations;
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
