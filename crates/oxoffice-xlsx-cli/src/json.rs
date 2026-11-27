//! The JSON records of cells, styles, comments and page setups, schema 1.

use oxoffice_xlsx::{
    Border, BorderSide, Cell, Color, Comment, Fill, PageBreak, PageSetup, ResolvedStyle, Value,
};
use serde_json::{Map, Value as Json, json};

/// `type` and `value` of a cell value: a number, a text, a boolean, an error
/// code, or `null` with the type `empty`.
fn value(value: &Value) -> (&'static str, Json) {
    match value {
        Value::Empty => ("empty", Json::Null),
        Value::Number(number) => ("number", json!(number)),
        Value::Text(text) => ("text", json!(text)),
        Value::Boolean(boolean) => ("boolean", json!(boolean)),
        Value::Error(error) => ("error", json!(error.code())),
    }
}

/// A cell of `text --json`: its address, value and displayed text; for a
/// formula its displayed form, and for a formula or a cell of an array
/// formula's range whether its value may be stale.
pub fn text_cell(cell: &Cell) -> Json {
    let (kind, value) = value(&cell.value);
    let mut record = Map::new();
    record.insert("ref".to_owned(), json!(cell.address.to_string()));
    record.insert("type".to_owned(), json!(kind));
    record.insert("value".to_owned(), value);
    record.insert("text".to_owned(), json!(cell.text));
    if let Some(formula) = &cell.formula {
        record.insert("formula".to_owned(), json!(formula));
    }
    if cell.formula.is_some() || cell.formula_kind.is_some() {
        record.insert("stale".to_owned(), json!(cell.stale));
    }
    Json::Object(record)
}

/// A cell of `cells --json`: every field of the cell.
pub fn full_cell(cell: &Cell) -> Json {
    let (kind, value) = value(&cell.value);
    json!({
        "ref": cell.address.to_string(),
        "type": kind,
        "value": value,
        "text": cell.text,
        "formula": cell.formula,
        "stored_formula": cell.stored_formula,
        "formula_kind": cell.formula_kind.map(|kind| kind.as_str()),
        "array_range": cell.array_range.map(|range| range.to_string()),
        "stale": cell.stale,
        "number_format": cell.number_format,
        "style": style(&cell.style),
    })
}

fn color(color: &Option<Color>) -> Json {
    match color {
        Some(color) => json!({
            "rgb": color.rgb,
            "source": color.source.kind(),
            "index": color.source.index(),
            "tint": color.tint,
        }),
        None => Json::Null,
    }
}

fn side(side: &BorderSide) -> Json {
    json!({"style": side.style, "color": color(&side.color)})
}

fn border(border: &Border) -> Json {
    json!({
        "left": side(&border.left),
        "right": side(&border.right),
        "top": side(&border.top),
        "bottom": side(&border.bottom),
        "diagonal": side(&border.diagonal),
        "diagonal_up": border.diagonal_up,
        "diagonal_down": border.diagonal_down,
    })
}

fn fill(fill: &Fill) -> Json {
    match fill {
        Fill::Pattern {
            pattern,
            foreground,
            background,
        } => json!({
            "type": "pattern",
            "pattern": pattern,
            "foreground": color(foreground),
            "background": color(background),
        }),
        Fill::Gradient {
            kind,
            degree,
            left,
            right,
            top,
            bottom,
            stops,
        } => json!({
            "type": "gradient",
            "gradient": kind,
            "degree": degree,
            "left": left,
            "right": right,
            "top": top,
            "bottom": bottom,
            "stops": stops
                .iter()
                .map(|stop| json!({"position": stop.position, "color": color(&stop.color)}))
                .collect::<Vec<_>>(),
        }),
    }
}

/// A resolved style.
pub fn style(style: &ResolvedStyle) -> Json {
    let font = &style.font;
    let alignment = &style.alignment;
    json!({
        "number_format": style.number_format,
        "font": {
            "name": font.name,
            "size": font.size,
            "bold": font.bold,
            "italic": font.italic,
            "underline": font.underline,
            "strike": font.strike,
            "vertical_align": font.vertical_align,
            "color": color(&font.color),
        },
        "fill": fill(&style.fill),
        "border": border(&style.border),
        "alignment": {
            "horizontal": alignment.horizontal,
            "vertical": alignment.vertical,
            "wrap_text": alignment.wrap_text,
            "shrink_to_fit": alignment.shrink_to_fit,
            "indent": alignment.indent,
            "text_rotation": alignment.text_rotation,
            "reading_order": alignment.reading_order,
        },
        "protection": {
            "locked": style.protection.locked,
            "hidden": style.protection.hidden,
        },
        "style_name": style.style_name,
    })
}

/// A note or a thread with its replies.
pub fn comment(comment: &Comment) -> Json {
    json!({
        "ref": comment.address.to_string(),
        "kind": comment.kind.as_str(),
        "author": comment.author,
        "date": comment.date,
        "text": comment.text,
        "resolved": comment.resolved,
        "replies": comment
            .replies
            .iter()
            .map(|reply| json!({"author": reply.author, "date": reply.date, "text": reply.text}))
            .collect::<Vec<_>>(),
    })
}

fn breaks(breaks: &[PageBreak]) -> Json {
    breaks
        .iter()
        .map(|brk| json!({"id": brk.id, "min": brk.min, "max": brk.max, "manual": brk.manual}))
        .collect()
}

/// A sheet's page setup, schema defaults filled.
pub fn page_setup(setup: &PageSetup) -> Json {
    let header_footer = &setup.header_footer;
    json!({
        "margins": setup.margins.map(|margins| json!({
            "left": margins.left,
            "right": margins.right,
            "top": margins.top,
            "bottom": margins.bottom,
            "header": margins.header,
            "footer": margins.footer,
        })),
        "paper_size": setup.paper_size,
        "paper_width": setup.paper_width,
        "paper_height": setup.paper_height,
        "orientation": setup.orientation.as_str(),
        "scale": setup.scale,
        "fit_to_page": setup.fit_to_page,
        "fit_to_width": setup.fit_to_width,
        "fit_to_height": setup.fit_to_height,
        "first_page_number": setup.first_page_number,
        "use_first_page_number": setup.use_first_page_number,
        "page_order": setup.page_order.as_str(),
        "black_and_white": setup.black_and_white,
        "draft": setup.draft,
        "cell_comments": setup.cell_comments.as_str(),
        "errors": setup.errors.as_str(),
        "horizontal_dpi": setup.horizontal_dpi,
        "vertical_dpi": setup.vertical_dpi,
        "copies": setup.copies,
        "horizontal_centered": setup.horizontal_centered,
        "vertical_centered": setup.vertical_centered,
        "headings": setup.headings,
        "grid_lines": setup.grid_lines,
        "header_footer": {
            "odd_header": header_footer.odd_header,
            "odd_footer": header_footer.odd_footer,
            "even_header": header_footer.even_header,
            "even_footer": header_footer.even_footer,
            "first_header": header_footer.first_header,
            "first_footer": header_footer.first_footer,
            "different_odd_even": header_footer.different_odd_even,
            "different_first": header_footer.different_first,
            "scale_with_doc": header_footer.scale_with_doc,
            "align_with_margins": header_footer.align_with_margins,
        },
        "row_breaks": breaks(&setup.row_breaks),
        "column_breaks": breaks(&setup.column_breaks),
        "print_area": setup.print_area,
        "print_titles": setup.print_titles,
    })
}
