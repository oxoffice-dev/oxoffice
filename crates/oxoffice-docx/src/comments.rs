//! Public comment handles and atomic document comment mutations.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;

use oxoffice_core::xml_reader::NsReader;
use oxoffice_docx_oxml::comments::{CT_Comment, CT_Comments};
use oxoffice_docx_oxml::comments_extended::{CT_CommentEx, CT_CommentsEx};
use oxoffice_docx_oxml::content_control::{CT_Sdt, SdtContent};
use oxoffice_docx_oxml::document::BodyContent;
use oxoffice_docx_oxml::table::{CT_Row, CT_Tbl, CT_Tc, CellContent};
use oxoffice_docx_oxml::text::{CT_P, CT_R, RangeAnchor, RunContent};
use oxoffice_opc::OpcPackage;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};

use crate::document::{
    CommentParagraphValue, visit_body_paragraphs_mut, visit_body_table_paragraphs_mut,
};
use crate::{ContentLocation, Document, Error, Result};

/// Refuses comment fields holding a character XML 1.0 cannot carry, as
/// python-docx refuses such text.
fn reject_non_xml_comment(author: &str, initials: Option<&str>, text: &str) -> Result<()> {
    oxoffice_core::xml::reject_non_xml_characters("comment author", author)?;
    oxoffice_core::xml::reject_non_xml_characters(
        "comment initials",
        initials.unwrap_or_default(),
    )?;
    oxoffice_core::xml::reject_non_xml_characters("comment text", text)?;
    Ok(())
}

pub(crate) const COMMENTS_EXTENDED_REL_TYPE: &str =
    "http://schemas.microsoft.com/office/2011/relationships/commentsExtended";
pub(crate) const COMMENTS_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml";
pub(crate) const COMMENTS_EXTENDED_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsExtended+xml";
const DEFAULT_COMMENTS_PART: &str = "/word/comments.xml";
const DEFAULT_COMMENTS_EXTENDED_PART: &str = "/word/commentsExtended.xml";
pub(crate) const COMMENTS_IDS_REL_TYPE: &str =
    "http://schemas.microsoft.com/office/2016/09/relationships/commentsIds";
pub(crate) const COMMENTS_EXTENSIBLE_REL_TYPE: &str =
    "http://schemas.microsoft.com/office/2018/08/relationships/commentsExtensible";
/// MS-DOCX bounds: a `w14:paraId` is below 0x80000000, a durable id of
/// `commentsIds` above 0 and below 0x7FFFFFFF.
const PARA_ID_LIMIT: u32 = 0x8000_0000;
const DURABLE_ID_LIMIT: u32 = 0x7FFF_FFFF;

/// A part beside the comments that oxoffice keeps as written, adding one
/// entry for each new comment: its relationship from the main part, content
/// type, namespace, root and entry names, and the name and prefix Word
/// gives it.
struct Companion {
    relationship: &'static str,
    content_type: &'static str,
    namespace: &'static str,
    root: &'static str,
    item: &'static str,
    part: &'static str,
    prefix: &'static str,
}

/// Each comment's durable id, keyed by its last paragraph (W10).
const COMMENT_IDS: Companion = Companion {
    relationship: COMMENTS_IDS_REL_TYPE,
    content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsIds+xml",
    namespace: "http://schemas.microsoft.com/office/word/2016/wordml/cid",
    root: "commentsIds",
    item: "commentId",
    part: "/word/commentsIds.xml",
    prefix: "w16cid",
};

/// Each comment's instant in UTC, keyed by its durable id (W10).
const COMMENT_EXTENSIBLE: Companion = Companion {
    relationship: COMMENTS_EXTENSIBLE_REL_TYPE,
    content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsExtensible+xml",
    namespace: "http://schemas.microsoft.com/office/word/2018/wordml/cex",
    root: "commentsExtensible",
    item: "commentExtensible",
    part: "/word/commentsExtensible.xml",
    prefix: "w16cex",
};

/// The authors Word knows (Q8): never made, only added to.
const PEOPLE: Companion = Companion {
    relationship: "http://schemas.microsoft.com/office/2011/relationships/people",
    content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.people+xml",
    namespace: oxoffice_docx_oxml::comments_extended::W15_NS,
    root: "people",
    item: "person",
    part: "/word/people.xml",
    prefix: "w15",
};

/// A stable insertion point between runs in a body paragraph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunPosition {
    /// Direct body child index, where a table or a block content control
    /// counts as one child. `Document::find_content_index` returns it.
    pub body_index: usize,
    /// Run boundary in the selected paragraph, counted over the runs that
    /// `Paragraph::runs` lists, including the runs inside inline content
    /// controls and tracked insertions.
    pub run_index: usize,
}

/// A half-open document run range, inclusive at `start` and exclusive at `end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RunRange {
    pub start: RunPosition,
    pub end: RunPosition,
}

/// The date a new comment, reply or tracked change carries in `w:date`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RevisionDate<'a> {
    /// The current time, in UTC to the second. Word itself writes its local
    /// time to the minute here, and for a comment the UTC time as
    /// `w16cex:dateUtc` in the extensible comments part
    /// (`commentsExtensible.xml`), which oxoffice does not write. Refused
    /// where the platform has no clock, such as WebAssembly in a browser:
    /// pass a date or [`RevisionDate::Undated`].
    #[default]
    Now,
    /// This RFC 3339 timestamp.
    At(&'a str),
    /// No date, as Word leaves comments and tracked changes once its
    /// Document Inspector removes personal information.
    Undated,
}

/// The date of a new comment or reply: [`RevisionDate`] under its first name.
pub type CommentDate<'a> = RevisionDate<'a>;

impl RevisionDate<'_> {
    /// The timestamp to write, or `None` for an undated comment. A timestamp
    /// that is not RFC 3339 is refused.
    pub(crate) fn resolve(self) -> Result<Option<String>> {
        match self {
            Self::Now => crate::revision::rfc3339_now().map(Some),
            Self::At(value) => {
                validate_comment_date(Some(value))?;
                Ok(Some(value.to_owned()))
            }
            Self::Undated => Ok(None),
        }
    }
}

/// Which match [`Document::add_comment_on_text`] anchors its comment on, and
/// what the comment carries besides its author and text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextCommentOptions<'a> {
    /// The match to anchor on, counted from zero in document order.
    pub occurrence: usize,
    /// Count only the matches in the paragraphs of table cells, nested
    /// tables included.
    pub tables_only: bool,
    /// The author's initials.
    pub initials: Option<&'a str>,
    /// The comment's date, now by default.
    pub date: CommentDate<'a>,
}

/// A stable insertion point between runs in a checked story paragraph.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StoryRunPosition {
    pub location: ContentLocation,
    pub run_index: usize,
}

/// A half-open run range within one checked story owner.
///
/// A comment anchor that [`Document::comment_anchor`] reads may instead run
/// from one table cell into a later cell of the same story, or between a cell
/// and the story around its table. Placing a range still takes one owner.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StoryRunRange {
    pub start: StoryRunPosition,
    pub end: StoryRunPosition,
}

/// The four paired marker families supported by checked story ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoryRangeKind {
    Bookmark {
        id: i32,
        name: String,
    },
    Comment {
        id: i32,
    },
    Permission {
        id: i32,
        editor: Option<String>,
        group: Option<String>,
    },
    Proofing {
        kind: String,
    },
}

/// One immutable pair of accepted-view story positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoryRangeRef {
    kind: StoryRangeKind,
    range: StoryRunRange,
    pub(crate) start_ordinal: usize,
    pub(crate) end_ordinal: usize,
}

impl StoryRangeRef {
    pub fn kind(&self) -> &StoryRangeKind {
        &self.kind
    }

    pub fn range(&self) -> &StoryRunRange {
        &self.range
    }

