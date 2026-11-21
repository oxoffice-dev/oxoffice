//! HTML element generation from OXML types.

use std::collections::HashMap;

use oxoffice_docx_oxml::document::CT_Body;
use oxoffice_docx_oxml::numbering::CT_Numbering;
use oxoffice_docx_oxml::properties::{CT_PPr, CT_RPr};
use oxoffice_docx_oxml::shared::ST_Jc;
use oxoffice_docx_oxml::styles::CT_Styles;
use oxoffice_docx_oxml::table::{CT_Row, CT_Tbl, VMerge};
use oxoffice_docx_oxml::text::{BreakType, CT_P, CT_R, RunContent, SpecialCharacter};

use crate::css;
use crate::sanitize::{escape_html, escape_html_attr, safe_url};
use crate::{Block, HtmlOptions, ImageData, body_blocks, cell_blocks};

/// Emit the full body content as HTML.
pub(crate) fn emit_body(
    body: &CT_Body,
    styles: &CT_Styles,
    numbering: Option<&CT_Numbering>,
    images: &HashMap<String, ImageData>,
    hyperlink_urls: &HashMap<String, String>,
    options: &HtmlOptions,
    charts: &HashMap<String, crate::ChartTable>,
) -> String {
    let mut out = String::new();
    let mut list_stack: Vec<ListState> = Vec::new();
    // The list and level of each open item, outermost first.
    let mut open_items: Vec<(u32, u32)> = Vec::new();

    for block in body_blocks(body, false) {
        match block {
            Block::Paragraph(p, _) => {
                // A heading numbered through its style, as Word numbers its
                // headings, stays a heading; only its own numbering makes it
                // a list item.
                let list_info = detect_list(&p, styles, numbering).filter(|_| {
                    detect_heading_level(p.properties.as_ref(), styles).is_none()
                        || crate::has_own_numbering(&p)
                });

                // A paragraph on a `none` level goes on the open item of its
                // list at its level or above, and that list stays open.
                let continued = if list_info.is_none()
                    && detect_heading_level(p.properties.as_ref(), styles).is_none()
                {
                    crate::unmarked_list_level(&p, styles, numbering).and_then(|(num_id, level)| {
                        open_items
                            .iter()
                            .rposition(|&(id, item)| id == num_id && item <= level)
                    })
                } else {
                    None
                };

                // Close lists that are no longer active
                while continued.is_none()
                    && let Some(top) = list_stack.last()
                {
                    if let Some((_, level, _)) = &list_info {
                        if *level <= top.level && top.level > 0 {
                            close_list(&mut out, &mut list_stack);
                        } else {
                            break;
                        }
                    } else {
                        close_list(&mut out, &mut list_stack);
                    }
                }

                if let Some(index) = continued {
                    if index + 1 == open_items.len() && out.ends_with("</li>\n") {
                        // Inside the item just written.
                        out.truncate(out.len() - "</li>\n".len());
                        out.push_str("<p>");
                        emit_paragraph_content(
                            &mut out,
                            &p,
                            styles,
                            images,
                            hyperlink_urls,
                            options,
                        );
                        out.push_str("</p></li>\n");
                    } else {
                        // After the deeper lists, in the list of that item.
                        let depth = open_items[index].1 as usize + 1;
                        while list_stack.len() > depth {
                            close_list(&mut out, &mut list_stack);
                        }
                        open_items.truncate(index + 1);
                        out.push_str("<p>");
                        emit_paragraph_content(
                            &mut out,
                            &p,
                            styles,
                            images,
                            hyperlink_urls,
                            options,
                        );
                        out.push_str("</p>\n");
                    }
                } else if let Some((is_ordered, level, num_id)) = list_info {
                    // Open new list levels if needed
                    let current_depth = list_stack.len() as u32;
                    if current_depth <= level {
                        for _ in current_depth..=level {
                            let tag = if is_ordered { "ol" } else { "ul" };
                            out.push_str(&format!("<{tag}>\n"));
                            list_stack.push(ListState {
                                ordered: is_ordered,
                                level,
                            });
                        }
                    }
                    out.push_str(&format!("<li{}>", direction_attribute(&p, styles)));
                    emit_paragraph_content(&mut out, &p, styles, images, hyperlink_urls, options);
                    out.push_str("</li>\n");
                    while open_items.last().is_some_and(|&(_, item)| item >= level) {
                        open_items.pop();
                    }
                    open_items.push((num_id, level));
                } else {
                    // Close all remaining lists
                    while !list_stack.is_empty() {
                        close_list(&mut out, &mut list_stack);
                    }
                    open_items.clear();
                    emit_paragraph(&mut out, &p, styles, images, hyperlink_urls, options);
                }
                for id in crate::paragraph_chart_ids(&p) {
                    if let Some(chart) = charts.get(&id) {
                        while !list_stack.is_empty() {
                            close_list(&mut out, &mut list_stack);
                        }
                        open_items.clear();
                        emit_chart_table(&mut out, chart);
                    }
                }
            }
            Block::Table(tbl) => {
                while !list_stack.is_empty() {
                    close_list(&mut out, &mut list_stack);
                }
                open_items.clear();
                emit_table(&mut out, tbl, styles, images, hyperlink_urls, options);
            }
        }
    }

    // Close remaining lists
    while !list_stack.is_empty() {
        close_list(&mut out, &mut list_stack);
    }

    out
}

