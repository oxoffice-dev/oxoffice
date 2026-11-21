//! HTML element generation from OXML types.

use std::collections::{HashMap, HashSet};

use oxoffice_docx_oxml::math::OfficeMath;
use oxoffice_docx_oxml::properties::{CT_PPr, CT_RPr};
use oxoffice_docx_oxml::shared::ST_Jc;
use oxoffice_docx_oxml::styles::{CT_Styles, StyleType};
use oxoffice_docx_oxml::table::{CT_Row, CT_Tbl, VMerge};
use oxoffice_docx_oxml::text::{BreakType, CT_P, CT_R, RunContent, SpecialCharacter};

use crate::css;
use crate::sanitize::{escape_html, escape_html_attr};
use crate::{
    Block, Counted, EquationWriter, HtmlInput, HtmlOptions, ImageData, ListCounter, ListItem,
    ListKind, body_blocks, cell_blocks,
};

/// Emit the full body content as HTML.
pub(crate) fn emit_body(
    input: &HtmlInput,
    options: &HtmlOptions,
    charts: &HashMap<String, crate::ChartTable>,
    equations: &dyn EquationWriter,
) -> String {
    let body = &input.document.body;
    let styles = &input.styles;
    let numbering = input.numbering.as_ref();
    let images = &input.images;
    let hyperlink_urls = &crate::Links::new(&input.hyperlink_urls, body);
    let mut out = String::new();
    let mut counter = ListCounter::new(styles, numbering);
    // The open lists, outermost first; each holds its open item, in which
    // the next list is nested.
    let mut lists: Vec<OpenList> = Vec::new();

    for block in body_blocks(body, false) {
        match block {
            Block::Paragraph(p, _) => {
                let heading = detect_heading_level(p.properties.as_ref(), styles).is_some();
                let counted = counter.paragraph(&p);
                // A heading numbered through its style, as Word numbers its
                // headings, stays a heading; only its own numbering makes it
                // a list item.
                let item = match counted {
                    Counted::Item(item) if !heading || crate::has_own_numbering(&p) => Some(item),
                    _ => None,
                };
                // A paragraph on a `none` level goes on the open item of a
                // list of its definition at its level or above, and that list
                // stays open, as layout keeps counting it. A paragraph whose
                // numbering is turned off (`numId` 0, as an HTML import writes
                // the paragraphs after an item's first) goes on the open item
                // at its level.
                let continued = if item.is_some() || heading {
                    None
                } else if let Counted::Unmarked {
                    level,
                    definition,
                    unmarked: true,
                    ..
                } = counted
                {
                    lists
                        .iter()
                        .rposition(|list| list.definition == definition && list.level <= level)
                } else {
                    crate::numbering_turned_off_level(&p)
                        .and_then(|level| lists.iter().position(|list| list.level == level))
                };

                if let Some(index) = continued {
                    while lists.len() > index + 1 {
                        close_list(&mut out, &mut lists);
                    }
                    out.push_str("<p>");
                    emit_paragraph_content(
                        &mut out,
                        &p,
                        styles,
                        images,
                        hyperlink_urls,
                        options,
                        equations,
                    );
                    out.push_str("</p>");
                } else if let Some(item) = item {
                    open_item(&mut out, &mut lists, &item);
                    out.push_str(&format!("<li{}>", direction_attribute(&p, styles)));
                    emit_paragraph_content(
                        &mut out,
                        &p,
                        styles,
                        images,
                        hyperlink_urls,
                        options,
                        equations,
                    );
                } else {
                    close_lists(&mut out, &mut lists);
                    emit_paragraph(
                        &mut out,
                        &p,
                        styles,
                        images,
                        hyperlink_urls,
                        options,
                        equations,
                    );
                }
                for id in crate::paragraph_chart_ids(&p) {
                    if let Some(chart) = charts.get(&id) {
                        close_lists(&mut out, &mut lists);
                        emit_chart_table(&mut out, chart);
                    }
                }
            }
            Block::Table(tbl) => {
                counter.table(tbl);
                close_lists(&mut out, &mut lists);
                emit_table(
                    &mut out,
                    tbl,
                    styles,
                    images,
                    hyperlink_urls,
                    options,
                    equations,
                );
            }
        }
    }

    close_lists(&mut out, &mut lists);
    out
}

