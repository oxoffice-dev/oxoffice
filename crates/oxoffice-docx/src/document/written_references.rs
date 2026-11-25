//! References to a style, a note or a comment, found by a walk of what the
//! body writes instead of a serialization of the main part.
//!
//! See `specs/2026-11-25-written-fragment-walk.md`. The walk hands every
//! raw fragment the writer writes verbatim to a search that reads local
//! names, so a fragment that inherits its `w:` prefix, or binds another one,
//! is read as Word reads it. Debug builds compare each answer with a scan of
//! the serialized main part.

use std::collections::HashSet;

use oxoffice_docx_oxml::document::WrittenVisitor;
use oxoffice_docx_oxml::table::CT_TblPr;
use oxoffice_docx_oxml::text::CommentRangeMarker;

use super::*;

/// A body block that holds a reference, and whether one of its hits sits in
/// a raw fragment (the block is then re-serialized to edit it).
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotHit {
    /// The block's index in the body's content.
    pub slot: usize,
    /// Whether a hit sits in a raw fragment.
    pub raw: bool,
}

const STYLE_ELEMENTS: &[&[u8]] = &[b"pStyle", b"rStyle", b"tblStyle"];
/// The revision elements the resolver selects by `w:id`
/// (`revision::is_revision_local`).
pub(crate) const REVISION_ELEMENTS: &[&[u8]] = &[
    b"ins",
    b"del",
    b"moveFrom",
    b"moveTo",
    b"rPrChange",
    b"pPrChange",
    b"tblPrChange",
    b"sectPrChange",
];
const COMMENT_ELEMENTS: &[&[u8]] = &[
    b"commentRangeStart",
    b"commentRangeEnd",
    b"commentReference",
];

/// Call `found` with the unescaped value of the attribute whose local name
/// is `attribute`, on each element start whose local name is one of
/// `elements`, whatever their prefixes. A value that does not unescape is
/// passed as written.
pub(crate) fn raw_attribute_values(
    xml: &[u8],
    elements: &[&[u8]],
    attribute: &[u8],
    found: &mut dyn FnMut(&str) -> bool,
) -> bool {
    let mut at = 0;
    while let Some(offset) = xml[at..].iter().position(|byte| *byte == b'<') {
        let start = at + offset + 1;
        at = start;
        let Some(&first) = xml.get(start) else {
            break;
        };
        // Markup that holds no element: an end tag, a comment, a CDATA
        // section, a processing instruction, a declaration.
        let skip_past: Option<&[u8]> = match first {
            b'/' => Some(b">"),
            b'?' => Some(b"?>"),
            b'!' if xml[start..].starts_with(b"!--") => Some(b"-->"),
            b'!' if xml[start..].starts_with(b"![CDATA[") => Some(b"]]>"),
            b'!' => Some(b">"),
            _ => None,
        };
        if let Some(end) = skip_past {
            match xml[start..]
                .windows(end.len())
                .position(|window| window == end)
            {
                Some(found) => at = start + found + end.len(),
                None => break,
            }
            continue;
        }
        let name_end = xml[start..]
            .iter()
            .position(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>'))
            .map_or(xml.len(), |end| start + end);
        if !elements.contains(&local_name(&xml[start..name_end])) {
            continue;
        }
        let mut cursor = name_end;
        while let Some((name, value, next)) = next_attribute(xml, cursor) {
            cursor = next;
            if local_name(name) != attribute {
                continue;
            }
            let value = String::from_utf8_lossy(value);
            let value = quick_xml::escape::unescape(&value)
                .map(|value| value.into_owned())
                .unwrap_or_else(|_| value.into_owned());
            if found(&value) {
                return true;
            }
        }
        at = cursor;
    }
    false
}

fn local_name(name: &[u8]) -> &[u8] {
    name.iter()
        .rposition(|byte| *byte == b':')
        .map_or(name, |colon| &name[colon + 1..])
}

/// The next attribute of a start tag from `at`, as (name, raw value, end of
/// the attribute), or `None` at the tag's end.
fn next_attribute(xml: &[u8], mut at: usize) -> Option<(&[u8], &[u8], usize)> {
    while xml.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    if matches!(xml.get(at), None | Some(b'/' | b'>')) {
        return None;
    }
    let name_start = at;
    while xml
        .get(at)
        .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'=' | b'/' | b'>'))
    {
        at += 1;
    }
    let name = &xml[name_start..at];
    while xml.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    if xml.get(at) != Some(&b'=') {
        return None;
    }
    at += 1;
    while xml.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    let quote = *xml.get(at).filter(|byte| matches!(byte, b'"' | b'\''))?;
    let value_start = at + 1;
    let value_end = value_start + xml[value_start..].iter().position(|byte| *byte == quote)?;
    Some((name, &xml[value_start..value_end], value_end + 1))
}