    pub fn bookmark_id(&self) -> Option<i32> {
        match self.kind {
            StoryRangeKind::Bookmark { id, .. } => Some(id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct StoryMarker {
    kind: StoryRangeKind,
    start: bool,
    run_index: usize,
    span: Range<usize>,
    location: ContentLocation,
    ordinal: usize,
}

fn word_marker_attribute(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    name: &[u8],
) -> Result<Option<String>> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if local.as_ref() == name
            && matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == oxoffice_docx_oxml::namespace::W_NS.as_bytes())
        {
            return Ok(Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                    .map_err(|error| Error::Other(error.to_string()))?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

fn scan_story_markers(xml: &[u8], location: &ContentLocation) -> Result<(Vec<StoryMarker>, usize)> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<(Vec<u8>, bool, Option<usize>)> = Vec::new();
    let mut markers = Vec::<StoryMarker>::new();
    let mut run_index = 0usize;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story marker scan failed: {error}")))?;
        let is_word = matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == oxoffice_docx_oxml::namespace::W_NS.as_bytes());
        drop(namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let local = element.local_name().as_ref().to_vec();
                let hidden = stack.iter().any(|(name, word, _)| {
                    *word
                        && matches!(
                            name.as_slice(),
                            b"del"
                                | b"moveFrom"
                                | b"txbxContent"
                                | b"smartTag"
                                | b"customXml"
                                | b"fldSimple"
                        )
                });
                if is_word && local == b"r" && !hidden {
                    run_index += 1;
                }
                // A comment marker inside tracked deleted or moved-away text
                // sits at the next run boundary of the accepted view, as
                // Google Docs writes a comment on a tracked change; the runs
                // there count for nothing. Other markers there stay hidden,
                // as the bookmark scan of the story text hides them.
                let marker_hidden = stack.iter().any(|(name, word, _)| {
                    *word
                        && matches!(
                            name.as_slice(),
                            b"txbxContent" | b"smartTag" | b"customXml" | b"fldSimple"
                        )
                }) || (hidden
                    && !matches!(local.as_slice(), b"commentRangeStart" | b"commentRangeEnd"));
                let marker = if is_word && !marker_hidden {
                    let id = word_marker_attribute(&reader, element, b"id")?;
                    match local.as_slice() {
                        b"bookmarkStart" | b"bookmarkEnd" => {
                            let id = id.and_then(|id| id.parse().ok()).ok_or_else(|| {
                                Error::Other("bookmark marker has no valid id".to_owned())
                            })?;
                            let name = word_marker_attribute(&reader, element, b"name")?
                                .unwrap_or_default();
                            Some((
                                StoryRangeKind::Bookmark { id, name },
                                local == b"bookmarkStart",
                            ))
                        }
                        b"commentRangeStart" | b"commentRangeEnd" => {
                            let id = id.and_then(|id| id.parse().ok()).ok_or_else(|| {
                                Error::Other("comment marker has no valid id".to_owned())
                            })?;
                            Some((
                                StoryRangeKind::Comment { id },
                                local == b"commentRangeStart",
                            ))
                        }
                        b"permStart" | b"permEnd" => {
                            let id = id.and_then(|id| id.parse().ok()).ok_or_else(|| {
                                Error::Other("permission marker has no valid id".to_owned())
                            })?;
                            let editor = word_marker_attribute(&reader, element, b"ed")?;
                            let group = word_marker_attribute(&reader, element, b"edGrp")?;
                            Some((
                                StoryRangeKind::Permission { id, editor, group },
                                local == b"permStart",
                            ))
                        }
                        b"proofErr" => {
                            let kind = word_marker_attribute(&reader, element, b"type")?;
                            match kind.as_deref() {
                                Some("spellStart") => Some((
                                    StoryRangeKind::Proofing {
                                        kind: "spell".to_owned(),
                                    },
                                    true,
                                )),
                                Some("spellEnd") => Some((
                                    StoryRangeKind::Proofing {
                                        kind: "spell".to_owned(),
                                    },
                                    false,
                                )),
                                Some("gramStart") => Some((
                                    StoryRangeKind::Proofing {
                                        kind: "gram".to_owned(),
                                    },
                                    true,
                                )),
                                Some("gramEnd") => Some((
                                    StoryRangeKind::Proofing {
                                        kind: "gram".to_owned(),
                                    },
                                    false,
                                )),
                                _ => None,
                            }
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                let marker_index = marker.map(|(kind, start)| {
                    markers.push(StoryMarker {
                        kind,
                        start,
                        run_index,
                        span: before..after,
                        location: location.clone(),
                        ordinal: 0,
                    });
                    markers.len() - 1
                });
                if matches!(event, Event::Start(_)) {
                    stack.push((local, is_word, marker_index));
                }
            }
            Event::End(_) => {
                if let Some((_, _, Some(index))) = stack.pop() {
                    markers[index].span.end = after;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok((markers, run_index))
}

/// What pairs a start with its end: the family and identifier, or the kind of
/// a proofing range, which has no identifier.
fn pairing_key(kind: &StoryRangeKind) -> (u8, i32, &str) {
    match kind {
        StoryRangeKind::Proofing { kind } => (3, 0, kind),
        _ => {
            let (family, id) = marker_identity_key(kind).expect("only proofing has no identity");
            (family, id, "")
        }
    }
}

fn marker_identity_key(kind: &StoryRangeKind) -> Option<(u8, i32)> {
    match kind {
        StoryRangeKind::Bookmark { id, .. } => Some((0, *id)),
        StoryRangeKind::Comment { id } => Some((1, *id)),
        StoryRangeKind::Permission { id, .. } => Some((2, *id)),
        StoryRangeKind::Proofing { .. } => None,
    }
}

/// Namespace closure inserts only into the root opening tag. A marker offset in
/// that closed paragraph, minus the inserted bytes, is its offset in the raw span.
fn story_marker_source_position(
    raw_span: &Range<usize>,
    closed_len: usize,
    marker_start: usize,
) -> Result<usize> {
    let raw_len = raw_span.len();
    if closed_len < raw_len {
        return Err(Error::Other(
            "story marker closure is shorter than its source span".to_owned(),
        ));
    }
    let delta = closed_len - raw_len;
    let offset = marker_start.checked_sub(delta).ok_or_else(|| {
        Error::Other("story marker position is outside its source span".to_owned())
    })?;
    if offset >= raw_len {
        return Err(Error::Other(
            "story marker position is outside its source span".to_owned(),
        ));
    }
    raw_span
        .start
        .checked_add(offset)
        .ok_or_else(|| Error::Other("story marker position is outside its source span".to_owned()))
}

/// Bookmark-start names and identifiers from the projected paragraphs.
///
/// This is the legacy `CT_P` count. It includes starts the marker scan hides,
/// and it does not depend on scanning or pairing those paragraphs.
#[allow(clippy::type_complexity)]
fn bookmark_start_names(
    paragraphs: &[(usize, usize, Range<usize>, ContentLocation, Vec<u8>)],
) -> Vec<(String, Option<i32>)> {
    let mut names = Vec::new();
    for (_, _, _, _, xml) in paragraphs {
        if let Ok(paragraph) = CT_P::from_xml_fragment(xml) {
            for marker in paragraph
                .bookmark_markers
                .iter()
                .filter(|marker| marker.is_start())
            {
                if let Some(name) = marker.name() {
                    names.push((name.to_owned(), marker.id()));
                }
            }
        }
    }
    names
}

/// The bookmark starts of the story sources, counted by name.
#[derive(Debug, Default)]
pub(crate) struct BookmarkStartNames {
    /// Every start, one with no end included: the names a new bookmark
    /// cannot take.
    pub(crate) taken: HashMap<String, usize>,
    /// The starts whose identifier has an end somewhere in the sources
    /// (all of them when the end scan fails): the bookmarks a field finds
    /// by name. A start with no end, as LibreOffice writes beside a copied
    /// form field, is no bookmark to Word.
    pub(crate) closed: HashMap<String, usize>,
}

/// The identifiers of every bookmark end of the story sources.
fn sources_bookmark_end_ids(sources: &[&[u8]]) -> Result<HashSet<i32>> {
    let mut ids = HashSet::new();
    for source in sources {
        ids.extend(crate::document::source_bookmark_end_ids(source)?);
    }
    Ok(ids)
}

/// The bookmark markers between blocks of each story source.
fn sources_block_bookmarks(
    sources: &[&[u8]],
) -> Result<Vec<Vec<crate::document::SourceBlockMarker>>> {
    sources
        .iter()
        .map(|source| crate::document::source_block_bookmarks(source))
        .collect()
}

/// The names of the bookmark starts between the blocks of every story: the
/// markers read as story paragraphs, which the story marker scan places on
/// them.
#[allow(clippy::type_complexity)]
fn block_bookmark_start_names(
    paragraphs: &[(usize, usize, Range<usize>, ContentLocation, Vec<u8>)],
    blocks: &[Vec<crate::document::SourceBlockMarker>],
) -> Vec<(String, Option<i32>)> {
    let read = paragraphs
        .iter()
        .map(|(source_index, _, span, _, _)| (*source_index, span.start))
        .collect::<HashSet<_>>();
    let mut names = Vec::new();
    for (source_index, markers) in blocks.iter().enumerate() {
        names.extend(
            markers
                .iter()
                .filter(|marker| marker.start && read.contains(&(source_index, marker.paragraph)))
                .map(|marker| (marker.name.clone(), Some(marker.id))),
        );
    }
    names
}

#[allow(clippy::type_complexity)]
fn located_story_markers(
    paragraphs: &[(usize, usize, Range<usize>, ContentLocation, Vec<u8>)],
    blocks: &[Vec<crate::document::SourceBlockMarker>],
) -> Result<(
    Vec<(usize, usize, usize, StoryMarker)>,
    HashMap<(u8, i32), (usize, usize)>,
)> {
    let mut scanned = Vec::with_capacity(paragraphs.len());
    for (_, _, _, location, xml) in paragraphs {
        scanned.push(scan_story_markers(xml, location)?);
    }
    // A bookmark marker between blocks belongs to the paragraph it wraps
    // (`source_block_bookmarks`).
    type BlockMarker = (usize, bool, i32, String);
    let mut leading = vec![Vec::<BlockMarker>::new(); paragraphs.len()];
    let mut trailing = vec![Vec::<BlockMarker>::new(); paragraphs.len()];
    let by_start = paragraphs
        .iter()
        .enumerate()
        .map(|(index, (source_index, _, span, _, _))| ((*source_index, span.start), index))
        .collect::<HashMap<_, _>>();
    for (source_index, markers) in blocks.iter().enumerate() {
        for marker in markers {
            let Some(&index) = by_start.get(&(source_index, marker.paragraph)) else {
                continue;
            };
            let entry = (marker.offset, marker.start, marker.id, marker.name.clone());
            if marker.leading {
                leading[index].push(entry);
            } else {
                trailing[index].push(entry);
            }
        }
    }

    let mut located = Vec::new();
    let mut identity_counts = HashMap::<(u8, i32), (usize, usize)>::new();
    let mut story = None;
    let mut story_sequence = 0usize;
    // Each located marker's story and its place in the source: its own
    // offset for a marker between blocks, its paragraph's start for one
    // inside, whose markers keep their scan order.
    let mut places = Vec::new();
    let mut enumeration = 0usize;
    for (index, ((source_index, _, raw_span, location, xml), (markers, run_count))) in
        paragraphs.iter().zip(scanned).enumerate()
    {
        if story.as_ref() != Some(location.story()) {
            story = Some(location.story().clone());
            story_sequence += 1;
        }
        let block = |(physical, start, id, name): &BlockMarker, run_index: usize| {
            (
                *physical,
                StoryMarker {
                    kind: StoryRangeKind::Bookmark {
                        id: *id,
                        name: if *start { name.clone() } else { String::new() },
                    },
                    start: *start,
                    run_index,
                    span: 0..0,
                    location: location.clone(),
                    ordinal: 0,
                },
            )
        };
        // `true` for a marker inside the paragraph.
        let mut all = leading[index]
            .iter()
            .map(|marker| Ok((block(marker, 0), false)))
            .collect::<Vec<Result<_>>>();
        for marker in markers {
            all.push(
                story_marker_source_position(raw_span, xml.len(), marker.span.start)
                    .map(|physical| ((physical, marker), true)),
            );
        }
        all.extend(
            trailing[index]
                .iter()
                .map(|marker| Ok((block(marker, run_count), false))),
        );
        for entry in all {
            let ((physical, marker), inside) = entry?;
            places.push((
                story_sequence,
                if inside { raw_span.start } else { physical },
            ));
            if let Some(key) = marker_identity_key(&marker.kind) {
                let counts = identity_counts.entry(key).or_default();
                if marker.start {
                    counts.0 += 1;
                } else {
                    counts.1 += 1;
                }
            }
            located.push((*source_index, physical, enumeration, marker));
            enumeration += 1;
        }
    }
    // A marker's ordinal is its rank among the markers of its story in
    // source order, as the marker selection counts them, those between
    // blocks included.
    let mut order = (0..located.len()).collect::<Vec<_>>();
    order.sort_by_key(|index| places[*index]);
    let mut previous_story = None;
    let mut ordinal = 0usize;
    for index in order {
        let (story, _) = places[index];
        if previous_story != Some(story) {
            previous_story = Some(story);
            ordinal = 0;
        }
        located[index].3.ordinal = ordinal;
        ordinal += 1;
    }
    Ok((located, identity_counts))
}

/// `bookmark_end_ids` holds the identifier of every bookmark end of the
/// sources, those the marker scan hides included.
fn pair_located_story_markers(
    located: &[(usize, usize, usize, StoryMarker)],
    identity_counts: &HashMap<(u8, i32), (usize, usize)>,
    bookmark_end_ids: &HashSet<i32>,
) -> Result<(Vec<StoryRangeRef>, Vec<StoryRangeKind>)> {
    let mut order: Vec<usize> = (0..located.len()).collect();
    order.sort_by_key(|&index| {
        let (source_index, physical, enumeration, _) = &located[index];
        (*source_index, *physical, *enumeration)
    });
    let mut bookmark_names = HashSet::new();
    let mut bookmark_ids = HashSet::new();
    let mut comment_ids = HashSet::new();
    let mut permission_ids = HashSet::new();
    let mut reserve = |kind: &StoryRangeKind| -> Result<()> {
        if let StoryRangeKind::Bookmark { name, .. } = kind
            && (name.is_empty() || !bookmark_names.insert(name.clone()))
        {
            return Err(Error::Other(
                "bookmark name is missing or duplicated".to_owned(),
            ));
        }
        let unique = match kind {
            StoryRangeKind::Bookmark { id, .. } => bookmark_ids.insert(*id),
            StoryRangeKind::Comment { id } => comment_ids.insert(*id),
            StoryRangeKind::Permission { id, editor, group } => {
                if editor.is_some() == group.is_some() {
                    return Err(Error::Other(
                        "permission start needs one editor or group".to_owned(),
                    ));
                }
                permission_ids.insert(*id)
            }
            StoryRangeKind::Proofing { .. } => true,
        };
        if !unique {
            return Err(Error::Other(
                "paired marker identity is duplicated".to_owned(),
            ));
        }
        Ok(())
    };
    // Word and Google write overlapping ranges with crossing markers, so an
    // end closes the open start of its own identity, not the last one. Proofing
    // ranges have none and nest within their own kind.
    let mut open = HashMap::<(u8, i32, &str), Vec<usize>>::new();
    let mut current_source: Option<(usize, String)> = None;
    let mut ranges = Vec::<(usize, StoryRangeRef)>::new();
    let mut across = Vec::<(u8, i32, StoryRangeKind)>::new();
    for index in order {
        let source_index = located[index].0;
        let part_name = located[index].3.location.story().part_name().to_owned();
        let is_start = located[index].3.start;
        let next_source = (source_index, part_name);
        if current_source.as_ref() != Some(&next_source) {
            if open.values().any(|starts| !starts.is_empty()) {
                return Err(Error::Other(
                    "paired marker crosses a story owner".to_owned(),
                ));
            }
            current_source = Some(next_source);
        }
        // A bookmark start whose identifier has no end anywhere in the
        // sources, as LibreOffice writes beside a copied form field, is no
        // bookmark to Word: it is left out. Its identifier stays taken (the
        // identifier scan reserves every start), and its name too
        // (`add_story_bookmark`). One whose end the scan hides is still
        // refused below.
        if is_start
            && let StoryRangeKind::Bookmark { id, .. } = &located[index].3.kind
            && !bookmark_end_ids.contains(id)
        {
            continue;
        }
        let key = pairing_key(&located[index].3.kind);
        if is_start {
            open.entry(key).or_default().push(index);
            continue;
        }
        let start_index = open
            .get_mut(&key)
            .and_then(Vec::pop)
            .ok_or_else(|| Error::Other("paired marker has an unmatched end".to_owned()))?;
        let start = &located[start_index].3;
        let end = &located[index].3;
        let same_owner = start.location.story() == end.location.story();
        let same_part = start.location.story().part_name() == end.location.story().part_name();
        let start_kind = start.kind.clone();
        let start_location = start.location.clone();
        let start_run_index = start.run_index;
        let start_ordinal = start.ordinal;
        let end_location = end.location.clone();
        let end_run_index = end.run_index;
        let end_ordinal = end.ordinal;
        let end_enumeration = located[index].2;
        if same_owner {
            reserve(&start_kind)?;
            ranges.push((
                end_enumeration,
                StoryRangeRef {
                    kind: start_kind,
                    range: StoryRunRange {
                        start: StoryRunPosition {
                            location: start_location,
                            run_index: start_run_index,
                        },
                        end: StoryRunPosition {
                            location: end_location,
                            run_index: end_run_index,
                        },
                    },
                    start_ordinal,
                    end_ordinal,
                },
            ));
            continue;
        }
        if matches!(start_kind, StoryRangeKind::Proofing { .. }) || !same_part {
            return Err(Error::Other(
                "paired marker crosses a story owner".to_owned(),
            ));
        }
        let Some(identity) = marker_identity_key(&start_kind) else {
            return Err(Error::Other(
                "paired marker crosses a story owner".to_owned(),
            ));
        };
        if identity_counts.get(&identity).copied() != Some((1, 1)) {
            return Err(Error::Other(
                "paired marker identity is duplicated".to_owned(),
            ));
        }
        reserve(&start_kind)?;
        across.push((identity.0, identity.1, start_kind));
    }
    if open.values().any(|starts| !starts.is_empty()) {
        return Err(Error::Other(
            "paired marker has an unmatched start".to_owned(),
        ));
    }
    ranges.sort_by_key(|(enumeration, _)| *enumeration);
    across.sort_by_key(|(family, id, _)| (*family, *id));
    Ok((
        ranges.into_iter().map(|(_, range)| range).collect(),
        across.into_iter().map(|(_, _, kind)| kind).collect(),
    ))
}

/// Immutable summary of one correlated bookmark or one reported marker issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmarkRef {
    id: Option<i32>,
    name: Option<String>,
    range: Option<RunRange>,
    direct_range: Option<RunRange>,
    text: String,
    issue: Option<String>,
}

impl BookmarkRef {
    pub fn id(&self) -> Option<i32> {
        self.id
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Return the accepted-view half-open range reported by `Document::bookmarks`.
    ///
    /// Its body index is the paragraph ordinal counted recursively through
    /// tables and block content controls, which field numbering reads.
    /// [`Self::direct_range`] reports the direct body child index instead.
    pub fn range(&self) -> Option<RunRange> {
        self.range
    }

    /// Return the same range with the direct body child index that
    /// `RunPosition`, `Document::add_bookmark` and
    /// `Document::find_content_index` use.
    ///
    /// It is `None` when either marker sits in a table cell or a block content
    /// control, which have no direct body index. Run indexes are the same
    /// accepted-view boundaries as [`Self::range`], which are the
    /// `RunPosition` run indexes that `Document::add_bookmark` and
    /// `Document::add_comment` take.
    pub fn direct_range(&self) -> Option<RunRange> {
        self.direct_range
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn issue(&self) -> Option<&str> {
        self.issue.as_deref()
    }
}

/// Read-only view of a comment and its thread metadata.
#[derive(Clone, Copy)]
pub struct CommentRef<'a> {
    document: &'a Document,
    inner: &'a CT_Comment,
    extension: Option<&'a CT_CommentEx>,
    parent_id: Option<i32>,
}

impl std::fmt::Debug for CommentRef<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommentRef")
            .field("inner", self.inner)
            .field("extension", &self.extension)
            .field("parent_id", &self.parent_id)
            .finish()
    }
}

impl CommentRef<'_> {
    /// Checked accepted-view range, without an invented parent range for replies.
    pub fn anchor(&self) -> Result<Option<StoryRunRange>> {
        self.document.comment_anchor(self.id())
    }

    /// Checked accepted span text. A point is empty and a known orphan is absent.
    pub fn anchor_text(&self) -> Result<Option<String>> {
        self.document.comment_anchor_text(self.id())
    }

    pub fn id(&self) -> i32 {
        self.inner.id
    }

    pub fn author(&self) -> Option<&str> {
        self.inner.author.as_deref()
    }

    pub fn initials(&self) -> Option<&str> {
        self.inner.initials.as_deref()
    }

    pub fn date(&self) -> Option<&str> {
        self.inner.date.as_deref()
    }

    pub fn text(&self) -> String {
        self.inner
            .paragraphs
            .iter()
            .map(CT_P::accepted_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn parent_id(&self) -> Option<i32> {
        self.parent_id
    }

    /// `None` when this comment has no `commentsExtended` entry.
    /// A present entry with no `w15:done` is `Some(false)`.
    pub fn resolved(&self) -> Option<bool> {
        self.extension.map(|entry| entry.done.unwrap_or(false))
    }

    /// The durable id Word keeps for the comment in `commentsIds.xml`, eight
    /// hexadecimal digits keyed by its last paragraph; `None` when the file
    /// holds none for it. It reads that part: for many comments,
    /// [`Document::comment_metadata`] reads it once.
    pub fn durable_id(&self) -> Option<String> {
        self.document
            .comment_metadata_lookup()
            .of(self.inner)
            .durable_id
    }

    /// The comment's instant in UTC, as Word writes it in
    /// `commentsExtensible.xml` beside the local time of [`Self::date`];
    /// `None` when the file holds none for it. As [`Self::durable_id`], it
    /// reads the parts.
    pub fn date_utc(&self) -> Option<String> {
        self.document
            .comment_metadata_lookup()
            .of(self.inner)
            .date_utc
    }
}

/// What Word keeps beside a comment, in the parts next to `comments.xml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommentMetadata {
    /// Its durable id, eight hexadecimal digits (`commentsIds.xml`).
    pub durable_id: Option<String>,
    /// Its instant in UTC (`commentsExtensible.xml`).
    pub date_utc: Option<String>,
}

/// The durable ids by paragraph id, and the UTC dates by durable id, both
/// keys in upper case: an unreadable part reads as empty.
struct CommentMetadataLookup {
    durable_ids: HashMap<String, String>,
    dates: HashMap<String, String>,
}

impl CommentMetadataLookup {
    fn of(&self, comment: &CT_Comment) -> CommentMetadata {
        let durable_id = last_para_id(comment)
            .and_then(|para_id| self.durable_ids.get(&para_id.to_ascii_uppercase()))
            .cloned();
        let date_utc = durable_id
            .as_ref()
            .and_then(|durable_id| self.dates.get(&durable_id.to_ascii_uppercase()))
            .cloned();
        CommentMetadata {
            durable_id,
            date_utc,
        }
    }
}

impl Document {
    /// Add a bookmark across one checked story owner.
    pub fn add_story_bookmark(&mut self, name: &str, range: StoryRunRange) -> Result<i32> {
        validate_bookmark_name(name)?;
        let (paired, start_names) = self.story_range_metadata()?;
        let (ranges, across) = paired?;
        // Every bookmark start names a bookmark, those left out of the
        // ranges included.
        if ranges.iter().map(StoryRangeRef::kind).chain(&across).any(|kind| {
            matches!(kind, StoryRangeKind::Bookmark { name: existing, .. } if oxoffice_docx_oxml::text::same_bookmark_name(existing, name))
        }) || start_names
            .taken
            .keys()
            .any(|existing| oxoffice_docx_oxml::text::same_bookmark_name(existing, name))
        {
            return Err(Error::Other(format!("bookmark name {name} already exists")));
        }
        let mut candidate = self.clone_for_staging();
        let mut identifiers = candidate.identifiers.clone();
        let id = identifiers.reserve_bookmark_id()?;
        candidate.anchor_story_range(
            &range,
            RangeAnchor::Bookmark { id, name },
            "bookmark",
            false,
        )?;
        candidate.identifiers = identifiers;
        candidate.story_ranges()?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(id)
    }

    /// Add a permission range with exactly one editor or editor group.
    pub fn add_story_permission_range(
        &mut self,
        editor: Option<&str>,
        group: Option<&str>,
        range: StoryRunRange,
    ) -> Result<i32> {
        check_permission_holder(editor, group)?;
        let mut candidate = self.clone_for_staging();
        // Reserved on the candidate, whose registry a success commits and a
        // refusal drops with it.
        let id = candidate.identifiers.reserve_permission_id()?;
        candidate.anchor_story_range(
            &range,
            RangeAnchor::Permission { id, editor, group },
            "permission",
            false,
        )?;
        candidate.story_ranges()?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(id)
    }

    /// [`Self::add_story_permission_range`] for a range whose two ends sit
    /// in one direct body paragraph of the main part
    /// (`direct_body_paragraph_at`, `block` found by the caller),
    /// swapped in place under a `BodyBlocks` unit, the id reserved from the
    /// registry in the same edit: `Some(id)`. The markers of other
    /// paragraphs and stories are not read: a document whose markers do not
    /// pair keeps them as they are, and the range added pairs. `None`,
    /// changing nothing, sends the caller to
    /// [`Self::add_story_permission_range`].
    #[doc(hidden)]
    pub fn add_story_permission_range_in_block(
        &mut self,
        editor: Option<&str>,
        group: Option<&str>,
        range: &StoryRunRange,
        block: crate::ControlBlock,
    ) -> Result<Option<i32>> {
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let result =
            self.add_story_permission_range_in_block_unchecked(editor, group, range, block);
        #[cfg(debug_assertions)]
        match &result {
            Ok(None) => assert!(
                self.clone_for_staging().to_bytes().ok()
                    == before.clone_for_staging().to_bytes().ok(),
                "a permission range left to the staged route changed the document"
            ),
            // The staged route refuses a document whose range markers did not
            // pair before the call, in any story; this route reads and writes
            // the markers of its paragraph alone, a pair of a fresh id.
            Ok(Some(_)) if before.story_ranges().is_err() => {}
            _ => self.assert_in_place_matches_staged_declarations_aside(
                before,
                &result
                    .as_ref()
                    .map(|id| id.unwrap_or_default())
                    .map_err(|error| Error::Other(error.to_string())),
                |staged| {
                    let mut range = range.clone();
                    range.start.location = staged.fingerprinted(&range.start.location);
                    range.end.location = staged.fingerprinted(&range.end.location);
                    staged.add_story_permission_range(editor, group, range)
                },
            ),
        }
        result
    }

    fn add_story_permission_range_in_block_unchecked(
        &mut self,
        editor: Option<&str>,
        group: Option<&str>,
        range: &StoryRunRange,
        block: crate::ControlBlock,
    ) -> Result<Option<i32>> {
        check_permission_holder(editor, group)?;
        let Some(id) = self.identifiers.lowest_free_permission_id() else {
            return Ok(None);
        };
        let Some(anchored) = self.story_range_anchored_block(
            range,
            block,
            RangeAnchor::Permission { id, editor, group },
            "permission",
        )?
        else {
            return Ok(None);
        };
        self.replace_body_block_in_place_with(block.slot, anchored, |document| {
            let reserved = document.identifiers.reserve_permission_id()?;
            debug_assert_eq!(
                reserved, id,
                "the permission id written is the one reserved"
            );
            Ok(())
        })?;
        Ok(Some(id))
    }

    /// Add a spelling (`spell`) or grammar (`gram`) proofing range.
    pub fn add_story_proofing_range(&mut self, kind: &str, range: StoryRunRange) -> Result<()> {
        if !matches!(kind, "spell" | "gram") {
            return Err(Error::Other(
                "proofing kind must be spell or gram".to_owned(),
            ));
        }
        self.story_ranges()?;
        let mut candidate = self.clone_for_staging();
        candidate.anchor_story_range(&range, RangeAnchor::Proofing { kind }, "proofing", false)?;
        candidate.story_ranges()?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Remove one checked bookmark pair by its identifier, a marker between
    /// paragraphs included.
    pub fn remove_story_bookmark(&mut self, id: i32) -> Result<bool> {
        let (ranges, across) = self.story_range_inventory()?;
        if across.iter().any(
            |kind| matches!(kind, StoryRangeKind::Bookmark { id: existing, .. } if *existing == id),
        ) {
            return Err(Error::Other(format!(
                "bookmark {id} runs across story owners and is not one story range"
            )));
        }
        let entry = ranges.into_iter().find(|entry| {
            matches!(entry.kind, StoryRangeKind::Bookmark { id: existing, .. } if existing == id)
        });
        let Some(entry) = entry else {
            return Ok(false);
        };
        let mut candidate = self.clone_for_staging();
        candidate.remove_story_range_markers(&entry, false)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(true)
    }

    /// Move one existing root comment and its replies without changing their
    /// thread identity.
    ///
    /// Each reply with its own range and reference crosses the root's new
    /// range as [`Self::reply_to`] writes it. A reply without any marker
    /// follows through the thread metadata. Unknown IDs, replies, incomplete
    /// anchors, a reply anchor that cannot travel and unsupported
    /// destinations are refused without publishing any change. Moving a root
    /// onto its own range publishes nothing when every anchored reply already
    /// crosses it, and otherwise brings the replies back onto it.
    pub fn move_comment(&mut self, id: i32, range: StoryRunRange) -> Result<()> {
        let selected = self.checked_movable_comment(id)?;
        if self.thread_on_range(id, &range)? {
            self.story_range_paragraph_snapshot(&range.start.location)?;
            if range.end.location != range.start.location {
                self.story_range_paragraph_snapshot(&range.end.location)?;
            }
            return Ok(());
        }
        self.move_story_range(&selected, range)
    }

    /// Move an existing root comment and its replies onto a literal
    /// main-story occurrence, as [`Self::move_comment`] does. Search and run
    /// splitting follow [`Self::add_comment_on_text`].
    pub fn move_comment_to_text(&mut self, id: i32, anchor: &str, occurrence: usize) -> Result<()> {
        let selected = self.checked_movable_comment(id)?;
        let mut candidate = self.clone_for_staging();
        // The move reads the main part as its source authority, which an
        // in-place edit such as `add_comment` leaves behind the typed body.
        candidate.flush_to_package()?;
        let replies = candidate.anchored_thread_replies(id)?;
        let reference = candidate.remove_comment_source_for_move(&selected)?;
        candidate.anchor_existing_comment_on_text(id, anchor, occurrence, false, None)?;
        candidate.restore_comment_reference_run(id, reference)?;
        candidate.move_replies_with_root(id, &replies)?;
        candidate.flush_to_package()?;
        candidate.comment_ownership_at(&candidate.doc_part_name)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// The anchored replies of `root`, parents first and siblings in id
    /// order, each with the nearest anchored ancestor its range crosses.
    /// Replying in that order writes the markers [`Self::reply_to`] writes.
    /// A reply without any marker follows through thread metadata alone.
    /// Any other reply anchor cannot travel with the thread and refuses
    /// before an edit. `self` must be flushed.
    fn anchored_thread_replies(&self, root: i32) -> Result<Vec<(i32, i32)>> {
        let ownership = self.comment_ownership_at(&self.doc_part_name)?;
        let counts = CommentOwnership::marker_counts(&ownership.markers);
        let mut replies = Vec::new();
        let mut parents = std::collections::VecDeque::from([(root, root)]);
        while let Some((parent, anchored)) = parents.pop_front() {
            for (&reply, _) in ownership
                .parents
                .iter()
                .filter(|(_, found)| **found == Some(parent))
            {
                match counts.get(&reply).copied().unwrap_or_default() {
                    [0, 0, 0] => parents.push_back((reply, anchored)),
                    [1, 1, 1] => {
                        replies.push((reply, anchored));
                        parents.push_back((reply, reply));
                    }
                    _ => {
                        return Err(Error::Other(format!(
                            "comment {root} cannot move: reply {reply} has an incomplete anchor"
                        )));
                    }
                }
            }
        }
        Ok(replies)
    }

    /// Bring each anchored reply onto its ancestor's current range on this
    /// staged candidate, after the root moved. The original reference run
    /// travels as the root's does. A failure names the root and the reply.
    fn move_replies_with_root(&mut self, root: i32, replies: &[(i32, i32)]) -> Result<()> {
        for &(reply, anchored) in replies {
            let mut moved = || -> Result<()> {
                self.flush_to_package()?;
                let selected = self.checked_flushed_comment_source(reply, false)?;
                let reference = self.remove_comment_source_for_move(&selected)?;
                let (_, written) = self.anchor_reply_crossing(anchored, Some(reply))?;
                // Anchoring rewrote that owner, so take its current identity.
                let story = self
                    .stories()?
                    .into_iter()
                    .find(|story| {
                        story.kind() == written.kind()
                            && story.part_name() == written.part_name()
                            && story.owner_index() == written.owner_index()
                    })
                    .ok_or_else(|| Error::Other("reply story disappeared".into()))?;
                self.splice_comment_reference_run(&story, reply, reference)
            };
            moved().map_err(|error| {
                Error::Other(format!(
                    "comment {root} cannot move: reply {reply}: {error}"
                ))
            })?;
        }
        Ok(())
    }

    /// Whether `root` already has `range` and every anchored reply crosses
    /// its ancestor's range, as [`Self::reply_to`] writes it.
    fn thread_on_range(&self, root: i32, range: &StoryRunRange) -> Result<bool> {
        let mut proof = self.clone_for_staging();
        proof.flush_to_package()?;
        let read = CommentAnchorRead::from_prepared(&proof, None)?;
        if read.project(root)?.0.as_ref() != Some(range) {
            return Ok(false);
        }
        for (reply, anchored) in proof.anchored_thread_replies(root)? {
            let (Some(reply), Some(anchored)) = (read.project(reply)?.0, read.project(anchored)?.0)
            else {
                return Ok(false);
            };
            if reply.start != anchored.start || reply.end.location != anchored.end.location {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn checked_movable_comment(&self, id: i32) -> Result<StoryRangeRef> {
        self.checked_comment_source(id, true)
    }

    /// Select one comment's complete paired range and reference. `root`
    /// refuses a reply, which only moves with its thread.
    pub(crate) fn checked_comment_source(&self, id: i32, root: bool) -> Result<StoryRangeRef> {
        let mut proof = self.clone_for_staging();
        proof.flush_to_package()?;
        proof.checked_flushed_comment_source(id, root)
    }

    /// [`Self::checked_comment_source`] on a document whose package is
    /// already flushed, such as a staged candidate.
    fn checked_flushed_comment_source(&self, id: i32, root: bool) -> Result<StoryRangeRef> {
        let ownership = self.comment_ownership_at(&self.doc_part_name)?;
        match ownership.parents.get(&id) {
            None => return Err(Error::Other(format!("unknown comment {id}"))),
            Some(Some(_)) if root => {
                return Err(Error::Other(format!(
                    "comment {id} is a reply, not a movable root: it moves with its thread root {}",
                    ownership.thread_root(id)
                )));
            }
            Some(_) => {}
        }
        let counts = CommentOwnership::marker_counts(&ownership.markers);
        if counts.get(&id) != Some(&[1, 1, 1]) {
            return Err(Error::Other(format!(
                "comment {id} requires one complete paired range and reference"
            )));
        }
        // This exact accepted projection also rejects hidden, reversed,
        // cross-story or unrepresentable endpoints rather than inventing one.
        // A range across table cells is read but placing it takes one owner.
        let anchor = CommentAnchorRead::from_prepared(self, Some(id))?
            .project(id)?
            .0
            .ok_or_else(|| Error::Other(format!("comment {id} has no movable paired range")))?;
        if anchor.start.location.story() != anchor.end.location.story() {
            return Err(Error::Other(format!(
                "comment {id} range runs across story owners and cannot be moved"
            )));
        }
        let (sources, paragraphs) = self.story_range_located_paragraphs_with_sources()?;
        let sources = sources.iter().map(AsRef::as_ref).collect::<Vec<_>>();
        let blocks = sources_block_bookmarks(&sources)?;
        let (mut located, identity_counts) = located_story_markers(&paragraphs, &blocks)?;
        located.retain(|(_, _, _, marker)| {
            matches!(marker.kind, StoryRangeKind::Comment { id: found } if found == id)
        });
        // Only comment markers are left: no bookmark start to leave out.
        let (ranges, across) =
            pair_located_story_markers(&located, &identity_counts, &HashSet::new())?;
        if !across.is_empty() {
            return Err(Error::Other(format!(
                "comment {id} range runs across story owners and cannot be moved"
            )));
        }
        let mut selected = ranges.into_iter();
        let entry = selected
            .next()
            .ok_or_else(|| Error::Other(format!("comment {id} has no selected source range")))?;
        if selected.next().is_some() {
            return Err(Error::Other(format!(
                "comment {id} has ambiguous source ranges"
            )));
        }
        Ok(entry)
    }

    /// Move one checked pair to `range`. A bookmark marker between
    /// paragraphs moves too: the pair is written inside the paragraphs of
    /// `range`.
    pub fn move_story_range(
        &mut self,
        selected: &StoryRangeRef,
        range: StoryRunRange,
    ) -> Result<()> {
        if let StoryRangeKind::Comment { id } = selected.kind() {
            let checked = self.checked_movable_comment(*id)?;
            if &checked != selected {
                return Err(Error::Other("selected comment range is stale".into()));
            }
            let mut candidate = self.clone_for_staging();
            // As in `move_comment_to_text`, bring the main part up to date.
            candidate.flush_to_package()?;
            let replies = candidate.anchored_thread_replies(*id)?;
            candidate.move_comment_story_range(selected, &range, *id)?;
            candidate.move_replies_with_root(*id, &replies)?;
            candidate.flush_to_package()?;
            candidate.comment_ownership_at(&candidate.doc_part_name)?;
            candidate.checked_movable_comment(*id)?;
            let reopened = candidate.prepare_and_reopen_staged()?;
            self.commit_staged_mutation(reopened);
            return Ok(());
        }
        if !self.story_ranges()?.contains(selected) {
            return Err(Error::Other("selected story range is stale".to_owned()));
        }
        let mut candidate = self.clone_for_staging();
        let (removed_part, removed) = candidate.remove_story_range_markers(selected, true)?;
        let anchor = match selected.kind() {
            StoryRangeKind::Bookmark { id, name } => RangeAnchor::Bookmark { id: *id, name },
            StoryRangeKind::Comment { id } => RangeAnchor::Comment(*id),
            StoryRangeKind::Permission { id, editor, group } => RangeAnchor::Permission {
                id: *id,
                editor: editor.as_deref(),
                group: group.as_deref(),
            },
            StoryRangeKind::Proofing { kind } => RangeAnchor::Proofing { kind },
        };
        let mut placement_check = self.clone_for_staging();
        placement_check.anchor_story_range(&range, anchor, "story", false)?;
        let refreshed = candidate.stories()?;
        let rebase = |position: &StoryRunPosition| -> Result<StoryRunPosition> {
            let source = position.location.story();
            let story = refreshed
                .iter()
                .find(|story| {
                    story.kind() == source.kind()
                        && story.part_name() == source.part_name()
                        && story.owner_index() == source.owner_index()
                })
                .ok_or_else(|| {
                    Error::Other("target story disappeared during range move".to_owned())
                })?;
            // A removed marker between blocks before the position was an
            // item of its story.
            let mut index_path = position.location.index_path().to_vec();
            if source.part_name() == removed_part
                && let Some(first) = index_path.first_mut()
            {
                *first -= self.story_items_among(source, &removed, *first)?;
            }
            Ok(StoryRunPosition {
                location: ContentLocation::new(
                    story.clone(),
                    position.location.item_kind(),
                    index_path,
                ),
                run_index: position.run_index,
            })
        };
        let range = StoryRunRange {
            start: rebase(&range.start)?,
            end: rebase(&range.end)?,
        };
        candidate.anchor_story_range(&range, anchor, "story", false)?;
        candidate.story_ranges()?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Remove one checked pair, a bookmark marker between paragraphs
    /// included. Its attached comment definition, if any, remains a separate
    /// comment operation.
    pub fn remove_story_range(&mut self, selected: &StoryRangeRef) -> Result<bool> {
        if !self.story_ranges()?.contains(selected) {
            return Ok(false);
        }
        let mut candidate = self.clone_for_staging();
        candidate.remove_story_range_markers(selected, false)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(true)
    }

    /// Return checked paired-marker ranges in legacy story order.
    ///
    /// Each end closes the start of its own identifier, so overlapping
    /// ranges, whose markers cross as Word and Google write them, are listed
    /// each with its own start and end. A bookmark start whose identifier
    /// has no end is left out, as Word ignores it; its name and identifier
    /// still count as taken. Other unmatched, reversed, duplicate and
    /// cross-part markers are rejected. A unique bookmark, comment or
    /// permission pair with one start and one end in two owners of one part
    /// is omitted when its physical order is valid. Its name and identifier
    /// still count as taken. Proofing is never omitted across owners.
    /// [`Self::comment_anchor`] reads a comment on its own.
    pub fn story_ranges(&self) -> Result<Vec<StoryRangeRef>> {
        Ok(self.story_range_inventory()?.0)
    }

    /// [`Self::story_ranges`] with the kinds of the pairs it leaves out
    /// across owners, whose names and identifiers stay taken.
    pub(crate) fn story_range_inventory(
        &self,
    ) -> Result<(Vec<StoryRangeRef>, Vec<StoryRangeKind>)> {
        let (paired, _) = self.story_range_metadata()?;
        paired
    }

    /// Pairing result beside bookmark-start name counts from one inventory.
    ///
    /// The outer result fails only when the paragraph projection fails. Name
    /// counts still return when marker scanning or pairing fails. They are the
    /// legacy `CT_P` bookmark-start counts, see [`BookmarkStartNames`]. Owner-grouped paragraph order still
    /// assigns ordinals and the returned range order. Validation order is the
    /// source index and the translated raw byte position.
    #[allow(clippy::type_complexity)]
    pub(crate) fn story_range_metadata(
        &self,
    ) -> Result<(
        Result<(Vec<StoryRangeRef>, Vec<StoryRangeKind>)>,
        BookmarkStartNames,
    )> {
        let (sources, paragraphs) = self.story_range_located_paragraphs_with_sources()?;
        let sources = sources.iter().map(AsRef::as_ref).collect::<Vec<_>>();
        // A bookmark start between the blocks of any story names a bookmark
        // too.
        let blocks = sources_block_bookmarks(&sources)?;
        let bookmark_end_ids = sources_bookmark_end_ids(&sources);
        let mut names = BookmarkStartNames::default();
        for (name, id) in bookmark_start_names(&paragraphs)
            .into_iter()
            .chain(block_bookmark_start_names(&paragraphs, &blocks))
        {
            let closed = match (&bookmark_end_ids, id) {
                (Ok(end_ids), Some(id)) => end_ids.contains(&id),
                _ => true,
            };
            *names.taken.entry(name.clone()).or_default() += 1;
            if closed {
                *names.closed.entry(name).or_default() += 1;
            }
        }
        let paired = bookmark_end_ids.and_then(|bookmark_end_ids| {
            let (located, identity_counts) = located_story_markers(&paragraphs, &blocks)?;
            pair_located_story_markers(&located, &identity_counts, &bookmark_end_ids)
        });
        Ok((paired, names))
    }

    /// Whether the bookmark markers with this identifier that sit between
    /// blocks lead the paragraph they are read as (`true`) or close it, for
    /// the start and for the end; `None` when no such marker or when they
    /// disagree.
    pub(crate) fn block_bookmark_sides(&self, id: i32) -> Result<(Option<bool>, Option<bool>)> {
        let (sources, _) = self.story_range_located_paragraphs_with_sources()?;
        let sources = sources.iter().map(AsRef::as_ref).collect::<Vec<_>>();
        let mut sides = (HashSet::new(), HashSet::new());
        for marker in sources_block_bookmarks(&sources)?.into_iter().flatten() {
            if marker.id == id {
                if marker.start {
                    sides.0.insert(marker.leading);
                } else {
                    sides.1.insert(marker.leading);
                }
            }
        }
        let one =
            |side: HashSet<bool>| (side.len() == 1).then(|| side.into_iter().next()).flatten();
        Ok((one(sides.0), one(sides.1)))
    }

    pub(crate) fn ensure_fragment_comment_models_staged(&mut self) -> Result<String> {
        self.ensure_comment_models()?;
        self.ensure_comment_relationships()?;
        self.comments_part_name
            .clone()
            .ok_or_else(|| Error::Other("comments part name is missing".to_owned()))
    }

    pub(crate) fn fragment_comment_dependency_xml(
        source: Option<&CT_Comments>,
        source_extended: Option<&CT_CommentsEx>,
        ids: &[String],
    ) -> Result<Option<Vec<u8>>> {
        if ids.is_empty() {
            return Ok(None);
        }
        let source = source.ok_or_else(|| {
            Error::Other("document fragment references a missing comments part".to_owned())
        })?;
        let included_ids = selected_fragment_comment_ids(source, source_extended, ids)?;
        let mut selected = source.clone();
        selected
            .comments
            .retain(|comment| included_ids.contains(&comment.id));
        selected.extra_xml.clear();
        Ok(Some(selected.to_xml()?))
    }

    pub(crate) fn import_fragment_comments_staged(
        &mut self,
        source: Option<&CT_Comments>,
        source_extended: Option<&CT_CommentsEx>,
        ids: &[String],
    ) -> Result<BTreeMap<String, String>> {
        if ids.is_empty() {
            return Ok(BTreeMap::new());
        }
        let source = source.ok_or_else(|| {
            Error::Other("document fragment references a missing comments part".to_owned())
        })?;
        self.ensure_comment_models()?;
        self.ensure_comment_relationships()?;
        let included_ids = selected_fragment_comment_ids(source, source_extended, ids)?;

        let mut remap = BTreeMap::<String, String>::new();
        let mut paragraph_remap = HashMap::<String, String>::new();
        let mut reserved_para_ids =
            occupied_para_ids(self.comments.as_ref(), self.comments_extended.as_ref());
        for comment in source
            .comments
            .iter()
            .filter(|comment| included_ids.contains(&comment.id))
        {
            let new_id = self.identifiers.reserve_comment_id()?;
            remap.insert(comment.id.to_string(), new_id.to_string());
            for old_para_id in comment.paragraph_ids.iter().flatten() {
                let new_para_id = allocate_para_id_from_occupied(&mut reserved_para_ids)?;
                paragraph_remap.insert(old_para_id.clone(), new_para_id);
            }
        }

        let mut imported_comments = Vec::new();
        for source_comment in source
            .comments
            .iter()
            .filter(|comment| included_ids.contains(&comment.id))
        {
            let new_id = remap
                .get(&source_comment.id.to_string())
                .and_then(|id| id.parse::<i32>().ok())
                .ok_or_else(|| {
                    Error::Other("document fragment comment remap is incomplete".to_owned())
                })?;
            let mut imported = source_comment.clone();
            imported.id = new_id;
            for para_id in imported.paragraph_ids.iter_mut().flatten() {
                *para_id = paragraph_remap.get(para_id).cloned().ok_or_else(|| {
                    Error::Other("document fragment paragraph remap is incomplete".to_owned())
                })?;
            }
            imported_comments.push(imported);
        }
        self.comments
            .as_mut()
            .expect("fragment comment models were initialized")
            .comments
            .extend(imported_comments);

        if let Some(source_extended) = source_extended {
            let mut imported_extended = Vec::new();
            for extension in &source_extended.comments {
                let Some(para_id) = paragraph_remap.get(&extension.para_id).cloned() else {
                    continue;
                };
                let para_id_parent = extension
                    .para_id_parent
                    .as_ref()
                    .map(|parent| {
                        paragraph_remap.get(parent).cloned().ok_or_else(|| {
                            Error::Other(
                                "document fragment comment parent is outside the selected closure"
                                    .to_owned(),
                            )
                        })
                    })
                    .transpose()?;
                let mut imported = extension.clone();
                imported.para_id = para_id;
                imported.para_id_parent = para_id_parent;
                imported_extended.push(imported);
            }
            self.comments_extended
                .as_mut()
                .expect("fragment comment extension model was initialized")
                .comments
                .extend(imported_extended);
        }
        self.comments_dirty = true;
        Ok(remap)
    }

    /// Return bookmarks and malformed marker reports in main-story paragraph order.
    ///
    /// Reported body indexes count typed paragraphs recursively through tables and
    /// block content controls. Reported run indexes use accepted-view run boundaries.
    /// `BookmarkRef::direct_range` also reports the direct body child index when
    /// both markers sit in direct body paragraphs.
    pub fn bookmarks(&self) -> Vec<BookmarkRef> {
        #[derive(Clone)]
        struct Marker {
            position: RunPosition,
            direct_position: Option<RunPosition>,
            start: bool,
            id: Option<i32>,
            name: Option<String>,
        }

        let mut markers = Vec::new();
        let mut paragraphs = Vec::new();
        for (direct_index, item) in self.document.body.content.iter().enumerate() {
            let mut item_paragraphs = Vec::new();
            collect_main_story_paragraphs(std::slice::from_ref(item), &mut item_paragraphs);
            let direct_index = matches!(item, BodyContent::Paragraph(_)).then_some(direct_index);
            paragraphs.extend(
                item_paragraphs
                    .into_iter()
                    .map(|paragraph| (paragraph, direct_index)),
            );
        }
        // A marker between blocks belongs to the paragraph it wraps.
        let block_markers =
            oxoffice_docx_oxml::text::BlockBookmarks::of_blocks(&self.document.body.content);
        for (body_index, (paragraph, direct_index)) in paragraphs.into_iter().enumerate() {
            for marker in block_markers.paragraph_projection(paragraph) {
                let run_index = marker.projected_run_index();
                markers.push(Marker {
                    position: RunPosition {
                        body_index,
                        run_index,
                    },
                    direct_position: direct_index.map(|body_index| RunPosition {
                        body_index,
                        run_index,
                    }),
                    start: marker.is_start(),
                    id: marker.id(),
                    name: marker.name().map(str::to_owned),
                });
            }
        }

        let mut by_id: HashMap<i32, Vec<usize>> = HashMap::new();
        let mut results = Vec::new();
        for (index, marker) in markers.iter().enumerate() {
            if let Some(id) = marker.id {
                by_id.entry(id).or_default().push(index);
            } else {
                results.push((
                    index,
                    BookmarkRef {
                        id: None,
                        name: marker.name.clone(),
                        range: None,
                        direct_range: None,
                        text: String::new(),
                        issue: Some("bookmark marker has a malformed or missing id".to_owned()),
                    },
                ));
            }
        }

        for (id, indices) in by_id {
            let starts = indices
                .iter()
                .copied()
                .filter(|index| markers[*index].start)
                .collect::<Vec<_>>();
            let ends = indices
                .iter()
                .copied()
                .filter(|index| !markers[*index].start)
                .collect::<Vec<_>>();
            let first = indices.iter().copied().min().unwrap_or(0);
            let name = starts
                .first()
                .and_then(|index| markers[*index].name.clone());
            let (range, issue) = if starts.len() != 1 || ends.len() != 1 {
                (
                    None,
                    Some(format!(
                        "bookmark id {id} has {} start markers and {} end markers",
                        starts.len(),
                        ends.len()
                    )),
                )
            } else if name.is_none() {
                (None, Some(format!("bookmark id {id} has a missing name")))
            } else {
                let candidate = RunRange {
                    start: markers[starts[0]].position,
                    end: markers[ends[0]].position,
                };
                if candidate.start > candidate.end
                    || (candidate.start == candidate.end && starts[0] > ends[0])
                {
                    (
                        None,
                        Some(format!("bookmark id {id} ends before it starts")),
                    )
                } else {
                    (Some(candidate), None)
                }
            };
            let direct_range = range.and_then(|_| {
                Some(RunRange {
                    start: markers[starts[0]].direct_position?,
                    end: markers[ends[0]].direct_position?,
                })
            });
            let text = range
                .map(|_| {
                    bookmark_range_text(
                        &self.document.body.content,
                        markers[starts[0]].position.body_index,
                        markers[starts[0]].position.run_index,
                        markers[ends[0]].position.body_index,
                        markers[ends[0]].position.run_index,
                    )
                })
                .unwrap_or_default();
            results.push((
                first,
                BookmarkRef {
                    id: Some(id),
                    name,
                    range,
                    direct_range,
                    text,
                    issue,
                },
            ));
        }

        let mut name_counts = HashMap::new();
        for (_, bookmark) in &results {
            if bookmark.range.is_some()
                && let Some(name) = bookmark.name.as_deref()
            {
                *name_counts.entry(name.to_owned()).or_insert(0usize) += 1;
            }
        }
        for (_, bookmark) in &mut results {
            if bookmark
                .name
                .as_ref()
                .is_some_and(|name| name_counts.get(name).copied().unwrap_or(0) > 1)
            {
                bookmark.issue = Some(format!(
                    "bookmark name {} is duplicated",
                    bookmark.name.as_deref().unwrap_or("")
                ));
                bookmark.range = None;
                bookmark.direct_range = None;
                bookmark.text.clear();
            }
        }
        results.sort_by_key(|(index, _)| *index);
        results.into_iter().map(|(_, bookmark)| bookmark).collect()
    }

    /// Insert a bookmark over a half-open range of body paragraph runs.
    ///
    /// Run indexes count the runs that `Paragraph::runs` lists. The markers
    /// go inside an inline content control when the range starts or ends
    /// between two of its runs, and around the control when the range covers
    /// it. A range that cannot be anchored exactly, such as one that crosses
    /// the edge of a control or ends between two runs of a tracked insertion,
    /// is an error and leaves the document unchanged.
    pub fn add_bookmark(&mut self, name: &str, range: RunRange) -> Result<i32> {
        self.insert_bookmark(name, range)
    }

    fn insert_bookmark(&mut self, name: &str, range: RunRange) -> Result<i32> {
        validate_bookmark_name(name)?;
        validate_bookmark_range(&self.document.body.content, range)?;
        if self.bookmarks().iter().any(|bookmark| {
            bookmark.name().is_some_and(|existing| {
                oxoffice_docx_oxml::text::same_bookmark_name(existing, name)
            })
        }) {
            return Err(Error::Other(format!("bookmark name {name} already exists")));
        }
        let mut identifiers = self.identifiers.clone();
        let id = identifiers.reserve_bookmark_id()?;
        anchor_body_range(
            &mut self.document.body.content,
            range,
            RangeAnchor::Bookmark { id, name },
            "bookmark",
        )?;
        self.identifiers = identifiers;
        self.invalidate_layout();
        Ok(id)
    }

    /// The number of paragraphs of comment `id`, `None` when there is no
    /// such comment. The binding asks it before an edit that adds a
    /// paragraph to an empty comment.
    #[doc(hidden)]
    pub fn comment_paragraph_count(&self, id: i32) -> Option<usize> {
        self.comments
            .as_ref()?
            .comments
            .iter()
            .find(|comment| comment.id == id)
            .map(|comment| comment.paragraphs.len())
    }

    /// Return comments in their package part order.
    pub fn comments(&self) -> Vec<CommentRef<'_>> {
        let Some(comments) = self.comments.as_ref() else {
            return Vec::new();
        };
        let by_para_id = comments
            .comments
            .iter()
            .flat_map(|comment| para_ids(comment).map(|para_id| (para_id, comment.id)))
            .collect::<HashMap<_, _>>();
        comments
            .comments
            .iter()
            .map(|comment| {
                let extension = last_para_id(comment).and_then(|para_id| {
                    self.comments_extended
                        .as_ref()?
                        .comments
                        .iter()
                        .find(|entry| entry.para_id == para_id)
                });
                let parent_id = extension
                    .and_then(|entry| entry.para_id_parent.as_deref())
                    .and_then(|para_id| by_para_id.get(para_id).copied());
                CommentRef {
                    document: self,
                    inner: comment,
                    extension,
                    parent_id,
                }
            })
            .collect()
    }

    /// Add a comment over a half-open range of body paragraph runs.
    ///
    /// Run indexes count the runs that `Paragraph::runs` lists, and the
    /// range markers are placed as [`Self::add_bookmark`] places its markers.
    /// The reference run follows the end marker. A range that cannot be
    /// anchored exactly is an error and leaves the document unchanged. Each
    /// line of `text` becomes one paragraph of the comment, dated now as
    /// [`CommentDate::Now`] dates it.
    pub fn add_comment(
        &mut self,
        range: RunRange,
        author: &str,
        initials: Option<&str>,
        text: &str,
    ) -> Result<i32> {
        self.add_comment_with_date(range, author, initials, text, CommentDate::Now)
    }

    /// Add a comment over a half-open range of body paragraph runs, dated as
    /// `date` says: now, at an RFC 3339 timestamp, or not at all. Each line
    /// of `text` becomes one paragraph of the comment.
    pub fn add_comment_with_date(
        &mut self,
        range: RunRange,
        author: &str,
        initials: Option<&str>,
        text: &str,
        date: CommentDate<'_>,
    ) -> Result<i32> {
        reject_non_xml_comment(author, initials, text)?;
        let date = date.resolve()?;
        let date = date.as_deref();
        // The paragraphs of the range, when they are in the body: the
        // comment is anchored on them in place; else the staged route
        // answers the refusal.
        let slots = range.start.body_index..range.end.body_index.saturating_add(1);
        if slots.is_empty() || slots.end > self.document.body.content.len() {
            return self.add_comment_with_date_staged(range, author, initials, text, date);
        }
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let savable_before = self.body_known_savable();
        let result = self.edit_in_place(
            // The flush of pending typed notes and comments writes their
            // parts; the first comment adds its parts, relationships and
            // overrides.
            &[
                crate::EditUnit::BodyBlocks {
                    slot: slots.start,
                    count: slots.len(),
                },
                crate::EditUnit::Comments,
                crate::EditUnit::Footnotes,
                crate::EditUnit::Styles,
                crate::EditUnit::Parts,
            ],
            |document| {
                let id = document.add_comment_staged(range, author, initials, text, date)?;
                document.flush_dirty_related_story_models()?;
                Ok(id)
            },
        );
        // The paragraphs took typed marks: a later edit need not check the
        // whole main part again.
        if result.is_ok() {
            self.keep_body_savable_after_typed_edits(slots.clone(), savable_before);
        }
        #[cfg(debug_assertions)]
        self.assert_in_place_matches_staged_declarations_aside(before, &result, |staged| {
            staged.add_comment_with_date_staged(range, author, initials, text, date)
        });
        result
    }

    /// The staged route of [`Self::add_comment_with_date`], its debug oracle.
    fn add_comment_with_date_staged(
        &mut self,
        range: RunRange,
        author: &str,
        initials: Option<&str>,
        text: &str,
        date: Option<&str>,
    ) -> Result<i32> {
        let mut candidate = self.clone_for_staging();
        let id = candidate.add_comment_staged(range, author, initials, text, date)?;
        candidate.flush_dirty_related_story_models()?;
        self.commit_staged_mutation(candidate);
        Ok(id)
    }

    /// Add a dated comment over a checked body, table-cell, header, footer, or note run range.
    ///
    /// A body location can also name a paragraph inside a block content
    /// control with the two-segment path that
    /// [`Document::paragraph_story_location`] returns. Run indexes count the
    /// runs that `Paragraph::runs` lists, as in [`Document::add_comment`].
    /// Each line of `text` becomes one paragraph of the comment, dated as
    /// `date` says.
    pub fn add_story_comment_with_date(
        &mut self,
        range: StoryRunRange,
        author: &str,
        initials: Option<&str>,
        text: &str,
        date: CommentDate<'_>,
    ) -> Result<i32> {
        let date = date.resolve()?;
        let date = date.as_deref();
        let mut candidate = self.clone_for_staging();
        let id = candidate.add_story_comment_staged(range, author, initials, text, date)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        reopened.story_ranges()?;
        self.commit_staged_mutation(reopened);
        Ok(id)
    }

    /// Add a comment over a checked body, table-cell, header, footer, or note run range, as
    /// [`Self::add_story_comment_with_date`] does, dated now.
    pub fn add_story_comment(
        &mut self,
        range: StoryRunRange,
        author: &str,
        initials: Option<&str>,
        text: &str,
    ) -> Result<i32> {
        self.add_story_comment_with_date(range, author, initials, text, CommentDate::Now)
    }

    fn add_story_comment_staged(
        &mut self,
        range: StoryRunRange,
        author: &str,
        initials: Option<&str>,
        text: &str,
        date: Option<&str>,
    ) -> Result<i32> {
        validate_comment_date(date)?;
        if range.start.location.story() != range.end.location.story() {
            return Err(Error::Other(
                "comment story range must stay in one story owner".to_owned(),
            ));
        }
        if range.start.location != range.end.location
            || range.start.location.index_path().len() == 2
            || range.end.location.index_path().len() == 2
            || matches!(
                range.start.location.story().kind(),
                crate::StoryKind::Header
                    | crate::StoryKind::Footer
                    | crate::StoryKind::Footnote
                    | crate::StoryKind::Endnote
                    | crate::StoryKind::Comment
                    | crate::StoryKind::TextBox
            )
        {
            // Reserved in place: the comment parts, styles and their
            // relationships this adds reserve in the same registry.
            let id = self.identifiers.reserve_comment_id()?;
            self.anchor_story_range(&range, RangeAnchor::Comment(id), "comment", false)?;
            self.ensure_comment_models()?;
            let styles = self.prepare_comment_authoring()?;
            // Before the relationship to a comments part not written yet,
            // which the stories this reads would follow.
            self.style_written_comment_reference(range.end.location.story(), id, &styles.1)?;
            self.ensure_comment_relationships()?;
            self.push_comment_definition(id, author, initials, text, date, &styles)?;
            self.comments_dirty = true;
            self.invalidate_layout();
            return Ok(id);
        }
        let mut start = self.story_paragraph_mut(&range.start.location)?.clone();
        let mut end = self.story_paragraph_mut(&range.end.location)?.clone();
        for (label, position, paragraph) in
            [("start", &range.start, &start), ("end", &range.end, &end)]
        {
            let run_count = paragraph.accepted_run_paths().len();
            if position.run_index > run_count {
                return Err(Error::Other(format!(
                    "comment range {label} run index {} exceeds paragraph run count {run_count}",
                    position.run_index,
                )));
            }
        }
        if range.start.location == range.end.location && range.start.run_index > range.end.run_index
        {
            return Err(Error::Other(
                "comment story range start must not follow its end".to_owned(),
            ));
        }

        let id = self.identifiers.reserve_comment_id()?;
        self.ensure_comment_models()?;
        let styles = self.prepare_comment_authoring()?;
        if range.start.location == range.end.location {
            anchor_paragraph_range(
                &mut start,
                Some(range.start.run_index),
                Some(range.end.run_index),
                RangeAnchor::Comment(id),
                "comment",
            )?;
            style_comment_reference(&mut start, id, &styles.1)?;
            *self.story_paragraph_mut(&range.start.location)? = start;
        } else {
            anchor_paragraph_range(
                &mut start,
                Some(range.start.run_index),
                None,
                RangeAnchor::Comment(id),
                "comment",
            )?;
            anchor_paragraph_range(
                &mut end,
                None,
                Some(range.end.run_index),
                RangeAnchor::Comment(id),
                "comment",
            )?;
            style_comment_reference(&mut end, id, &styles.1)?;
            *self.story_paragraph_mut(&range.start.location)? = start;
            *self.story_paragraph_mut(&range.end.location)? = end;
        }

        // After the paragraphs are written back: finding them reads the
        // stories, which would follow the relationship to a comments part
        // not written yet.
        self.ensure_comment_relationships()?;
        self.push_comment_definition(id, author, initials, text, date, &styles)?;
        self.comments_dirty = true;
        self.invalidate_layout();
        Ok(id)
    }

    fn add_comment_staged(
        &mut self,
        range: RunRange,
        author: &str,
        initials: Option<&str>,
        text: &str,
        date: Option<&str>,
    ) -> Result<i32> {
        validate_comment_date(date)?;
        self.validate_run_range(range)?;
        self.ensure_comment_models()?;
        self.ensure_comment_relationships()?;
        let id = self.identifiers.reserve_comment_id()?;
        let styles = self.prepare_comment_authoring()?;
        self.push_comment_definition(id, author, initials, text, date, &styles)?;
        anchor_body_range(
            &mut self.document.body.content,
            range,
            RangeAnchor::Comment(id),
            "comment",
        )?;
        style_comment_reference(
            body_paragraph_mut(&mut self.document.body.content, range.end.body_index)
                .expect("range was validated"),
            id,
            &styles.1,
        )?;
        self.comments_dirty = true;
        self.invalidate_layout();
        Ok(id)
    }

    /// Add a comment on a match of `anchor` in the main story, the
    /// `options.occurrence`-th counted from zero, among the matches in the
    /// paragraphs of table cells only with `options.tables_only`.
    ///
    /// Matches are case-sensitive and non-overlapping, in document order
    /// through body paragraphs, tables (nested ones included) and block
    /// content controls, and one match never spans two paragraphs. They are
    /// found in the literal run text that [`Document::split_run`] offsets
    /// count, so tabs and breaks have no width. The runs at both ends of the
    /// match are split and the comment is anchored on the runs between the
    /// splits as [`Self::add_comment`] anchors a run range, dated as
    /// `options.date` says, with `options.initials`. The anchor is read back
    /// on a copy before anything is published: a missing occurrence or a
    /// match that cannot be anchored exactly is an error and leaves the
    /// document unchanged. A match is not exact when its range would also
    /// show text that the literal text leaves out, such as the result of a
    /// field between two of its runs. Each line of `text` becomes one
    /// paragraph of the comment.
    pub fn add_comment_on_text(
        &mut self,
        anchor: &str,
        author: &str,
        text: &str,
        options: TextCommentOptions<'_>,
    ) -> Result<i32> {
        reject_non_xml_comment(author, options.initials, text)?;
        let date = options.date.resolve()?;
        let mut candidate = self.clone_for_staging();
        let id = candidate.add_comment_on_text_staged(
            anchor,
            &options,
            author,
            text,
            date.as_deref(),
        )?;
        candidate.flush_dirty_related_story_models()?;
        self.commit_staged_mutation(candidate);
        Ok(id)
    }

    fn add_comment_on_text_staged(
        &mut self,
        anchor: &str,
        options: &TextCommentOptions<'_>,
        author: &str,
        text: &str,
        date: Option<&str>,
    ) -> Result<i32> {
        validate_comment_date(date)?;
        if anchor.is_empty() {
            return Err(Error::Other(
                "comment anchor text must not be empty".to_owned(),
            ));
        }
        self.ensure_comment_models()?;
        self.ensure_comment_relationships()?;
        let id = self.identifiers.reserve_comment_id()?;
        let styles = self.prepare_comment_authoring()?;
        self.anchor_existing_comment_on_text(
            id,
            anchor,
            options.occurrence,
            options.tables_only,
            Some(&styles.1),
        )?;
        self.push_comment_definition(id, author, options.initials, text, date, &styles)?;
        self.comments_dirty = true;
        self.invalidate_layout();
        Ok(id)
    }

    // Shared existing literal finder and checked splitter. Moving a comment
    // supplies its identity directly and never allocates a temporary thread.
    // `tables_only` searches only the paragraphs of table cells. A new
    // comment names its `reference_style`; a moved one keeps its own run.
    fn anchor_existing_comment_on_text(
        &mut self,
        id: i32,
        anchor: &str,
        occurrence: usize,
        tables_only: bool,
        reference_style: Option<&str>,
    ) -> Result<()> {
        if anchor.is_empty() {
            return Err(Error::Other("comment anchor text must not be empty".into()));
        }
        let mut remaining = occurrence;
        let mut anchored = None;
        let visit = if tables_only {
            visit_body_table_paragraphs_mut
        } else {
            visit_body_paragraphs_mut
        };
        visit(&mut self.document.body.content, &mut |paragraph| {
            if anchored.is_some() {
                return;
            }
            let literal = paragraph.accepted_literal_text();
            let matches = literal
                .match_indices(anchor)
                .map(|(byte, _)| byte)
                .collect::<Vec<_>>();
            let Some(byte) = matches.get(remaining) else {
                remaining -= matches.len();
                return;
            };
            let start = literal[..*byte].chars().count();
            let end = start + anchor.chars().count();
            anchored = Some(
                paragraph
                    .split_accepted_literal_span(start, end)
                    .map_err(|error| {
                        Error::Other(format!("comment anchor text cannot be split: {error}"))
                    })
                    .and_then(|(start, end)| {
                        anchor_paragraph_range(
                            paragraph,
                            Some(start),
                            Some(end),
                            RangeAnchor::Comment(id),
                            "comment",
                        )
                    })
                    .and_then(|()| match reference_style {
                        Some(style) => style_comment_reference(paragraph, id, style),
                        None => Ok(()),
                    }),
            );
        });
        anchored.ok_or_else(|| {
            // Every paragraph was searched, so `remaining` counts past them all.
            let found = occurrence - remaining;
            let times = if found == 1 { "time" } else { "times" };
            let scope = if tables_only {
                "the table cells of the main story"
            } else {
                "the main story"
            };
            Error::Other(format!(
                "comment anchor text {anchor:?} has no occurrence {occurrence}: it occurs {found} {times} in {scope}"
            ))
        })??;
        // Read actual namespace scopes after the mutable walk releases its borrow.
        // Preserved field results count, while tabs and breaks have zero width.
        let shown = self.comment_literal_range_text(id)?.unwrap_or_default();
        if shown != anchor {
            return Err(Error::Other(format!(
                "comment anchor text {anchor:?} occurrence {occurrence} cannot be anchored exactly: its range would show {shown:?}"
            )));
        }
        self.invalidate_layout();
        Ok(())
    }

    /// Append comment `id`, holding one paragraph per line of `text`, and its
    /// thread entry to the comment models, which must already exist. `styles`
    /// are those [`Self::prepare_comment_authoring`] returns.
    fn push_comment_definition(
        &mut self,
        id: i32,
        author: &str,
        initials: Option<&str>,
        text: &str,
        date: Option<&str>,
        styles: &(String, String),
    ) -> Result<()> {
        let mut occupied =
            occupied_para_ids(self.comments.as_ref(), self.comments_extended.as_ref());
        let (paragraphs, paragraph_ids, para_id) =
            comment_text_paragraphs(text, &mut occupied, styles)?;
        self.comments
            .as_mut()
            .expect("comment model was initialized")
            .comments
            .push(CT_Comment {
                id,
                author: Some(author.to_owned()),
                date: date.map(str::to_owned),
                initials: initials.map(str::to_owned),
                paragraphs,
                paragraph_ids,
                extra_attributes: Vec::new(),
                extra_xml: Vec::new(),
            });
        self.comments_extended
            .as_mut()
            .expect("comments-extended model was initialized")
            .comments
            .push(CT_CommentEx {
                para_id: para_id.clone(),
                para_id_parent: None,
                done: Some(false),
                extra_attributes: Vec::new(),
            });
        self.push_comment_companions(&para_id, author, date)
    }

    /// The part `companion` names from the main part, and its entries.
    fn comment_companion(
        &self,
        companion: &Companion,
    ) -> Result<Option<(String, Vec<CommentPartEntry>)>> {
        let Some(part) =
            self.comment_relationship_part_at(&self.doc_part_name, companion.relationship)?
        else {
            return Ok(None);
        };
        let entries = comment_part_entries(
            self.package.get_part(&part).expect("checked part"),
            companion.namespace,
            companion.root.as_bytes(),
            companion.item.as_bytes(),
        )?;
        Ok(Some((part, entries)))
    }

    /// The durable id and UTC date of every comment, by id, each part read
    /// once; a comment the parts hold nothing for is absent.
    pub fn comment_metadata(&self) -> HashMap<i32, CommentMetadata> {
        let lookup = self.comment_metadata_lookup();
        self.comments
            .iter()
            .flat_map(|comments| &comments.comments)
            .map(|comment| (comment.id, lookup.of(comment)))
            .filter(|(_, metadata)| metadata != &CommentMetadata::default())
            .collect()
    }

    fn comment_metadata_lookup(&self) -> CommentMetadataLookup {
        let map = |companion: &Companion, key: &str, value: &str| {
            let entries = self.comment_companion(companion).ok().flatten();
            entries
                .into_iter()
                .flat_map(|(_, entries)| entries)
                .filter_map(|mut entry| {
                    let key = entry.attributes.remove(key)?.to_ascii_uppercase();
                    Some((key, entry.attributes.remove(value)?))
                })
                .collect()
        };
        CommentMetadataLookup {
            durable_ids: map(&COMMENT_IDS, "paraId", "durableId"),
            dates: map(&COMMENT_EXTENSIBLE, "durableId", "dateUtc"),
        }
    }

    /// Write what Word keeps beside a new comment or reply whose last
    /// paragraph is `para_id` (W10): a durable id in `commentsIds`, and under
    /// it the comment's instant in UTC in `commentsExtensible`, each part
    /// made with its first entry; and its author in `people.xml`, only when
    /// the file has that part and does not list the author yet (Q8).
    fn push_comment_companions(
        &mut self,
        para_id: &str,
        author: &str,
        date: Option<&str>,
    ) -> Result<()> {
        // Parts that do not read, or two of a kind, take no entry: the
        // comment needs none, and Word gives it one on its next save.
        let (Ok(ids), Ok(extensible), Ok(people)) = (
            self.comment_companion(&COMMENT_IDS),
            self.comment_companion(&COMMENT_EXTENSIBLE),
            self.comment_companion(&PEOPLE),
        ) else {
            return Ok(());
        };
        let mut occupied = ids
            .iter()
            .chain(&extensible)
            .flat_map(|(_, entries)| entries)
            .filter_map(|entry| entry.attributes.get("durableId"))
            .filter_map(|value| parse_para_id(value))
            .collect();
        let durable_id = allocate_hex_id(&mut occupied, DURABLE_ID_LIMIT)?;
        self.append_comment_companion_entry(
            &COMMENT_IDS,
            ids.map(|(part, _)| part),
            |p, declaration| {
                format!(
                    r#"<{p}:commentId{declaration} {p}:paraId="{para_id}" {p}:durableId="{durable_id}"/>"#
                )
            },
        )?;
        let date_utc = date.and_then(crate::revision::utc_rfc3339);
        self.append_comment_companion_entry(
            &COMMENT_EXTENSIBLE,
            extensible.map(|(part, _)| part),
            |p, declaration| {
                let date = date_utc
                    .as_ref()
                    .map_or(String::new(), |date| format!(r#" {p}:dateUtc="{date}""#));
                format!(
                    r#"<{p}:commentExtensible{declaration} {p}:durableId="{durable_id}"{date}/>"#
                )
            },
        )?;
        if let Some((part, people)) = people
            && !people
                .iter()
                .any(|person| person.attributes.get("author").map(String::as_str) == Some(author))
        {
            let author = oxoffice_core::xml::escape_attribute_value(author);
            self.append_comment_companion_entry(&PEOPLE, Some(part), |p, declaration| {
                format!(
                    r#"<{p}:person{declaration} {p}:author="{author}"><{p}:presenceInfo {p}:providerId="None" {p}:userId="{author}"/></{p}:person>"#
                )
            })?;
        }
        Ok(())
    }

    /// Append to the root of `part`, a part of `companion` (made when
    /// `None`), the entry `entry` writes with a prefix and the declaration
    /// that prefix needs, empty when the root binds it.
    fn append_comment_companion_entry(
        &mut self,
        companion: &Companion,
        part: Option<String>,
        entry: impl Fn(&str, &str) -> String,
    ) -> Result<()> {
        let (part, xml) = match part {
            Some(part) => {
                let xml = self.package.get_part(&part).expect("checked part").to_vec();
                (part, xml)
            }
            None => {
                let part = self.reserve_document_part_bundle(
                    None,
                    companion.part,
                    companion.relationship,
                    companion.content_type,
                )?;
                let p = companion.prefix;
                let xml = format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<{p}:{} xmlns:mc=\"{}\" xmlns:{p}=\"{}\" mc:Ignorable=\"{p}\"/>",
                    companion.root,
                    oxoffice_docx_oxml::namespace::MC_NS,
                    companion.namespace
                );
                (part, xml.into_bytes())
            }
        };
        let root = oxoffice_core::xml_element::root_element_span(&xml)?;
        let declarations = crate::document::root_namespace_declarations(&xml)?;
        let prefix = declarations.iter().find_map(|(name, uri)| {
            (uri == companion.namespace)
                .then(|| name.strip_prefix("xmlns:"))
                .flatten()
        });
        let addition = match prefix {
            Some(prefix) => entry(prefix, ""),
            None => entry(
                companion.prefix,
                &format!(" xmlns:{}=\"{}\"", companion.prefix, companion.namespace),
            ),
        };
        let mut updated = xml[..root.start_tag.start].to_vec();
        updated.extend(oxoffice_core::xml_element::append_content(
            &xml[root.start_tag.start..],
            &addition,
        )?);
        self.package.set_part(&part, updated);
        Ok(())
    }

    /// Prepare only the part being authored. Imported parts remain unchanged
    /// until an add or reply actually needs these Word conventions.
    ///
    /// Returns the ids of the comment text and comment reference styles: Word's
    /// built-in "annotation text" and "annotation reference", found by name
    /// whatever their localised id (`Commentaire` in French), else by the ids
    /// `CommentText` and `CommentReference`, else created under those ids with
    /// the built-in names and formatting, based on the file's default
    /// paragraph style and default paragraph font (else `Normal` and
    /// `DefaultParagraphFont`, when they are a paragraph and a character
    /// style).
    fn prepare_comment_authoring(&mut self) -> Result<(String, String)> {
        use oxoffice_docx_oxml::styles::StyleType;
        let definitions = [
            ("CommentText", "annotation text", StyleType::Paragraph),
            (
                "CommentReference",
                "annotation reference",
                StyleType::Character,
            ),
        ];
        let mut ids = Vec::with_capacity(definitions.len());
        for (id, built_in, kind) in definitions {
            let by_name = self
                .styles
                .styles
                .iter()
                .find(|style| {
                    style.style_type == kind
                        && style
                            .name
                            .as_deref()
                            .is_some_and(|name| name.eq_ignore_ascii_case(built_in))
                })
                .map(|style| style.style_id.clone());
            if let Some(found) = by_name {
                ids.push(found);
                continue;
            }
            if let Some(existing) = self.styles.get_by_id(id) {
                if existing.style_type != kind {
                    return Err(Error::Other(format!(
                        "comment style {id} has the wrong type"
                    )));
                }
                ids.push(id.to_owned());
                continue;
            }
            // The file's default style of that kind, else Word's usual id
            // when it names a style of that kind.
            let fallback = if kind == StyleType::Paragraph {
                "Normal"
            } else {
                "DefaultParagraphFont"
            };
            let base = self
                .styles
                .get_default(kind)
                .or_else(|| {
                    self.styles
                        .get_by_id(fallback)
                        .filter(|style| style.style_type == kind)
                })
                .map(|style| style.style_id.clone());
            // Word's built-in definitions: comment text is 10 pt on single
            // lines, the comment mark 8 pt, both kept out of the style gallery
            // until used.
            let mut builder = if kind == StyleType::Paragraph {
                crate::StyleBuilder::paragraph(id, built_in)
                    .paragraph_properties(oxoffice_docx_oxml::properties::CT_PPr {
                        line_spacing: Some(crate::Twips(240)),
                        line_rule: Some("auto".to_owned()),
                        ..Default::default()
                    })
                    .size(10.0)
            } else {
                crate::StyleBuilder::character(id, built_in).size(8.0)
            }
            .priority(99)
            .semi_hidden(true)
            .unhide_when_used(true);
            if let Some(base) = base {
                builder = builder.based_on(&base);
            }
            self.add_style(builder)?;
            ids.push(id.to_owned());
        }
        let attributes = &mut self
            .comments
            .as_mut()
            .expect("comment model exists")
            .root_attributes;
        const MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
        let namespace = |prefix: &str| {
            attributes
                .iter()
                .find(|(name, _)| name == &format!("xmlns:{prefix}"))
                .map(|(_, value)| value.as_str())
        };
        let existing = attributes.iter().position(|(name, _)| {
            name.split_once(':').is_some_and(|(prefix, local)| {
                local == "Ignorable" && namespace(prefix) == Some(MC)
            })
        });
        if let Some(index) = existing {
            let tokens = &mut attributes[index].1;
            if !tokens.split_whitespace().any(|token| token == "w14") {
                if !tokens.is_empty() {
                    tokens.push(' ');
                }
                tokens.push_str("w14");
            }
        } else {
            let prefix = attributes
                .iter()
                .find_map(|(name, uri)| {
                    name.strip_prefix("xmlns:")
                        .filter(|_| uri == MC)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| {
                    (0usize..)
                        .map(|index| {
                            if index == 0 {
                                "mc".to_owned()
                            } else {
                                format!("mc{index}")
                            }
                        })
                        .find(|prefix| namespace(prefix).is_none())
                        .expect("finite attribute list")
                });
            if namespace(&prefix).is_none() {
                attributes.push((format!("xmlns:{prefix}"), MC.to_owned()));
            }
            attributes.push((format!("{prefix}:Ignorable"), "w14".to_owned()));
        }
        if !attributes.iter().any(|(name, _)| name == "xmlns:w14") {
            attributes.push((
                "xmlns:w14".to_owned(),
                "http://schemas.microsoft.com/office/word/2010/wordml".to_owned(),
            ));
        }
        let reference = ids.pop().expect("two comment styles");
        let text = ids.pop().expect("two comment styles");
        Ok((text, reference))
    }

    /// Give the reference run that anchoring a story range wrote for comment
    /// `id` in the owner of `written` the comment reference style `style`,
    /// as Word styles a comment mark. Anchoring rewrote that owner, so its
    /// current identity is taken first.
    fn style_written_comment_reference(
        &mut self,
        written: &crate::StoryId,
        id: i32,
        style: &str,
    ) -> Result<()> {
        let story = self
            .stories()?
            .into_iter()
            .find(|story| {
                story.kind() == written.kind()
                    && story.part_name() == written.part_name()
                    && story.owner_index() == written.owner_index()
            })
            .ok_or_else(|| Error::Other("comment story disappeared".into()))?;
        let reference = format!(
            r#"<w:r><w:rPr><w:rStyle w:val="{}"/></w:rPr><w:commentReference w:id="{id}"/></w:r>"#,
            oxoffice_core::xml::escape_attribute_value(style)
        );
        self.splice_comment_reference_run(&story, id, reference.into_bytes())
    }

    /// Add a reply linked to the selected comment's last paragraph, dated
    /// now. Each line of `text` becomes one paragraph of the reply.
    pub fn reply_to(&mut self, parent_id: i32, author: &str, text: &str) -> Result<i32> {
        self.reply_to_with_initials(parent_id, author, None, text, CommentDate::Now)
    }

    /// Add a reply linked to the selected comment's last paragraph, dated as
    /// `date` says. Each line of `text` becomes one paragraph of the reply.
    pub fn reply_to_with_date(
        &mut self,
        parent_id: i32,
        author: &str,
        text: &str,
        date: CommentDate<'_>,
    ) -> Result<i32> {
        self.reply_to_with_initials(parent_id, author, None, text, date)
    }

    /// Add a reply with optional author initials, dated as `date` says.
    ///
    /// The reply has its own crossing range around the selected parent's text
    /// and reference. A parent without a projectable range/reference refuses.
    pub fn reply_to_with_initials(
        &mut self,
        parent_id: i32,
        author: &str,
        initials: Option<&str>,
        text: &str,
        date: CommentDate<'_>,
    ) -> Result<i32> {
        reject_non_xml_comment(author, initials, text)?;
        let date = date.resolve()?;
        let date = date.as_deref();
        let mut candidate = self.clone_for_staging();
        let id = candidate.reply_to_staged(parent_id, author, initials, text, date)?;
        candidate.flush_dirty_related_story_models()?;
        self.commit_staged_mutation(candidate);
        Ok(id)
    }

    fn reply_to_staged(
        &mut self,
        parent_id: i32,
        author: &str,
        initials: Option<&str>,
        text: &str,
        date: Option<&str>,
    ) -> Result<i32> {
        validate_comment_date(date)?;
        let parent_index = self
            .comments
            .as_ref()
            .and_then(|comments| {
                comments
                    .comments
                    .iter()
                    .position(|item| item.id == parent_id)
            })
            .ok_or_else(|| Error::Other(format!("comment id {parent_id} does not exist")))?;
        let (id, story) = self.anchor_reply_crossing(parent_id, None)?;
        let styles = self.prepare_comment_authoring()?;
        self.style_written_comment_reference(&story, id, &styles.1)?;
        let existing_parent_para_id = last_para_id(
            &self
                .comments
                .as_ref()
                .expect("parent lookup proved the model exists")
                .comments[parent_index],
        )
        .map(str::to_owned);
        let mut occupied =
            occupied_para_ids(self.comments.as_ref(), self.comments_extended.as_ref());
        let parent_para_id = match existing_parent_para_id {
            Some(para_id) => para_id,
            None => allocate_para_id_from_occupied(&mut occupied)?,
        };
        let (paragraphs, paragraph_ids, para_id) =
            comment_text_paragraphs(text, &mut occupied, &styles)?;
        self.ensure_comment_models()?;
        self.ensure_comment_relationships()?;

        let comments = self.comments.as_mut().expect("model was initialized");
        let parent = &mut comments.comments[parent_index];
        if parent.paragraphs.is_empty() {
            parent.paragraphs.push(CT_P::new());
        }
        if parent.paragraph_ids.len() < parent.paragraphs.len() {
            parent.paragraph_ids.resize(parent.paragraphs.len(), None);
        }
        parent.paragraph_ids[parent.paragraphs.len() - 1] = Some(parent_para_id.clone());
        comments.comments.push(CT_Comment {
            id,
            author: Some(author.to_owned()),
            date: date.map(str::to_owned),
            initials: initials.map(str::to_owned),
            paragraphs,
            paragraph_ids,
            extra_attributes: Vec::new(),
            extra_xml: Vec::new(),
        });

        let extended = self
            .comments_extended
            .as_mut()
            .expect("model was initialized");
        if extended
            .comments
            .iter()
            .all(|entry| entry.para_id != parent_para_id)
        {
            extended.comments.push(CT_CommentEx {
                para_id: parent_para_id.clone(),
                para_id_parent: None,
                done: Some(false),
                extra_attributes: Vec::new(),
            });
        }
        // Threads are flat: a reply to a reply names the thread's root, as
        // Word rewrites it (W11), and takes the thread's state (W10).
        let root_para_id = thread_root_para_id(self.comments.as_ref(), extended, &parent_para_id);
        let done = extended
            .comments
            .iter()
            .find(|entry| entry.para_id == root_para_id)
            .map_or(Some(false), |root| Some(root.done.unwrap_or(false)));
        extended.comments.push(CT_CommentEx {
            para_id: para_id.clone(),
            para_id_parent: Some(root_para_id),
            done,
            extra_attributes: Vec::new(),
        });
        self.push_comment_companions(&para_id, author, date)?;
        self.comments_dirty = true;
        self.invalidate_layout();
        Ok(id)
    }

    /// Reuse checked story endpoints, extending the reply through the parent's
    /// reference run. Physical start adjacency is then enforced in that same
    /// owner without moving or rewriting any intervening producer payload.
    /// A new reply reserves its id once the parent proves projectable, while
    /// a reply moving with its thread keeps `existing`. Returns the reply id
    /// and the story that now holds its markers.
    fn anchor_reply_crossing(
        &mut self,
        parent_id: i32,
        existing: Option<i32>,
    ) -> Result<(i32, crate::StoryId)> {
        let no_range = || Error::Other(format!("comment id {parent_id} has no range to reply to"));
        self.flush_to_package()?;
        let read = CommentAnchorRead::from_prepared(self, Some(parent_id))?;
        let parent = read.project(parent_id)?.0.ok_or_else(no_range)?;
        let (story, end) = read.reply_boundary(parent_id, &parent)?;
        drop(read);
        let id = match existing {
            Some(id) => id,
            None => self.identifiers.reserve_comment_id()?,
        };
        self.anchor_story_range(
            &StoryRunRange {
                start: parent.start,
                end,
            },
            RangeAnchor::Comment(id),
            "comment reply",
            false,
        )?;
        let (mut xml, owner) = self
            .refreshed_story_owner_span(&story)?
            .ok_or_else(no_range)?;
        let markers = comment_source_markers(&xml)?;
        let selected = |selected_id| {
            markers.iter().find(|marker| {
                marker.id == selected_id && marker.family == 0 && owner.contains(&marker.span.start)
            })
        };
        let parent_start = selected(parent_id).ok_or_else(no_range)?.span.clone();
        let reply_start = selected(id).ok_or_else(no_range)?.span.clone();
        if parent_start.end != reply_start.start {
            let marker = xml[reply_start.clone()].to_vec();
            let destination = parent_start.end
                - if reply_start.end <= parent_start.start {
                    reply_start.len()
                } else {
                    0
                };
            xml.drain(reply_start);
            xml.splice(destination..destination, marker);
            self.write_story_part(story.part_name(), xml)?;
        }
        Ok((id, story))
    }

    /// Set or clear the resolved state for a comment.
    pub fn resolve_comment(&mut self, id: i32, resolved: bool) -> Result<bool> {
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let result = self.edit_in_place(&[crate::EditUnit::Comments], |document| {
            let updated = document.resolve_comment_staged(id, resolved)?;
            if updated {
                document.flush_dirty_related_story_models()?;
            }
            Ok(updated)
        });
        #[cfg(debug_assertions)]
        self.assert_in_place_matches_staged(before, &result, |staged| {
            staged.resolve_comment_on_a_copy(id, resolved)
        });
        result
    }

    /// The staged route of [`Self::resolve_comment`], the debug oracle of
    /// its in-place edit.
    #[cfg(debug_assertions)]
    fn resolve_comment_on_a_copy(&mut self, id: i32, resolved: bool) -> Result<bool> {
        let mut candidate = self.clone_for_staging();
        let updated = candidate.resolve_comment_staged(id, resolved)?;
        if updated {
            candidate.flush_dirty_related_story_models()?;
            self.commit_staged_mutation(candidate);
        }
        Ok(updated)
    }

    fn resolve_comment_staged(&mut self, id: i32, resolved: bool) -> Result<bool> {
        let Some(comment_index) = self
            .comments
            .as_ref()
            .and_then(|comments| comments.comments.iter().position(|item| item.id == id))
        else {
            return Ok(false);
        };
        let existing_para_id = last_para_id(
            &self
                .comments
                .as_ref()
                .expect("comment lookup proved the model exists")
                .comments[comment_index],
        )
        .map(str::to_owned);
        let para_id = match existing_para_id {
            Some(para_id) => para_id,
            None => allocate_para_id(self.comments.as_ref(), self.comments_extended.as_ref())?,
        };
        self.ensure_comment_models()?;
        self.ensure_comment_relationships()?;
        let comment = &mut self
            .comments
            .as_mut()
            .expect("model was initialized")
            .comments[comment_index];
        if comment.paragraphs.is_empty() {
            comment.paragraphs.push(CT_P::new());
        }
        if comment.paragraph_ids.len() < comment.paragraphs.len() {
            comment.paragraph_ids.resize(comment.paragraphs.len(), None);
        }
        let last = comment.paragraphs.len() - 1;
        comment.paragraph_ids[last] = Some(para_id.clone());
        let extended = self
            .comments_extended
            .as_mut()
            .expect("model was initialized");
        let root_para_id = thread_root_para_id(self.comments.as_ref(), extended, &para_id);
        if extended
            .comments
            .iter()
            .all(|entry| entry.para_id != root_para_id)
        {
            extended.comments.push(CT_CommentEx {
                para_id: root_para_id.clone(),
                para_id_parent: None,
                done: None,
                extra_attributes: Vec::new(),
            });
        }
        // The whole thread, as Word saves a resolved thread (W10).
        let thread = extended
            .comments
            .iter()
            .map(|entry| {
                thread_root_para_id(self.comments.as_ref(), extended, &entry.para_id)
                    == root_para_id
            })
            .collect::<Vec<_>>();
        for (entry, in_thread) in extended.comments.iter_mut().zip(thread) {
            if in_thread {
                entry.done = Some(resolved);
            }
        }
        self.comments_dirty = true;
        self.invalidate_layout();
        Ok(true)
    }

    /// Remove a qualified comment thread and its source anchors atomically.
    /// Unsupported or ambiguous companion ownership leaves the document unchanged.
    pub fn remove_comment(&mut self, id: i32) -> Result<bool> {
        if !self.comments().iter().any(|comment| comment.id() == id) {
            return Ok(false);
        }
        match self.remove_comment_in_place(id) {
            Ok(Some(_)) => Ok(true),
            Ok(None) => self.remove_comment_staged(id),
            Err(error) => Err(error),
        }
    }

    /// Remove the thread of the comment `id` in place when the in-place
    /// route takes it, and answer the stories of its comments; `None`, with
    /// nothing changed, when it leaves the thread to the staged route. The
    /// binding publishes the body blocks of the edit and those stories.
    #[doc(hidden)]
    pub fn remove_comment_in_place(&mut self, id: i32) -> Result<Option<Vec<crate::StoryId>>> {
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let result = self.remove_comment_in_place_unchecked(id);
        #[cfg(debug_assertions)]
        if !matches!(result, Ok(None)) {
            let answer = match &result {
                Ok(_) => Ok(true),
                Err(error) => Err(Error::Other(error.to_string())),
            };
            self.assert_in_place_matches_staged_declarations_aside(before, &answer, |staged| {
                staged.remove_comment_staged(id)
            });
        }
        result
    }

    /// [`Self::remove_comment`] on the live document, at the cost of the
    /// comment parts and the body blocks that hold the thread's marks (the
    /// written fragment walk finds them, one unit each): the thread and its ownership are read with the main part
    /// replaced by those blocks written alone, which lose the thread's marks
    /// as the staged route's byte removal removes them, and are read back.
    /// `None`, with nothing changed, for what the staged route keeps: stale
    /// story parts, a thread with no mark in the body, a removal that leaves
    /// no comment (it may remove the owned comment parts), a block that does
    /// not read back, a thread the ownership reads otherwise.
    fn remove_comment_in_place_unchecked(
        &mut self,
        id: i32,
    ) -> Result<Option<Vec<crate::StoryId>>> {
        // The staged route reads the comment parts it flushes first.
        let unwritten = |part: &Option<String>| {
            part.as_ref()
                .is_some_and(|part| self.package.get_part(part).is_none())
        };
        if self.comments_dirty
            || self.footnotes_dirty
            || unwritten(&self.comments_part_name)
            || unwritten(&self.comments_extended_part_name)
        {
            return Ok(None);
        }
        let comments = self.comments();
        let mut ids = HashSet::from([id]);
        loop {
            let before = ids.len();
            for comment in &comments {
                if comment
                    .parent_id()
                    .is_some_and(|parent| ids.contains(&parent))
                {
                    ids.insert(comment.id());
                }
            }
            if ids.len() == before {
                break;
            }
        }
        if comments.iter().all(|comment| ids.contains(&comment.id())) {
            return Ok(None);
        }
        // A removal that empties a metadata part this session made: the
        // staged route removes the part.
        let removed = comments
            .iter()
            .filter(|comment| ids.contains(&comment.id()))
            .filter_map(|comment| last_para_id(comment.inner))
            .map(str::to_ascii_uppercase)
            .collect::<HashSet<_>>();
        drop(comments);
        let mut removed_durable = HashSet::new();
        for (companion, key) in [(&COMMENT_IDS, "paraId"), (&COMMENT_EXTENSIBLE, "durableId")] {
            let Ok(Some((part, entries))) = self.comment_companion(companion) else {
                continue;
            };
            let gone = |value: &String| match key {
                "paraId" => removed.contains(&value.to_ascii_uppercase()),
                _ => removed_durable.contains(&value.to_ascii_uppercase()),
            };
            if !entries.is_empty()
                && !self.identifiers.part_is_preserved(&part)
                && entries
                    .iter()
                    .all(|entry| entry.attributes.get(key).is_some_and(gone))
            {
                return Ok(None);
            }
            let durable = entries
                .iter()
                .filter(|entry| entry.attributes.get("paraId").is_some_and(gone))
                .filter_map(|entry| entry.attributes.get("durableId"))
                .map(|value| value.to_ascii_uppercase())
                .collect::<Vec<_>>();
            removed_durable.extend(durable);
        }
        let hits = self.body_slots_referencing_comments(&ids);
        if hits.is_empty() {
            return Ok(None);
        }
        // The main part as the ownership reads it: the hit blocks, each
        // written alone, under the part's head and tail.
        let mut main = Vec::new();
        let mut tail = Vec::new();
        let mut blocks = Vec::with_capacity(hits.len());
        for hit in &hits {
            let Some(alone) = self.body_block_part(hit.slot)? else {
                return Ok(None);
            };
            if main.is_empty() {
                main.extend_from_slice(&alone.head);
                tail = alone.tail_bytes().to_vec();
            }
            main.extend_from_slice(alone.block_of(&alone.xml));
            let Some(block) =
                self.body_block_after_byte_edit(hit.slot, |xml| without_comment_marks(xml, &ids))?
            else {
                return Ok(None);
            };
            blocks.push((hit.slot, block));
        }
        main.extend_from_slice(&tail);
        let ownership = self.comment_ownership_with(&self.doc_part_name, Some(&main))?;
        if ownership.descendants(id) != ids {
            return Ok(None);
        }
        // The stories of the thread: the comment owners of the part, in the
        // order of the comments model.
        let Some(part_name) = self.comments_part_name.clone() else {
            return Ok(None);
        };
        let Some(part) = self.package.get_part(&part_name) else {
            return Ok(None);
        };
        let owners = crate::document::scan_story_owners(part, crate::StoryKind::Comment)?
            .into_iter()
            .filter(|owner| owner.kind == crate::StoryKind::Comment)
            .collect::<Vec<_>>();
        let model = &self.comments.as_ref().expect("a comment exists").comments;
        if owners.len() != model.len() {
            return Ok(None);
        }
        let stories = model
            .iter()
            .zip(owners)
            .filter(|(comment, _)| ids.contains(&comment.id))
            .map(|(_, owner)| {
                crate::StoryId::of_owner(crate::StoryKind::Comment, &part_name, &owner)
            })
            .collect::<Vec<_>>();
        // One unit per hit block: a span from the first to the last could
        // cover the whole body.
        let units = hits
            .iter()
            .map(|hit| crate::EditUnit::BodyBlocks {
                slot: hit.slot,
                count: 1,
            })
            .chain([crate::EditUnit::Comments, crate::EditUnit::Parts])
            .collect::<Vec<_>>();
        self.edit_in_place_with_body_blocks(&units, blocks, |document| {
            // The thread entries as the model holds them, which the staged
            // route flushes first: a reply's entry may not be written yet.
            if let (Some(model), Some(part)) = (
                &document.comments_extended,
                document.comments_extended_part_name.as_deref(),
            ) && document
                .package
                .get_part(part)
                .is_some_and(|xml| CT_CommentsEx::from_xml(xml).ok().as_ref() != Some(model))
            {
                let xml = model.to_xml()?;
                document.package.set_part(part, xml);
            }
            let owner = document.doc_part_name.clone();
            document.remove_comment_ids_with(&owner, &ids, Some(&main))
        })?;
        Ok(Some(stories))
    }

    /// The staged route of [`Self::remove_comment`]: the whole main part is
    /// serialized and its marks inventoried, the debug oracle of the
    /// in-place route.
    fn remove_comment_staged(&mut self, id: i32) -> Result<bool> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let ownership = candidate.comment_ownership_at(&candidate.doc_part_name)?;
        let removed = ownership.descendants(id);
        candidate.remove_comment_ids_staged_at(&candidate.doc_part_name.clone(), &removed)?;
        // An inverse add/remove can recover the complete retained producer source.
        // Compare every modeled value and raw child, ignoring only declarations
        // introduced by the intermediate canonical serialization.
        if let Some(retained) = self.package.get_part(&self.doc_part_name)
            && let Ok(original) = oxoffice_docx_oxml::document::CT_Document::from_xml(retained)
        {
            let declarations_preserved = original
                .extra_namespaces
                .iter()
                .all(|binding| candidate.document.extra_namespaces.contains(binding));
            let only_added_canonical_wp = candidate.document.extra_namespaces.iter().all(|binding|
                original.extra_namespaces.contains(binding)
                    || (binding.0 == "xmlns:wp"
                        && binding.1 == "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
                        && !original.extra_namespaces.iter().any(|original| original.0 == binding.0)));
            let mut restored = candidate.document.clone();
            restored.extra_namespaces = original.extra_namespaces.clone();
            // Full canonical XML equality includes all modeled values and opaque
            // bytes while ignoring unused inherited-prefix projection caches.
            if declarations_preserved
                && only_added_canonical_wp
                && restored.to_xml()? == original.to_xml()?
            {
                let part = candidate.doc_part_name.clone();
                crate::document::set_story_source_xml(&mut candidate, &part, retained.to_vec())?;
            }
        }
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(true)
    }

    fn validate_run_range(&self, range: RunRange) -> Result<()> {
        if range.start > range.end {
            return Err(Error::Other(
                "comment range start must not follow its end".to_owned(),
            ));
        }
        for (label, position) in [("start", range.start), ("end", range.end)] {
            let paragraph = body_paragraph(&self.document.body.content, position.body_index)
                .ok_or_else(|| {
                    Error::Other(format!(
                        "comment range {label} body index {} is not a paragraph",
                        position.body_index
                    ))
                })?;
            let run_count = paragraph.accepted_run_paths().len();
            if position.run_index > run_count {
                return Err(Error::Other(format!(
                    "comment range {label} run index {} exceeds paragraph run count {run_count}",
                    position.run_index,
                )));
            }
        }
        Ok(())
    }

    fn ensure_comment_models(&mut self) -> Result<()> {
        self.identifiers.observe_package_graph(&self.package)?;
        if self.comments.is_none() {
            self.comments = Some(CT_Comments::new());
        }
        if self.comments_part_name.is_none() {
            self.comments_part_name = Some(
                self.identifiers
                    .reserve_preferred_part_name(DEFAULT_COMMENTS_PART)?,
            );
            self.comments_owned = true;
        }
        if self.comments_extended.is_none() {
            self.comments_extended = Some(CT_CommentsEx::new());
        }
        if self.comments_extended_part_name.is_none() {
            self.comments_extended_part_name = Some(
                self.identifiers
                    .reserve_preferred_part_name(DEFAULT_COMMENTS_EXTENDED_PART)?,
            );
            self.comments_extended_owned = true;
        }
        Ok(())
    }

    fn ensure_comment_relationships(&mut self) -> Result<()> {
        let comments_part = self
            .comments_part_name
            .clone()
            .ok_or_else(|| Error::Other("comments part name is missing".to_owned()))?;
        let comments_extended_part = self
            .comments_extended_part_name
            .clone()
            .ok_or_else(|| Error::Other("comments-extended part name is missing".to_owned()))?;
        self.ensure_part_relationship_checked(
            &comments_part,
            oxoffice_opc::relationship::rel_types::COMMENTS,
            COMMENTS_CONTENT_TYPE,
        )
        .map_err(|error| {
            Error::Other(format!("comments relationship allocation failed: {error}"))
        })?;
        self.ensure_part_relationship_checked(
            &comments_extended_part,
            COMMENTS_EXTENDED_REL_TYPE,
            COMMENTS_EXTENDED_CONTENT_TYPE,
        )
        .map_err(|error| {
            Error::Other(format!(
                "comments-extended relationship allocation failed: {error}"
            ))
        })?;
        Ok(())
    }

    fn remove_owned_empty_comment_parts(&mut self) {
        if self.comments.as_ref().is_some_and(|comments| {
            comments.comments.is_empty()
                && comments.extra_xml.is_empty()
                && only_authored_comment_root_attributes(&comments.root_attributes)
        }) && self.comments_owned
        {
            if let Some(part) = self.comments_part_name.take() {
                remove_owned_part(self, &part, oxoffice_opc::relationship::rel_types::COMMENTS);
            }
            self.comments = None;
            self.comments_owned = false;
        }
        // The durable ids and UTC dates parts this session made, left empty.
        for companion in [&COMMENT_IDS, &COMMENT_EXTENSIBLE] {
            if let Ok(Some((part, entries))) = self.comment_companion(companion)
                && entries.is_empty()
                && !self.identifiers.part_is_preserved(&part)
            {
                remove_owned_part(self, &part, companion.relationship);
            }
        }
        if self.comments_extended.as_ref().is_some_and(|extended| {
            extended.comments.is_empty()
                && extended.extra_xml.is_empty()
                && extended
                    .root_attributes
                    .iter()
                    .all(|(key, _)| key == "xmlns" || key.starts_with("xmlns:"))
        }) && self.comments_extended_owned
        {
            if let Some(part) = self.comments_extended_part_name.take() {
                remove_owned_part(self, &part, COMMENTS_EXTENDED_REL_TYPE);
            }
            self.comments_extended = None;
            self.comments_extended_owned = false;
        }
    }
}

fn only_authored_comment_root_attributes(attributes: &[(String, String)]) -> bool {
    let binding = |prefix: &str| {
        attributes
            .iter()
            .find(|(name, _)| name == &format!("xmlns:{prefix}"))
            .map(|(_, uri)| uri.as_str())
    };
    let mut ignorable = false;
    attributes.iter().all(|(name, value)| {
        if name == "xmlns" || name.starts_with("xmlns:") {
            return true;
        }
        let Some((prefix, local)) = name.split_once(':') else {
            return false;
        };
        if local != "Ignorable"
            || binding(prefix)
                != Some("http://schemas.openxmlformats.org/markup-compatibility/2006")
            || binding("w14") != Some("http://schemas.microsoft.com/office/word/2010/wordml")
            || !value.split_whitespace().eq(["w14"])
            || ignorable
        {
            return false;
        }
        ignorable = true;
        true
    })
}

fn validate_comment_date(date: Option<&str>) -> Result<()> {
    if let Some(value) = date
        && crate::revision::parse_rfc3339(value).is_none()
    {
        return Err(Error::Other(format!(
            "invalid RFC 3339 comment timestamp: {value}"
        )));
    }
    Ok(())
}

fn selected_fragment_comment_ids(
    source: &CT_Comments,
    source_extended: Option<&CT_CommentsEx>,
    ids: &[String],
) -> Result<HashSet<i32>> {
    let mut included_ids = ids
        .iter()
        .map(|id| {
            id.parse::<i32>()
                .map_err(|_| Error::Other(format!("document fragment comment id {id} is invalid")))
        })
        .collect::<Result<HashSet<_>>>()?;
    for id in &included_ids {
        if !source.comments.iter().any(|comment| comment.id == *id) {
            return Err(Error::Other(format!(
                "document fragment comment id {id} has no definition"
            )));
        }
    }
    if let Some(extended) = source_extended {
        loop {
            let included_para_ids = source
                .comments
                .iter()
                .filter(|comment| included_ids.contains(&comment.id))
                .flat_map(para_ids)
                .collect::<HashSet<_>>();
            let before = included_ids.len();
            for extension in &extended.comments {
                if extension
                    .para_id_parent
                    .as_deref()
                    .is_some_and(|parent| included_para_ids.contains(parent))
                    && let Some(comment) = source
                        .comments
                        .iter()
                        .find(|comment| para_ids(comment).any(|para| para == extension.para_id))
                {
                    included_ids.insert(comment.id);
                }
            }
            if included_ids.len() == before {
                break;
            }
        }
    }
    Ok(included_ids)
}

/// The `w14:paraId` of the comment's last paragraph, which keys its
/// `w15:commentEx` entry and names it as a reply's `w15:paraIdParent`.
fn last_para_id(comment: &CT_Comment) -> Option<&str> {
    let last = comment.paragraphs.len().checked_sub(1)?;
    comment.paragraph_ids.get(last)?.as_deref()
}

/// Every `w14:paraId` of the comment. Identifiers are unique, so a parent
/// link naming any of them, as older producers wrote, still finds it.
fn para_ids(comment: &CT_Comment) -> impl Iterator<Item = &str> {
    comment.paragraph_ids.iter().filter_map(Option::as_deref)
}

/// Prove the whole selected reference run adds no unrelated payload. The
/// existing writer supplies retained namespace context for raw empty Word Text,
/// which the preservation parser keeps as raw XML rather than typed content.
fn reply_reference_has_only_metadata(
    run: &CT_R,
    parent_id: i32,
) -> oxoffice_docx_oxml::Result<bool> {
    let mut remainder = run.clone();
    remainder.remove_comment_references(&[parent_id]);
    if !remainder.content.iter().all(|content| match content {
        RunContent::Text(text) => text.text.is_empty(),
        _ => false,
    }) || !remainder.alt_drawings.is_empty()
    {
        return Ok(false);
    }
    if remainder.extra_xml.iter().enumerate().all(|(index, raw)| {
        remainder
            .extra_xml_positions
            .get(index)
            .is_some_and(|encoded| CT_R::raw_child_is_root_attributes(*encoded))
            && oxoffice_docx_oxml::text::is_root_attribute_record(raw)
    }) {
        return Ok(true);
    }
    let mut writer = quick_xml::Writer::new(Vec::new());
    let mut wrapper = BytesStart::new("rdocxReplyMetadata");
    wrapper.push_attribute(("xmlns:w", oxoffice_docx_oxml::namespace::W_NS));
    writer.write_event(Event::Start(wrapper))?;
    remainder.to_xml(&mut writer)?;
    writer.write_event(Event::End(BytesEnd::new("rdocxReplyMetadata")))?;
    let xml = writer.into_inner();
    let mut reader = NsReader::from_reader(xml.as_slice());
    let mut buffer = Vec::new();
    let mut depth = 0;
    loop {
        let event = reader.read_event_into(&mut buffer)?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                let word = matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == oxoffice_docx_oxml::namespace::W_NS.as_bytes());
                if depth == 1 && (!word || local.as_ref() != b"r") {
                    return Ok(false);
                }
                if depth == 2 {
                    if !word || !matches!(local.as_ref(), b"rPr" | b"t") {
                        return Ok(false);
                    }
                    if !empty && local.as_ref() == b"rPr" {
                        reader.read_to_end_into(element.name(), &mut Vec::new())?;
                    } else if !empty {
                        // Empty typed Text serializes as paired tags. Raw opaque
                        // children inside Text are never treated as metadata.
                        loop {
                            match reader.read_event_into(&mut Vec::new())? {
                                Event::End(end) if end.name() == element.name() => break,
                                Event::Text(text) if text.is_empty() => {}
                                Event::CData(text) if text.is_empty() => {}
                                _ => return Ok(false),
                            }
                        }
                    }
                } else if !empty {
                    depth += 1;
                }
            }
            Event::End(_) => depth -= 1,
            Event::Text(text) if text.iter().all(u8::is_ascii_whitespace) => {}
            Event::Eof => return Ok(depth == 0),
            _ => return Ok(false),
        }
        buffer.clear();
    }
}

/// Build one comment paragraph per line of `text`, each with a fresh
/// `w14:paraId`, and return the last one's identifier with them. `styles` are
/// the ids of the comment text and comment reference styles.
fn comment_text_paragraphs(
    text: &str,
    occupied: &mut HashSet<u32>,
    styles: &(String, String),
) -> Result<(Vec<CT_P>, Vec<Option<String>>, String)> {
    let mut paragraphs = Vec::new();
    let mut paragraph_ids = Vec::new();
    for (index, line) in text.split('\n').enumerate() {
        let mut paragraph = CT_P::new();
        paragraph
            .properties
            .get_or_insert_with(Default::default)
            .style_id = Some(styles.0.clone());
        if index == 0 {
            let mut reference = oxoffice_docx_oxml::text::CT_R::new("");
            reference.content.clear();
            reference
                .properties
                .get_or_insert_with(Default::default)
                .style_id = Some(styles.1.clone());
            reference.extra_xml.push(b"<w:annotationRef/>".to_vec());
            paragraph.runs.push(reference);
        }
        paragraph.add_run(line.strip_suffix('\r').unwrap_or(line));
        paragraphs.push(paragraph);
        paragraph_ids.push(Some(allocate_para_id_from_occupied(occupied)?));
    }
    let para_id = paragraph_ids
        .last()
        .cloned()
        .flatten()
        .expect("splitting text yields at least one line");
    Ok((paragraphs, paragraph_ids, para_id))
}

pub(crate) fn validate_bookmark_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::Other("bookmark name must not be empty".to_owned()));
    }
    if name.starts_with('_') {
        return Err(Error::Other(format!(
            "bookmark name {name} is reserved for producer use"
        )));
    }
    if name.len() > 40
        || !name
            .chars()
            .next()
            .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        || name
            .chars()
            .any(|character| !(character == '_' || character.is_ascii_alphanumeric()))
    {
        return Err(Error::Other(format!(
            "bookmark name {name} is not a valid Word bookmark name"
        )));
    }
    Ok(())
}

fn validate_bookmark_range(content: &[BodyContent], range: RunRange) -> Result<()> {
    if range.start > range.end {
        return Err(Error::Other(
            "bookmark range start must not follow its end".to_owned(),
        ));
    }
    for (label, position) in [("start", range.start), ("end", range.end)] {
        let paragraph = body_paragraph(content, position.body_index).ok_or_else(|| {
            Error::Other(format!(
                "bookmark range {label} body index {} is not a paragraph",
                position.body_index
            ))
        })?;
        let run_count = paragraph.accepted_run_paths().len();
        if position.run_index > run_count {
            return Err(Error::Other(format!(
                "bookmark range {label} run index {} exceeds paragraph run count {run_count}",
                position.run_index,
            )));
        }
    }
    Ok(())
}

fn bookmark_range_text(
    content: &[BodyContent],
    start_body_index: usize,
    start_run_index: usize,
    end_body_index: usize,
    end_run_index: usize,
) -> String {
    let mut paragraphs = Vec::<String>::new();
    for body_index in start_body_index..=end_body_index {
        let Some(paragraph) = story_paragraph(content, body_index) else {
            continue;
        };
        let runs = paragraph.accepted_bookmark_runs();
        let start = if body_index == start_body_index {
            start_run_index
        } else {
            0
        };
        let end = if body_index == end_body_index {
            end_run_index
        } else {
            runs.len()
        };
        let start = start.min(runs.len());
        let end = end.min(runs.len());
        paragraphs.push(
            runs[start..end]
                .iter()
                .map(|run| run.accepted_text())
                .collect(),
        );
    }
    paragraphs.join("\n")
}

fn story_paragraph(content: &[BodyContent], index: usize) -> Option<&CT_P> {
    let mut remaining = index;
    for item in content {
        if let Some(paragraph) = paragraph_in_body_content(item, &mut remaining) {
            return Some(paragraph);
        }
    }
    None
}

fn body_paragraph_mut(content: &mut [BodyContent], index: usize) -> Option<&mut CT_P> {
    match content.get_mut(index)? {
        BodyContent::Paragraph(paragraph) => Some(paragraph),
        BodyContent::Table(_) | BodyContent::ContentControl(_) | BodyContent::RawXml(_) => None,
    }
}

fn body_paragraph(content: &[BodyContent], index: usize) -> Option<&CT_P> {
    match content.get(index)? {
        BodyContent::Paragraph(paragraph) => Some(paragraph),
        BodyContent::Table(_) | BodyContent::ContentControl(_) | BodyContent::RawXml(_) => None,
    }
}

/// Write range markers over a validated body range. The paragraphs change
/// only when every marker can be placed exactly.
fn anchor_body_range(
    content: &mut [BodyContent],
    range: RunRange,
    anchor: RangeAnchor<'_>,
    label: &str,
) -> Result<()> {
    let (start, end) = (range.start, range.end);
    let paragraph = |content: &[BodyContent], index| {
        body_paragraph(content, index)
            .cloned()
            .expect("range was validated")
    };
    let mut first = paragraph(content, start.body_index);
    if start.body_index == end.body_index {
        anchor_paragraph_range(
            &mut first,
            Some(start.run_index),
            Some(end.run_index),
            anchor,
            label,
        )?;
    } else {
        let mut last = paragraph(content, end.body_index);
        anchor_paragraph_range(&mut first, Some(start.run_index), None, anchor, label)?;
        anchor_paragraph_range(&mut last, None, Some(end.run_index), anchor, label)?;
        *body_paragraph_mut(content, end.body_index).expect("range was validated") = last;
    }
    *body_paragraph_mut(content, start.body_index).expect("range was validated") = first;
    Ok(())
}

/// Write range markers at accepted-view run boundaries, the run index space
/// that `Paragraph::runs` lists. A missing side continues in another
/// paragraph.
fn anchor_paragraph_range(
    paragraph: &mut CT_P,
    start: Option<usize>,
    end: Option<usize>,
    anchor: RangeAnchor<'_>,
    label: &str,
) -> Result<()> {
    paragraph
        .anchor_accepted_range(start, end, anchor)
        .map_err(|error| Error::Other(format!("{label} range cannot be anchored: {error}")))
}

/// Give the reference run that anchoring wrote for comment `id` in
/// `paragraph` the comment reference style `style`, as Word styles a
/// comment mark.
fn style_comment_reference(paragraph: &mut CT_P, id: i32, style: &str) -> Result<()> {
    if paragraph.style_comment_reference_run(id, style) {
        Ok(())
    } else {
        Err(Error::Other(format!(
            "comment {id} has no reference run to style"
        )))
    }
}

fn collect_main_story_paragraphs<'a>(content: &'a [BodyContent], output: &mut Vec<&'a CT_P>) {
    for item in content {
        match item {
            BodyContent::Paragraph(paragraph) => output.push(paragraph),
            BodyContent::Table(table) => collect_table_paragraphs(table, output),
            BodyContent::ContentControl(control) => {
                collect_control_paragraphs(control, BlockControlOwner::Body, output)
            }
            BodyContent::RawXml(_) => {}
        }
    }
}

fn collect_table_paragraphs<'a>(table: &'a CT_Tbl, output: &mut Vec<&'a CT_P>) {
    for boundary in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == boundary)
        {
            collect_control_paragraphs(control, BlockControlOwner::Table, output);
        }
        if let Some(row) = table.rows.get(boundary) {
            collect_row_paragraphs(row, output);
        }
    }
}

fn collect_row_paragraphs<'a>(row: &'a CT_Row, output: &mut Vec<&'a CT_P>) {
    for boundary in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == boundary)
        {
            collect_control_paragraphs(control, BlockControlOwner::Row, output);
        }
        if let Some(cell) = row.cells.get(boundary) {
            collect_cell_paragraphs(cell, output);
        }
    }
}

