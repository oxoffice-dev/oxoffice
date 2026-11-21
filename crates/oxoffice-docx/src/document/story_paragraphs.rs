//! Every paragraph of every story with its style, and text counts by story.

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

use quick_xml::XmlVersion;
use quick_xml::events::Event;
use quick_xml::reader::NsReader;

use super::{
    ContentLocation, Document, StoryId, StoryItemKind, StoryKind, StoryNamespace,
    scan_story_items_with_scope, scan_story_owners, scoped_story_fragment, story_namespace,
    story_namespace_scopes_at, story_range_paragraph_spans_from_inventory, word_element,
};
use crate::error::{Error, Result};

/// One paragraph of a story, as [`Document::story_paragraphs`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoryParagraph {
    /// The story of the part the paragraph sits in: [`StoryKind::Body`],
    /// `Header`, `Footer`, `Footnote`, `Endnote` or `Comment`. A paragraph of
    /// a table cell or a text box takes the kind of the story around it.
    pub kind: StoryKind,
    /// Where the paragraph is: the story that holds it (the part's story, a
    /// table cell or a text box, whose part name it carries) and the item
    /// path that [`Document::story_items`] uses. That path is the index of
    /// the paragraph's item, which counts every item of the story (inline
    /// controls, fields and drawings too), not a paragraph ordinal. A
    /// paragraph of a block content control has the control's index then its
    /// own index in the control; one of a block element the story model
    /// keeps verbatim, such as a block `w:customXml`, the element's index
    /// then its own index in it. Story range calls refuse the latter.
    pub location: ContentLocation,
    /// The style the paragraph names in its own properties (`w:pStyle`), or
    /// `None` when it names none and the default paragraph style applies.
    pub style: Option<String>,
    /// The text a reader sees, in the accepted view: tracked insertions in,
    /// deletions and moved-away text out, the text of content controls,
    /// fields (their results, not their codes), hyperlinks, smart tags and
    /// custom XML elements included, a tab as `\t` and a break as `\n`. A
    /// text box is listed on its own, after the paragraph that anchors it:
    /// as a text box story, or, inside a block element the story model keeps
    /// verbatim, as a paragraph of that element. Its text is not in the
    /// paragraph that anchors it. One exception, not listed at all yet: a box
    /// in a run inside an inline smart tag or custom XML element (#78).
    pub text: String,
}

impl StoryParagraph {
    /// The package part that holds the paragraph, such as
    /// `/word/header1.xml`.
    pub fn part_name(&self) -> &str {
        self.location.story().part_name()
    }
}