/// Whether the written content names `style_id` as a paragraph, run or
/// table style.
struct StyleSearch<'a> {
    style_id: &'a str,
    found: bool,
}

impl WrittenVisitor for StyleSearch<'_> {
    fn raw(&mut self, xml: &[u8]) {
        if !self.found {
            let style_id = self.style_id;
            self.found =
                raw_attribute_values(xml, STYLE_ELEMENTS, b"val", &mut |value| value == style_id);
        }
    }

    fn paragraph_properties(&mut self, properties: &CT_PPr) {
        self.found |= properties.style_id.as_deref() == Some(self.style_id);
    }

    fn run_properties(&mut self, properties: &CT_RPr) {
        self.found |= properties.style_id.as_deref() == Some(self.style_id);
    }

    fn table_properties(&mut self, properties: &CT_TblPr) {
        self.found |= properties.style_id.as_deref() == Some(self.style_id);
    }
}

/// Whether the written content holds a reference that `typed` accepts or
/// whose `id` attribute `raw` accepts on one of `elements`; `raw_hit` tells
/// whether a hit came from a raw fragment.
struct ReferenceSearch<'a> {
    elements: &'a [&'a [u8]],
    raw_id: &'a dyn Fn(i32) -> bool,
    typed_run: &'a dyn Fn(&RunContent) -> bool,
    comment_ranges: bool,
    /// How many references the typed owners hold.
    typed: usize,
    raw_hit: bool,
}

impl WrittenVisitor for ReferenceSearch<'_> {
    fn raw(&mut self, xml: &[u8]) {
        if self.raw_hit {
            return;
        }
        let raw_id = self.raw_id;
        if raw_attribute_values(xml, self.elements, b"id", &mut |value| {
            value.trim().parse::<i32>().is_ok_and(raw_id)
        }) {
            self.raw_hit = true;
        }
    }

    fn paragraph(&mut self, paragraph: &CT_P) {
        if self.comment_ranges {
            self.typed +=
                paragraph
                    .comment_ranges
                    .iter()
                    .filter(|marker| {
                        let (CommentRangeMarker::Start { id, .. }
                        | CommentRangeMarker::End { id, .. }) = marker;
                        (self.raw_id)(*id)
                    })
                    .count();
        }
    }

    fn run(&mut self, run: &CT_R) {
        self.typed += run
            .content
            .iter()
            .filter(|content| (self.typed_run)(content))
            .count();
    }
}

/// Each block's hit, with how many references its typed owners hold.
fn search_slots(
    document: &Document,
    search: &mut dyn FnMut(&BodyContent) -> (usize, bool),
) -> Vec<(SlotHit, usize)> {
    document
        .document
        .body
        .content
        .iter()
        .enumerate()
        .filter_map(|(slot, block)| {
            let (typed, raw) = search(block);
            (typed > 0 || raw).then_some((SlotHit { slot, raw }, typed))
        })
        .collect()
}

impl Document {
    /// Whether the main part's content names `style_id`: its blocks, the
    /// body's section properties and the background.
    pub(super) fn main_part_references_style(&self, style_id: &str) -> bool {
        let mut search = StyleSearch {
            style_id,
            found: false,
        };
        for block in &self.document.body.content {
            block.visit_written(&mut search);
            if search.found {
                break;
            }
        }
        if !search.found {
            self.document.visit_written_outside_body(&mut search);
        }
        #[cfg(debug_assertions)]
        if let Ok(xml) = self.document.to_xml() {
            let serialized = super::styles::xml_references_style(&xml, style_id)
                .expect("the serialized main part reads");
            assert_eq!(
                search.found, serialized,
                "the written-fragment walk and the serialized main part disagree on style '{style_id}'"
            );
        }
        search.found
    }