fn collect_cell_paragraphs<'a>(cell: &'a CT_Tc, output: &mut Vec<&'a CT_P>) {
    for item in &cell.content {
        match item {
            CellContent::Paragraph(paragraph) => output.push(paragraph),
            CellContent::Table(table) => collect_table_paragraphs(table, output),
            CellContent::ContentControl(control) => {
                collect_control_paragraphs(control, BlockControlOwner::Cell, output)
            }
        }
    }
}

#[derive(Clone, Copy)]
enum BlockControlOwner {
    Body,
    Table,
    Row,
    Cell,
}

fn collect_control_paragraphs<'a>(
    control: &'a CT_Sdt,
    owner: BlockControlOwner,
    output: &mut Vec<&'a CT_P>,
) {
    for item in &control.content {
        match (owner, item) {
            (
                BlockControlOwner::Body | BlockControlOwner::Cell,
                SdtContent::Paragraph(paragraph),
            ) => output.push(paragraph),
            (BlockControlOwner::Body | BlockControlOwner::Cell, SdtContent::Table(table)) => {
                collect_table_paragraphs(table, output)
            }
            (BlockControlOwner::Table, SdtContent::Row(row)) => collect_row_paragraphs(row, output),
            (BlockControlOwner::Row, SdtContent::Cell(cell)) => {
                collect_cell_paragraphs(cell, output)
            }
            (_, SdtContent::ContentControl(control)) => {
                collect_control_paragraphs(control, owner, output)
            }
            _ => {}
        }
    }
}

