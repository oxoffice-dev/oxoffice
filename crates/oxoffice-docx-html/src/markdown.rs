//! Markdown generation from OXML types (GFM-compatible).

use std::cmp::Reverse;
use std::collections::HashMap;

use oxoffice_docx_oxml::math::OfficeMath;
use oxoffice_docx_oxml::properties::CT_PPr;
use oxoffice_docx_oxml::styles::CT_Styles;
use oxoffice_docx_oxml::table::CT_Tbl;
use oxoffice_docx_oxml::text::{BreakType, CT_P, CT_R, RunContent, SpecialCharacter};

use crate::{
    Block, Counted, EquationWriter, HtmlInput, ImageData, ListCounter, body_blocks, cell_blocks,
};

/// Emit the full body content as Markdown.
pub(crate) fn emit_markdown(
    input: &HtmlInput,
    charts: &HashMap<String, crate::ChartTable>,
    equations: &dyn EquationWriter,
) -> String {
    let styles = &input.styles;
    let hyperlink_urls = &crate::Links::new(&input.hyperlink_urls, &input.document.body);
    let images = &input.images;
    let mut out = String::new();
    // The open list levels, each with the column its item text starts at.
    let mut list_levels: Vec<MarkdownList> = Vec::new();
    let mut counter = ListCounter::new(styles, input.numbering.as_ref());

    let mut previous_quote_owner = None;
    for block in body_blocks(&input.document.body, true) {
        let style_id = match &block {
            Block::Paragraph(p, _) => p.properties.as_ref().and_then(|p| p.style_id.as_deref()),
            Block::Table(t) => t.properties.as_ref().and_then(|p| p.style_id.as_deref()),
        };
        let owner = quote_owner(style_id, styles);
        let shared_depth =
            owner
                .zip(previous_quote_owner)
                .map_or(0, |(current, previous): (&str, &str)| {
                    current
                        .split('.')
                        .zip(previous.split('.'))
                        .take_while(|(a, b)| a == b)
                        .count()
                });
        if shared_depth > 0 && out.ends_with("\n\n") {
            out.pop();
            let path = container_path(style_id, styles).unwrap_or_default();
            if !path.starts_with('q')
                && let Some(MarkdownList { column, .. }) = list_levels.last()
            {
                out.push_str(&" ".repeat(*column));
            }
            out.push_str(&"> ".repeat(shared_depth));
            out.push('\n');
        }
        if previous_quote_owner.is_some() && owner != previous_quote_owner && shared_depth == 0 {
            leave_list(&mut out, &mut list_levels);
            if !out.ends_with("\n\n") {
                out.push('\n');
            }
        }
        previous_quote_owner = owner;
        match block {
            Block::Paragraph(p, boundaries) => {
                let counted = counter.paragraph(&p);
                emit_paragraph(
                    &mut out,
                    &p,
                    &boundaries,
                    input,
                    (equations, hyperlink_urls),
                    counted,
                    &mut list_levels,
                );
                for id in crate::paragraph_chart_ids(&p) {
                    if let Some(chart) = charts.get(&id) {
                        leave_list(&mut out, &mut list_levels);
                        emit_chart_table(&mut out, chart);
                    }
                }
            }
            Block::Table(tbl) => {
                counter.table(tbl);
                let mut table = String::new();
                emit_table(&mut table, tbl, styles, hyperlink_urls, images, equations);
                if !table.is_empty() {
                    let path = container_path(
                        tbl.properties.as_ref().and_then(|p| p.style_id.as_deref()),
                        styles,
                    )
                    .unwrap_or_default();
                    let outer_prefix = "> ".repeat(path.chars().take_while(|&c| c == 'q').count());
                    if path.contains('q') {
                        let prefix = "> ".repeat(path.chars().filter(|&c| c == 'q').count());
                        table = table
                            .lines()
                            .map(|line| {
                                if line.is_empty() {
                                    String::new()
                                } else {
                                    format!("{prefix}{line}")
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                            + "\n";
                    }
                    if tbl
                        .properties
                        .as_ref()
                        .and_then(|properties| properties.indent.as_ref())
                        .is_some_and(|indent| indent.width_type == "dxa" && indent.w > 0)
                        && let Some(MarkdownList { column, .. }) = list_levels.last()
                    {
                        if !out.ends_with("\n\n") {
                            out.push_str(&outer_prefix);
                            out.push('\n');
                        }
                        for line in table.lines() {
                            let line = line.strip_prefix(&outer_prefix).unwrap_or(line);
                            out.push_str(&format!("{outer_prefix}{}{line}\n", " ".repeat(*column)));
                        }
                        out.push_str(&outer_prefix);
                        out.push('\n');
                    } else {
                        leave_quoted_list(&mut out, &mut list_levels, &outer_prefix);
                        out.push_str(&table);
                    }
                }
            }
        }
    }

    out
}

/// End an open list before another kind of block.
///
/// A list item ends in a single newline, and a block that follows it without
/// a blank line is read as a lazy continuation of the last item.
fn leave_list(out: &mut String, list_levels: &mut Vec<MarkdownList>) {
    if !list_levels.is_empty() && !out.ends_with("\n\n") {
        out.push('\n');
    }
    list_levels.clear();
}

fn leave_quoted_list(out: &mut String, list_levels: &mut Vec<MarkdownList>, prefix: &str) {
    if !list_levels.is_empty() && !out.ends_with("\n\n") {
        out.push_str(prefix);
        out.push('\n');
    }
    list_levels.clear();
}

/// An open Markdown list at one level.
struct MarkdownList {
    /// The level of its items.
    level: u32,
    /// The column its item text starts at.
    column: usize,
    /// The definition that counts its items.
    definition: u32,
    ordered: bool,
    /// The number of its last item.
    last: u32,
}

/// Detect heading level from paragraph properties/style.
fn detect_heading_level(ppr: Option<&CT_PPr>, styles: &CT_Styles) -> Option<u32> {
    let ppr = ppr?;

    if let Some(lvl) = ppr.outline_lvl {
        return Some(lvl + 1);
    }

    let style_id = ppr.style_id.as_deref()?;
    if let Some(level) = style_id.strip_prefix("Heading")
        && let Ok(n) = level.parse::<u32>()
        && (1..=6).contains(&n)
    {
        return Some(n);
    }

    if let Some(style) = styles.get_by_id(style_id)
        && let Some(spr) = &style.ppr
        && let Some(lvl) = spr.outline_lvl
    {
        return Some(lvl + 1);
    }

    None
}

/// Emit a paragraph as Markdown.
fn emit_paragraph(
    out: &mut String,
    para: &CT_P,
    boundaries: &[usize],
    input: &HtmlInput,
    // The equation writer and the link targets.
    (equations, hyperlink_urls): (&dyn EquationWriter, &crate::Links<'_>),
    counted: Counted,
    list_levels: &mut Vec<MarkdownList>,
) {
    let styles = &input.styles;
    let images = &input.images;
    let detected_level = detect_heading_level(para.properties.as_ref(), styles);
    // A heading numbered through its style, as Word numbers its headings,
    // stays a heading; only its own numbering makes it a list item.
    let list_info = match counted {
        Counted::Item(item) if detected_level.is_none() || crate::has_own_numbering(para) => {
            Some(item)
        }
        _ => None,
    };
    // Outline level 9 (level 10 here) is body text, the level of Word's TOC
    // Heading style: written as a paragraph, which, as in HTML, takes no
    // numbering from its style either.
    let mut heading_level = detected_level.filter(|&level| level < 10);

    // Collect inline text for the paragraph
    let mut text =
        collect_paragraph_text(para, styles, hyperlink_urls, images, boundaries, equations);

    let style_id = para
        .properties
        .as_ref()
        .and_then(|properties| properties.style_id.as_deref());
    let compound_quote = quote_owner(style_id, styles).is_some();
    let container_path = container_path(style_id, styles).unwrap_or_default();
    let outer_quotes = container_path.chars().take_while(|&c| c == 'q').count();
    let style_id = style_id.and_then(|id| {
        styles
            .get_by_id(id)
            .filter(|_| !container_path.is_empty())
            .and_then(|style| style.based_on.as_deref())
            .or(Some(id))
    });
    let style_name = style_id
        .and_then(|id| styles.get_by_id(id))
        .and_then(|style| style.name.as_deref());
    let role = |id: &str, name: &str| {
        style_id == Some(id)
            || style_name.is_some_and(|existing| existing.eq_ignore_ascii_case(name))
    };
    let quoted_pre = role("HTMLQuotedPreformatted", "HTML Quoted Preformatted");
    if role("HTMLThematicBreak", "HTML Thematic Break") {
        text = "---".to_owned();
    } else if role("HTMLPreformatted", "HTML Preformatted") || quoted_pre {
        let code = para.accepted_text();
        let code = code.strip_suffix('\n').unwrap_or(&code);
        let length = code
            .split(|character| character != '`')
            .map(str::len)
            .max()
            .unwrap_or(0);
        let fence = "`".repeat((length + 1).max(3));
        text = format!("{fence}\n{code}\n{fence}");
    } else if heading_level.is_none() && list_info.is_none() {
        text = keep_leading_whitespace(&text);
    }
    let inner_quotes = container_path
        .rsplit('l')
        .next()
        .unwrap_or("")
        .chars()
        .filter(|&c| c == 'q')
        .count()
        .saturating_sub(if !container_path.contains('l') {
            outer_quotes
        } else {
            0
        });
    let item_quotes =
        container_path.chars().filter(|&c| c == 'q').count() - outer_quotes - inner_quotes;
    let item_quote_prefix = "> ".repeat(item_quotes);
    if inner_quotes > 0 || ((role("Quote", "Quote") || quoted_pre) && container_path.is_empty()) {
        if let Some(level) = heading_level.take() {
            text = format!("{} {}", "#".repeat(level.min(6) as usize), text.trim());
        }
        let depth = para
            .properties
            .as_ref()
            .and_then(|properties| properties.ind_left)
            .map_or(1, |indent| (indent.0 / 360).clamp(1, 256) as usize);
        let prefix = "> ".repeat(if container_path.is_empty() {
            depth
        } else {
            inner_quotes
        });
        text = text
            .split('\n')
            .map(|line| format!("{prefix}{line}"))
            .collect::<Vec<_>>()
            .join("\n");
    }

    if let Some(level) = heading_level
        && (list_info.is_some()
            || para
                .properties
                .as_ref()
                .is_some_and(|p| p.num_id == Some(0)))
    {
        text = format!(
            "{} {}",
            "#".repeat(level.min(6) as usize),
            text.trim().replace(LINE_BREAK, "<br>")
        );
        heading_level = None;
    }
    let quote_prefix = "> ".repeat(outer_quotes);

    let continued = if detected_level.is_none()
        && para
            .properties
            .as_ref()
            .is_some_and(|properties| properties.num_id == Some(0))
    {
        crate::numbering_turned_off_level(para)
            .and_then(|level| list_levels.iter().position(|list| list.level == level))
    } else if detected_level.is_none()
        && list_info.is_none()
        && let Counted::Unmarked {
            level,
            definition,
            unmarked: true,
        } = counted
    {
        // A paragraph on a `none` level goes on the open item of a list of
        // its definition at its level or above.
        list_levels
            .iter()
            .rposition(|list| list.definition == definition && list.level <= level)
    } else {
        None
    };
    if let Some(index) = continued {
        let indent = list_levels[index].column;
        let blank_prefix = if quote_prefix.is_empty() && inner_quotes > 0 && compound_quote {
            format!("{}> ", " ".repeat(indent))
        } else {
            quote_prefix.clone()
        };
        list_levels.truncate(index + 1);
        if !out.ends_with("\n\n") && !out.ends_with("> \n") {
            out.push_str(&blank_prefix);
            out.push('\n');
        }
        for line in text.lines() {
            out.push_str(&format!("{quote_prefix}{}{line}\n", " ".repeat(indent)));
        }
        out.push_str(&blank_prefix);
        out.push('\n');
        return;
    }

    // Skip empty paragraphs (but still emit blank line for spacing)
    if text.is_empty() && heading_level.is_none() && list_info.is_none() {
        out.push('\n');
        return;
    }

    if let Some(level) = heading_level {
        // Heading
        leave_quoted_list(out, list_levels, &quote_prefix);
        // An ATX heading is one line, so a line break is an HTML one.
        let hashes = "#".repeat(level.min(6) as usize);
        out.push_str(&quote_prefix);
        out.push_str(&hashes);
        out.push(' ');
        out.push_str(&text.trim().replace(LINE_BREAK, "<br>"));
        out.push_str("\n\n");
    } else if let Some(item) = list_info {
        if item_quotes > 0 && !list_levels.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        // The outermost deeper list closed here had its markers at this
        // item's column.
        let mut closed = None;
        while list_levels
            .last()
            .is_some_and(|list| list.level > item.level)
        {
            closed = list_levels.pop().map(|list| list.ordered);
        }
        let ordered = item.kind.ordered();
        // The kind of the list before this item at its column, and its last
        // number when this item may go on with it.
        let previous = match list_levels.pop_if(|list| list.level == item.level) {
            Some(list) => Some((list.ordered, Some(list.last))),
            None => closed.map(|ordered| (ordered, None)),
        };
        // A nested item is indented to the text column of its parent item,
        // which is three columns after `1. ` and two after `- `.
        let indent = list_levels.last().map_or(0, |list| list.column);
        let blank_line = |out: &mut String| {
            if !out.ends_with("\n\n") {
                out.push_str(&quote_prefix);
                out.push('\n');
            }
        };
        let goes_on = |last: Option<u32>| {
            last.is_some_and(|last| !ordered || last.checked_add(1) == Some(item.count))
        };
        match previous {
            // An item of the same kind that does not go on with the list
            // before it at its column starts a new list: CommonMark would
            // join the two, so an empty comment parts them, as its spec
            // advises.
            Some((was_ordered, last)) if was_ordered == ordered && !goes_on(last) => {
                blank_line(out);
                out.push_str(&format!("{quote_prefix}{}<!-- -->\n", " ".repeat(indent)));
                blank_line(out);
            }
            // Another kind starts a new list by itself, but a number other
            // than 1 cannot interrupt the text before it, in a parent item
            // either: a blank line comes first.
            Some((was_ordered, _)) if was_ordered != ordered && ordered && item.count != 1 => {
                blank_line(out)
            }
            None if ordered && item.count != 1 && !list_levels.is_empty() => blank_line(out),
            _ => {}
        }
        let marker = if ordered {
            format!("{}. ", item.count)
        } else {
            "- ".to_owned()
        };
        list_levels.push(MarkdownList {
            level: item.level,
            column: indent + item_quote_prefix.len() + marker.len(),
            definition: item.definition,
            ordered,
            last: item.count,
        });
        let mut lines = text.trim_matches(|c: char| c.is_ascii_whitespace()).lines();
        out.push_str(&format!(
            "{quote_prefix}{}{item_quote_prefix}{marker}{}\n",
            " ".repeat(indent),
            lines.next().unwrap_or("")
        ));
        for line in lines {
            out.push_str(&format!(
                "{quote_prefix}{}{line}\n",
                " ".repeat(indent + item_quote_prefix.len() + marker.len())
            ));
        }
    } else {
        // Retain original edge whitespace and terminal hard-break spelling.
        leave_quoted_list(out, list_levels, &quote_prefix);
        if outer_quotes > 0 {
            for line in text.split('\n') {
                out.push_str(&quote_prefix);
                out.push_str(line);
                out.push('\n');
            }
            out.push('\n');
        } else {
            out.push_str(&text);
            out.push_str("\n\n");
        }
    }
}

fn container_path<'a>(id: Option<&str>, styles: &'a CT_Styles) -> Option<&'a str> {
    let style = styles.get_by_id(id?)?;
    if !style.style_id.starts_with("RDOCXHTMLContainers") {
        return None;
    }
    let path = style
        .name
        .as_deref()?
        .strip_prefix("RDOCX HTML containers ")?
        .split('|')
        .next()?;
    (!path.is_empty() && path.len() <= 256 && path.bytes().all(|b| matches!(b, b'q' | b'l')))
        .then_some(path)
}

fn quote_owner<'a>(id: Option<&str>, styles: &'a CT_Styles) -> Option<&'a str> {
    let style = styles.get_by_id(id?)?;
    if !style.style_id.starts_with("RDOCXHTMLContainers") {
        return None;
    }
    let owner = style
        .name
        .as_deref()?
        .strip_prefix("RDOCX HTML containers ")?
        .split_once('|')?
        .1;
    (!owner.is_empty()
        && owner
            .split('.')
            .all(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())))
    .then_some(owner)
}