/// An open `<ol>` or `<ul>` and its open item.
struct OpenList {
    kind: ListKind,
    /// The level of its items.
    level: u32,
    /// The definition that counts its items.
    definition: u32,
    /// The number of its last item.
    last: u32,
}

/// Get the list for `item` ready, its previous item closed: close the
/// deeper lists, go on with the list at its level when the item continues
/// it (same kind and, numbered, the next number), else open a new list,
/// nested in the open item of the list above.
fn open_item(out: &mut String, lists: &mut Vec<OpenList>, item: &ListItem) {
    while lists.last().is_some_and(|list| list.level > item.level) {
        close_list(out, lists);
    }
    if let Some(list) = lists.last_mut().filter(|list| list.level == item.level) {
        if list.kind == item.kind
            && (!item.kind.ordered() || list.last.checked_add(1) == Some(item.count))
        {
            out.push_str("</li>\n");
            list.last = item.count;
            list.definition = item.definition;
            return;
        }
        close_list(out, lists);
    }
    let tag = if item.kind.ordered() { "ol" } else { "ul" };
    let kind = match item.kind {
        ListKind::LowerLetter => " type=\"a\"",
        ListKind::UpperLetter => " type=\"A\"",
        ListKind::LowerRoman => " type=\"i\"",
        ListKind::UpperRoman => " type=\"I\"",
        ListKind::Bullet | ListKind::Decimal => "",
    };
    let start = if item.kind.ordered() && item.count != 1 {
        format!(" start=\"{}\"", item.count)
    } else {
        String::new()
    };
    if !lists.is_empty() {
        // Nested in the parent's open item.
        out.push('\n');
    }
    out.push_str(&format!("<{tag}{kind}{start}>\n"));
    lists.push(OpenList {
        kind: item.kind,
        level: item.level,
        definition: item.definition,
        last: item.count,
    });
}

/// Close the innermost list and its open item.
fn close_list(out: &mut String, lists: &mut Vec<OpenList>) {
    if let Some(list) = lists.pop() {
        let tag = if list.kind.ordered() { "ol" } else { "ul" };
        out.push_str(&format!("</li>\n</{tag}>\n"));
    }
}

fn close_lists(out: &mut String, lists: &mut Vec<OpenList>) {
    while !lists.is_empty() {
        close_list(out, lists);
    }
}

/// Detect heading level from paragraph style.
fn detect_heading_level(ppr: Option<&CT_PPr>, styles: &CT_Styles) -> Option<u32> {
    let ppr = ppr?;

    // Check outline_lvl directly
    if let Some(lvl) = ppr.outline_lvl {
        return Some(lvl + 1); // outline_lvl is 0-based, headings are 1-based
    }

    // Check style_id
    let style_id = ppr.style_id.as_deref()?;
    if let Some(level) = style_id.strip_prefix("Heading")
        && let Ok(n) = level.parse::<u32>()
        && (1..=6).contains(&n)
    {
        return Some(n);
    }

    // Check style's outline level
    if let Some(style) = styles.get_by_id(style_id)
        && let Some(spr) = &style.ppr
        && let Some(lvl) = spr.outline_lvl
    {
        return Some(lvl + 1);
    }

    None
}

/// Emit a complete paragraph element.
fn emit_paragraph(
    out: &mut String,
    para: &CT_P,
    styles: &CT_Styles,
    images: &HashMap<String, ImageData>,
    hyperlink_urls: &crate::Links<'_>,
    options: &HtmlOptions,
    equations: &dyn EquationWriter,
) {
    let heading_level = detect_heading_level(para.properties.as_ref(), styles);

    let tag = match heading_level {
        Some(1) => "h1",
        Some(2) => "h2",
        Some(3) => "h3",
        Some(4) => "h4",
        Some(5) => "h5",
        Some(6) => "h6",
        _ => "p",
    };

    let style = css::paragraph_style(para.properties.as_ref());
    let direction = direction_attribute(para, styles);
    if style.is_empty() {
        out.push_str(&format!("<{tag}{direction}>"));
    } else {
        out.push_str(&format!(
            "<{tag}{direction} style=\"{}\">",
            escape_html_attr(&style)
        ));
    }

    emit_paragraph_content(
        out,
        para,
        styles,
        images,
        hyperlink_urls,
        options,
        equations,
    );

    out.push_str(&format!("</{tag}>\n"));
}