fn paragraph_in_body_content<'a>(
    content: &'a BodyContent,
    remaining: &mut usize,
) -> Option<&'a CT_P> {
    match content {
        BodyContent::Paragraph(paragraph) => take_paragraph(paragraph, remaining),
        BodyContent::Table(table) => paragraph_in_table(table, remaining),
        BodyContent::ContentControl(control) => {
            paragraph_in_control(control, BlockControlOwner::Body, remaining)
        }
        BodyContent::RawXml(_) => None,
    }
}

fn paragraph_in_table<'a>(table: &'a CT_Tbl, remaining: &mut usize) -> Option<&'a CT_P> {
    for boundary in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == boundary)
        {
            if let Some(paragraph) =
                paragraph_in_control(control, BlockControlOwner::Table, remaining)
            {
                return Some(paragraph);
            }
        }
        if let Some(row) = table.rows.get(boundary)
            && let Some(paragraph) = paragraph_in_row(row, remaining)
        {
            return Some(paragraph);
        }
    }
    None
}

fn paragraph_in_row<'a>(row: &'a CT_Row, remaining: &mut usize) -> Option<&'a CT_P> {
    for boundary in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == boundary)
        {
            if let Some(paragraph) =
                paragraph_in_control(control, BlockControlOwner::Row, remaining)
            {
                return Some(paragraph);
            }
        }
        if let Some(cell) = row.cells.get(boundary)
            && let Some(paragraph) = paragraph_in_cell(cell, remaining)
        {
            return Some(paragraph);
        }
    }
    None
}