    /// The body blocks that hold a reference to the note `id` of `kind`
    /// (`StoryKind::Footnote` or `StoryKind::Endnote`).
    #[doc(hidden)]
    pub fn body_slots_referencing_note(&self, kind: StoryKind, id: i32) -> Vec<SlotHit> {
        let (element, footnote): (&[u8], bool) = match kind {
            StoryKind::Footnote => (b"footnoteReference", true),
            StoryKind::Endnote => (b"endnoteReference", false),
            other => panic!("{other:?} is not a note kind"),
        };
        let elements = [element];
        let raw_id = move |value: i32| value == id;
        let typed_run = move |content: &RunContent| match content {
            RunContent::FootnoteRef { id: value, .. } => footnote && *value == id,
            RunContent::EndnoteRef { id: value, .. } => !footnote && *value == id,
            _ => false,
        };
        let hits = search_slots(self, &mut |block| {
            let mut search = ReferenceSearch {
                elements: &elements,
                raw_id: &raw_id,
                typed_run: &typed_run,
                comment_ranges: false,
                typed: 0,
                raw_hit: false,
            };
            block.visit_written(&mut search);
            (search.typed, search.raw_hit)
        });
        #[cfg(debug_assertions)]
        self.assert_slots_match_serialized(&hits, &elements, &raw_id);
        hits.into_iter().map(|(hit, _)| hit).collect()
    }

    /// The body blocks that hold a range mark or a reference of one of the
    /// comments `ids`.
    #[doc(hidden)]
    pub fn body_slots_referencing_comments(&self, ids: &HashSet<i32>) -> Vec<SlotHit> {
        self.body_slots_counting_comment_references(ids)
            .into_iter()
            .map(|(hit, _)| hit)
            .collect()
    }

    /// [`Self::body_slots_referencing_comments`], with how many range marks
    /// and reference runs of those comments the typed owners of each block
    /// hold.
    pub(crate) fn body_slots_counting_comment_references(
        &self,
        ids: &HashSet<i32>,
    ) -> Vec<(SlotHit, usize)> {
        let raw_id = |value: i32| ids.contains(&value);
        let typed_run = |content: &RunContent| matches!(content, RunContent::CommentReference { id, .. } if ids.contains(id));
        let hits = search_slots(self, &mut |block| {
            let mut search = ReferenceSearch {
                elements: COMMENT_ELEMENTS,
                raw_id: &raw_id,
                typed_run: &typed_run,
                comment_ranges: true,
                typed: 0,
                raw_hit: false,
            };
            block.visit_written(&mut search);
            (search.typed, search.raw_hit)
        });
        #[cfg(debug_assertions)]
        self.assert_slots_match_serialized(&hits, COMMENT_ELEMENTS, &raw_id);
        hits
    }

    /// The body blocks that hold a revision element of id `id` (an
    /// insertion, a deletion, a move, a property change). Every revision is
    /// written from bytes the model keeps, so each hit is raw.
    #[doc(hidden)]
    pub fn body_slots_with_revision_id(&self, id: i32) -> Vec<SlotHit> {
        let raw_id = move |value: i32| value == id;
        let typed_run = |_: &RunContent| false;
        let hits = search_slots(self, &mut |block| {
            let mut search = ReferenceSearch {
                elements: REVISION_ELEMENTS,
                raw_id: &raw_id,
                typed_run: &typed_run,
                comment_ranges: false,
                typed: 0,
                raw_hit: false,
            };
            block.visit_written(&mut search);
            (search.typed, search.raw_hit)
        });
        #[cfg(debug_assertions)]
        self.assert_slots_match_serialized(&hits, REVISION_ELEMENTS, &raw_id);
        hits.into_iter().map(|(hit, _)| hit).collect()
    }

    /// Whether what the main part writes outside its body's blocks (the
    /// background, the body's final section properties) holds a revision
    /// element of id `id`.
    pub(crate) fn main_part_frame_has_revision_id(&self, id: i32) -> bool {
        let raw_id = move |value: i32| value == id;
        let typed_run = |_: &RunContent| false;
        let mut search = ReferenceSearch {
            elements: REVISION_ELEMENTS,
            raw_id: &raw_id,
            typed_run: &typed_run,
            comment_ranges: false,
            typed: 0,
            raw_hit: false,
        };
        self.document.visit_written_outside_body(&mut search);
        search.raw_hit
    }