/// ` dir="rtl"` for a right-to-left paragraph, so that a browser lays it out
/// right to left; empty otherwise. `w:bidi` is read as layout resolves it:
/// the paragraph's own, its style's (the default paragraph style when it
/// names none) through `basedOn`, then the document defaults.
fn direction_attribute(para: &CT_P, styles: &CT_Styles) -> &'static str {
    let direct = para.properties.as_ref();
    let mut bidi = direct.and_then(|properties| properties.bidi);
    let mut style_id = direct
        .and_then(|properties| properties.style_id.as_deref())
        .or_else(|| {
            styles
                .get_default(StyleType::Paragraph)
                .map(|style| style.style_id.as_str())
        });
    let mut seen = HashSet::new();
    while bidi.is_none()
        && let Some(id) = style_id
        && seen.insert(id)
        && let Some(style) = styles.get_by_id(id)
    {
        bidi = style.ppr.as_ref().and_then(|properties| properties.bidi);
        style_id = style.based_on.as_deref();
    }
    let bidi = bidi.or_else(|| {
        styles
            .doc_defaults
            .as_ref()
            .and_then(|defaults| defaults.ppr.as_ref())
            .and_then(|properties| properties.bidi)
    });
    if bidi == Some(true) {
        " dir=\"rtl\""
    } else {
        ""
    }
}

/// Emit the inner content of a paragraph (the runs of its accepted view and
/// hyperlinks).
fn emit_paragraph_content(
    out: &mut String,
    para: &CT_P,
    _styles: &CT_Styles,
    images: &HashMap<String, ImageData>,
    hyperlink_urls: &crate::Links<'_>,
    options: &HtmlOptions,
    equations: &dyn EquationWriter,
) {
    // The bookmarks a link targets, read before the accepted view drops
    // the markers.
    let anchors = crate::bookmark_anchors(para, &hyperlink_urls.bookmarks);
    // The runs the text readers read, those of content controls, tracked
    // insertions, smart tags and custom XML included, deleted ones left out.
    let para = &para.accepted_view();
    let hyperlink_map = crate::hyperlink_targets(para, hyperlink_urls);
    let emit_anchors = |out: &mut String, boundary: usize| {
        for (_, name) in anchors.iter().filter(|(at, _)| *at == boundary) {
            out.push_str(&format!("<a id=\"{}\"></a>", escape_html_attr(name)));
        }
    };

    let mut current_link: Option<&str> = None;

    for (run_idx, run) in para.runs.iter().enumerate() {
        let in_link = hyperlink_map.get(&run_idx).map(String::as_str);
        emit_anchors(out, run_idx);
        // An equation between a link and what follows it is not in the link.
        if crate::equations_at(para, run_idx).next().is_some() {
            if current_link.is_some() && current_link != in_link {
                out.push_str("</a>");
                current_link = None;
            }
            emit_equations(out, para, run_idx, equations);
        }

        // Open/close link tags as needed
        match (current_link, in_link) {
            (None, Some(url)) => {
                out.push_str(&format!("<a href=\"{}\">", escape_html_attr(url)));
                current_link = Some(url);
            }
            (Some(_), None) => {
                out.push_str("</a>");
                current_link = None;
            }
            (Some(old), Some(new)) if old != new => {
                out.push_str("</a>");
                out.push_str(&format!("<a href=\"{}\">", escape_html_attr(new)));
                current_link = Some(new);
            }
            _ => {}
        }

        emit_run(out, run, images, options);
    }

    // Close any open link
    if current_link.is_some() {
        out.push_str("</a>");
    }
    emit_anchors(out, para.runs.len());
    emit_equations(out, para, para.runs.len(), equations);
}

/// Write the equations at a run boundary as MathML, a display equation as
/// block `<math>` elements.
fn emit_equations(out: &mut String, para: &CT_P, boundary: usize, equations: &dyn EquationWriter) {
    for equation in crate::equations_at(para, boundary) {
        match equation {
            OfficeMath::Inline(math) => {
                if let Some(mathml) = equations.mathml(math) {
                    out.push_str(&mathml);
                }
            }
            OfficeMath::Display(display) => {
                equations.display(display);
                for math in &display.equations {
                    if let Some(mathml) = equations.mathml(math) {
                        out.push_str(&match mathml.strip_prefix("<math") {
                            Some(rest) => format!("<math display=\"block\"{rest}"),
                            None => mathml,
                        });
                    }
                }
            }
        }
    }
}