fn paragraph_in_cell<'a>(cell: &'a CT_Tc, remaining: &mut usize) -> Option<&'a CT_P> {
    for content in &cell.content {
        let paragraph = match content {
            CellContent::Paragraph(paragraph) => take_paragraph(paragraph, remaining),
            CellContent::Table(table) => paragraph_in_table(table, remaining),
            CellContent::ContentControl(control) => {
                paragraph_in_control(control, BlockControlOwner::Cell, remaining)
            }
        };
        if paragraph.is_some() {
            return paragraph;
        }
    }
    None
}

fn paragraph_in_control<'a>(
    control: &'a CT_Sdt,
    owner: BlockControlOwner,
    remaining: &mut usize,
) -> Option<&'a CT_P> {
    for content in &control.content {
        let paragraph = match (owner, content) {
            (
                BlockControlOwner::Body | BlockControlOwner::Cell,
                SdtContent::Paragraph(paragraph),
            ) => take_paragraph(paragraph, remaining),
            (BlockControlOwner::Body | BlockControlOwner::Cell, SdtContent::Table(table)) => {
                paragraph_in_table(table, remaining)
            }
            (BlockControlOwner::Table, SdtContent::Row(row)) => paragraph_in_row(row, remaining),
            (BlockControlOwner::Row, SdtContent::Cell(cell)) => paragraph_in_cell(cell, remaining),
            (_, SdtContent::ContentControl(control)) => {
                paragraph_in_control(control, owner, remaining)
            }
            _ => None,
        };
        if paragraph.is_some() {
            return paragraph;
        }
    }
    None
}

