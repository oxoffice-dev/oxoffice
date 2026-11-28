//! Styles written: the parts of a cell's style an edit changes, built into a
//! cell format that reuses an identical record, else appends one.
//!
//! A style edit takes the cell's `cellXfs` record, changes the parts the
//! patch names (its font, fill, border, alignment or number format, each
//! with its `apply*` set, as Excel writes them) and finds the font, fill,
//! border and cell format records it needs among those the part has,
//! compared on what they say (prefixes, attribute order and the spellings of
//! `true` and `1.0` aside), or by their bytes when they hold what the model
//! does not know (`extLst`). A record is appended only when none matches, so
//! the indexes of every other cell stay valid. A number format reuses a
//! built-in id only when every language of Excel shows it the same, else an
//! identical custom `numFmt`, else takes a new id from 164 up. Colours are
//! written as `rgb`, `FF` and the six hex digits.

use std::collections::HashMap;

use oxoffice_numfmt::{Locale, NumberFormat, builtin_code, is_locale_independent};
use oxoffice_xlsx_oxml::styles::{StyleList, StylesXml};
use oxoffice_xlsx_oxml::{CellRef, Element, Node, format_f64, qualified, xstring};

use crate::style::StyleTable;
use crate::write::{CellWrite, Region, StyleChange};
use crate::{Error, Result, Workbook, WorksheetMut};

/// The most cells a style edit writes at once: a cell an edit names is
/// written even when it holds nothing.
const MAX_STYLED_CELLS: u64 = 1_000_000;

/// The longest number format code Excel reads.
const MAX_FORMAT_CODE: usize = 255;

/// The parts of a cell's style an edit changes; `None` leaves a part as it
/// is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StylePatch {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    /// A single underline, or none.
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    pub font_name: Option<String>,
    /// In points, 1 to 409.
    pub font_size: Option<f64>,
    /// `RRGGBB`, with a leading `#` or not.
    pub font_color: Option<String>,
    /// A solid fill of this colour, `RRGGBB`.
    pub fill: Option<String>,
    /// The style of the four sides of the border: `none`, `thin`, `medium`,
    /// `thick`, `double`, `dashed`, `dotted`, `hair`...
    pub border: Option<String>,
    /// The colour of the four sides, `RRGGBB`; automatic when not given.
    pub border_color: Option<String>,
    /// `general`, `left`, `center`, `right`, `fill`, `justify`,
    /// `centerContinuous` or `distributed`.
    pub horizontal: Option<String>,
    /// `top`, `center`, `bottom`, `justify` or `distributed`.
    pub vertical: Option<String>,
    pub wrap_text: Option<bool>,
    /// A number format code (`0.00`, `yyyy-mm-dd`).
    pub number_format: Option<String>,
}

const HORIZONTAL: [&str; 8] = [
    "general",
    "left",
    "center",
    "right",
    "fill",
    "justify",
    "centerContinuous",
    "distributed",
];
const VERTICAL: [&str; 5] = ["top", "center", "bottom", "justify", "distributed"];
const BORDER_STYLES: [&str; 14] = [
    "none",
    "thin",
    "medium",
    "dashed",
    "dotted",
    "thick",
    "double",
    "hair",
    "mediumDashed",
    "dashDot",
    "mediumDashDot",
    "dashDotDot",
    "mediumDashDotDot",
    "slantDashDot",
];

/// The children of a font, in the order Excel writes them.
const FONT_ORDER: [&str; 15] = [
    "b",
    "i",
    "strike",
    "condense",
    "extend",
    "outline",
    "shadow",
    "u",
    "vertAlign",
    "sz",
    "color",
    "name",
    "family",
    "charset",
    "scheme",
];
/// The sides of a border, in schema order, each writing beside its twin.
const BORDER_ORDER: [&str; 9] = [
    "left",
    "start",
    "right",
    "end",
    "top",
    "bottom",
    "diagonal",
    "vertical",
    "horizontal",
];
/// The children of a cell format, in schema order.
const XF_ORDER: [&str; 3] = ["alignment", "protection", "extLst"];

