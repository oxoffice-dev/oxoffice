//! The `cols` element of a worksheet: the width of columns, written as
//! `col` spans.
//!
//! A `col` names a run of columns (`min` to `max`, 1-based) and what they
//! share: a width, a style, hidden, an outline level. Setting a width splits
//! the spans the columns cross, writes the width on the part inside the
//! range and keeps everything else a span says (its style, whether it is
//! hidden), and merges neighbours that came to say the same, as Excel
//! writes them.

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
    let mut cols = cols.unwrap_or_else(|| Element::new(&qualified(prefix, "cols")));
    let prefix = prefix_of_name(&cols.tag.name()).to_owned();
    let width_of = |tag: &mut Tag| {
        tag.set_f64("width", Some(width));
        tag.set_bool("customWidth", Some(true));
        tag.set("bestFit", None);
    };

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
        let mut inside = span.cut(span.first.max(first), span.last.min(last));
        width_of(&mut inside.element.tag);
        written.push(inside);
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
        width_of(&mut element.tag);
        written.push(Span {
            first: from,
            last: to,
            element,
        });
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
        let cases: [Case; 9] = [
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
                "a col that names no columns stays where it is",
                r#"<cols><col width="3"/></cols>"#,
                (0, 0),
                4.0,
                r#"<cols><col min="1" max="1" width="4" customWidth="1"/><col width="3"/></cols>"#,
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
    fn a_missing_cols_and_its_spans_take_the_prefix_of_the_sheet() {
        let written = set_width(None, "x", 0, 0, 5.0).to_xml();
        assert_eq!(
            String::from_utf8(written).unwrap(),
            r#"<x:cols><x:col min="1" max="1" width="5" customWidth="1"/></x:cols>"#
        );
    }
}