fn take_paragraph<'a>(paragraph: &'a CT_P, remaining: &mut usize) -> Option<&'a CT_P> {
    if *remaining == 0 {
        Some(paragraph)
    } else {
        *remaining -= 1;
        None
    }
}

/// The last paragraph id of the thread root of the comment whose paragraph
/// is `para_id`. A parent link may name any paragraph of the parent, as
/// older producers wrote: it is read as the parent's last paragraph.
fn thread_root_para_id(
    comments: Option<&CT_Comments>,
    extended: &CT_CommentsEx,
    para_id: &str,
) -> String {
    let last = |para_id: &str| {
        comments
            .into_iter()
            .flat_map(|comments| &comments.comments)
            .find(|comment| para_ids(comment).any(|id| id == para_id))
            .and_then(last_para_id)
            .unwrap_or(para_id)
            .to_owned()
    };
    let mut current = last(para_id);
    let mut seen = HashSet::new();
    while seen.insert(current.clone()) {
        let Some(parent) = extended
            .comments
            .iter()
            .find(|entry| entry.para_id == current)
            .and_then(|entry| entry.para_id_parent.as_ref())
        else {
            break;
        };
        current = last(parent);
    }
    current
}

fn allocate_para_id(
    comments: Option<&CT_Comments>,
    extended: Option<&CT_CommentsEx>,
) -> Result<String> {
    allocate_para_id_from_occupied(&mut occupied_para_ids(comments, extended))
}

fn occupied_para_ids(
    comments: Option<&CT_Comments>,
    extended: Option<&CT_CommentsEx>,
) -> HashSet<u32> {
    let mut occupied = comments
        .into_iter()
        .flat_map(|comments| comments.comments.iter())
        .flat_map(|comment| comment.paragraph_ids.iter())
        .filter_map(Option::as_deref)
        .filter_map(parse_para_id)
        .collect::<HashSet<_>>();
    // The paragraphs of a table in a comment are kept as written.
    for (_, raw) in comments
        .into_iter()
        .flat_map(|comments| comments.comments.iter())
        .flat_map(|comment| comment.extra_xml.iter())
    {
        occupied.extend(raw_para_ids(raw));
    }
    occupied.extend(
        extended
            .into_iter()
            .flat_map(|extended| extended.comments.iter())
            .filter_map(|entry| parse_para_id(&entry.para_id)),
    );
    occupied
}

pub(crate) fn allocate_para_id_from_occupied(occupied: &mut HashSet<u32>) -> Result<String> {
    allocate_hex_id(occupied, PARA_ID_LIMIT)
}

pub(crate) fn allocate_durable_id_from_occupied(occupied: &mut HashSet<u32>) -> Result<String> {
    allocate_hex_id(occupied, DURABLE_ID_LIMIT)
}

/// An id above every one of `occupied`, else the lowest free one, above 0
/// and below `limit`, written as eight hexadecimal digits.
fn allocate_hex_id(occupied: &mut HashSet<u32>, limit: u32) -> Result<String> {
    let allocated = occupied
        .iter()
        .copied()
        .max()
        .map_or(1, |max| max.saturating_add(1));
    let allocated = match allocated < limit && allocated > 0 {
        true => allocated,
        false => (1..limit)
            .find(|candidate| !occupied.contains(candidate))
            .ok_or_else(|| Error::Other("no available comment id remains".to_owned()))?,
    };
    occupied.insert(allocated);
    Ok(format!("{allocated:08X}"))
}