/// Emit a single run.
fn emit_run(
    out: &mut String,
    run: &CT_R,
    images: &HashMap<String, ImageData>,
    options: &HtmlOptions,
) {
    let tags = css::run_tags(run.properties.as_ref());
    let style = css::run_style(run.properties.as_ref());

    // Open semantic tags
    let has_style = !style.is_empty();
    if has_style {
        out.push_str(&format!("<span style=\"{}\">", escape_html_attr(&style)));
    }
    if tags.bold {
        out.push_str("<strong>");
    }
    if tags.italic {
        out.push_str("<em>");
    }
    if tags.underline {
        out.push_str("<u>");
    }
    if tags.strike {
        out.push_str("<s>");
    }
    if tags.superscript {
        out.push_str("<sup>");
    }
    if tags.subscript {
        out.push_str("<sub>");
    }

    // Emit content
    for content in &run.content {
        match content {
            RunContent::Text(t) | RunContent::DeletedText(t) => {
                out.push_str(&escape_html(&t.text));
            }
            RunContent::Tab => {
                out.push_str("&emsp;");
            }
            RunContent::Break(bt) => match bt {
                BreakType::Line => out.push_str("<br>"),
                BreakType::Page => out.push_str("<hr>"),
                BreakType::Column => out.push_str("<br>"),
            },
            RunContent::Drawing(drawing) => {
                if options.inline_images {
                    // Try to find image data
                    let picture = drawing
                        .inline
                        .as_ref()
                        .map(|i| (i.embed_id.as_str(), i.description.as_deref()))
                        .or_else(|| {
                            drawing
                                .anchor
                                .as_ref()
                                .map(|a| (a.embed_id.as_str(), a.description.as_deref()))
                        });

                    if let Some((eid, description)) = picture
                        && let Some(img_data) = images.get(eid)
                    {
                        emit_image(out, img_data, description);
                    }
                }
            }
            RunContent::Field(field) => {
                for (text, properties) in field.cached_display_segments() {
                    emit_field_display(out, text, properties);
                }
            }
            // A symbol is font-encoded rather than Unicode, so there is no
            // portable HTML spelling for it.
            RunContent::Symbol { .. } => {}
            RunContent::SpecialCharacter(character) => match character {
                SpecialCharacter::CarriageReturn => out.push_str("<br>"),
                SpecialCharacter::NoBreakHyphen => out.push_str("&#8209;"),
                SpecialCharacter::SoftHyphen => out.push_str("&shy;"),
                SpecialCharacter::PositionalTab { .. } => out.push_str("&emsp;"),
            },
            RunContent::FootnoteRef { .. }
            | RunContent::EndnoteRef { .. }
            | RunContent::CommentReference { .. } => {}
        }
    }

    // Close semantic tags (reverse order)
    if tags.subscript {
        out.push_str("</sub>");
    }
    if tags.superscript {
        out.push_str("</sup>");
    }
    if tags.strike {
        out.push_str("</s>");
    }
    if tags.underline {
        out.push_str("</u>");
    }
    if tags.italic {
        out.push_str("</em>");
    }
    if tags.bold {
        out.push_str("</strong>");
    }
    if has_style {
        out.push_str("</span>");
    }
}

fn emit_field_display(out: &mut String, text: &str, properties: Option<&CT_RPr>) {
    let tags = css::run_tags(properties);
    let style = css::run_style(properties);
    if !style.is_empty() {
        out.push_str(&format!("<span style=\"{}\">", escape_html_attr(&style)));
    }
    if tags.bold {
        out.push_str("<strong>");
    }
    if tags.italic {
        out.push_str("<em>");
    }
    if tags.underline {
        out.push_str("<u>");
    }
    if tags.strike {
        out.push_str("<s>");
    }
    if tags.superscript {
        out.push_str("<sup>");
    }
    if tags.subscript {
        out.push_str("<sub>");
    }
    for character in text.chars() {
        match character {
            '\t' => out.push_str("&emsp;"),
            '\n' | '\u{000b}' => out.push_str("<br>"),
            '\u{000c}' => out.push_str("<hr>"),
            character => out.push_str(&escape_html(&character.to_string())),
        }
    }
    if tags.subscript {
        out.push_str("</sub>");
    }
    if tags.superscript {
        out.push_str("</sup>");
    }
    if tags.strike {
        out.push_str("</s>");
    }
    if tags.underline {
        out.push_str("</u>");
    }
    if tags.italic {
        out.push_str("</em>");
    }
    if tags.bold {
        out.push_str("</strong>");
    }
    if !style.is_empty() {
        out.push_str("</span>");
    }
}