/// `RRGGBB` or `#RRGGBB`, as `FFRRGGBB`.
fn argb(color: &str, what: &str) -> Result<String> {
    let hex = color.strip_prefix('#').unwrap_or(color);
    if hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(format!("FF{}", hex.to_ascii_uppercase()))
    } else {
        Err(Error::InvalidArgument(format!(
            "{what} is a colour of six hex digits such as \"FF0000\", not {color:?}"
        )))
    }
}

impl StylePatch {
    /// Refuses a value Excel would not write.
    pub(crate) fn check(&self) -> Result<()> {
        let invalid = |message: String| Err(Error::InvalidArgument(message));
        if let Some(size) = self.font_size
            && !(size.is_finite() && (1.0..=409.0).contains(&size))
        {
            return invalid(format!("a font size is from 1 to 409 points, not {size}"));
        }
        if let Some(name) = &self.font_name
            && (name.is_empty() || name.chars().count() > 31)
        {
            return invalid(format!("a font name has 1 to 31 characters, not {name:?}"));
        }
        for (color, what) in [
            (&self.font_color, "font_color"),
            (&self.fill, "fill"),
            (&self.border_color, "border_color"),
        ] {
            if let Some(color) = color {
                argb(color, what)?;
            }
        }
        if let Some(border) = &self.border
            && !BORDER_STYLES.contains(&border.as_str())
        {
            return invalid(format!(
                "{border:?} is not a border style: write one of {}",
                BORDER_STYLES.join(", ")
            ));
        }
        if self.border_color.is_some() && self.border.is_none() {
            return invalid("border_color goes with border, the style of the sides".to_owned());
        }
        if let Some(horizontal) = &self.horizontal
            && !HORIZONTAL.contains(&horizontal.as_str())
        {
            return invalid(format!(
                "{horizontal:?} is not a horizontal alignment: write one of {}",
                HORIZONTAL.join(", ")
            ));
        }
        if let Some(vertical) = &self.vertical
            && !VERTICAL.contains(&vertical.as_str())
        {
            return invalid(format!(
                "{vertical:?} is not a vertical alignment: write one of {}",
                VERTICAL.join(", ")
            ));
        }
        if let Some(code) = &self.number_format {
            check_number_format(code)?;
        }
        Ok(())
    }

    fn touches_font(&self) -> bool {
        self.bold.is_some()
            || self.italic.is_some()
            || self.underline.is_some()
            || self.strike.is_some()
            || self.font_name.is_some()
            || self.font_size.is_some()
            || self.font_color.is_some()
    }

    fn touches_alignment(&self) -> bool {
        self.horizontal.is_some() || self.vertical.is_some() || self.wrap_text.is_some()
    }
}

/// Refuses a number format code Excel would show as General or repair.
fn check_number_format(code: &str) -> Result<()> {
    if code.is_empty() || code.chars().count() > MAX_FORMAT_CODE {
        return Err(Error::InvalidArgument(format!(
            "a number format code has 1 to {MAX_FORMAT_CODE} characters"
        )));
    }
    NumberFormat::parse(code).map_err(|error| {
        Error::InvalidArgument(format!(
            "{code:?} is not a number format code Excel reads: {error}"
        ))
    })?;
    Ok(())
}

