//! The `cols` element of a worksheet: the width and the style of columns,
//! written as `col` spans.
//!
//! A `col` names a run of columns (`min` to `max`, 1-based) and what they
//! share: a width, a style, hidden, an outline level. Setting a width or a
//! style splits the spans the columns cross, writes it on the part inside
//! the range and keeps everything else a span says (its style or width,
//! whether it is hidden), and merges neighbours that came to say the same,
//! as Excel writes them.

use crate::reference::MAX_COLUMNS;
use crate::xml::{Element, Node, Tag, prefix_of_name, qualified};

/// A `col` and the columns it names, 0-based, both ends included.
struct Span {
    first: u32,
    last: u32,
    element: Element,
}

impl Span {
    /// The span read from a `col`, or `None` when it names no columns of
    /// the grid: such an element is kept where it was.
    fn read(element: &Element) -> Option<Self> {
        if !element.tag.is("col") {
            return None;
        }
        let (min, max) = (element.tag.u32("min")?, element.tag.u32("max")?);
        ((1..=MAX_COLUMNS).contains(&min) && (min..=MAX_COLUMNS).contains(&max)).then(|| Span {
            first: min - 1,
            last: max - 1,
            element: element.clone(),
        })
    }

    /// The same span cut to `first..=last`.
    fn cut(&self, first: u32, last: u32) -> Span {
        Span {
            first,
            last,
            element: self.element.clone(),
        }
    }

    /// Writes the columns the span names in its tag.
    fn written(mut self) -> Element {
        self.element
            .tag
            .set("min", Some(&(self.first + 1).to_string()));
        self.element
            .tag
            .set("max", Some(&(self.last + 1).to_string()));
        self.element
    }
}

/// `cols` with the columns `first..=last` (0-based) `width` wide, the
/// stored width: a span that starts or ends outside the range is cut, a
/// column no span names gets one, `customWidth` is set and `bestFit`, which
/// says the width was fitted to the content, goes. `prefix` names the
/// elements a missing `cols` and its spans are written with.
pub fn set_width(
    cols: Option<Element>,
    prefix: &str,
    first: u32,
    last: u32,
    width: f64,
) -> Element {
    let width_of = |tag: &mut Tag| {
        tag.set_f64("width", Some(width));
        tag.set_bool("customWidth", Some(true));
        tag.set("bestFit", None);
    };
    edit_spans(
        cols,
        prefix,
        first,
        last,
        &mut |tag| width_of(tag),
        &mut |tag| {
            width_of(tag);
            true
        },
    )
}

/// `cols` with the style of each column of `first..=last` (0-based) the one
/// `style` gives for the style it had (0 when none): the spans cut as
/// [`set_width`] cuts them, keeping their width, `hidden` and outline level,
/// and a column no span names given one `default_width` wide when its new
/// style is not 0.
pub fn set_style(
    cols: Option<Element>,
    prefix: &str,
    first: u32,
    last: u32,
    style: &dyn Fn(u32) -> u32,
    default_width: f64,
) -> Element {
    let restyle = |tag: &mut Tag| {
        let old = tag.u32("style").unwrap_or(0);
        let new = style(old);
        if new != old {
            tag.set("style", (new != 0).then(|| new.to_string()).as_deref());
        }
    };
    edit_spans(
        cols,
        prefix,
        first,
        last,
        &mut |tag| restyle(tag),
        &mut |tag| {
            let new = style(0);
            if new == 0 {
                return false;
            }
            tag.set_f64("width", Some(default_width));
            tag.set("style", Some(&new.to_string()));
            true
        },
    )
}