/// Emit an inline image as a base64 data URI, with the picture's
/// description as its alt text when it has one.
fn emit_image(out: &mut String, img: &ImageData, description: Option<&str>) {
    let base64 = base64_encode(&img.data);
    let alt = description
        .map(|description| format!(" alt=\"{}\"", escape_html_attr(description)))
        .unwrap_or_default();
    // `content_type` is a public field, so it is not necessarily one of the
    // literals oxoffice derives from the part name.
    out.push_str(&format!(
        "<img src=\"data:{};base64,{}\"{alt} style=\"max-width:100%\">",
        escape_html_attr(&img.content_type),
        base64
    ));
}

/// Emit a table.
fn emit_table(
    out: &mut String,
    tbl: &CT_Tbl,
    styles: &CT_Styles,
    images: &HashMap<String, ImageData>,
    hyperlink_urls: &crate::Links<'_>,
    options: &HtmlOptions,
    equations: &dyn EquationWriter,
) {
    if tbl.accepted_view_removes() {
        return;
    }
    let mut table_style = String::new();

    if let Some(props) = &tbl.properties {
        // Table alignment
        if let Some(jc) = &props.jc {
            match jc {
                ST_Jc::Center => table_style.push_str("margin-left:auto;margin-right:auto;"),
                ST_Jc::Right => table_style.push_str("margin-left:auto;"),
                _ => {}
            }
        }

        // Table borders
        // An edge set to `nil` or `none` draws nothing.
        if let Some(borders) = &props.borders
            && borders.draws_an_edge()
        {
            table_style.push_str("border:1px solid #000;");
        }
    }

    if table_style.is_empty() {
        out.push_str("<table>\n");
    } else {
        out.push_str(&format!(
            "<table style=\"{}\">\n",
            escape_html_attr(&table_style)
        ));
    }

    // Rows and cells that content controls wrap are written in place.
    let rows = tbl
        .rows()
        .into_iter()
        .filter(|row| !row.accepted_view_removes())
        .collect::<Vec<_>>();
    let limit = grid_limit(tbl);
    for (row_idx, row) in rows.iter().enumerate() {
        out.push_str("<tr>\n");
        // A row marked to repeat as a header holds header cells.
        let cell_tag = if row
            .properties
            .as_ref()
            .and_then(|properties| properties.header)
            == Some(true)
        {
            "th"
        } else {
            "td"
        };
        let ranges = grid_ranges(row, limit);
        for (cell, &range) in row.cells().into_iter().zip(&ranges) {
            // Skip vmerge continue cells
            if let Some(props) = &cell.properties
                && matches!(props.v_merge, Some(VMerge::Continue))
            {
                continue;
            }

            let mut td_attrs = String::new();
            let mut td_style = String::new();

            if let Some(props) = &cell.properties {
                // Column span
                if let Some(span) = props.grid_span
                    && span > 1
                {
                    td_attrs.push_str(&format!(" colspan=\"{span}\""));
                }

                // Row span (count consecutive vmerge continue cells below)
                if matches!(props.v_merge, Some(VMerge::Restart)) {
                    let rowspan = count_vmerge_span(&rows[row_idx + 1..], range, limit);
                    if rowspan > 1 {
                        td_attrs.push_str(&format!(" rowspan=\"{rowspan}\""));
                    }
                }

                // Vertical alignment
                if let Some(va) = &props.v_align {
                    // `both` distributes vertically in Word. HTML has no
                    // equivalent on a cell, and top is what this workspace
                    // already lays it out as.
                    let css_va = match va {
                        oxoffice_docx_oxml::table::ST_VerticalJc::Center => "middle",
                        oxoffice_docx_oxml::table::ST_VerticalJc::Bottom => "bottom",
                        oxoffice_docx_oxml::table::ST_VerticalJc::Top | _ => "top",
                    };
                    td_style.push_str(&format!("vertical-align:{css_va};"));
                }

                // Cell shading
                if let Some(shd) = &props.shading
                    && let Some(fill) = &shd.fill
                    && fill != "FFFFFF"
                    && let Some(color) = crate::sanitize::hex_color(fill)
                {
                    td_style.push_str(&format!("background-color:{color};"));
                }

                // Cell borders
                if let Some(borders) = &props.borders
                    && [&borders.top, &borders.bottom, &borders.left, &borders.right]
                        .into_iter()
                        .flatten()
                        .any(|edge| !edge.val.is_none())
                {
                    td_style.push_str("border:1px solid #000;");
                }
            }

            if td_style.is_empty() {
                out.push_str(&format!("<{cell_tag}{td_attrs}>"));
            } else {
                out.push_str(&format!(
                    "<{cell_tag}{td_attrs} style=\"{}\">",
                    escape_html_attr(&td_style)
                ));
            }

            for block in cell_blocks(cell) {
                match block {
                    Block::Paragraph(p, _) => {
                        emit_paragraph(out, &p, styles, images, hyperlink_urls, options, equations);
                    }
                    Block::Table(nested) => {
                        emit_table(
                            out,
                            nested,
                            styles,
                            images,
                            hyperlink_urls,
                            options,
                            equations,
                        );
                    }
                }
            }

            out.push_str(&format!("</{cell_tag}>\n"));
        }
        out.push_str("</tr>\n");
    }

    out.push_str("</table>\n");
}