impl Document {
    /// Every paragraph of every story, as a reader of the document sees it.
    ///
    /// The parts come in [`Document::stories`] order: the main part, then the
    /// headers, footers, notes and comments parts in the order of their
    /// relationships, each part once however many sections use it. The
    /// paragraphs of a part come in document order: a table
    /// cell's paragraphs where the table is, a text box's right after the
    /// paragraph that anchors it, those of a block content control (a table
    /// of contents, for one) one by one, and those of a block element the
    /// story model keeps verbatim, such as a block `w:customXml`, which
    /// other calls do not reach. A text box that Word also writes as a VML
    /// fallback is read once. The separator entries of the notes parts are
    /// not stories, and a paragraph that a tracked deletion removes is
    /// listed with the text that is left, often none.
    pub fn story_paragraphs(&self) -> Result<Vec<StoryParagraph>> {
        let mut paragraphs = Vec::new();
        let mut seen = HashSet::new();
        for source in self.story_sources()? {
            let xml = source.xml.as_ref();
            let owners = scan_story_owners(xml, source.root_kind)?
                .into_iter()
                .filter(|owner| {
                    seen.insert((owner.kind, source.part_name.clone(), owner.owner_index))
                })
                .map(|owner| {
                    let story = StoryId {
                        kind: owner.kind,
                        part_name: source.part_name.clone(),
                        owner_index: owner.owner_index,
                        fingerprint: owner.fingerprint,
                    };
                    (story, owner)
                })
                .collect::<Vec<_>>();
            let owner_scopes =
                story_namespace_scopes_at(xml, owners.iter().map(|(_, owner)| owner.full.start))?;
            let mut inventories = Vec::with_capacity(owners.len());
            for (_, owner) in &owners {
                let scope = owner_scopes.get(&owner.full.start).ok_or_else(|| {
                    Error::Other("story owner namespace scope was not inventoried".to_owned())
                })?;
                inventories.push(scan_story_items_with_scope(xml, owner, scope, None)?);
            }
            let control_scopes = story_namespace_scopes_at(
                xml,
                inventories
                    .iter()
                    .flatten()
                    .filter(|item| item.kind == StoryItemKind::ContentControl)
                    .map(|item| item.full.start),
            )?;
            let mut spans = story_range_paragraph_spans_from_inventory(
                xml,
                &owners,
                &inventories,
                &control_scopes,
            )?;
            // The paragraphs of a direct item the story model keeps verbatim.
            let preserved = inventories
                .iter()
                .enumerate()
                .flat_map(|(owner, items)| {
                    items.iter().enumerate().filter_map(move |(index, item)| {
                        (item.kind == StoryItemKind::PreservedNode && item.direct_owner_child)
                            .then_some((owner, index, item.full.clone()))
                    })
                })
                .collect::<Vec<_>>();
            let preserved_scopes =
                story_namespace_scopes_at(xml, preserved.iter().map(|(_, _, full)| full.start))?;
            for (owner, index, full) in preserved {
                let scope = preserved_scopes.get(&full.start).ok_or_else(|| {
                    Error::Other("preserved item namespace scope was not inventoried".to_owned())
                })?;
                for (paragraph, span) in preserved_paragraph_spans(xml, &full, scope)?
                    .into_iter()
                    .enumerate()
                {
                    let location = ContentLocation::new(
                        owners[owner].0.clone(),
                        StoryItemKind::Paragraph,
                        vec![index, paragraph],
                    );
                    spans[owner].push((location, span));
                }
            }
            let scopes =
                story_namespace_scopes_at(xml, spans.iter().flatten().map(|(_, span)| span.start))?;
            let mut part = Vec::new();
            for (location, span) in spans.into_iter().flatten() {
                let scope = scopes.get(&span.start).ok_or_else(|| {
                    Error::Other("story paragraph namespace scope was not inventoried".to_owned())
                })?;
                let (style, text) = read_paragraph(xml, &span, scope)?;
                part.push((
                    span.start,
                    StoryParagraph {
                        kind: source.root_kind,
                        location,
                        style,
                        text,
                    },
                ));
            }
            part.sort_by_key(|(start, _)| *start);
            paragraphs.extend(part.into_iter().map(|(_, paragraph)| paragraph));
        }
        Ok(paragraphs)
    }

    /// Count the occurrences of `text` in the paragraphs of
    /// [`Document::story_paragraphs`], by the kind of their story.
    ///
    /// Each kind found comes once, in a fixed order: body, header, footer,
    /// footnote, endnote, comment. Occurrences do not overlap and never cross
    /// the end of a paragraph. A header or a
    /// footer counts once per part, so a text in the default and the first
    /// page header counts twice, as replacing it does. An empty `text` is
    /// refused.
    pub fn count_text(&self, text: &str) -> Result<Vec<(StoryKind, usize)>> {
        if text.is_empty() {
            return Err(Error::Other("count_text needs a text to count".to_owned()));
        }
        let mut counts: Vec<(StoryKind, usize)> = Vec::new();
        for paragraph in self.story_paragraphs()? {
            let found = paragraph.text.matches(text).count();
            if found == 0 {
                continue;
            }
            match counts.iter_mut().find(|(kind, _)| *kind == paragraph.kind) {
                Some((_, count)) => *count += found,
                None => counts.push((paragraph.kind, found)),
            }
        }
        counts.sort_by_key(|(kind, _)| {
            [
                StoryKind::Body,
                StoryKind::Header,
                StoryKind::Footer,
                StoryKind::Footnote,
                StoryKind::Endnote,
                StoryKind::Comment,
            ]
            .iter()
            .position(|known| known == kind)
        });
        Ok(counts)
    }
}