/// What a record says, as a key: its local names, its attributes sorted with
/// `true` and `1.0` written `1` and `1`, its children in order; or its bytes
/// when it holds what the model does not know.
fn key(record: &Element) -> String {
    fn unmodelled(element: &Element) -> bool {
        element.children.iter().any(|child| match child {
            Node::Element(child) => child.tag.is("extLst") || unmodelled(child),
            Node::Raw(_) => true,
            Node::Text(_) => false,
        })
    }
    fn normalized(value: &str) -> String {
        match value {
            "true" => "1".to_owned(),
            "false" => "0".to_owned(),
            _ => match value.split_once('.') {
                Some((whole, zeros))
                    if !whole.is_empty()
                        && whole.bytes().all(|byte| byte.is_ascii_digit())
                        && !zeros.is_empty()
                        && zeros.bytes().all(|byte| byte == b'0') =>
                {
                    whole.to_owned()
                }
                _ => value.to_owned(),
            },
        }
    }
    fn write(element: &Element, out: &mut String) {
        out.push('<');
        out.push_str(&element.tag.local_name());
        let mut attributes: Vec<(String, String)> = element
            .tag
            .attributes_except(&[])
            .into_iter()
            .filter(|(name, _)| name != "xmlns" && !name.starts_with("xmlns:"))
            .map(|(name, value)| (name, normalized(&value)))
            .collect();
        attributes.sort();
        for (name, value) in attributes {
            out.push_str(&format!(" {name}={value:?}"));
        }
        out.push('>');
        for child in &element.children {
            match child {
                Node::Element(child) => write(child, out),
                Node::Text(text) => out.push_str(&format!("{text:?}")),
                Node::Raw(bytes) => out.push_str(&String::from_utf8_lossy(bytes)),
            }
        }
        out.push_str("</>");
    }
    if unmodelled(record) {
        return format!("raw {}", String::from_utf8_lossy(&record.to_xml()));
    }
    let mut out = String::new();
    write(record, &mut out);
    out
}

/// The records of the styles part by what they say: what finds an identical
/// one. Built at the first style edit, kept up to date by the records the
/// edits append.
#[derive(Clone, Debug, Default)]
pub(crate) struct StyleIndex {
    lists: HashMap<&'static str, HashMap<String, u32>>,
    /// The custom number formats by code, the first of each.
    codes: HashMap<String, u32>,
    /// The ids of the custom number formats.
    custom_ids: HashMap<u32, String>,
    next_id: u32,
}

const INDEXED: [(StyleList, &str); 4] = [
    (StyleList::Fonts, "fonts"),
    (StyleList::Fills, "fills"),
    (StyleList::Borders, "borders"),
    (StyleList::CellXfs, "cellXfs"),
];

impl StyleIndex {
    fn new(styles: &StylesXml) -> Self {
        let mut index = StyleIndex::default();
        for (list, name) in INDEXED {
            let keys = index.lists.entry(name).or_default();
            for (position, record) in styles.list(list).iter().enumerate() {
                keys.entry(key(record)).or_insert(position as u32);
            }
        }
        let mut highest = 163;
        for format in styles.num_fmts() {
            if let Some(id) = format.id() {
                let code = xstring::decode(&format.code()).into_owned();
                index.codes.entry(code.clone()).or_insert(id);
                index.custom_ids.entry(id).or_insert(code);
                highest = highest.max(id);
            }
        }
        index.next_id = highest + 1;
        index
    }
}

fn list_name(list: StyleList) -> &'static str {
    INDEXED
        .iter()
        .find(|(known, _)| *known == list)
        .map_or("", |(_, name)| name)
}

/// What a style edit writes into: the styles part, its resolved formats and
/// the index of its records.
struct StyleEditor<'a> {
    styles: &'a mut StylesXml,
    table: &'a mut StyleTable,
    index: &'a mut StyleIndex,
}