/// CommonMark drops the whitespace a paragraph line starts with, and reads
/// four columns of it as an indented code block. Written as a character
/// reference, the first whitespace character of a line keeps the line's
/// whitespace and the paragraph. Two line breaks in a row leave a line of
/// spaces after a line of text, which a reader takes for a blank line that
/// ends the paragraph: it is written as a backslash hard break, or with its
/// first character as a reference when it holds more than the break.
fn keep_leading_whitespace(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let last = lines.len() - 1;
    let mut written: Vec<String> = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        let blank = !line.is_empty() && line.bytes().all(|byte| byte == b' ' || byte == b'\t');
        let after_text = written
            .last()
            .is_some_and(|previous| !previous.trim().is_empty());
        let first = line.chars().next();
        let out = match first {
            Some(first @ (' ' | '\t')) if !blank => {
                format!("&#{};{}", u32::from(first), &line[1..])
            }
            Some(first) if blank && index < last && after_text && line.ends_with("  ") => {
                if *line == "  " {
                    "\\".to_owned()
                } else {
                    format!("&#{};{}", u32::from(first), &line[1..])
                }
            }
            _ => (*line).to_owned(),
        };
        written.push(out);
    }
    written.join("\n")
}

fn escape_block_starts(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let offset = line.bytes().take_while(|&b| b == b' ').count().min(3);
            let rest = &line[offset..];
            let escaped = if rest.starts_with(['#', '>', '+', '-', '=']) {
                Some(offset)
            } else {
                let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                (digits > 0
                    && digits <= 9
                    && rest
                        .as_bytes()
                        .get(digits)
                        .is_some_and(|b| matches!(b, b'.' | b')')))
                .then_some(offset + digits)
            };
            if let Some(at) = escaped {
                format!("{}\\{}", &line[..at], &line[at..])
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A hard line break inside a paragraph.
const LINE_BREAK: &str = "  \n";
/// A page break, written as a thematic break between blank lines. Without
/// the blank line before it, `---` turns the line above into a heading.
const PAGE_BREAK: &str = "\n\n---\n\n";

/// The run formatting Markdown can spell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mark {
    Strike,
    Bold,
    Italic,
}

impl Mark {
    /// Every mark, outermost first when several start and end together.
    /// Strikethrough goes innermost, next to the text, because cmark-gfm
    /// reads a `~` beside a `*` run as a letter rather than punctuation.
    const ALL: [Mark; 3] = [Mark::Bold, Mark::Italic, Mark::Strike];

    fn delimiter(self) -> &'static str {
        match self {
            Mark::Strike => "~~",
            Mark::Bold => "**",
            Mark::Italic => "*",
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Mark::Strike => "del",
            Mark::Bold => "strong",
            Mark::Italic => "em",
        }
    }
}