/// The grid columns each cell of `row` covers, as `(start, end)`: the
/// columns the row omits before its first cell, then each cell's
/// `gridSpan`. Both are clamped to `limit`, the table's width, since the
/// counts come from the file unchecked.
pub(crate) fn grid_ranges(row: &CT_Row, limit: usize) -> Vec<(usize, usize)> {
    let mut column = (row
        .properties
        .as_ref()
        .and_then(|properties| properties.grid_before)
        .unwrap_or(0) as usize)
        .min(limit);
    row.cells()
        .into_iter()
        .map(|cell| {
            let start = column;
            let span = cell
                .properties
                .as_ref()
                .and_then(|properties| properties.grid_span)
                .unwrap_or(1)
                .max(1) as usize;
            column = column.saturating_add(span).min(limit);
            (start, column)
        })
        .collect()
}

/// The width of `table` in grid columns: its grid, or more if a row holds
/// more cells.
pub(crate) fn grid_limit(table: &CT_Tbl) -> usize {
    let grid = table.grid.as_ref().map_or(0, |grid| grid.columns.len());
    table
        .rows()
        .into_iter()
        .map(|row| row.cells().len())
        .fold(grid, usize::max)
}

/// How many rows a cell merged down over the grid columns `range` spans: the
/// rows below whose cell over the same columns continues the merge.
fn count_vmerge_span(rows_below: &[&CT_Row], range: (usize, usize), limit: usize) -> u32 {
    let mut span = 1;
    for row in rows_below {
        let cells = row.cells();
        if let Some(index) = grid_ranges(row, limit)
            .iter()
            .position(|&other| other == range)
            && let Some(props) = &cells[index].properties
            && matches!(props.v_merge, Some(VMerge::Continue))
        {
            span += 1;
            continue;
        }
        break;
    }

    span
}

/// Simple base64 encoding.
pub(crate) fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);
    let chunks = data.chunks(3);

    for chunk in chunks {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };

        let n = (b0 << 16) | (b1 << 8) | b2;

        result.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        result.push(ALPHABET[((n >> 12) & 63) as usize] as char);

        if chunk.len() > 1 {
            result.push(ALPHABET[((n >> 6) & 63) as usize] as char);
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(ALPHABET[(n & 63) as usize] as char);
        } else {
            result.push('=');
        }
    }

    result
}

fn emit_chart_table(out: &mut String, chart: &crate::ChartTable) {
    if let Some(title) = chart.title.as_ref().filter(|title| !title.is_empty()) {
        out.push_str(&format!(
            "<p class=\"chart-title\">{}</p>\n",
            escape_html(title)
        ));
    }
    out.push_str("<table class=\"chart-data\">\n");
    for (index, row) in chart.rows().iter().enumerate() {
        out.push_str("<tr>");
        let tag = if index == 0 { "th" } else { "td" };
        for cell in row {
            out.push_str(&format!("<{tag}>{}</{tag}>", escape_html(cell)));
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</table>\n");
}