/// The style a paragraph names in its own properties (`w:pPr/w:pStyle`)
/// and the text a reader sees in it, in one pass over the paragraph.
///
/// Text is the content of `w:t`, with `w:tab` as a tab and `w:br` and
/// `w:cr` as a line break, as `Paragraph::text` reads them, and the base of a
/// ruby, which `Paragraph::text` also reads when the ruby is a child of the
/// paragraph, but leaves out with its guide when Word writes it in a run.
/// Left out: tracked
/// deletions and moved-away text, the properties (where a tracked style
/// change keeps the old style), the phonetic guide of a ruby (`w:rt`), the
/// `mc:Fallback` copy of a drawing, text boxes (listed on their own) and
/// field codes. A complex field shows its
/// text once every field open around it has reached its result.
fn read_paragraph(
    xml: &[u8],
    paragraph: &Range<usize>,
    scope: &BTreeMap<String, String>,
) -> Result<(Option<String>, String)> {
    let (closed, _) = scoped_story_fragment(xml, paragraph.clone(), scope)?;
    let mut reader = NsReader::from_reader(closed.as_slice());
    reader.config_mut().trim_text(false);
    let mut style = None;
    let mut text = String::new();
    let mut depth = 0usize;
    // The depth of the element whose subtree is left out, while inside it.
    let mut skipped: Option<usize> = None;
    let mut in_properties = false;
    let mut in_text = false;
    // For each complex field open here, whether it has reached its result.
    let mut fields: Vec<bool> = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story paragraph scan failed: {error}")))?;
        let kind = story_namespace(&namespace);
        let word = kind == StoryNamespace::Word;
        let visible = skipped.is_none() && fields.iter().all(|in_result| *in_result);
        match event {
            Event::Start(element) => {
                let local = element.local_name();
                let local = local.as_ref();
                if depth == 1 {
                    in_properties = word && local == b"pPr";
                }
                if in_properties && depth == 2 && word && local == b"pStyle" {
                    style = word_value(&reader, &element)?;
                }
                if skipped.is_none()
                    && ((word
                        && matches!(
                            local,
                            b"pPr" | b"rPr" | b"del" | b"moveFrom" | b"txbxContent" | b"rt"
                        ))
                        || (kind == StoryNamespace::MarkupCompatibility && local == b"Fallback"))
                {
                    skipped = Some(depth);
                }
                if word && local == b"t" {
                    in_text = true;
                }
                if skipped.is_none() && word && local == b"fldChar" {
                    field_char(&reader, &element, &mut fields)?;
                }
                depth += 1;
            }
            Event::Empty(element) => {
                let local = element.local_name();
                let local = local.as_ref();
                if in_properties && depth == 2 && word && local == b"pStyle" {
                    style = word_value(&reader, &element)?;
                }
                if skipped.is_none() && word && local == b"fldChar" {
                    field_char(&reader, &element, &mut fields)?;
                } else if visible && word {
                    match local {
                        b"tab" => text.push('\t'),
                        b"br" | b"cr" => text.push('\n'),
                        _ => {}
                    }
                }
            }
            Event::End(element) => {
                depth -= 1;
                if skipped == Some(depth) {
                    skipped = None;
                }
                if word && element.local_name().as_ref() == b"t" {
                    in_text = false;
                }
                if depth == 1 {
                    in_properties = false;
                }
            }
            Event::Text(value) if visible && in_text => text.push_str(
                &quick_xml::escape::unescape(&value.decode().map_err(|error| {
                    Error::Other(format!("story paragraph text decode failed: {error}"))
                })?)
                .map_err(|error| {
                    Error::Other(format!("story paragraph text unescape failed: {error}"))
                })?,
            ),
            Event::CData(value) if visible && in_text => {
                text.push_str(&value.decode().map_err(|error| {
                    Error::Other(format!("story paragraph CDATA decode failed: {error}"))
                })?)
            }
            Event::GeneralRef(value) if visible && in_text => {
                text.push_str(&oxoffice_core::xml_text::resolve_entity(&value))
            }
            Event::Eof => return Ok((style, text)),
            _ => {}
        }
        buffer.clear();
    }
}