/// The `paraId` attributes of `raw`, an element kept as written.
fn raw_para_ids(raw: &[u8]) -> Vec<u32> {
    let mut found = Vec::new();
    let mut reader = NsReader::from_reader(raw);
    let mut buffer = Vec::new();
    while let Ok(event) = reader.read_event_into(&mut buffer) {
        match event {
            Event::Start(element) | Event::Empty(element) => {
                for attribute in element.attributes().flatten() {
                    if attribute.key.local_name().as_ref() == b"paraId"
                        && let Ok(value) = attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                    {
                        found.extend(parse_para_id(&value));
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    found
}

fn parse_para_id(value: &str) -> Option<u32> {
    (value.len() == 8)
        .then(|| u32::from_str_radix(value, 16).ok())
        .flatten()
}

fn remove_owned_part(document: &mut Document, part: &str, relationship_type: &str) {
    document.package.remove_part(part);
    document.package.remove_part_rels(part);
    document.package.content_types.remove_override(part);
    document.identifiers.retire_authored_part(part);
    let owner = document.doc_part_name.clone();
    let mut removed_relationship_ids = Vec::new();
    if let Some(relationships) = document.package.get_part_rels_mut(&owner) {
        relationships.items.retain(|relationship| {
            let targets_part = relationship.rel_type == relationship_type
                && crate::document::relationship_is_internal(relationship)
                && OpcPackage::resolve_rel_target(&owner, &relationship.target) == part;
            let remove = targets_part
                && !document
                    .identifiers
                    .relationship_is_preserved(&owner, &relationship.id);
            if remove {
                removed_relationship_ids.push(relationship.id.clone());
            }
            !remove
        });
        if relationships.items.is_empty() {
            document.package.remove_part_rels(&owner);
        }
    }
    document
        .identifiers
        .retire_authored_story_relationships(&owner, removed_relationship_ids);
}

// Source marker presence deliberately ignores accepted-view range pairing.
// A raw reference inside an opaque wrapper still owns its definition.
#[derive(Debug)]
pub(crate) struct CommentSourceMarker {
    pub(crate) id: i32,
    pub(crate) family: usize,
    pub(crate) span: Range<usize>,
    empty_run: Option<Range<usize>>,
}

struct CommentPartEntry {
    attributes: BTreeMap<String, String>,
    span: Range<usize>,
}

struct CommentOwnership {
    markers: BTreeMap<String, Vec<CommentSourceMarker>>,
    parents: BTreeMap<i32, Option<i32>>,
    entries: BTreeMap<String, Vec<(i32, Range<usize>)>>,
}

impl CommentOwnership {
    fn marker_counts(
        markers: &BTreeMap<String, Vec<CommentSourceMarker>>,
    ) -> BTreeMap<i32, [usize; 3]> {
        let mut counts = BTreeMap::new();
        for marker in markers.values().flatten() {
            counts.entry(marker.id).or_insert([0; 3])[marker.family] += 1;
        }
        counts
    }

    fn descendants(&self, id: i32) -> HashSet<i32> {
        let mut ids = HashSet::from([id]);
        loop {
            let before = ids.len();
            for (&child, parent) in &self.parents {
                if parent.is_some_and(|parent| ids.contains(&parent)) {
                    ids.insert(child);
                }
            }
            if ids.len() == before {
                return ids;
            }
        }
    }

    /// The root of `id`'s thread, following defined parents. A malformed
    /// parent cycle stops once every definition was visited.
    fn thread_root(&self, id: i32) -> i32 {
        let mut root = id;
        for _ in 0..self.parents.len() {
            match self.parents.get(&root) {
                Some(Some(parent)) if self.parents.contains_key(parent) => root = *parent,
                _ => break,
            }
        }
        root
    }
}

// One actual prepared-source read shared by anchor projection and reply checks.
#[allow(clippy::type_complexity)] // Preserve the existing physical paragraph read tuple.
struct CommentAnchorRead {
    ownership: CommentOwnership,
    counts: BTreeMap<i32, [usize; 3]>,
    paragraphs: Vec<(
        ContentLocation,
        CommentParagraphValue,
        HashSet<i32>,
        Vec<u8>,
        (usize, usize),
        usize,
    )>,
    marker_paragraphs: HashMap<i32, Vec<usize>>,
    flow_paragraphs: HashMap<(usize, usize), Vec<usize>>,
}

impl CommentAnchorRead {
    fn from_prepared(source: &Document, selected: Option<i32>) -> Result<Self> {
        let ownership = source.comment_owned_graph_at(&source.doc_part_name, None, None)?;
        if let Some(id) = selected
            && !ownership.parents.contains_key(&id)
        {
            return Err(Error::Other(format!("unknown comment {id}")));
        }
        let counts = CommentOwnership::marker_counts(&ownership.markers);
        let located = source.comment_story_range_paragraphs()?;
        let mut values = source.comment_paragraph_read();
        let paragraphs = located
            .into_iter()
            .map(|(flow, offset, location, xml)| {
                let ids = comment_source_markers(&xml)?
                    .into_iter()
                    .map(|marker| marker.id)
                    .collect::<HashSet<_>>();
                let paragraph = values.read(&location, &xml)?;
                Ok((location, paragraph, ids, xml, flow, offset))
            })
            .collect::<Result<Vec<_>>>()?;
        // Physical order: the source, then the offset in it.
        let order = |index: &usize| (paragraphs[*index].4.0, paragraphs[*index].5);
        let mut marker_paragraphs = HashMap::<i32, Vec<usize>>::new();
        // A range runs through one flow, the paragraphs of one story owner and
        // of the table cells it holds, in physical order.
        let mut flow_paragraphs = HashMap::<(usize, usize), Vec<usize>>::new();
        for (index, (_, _, ids, _, flow, _)) in paragraphs.iter().enumerate() {
            flow_paragraphs.entry(*flow).or_default().push(index);
            for &id in ids {
                marker_paragraphs.entry(id).or_default().push(index);
            }
        }
        for indexes in flow_paragraphs
            .values_mut()
            .chain(marker_paragraphs.values_mut())
        {
            indexes.sort_by_key(order);
        }
        Ok(Self {
            ownership,
            counts,
            paragraphs,
            marker_paragraphs,
            flow_paragraphs,
        })
    }

    fn project(&self, id: i32) -> Result<(Option<StoryRunRange>, Option<String>)> {
        let order = |index: &usize| (self.paragraphs[*index].4.0, self.paragraphs[*index].5);
        let count = self.counts.get(&id).copied().unwrap_or_default();
        if count == [0; 3] {
            return Ok((None, None));
        }
        if count == [0, 0, 1] {
            return Ok((None, Some(String::new())));
        }
        if count[0] != 1 || count[1] != 1 || count[2] > 1 {
            return Err(Error::Other(format!(
                "comment {id} has duplicate or unmatched source markers"
            )));
        }
        let mut start = None::<StoryRunPosition>;
        let mut end = None::<StoryRunPosition>;
        let mut open = false;
        let mut text = String::new();
        let mut first_paragraph = None;
        for &index in self.marker_paragraphs.get(&id).into_iter().flatten() {
            let (_, _, _, xml, _, _) = &self.paragraphs[index];
            let (_, _, first, last) = CT_P::accepted_comment_source_projection(xml, id, false)?;
            if first.is_some() || last.is_some() {
                first_paragraph = Some(index);
                break;
            }
        }
        let first_paragraph = first_paragraph.ok_or_else(|| Error::Other(format!(
            "comment {id} source cannot be projected faithfully on the existing accepted run axis"
        )))?;
        let flow = &self.flow_paragraphs[&self.paragraphs[first_paragraph].4];
        let offset = flow
            .binary_search_by_key(&order(&first_paragraph), order)
            .expect("registered flow paragraph");
        for &index in &flow[offset..] {
            let (location, paragraph, ids, xml, _, _) = &self.paragraphs[index];
            let was_open = open;
            let (contribution, next_open, first, last) = if ids.contains(&id) {
                CT_P::accepted_comment_source_projection(xml, id, open)?
            } else {
                (paragraph.accepted_text(), open, None, None)
            };
            if let Some(index) = first {
                if start.is_some() {
                    return Err(Error::Other(format!(
                        "comment {id} has duplicate projected starts"
                    )));
                }
                start = Some(StoryRunPosition {
                    location: location.clone(),
                    run_index: index,
                });
            }
            if was_open || first.is_some() {
                if was_open {
                    text.push('\n');
                }
                text.push_str(&contribution);
            }
            if let Some(index) = last {
                if end.is_some() {
                    return Err(Error::Other(format!(
                        "comment {id} has duplicate projected ends"
                    )));
                }
                end = Some(StoryRunPosition {
                    location: location.clone(),
                    run_index: index,
                });
            }
            open = next_open;
            if end.is_some() {
                break;
            }
        }
        match (start, end) {
            (Some(start), Some(end)) if !open => {
                Ok((Some(StoryRunRange { start, end }), Some(text)))
            }
            (None, _) => Err(Error::Other(format!(
                "comment {id} source cannot be projected faithfully on the existing accepted run axis"
            ))),
            _ => Err(Error::Other(format!(
                "comment {id} range has no ordered end in the same story or its table cells"
            ))),
        }
    }

    fn reply_boundary(
        &self,
        parent_id: i32,
        parent: &StoryRunRange,
    ) -> Result<(crate::StoryId, StoryRunPosition)> {
        let no_range = || Error::Other(format!("comment id {parent_id} has no range to reply to"));
        let story = parent.start.location.story().clone();
        let inventory = &self.ownership.markers;
        let mut counts = [0usize; 3];
        for (part, markers) in inventory {
            for marker in markers.iter().filter(|marker| marker.id == parent_id) {
                if part != story.part_name() {
                    return Err(no_range());
                }
                counts[marker.family] += 1;
            }
        }
        if counts != [1, 1, 1] || parent.end.location.story() != &story {
            return Err(no_range());
        }
        let markers = inventory.get(story.part_name()).ok_or_else(no_range)?;
        let parent_end = markers
            .iter()
            .find(|marker| marker.id == parent_id && marker.family == 1)
            .ok_or_else(no_range)?;
        let parent_reference = markers
            .iter()
            .find(|marker| marker.id == parent_id && marker.family == 2)
            .ok_or_else(no_range)?;
        if parent_end.span.end > parent_reference.span.start {
            return Err(no_range());
        }
        let mut reference = None;
        for (location, paragraph, _, _, _, _) in &self.paragraphs {
            if location.story() != &story {
                continue;
            }
            for (index, path) in paragraph.accepted_run_paths().iter().enumerate() {
                let run = paragraph.accepted_run(path).ok_or_else(no_range)?;
                if !run.content.iter().any(|content| match content {
                    RunContent::CommentReference { id, .. } => *id == parent_id,
                    _ => false,
                }) {
                    continue;
                }
                if reference.is_some()
                    || location != &parent.end.location
                    || index != parent.end.run_index
                {
                    return Err(no_range());
                }
                if !reply_reference_has_only_metadata(run, parent_id)? {
                    return Err(no_range());
                }
                reference = Some(StoryRunPosition {
                    location: location.clone(),
                    run_index: index + 1,
                });
            }
        }
        let end = reference.ok_or_else(no_range)?;
        Ok((story, end))
    }
}

/// `xml` without the range marks and references of the comments `ids`, as
/// the staged removal drains them: a reference with its run when the run
/// holds nothing else.
fn without_comment_marks(xml: &[u8], ids: &HashSet<i32>) -> Result<Vec<u8>> {
    let mut ranges = comment_source_markers(xml)?
        .into_iter()
        .filter(|marker| ids.contains(&marker.id))
        .map(|marker| marker.empty_run.unwrap_or(marker.span))
        .collect::<Vec<_>>();
    ranges.sort_by_key(|range| range.start);
    ranges.dedup();
    let mut outer = Vec::<Range<usize>>::new();
    for range in ranges {
        if let Some(previous) = outer.last() {
            if range.end <= previous.end {
                continue;
            }
            if range.start < previous.end {
                return Err(Error::Other(
                    "comment removal spans overlap ambiguously".into(),
                ));
            }
        }
        outer.push(range);
    }
    let mut xml = xml.to_vec();
    for range in outer.into_iter().rev() {
        xml.drain(range);
    }
    Ok(xml)
}

pub(crate) fn comment_source_markers(xml: &[u8]) -> Result<Vec<CommentSourceMarker>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut markers = Vec::new();
    let mut preceding_run: Option<(usize, usize, Vec<u8>)> = None;
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(error.to_string()))?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let preceding = preceding_run.take();
                let end = reader.buffer_position() as usize;
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == oxoffice_docx_oxml::namespace::W_NS.as_bytes())
                {
                    if local.as_ref() == b"r"
                        && !empty
                        && element.attributes().all(|attribute| {
                            attribute.is_ok_and(|attribute| {
                                attribute.key.as_ref() == b"xmlns"
                                    || attribute.key.as_ref().starts_with(b"xmlns:")
                            })
                        })
                    {
                        preceding_run = Some((start, end, element.name().as_ref().to_vec()));
                    }
                    // Word styles a comment mark: run properties before the
                    // reference still leave a run holding nothing else.
                    if local.as_ref() == b"rPr"
                        && let Some((run_start, run_end, name)) = preceding.as_ref()
                        && *run_end == start
                    {
                        if !empty {
                            reader
                                .read_to_end_into(element.name(), &mut Vec::new())
                                .map_err(|error| Error::Other(error.to_string()))?;
                        }
                        preceding_run =
                            Some((*run_start, reader.buffer_position() as usize, name.clone()));
                    }
                    let family = match local.as_ref() {
                        b"commentRangeStart" => Some(0),
                        b"commentRangeEnd" => Some(1),
                        b"commentReference" => Some(2),
                        _ => None,
                    };
                    if let Some(family) = family {
                        let mut ids = 0;
                        for attribute in element.attributes() {
                            let attribute =
                                attribute.map_err(|error| Error::Other(error.to_string()))?;
                            let (namespace, local) =
                                reader.resolver().resolve_attribute(attribute.key);
                            if local.as_ref() == b"id"
                                && matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == oxoffice_docx_oxml::namespace::W_NS.as_bytes())
                            {
                                ids += 1;
                            }
                        }
                        if ids != 1 {
                            return Err(Error::Other(
                                "comment marker has missing or ambiguous qualified id".into(),
                            ));
                        }
                        let id = word_marker_attribute(&reader, &element, b"id")?
                            .and_then(|id| id.parse::<i32>().ok())
                            .ok_or_else(|| {
                                Error::Other("comment marker has an invalid qualified id".into())
                            })?;
                        if !empty {
                            reader
                                .read_to_end_into(element.name(), &mut Vec::new())
                                .map_err(|error| Error::Other(error.to_string()))?;
                        }
                        let marker_end = reader.buffer_position() as usize;
                        let empty_run = preceding.and_then(|(run_start, run_end, name)| {
                            let mut close = b"</".to_vec();
                            close.extend(name);
                            close.push(b'>');
                            (family == 2
                                && run_end == start
                                && xml.get(marker_end..marker_end + close.len())
                                    == Some(close.as_slice()))
                            .then_some(run_start..marker_end + close.len())
                        });
                        markers.push(CommentSourceMarker {
                            id,
                            family,
                            span: start..marker_end,
                            empty_run,
                        });
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(markers)
}

/// Scan only qualified direct entries, retaining exact source spans for removal.
fn comment_part_entries(
    xml: &[u8],
    namespace: &str,
    root: &[u8],
    item: &[u8],
) -> Result<Vec<CommentPartEntry>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut saw_root = false;
    let mut entries = Vec::new();
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(error.to_string()))?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let (resolved, local) = reader.resolver().resolve_element(element.name());
                let qualified = matches!(resolved, ResolveResult::Bound(Namespace(uri)) if uri == namespace.as_bytes());
                if depth == 0 {
                    if saw_root || !qualified || local.as_ref() != root {
                        return Err(Error::Other(
                            "comment part has no unique qualified root".into(),
                        ));
                    }
                    saw_root = true;
                } else if depth == 1 && qualified && local.as_ref() == item {
                    let mut attributes = BTreeMap::new();
                    for attribute in element.attributes() {
                        let attribute =
                            attribute.map_err(|error| Error::Other(error.to_string()))?;
                        let (resolved, local) = reader.resolver().resolve_attribute(attribute.key);
                        if matches!(resolved, ResolveResult::Bound(Namespace(uri)) if uri == namespace.as_bytes())
                        {
                            let key = String::from_utf8_lossy(local.as_ref()).into_owned();
                            let value = attribute
                                .decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|error| Error::Other(error.to_string()))?
                                .into_owned();
                            if attributes.insert(key, value).is_some() {
                                return Err(Error::Other(
                                    "comment part has duplicate qualified attributes".into(),
                                ));
                            }
                        }
                    }
                    if !empty {
                        reader
                            .read_to_end_into(element.name(), &mut Vec::new())
                            .map_err(|error| Error::Other(error.to_string()))?;
                    }
                    entries.push(CommentPartEntry {
                        attributes,
                        span: start..reader.buffer_position() as usize,
                    });
                    buffer.clear();
                    continue;
                }
                if !empty {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| Error::Other("comment XML is unbalanced".into()))?;
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !saw_root || depth != 0 {
        return Err(Error::Other("comment XML has no complete root".into()));
    }
    Ok(entries)
}

impl Document {
    /// Return a selected comment's exact accepted-view run range.
    /// Unknown or malformed ownership and unrepresentable endpoints are errors.
    /// Known orphans and reference-only points have no paired range. A range
    /// may run from one table cell into a later cell of the same story, or
    /// between a table cell and the story around its table, so its two ends
    /// are then in different story owners.
    pub fn comment_anchor(&self, id: i32) -> Result<Option<StoryRunRange>> {
        Ok(self
            .comment_anchor_snapshots_selected(Some(id))?
            .remove(&id)
            .ok_or_else(|| Error::Other(format!("unknown comment {id}")))??
            .0)
    }

    /// Return accepted span text, empty for a reference-only point and absent
    /// for a known orphan or a reply without its own source markers. One
    /// newline separates two paragraphs, table cells included.
    pub fn comment_anchor_text(&self, id: i32) -> Result<Option<String>> {
        Ok(self
            .comment_anchor_snapshots_selected(Some(id))?
            .remove(&id)
            .ok_or_else(|| Error::Other(format!("unknown comment {id}")))??
            .1)
    }

    /// Build checked owned anchor snapshots with one source inventory for listings.
    ///
    /// A failure of the document, such as its comment ownership, fails the
    /// whole listing. One comment that cannot be read fails this strict
    /// listing too. [`Self::comment_anchor_snapshot_results`] keeps that
    /// comment's error and still returns the others.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // Concrete shared range and text listing contract.
    pub fn comment_anchor_snapshots(
        &self,
    ) -> Result<BTreeMap<i32, (Option<StoryRunRange>, Option<String>)>> {
        self.comment_anchor_snapshot_results()?
            .into_iter()
            .map(|(id, snapshot)| Ok((id, snapshot?)))
            .collect()
    }

    /// Same inventory as [`Self::comment_anchor_snapshots`], with each
    /// comment's own projection result. An ownership or inventory failure
    /// still fails the whole map.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // Per-comment projection beside the strict listing.
    pub fn comment_anchor_snapshot_results(
        &self,
    ) -> Result<BTreeMap<i32, Result<(Option<StoryRunRange>, Option<String>)>>> {
        self.comment_anchor_snapshots_selected(None)
    }

    #[allow(clippy::type_complexity)] // Same concrete payload as the public batch accessor.
    fn comment_anchor_snapshots_selected(
        &self,
        selected: Option<i32>,
    ) -> Result<BTreeMap<i32, Result<(Option<StoryRunRange>, Option<String>)>>> {
        let mut source = self.clone_for_staging();
        source.flush_to_package()?;
        let read = CommentAnchorRead::from_prepared(&source, selected)?;
        Ok(read
            .ownership
            .parents
            .keys()
            .filter(|id| selected.is_none_or(|selected| selected == **id))
            .map(|&id| (id, read.project(id)))
            .collect())
    }

    /// Classify glossary review ownership from one actual internal relationship.
    pub(crate) fn glossary_comment_owner(&self) -> Result<Option<String>> {
        let Some(owner) = self.glossary_part_name.as_deref() else {
            return Ok(None);
        };
        let Some(_part) = self
            .comment_relationship_part_at(owner, oxoffice_opc::relationship::rel_types::COMMENTS)?
        else {
            if self
                .package
                .get_part_rels(owner)
                .iter()
                .flat_map(|rels| &rels.items)
                .any(|relationship| {
                    matches!(
                        relationship.rel_type.as_str(),
                        COMMENTS_EXTENDED_REL_TYPE
                            | COMMENTS_IDS_REL_TYPE
                            | COMMENTS_EXTENSIBLE_REL_TYPE
                    )
                })
            {
                return Err(Error::Other(
                    "glossary comment companions have no owned definitions relationship".into(),
                ));
            }
            return Ok(None);
        };
        self.comment_definition_entries_at(owner)?;
        self.comment_owned_graph_at(owner, None, None)?;
        // Two review graphs cannot claim the same physical part.
        for local in self
            .package
            .get_part_rels(owner)
            .iter()
            .flat_map(|rels| &rels.items)
        {
            if !matches!(
                local.rel_type.as_str(),
                oxoffice_opc::relationship::rel_types::COMMENTS
                    | COMMENTS_EXTENDED_REL_TYPE
                    | COMMENTS_IDS_REL_TYPE
                    | COMMENTS_EXTENSIBLE_REL_TYPE
            ) {
                continue;
            }
            if !crate::document::relationship_is_internal(local) {
                return Err(Error::Other(
                    "glossary comment companion is external".into(),
                ));
            }
            let target = OpcPackage::resolve_rel_target(owner, &local.target);
            if self
                .package
                .get_part_rels(&self.doc_part_name)
                .iter()
                .flat_map(|rels| &rels.items)
                .any(|main| {
                    crate::document::relationship_is_internal(main)
                        && crate::document::part_name_identity(&OpcPackage::resolve_rel_target(
                            &self.doc_part_name,
                            &main.target,
                        )) == crate::document::part_name_identity(&target)
                })
            {
                return Err(Error::Other(
                    "glossary and main comment owners share a dependency target".into(),
                ));
            }
        }
        // Shared unannotated notes are harmless. A marked physical source,
        // however, cannot belong to both independent review graphs.
        let main_sources = self.word_story_part_names();
        for local in self
            .package
            .get_part_rels(owner)
            .iter()
            .flat_map(|rels| &rels.items)
        {
            if !crate::document::relationship_is_internal(local)
                || !matches!(
                    local.rel_type.as_str(),
                    oxoffice_opc::relationship::rel_types::FOOTNOTES
                        | oxoffice_opc::relationship::rel_types::ENDNOTES
                )
            {
                continue;
            }
            let target = OpcPackage::resolve_rel_target(owner, &local.target);
            let identity = crate::document::part_name_identity(&target);
            if main_sources.iter().any(|part| {
                crate::document::part_name_identity(part) == identity
                    && crate::document::part_name_identity(part)
                        != crate::document::part_name_identity(owner)
            }) && let Some(xml) = self.package.get_part(&target)
                && !comment_source_markers(xml)?.is_empty()
            {
                return Err(Error::Other(
                    "glossary and main review owners share a marked physical source".into(),
                ));
            }
        }
        Ok(Some(owner.to_owned()))
    }

    /// Omit only qualified marker spans, retaining mixed runs and opaque siblings.
    pub(crate) fn omit_comment_markers(xml: &[u8]) -> Result<Vec<u8>> {
        let edits = Self::omit_comment_marker_edits(xml)?;
        let mut result = xml.to_vec();
        for (start, end, _) in edits {
            result.drain(start..end);
        }
        Ok(result)
    }

    pub(crate) fn omit_comment_marker_edits(xml: &[u8]) -> Result<Vec<(usize, usize, Vec<u8>)>> {
        let markers = comment_source_markers(xml)?;
        let mut edits = Vec::new();
        for marker in markers.into_iter().rev() {
            let mut reader =
                oxoffice_core::xml_reader::Reader::from_reader(&xml[marker.span.clone()]);
            if matches!(
                reader
                    .read_event()
                    .map_err(|error| Error::Other(error.to_string()))?,
                Event::Start(_)
            ) && !matches!(
                reader
                    .read_event()
                    .map_err(|error| Error::Other(error.to_string()))?,
                Event::End(_)
            ) {
                return Err(Error::Other(format!(
                    "cannot omit comment {} with opaque marker content",
                    marker.id
                )));
            }
            edits.push((marker.span.start, marker.span.end, Vec::new()));
        }
        Ok(edits)
    }

    fn comment_relationship_part_at(
        &self,
        owner: &str,
        relationship_type: &str,
    ) -> Result<Option<String>> {
        let mut target = None;
        for relationship in self
            .package
            .get_part_rels(owner)
            .iter()
            .flat_map(|rels| &rels.items)
            .filter(|relationship| relationship.rel_type == relationship_type)
        {
            if !crate::document::relationship_is_internal(relationship) || target.is_some() {
                return Err(Error::Other(
                    "comment part ownership is external or ambiguous".into(),
                ));
            }
            let part = OpcPackage::resolve_rel_target(owner, &relationship.target);
            if self.package.get_part(&part).is_none() {
                return Err(Error::Other(format!(
                    "comment relationship targets missing part {part}"
                )));
            }
            target = Some(part);
        }
        Ok(target)
    }

    pub(crate) fn comment_source_inventory_at(
        &self,
        owner: &str,
    ) -> Result<BTreeMap<String, Vec<CommentSourceMarker>>> {
        self.comment_source_inventory_with(owner, None)
    }

    /// [`Self::comment_source_inventory_at`], the main part read as `main`
    /// when given (the body blocks an in-place edit changes, written alone).
    fn comment_source_inventory_with(
        &self,
        owner: &str,
        main: Option<&[u8]>,
    ) -> Result<BTreeMap<String, Vec<CommentSourceMarker>>> {
        if self.package.get_part(&self.doc_part_name).is_none() {
            return Err(Error::Other("main comment source part is missing".into()));
        }
        let mut markers = BTreeMap::new();
        let local = if owner == self.doc_part_name {
            self.glossary_comment_owner()?
        } else {
            None
        };
        let parts = if owner == self.doc_part_name {
            self.word_story_part_names()
                .into_iter()
                .filter(|part| local.as_deref() != Some(part.as_str()))
                .collect::<Vec<_>>()
        } else {
            let mut parts = vec![owner.to_owned()];
            for relationship in self
                .package
                .get_part_rels(owner)
                .iter()
                .flat_map(|rels| &rels.items)
            {
                if crate::document::relationship_is_internal(relationship)
                    && matches!(
                        relationship.rel_type.as_str(),
                        oxoffice_opc::relationship::rel_types::COMMENTS
                            | oxoffice_opc::relationship::rel_types::FOOTNOTES
                            | oxoffice_opc::relationship::rel_types::ENDNOTES
                    )
                {
                    parts.push(OpcPackage::resolve_rel_target(owner, &relationship.target));
                }
            }
            parts
        };
        for part in parts {
            // Dangling story relationships contain no source to inventory.
            // Package validation diagnoses them independently of comment ownership.
            let xml = match main {
                Some(main) if part == self.doc_part_name => Some(main),
                _ => self.package.get_part(&part),
            };
            if let Some(xml) = xml {
                markers.insert(part, comment_source_markers(xml)?);
            }
        }
        Ok(markers)
    }

    fn comment_definition_entries_at(
        &self,
        owner: &str,
    ) -> Result<Option<(String, Vec<CommentPartEntry>)>> {
        let Some(part) = self
            .comment_relationship_part_at(owner, oxoffice_opc::relationship::rel_types::COMMENTS)?
        else {
            if owner == self.doc_part_name && self.comments_part_name.is_some() {
                return Err(Error::Other(
                    "owned comment definition relationship is missing".into(),
                ));
            }
            return Ok(None);
        };
        let entries = comment_part_entries(
            self.package.get_part(&part).expect("checked part"),
            oxoffice_docx_oxml::namespace::W_NS,
            b"comments",
            b"comment",
        )?;
        let mut ids = HashSet::new();
        for entry in &entries {
            let id = entry
                .attributes
                .get("id")
                .and_then(|id| id.parse::<i32>().ok())
                .ok_or_else(|| Error::Other("comment definition has invalid id".into()))?;
            if !ids.insert(id) {
                return Err(Error::Other(format!(
                    "comment {id} has duplicate definitions"
                )));
            }
        }
        Ok(Some((part, entries)))
    }