struct ListState {
    ordered: bool,
    level: u32,
}

fn close_list(out: &mut String, stack: &mut Vec<ListState>) {
    if let Some(state) = stack.pop() {
        let tag = if state.ordered { "ol" } else { "ul" };
        out.push_str(&format!("</{tag}>\n"));
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

/// Detect if a paragraph is a list item, through its style too. Returns
/// (is_ordered, ilvl, num_id).
fn detect_list(
    para: &CT_P,
    styles: &CT_Styles,
    numbering: Option<&CT_Numbering>,
) -> Option<(bool, u32, u32)> {
    let (num_id, ilvl, level, _) = crate::list_level(para, styles, numbering?)?;

    use oxoffice_docx_oxml::numbering::ST_NumberFormat;
    let is_ordered = match level.num_fmt.as_ref() {
        Some(ST_NumberFormat::Bullet) => false,
        None
        | Some(
            ST_NumberFormat::Decimal
            | ST_NumberFormat::UpperRoman
            | ST_NumberFormat::LowerRoman
            | ST_NumberFormat::UpperLetter
            | ST_NumberFormat::LowerLetter
            | ST_NumberFormat::Ordinal,
        ) => true,
        // A `none` level draws no marker, as in layout: not a list item.
        Some(_) => return None,
    };

    Some((is_ordered, ilvl, num_id))
}

/// Emit a complete paragraph element.
fn emit_paragraph(
    out: &mut String,
    para: &CT_P,
    styles: &CT_Styles,
    images: &HashMap<String, ImageData>,
    hyperlink_urls: &HashMap<String, String>,
    options: &HtmlOptions,
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

    emit_paragraph_content(out, para, styles, images, hyperlink_urls, options);

    out.push_str(&format!("</{tag}>\n"));
}

/// ` dir="rtl"` for a right-to-left paragraph (`w:bidi`, its own or its
/// style's), so that a browser lays it out right to left; empty otherwise.
fn direction_attribute(para: &CT_P, styles: &CT_Styles) -> &'static str {
    let properties = para.properties.as_ref();
    let bidi = properties
        .and_then(|properties| properties.bidi)
        .or_else(|| {
            properties
                .and_then(|properties| properties.style_id.as_deref())
                .and_then(|id| styles.get_by_id(id))
                .and_then(|style| style.ppr.as_ref())
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
    hyperlink_urls: &HashMap<String, String>,
    options: &HtmlOptions,
) {
    // The runs the text readers read, those of content controls, tracked
    // insertions, smart tags and custom XML included, deleted ones left out.
    let para = &para.accepted_view();

    // Build a map of which runs are inside hyperlinks
    let mut hyperlink_map: HashMap<usize, &str> = HashMap::new();
    for hl in &para.hyperlinks {
        // A target with an unsafe scheme yields no <a> at all, so the run text
        // still renders but cannot become a script trigger.
        if let Some(rel_id) = &hl.rel_id
            && let Some(url) = hyperlink_urls
                .get(rel_id)
                .map(String::as_str)
                .and_then(safe_url)
        {
            for i in hl.run_start..hl.run_end {
                hyperlink_map.insert(i, url);
            }
        }
    }

    let mut current_link: Option<&str> = None;

    for (run_idx, run) in para.runs.iter().enumerate() {
        let in_link = hyperlink_map.get(&run_idx).copied();

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
    hyperlink_urls: &HashMap<String, String>,
    options: &HtmlOptions,
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
    for (row_idx, row) in rows.iter().enumerate() {
        out.push_str("<tr>\n");
        for (col_idx, cell) in row.cells().into_iter().enumerate() {
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
                    let rowspan = count_vmerge_span(&rows[row_idx + 1..], col_idx);
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
                out.push_str(&format!("<td{td_attrs}>"));
            } else {
                out.push_str(&format!(
                    "<td{td_attrs} style=\"{}\">",
                    escape_html_attr(&td_style)
                ));
            }

            for block in cell_blocks(cell) {
                match block {
                    Block::Paragraph(p, _) => {
                        emit_paragraph(out, &p, styles, images, hyperlink_urls, options);
                    }
                    Block::Table(nested) => {
                        emit_table(out, nested, styles, images, hyperlink_urls, options);
                    }
                }
            }

            out.push_str("</td>\n");
        }
        out.push_str("</tr>\n");
    }

    out.push_str("</table>\n");
}

/// Count the rowspan for a vmerge restart cell in column `col_idx`, from the
/// rows below it.
fn count_vmerge_span(rows_below: &[&CT_Row], col_idx: usize) -> u32 {
    let mut span = 1;
    for row in rows_below {
        if let Some(next_cell) = row.cells().get(col_idx)
            && let Some(props) = &next_cell.properties
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