/// `cols` with each span's part inside `first..=last` edited by `inside`,
/// and the columns of the range no span names given a span of their own when
/// `gap` (given its tag with `min` and `max`) says so; neighbours that came
/// to say the same merge.
fn edit_spans(
    cols: Option<Element>,
    prefix: &str,
    first: u32,
    last: u32,
    inside: &mut dyn FnMut(&mut Tag),
    gap: &mut dyn FnMut(&mut Tag) -> bool,
) -> Element {
    let mut cols = cols.unwrap_or_else(|| Element::new(&qualified(prefix, "cols")));
    let prefix = prefix_of_name(&cols.tag.name()).to_owned();

    let mut spans = Vec::new();
    let mut kept = Vec::new();
    for node in std::mem::take(&mut cols.children) {
        match &node {
            Node::Element(element) => match Span::read(element) {
                Some(span) => spans.push(span),
                None => kept.push(node),
            },
            _ => kept.push(node),
        }
    }

    let mut written = Vec::new();
    for span in spans {
        if span.last < first || span.first > last {
            written.push(span);
            continue;
        }
        if span.first < first {
            written.push(span.cut(span.first, first - 1));
        }
        let mut part = span.cut(span.first.max(first), span.last.min(last));
        inside(&mut part.element.tag);
        written.push(part);
        if span.last > last {
            written.push(span.cut(last + 1, span.last));
        }
    }
    // The columns of the range no span named.
    let mut covered: Vec<(u32, u32)> = written
        .iter()
        .filter(|span| span.last >= first && span.first <= last)
        .map(|span| (span.first.max(first), span.last.min(last)))
        .collect();
    covered.sort_unstable();
    let mut next = first;
    let mut gaps = Vec::new();
    for (from, to) in covered {
        if from > next {
            gaps.push((next, from - 1));
        }
        next = next.max(to + 1);
    }
    if next <= last {
        gaps.push((next, last));
    }
    for (from, to) in gaps {
        let mut element = Element::new(&qualified(&prefix, "col"));
        element.tag.set("min", Some(&(from + 1).to_string()));
        element.tag.set("max", Some(&(to + 1).to_string()));
        if gap(&mut element.tag) {
            written.push(Span {
                first: from,
                last: to,
                element,
            });
        }
    }

    written.sort_by_key(|span| span.first);
    let mut merged: Vec<Span> = Vec::new();
    for span in written {
        match merged.last_mut() {
            Some(before)
                if before.last + 1 == span.first
                    && before.element.tag.attributes_except(&["min", "max"])
                        == span.element.tag.attributes_except(&["min", "max"]) =>
            {
                before.last = span.last;
            }
            _ => merged.push(span),
        }
    }

    cols.children = merged
        .into_iter()
        .map(|span| Node::Element(span.written()))
        .chain(kept)
        .collect();
    cols
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(xml: &str) -> Option<Element> {
        (!xml.is_empty()).then(|| Element::parse(xml.as_bytes()).unwrap())
    }

    fn set(xml: &str, first: u32, last: u32, width: f64) -> String {
        String::from_utf8(set_width(cols(xml), "", first, last, width).to_xml()).unwrap()
    }

    #[test]
    fn a_width_splits_the_spans_it_crosses_and_keeps_what_else_they_say() {
        type Case<'a> = (&'a str, &'a str, (u32, u32), f64, &'a str);
        let cases: [Case; 8] = [
            (
                "a sheet with no cols",
                "",
                (0, 2),
                20.7109375,
                r#"<cols><col min="1" max="3" width="20.7109375" customWidth="1"/></cols>"#,
            ),
            (
                "a gap between two spans, the spans kept",
                r#"<cols><col min="1" max="1" width="5" customWidth="1"/><col min="5" max="5" width="6" customWidth="1"/></cols>"#,
                (2, 2),
                9.0,
                r#"<cols><col min="1" max="1" width="5" customWidth="1"/><col min="3" max="3" width="9" customWidth="1"/><col min="5" max="5" width="6" customWidth="1"/></cols>"#,
            ),
            (
                "a range inside a span cuts it in three, the style and hidden kept, bestFit gone",
                r#"<cols><col min="1" max="5" width="9" style="3" hidden="1" bestFit="1" customWidth="1"/></cols>"#,
                (1, 2),
                12.5,
                concat!(
                    r#"<cols><col min="1" max="1" width="9" style="3" hidden="1" bestFit="1" customWidth="1"/>"#,
                    r#"<col min="2" max="3" width="12.5" style="3" hidden="1" customWidth="1"/>"#,
                    r#"<col min="4" max="5" width="9" style="3" hidden="1" bestFit="1" customWidth="1"/></cols>"#
                ),
            ),
            (
                "a range that spans two",
                r#"<cols><col min="1" max="2" width="9"/><col min="3" max="4" width="9" style="1"/></cols>"#,
                (1, 2),
                7.0,
                concat!(
                    r#"<cols><col min="1" max="1" width="9"/><col min="2" max="2" width="7" customWidth="1"/>"#,
                    r#"<col min="3" max="3" width="7" style="1" customWidth="1"/><col min="4" max="4" width="9" style="1"/></cols>"#
                ),
            ),
            (
                "neighbours that came to say the same merge",
                r#"<cols><col min="1" max="1" width="20" customWidth="1"/><col min="2" max="2" width="9" customWidth="1"/><col min="3" max="3" width="20" customWidth="1"/></cols>"#,
                (1, 1),
                20.0,
                r#"<cols><col min="1" max="3" width="20" customWidth="1"/></cols>"#,
            ),
            (
                "LibreOffice's spellings are kept, its width written in the fixed form",
                r#"<cols><col collapsed="false" customWidth="false" hidden="false" outlineLevel="0" max="2" min="1" style="0" width="11.52"/></cols>"#,
                (0, 0),
                15.0,
                concat!(
                    r#"<cols><col collapsed="false" customWidth="1" hidden="false" outlineLevel="0" max="1" min="1" style="0" width="15"/>"#,
                    r#"<col collapsed="false" customWidth="false" hidden="false" outlineLevel="0" max="2" min="2" style="0" width="11.52"/></cols>"#
                ),
            ),
            (
                "the last column of the grid",
                r#"<cols><col min="16383" max="16384" width="3"/></cols>"#,
                (16_383, 16_383),
                4.0,
                r#"<cols><col min="16383" max="16383" width="3"/><col min="16384" max="16384" width="4" customWidth="1"/></cols>"#,
            ),
            (
                "the same width again changes nothing",
                r#"<cols><col min="1" max="3" width="20" customWidth="1"/></cols>"#,
                (0, 2),
                20.0,
                r#"<cols><col min="1" max="3" width="20" customWidth="1"/></cols>"#,
            ),
        ];
        for (case, before, (first, last), width, expected) in cases {
            assert_eq!(set(before, first, last, width), expected, "{case}");
        }
    }

    #[test]
    fn a_style_splits_the_spans_it_crosses_and_gives_a_span_to_the_columns_it_styles() {
        let restyle = |xml: &str, first, last, style: &dyn Fn(u32) -> u32| {
            String::from_utf8(set_style(cols(xml), "", first, last, style, 9.140625).to_xml())
                .unwrap()
        };
        let bold = |old: u32| if old == 0 { 5 } else { old + 10 };
        for (case, before, (first, last), style, expected) in [
            (
                "columns no span names, one styled span of the default width",
                "",
                (1, 2),
                &bold as &dyn Fn(u32) -> u32,
                r#"<cols><col min="2" max="3" width="9.140625" style="5"/></cols>"#,
            ),
            (
                "a span crossed keeps its width and hidden, each part its own style",
                r#"<cols><col min="1" max="3" width="12" style="2" hidden="1" customWidth="1"/></cols>"#,
                (1, 3),
                &bold,
                concat!(
                    r#"<cols><col min="1" max="1" width="12" style="2" hidden="1" customWidth="1"/>"#,
                    r#"<col min="2" max="3" width="12" style="12" hidden="1" customWidth="1"/>"#,
                    r#"<col min="4" max="4" width="9.140625" style="5"/></cols>"#
                ),
            ),
            (
                "styles cleared: the style goes, a column without a span gets none",
                r#"<cols><col min="2" max="2" width="12" style="2" customWidth="1"/></cols>"#,
                (0, 2),
                &|_| 0,
                r#"<cols><col min="2" max="2" width="12" customWidth="1"/></cols>"#,
            ),
        ] {
            assert_eq!(restyle(before, first, last, style), expected, "{case}");
        }
    }

    #[test]
    fn a_missing_cols_and_its_spans_take_the_prefix_of_the_sheet() {
        let written = set_width(None, "x", 0, 0, 5.0).to_xml();
        assert_eq!(
            String::from_utf8(written).unwrap(),
            r#"<x:cols><x:col min="1" max="1" width="5" customWidth="1"/></x:cols>"#
        );
    }
}