/// The `w:val` attribute of an element.
fn word_value(
    reader: &NsReader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
) -> Result<Option<String>> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("story paragraph attribute scan failed: {error}"))
        })?;
        let (namespace, local_name) = reader.resolver().resolve_attribute(attribute.key);
        if word_element(&namespace) && local_name.as_ref() == b"val" {
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                .map_err(|error| {
                    Error::Other(format!("story paragraph attribute decode failed: {error}"))
                })?;
            return Ok(Some(value.into_owned()));
        }
    }
    Ok(None)
}

/// Open, advance or close a complex field on a `w:fldChar`. An end without
/// its begin in this paragraph, as in the last paragraph of a table of
/// contents, closes nothing.
fn field_char(
    reader: &NsReader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
    fields: &mut Vec<bool>,
) -> Result<()> {
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("story paragraph field scan failed: {error}")))?;
        let (namespace, local_name) = reader.resolver().resolve_attribute(attribute.key);
        if !word_element(&namespace) || local_name.as_ref() != b"fldCharType" {
            continue;
        }
        match attribute.value.as_ref() {
            b"begin" => fields.push(false),
            b"separate" => {
                if let Some(in_result) = fields.last_mut() {
                    *in_result = true;
                }
            }
            b"end" => {
                fields.pop();
            }
            _ => {}
        }
    }
    Ok(())
}

/// The paragraphs inside a block element the story model keeps verbatim, in
/// document order: every `w:p` but those of an `mc:Fallback` copy. A text
/// box inside such an element is no story of its own, so its paragraphs are
/// listed here, after the paragraph that anchors it.
fn preserved_paragraph_spans(
    xml: &[u8],
    full: &Range<usize>,
    scope: &BTreeMap<String, String>,
) -> Result<Vec<Range<usize>>> {
    let (closed, added) = scoped_story_fragment(xml, full.clone(), scope)?;
    let source = |local: usize| {
        if local == 0 {
            full.start
        } else {
            full.start + local - added
        }
    };
    let mut reader = NsReader::from_reader(closed.as_slice());
    let mut spans = Vec::new();
    let mut depth = 0usize;
    // The depth and start of each open paragraph.
    let mut open: Vec<(usize, usize)> = Vec::new();
    // The depth of the `mc:Fallback` copy left out, while inside it.
    let mut fallback: Option<usize> = None;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("preserved paragraph scan failed: {error}")))?;
        let kind = story_namespace(&namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) if fallback.is_none() => {
                let local = element.local_name();
                if kind == StoryNamespace::Word && local.as_ref() == b"p" {
                    open.push((depth, before));
                } else if kind == StoryNamespace::MarkupCompatibility
                    && local.as_ref() == b"Fallback"
                {
                    fallback = Some(depth);
                }
                depth += 1;
            }
            Event::Start(_) => depth += 1,
            Event::Empty(element)
                if fallback.is_none()
                    && kind == StoryNamespace::Word
                    && element.local_name().as_ref() == b"p" =>
            {
                spans.push(source(before)..source(after));
            }
            Event::End(_) => {
                depth -= 1;
                if fallback == Some(depth) {
                    fallback = None;
                } else if open.last().is_some_and(|(at, _)| *at == depth) {
                    let (_, start) = open.pop().expect("an open paragraph");
                    spans.push(source(start)..source(after));
                }
            }
            Event::Eof => {
                spans.sort_by_key(|span| span.start);
                return Ok(spans);
            }
            _ => {}
        }
        buffer.clear();
    }
}