    /// Maintain the actual final paragraph identity of a changed comment.
    /// Unchanged legacy absence and imported durable-ID bytes are preserved.
    pub(crate) fn reconcile_comment_paragraph_publication(
        &mut self,
        next: &mut CT_Comments,
    ) -> Result<bool> {
        let Some(old) = self.comments.as_ref() else {
            return Ok(false);
        };
        let old_by_id = old
            .comments
            .iter()
            .map(|comment| (comment.id, comment))
            .collect::<BTreeMap<_, _>>();
        let needs_publication = next.comments.iter().any(|comment| {
            old_by_id.get(&comment.id).is_some_and(|old| {
                (comment.paragraphs.len() > old.paragraphs.len() && last_para_id(comment).is_none())
                    || (last_para_id(old).is_some() && last_para_id(old) != last_para_id(comment))
            })
        });
        if !needs_publication {
            return Ok(false);
        }
        self.comment_owned_graph_at(&self.doc_part_name, Some(old), None)?;
        let next_ids = next
            .comments
            .iter()
            .map(|comment| comment.id)
            .collect::<HashSet<_>>();
        if next_ids.len() != next.comments.len() || next_ids != old_by_id.keys().copied().collect()
        {
            return Err(Error::Other(
                "comment paragraph publication changes definition ownership".into(),
            ));
        }
        let mut occupied = occupied_para_ids(Some(old), self.comments_extended.as_ref());
        occupied.extend(
            next.comments
                .iter()
                .flat_map(para_ids)
                .filter_map(parse_para_id),
        );
        let mut rebind = BTreeMap::new();
        for comment in &mut next.comments {
            let previous = old_by_id[&comment.id];
            let appended = comment.paragraphs.len() > previous.paragraphs.len();
            let changed_terminal =
                last_para_id(previous).is_some() && last_para_id(previous) != last_para_id(comment);
            if !(appended && last_para_id(comment).is_none()) && !changed_terminal {
                continue;
            }
            if comment.paragraphs.is_empty() {
                return Err(Error::Other(
                    "comment companion needs an actual final paragraph".into(),
                ));
            }
            if last_para_id(comment).is_none() && appended {
                comment.paragraph_ids.resize(comment.paragraphs.len(), None);
                let last = comment.paragraphs.len() - 1;
                comment.paragraph_ids[last] = Some(allocate_para_id_from_occupied(&mut occupied)?);
            }
            if let Some(old_id) = last_para_id(previous) {
                let new_id = last_para_id(comment).ok_or_else(|| {
                    Error::Other(
                        "comment publication lost its existing final paragraph identity".into(),
                    )
                })?;
                if !old_id.eq_ignore_ascii_case(new_id) {
                    rebind.insert(old_id.to_ascii_uppercase(), new_id.to_owned());
                }
            }
        }
        let mut updates = BTreeMap::new();
        let mut next_extended = None;
        if let Some(part) =
            self.comment_relationship_part_at(&self.doc_part_name, COMMENTS_EXTENDED_REL_TYPE)?
        {
            let source = self
                .package
                .get_part(&part)
                .expect("checked companion source");
            let mut source_spellings = BTreeMap::new();
            for entry in comment_part_entries(
                source,
                oxoffice_docx_oxml::comments_extended::W15_NS,
                b"commentsEx",
                b"commentEx",
            )? {
                for key in ["paraId", "paraIdParent"] {
                    if let Some(old_value) = entry.attributes.get(key)
                        && let Some(new_value) = rebind.get(&old_value.to_ascii_uppercase())
                    {
                        source_spellings.insert(old_value.clone(), new_value.clone());
                    }
                }
            }
            if !source_spellings.is_empty() {
                let xml = crate::field::patch_ref_comment_identity_attributes(
                    source,
                    &BTreeMap::new(),
                    &source_spellings,
                    &BTreeMap::new(),
                )?;
                next_extended = Some(CT_CommentsEx::from_xml(&xml)?);
                updates.insert(part, xml);
            }
        }
        // The durable ids follow the last paragraph too: an entry of
        // commentsIds is keyed by it (W10). Its other bytes stay as written.
        if let Some((part, entries)) = self.comment_companion(&COMMENT_IDS)? {
            let source_spellings = entries
                .iter()
                .filter_map(|entry| {
                    let old_value = entry.attributes.get("paraId")?;
                    let new_value = rebind.get(&old_value.to_ascii_uppercase())?;
                    Some((old_value.clone(), new_value.clone()))
                })
                .collect::<BTreeMap<_, _>>();
            if !source_spellings.is_empty() {
                let source = self
                    .package
                    .get_part(&part)
                    .expect("checked companion source");
                let xml = crate::field::patch_ref_comment_identity_attributes(
                    source,
                    &BTreeMap::new(),
                    &source_spellings,
                    &BTreeMap::new(),
                )?;
                updates.insert(part, xml);
            }
        }
        // A companion whose old target cannot remain valid refuses here.
        self.comment_owned_graph_at(&self.doc_part_name, Some(next), Some(&updates))?;
        for (part, xml) in updates {
            self.package.set_part(&part, xml);
        }
        if let Some(extended) = next_extended {
            self.comments_extended = Some(extended);
        }
        Ok(true)
    }

    fn comment_owned_graph_at(
        &self,
        owner: &str,
        parsed: Option<&CT_Comments>,
        companion_overrides: Option<&BTreeMap<String, Vec<u8>>>,
    ) -> Result<CommentOwnership> {
        self.comment_owned_graph_with(owner, parsed, companion_overrides, None)
    }

    fn comment_owned_graph_with(
        &self,
        owner: &str,
        parsed: Option<&CT_Comments>,
        companion_overrides: Option<&BTreeMap<String, Vec<u8>>>,
        main: Option<&[u8]>,
    ) -> Result<CommentOwnership> {
        let mut ownership = CommentOwnership {
            markers: self.comment_source_inventory_with(owner, main)?,
            parents: BTreeMap::new(),
            entries: BTreeMap::new(),
        };
        let source_definitions = self.comment_definition_entries_at(owner)?;
        let owned;
        let comments = if let Some(parsed) = parsed {
            parsed
        } else {
            owned = if let Some((part, _)) = &source_definitions {
                CT_Comments::from_xml(self.package.get_part(part).expect("checked part"))?
            } else {
                CT_Comments::new()
            };
            &owned
        };
        let mut by_para = BTreeMap::new();
        let mut by_parent_para = BTreeMap::new();
        let mut definitions = Vec::new();
        for entry in source_definitions
            .as_ref()
            .into_iter()
            .flat_map(|(_, entries)| entries)
        {
            let id = entry
                .attributes
                .get("id")
                .and_then(|id| id.parse::<i32>().ok())
                .ok_or_else(|| Error::Other("comment definition has invalid id".into()))?;
            if ownership.parents.insert(id, None).is_some() {
                return Err(Error::Other(format!(
                    "comment {id} has duplicate definitions"
                )));
            }
            definitions.push((id, entry.span.clone()));
        }
        if comments.comments.len() != definitions.len() {
            return Err(Error::Other(
                "comment definitions cannot be qualified without losing opaque ownership".into(),
            ));
        }
        for comment in &comments.comments {
            for para in para_ids(comment) {
                if by_parent_para
                    .insert(para.to_ascii_uppercase(), comment.id)
                    .is_some()
                {
                    return Err(Error::Other(
                        "comment paragraph identity is ambiguous".into(),
                    ));
                }
            }
            if !ownership.parents.contains_key(&comment.id) {
                return Err(Error::Other(
                    "comment model differs from qualified source definitions".into(),
                ));
            }
            if let Some(para) = last_para_id(comment)
                && by_para
                    .insert(para.to_ascii_uppercase(), comment.id)
                    .is_some()
            {
                return Err(Error::Other(
                    "comment last-paragraph identity is ambiguous".into(),
                ));
            }
        }
        if let Some((part, _)) = source_definitions {
            ownership.entries.insert(part, definitions);
        }
        const IDS_REL: &str = COMMENTS_IDS_REL_TYPE;
        const EXTENSIBLE_REL: &str = COMMENTS_EXTENSIBLE_REL_TYPE;
        const IDS_NS: &str = COMMENT_IDS.namespace;
        const CEX_NS: &str = COMMENT_EXTENSIBLE.namespace;
        let mut by_durable = BTreeMap::new();
        for (relationship, namespace, root, item, key) in [
            (
                COMMENTS_EXTENDED_REL_TYPE,
                oxoffice_docx_oxml::comments_extended::W15_NS,
                b"commentsEx".as_slice(),
                b"commentEx".as_slice(),
                "paraId",
            ),
            (
                IDS_REL,
                IDS_NS,
                b"commentsIds".as_slice(),
                b"commentId".as_slice(),
                "paraId",
            ),
            (
                EXTENSIBLE_REL,
                CEX_NS,
                b"commentsExtensible".as_slice(),
                b"commentExtensible".as_slice(),
                "durableId",
            ),
        ] {
            let Some(part) = self.comment_relationship_part_at(owner, relationship)? else {
                continue;
            };
            let xml = companion_overrides
                .and_then(|parts| parts.get(&part))
                .map(Vec::as_slice)
                .unwrap_or_else(|| self.package.get_part(&part).expect("checked part"));
            let mut mapped = Vec::new();
            let mut seen = HashSet::new();
            for entry in comment_part_entries(xml, namespace, root, item)? {
                let value = entry
                    .attributes
                    .get(key)
                    .ok_or_else(|| Error::Other(format!("comment companion has no {key}")))?
                    .to_ascii_uppercase();
                if !seen.insert(value.clone()) {
                    return Err(Error::Other(format!(
                        "comment companion {key} {value} is ambiguous"
                    )));
                }
                let id = if key == "paraId" {
                    by_para.get(&value)
                } else {
                    by_durable.get(&value)
                }
                .copied()
                .ok_or_else(|| {
                    Error::Other(format!(
                        "comment companion {key} {value} has unprovable linkage"
                    ))
                })?;
                if relationship == COMMENTS_EXTENDED_REL_TYPE
                    && let Some(parent) = entry.attributes.get("paraIdParent")
                {
                    let parent = by_parent_para
                        .get(&parent.to_ascii_uppercase())
                        .copied()
                        .ok_or_else(|| {
                            Error::Other(format!("comment {id} has unknown parent {parent}"))
                        })?;
                    ownership.parents.insert(id, Some(parent));
                }
                if relationship == IDS_REL {
                    let durable = entry
                        .attributes
                        .get("durableId")
                        .ok_or_else(|| {
                            Error::Other(format!("comment {id} has no durable identity"))
                        })?
                        .to_ascii_uppercase();
                    if durable.len() != 8
                        || u32::from_str_radix(&durable, 16).is_err()
                        || by_durable.insert(durable, id).is_some()
                    {
                        return Err(Error::Other(format!(
                            "comment {id} has invalid or duplicate durable identity"
                        )));
                    }
                }
                mapped.push((id, entry.span));
            }
            ownership.entries.insert(part, mapped);
        }
        for &id in ownership.parents.keys() {
            let mut seen = HashSet::new();
            let mut ancestor = Some(id);
            while let Some(current) = ancestor {
                if !seen.insert(current) {
                    return Err(Error::Other(format!(
                        "comment {id} has cyclic thread ownership"
                    )));
                }
                ancestor = ownership.parents[&current];
            }
        }
        Ok(ownership)
    }

    fn comment_ownership_at(&self, owner: &str) -> Result<CommentOwnership> {
        self.comment_ownership_with(owner, None)
    }

    /// [`Self::comment_ownership_at`], the main part read as `main` when
    /// given.
    fn comment_ownership_with(&self, owner: &str, main: Option<&[u8]>) -> Result<CommentOwnership> {
        let ownership = self.comment_owned_graph_with(owner, None, None, main)?;
        let counts = CommentOwnership::marker_counts(&ownership.markers);
        if !counts.is_empty()
            && self
                .comment_relationship_part_at(
                    owner,
                    oxoffice_opc::relationship::rel_types::COMMENTS,
                )?
                .is_none()
        {
            return Err(Error::Other(
                "comment markers have no owned definitions part".into(),
            ));
        }
        for id in counts.keys() {
            if !ownership.parents.contains_key(id) {
                return Err(Error::Other(format!(
                    "comment {id} has markers but no definition"
                )));
            }
        }
        Ok(ownership)
    }

    /// Check raw comment presence and thread ownership across relationship-resolved stories.
    #[doc(hidden)]
    pub fn validate_comment_ownership(&self) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let mut owners = vec![candidate.doc_part_name.clone()];
        owners.extend(candidate.glossary_comment_owner()?);
        for owner in owners {
            let ownership = candidate.comment_ownership_at(&owner)?;
            let counts = CommentOwnership::marker_counts(&ownership.markers);
            for (&id, parent) in &ownership.parents {
                let count = counts.get(&id).copied().unwrap_or_default();
                if count.iter().any(|count| *count > 1) || count[0] != count[1] {
                    return Err(Error::Other(format!(
                        "comment {id} has duplicate or unmatched source markers"
                    )));
                }
                if parent.is_none() && count == [0; 3] {
                    return Err(Error::Other(format!(
                        "comment {id} is an orphan root with no source range or reference"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Compare one unpublished destructive edit with its frozen source inventory.
    pub(crate) fn reconcile_comment_removal(&mut self, before: &Document) -> Result<()> {
        let mut owners = vec![before.doc_part_name.clone()];
        let local = before.glossary_comment_owner()?;
        if local != self.glossary_comment_owner()? {
            return Err(Error::Other(
                "glossary comment relationship owner changed during edit".into(),
            ));
        }
        owners.extend(local);
        for owner in owners {
            self.reconcile_comment_removal_at(before, &owner)?;
        }
        Ok(())
    }

    /// Copy from `source` the definition of every comment a marker of this
    /// document names and no definition here holds, into this document's
    /// comments part, made from `source`'s when it has none. A comparison
    /// keeps the markers of a removed comment in the deleted content until
    /// the deletion is accepted, and so the comment. Its companion entries
    /// (reply, resolved state) are not copied: a comment needs none.
    pub(crate) fn keep_comment_definitions_from(&mut self, source: &Document) -> Result<()> {
        let owner = self.doc_part_name.clone();
        let defined_ids = |entries: &[CommentPartEntry]| {
            entries
                .iter()
                .filter_map(|entry| entry.attributes.get("id")?.parse::<i32>().ok())
                .collect::<HashSet<_>>()
        };
        let defined = self
            .comment_definition_entries_at(&owner)?
            .map(|(_, entries)| defined_ids(&entries))
            .unwrap_or_default();
        let missing = self
            .comment_source_inventory_at(&owner)?
            .values()
            .flatten()
            .map(|marker| marker.id)
            .filter(|id| !defined.contains(id))
            .collect::<HashSet<_>>();
        if missing.is_empty() {
            return Ok(());
        }
        let Some((source_part, entries)) =
            source.comment_definition_entries_at(&source.doc_part_name)?
        else {
            return Ok(());
        };
        let source_xml = source
            .package
            .get_part(&source_part)
            .expect("checked comments part");
        let mut kept = Vec::new();
        for entry in entries.iter().filter(|entry| {
            entry
                .attributes
                .get("id")
                .and_then(|id| id.parse::<i32>().ok())
                .is_some_and(|id| missing.contains(&id))
        }) {
            let scope = crate::document::story_namespace_scope_at(source_xml, entry.span.start)?;
            kept.extend(crate::document::close_content_fragment_namespaces(
                &source_xml[entry.span.clone()],
                &scope,
            )?);
        }
        if kept.is_empty() {
            return Ok(());
        }
        let kept = String::from_utf8(kept).map_err(|error| Error::Other(error.to_string()))?;
        let text = |xml: &[u8]| {
            std::str::from_utf8(xml)
                .map(str::to_owned)
                .map_err(|error| Error::Other(error.to_string()))
        };
        let with_kept = |xml: &str| -> Result<String> {
            let root = oxoffice_core::xml_element::root_element_span(xml)?;
            let (start, end) = (root.start_tag.start, root.end_tag.end);
            Ok(format!(
                "{}{}{}",
                &xml[..start],
                oxoffice_core::xml_element::append_content(&xml[start..end], &kept)?,
                &xml[end..]
            ))
        };
        let part = self.comment_relationship_part_at(
            &owner,
            oxoffice_opc::relationship::rel_types::COMMENTS,
        )?;
        if let Some(part) = part {
            let xml = with_kept(&text(self.package.get_part(&part).expect("checked part"))?)?;
            self.package.set_part(&part, xml.into_bytes());
            return Ok(());
        }
        // A part of the source's own name, holding only the kept comments.
        if self.package.get_part(&source_part).is_some() {
            return Err(Error::Other(format!(
                "comment definitions cannot be kept: {source_part} is another part"
            )));
        }
        let shell = text(source_xml)?;
        let root = oxoffice_core::xml_element::root_element_span(&shell)?;
        let (start, end) = (root.start_tag.start, root.end_tag.end);
        let empty = oxoffice_core::xml_element::replace_content(&shell[start..end], "")?.0;
        let xml = with_kept(&format!("{}{empty}{}", &shell[..start], &shell[end..]))?;
        self.package.set_part(&source_part, xml.into_bytes());
        self.package
            .content_types
            .add_override(&source_part, COMMENTS_CONTENT_TYPE);
        let target = source_part
            .strip_prefix(&format!(
                "{}/",
                owner.rsplit_once('/').map_or("", |(folder, _)| folder)
            ))
            .unwrap_or(&source_part)
            .to_owned();
        self.package
            .get_or_create_part_rels(&owner)
            .add(oxoffice_opc::relationship::rel_types::COMMENTS, &target);
        Ok(())
    }

    fn reconcile_comment_removal_at(&mut self, before: &Document, owner: &str) -> Result<()> {
        let mut source = before.clone_for_staging();
        source.flush_to_package()?;
        self.flush_to_package()?;
        let before_counts =
            CommentOwnership::marker_counts(&source.comment_source_inventory_at(owner)?);
        let after_counts =
            CommentOwnership::marker_counts(&self.comment_source_inventory_at(owner)?);
        // Only source ownership decreases need deletion proof. Unchanged
        // or cloned anchors must not validate or repair unrelated producer
        // definitions. Explicit deletion and CLI validation remain strict.
        if before_counts.iter().all(|(id, count)| {
            let after = after_counts.get(id).copied().unwrap_or_default();
            count
                .iter()
                .zip(after)
                .all(|(before, after)| after >= *before)
        }) {
            return Ok(());
        }
        // A demonstrably undefined source marker cannot orphan an absent
        // definition. Retain its established editing/comparison lifecycle.
        // Raw qualified entries prove absence even if typed projection omits one.
        let definitions = source.comment_definition_entries_at(owner)?;
        let defined_decrease = definitions.as_ref().is_some_and(|(_, entries)| {
            entries.iter().any(|entry| {
                let id = entry.attributes["id"]
                    .parse::<i32>()
                    .expect("validated definition id");
                let before = before_counts.get(&id).copied().unwrap_or_default();
                let after = after_counts.get(&id).copied().unwrap_or_default();
                before
                    .iter()
                    .zip(after)
                    .any(|(before, after)| after < *before)
            })
        });
        if !defined_decrease {
            return Ok(());
        }
        let original = source.comment_ownership_at(owner)?;
        let remaining = self.comment_ownership_at(owner)?;
        let before_counts = CommentOwnership::marker_counts(&original.markers);
        let after_counts = CommentOwnership::marker_counts(&remaining.markers);
        let mut removed = HashSet::new();
        for (&id, count) in &before_counts {
            let after = after_counts.get(&id).copied().unwrap_or_default();
            if count
                .iter()
                .zip(after)
                .all(|(before, after)| after >= *before)
            {
                continue;
            }
            if after != [0; 3] || count.iter().any(|count| *count > 1) || count[0] != count[1] {
                return Err(Error::Other(format!(
                    "cannot remove part of comment {id}; its source graph survives or is ambiguous"
                )));
            }
            removed.extend(original.descendants(id));
        }
        for id in &removed {
            if after_counts.get(id).is_some_and(|counts| *counts != [0; 3]) {
                return Err(Error::Other(format!(
                    "cannot remove comment {id}; a descendant source anchor survives"
                )));
            }
        }
        if !removed.is_empty() {
            self.remove_comment_ids_staged_at(owner, &removed)?;
        }
        Ok(())
    }

    pub(crate) fn refuse_commented_fragment(xml: &[u8]) -> Result<()> {
        if let Some(marker) = comment_source_markers(xml)?.first() {
            return Err(Error::Other(format!(
                "cannot detach content bearing comment {}; fragments do not own comment threads",
                marker.id
            )));
        }
        Ok(())
    }

    fn remove_comment_ids_staged_at(&mut self, owner: &str, ids: &HashSet<i32>) -> Result<()> {
        self.remove_comment_ids_with(owner, ids, None)
    }

    /// [`Self::remove_comment_ids_staged_at`], the main part read as `main`
    /// when given: its marks are left to the caller, which removes them
    /// from the body blocks `main` holds.
    fn remove_comment_ids_with(
        &mut self,
        owner: &str,
        ids: &HashSet<i32>,
        main: Option<&[u8]>,
    ) -> Result<()> {
        let ownership = self.comment_ownership_with(owner, main)?;
        for (part, entries) in &ownership.entries {
            for (_, span) in entries.iter().filter(|(id, _)| ids.contains(id)) {
                if let Some(marker) =
                    ownership
                        .markers
                        .get(part)
                        .into_iter()
                        .flatten()
                        .find(|marker| {
                            !ids.contains(&marker.id)
                                && span.start <= marker.span.start
                                && marker.span.end <= span.end
                        })
                {
                    return Err(Error::Other(format!(
                        "cannot remove a comment definition carrying unrelated comment {}",
                        marker.id
                    )));
                }
            }
        }
        let mut spans = BTreeMap::<String, Vec<Range<usize>>>::new();
        for (part, markers) in ownership.markers {
            for marker in markers
                .into_iter()
                .filter(|marker| ids.contains(&marker.id))
            {
                spans
                    .entry(part.clone())
                    .or_default()
                    .push(marker.empty_run.unwrap_or(marker.span));
            }
        }
        for (part, entries) in ownership.entries {
            for (id, span) in entries.into_iter().filter(|(id, _)| ids.contains(id)) {
                let _ = id;
                spans.entry(part.clone()).or_default().push(span);
            }
        }
        for (part, mut ranges) in spans {
            if main.is_some() && part == self.doc_part_name {
                continue;
            }
            ranges.sort_by_key(|range| range.start);
            ranges.dedup();
            // A removed definition can contain a source marker of another removed comment.
            let mut outer = Vec::<Range<usize>>::new();
            for range in ranges {
                if let Some(previous) = outer.last() {
                    if range.end <= previous.end {
                        continue;
                    }
                    if range.start < previous.end {
                        return Err(Error::Other(
                            "comment removal spans overlap ambiguously".into(),
                        ));
                    }
                }
                outer.push(range);
            }
            let mut xml = self
                .package
                .get_part(&part)
                .ok_or_else(|| Error::Other("comment removal part disappeared".into()))?
                .to_vec();
            for range in outer.into_iter().rev() {
                xml.drain(range);
            }
            if self.comments_extended_part_name.as_deref() == Some(part.as_str()) {
                self.comments_extended = Some(CT_CommentsEx::from_xml(&xml)?);
            }
            crate::document::set_story_source_xml(self, &part, xml)?;
        }
        if owner == self.doc_part_name {
            self.identifiers
                .retire_authored_comment_ids(ids.iter().copied());
            self.remove_owned_empty_comment_parts();
            self.comments_dirty = false;
        }
        self.invalidate_layout();
        Ok(())
    }
}

/// A permission range names exactly one editor or editor group, of XML text.
fn check_permission_holder(editor: Option<&str>, group: Option<&str>) -> Result<()> {
    if editor.is_some() == group.is_some()
        || editor.is_some_and(str::is_empty)
        || group.is_some_and(str::is_empty)
    {
        return Err(Error::Other(
            "permission range needs one editor or group".to_owned(),
        ));
    }
    oxoffice_core::xml::reject_non_xml_characters("permission editor", editor.unwrap_or_default())?;
    oxoffice_core::xml::reject_non_xml_characters("permission group", group.unwrap_or_default())?;
    Ok(())
}