/// The set of marks a stretch of text carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Marks {
    strike: bool,
    bold: bool,
    italic: bool,
}

impl Marks {
    fn from_rpr(rpr: Option<&oxoffice_docx_oxml::properties::CT_RPr>) -> Self {
        Marks {
            strike: rpr.is_some_and(|r| r.strike == Some(true) || r.dstrike == Some(true)),
            bold: rpr.is_some_and(|r| r.bold == Some(true)),
            italic: rpr.is_some_and(|r| r.italic == Some(true)),
        }
    }

    fn overlay(self, properties: Option<&oxoffice_docx_oxml::properties::CT_RPr>) -> Self {
        let Some(properties) = properties else {
            return self;
        };
        Marks {
            strike: properties
                .strike
                .or(properties.dstrike)
                .unwrap_or(self.strike),
            bold: properties.bold.unwrap_or(self.bold),
            italic: properties.italic.unwrap_or(self.italic),
        }
    }

    fn has(self, mark: Mark) -> bool {
        match mark {
            Mark::Strike => self.strike,
            Mark::Bold => self.bold,
            Mark::Italic => self.italic,
        }
    }
}

/// Collect all text from the accepted view of a paragraph, applying inline
/// formatting.
fn collect_paragraph_text(
    para: &CT_P,
    styles: &CT_Styles,
    hyperlink_urls: &crate::Links<'_>,
    images: &HashMap<String, ImageData>,
    boundaries: &[usize],
    equations: &dyn EquationWriter,
) -> String {
    let mut out = String::new();

    // The bookmarks a link targets, read before the accepted view drops
    // the markers, written as inline HTML anchors.
    let anchors = crate::bookmark_anchors(para, &hyperlink_urls.bookmarks);
    let anchor_html = |boundary: usize| {
        anchors
            .iter()
            .filter(|(at, _)| *at == boundary)
            .map(|(_, name)| format!("<a id=\"{}\"></a>", crate::sanitize::escape_html_attr(name)))
            .collect::<String>()
    };

    // The runs the text readers read, those of content controls, tracked
    // insertions, smart tags and custom XML included, deleted ones left out.
    let (view, projected_boundaries) = para.accepted_view_with_field_owners();
    let para = &view;

    // Markdown renderers turn `[text](javascript:...)` into a live link too,
    // so the same scheme allowlist applies here.
    let hyperlink_map = crate::hyperlink_targets(para, hyperlink_urls);

    // Consecutive runs in the same link are written as one link, and the
    // formatting inside and outside each link is rendered as a whole.
    let mut current_link: Option<&str> = None;
    let mut pieces: Vec<(String, Marks)> = Vec::new();

    for (run_idx, run) in para.runs.iter().enumerate() {
        if boundaries.contains(&run_idx) || projected_boundaries.contains(&run_idx) {
            pieces.push((String::new(), Marks::default()));
        }
        let anchor = anchor_html(run_idx);
        if !anchor.is_empty() {
            push_inline(&mut out, &pieces, current_link);
            pieces.clear();
            out.push_str(&anchor);
        }
        if let Some(math) = equations_latex(para, run_idx, equations) {
            push_inline(&mut out, &pieces, current_link);
            pieces.clear();
            out.push_str(&math);
        }
        let in_link = hyperlink_map.get(&run_idx).map(String::as_str);
        if in_link != current_link {
            push_inline(&mut out, &pieces, current_link);
            pieces.clear();
            current_link = in_link;
        }
        collect_run_pieces(run, styles, images, &mut pieces);
    }
    push_inline(&mut out, &pieces, current_link);
    out.push_str(&anchor_html(para.runs.len()));
    if let Some(math) = equations_latex(para, para.runs.len(), equations) {
        out.push_str(&math);
    }

    out
}