impl StyleEditor<'_> {
    fn element(&self, local: &str) -> Element {
        Element::new(&qualified(self.styles.prefix(), local))
    }

    /// The index of a record of `list` saying what `record` says, appended
    /// when none does.
    fn record(&mut self, list: StyleList, record: Element) -> u32 {
        let keys = self.index.lists.entry(list_name(list)).or_default();
        let key = key(&record);
        if let Some(&found) = keys.get(&key) {
            return found;
        }
        let position = self.styles.push(list, record);
        keys.insert(key, position as u32);
        if list == StyleList::CellXfs {
            self.table.add_format(self.styles, position);
        }
        position as u32
    }

    /// The id of a number format showing `code`.
    fn number_format(&mut self, code: &str) -> u32 {
        if let Some(id) = (0..50).find(|&id| {
            is_locale_independent(id)
                && builtin_code(id, Locale::EnUs) == Some(code)
                && self
                    .index
                    .custom_ids
                    .get(&id)
                    .is_none_or(|custom| custom == code)
        }) {
            return id;
        }
        if let Some(&id) = self.index.codes.get(code) {
            return id;
        }
        let id = self.index.next_id;
        self.index.next_id += 1;
        let mut record = self.element("numFmt");
        record.tag.set("numFmtId", Some(&id.to_string()));
        record.tag.set("formatCode", Some(&xstring::encode(code)));
        self.styles.push(StyleList::NumFmts, record);
        self.index.codes.insert(code.to_owned(), id);
        self.index.custom_ids.insert(id, code.to_owned());
        self.table.add_number_format(id, code);
        id
    }

    /// The cell format of record `s` with `patch` made.
    fn patched(&mut self, s: u32, patch: &StylePatch) -> u32 {
        let records = self.styles.list(StyleList::CellXfs);
        let original = records.get(s as usize).or_else(|| records.get(0)).cloned();
        let mut xf = original.clone().unwrap_or_else(|| {
            let mut xf = self.element("xf");
            for attribute in ["numFmtId", "fontId", "fillId", "borderId", "xfId"] {
                xf.tag.set(attribute, Some("0"));
            }
            xf
        });
        if patch.touches_font() {
            let fonts = self.styles.list(StyleList::Fonts);
            let current = xf.tag.u32("fontId").unwrap_or(0) as usize;
            let mut font = fonts
                .get(current)
                .or_else(|| fonts.get(0))
                .cloned()
                .unwrap_or_else(|| self.element("font"));
            self.patch_font(&mut font, patch);
            let id = self.record(StyleList::Fonts, font);
            xf.tag.set("fontId", Some(&id.to_string()));
            xf.tag.set("applyFont", Some("1"));
        }
        if let Some(color) = &patch.fill {
            let fill = self.solid_fill(color);
            let id = self.record(StyleList::Fills, fill);
            xf.tag.set("fillId", Some(&id.to_string()));
            xf.tag.set("applyFill", Some("1"));
        }
        if let Some(style) = &patch.border {
            let borders = self.styles.list(StyleList::Borders);
            let current = xf.tag.u32("borderId").unwrap_or(0) as usize;
            let mut border = borders
                .get(current)
                .cloned()
                .unwrap_or_else(|| self.element("border"));
            self.patch_border(&mut border, style, patch.border_color.as_deref());
            let id = self.record(StyleList::Borders, border);
            xf.tag.set("borderId", Some(&id.to_string()));
            xf.tag.set("applyBorder", Some("1"));
        }
        if patch.touches_alignment() {
            let mut alignment = xf
                .child("alignment")
                .cloned()
                .unwrap_or_else(|| self.element("alignment"));
            if let Some(horizontal) = &patch.horizontal {
                alignment.tag.set("horizontal", Some(horizontal));
            }
            if let Some(vertical) = &patch.vertical {
                alignment.tag.set("vertical", Some(vertical));
            }
            if let Some(wrap) = patch.wrap_text {
                alignment.tag.set("wrapText", wrap.then_some("1"));
            }
            place_child(&mut xf, alignment, &XF_ORDER);
            xf.tag.set("applyAlignment", Some("1"));
        }
        if let Some(code) = &patch.number_format {
            let id = self.number_format(code);
            xf.tag.set("numFmtId", Some(&id.to_string()));
            xf.tag.set("applyNumberFormat", Some("1"));
        }
        // A patch that says what the record says already changes nothing,
        // whatever its `apply*`.
        let without_apply = |xf: &Element| {
            let mut xf = xf.clone();
            for (name, _) in xf.tag.attributes_except(&[]) {
                if name.starts_with("apply") {
                    xf.tag.set(&name, None);
                }
            }
            key(&xf)
        };
        if original
            .as_ref()
            .is_some_and(|original| without_apply(original) == without_apply(&xf))
        {
            return s;
        }
        self.record(StyleList::CellXfs, xf)
    }

    fn patch_font(&self, font: &mut Element, patch: &StylePatch) {
        for (local, set) in [
            ("b", patch.bold),
            ("i", patch.italic),
            ("strike", patch.strike),
            ("u", patch.underline),
        ] {
            match set {
                Some(true) => place_child(font, self.element(local), &FONT_ORDER),
                Some(false) => remove_child(font, local),
                None => {}
            }
        }
        if let Some(size) = patch.font_size {
            let mut sz = self.element("sz");
            sz.tag.set("val", Some(&format_f64(size)));
            place_child(font, sz, &FONT_ORDER);
        }
        if let Some(color) = &patch.font_color {
            let mut element = self.element("color");
            element.tag.set(
                "rgb",
                Some(&argb(color, "font_color").expect("a checked colour")),
            );
            place_child(font, element, &FONT_ORDER);
        }
        if let Some(name) = &patch.font_name {
            let mut element = self.element("name");
            element.tag.set("val", Some(name));
            place_child(font, element, &FONT_ORDER);
            // A theme font's scheme would win over the name, and its family
            // and character set describe the font it was.
            for local in ["scheme", "family", "charset"] {
                remove_child(font, local);
            }
        }
    }

    /// A solid fill of `color`, as Excel writes one.
    fn solid_fill(&self, color: &str) -> Element {
        let mut foreground = self.element("fgColor");
        foreground
            .tag
            .set("rgb", Some(&argb(color, "fill").expect("a checked colour")));
        let mut background = self.element("bgColor");
        background.tag.set("indexed", Some("64"));
        let mut pattern = self.element("patternFill");
        pattern.tag.set("patternType", Some("solid"));
        pattern.children = vec![Node::Element(foreground), Node::Element(background)];
        let mut fill = self.element("fill");
        fill.children.push(Node::Element(pattern));
        fill
    }

    /// The four sides of `border` in `style`, of `color` or automatic.
    fn patch_border(&self, border: &mut Element, style: &str, color: Option<&str>) {
        for sides in [
            ["left", "start"],
            ["right", "end"],
            ["top", "top"],
            ["bottom", "bottom"],
        ] {
            let local = sides
                .into_iter()
                .find(|local| border.child(local).is_some())
                .unwrap_or(sides[0]);
            let mut side = self.element(local);
            if style != "none" {
                side.tag.set("style", Some(style));
                let mut element = self.element("color");
                match color {
                    Some(color) => element.tag.set(
                        "rgb",
                        Some(&argb(color, "border_color").expect("a checked colour")),
                    ),
                    None => element.tag.set("indexed", Some("64")),
                }
                side.children.push(Node::Element(element));
            }
            place_child(border, side, &BORDER_ORDER);
        }
    }
}