    /// Debug proof of a slot query: the blocks of the serialized main part
    /// whose bytes hold a WordprocessingML element of `elements` with an
    /// accepted `w:id` are the slots found, and a block whose bytes hold
    /// more of them than its typed owners is found raw (a reference the
    /// typed owners do not hold is in a raw fragment). A main part that
    /// does not serialize proves nothing.
    #[cfg(debug_assertions)]
    fn assert_slots_match_serialized(
        &self,
        hits: &[(SlotHit, usize)],
        elements: &[&[u8]],
        accepted: &dyn Fn(i32) -> bool,
    ) {
        let Ok((xml, _content, children)) = self.document.to_xml_with_body_spans() else {
            return;
        };
        let offsets = word_attribute_offsets(&xml, elements, b"id", &|value| {
            value.trim().parse::<i32>().is_ok_and(accepted)
        })
        .expect("the serialized main part reads");
        let mut serialized = Vec::<(usize, usize)>::new();
        for slot in offsets
            .into_iter()
            .filter_map(|offset| children.iter().position(|range| range.contains(&offset)))
        {
            match serialized.last_mut() {
                Some((last, count)) if *last == slot => *count += 1,
                _ => serialized.push((slot, 1)),
            }
        }
        let walked = hits.iter().map(|(hit, _)| hit.slot).collect::<Vec<_>>();
        assert_eq!(
            walked,
            serialized.iter().map(|(slot, _)| *slot).collect::<Vec<_>>(),
            "the written-fragment walk and the serialized main part disagree on the blocks"
        );
        for ((hit, typed), (_, count)) in hits.iter().zip(&serialized) {
            assert!(
                hit.raw || *typed >= *count,
                "block {} writes {count} references, its typed owners hold {typed}, and no raw fragment was found",
                hit.slot
            );
        }
    }
}

/// The offsets of the WordprocessingML elements of `elements` whose
/// `w:`-namespaced or unprefixed `attribute` is accepted, in a whole part.
#[cfg(debug_assertions)]
fn word_attribute_offsets(
    xml: &[u8],
    elements: &[&[u8]],
    attribute: &[u8],
    accepted: &dyn Fn(&str) -> bool,
) -> Result<Vec<usize>> {
    use quick_xml::events::Event;
    use quick_xml::name::{Namespace, ResolveResult};
    let mut reader = quick_xml::NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut offsets = Vec::new();
    loop {
        let offset = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(error.to_string()))?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element)
                if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == WORD_NAMESPACE.as_bytes())
                    && elements.contains(&element.local_name().as_ref()) =>
            {
                for attribute_value in element.attributes() {
                    let attribute_value =
                        attribute_value.map_err(|error| Error::Other(error.to_string()))?;
                    let (_, local) = reader.resolver().resolve_attribute(attribute_value.key);
                    if local.as_ref() == attribute
                        && accepted(
                            &attribute_value
                                .decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|error| Error::Other(error.to_string()))?,
                        )
                    {
                        offsets.push(offset);
                        break;
                    }
                }
            }
            Event::Eof => return Ok(offsets),
            _ => {}
        }
        buffer.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styles(xml: &str) -> Vec<String> {
        let mut values = Vec::new();
        raw_attribute_values(xml.as_bytes(), STYLE_ELEMENTS, b"val", &mut |value| {
            values.push(value.to_string());
            false
        });
        values
    }

    #[test]
    fn raw_search_reads_local_names_whatever_the_prefix() {
        assert_eq!(styles(r#"<w:pStyle w:val="A"/>"#), ["A"]);
        assert_eq!(styles(r#"<x:rStyle x:val='B' />"#), ["B"]);
        assert_eq!(styles(r#"<tblStyle val="C&amp;D"></tblStyle>"#), ["C&D"]);
        assert_eq!(
            styles(
                r#"<w:pPr><w:pStyle w:other="Z" w:val="E"/></w:pPr><w:r><w:rStyle w:val="F"/></w:r>"#
            ),
            ["E", "F"]
        );
    }

    #[test]
    fn raw_search_skips_other_elements_and_end_tags() {
        assert!(styles(r#"<w:pStyles w:val="A"/></w:pStyle><w:t>pStyle val="B"</w:t>"#).is_empty());
        assert!(styles(r#"<!-- <w:pStyle w:val="A"/> -->"#).is_empty());
        assert!(styles(r#"<w:t><![CDATA[<w:rStyle w:val="A"/>]]></w:t>"#).is_empty());
        assert_eq!(
            styles(r#"<?probe <w:rStyle w:val="A"/> ?><w:rStyle w:val="B"/>"#),
            ["B"]
        );
        assert_eq!(
            styles(r#"<w:p a="x>y"><!-- -- > --><w:pStyle w:val='C'/></w:p>"#),
            ["C"]
        );
    }
}