/// The equations at a run boundary as LaTeX: `$...$` inline, `$$...$$` for
/// each equation of a display, on one line so that a list item or a table
/// cell holds it.
fn equations_latex(para: &CT_P, boundary: usize, equations: &dyn EquationWriter) -> Option<String> {
    let mut out = String::new();
    let mut push = |math: &oxoffice_docx_oxml::math::CT_OMath, delimiter: &str| {
        if let Some(latex) = equations.latex(math) {
            let latex = latex.split_whitespace().collect::<Vec<_>>().join(" ");
            if !latex.is_empty() {
                out.push_str(&format!("{delimiter}{latex}{delimiter}"));
            }
        }
    };
    for equation in crate::equations_at(para, boundary) {
        match equation {
            OfficeMath::Inline(math) => push(math, "$"),
            OfficeMath::Display(display) => {
                equations.display(display);
                for math in &display.equations {
                    push(math, "$$");
                }
            }
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Append formatted pieces to `out`, as the text of a link when `link` is set.
fn push_inline(out: &mut String, pieces: &[(String, Marks)], link: Option<&str>) {
    let mut start = 0;
    for (index, (text, _)) in pieces.iter().enumerate() {
        if text == PAGE_BREAK {
            push_inline_segment(out, &pieces[start..index], link);
            out.push_str(PAGE_BREAK);
            start = index + 1;
        }
    }
    push_inline_segment(out, &pieces[start..], link);
}

fn push_inline_segment(out: &mut String, pieces: &[(String, Marks)], link: Option<&str>) {
    let text = escape_block_starts(&render_inline(pieces));
    if text.is_empty() {
        return;
    }
    match link {
        Some(url) => {
            // `![` would start an image.
            if out.ends_with('!') {
                out.pop();
                out.push_str("\\!");
            }
            out.push('[');
            out.push_str(&text);
            out.push_str("](");
            push_link_destination(out, url);
            out.push(')');
        }
        None => out.push_str(&text),
    }
}

/// Write a URL as a link destination. A space or control character ends a
/// destination and an unbalanced parenthesis ends the link, so those are
/// percent-encoded or escaped.
fn push_link_destination(out: &mut String, url: &str) {
    for (index, character) in url.char_indices() {
        match character {
            // A reader decodes a character reference in a destination too.
            '&' if starts_character_reference(&url[index + 1..]) => out.push_str("\\&"),
            ' ' => out.push_str("%20"),
            character if character.is_ascii_control() => {
                out.push_str(&format!("%{:02X}", u32::from(character)));
            }
            '(' | ')' | '<' | '>' | '\\' => {
                out.push('\\');
                out.push(character);
            }
            character => out.push(character),
        }
    }
}

/// Append `text` with `marks`, merged into the previous piece when its marks
/// are the same.
fn push_piece(pieces: &mut Vec<(String, Marks)>, text: &str, marks: Marks) {
    if text.is_empty() {
        return;
    }
    match pieces.last_mut() {
        Some((last, last_marks)) if !last.is_empty() && *last_marks == marks => {
            escape_reference_across(last, text);
            last.push_str(text);
        }
        _ => pieces.push((text.to_owned(), marks)),
    }
}

/// Runs with the same formatting are written as one text, so a character
/// reference split across them (Word splits runs around a word it flags)
/// becomes one there: escape the `&` it starts with, at the end of `last`.
fn escape_reference_across(last: &mut String, next: &str) {
    // A reference body is at most 32 bytes, so its `&` is among the last 33.
    let from = last.len().saturating_sub(33);
    let Some(at) = last.as_bytes()[from..]
        .iter()
        .rposition(|&byte| byte == b'&')
        .map(|index| index + from)
    else {
        return;
    };
    let backslashes = last.as_bytes()[..at]
        .iter()
        .rev()
        .take_while(|&&byte| byte == b'\\')
        .count();
    if backslashes % 2 == 0 && starts_character_reference(&format!("{}{next}", &last[at + 1..])) {
        last.insert(at, '\\');
    }
}

/// Append a character of document text, escaping the punctuation that would
/// otherwise start emphasis, a code span, a link, raw HTML or strikethrough.
fn push_escaped(text: &mut String, character: char) {
    // `$` delimits the equations this export writes.
    if matches!(
        character,
        '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '~' | '$'
    ) {
        text.push('\\');
    }
    text.push(character);
}

/// Append document text as `push_escaped` does, escaping as well an `&` that
/// starts a character reference, which a reader would turn into another
/// character.
fn push_escaped_text(out: &mut String, text: &str) {
    for (index, character) in text.char_indices() {
        push_escaped_at(out, text, index, character);
    }
}

/// `push_escaped` for the character at `index` of `text`, with the `&` of a
/// character reference escaped.
fn push_escaped_at(out: &mut String, text: &str, index: usize, character: char) {
    if character == '&' && starts_character_reference(&text[index + 1..]) {
        out.push('\\');
    }
    push_escaped(out, character);
}

/// Whether `rest`, the text after an `&`, starts as a character reference
/// does: a name, `#` and decimal digits, or `#x` and hexadecimal digits,
/// then `;`. A reference body is at most 32 bytes, so only that far is
/// read.
fn starts_character_reference(rest: &str) -> bool {
    let bytes = rest.as_bytes();
    match bytes.iter().take(33).position(|&byte| byte == b';') {
        Some(end) => {
            let body = &bytes[..end];
            match body.strip_prefix(b"#") {
                Some(number) => match number.strip_prefix(b"x").or(number.strip_prefix(b"X")) {
                    Some(hex) => {
                        (1..=6).contains(&hex.len()) && hex.iter().all(u8::is_ascii_hexdigit)
                    }
                    None => {
                        (1..=7).contains(&number.len()) && number.iter().all(u8::is_ascii_digit)
                    }
                },
                None => {
                    (1..=32).contains(&body.len())
                        && body[0].is_ascii_alphabetic()
                        && body.iter().all(u8::is_ascii_alphanumeric)
                }
            }
        }
        None => false,
    }
}

/// Collect the text of a single run as formatted pieces.
fn collect_run_pieces(
    run: &CT_R,
    styles: &CT_Styles,
    images: &HashMap<String, ImageData>,
    pieces: &mut Vec<(String, Marks)>,
) {
    let marks = Marks::from_rpr(run.properties.as_ref());
    let mut raw = String::new();
    let code = run
        .properties
        .as_ref()
        .and_then(|properties| properties.style_id.as_deref())
        .is_some_and(|id| {
            id == "HTMLCode"
                || styles
                    .get_by_id(id)
                    .and_then(|style| style.name.as_deref())
                    .is_some_and(|name| name.eq_ignore_ascii_case("HTML Code"))
        });
    if code {
        for content in &run.content {
            match content {
                RunContent::Text(text) | RunContent::DeletedText(text) => raw.push_str(&text.text),
                RunContent::Tab => raw.push('\t'),
                _ => {}
            }
        }
        let delimiter = "`".repeat(
            raw.split(|character| character != '`')
                .map(str::len)
                .max()
                .unwrap_or(0)
                + 1,
        );
        let pad = raw.starts_with('`')
            || raw.ends_with('`')
            || (raw.starts_with(' ')
                && raw.ends_with(' ')
                && !raw.chars().all(|character| character == ' '));
        let padding = if pad { " " } else { "" };
        push_piece(
            pieces,
            &format!("{delimiter}{padding}{raw}{padding}{delimiter}"),
            marks,
        );
        return;
    }

    for content in &run.content {
        match content {
            RunContent::Text(t) | RunContent::DeletedText(t) => {
                push_escaped_text(&mut raw, &t.text);
            }
            RunContent::Drawing(drawing) => {
                let picture = drawing
                    .inline
                    .as_ref()
                    .map(|inline| (inline.embed_id.as_str(), inline.description.as_deref()))
                    .or_else(|| {
                        drawing
                            .anchor
                            .as_ref()
                            .map(|anchor| (anchor.embed_id.as_str(), anchor.description.as_deref()))
                    });
                if let Some((relationship, description)) = picture
                    && let Some(image) = images.get(relationship)
                {
                    let mut alt = String::new();
                    push_escaped_text(&mut alt, description.unwrap_or(""));
                    raw.push_str(&format!(
                        "![{alt}](data:{};base64,{})",
                        image.content_type,
                        crate::emitter::base64_encode(&image.data)
                    ));
                }
            }
            RunContent::Tab => raw.push('\t'),
            RunContent::Break(bt, _) => match bt {
                BreakType::Line | BreakType::Column => {
                    push_piece(pieces, &std::mem::take(&mut raw), marks);
                    pieces.push((String::new(), Marks::default()));
                    pieces.push((LINE_BREAK.to_owned(), Marks::default()));
                    pieces.push((String::new(), Marks::default()));
                }
                BreakType::Page => {
                    push_piece(pieces, &std::mem::take(&mut raw), marks);
                    pieces.push((String::new(), Marks::default()));
                    pieces.push((PAGE_BREAK.to_owned(), Marks::default()));
                    pieces.push((String::new(), Marks::default()));
                }
            },
            RunContent::Field(field) => {
                push_piece(pieces, &std::mem::take(&mut raw), marks);
                pieces.push((String::new(), Marks::default()));
                for (text, properties) in field.cached_display_segments() {
                    let field_marks = marks.overlay(properties);
                    let mut display = String::new();
                    for (index, character) in text.char_indices() {
                        match character {
                            '\t' => display.push('\t'),
                            '\n' | '\u{000b}' => {
                                push_piece(pieces, &std::mem::take(&mut display), field_marks);
                                pieces.push((String::new(), Marks::default()));
                                pieces.push((LINE_BREAK.to_owned(), Marks::default()));
                                pieces.push((String::new(), Marks::default()));
                            }
                            '\u{000c}' => {
                                push_piece(pieces, &std::mem::take(&mut display), field_marks);
                                pieces.push((String::new(), Marks::default()));
                                pieces.push((PAGE_BREAK.to_owned(), Marks::default()));
                                pieces.push((String::new(), Marks::default()));
                            }
                            character => push_escaped_at(&mut display, text, index, character),
                        }
                    }
                    push_piece(pieces, &display, field_marks);
                }
                pieces.push((String::new(), Marks::default()));
            }
            RunContent::SpecialCharacter(character) => match character {
                SpecialCharacter::CarriageReturn => {
                    push_piece(pieces, &std::mem::take(&mut raw), marks);
                    pieces.push((String::new(), Marks::default()));
                    pieces.push((LINE_BREAK.to_owned(), Marks::default()));
                    pieces.push((String::new(), Marks::default()));
                }
                SpecialCharacter::NoBreakHyphen => raw.push('\u{2011}'),
                SpecialCharacter::SoftHyphen => raw.push('\u{00ad}'),
                SpecialCharacter::PositionalTab { .. } => raw.push('\t'),
            },
            // A symbol is font-encoded rather than Unicode, so there is no
            // portable Markdown spelling for it.
            RunContent::Symbol { .. }
            | RunContent::FootnoteRef { .. }
            | RunContent::EndnoteRef { .. }
            | RunContent::CommentReference { .. } => {}
        }
    }

    push_piece(pieces, &raw, marks);
}

/// One emphasis, strikethrough or bold span between two boundaries.
struct Span {
    mark: Mark,
    /// The boundaries it opens and closes at.
    open: usize,
    close: usize,
    /// Written as an HTML element because CommonMark would not read
    /// delimiters at one of its boundaries as intended.
    html: bool,
}

/// Render formatted pieces as CommonMark inline text.
///
/// Whitespace at the edge of a piece is written outside the delimiters,
/// because a delimiter next to whitespace on its inner side does not open or
/// close. A piece of whitespace alone takes no delimiters. Marks are opened
/// and closed only where they change, so neighbouring pieces with the same
/// marks share one pair of delimiters, and spans nest. A span whose
/// delimiters CommonMark would read as literal text at a boundary, next to
/// punctuation inside a word for example, is written as an HTML element.
fn render_inline(pieces: &[(String, Marks)]) -> String {
    // An empty piece is a physical field/break boundary, not empty source text.
    // Render each side independently so equal marks cannot cross that owner.
    pieces
        .split(|(text, _)| text.is_empty())
        .map(render_inline_segment)
        .collect()
}

fn render_inline_segment(pieces: &[(String, Marks)]) -> String {
    // The text between whitespace, and the whitespace before, between and
    // after it. `gaps` has one more entry than `cores`.
    let mut cores: Vec<(String, Marks)> = Vec::new();
    let mut gaps = vec![String::new()];
    for (text, marks) in pieces {
        let core = text.trim();
        if core.is_empty() {
            gaps.last_mut().expect("gaps is never empty").push_str(text);
            continue;
        }
        let start = text.len() - text.trim_start().len();
        let gap = gaps.last_mut().expect("gaps is never empty");
        gap.push_str(&text[..start]);
        match cores.last_mut() {
            Some((last, last_marks)) if gap.is_empty() && last_marks == marks => {
                last.push_str(core);
            }
            _ => {
                cores.push((core.to_owned(), *marks));
                gaps.push(String::new());
            }
        }
        gaps.last_mut()
            .expect("gaps is never empty")
            .push_str(&text[start + core.len()..]);
    }

    // Boundary `b` lies before core `b`, and the last boundary after every
    // core. Each lists the spans it closes, innermost first, those that stay
    // open across it, then those it opens, outermost first. Each mark is
    // open at most once, so none of these lists is longer than three.
    let boundaries = cores.len() + 1;
    let mut spans: Vec<Span> = Vec::new();
    let mut closes: Vec<Vec<usize>> = vec![Vec::new(); boundaries];
    let mut encloses: Vec<Vec<usize>> = vec![Vec::new(); boundaries];
    let mut opens: Vec<Vec<usize>> = vec![Vec::new(); boundaries];
    let mut stack: Vec<usize> = Vec::new();
    for boundary in 0..boundaries {
        let wanted = cores
            .get(boundary)
            .map_or(Marks::default(), |(_, marks)| *marks);
        // Spans nest, so a mark that ends closes every span opened after it.
        if let Some(depth) = stack.iter().position(|&span| !wanted.has(spans[span].mark)) {
            for span in stack.drain(depth..).rev() {
                spans[span].close = boundary;
                closes[boundary].push(span);
            }
        }
        encloses[boundary].clone_from(&stack);
        let mut starting: Vec<Mark> = Mark::ALL
            .into_iter()
            .filter(|&mark| wanted.has(mark) && !stack.iter().any(|&span| spans[span].mark == mark))
            .collect();
        // The longest lasting mark goes outermost, so it need not be closed
        // and reopened when a shorter one ends.
        starting.sort_by_key(|&mark| {
            Reverse(
                cores[boundary..]
                    .iter()
                    .take_while(|(_, marks)| marks.has(mark))
                    .count(),
            )
        });
        for mark in starting {
            stack.push(spans.len());
            opens[boundary].push(spans.len());
            spans.push(Span {
                mark,
                open: boundary,
                close: boundaries,
                html: false,
            });
        }
    }

    // A span becomes HTML when one of its boundaries fails, which changes
    // the neighbours of the delimiters at its other boundary, so that one is
    // checked again. Fewer delimiters never make another boundary fail.
    let mut pending: Vec<usize> = (0..boundaries).collect();
    while let Some(boundary) = pending.pop() {
        let before = boundary
            .checked_sub(1)
            .and_then(|core| cores[core].0.chars().last());
        let after = cores
            .get(boundary)
            .and_then(|(core, _)| core.chars().next());
        let closing: Vec<(&Span, bool)> = closes[boundary]
            .iter()
            .map(|&span| (&spans[span], false))
            .collect();
        let opening: Vec<(&Span, bool)> = opens[boundary]
            .iter()
            .map(|&span| (&spans[span], true))
            .collect();
        let enclosed = encloses[boundary]
            .iter()
            .any(|&span| !spans[span].html && spans[span].mark != Mark::Strike);
        let gap = &gaps[boundary];
        let holds = if gap.is_empty() {
            delimiters_hold(before, &[closing, opening].concat(), after, enclosed)
        } else {
            delimiters_hold(before, &closing, gap.chars().next(), enclosed)
                && delimiters_hold(gap.chars().last(), &opening, after, enclosed)
        };
        if holds {
            continue;
        }
        for &span in closes[boundary].iter().chain(&opens[boundary]) {
            if !spans[span].html {
                spans[span].html = true;
                pending.push(spans[span].open);
                pending.push(spans[span].close);
            }
        }
    }

    let mut out = String::new();
    for boundary in 0..boundaries {
        for &span in &closes[boundary] {
            let Span { mark, html, .. } = spans[span];
            if html {
                out.push_str(&format!("</{}>", mark.tag()));
            } else {
                out.push_str(mark.delimiter());
            }
        }
        out.push_str(&gaps[boundary]);
        for &span in &opens[boundary] {
            let Span { mark, html, .. } = spans[span];
            if html {
                out.push_str(&format!("<{}>", mark.tag()));
            } else {
                out.push_str(mark.delimiter());
            }
        }
        if let Some((core, _)) = cores.get(boundary) {
            out.push_str(core);
        }
    }
    out
}

/// How a delimiter run sees the character on one side of it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Flank {
    Whitespace,
    Punctuation,
    Other,
}

impl Flank {
    /// The readings of a character. `None` is the edge of the inline text,
    /// which counts as whitespace. cmark-gfm does not count `~` as
    /// punctuation, unlike the CommonMark spec, and CommonMark versions
    /// differ on which other Unicode symbols are, so those are read both ways.
    fn of(character: Option<char>) -> &'static [Flank] {
        match character {
            None => &[Flank::Whitespace],
            Some(c) if c.is_whitespace() => &[Flank::Whitespace],
            Some(c) if c.is_alphanumeric() => &[Flank::Other],
            Some(c) if c.is_ascii_punctuation() && c != '~' => &[Flank::Punctuation],
            Some(_) => &[Flank::Punctuation, Flank::Other],
        }
    }
}

/// Whether CommonMark reads each delimiter in `tokens`, written between the
/// characters `before` and `after`, as the opener or closer it is meant to
/// be. `enclosed` says whether a `*` span stays open across the boundary.
///
/// Neighbouring delimiters of the same character form one delimiter run. A
/// run that closes must be right-flanking and one that opens left-flanking.
/// A run that is both could close an enclosing `*` span instead of opening,
/// and past three characters the rule of three can pair it differently. A
/// `~` run must be exactly `~~`. Every reading of a neighbouring character
/// must hold.
fn delimiters_hold(
    before: Option<char>,
    tokens: &[(&Span, bool)],
    after: Option<char>,
    enclosed: bool,
) -> bool {
    let character = |span: &Span| if span.mark == Mark::Strike { '~' } else { '*' };
    // A neighbouring token is an HTML tag or a delimiter of the other character.
    let neighbour = |span: &Span| Flank::of(Some(if span.html { '<' } else { character(span) }));

    let mut start = 0;
    while start < tokens.len() {
        let (span, _) = tokens[start];
        if span.html {
            start += 1;
            continue;
        }
        let delimiter = character(span);
        let mut end = start;
        while end < tokens.len() && !tokens[end].0.html && character(tokens[end].0) == delimiter {
            end += 1;
        }
        let run = &tokens[start..end];
        let previous = start
            .checked_sub(1)
            .map_or(Flank::of(before), |index| neighbour(tokens[index].0));
        let next = tokens
            .get(end)
            .map_or(Flank::of(after), |(span, _)| neighbour(span));
        let length: usize = run
            .iter()
            .map(|(span, _)| span.mark.delimiter().len())
            .sum();
        let closes = run.iter().any(|&(_, opens)| !opens);
        let opens = run.iter().any(|&(_, opens)| opens);
        for &previous in previous {
            for &next in next {
                let left = next != Flank::Whitespace
                    && (next != Flank::Punctuation || previous != Flank::Other);
                let right = previous != Flank::Whitespace
                    && (previous != Flank::Punctuation || next != Flank::Other);
                if (closes && !right)
                    || (opens && !left)
                    || (opens && right && enclosed && delimiter == '*')
                    || (left && right && length > 3)
                    || (delimiter == '~' && length != 2)
                {
                    return false;
                }
            }
        }
        start = end;
    }
    true
}

/// Emit a table as a GFM pipe table.
fn emit_table(
    out: &mut String,
    tbl: &CT_Tbl,
    styles: &CT_Styles,
    hyperlink_urls: &crate::Links<'_>,
    images: &HashMap<String, ImageData>,
    equations: &dyn EquationWriter,
) {
    if tbl.rows().is_empty() {
        return;
    }

    // Collect all rows as cell text vectors
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut max_cols = 0;

    let limit = crate::emitter::grid_limit(tbl);
    // Rows and cells that content controls wrap are written in place.
    for row in tbl
        .rows()
        .into_iter()
        .filter(|row| !row.accepted_view_removes())
    {
        // GFM cannot span: a cell spanning several grid columns, or the
        // columns a row omits before its first cell, leave empty cells, so
        // that every cell stays in its grid column.
        let mut cells: Vec<String> = Vec::new();
        let ranges = crate::emitter::grid_ranges(row, limit);
        for (cell, &(start, _)) in row.cells().into_iter().zip(&ranges) {
            // Starts clamped to the table's width can repeat: only grow.
            if cells.len() < start {
                cells.resize(start, String::new());
            }
            let text = collect_cell_text(cell, styles, hyperlink_urls, images, equations);
            cells.push(text);
        }
        if cells.len() > max_cols {
            max_cols = cells.len();
        }
        rows.push(cells);
    }

    if rows.is_empty() || max_cols == 0 {
        return;
    }

    // Pad rows to same length
    for row in &mut rows {
        while row.len() < max_cols {
            row.push(String::new());
        }
    }

    // Emit header row (first row)
    out.push('|');
    for cell in &rows[0] {
        out.push_str(&format!(" {} |", cell.trim()));
    }
    out.push('\n');

    // Emit separator
    out.push('|');
    // Each column takes the alignment of the header cell that starts there.
    let header = tbl
        .rows()
        .into_iter()
        .find(|row| !row.accepted_view_removes());
    let header_cells = header.map(|row| row.cells()).unwrap_or_default();
    let header_ranges = header.map_or_else(Vec::new, |row| crate::emitter::grid_ranges(row, limit));
    for column in 0..max_cols {
        let alignment = header_ranges
            .iter()
            .position(|&(start, _)| start == column)
            .and_then(|index| header_cells.get(index))
            .and_then(|cell| cell.paragraphs().first().copied())
            .and_then(|paragraph| paragraph.properties.as_ref())
            .and_then(|properties| properties.jc);
        out.push_str(match alignment {
            Some(oxoffice_docx_oxml::shared::ST_Jc::Center) => " :---: |",
            Some(
                oxoffice_docx_oxml::shared::ST_Jc::Right | oxoffice_docx_oxml::shared::ST_Jc::End,
            ) => " ---: |",
            Some(
                oxoffice_docx_oxml::shared::ST_Jc::Left | oxoffice_docx_oxml::shared::ST_Jc::Start,
            ) => " :--- |",
            _ => " --- |",
        });
    }
    out.push('\n');

    // Emit data rows
    for row in rows.iter().skip(1) {
        out.push('|');
        for cell in row {
            out.push_str(&format!(" {} |", cell.trim()));
        }
        out.push('\n');
    }

    out.push('\n');
}

/// Collect text from a table cell.
fn collect_cell_text(
    cell: &oxoffice_docx_oxml::table::CT_Tc,
    styles: &CT_Styles,
    hyperlink_urls: &crate::Links<'_>,
    images: &HashMap<String, ImageData>,
    equations: &dyn EquationWriter,
) -> String {
    let mut parts = Vec::new();

    for block in cell_blocks(cell) {
        match block {
            Block::Paragraph(p, boundaries) => {
                // A pipe table row is one line, so breaks inside a cell are
                // written as HTML line breaks. A pipe would end the cell, in
                // a link destination too, so every pipe is escaped, and GFM
                // removes the escape before it reads the cell's inlines.
                let text = collect_paragraph_text(
                    &p,
                    styles,
                    hyperlink_urls,
                    images,
                    &boundaries,
                    equations,
                )
                .replace(PAGE_BREAK, " ");
                let trimmed = text.trim().replace(LINE_BREAK, "<br>").replace('|', "\\|");
                if !trimmed.is_empty() {
                    parts.push(trimmed);
                }
            }
            Block::Table(table) if table.accepted_view_removes() => {}
            // GFM has no nested tables: a nested table's rows go into the
            // cell, each on a line of its own, its cells between pipes.
            Block::Table(table) => {
                let rows: Vec<String> = table
                    .rows()
                    .into_iter()
                    .filter(|row| !row.accepted_view_removes())
                    .map(|row| {
                        row.cells()
                            .into_iter()
                            .map(|cell| {
                                collect_cell_text(cell, styles, hyperlink_urls, images, equations)
                            })
                            .filter(|text| !text.is_empty())
                            .collect::<Vec<_>>()
                            .join(" \\| ")
                    })
                    .filter(|row| !row.is_empty())
                    .collect();
                if !rows.is_empty() {
                    parts.push(rows.join("<br>"));
                }
            }
        }
    }

    parts.join(" ")
}

fn emit_chart_table(out: &mut String, chart: &crate::ChartTable) {
    let escaped = |text: &str| {
        let mut result = String::new();
        for (index, ch) in text.char_indices() {
            if ch == '|' {
                result.push('\\');
            }
            push_escaped_at(&mut result, text, index, ch);
        }
        result.replace('\n', "<br>").replace('\r', "")
    };
    if let Some(title) = chart.title.as_ref().filter(|title| !title.is_empty()) {
        out.push_str(&escape_block_starts(&escaped(title)));
        out.push_str("\n\n");
    }
    for (index, row) in chart.rows().iter().enumerate() {
        out.push('|');
        for cell in row {
            out.push_str(&format!(" {} |", escaped(cell)));
        }
        out.push('\n');
        if index == 0 {
            out.push('|');
            for _ in row {
                out.push_str(" --- |");
            }
            out.push('\n');
        }
    }
    out.push('\n');
}