/// Puts `child` in `parent`: in place of the first child of its name, else
/// before the first child `order` puts after it, else at the end.
fn place_child(parent: &mut Element, child: Element, order: &[&str]) {
    let local = child.tag.local_name();
    if let Some(existing) = parent.children.iter_mut().find_map(|node| match node {
        Node::Element(element) if element.tag.is(&local) => Some(element),
        _ => None,
    }) {
        *existing = child;
        return;
    }
    let rank = |name: &str| order.iter().position(|known| *known == name);
    let own = rank(&local);
    let at = parent
        .children
        .iter()
        .position(|node| match node {
            Node::Element(element) => {
                matches!((rank(&element.tag.local_name()), own), (Some(other), Some(own)) if other > own)
            }
            _ => false,
        })
        .unwrap_or(parent.children.len());
    parent.children.insert(at, Node::Element(child));
}

fn remove_child(parent: &mut Element, local: &str) {
    parent
        .children
        .retain(|node| !matches!(node, Node::Element(element) if element.tag.is(local)));
}

impl Workbook {
    /// The cell format of record `s` with `patch` made: an identical record,
    /// else one appended with the font, fill, border and number format it
    /// needs.
    pub(crate) fn styled(&mut self, s: u32, patch: &StylePatch) -> Result<u32> {
        self.style_table()?;
        if self.styles_xml()?.is_none() {
            return Err(Error::Refused(
                "the workbook has no styles part, where a cell's format is written".to_owned(),
            ));
        }
        let Some(Ok(Some(styles))) = self.read.styles_xml.get_mut() else {
            unreachable!("the styles were just read");
        };
        let Some(Ok(table)) = self.read.styles.get_mut() else {
            unreachable!("the formats were just read");
        };
        let index = self
            .edit
            .styles
            .get_or_insert_with(|| StyleIndex::new(styles));
        let mut editor = StyleEditor {
            styles,
            table: std::sync::Arc::make_mut(table),
            index,
        };
        Ok(editor.patched(s, patch))
    }
}

impl WorksheetMut<'_> {
    /// Changes the parts of the style of every cell of `range` that `patch`
    /// names (`A1:F1`, `B3`, whole columns `A:C` or rows `2:5` clipped to
    /// the used range), at most 1,000,000 cells at once: a cell the sheet
    /// does not write is written, with its style. The records are reused
    /// when identical ones exist, else appended (see the
    /// [module](crate::format)).
    pub fn set_style(&mut self, range: &str, patch: &StylePatch) -> Result<()> {
        patch.check()?;
        self.require_worksheet("cells")?;
        let Some(rectangle) = self.read().range(range)? else {
            return Ok(());
        };
        let count = (u64::from(rectangle.last.row - rectangle.first.row) + 1)
            * (u64::from(rectangle.last.column - rectangle.first.column) + 1);
        if count > MAX_STYLED_CELLS {
            return Err(Error::Refused(format!(
                "{rectangle} holds {count} cells, and at most {MAX_STYLED_CELLS} are styled at once"
            )));
        }
        if *patch == StylePatch::default() {
            return Ok(());
        }
        let mut writes = Vec::new();
        for row in rectangle.first.row..=rectangle.last.row {
            for column in rectangle.first.column..=rectangle.last.column {
                writes.push(CellWrite {
                    at: CellRef::new(row, column),
                    content: None,
                    date_code: None,
                    style: StyleChange::Patch,
                });
            }
        }
        self.apply(writes, Region::Cells, Some(patch))
    }

    /// Sets the number format code of every cell of `range` (`0.00`,
    /// `yyyy-mm-dd`), as [`WorksheetMut::set_style`] sets a style.
    pub fn set_number_format(&mut self, range: &str, code: &str) -> Result<()> {
        self.set_style(
            range,
            &StylePatch {
                number_format: Some(code.to_owned()),
                ..StylePatch::default()
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_key_reads_what_it_says_and_keeps_what_the_model_does_not_know() {
        let key_of = |xml: &str| key(&Element::parse(xml.as_bytes()).unwrap());
        for (a, b, same) in [
            (
                r#"<font><b/><sz val="11"/></font>"#,
                r#"<x:font><x:b/><x:sz val="11.0"/></x:font>"#,
                true,
            ),
            (
                r#"<xf numFmtId="0" applyFont="true"/>"#,
                r#"<xf applyFont="1" numFmtId='0'/>"#,
                true,
            ),
            (
                r#"<font><b/><sz val="11"/></font>"#,
                r#"<font><sz val="11"/><b/></font>"#,
                false,
            ),
            (
                r#"<numFmt formatCode="00000"/>"#,
                r#"<numFmt formatCode="0"/>"#,
                false,
            ),
            (
                r#"<font><b/><extLst><ext uri="x"/></extLst></font>"#,
                r#"<font><b/><extLst><ext uri='x'/></extLst></font>"#,
                false,
            ),
        ] {
            assert_eq!(key_of(a) == key_of(b), same, "{a} / {b}");
        }
    }
}
