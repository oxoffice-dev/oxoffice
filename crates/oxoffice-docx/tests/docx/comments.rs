//! Comments: adding them on text, on run ranges and in other stories, author,
//! initials and date, reply threads, the done state, editing, moving and
//! removing them, and the comments Word, Google Docs and LibreOffice saved.

use std::fs;
use std::io::Cursor;
use std::path::Path;

use oxoffice_docx::paragraph::ParagraphItemRef;
use oxoffice_docx::{
    CommentDate, CommentRef, ContentLocation, Document, HalfPoint, RunItemRef, RunPosition,
    RunRange, StoryItemKind, StoryKind, StoryRunPosition, StoryRunRange, StyleBuilder, StyleType,
    TextCommentOptions, Twips,
};
use oxoffice_opc::OpcPackage;

use crate::support::{corpus_doc, doc_with, doc_with_body, reopen, texts};

const PRODUCERS: [&str; 4] = [
    "word/report.docx",
    "google/report.docx",
    "libreoffice/report.docx",
    "authored/report.docx",
];
const CORPUS_ANCHOR: &str = "The budget for the billing migration is still to be confirmed.";
const CORPUS_TEXT: &str = "Confirm with finance before sending.";

/// Runs `start_run..end_run`, from body paragraph `start` to body paragraph `end`.
fn run_range(start: usize, start_run: usize, end: usize, end_run: usize) -> RunRange {
    RunRange {
        start: RunPosition {
            body_index: start,
            run_index: start_run,
        },
        end: RunPosition {
            body_index: end,
            run_index: end_run,
        },
    }
}

/// Runs `start_run..end_run` of the story paragraph at `location`.
fn story_range(location: &ContentLocation, start_run: usize, end_run: usize) -> StoryRunRange {
    StoryRunRange {
        start: StoryRunPosition {
            location: location.clone(),
            run_index: start_run,
        },
        end: StoryRunPosition {
            location: location.clone(),
            run_index: end_run,
        },
    }
}

/// The first paragraph of the `nth` story of this kind, in story order.
fn story_paragraph(doc: &Document, kind: StoryKind, nth: usize) -> ContentLocation {
    let story = doc
        .stories()
        .expect("stories")
        .into_iter()
        .filter(|story| story.kind() == kind)
        .nth(nth)
        .unwrap_or_else(|| panic!("{kind:?} story {nth} exists"));
    doc.story_items(&story).expect("story items")[0]
        .location()
        .clone()
}

/// Comment `id`, as `Document::comments` lists it.
fn comment(doc: &Document, id: i32) -> CommentRef<'_> {
    doc.comments()
        .into_iter()
        .find(|comment| comment.id() == id)
        .unwrap_or_else(|| panic!("comment {id} exists"))
}

/// The paragraphs of comment `id`, as the story locations that
/// `Document::set_story_text` edits. Each comment is one story, in the order
/// `Document::comments` lists them.
fn comment_paragraphs(doc: &Document, id: i32) -> Vec<ContentLocation> {
    let position = doc
        .comments()
        .iter()
        .position(|comment| comment.id() == id)
        .unwrap_or_else(|| panic!("comment {id} exists"));
    let story = doc
        .stories()
        .expect("stories")
        .into_iter()
        .filter(|story| story.kind() == StoryKind::Comment)
        .nth(position)
        .expect("one story per comment");
    doc.story_items(&story)
        .expect("comment story items")
        .iter()
        .map(|item| item.location().clone())
        .collect()
}

/// The story kind where the anchor of comment `id` starts.
fn anchor_kind(doc: &Document, id: i32) -> StoryKind {
    doc.comment_anchor(id)
        .expect("anchor reads")
        .expect("comment has a range")
        .start
        .location
        .story()
        .kind()
}

/// The comment ids marked in the body paragraphs (range starts, range ends
/// and references), in document order.
fn comment_marks(doc: &Document) -> Vec<i32> {
    let mut marks = Vec::new();
    for paragraph in doc.paragraphs() {
        for item in paragraph.items() {
            match item {
                ParagraphItemRef::CommentRangeStart { id, .. }
                | ParagraphItemRef::CommentRangeEnd { id, .. } => marks.push(id),
                ParagraphItemRef::Run(run) => {
                    marks.extend(run.items().filter_map(|item| match item {
                        RunItemRef::CommentReference(id) => Some(id),
                        _ => None,
                    }))
                }
                _ => {}
            }
        }
    }
    marks
}

/// One part of a corpus file as its producer wrote it.
fn corpus_part(name: &str, part: &str) -> Option<Vec<u8>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files")
        .join(name);
    let bytes = fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let package = OpcPackage::from_reader(Cursor::new(bytes)).expect("corpus package opens");
    package.get_part(part).map(<[u8]>::to_vec)
}

/// "Before", a 2 x 2 table (north, south / east, west), "After".
fn table_doc() -> Document {
    let mut doc = doc_with(&["Before"]);
    let mut table = doc.add_table(2, 2);
    for (row, col, text) in [
        (0, 0, "north"),
        (0, 1, "south"),
        (1, 0, "east"),
        (1, 1, "west"),
    ] {
        table.cell(row, col).expect("cell").set_text(text);
    }
    doc.add_paragraph("After");
    doc
}

#[test]
fn author_initials_and_date_read_back_as_given() {
    let mut doc = doc_with(&["Hello brave world"]);
    let full = doc
        .add_comment_on_text(
            "brave",
            "Ada Lovelace",
            "Which one?",
            TextCommentOptions {
                initials: Some("AL"),
                date: CommentDate::At("2026-11-20T10:00:00Z"),
                ..Default::default()
            },
        )
        .unwrap();
    let bare = doc
        .add_comment_on_text(
            "world",
            "",
            "No initials, no date",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();

    let doc = reopen(&mut doc);
    let full = comment(&doc, full);
    assert_eq!(full.author(), Some("Ada Lovelace"));
    assert_eq!(full.initials(), Some("AL"));
    assert_eq!(full.date(), Some("2026-11-20T10:00:00Z"));
    assert_eq!(full.text(), "Which one?");
    assert_eq!(full.anchor_text().unwrap().as_deref(), Some("brave"));
    assert_eq!(full.parent_id(), None);
    assert_eq!(full.resolved(), Some(false));
    let bare = comment(&doc, bare);
    assert_eq!(bare.author(), Some(""));
    assert_eq!(bare.initials(), None);
    assert_eq!(bare.date(), None);
    assert_eq!(texts(&doc), ["Hello brave world"]);
}

#[test]
fn a_comment_on_text_after_a_tab_leaves_the_tab_out_of_its_range() {
    let mut doc =
        doc_with_body(r#"<w:p><w:r><w:t>Name:</w:t><w:tab/><w:t>John Smith</w:t></w:r></w:p>"#);
    let id = doc
        .add_comment_on_text(
            "John Smith",
            "Ada",
            "Who?",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();

    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Name:\tJohn Smith"]);
    assert_eq!(
        doc.comment_anchor_text(id).unwrap().as_deref(),
        Some("John Smith")
    );
}

#[test]
fn each_line_of_a_comment_becomes_one_of_its_paragraphs() {
    let mut doc = doc_with(&["Hello brave world"]);
    let id = doc
        .add_comment_on_text(
            "brave",
            "Ada",
            "  padded\tline  \r\nsecond\n\nfourth",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();

    let doc = reopen(&mut doc);
    // A CRLF ends a line like a LF, and spaces and tabs are kept as typed.
    assert_eq!(
        comment(&doc, id).text(),
        "  padded\tline  \nsecond\n\nfourth"
    );
    assert_eq!(comment_paragraphs(&doc, id).len(), 4);
}

#[test]
fn commenting_text_splits_the_runs_and_keeps_their_format() {
    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("");
    paragraph.add_run("Hel").set_bold(true);
    paragraph.add_run("lo wor");
    paragraph.add_run("ld").set_italic(true);
    let id = doc
        .add_comment_on_text(
            "llo wo",
            "Ada",
            "Split me",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();

    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Hello world"]);
    assert_eq!(
        doc.comment_anchor_text(id).unwrap().as_deref(),
        Some("llo wo")
    );
    let runs: Vec<(String, bool, bool)> = doc
        .paragraph(0)
        .unwrap()
        .runs()
        .filter(|run| !run.text().is_empty())
        .map(|run| (run.text(), run.is_bold(), run.is_italic()))
        .collect();
    let expected = [
        ("He", true, false),
        ("l", true, false),
        ("lo wo", false, false),
        ("r", false, false),
        ("ld", false, true),
    ]
    .map(|(text, bold, italic)| (text.to_owned(), bold, italic));
    assert_eq!(runs, expected);
}

#[test]
fn occurrences_are_counted_in_document_order_through_tables() {
    let mut doc = doc_with(&["word before"]);
    let mut table = doc.add_table(1, 2);
    table.cell(0, 0).unwrap().set_text("word in a");
    table.cell(0, 1).unwrap().set_text("word in b");
    doc.add_paragraph("word after");

    let ids: Vec<i32> = (0..4)
        .map(|occurrence| {
            doc.add_comment_on_text(
                "word",
                "Ada",
                "Here",
                TextCommentOptions {
                    occurrence,
                    date: CommentDate::Undated,
                    ..Default::default()
                },
            )
            .unwrap()
        })
        .collect();

    let doc = reopen(&mut doc);
    let kinds: Vec<StoryKind> = ids.iter().map(|&id| anchor_kind(&doc, id)).collect();
    assert_eq!(
        kinds,
        [
            StoryKind::Body,
            StoryKind::TableCell,
            StoryKind::TableCell,
            StoryKind::Body
        ]
    );
    let cells: Vec<String> = ids[1..3]
        .iter()
        .map(|&id| {
            let anchor = doc.comment_anchor(id).unwrap().unwrap();
            doc.story_items(anchor.start.location.story()).unwrap()[0]
                .text()
                .unwrap()
                .unwrap()
        })
        .collect();
    assert_eq!(cells, ["word in a", "word in b"]);
}

#[test]
fn a_missing_occurrence_is_refused_and_says_how_many_there_are() {
    let mut doc = doc_with(&["text and text", "aaa"]);
    let before = doc.to_bytes().unwrap();

    let error = doc
        .add_comment_on_text(
            "text",
            "Ada",
            "Third?",
            TextCommentOptions {
                occurrence: 2,
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("it occurs 2 times"), "{error}");
    // Matches do not overlap: "aaa" holds "aa" once.
    let error = doc
        .add_comment_on_text(
            "aa",
            "Ada",
            "Second?",
            TextCommentOptions {
                occurrence: 1,
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("it occurs 1 time "), "{error}");

    assert!(doc.comments().is_empty());
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn anchors_the_main_story_search_cannot_find_are_refused() {
    let mut doc = doc_with(&["end of one", "start of two"]);
    doc.set_header("Header words");
    let before = doc.to_bytes().unwrap();

    let empty = doc
        .add_comment_on_text(
            "",
            "Ada",
            "Nothing",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(empty.to_string().contains("must not be empty"), "{empty}");
    // One match never spans two paragraphs.
    assert!(
        doc.add_comment_on_text(
            "one start",
            "Ada",
            "Across",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            }
        )
        .is_err()
    );
    // Headers, footers and notes are not searched: comment them by range.
    let header = doc
        .add_comment_on_text(
            "Header",
            "Ada",
            "In the header",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(header.to_string().contains("in the main story"), "{header}");
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn a_run_range_comment_covers_exactly_its_runs() {
    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("");
    paragraph.add_run("left ");
    paragraph.add_run("middle ");
    paragraph.add_run("right");
    doc.add_paragraph("second");
    doc.add_paragraph("third");

    let middle = doc
        .add_comment(run_range(0, 1, 0, 2), "Ada", None, "Middle")
        .unwrap();
    let span = doc
        .add_comment_with_date(
            run_range(0, 2, 2, 1),
            "Ben",
            Some("BN"),
            "To the end",
            CommentDate::At("2026-11-20T10:00:00Z"),
        )
        .unwrap();
    let point = doc
        .add_comment(run_range(1, 1, 1, 1), "Cy", None, "Here")
        .unwrap();

    let doc = reopen(&mut doc);
    let anchor = |id| doc.comment_anchor_text(id).unwrap();
    assert_eq!(anchor(middle).as_deref(), Some("middle "));
    // One line per paragraph the range crosses.
    assert_eq!(anchor(span).as_deref(), Some("right\nsecond\nthird"));
    // A point comment sits between two runs and covers no text.
    assert_eq!(anchor(point).as_deref(), Some(""));
    assert!(doc.comment_anchor(point).unwrap().is_some());
    assert_eq!(texts(&doc), ["left middle right", "second", "third"]);
}

#[test]
fn a_run_range_outside_its_paragraph_is_refused_without_a_change() {
    let mut doc = doc_with(&["only run"]);
    doc.add_table(1, 1);
    let before = doc.to_bytes().unwrap();

    let refusals = [
        (run_range(0, 0, 0, 2), "exceeds paragraph run count 1"),
        (run_range(0, 1, 0, 0), "start must not follow its end"),
        (run_range(1, 0, 1, 1), "is not a paragraph"),
    ];
    for (range, reason) in refusals {
        let error = doc
            .add_comment(range, "Ada", None, "Nowhere")
            .unwrap_err()
            .to_string();
        assert!(error.contains(reason), "{error}");
    }
    assert!(doc.comments().is_empty());
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn unicode_rtl_cjk_markup_and_long_texts_survive_a_save() {
    let mut doc = doc_with(&["你好世界 and مرحبا بالعالم, R&D <team> 👩‍💻"]);
    let long = "Review this clause again. ".repeat(800);
    let cases = [
        ("世界", "Zoë Ådahl", Some("ZÅ"), "注释 🎉".to_owned()),
        ("بالعالم", "علي", None, "تعليق".to_owned()),
        (
            "R&D <team>",
            "<A&B>",
            None,
            "Check \"quotes\" & <tags>".to_owned(),
        ),
        ("👩‍💻", "Ada", None, long),
    ];
    let ids: Vec<i32> = cases
        .iter()
        .map(|(anchor, author, initials, text)| {
            doc.add_comment_on_text(
                anchor,
                author,
                text,
                TextCommentOptions {
                    initials: *initials,
                    date: CommentDate::Undated,
                    ..Default::default()
                },
            )
            .unwrap()
        })
        .collect();

    let doc = reopen(&mut doc);
    for (id, (anchor, author, initials, text)) in ids.into_iter().zip(&cases) {
        let comment = comment(&doc, id);
        assert_eq!(comment.anchor_text().unwrap().as_deref(), Some(*anchor));
        assert_eq!(comment.author(), Some(*author));
        assert_eq!(comment.initials(), *initials);
        assert_eq!(&comment.text(), text);
    }
    assert_eq!(texts(&doc), ["你好世界 and مرحبا بالعالم, R&D <team> 👩‍💻"]);
}

#[test]
fn characters_xml_cannot_carry_are_refused() {
    let mut doc = doc_with(&["text"]);
    let error = doc
        .add_comment_on_text(
            "text",
            "Ada",
            "bell\u{7}",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("U+0007"), "{error}");
    assert!(
        doc.add_comment_on_text(
            "text",
            "Ada\u{1}",
            "Fine",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(doc.comments().is_empty());
}

#[test]
fn a_date_is_kept_as_written_and_a_date_that_is_not_rfc_3339_is_refused() {
    let mut doc = doc_with(&["text"]);
    for date in ["2026-02-30", "yesterday", "2026-11-20 10:00"] {
        let error = doc
            .add_comment_on_text(
                "text",
                "Ada",
                "Dated",
                TextCommentOptions {
                    date: CommentDate::At(date),
                    ..Default::default()
                },
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid RFC 3339"), "{date}: {error}");
    }
    let id = doc
        .add_comment_on_text(
            "text",
            "Ada",
            "Dated",
            TextCommentOptions {
                date: CommentDate::At("2026-09-16T11:00:00+01:00"),
                ..Default::default()
            },
        )
        .unwrap();
    let reply = doc
        .reply_to_with_date(
            id,
            "Ben",
            "Later",
            CommentDate::At("2026-09-16T12:30:15.250Z"),
        )
        .unwrap();

    let doc = reopen(&mut doc);
    assert_eq!(comment(&doc, id).date(), Some("2026-09-16T11:00:00+01:00"));
    assert_eq!(
        comment(&doc, reply).date(),
        Some("2026-09-16T12:30:15.250Z")
    );
}

#[test]
fn comments_get_distinct_ids_and_are_listed_in_the_order_they_were_added() {
    let mut doc = doc_with(&["First paragraph", "Second paragraph"]);
    let second = doc
        .add_comment_on_text(
            "Second",
            "Ada",
            "On the second",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
    let first = doc
        .add_comment_on_text(
            "First",
            "Ben",
            "On the first",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
    let reply = doc.reply_to(second, "Cy", "Reply").unwrap();

    let doc = reopen(&mut doc);
    let listed: Vec<(i32, String)> = doc
        .comments()
        .iter()
        .map(|comment| (comment.id(), comment.text()))
        .collect();
    assert_eq!(
        listed,
        [
            (second, "On the second".to_owned()),
            (first, "On the first".to_owned()),
            (reply, "Reply".to_owned()),
        ]
    );
    assert!(second != first && first != reply && second != reply);
}

#[test]
fn the_comment_parts_come_with_the_first_comment_and_go_with_the_last() {
    let mut doc = doc_with(&["review this"]);
    let has_part =
        |doc: &Document, part: &str| doc.part_names().unwrap().iter().any(|name| name == part);
    assert!(!has_part(&doc, "/word/comments.xml"));

    let id = doc
        .add_comment_on_text(
            "review",
            "Ada",
            "Please review",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(has_part(&doc, "/word/comments.xml"));
    assert!(has_part(&doc, "/word/commentsExtended.xml"));
    let types = doc.part_xml("[Content_Types].xml").unwrap();
    assert!(types.contains("wordprocessingml.comments+xml"), "{types}");
    assert!(
        types.contains("wordprocessingml.commentsExtended+xml"),
        "{types}"
    );
    let relationships = doc.part_xml("word/_rels/document.xml.rels").unwrap();
    assert!(
        relationships.contains("relationships/comments\""),
        "{relationships}"
    );
    assert!(
        relationships.contains("relationships/commentsExtended\""),
        "{relationships}"
    );
    let style_type = |id: &str| doc.style(id).map(|style| style.style_type());
    assert_eq!(style_type("CommentText"), Some(StyleType::Paragraph));
    assert_eq!(style_type("CommentReference"), Some(StyleType::Character));

    assert!(doc.remove_comment(id).unwrap());
    assert!(!has_part(&doc, "/word/comments.xml"));
    assert!(!has_part(&doc, "/word/commentsExtended.xml"));
}

#[test]
fn a_reply_is_linked_to_its_parent_and_covers_the_same_text() {
    let mut doc = doc_with(&["The deadline is thirty days."]);
    let root = doc
        .add_comment_on_text(
            "thirty days",
            "Ada",
            "Too short?",
            TextCommentOptions {
                initials: Some("AL"),
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
    let reply = doc
        .reply_to_with_initials(
            root,
            "Ben",
            Some("BN"),
            "Agreed.\nMake it sixty.",
            CommentDate::At("2026-11-21T09:30:00Z"),
        )
        .unwrap();

    let doc = reopen(&mut doc);
    let reply = comment(&doc, reply);
    assert_eq!(reply.parent_id(), Some(root));
    assert_eq!(reply.author(), Some("Ben"));
    assert_eq!(reply.initials(), Some("BN"));
    assert_eq!(reply.date(), Some("2026-11-21T09:30:00Z"));
    assert_eq!(reply.text(), "Agreed.\nMake it sixty.");
    assert_eq!(reply.anchor_text().unwrap().as_deref(), Some("thirty days"));
    assert_eq!(comment(&doc, root).parent_id(), None);
    assert_eq!(texts(&doc), ["The deadline is thirty days."]);
}

#[test]
fn a_thread_keeps_every_reply_under_its_own_parent() {
    let mut doc = doc_with(&["Hello brave world"]);
    let root = doc
        .add_comment_on_text(
            "brave",
            "Ada",
            "root",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
    let first = doc.reply_to(root, "Ben", "first reply").unwrap();
    let second = doc.reply_to(root, "Cy", "second reply").unwrap();
    let nested = doc.reply_to(first, "Di", "reply to the first").unwrap();

    let doc = reopen(&mut doc);
    let parents: Vec<(i32, Option<i32>)> = doc
        .comments()
        .iter()
        .map(|comment| (comment.id(), comment.parent_id()))
        .collect();
    assert_eq!(
        parents,
        [
            (root, None),
            (first, Some(root)),
            (second, Some(root)),
            (nested, Some(first)),
        ]
    );
}

#[test]
fn unknown_comment_ids_are_refused_or_reported_without_a_change() {
    let mut doc = doc_with(&["text"]);
    doc.add_comment_on_text(
        "text",
        "Ada",
        "Known",
        TextCommentOptions {
            date: CommentDate::Undated,
            ..Default::default()
        },
    )
    .unwrap();
    let before = doc.to_bytes().unwrap();

    let reply = doc.reply_to(99, "Ben", "To nobody").unwrap_err();
    assert!(reply.to_string().contains("99 does not exist"), "{reply}");
    assert!(!doc.resolve_comment(99, true).unwrap());
    assert!(!doc.remove_comment(99).unwrap());
    let moved = doc.move_comment_to_text(99, "text", 0).unwrap_err();
    assert!(moved.to_string().contains("unknown comment 99"), "{moved}");
    assert!(doc.comment_anchor_text(99).is_err());
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn resolving_a_thread_marks_it_done_and_reopening_it_clears_the_mark() {
    let mut doc = doc_with(&["Hello brave world"]);
    let root = doc
        .add_comment_on_text(
            "brave",
            "Ada",
            "Please review",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
    doc.reply_to(root, "Ben", "Done").unwrap();

    assert!(doc.resolve_comment(root, true).unwrap());
    let mut resolved = reopen(&mut doc);
    assert_eq!(comment(&resolved, root).resolved(), Some(true));

    assert!(resolved.resolve_comment(root, false).unwrap());
    let reopened = reopen(&mut resolved);
    assert_eq!(comment(&reopened, root).resolved(), Some(false));
    assert_eq!(reopened.comments().len(), 2);
}

#[test]
fn resolving_a_reply_resolves_its_thread() {
    let mut doc = doc_with(&["Hello brave world"]);
    let root = doc
        .add_comment_on_text(
            "brave",
            "Ada",
            "root",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
    let reply = doc.reply_to(root, "Ben", "reply").unwrap();

    assert!(doc.resolve_comment(reply, true).unwrap());
    let doc = reopen(&mut doc);
    assert_eq!(comment(&doc, root).resolved(), Some(true));
}

#[test]
fn a_comment_or_reply_text_is_edited_through_its_story_paragraphs() {
    let mut doc = doc_with(&["Hello brave world"]);
    let root = doc
        .add_comment_on_text(
            "brave",
            "Ada",
            "first line\nsecond line",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
    let reply = doc.reply_to(root, "Ben", "reply").unwrap();

    let root_paragraphs = comment_paragraphs(&doc, root);
    doc.set_story_text(&root_paragraphs[1], "second line, edited")
        .unwrap();
    let reply_paragraphs = comment_paragraphs(&doc, reply);
    doc.set_story_text(&reply_paragraphs[0], "reply, edited")
        .unwrap();

    let doc = reopen(&mut doc);
    let root_comment = comment(&doc, root);
    assert_eq!(root_comment.text(), "first line\nsecond line, edited");
    assert_eq!(root_comment.author(), Some("Ada"));
    let reply = comment(&doc, reply);
    assert_eq!(reply.text(), "reply, edited");
    // The edit kept the paragraph that links the reply to its parent.
    assert_eq!(reply.parent_id(), Some(root));
    assert_eq!(reply.anchor_text().unwrap().as_deref(), Some("brave"));
}

#[test]
fn removing_a_comment_keeps_the_text_it_covered() {
    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("");
    paragraph.add_run("left ");
    paragraph.add_run("middle ");
    paragraph.add_run("right");
    // Right to left: each comment adds its reference run after its range,
    // which shifts the run indexes that follow.
    let kept = doc
        .add_comment(run_range(0, 2, 0, 3), "Ben", None, "keep me")
        .unwrap();
    let removed = doc
        .add_comment(run_range(0, 0, 0, 1), "Ada", None, "remove me")
        .unwrap();

    assert!(doc.remove_comment(removed).unwrap());
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["left middle right"]);
    assert_eq!(doc.comments().len(), 1);
    assert_eq!(
        comment(&doc, kept).anchor_text().unwrap().as_deref(),
        Some("right")
    );
    assert_eq!(comment_marks(&doc), [kept, kept, kept]);
}

#[test]
fn removing_a_comment_removes_the_replies_under_it() {
    let mut doc = doc_with(&["Hello brave world"]);
    let root = doc
        .add_comment_on_text(
            "brave",
            "Ada",
            "root",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
    let first = doc.reply_to(root, "Ben", "first reply").unwrap();
    let second = doc.reply_to(root, "Cy", "second reply").unwrap();
    doc.reply_to(first, "Di", "reply to the first").unwrap();

    assert!(doc.remove_comment(first).unwrap());
    let mut doc = reopen(&mut doc);
    let left: Vec<i32> = doc.comments().iter().map(CommentRef::id).collect();
    assert_eq!(left, [root, second]);
    assert_eq!(comment(&doc, second).parent_id(), Some(root));

    assert!(doc.remove_comment(root).unwrap());
    let doc = reopen(&mut doc);
    assert!(doc.comments().is_empty());
    assert!(comment_marks(&doc).is_empty());
    assert_eq!(texts(&doc), ["Hello brave world"]);
}

#[test]
fn deleting_commented_content_takes_whole_threads_and_refuses_half_ones() {
    let mut doc = doc_with(&["keep", "commented", "first", "second"]);
    let whole = doc
        .add_comment_on_text(
            "commented",
            "Ada",
            "Whole paragraph",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
    doc.reply_to(whole, "Ben", "Reply").unwrap();
    let span = doc
        .add_comment(run_range(2, 0, 3, 1), "Cy", None, "Two paragraphs")
        .unwrap();

    assert!(doc.try_remove_content(1).unwrap());
    let ids: Vec<i32> = doc.comments().iter().map(CommentRef::id).collect();
    assert_eq!(ids, [span]);

    // "first" holds only the start of the span comment.
    let error = doc.try_remove_content(1).unwrap_err();
    assert!(error.to_string().contains("part of comment"), "{error}");
    // The run after "second" holds the reference of the span comment.
    let error = doc.paragraph_mut(2).unwrap().remove_run(1).unwrap_err();
    assert!(error.to_string().contains("comment"), "{error}");

    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["keep", "first", "second"]);
    assert_eq!(
        doc.comment_anchor_text(span).unwrap().as_deref(),
        Some("first\nsecond")
    );
}

#[test]
fn removing_an_inline_control_takes_whole_threads_and_refuses_half_ones() {
    // The control holds the whole thread, or only its start and its text.
    for kind in [StoryKind::Body, StoryKind::Header] {
        for whole in [true, false] {
            let mut doc = doc_with(&["Body"]);
            doc.set_header("Header words");
            let paragraph = story_paragraph(&doc, kind, 0);
            let part = doc
                .stories()
                .unwrap()
                .into_iter()
                .find(|story| story.kind() == kind)
                .unwrap()
                .part_name()
                .trim_start_matches('/')
                .to_owned();
            if kind == StoryKind::Body {
                doc.add_comment(run_range(0, 0, 0, 1), "Ada", None, "Thread")
                    .unwrap();
            } else {
                doc.add_story_comment(story_range(&paragraph, 0, 1), "Ada", None, "Thread")
                    .unwrap();
            }
            let bytes = doc.to_bytes().unwrap();
            let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
            let name = format!("/{part}");
            let xml = String::from_utf8(package.get_part(&name).unwrap().to_vec()).unwrap();
            let start = xml.find("<w:commentRangeStart").unwrap();
            let end = if whole {
                let reference = xml.find("<w:commentReference").unwrap();
                reference + xml[reference..].find("</w:r>").unwrap() + "</w:r>".len()
            } else {
                xml.find("<w:commentRangeEnd").unwrap()
            };
            let xml = format!(
                "{}<w:sdt><w:sdtContent>{}</w:sdtContent></w:sdt>{}",
                &xml[..start],
                &xml[start..end],
                &xml[end..]
            );
            package.set_part(&name, xml.into_bytes());
            let mut out = Cursor::new(Vec::new());
            package.write_to(&mut out).unwrap();
            let mut doc = Document::from_bytes(&out.into_inner()).unwrap();
            let story = story_paragraph(&doc, kind, 0).story().clone();
            let control = doc
                .story_items(&story)
                .unwrap()
                .into_iter()
                .find(|item| item.kind() == StoryItemKind::ContentControl)
                .expect("the control")
                .location()
                .clone();
            let result = doc.remove_content_control(&control);
            if whole {
                result.unwrap();
                let doc = reopen(&mut doc);
                assert!(doc.comments().is_empty(), "{kind:?}");
                assert!(
                    !doc.part_xml(&part).unwrap().contains("w:comment"),
                    "{kind:?}"
                );
            } else {
                let error = result.unwrap_err();
                assert!(
                    error.to_string().contains("part of comment"),
                    "{kind:?}: {error}"
                );
                assert_eq!(doc.comments().len(), 1, "{kind:?}");
            }
        }
    }
}

#[test]
fn moving_a_comment_to_other_text_brings_its_replies() {
    let mut doc = doc_with(&["alpha beta", "gamma delta"]);
    let root = doc
        .add_comment_on_text(
            "beta",
            "Ada",
            "root",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
    let reply = doc.reply_to(root, "Ben", "reply").unwrap();

    doc.move_comment_to_text(root, "delta", 0).unwrap();
    let error = doc.move_comment_to_text(reply, "alpha", 0).unwrap_err();
    assert!(error.to_string().contains("is a reply"), "{error}");

    let doc = reopen(&mut doc);
    for id in [root, reply] {
        assert_eq!(
            doc.comment_anchor_text(id).unwrap().as_deref(),
            Some("delta")
        );
    }
    assert_eq!(comment(&doc, reply).parent_id(), Some(root));
    assert_eq!(comment(&doc, root).text(), "root");
    assert_eq!(texts(&doc), ["alpha beta", "gamma delta"]);
}

#[test]
fn editing_the_commented_text_keeps_the_comment_on_it() {
    let mut doc = doc_with(&["Hello brave world", "Other"]);
    let id = doc
        .add_comment_on_text(
            "brave",
            "Ada",
            "Which one?",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();

    assert_eq!(doc.try_replace_text("brave", "bold").unwrap(), 1);
    assert_eq!(
        doc.comment_anchor_text(id).unwrap().as_deref(),
        Some("bold")
    );
    // Setting the paragraph text moves the range around the new text.
    doc.paragraph_mut(0)
        .unwrap()
        .set_text("Entirely new")
        .unwrap();

    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Entirely new", "Other"]);
    assert_eq!(
        doc.comment_anchor_text(id).unwrap().as_deref(),
        Some("Entirely new")
    );
    assert_eq!(comment(&doc, id).text(), "Which one?");
}

#[test]
fn a_comment_in_a_table_cell_reads_back_its_cell_and_goes_with_its_row() {
    let mut doc = table_doc();
    let south = doc
        .add_comment_on_text(
            "south",
            "Ada",
            "On text in a cell",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
    // The fourth cell, west, commented by range over its one run.
    let west_cell = story_paragraph(&doc, StoryKind::TableCell, 3);
    let west = doc
        .add_story_comment(story_range(&west_cell, 0, 1), "Ben", None, "Whole cell")
        .unwrap();

    let mut doc = reopen(&mut doc);
    for (id, text) in [(south, "south"), (west, "west")] {
        assert_eq!(anchor_kind(&doc, id), StoryKind::TableCell);
        assert_eq!(doc.comment_anchor_text(id).unwrap().as_deref(), Some(text));
    }

    assert!(doc.remove_table_row(0, 1).unwrap());
    let doc = reopen(&mut doc);
    let ids: Vec<i32> = doc.comments().iter().map(CommentRef::id).collect();
    assert_eq!(ids, [south]);
}

#[test]
fn a_comment_in_a_header_holds_a_whole_thread() {
    let mut doc = doc_with(&["Body"]);
    doc.set_header("Header words");
    let header = story_paragraph(&doc, StoryKind::Header, 0);
    let root = doc
        .add_story_comment_with_date(
            story_range(&header, 0, 1),
            "Ada",
            Some("AL"),
            "On the header",
            CommentDate::At("2026-11-20T10:00:00Z"),
        )
        .unwrap();
    let reply = doc.reply_to(root, "Ben", "Reply").unwrap();
    assert!(doc.resolve_comment(root, true).unwrap());

    let mut doc = reopen(&mut doc);
    assert_eq!(anchor_kind(&doc, root), StoryKind::Header);
    assert_eq!(
        doc.comment_anchor_text(root).unwrap().as_deref(),
        Some("Header words")
    );
    assert_eq!(comment(&doc, reply).parent_id(), Some(root));
    assert_eq!(comment(&doc, root).resolved(), Some(true));

    assert!(doc.remove_comment(root).unwrap());
    let doc = reopen(&mut doc);
    assert!(doc.comments().is_empty());
    let header = story_paragraph(&doc, StoryKind::Header, 0);
    let story = doc.story_items(header.story()).unwrap();
    assert_eq!(story[0].text().unwrap().as_deref(), Some("Header words"));
    assert!(
        !doc.part_xml("word/header1.xml")
            .unwrap()
            .contains("comment")
    );
}

#[test]
fn a_commented_document_saves_the_same_bytes_every_time() {
    let mut doc = doc_with(&["Hello brave world"]);
    let root = doc
        .add_comment_on_text(
            "brave",
            "Ada",
            "Which one?",
            TextCommentOptions {
                initials: Some("AL"),
                date: CommentDate::At("2026-11-20T10:00:00Z"),
                ..Default::default()
            },
        )
        .unwrap();
    doc.reply_to(root, "Ben", "This one").unwrap();
    doc.resolve_comment(root, true).unwrap();

    let first = doc.to_bytes().unwrap();
    assert_eq!(doc.to_bytes().unwrap(), first);
    let mut reopened = Document::from_bytes(&first).unwrap();
    assert_eq!(reopened.to_bytes().unwrap(), first);
}

#[test]
fn every_producer_reports_the_corpus_comment_as_it_saved_it() {
    // Word truncates the time to the minute, Google writes no initials, and
    // only Word and Google write the commentsExtended part that holds the
    // done state.
    let expected = [
        (
            "word/report.docx",
            Some("RV"),
            "2026-11-20T21:51:00Z",
            Some(false),
        ),
        (
            "google/report.docx",
            None,
            "2026-11-20T21:51:59Z",
            Some(false),
        ),
        (
            "libreoffice/report.docx",
            Some("RV"),
            "2026-11-20T21:51:59Z",
            None,
        ),
        (
            "authored/report.docx",
            Some("RV"),
            "2026-11-20T21:51:59Z",
            None,
        ),
    ];
    for (name, initials, date, resolved) in expected {
        let doc = corpus_doc(name);
        let comments = doc.comments();
        assert_eq!(comments.len(), 1, "{name}");
        let comment = comments[0];
        assert_eq!(comment.id(), 0, "{name}");
        assert_eq!(comment.author(), Some("Reviewer"), "{name}");
        assert_eq!(comment.initials(), initials, "{name}");
        assert_eq!(comment.date(), Some(date), "{name}");
        assert_eq!(comment.text(), CORPUS_TEXT, "{name}");
        assert_eq!(comment.parent_id(), None, "{name}");
        assert_eq!(comment.resolved(), resolved, "{name}");
        assert_eq!(
            comment.anchor_text().unwrap().as_deref(),
            Some(CORPUS_ANCHOR),
            "{name}"
        );
    }
}

#[test]
fn the_corpus_comment_of_every_producer_takes_a_reply_and_a_resolution() {
    for name in PRODUCERS {
        let mut doc = corpus_doc(name);
        let reply = doc
            .reply_to_with_initials(
                0,
                "Ben",
                Some("BN"),
                "Finance agreed.",
                CommentDate::At("2026-11-21T09:00:00Z"),
            )
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(doc.resolve_comment(0, true).unwrap(), "{name}");

        let doc = reopen(&mut doc);
        let root = comment(&doc, 0);
        assert_eq!(root.text(), CORPUS_TEXT, "{name}");
        // LibreOffice and python-docx wrote no done state: resolving adds it.
        assert_eq!(root.resolved(), Some(true), "{name}");
        let reply = comment(&doc, reply);
        assert_eq!(reply.parent_id(), Some(0), "{name}");
        assert_eq!(reply.initials(), Some("BN"), "{name}");
        assert_eq!(reply.text(), "Finance agreed.", "{name}");
        assert_eq!(
            reply.anchor_text().unwrap().as_deref(),
            Some(CORPUS_ANCHOR),
            "{name}"
        );
    }
}

#[test]
fn editing_the_word_comment_text_keeps_its_done_state_and_anchor() {
    let mut doc = corpus_doc("word/report.docx");
    let paragraphs = comment_paragraphs(&doc, 0);
    assert_eq!(paragraphs.len(), 1);
    doc.set_story_text(&paragraphs[0], "Confirmed with finance.")
        .unwrap();

    let doc = reopen(&mut doc);
    let edited = comment(&doc, 0);
    assert_eq!(edited.text(), "Confirmed with finance.");
    assert_eq!(edited.author(), Some("Reviewer"));
    assert_eq!(edited.date(), Some("2026-11-20T21:51:00Z"));
    // The done state is keyed by the paragraph id the edit kept.
    assert_eq!(edited.resolved(), Some(false));
    assert_eq!(
        edited.anchor_text().unwrap().as_deref(),
        Some(CORPUS_ANCHOR)
    );
}

#[test]
fn removing_the_corpus_comment_also_clears_its_word_and_google_companions() {
    let companions = [
        ("word/commentsExtended.xml", "<w15:commentEx "),
        ("word/commentsIds.xml", "<w16cid:commentId "),
        ("word/commentsExtensible.xml", "<w16cex:commentExtensible "),
    ];
    for name in ["word/report.docx", "google/report.docx"] {
        let mut doc = corpus_doc(name);
        let text = doc.text();
        assert!(doc.remove_comment(0).unwrap(), "{name}");

        let doc = reopen(&mut doc);
        assert!(doc.comments().is_empty(), "{name}");
        assert!(comment_marks(&doc).is_empty(), "{name}");
        assert_eq!(doc.text(), text, "{name}");
        for (part, entry) in companions {
            // Word wrote no commentsExtensible part; Google wrote all three.
            if let Ok(xml) = doc.part_xml(part) {
                assert!(!xml.contains(entry), "{name}: {part} keeps {entry}");
            }
        }
    }
}

#[test]
fn commenting_across_producer_runs_keeps_their_format_and_the_existing_comment() {
    for name in ["word/report.docx", "google/report.docx"] {
        let mut doc = corpus_doc(name);
        // "6 h 40" and "3 h 15" are bold runs between plain ones.
        let id = doc
            .add_comment_on_text(
                "from 6 h 40 to 3",
                "Ada",
                "Check",
                TextCommentOptions {
                    initials: Some("AL"),
                    date: CommentDate::Undated,
                    ..Default::default()
                },
            )
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_ne!(id, 0, "{name}");

        let doc = reopen(&mut doc);
        let index = texts(&doc)
            .iter()
            .position(|text| text.starts_with("Median first response time"))
            .expect("the support paragraph");
        let paragraph = doc.paragraph(index).unwrap();
        assert_eq!(
            paragraph.text(),
            "Median first response time went from 6 h 40 to 3 h 15. \
             The team handled 18,400 tickets, 9 % more than in Q2.",
            "{name}"
        );
        let bold: String = paragraph
            .runs()
            .filter(|run| run.is_bold())
            .map(|run| run.text())
            .collect();
        assert_eq!(bold, "6 h 403 h 15", "{name}");
        assert_eq!(
            doc.comment_anchor_text(id).unwrap().as_deref(),
            Some("from 6 h 40 to 3"),
            "{name}"
        );
        assert_eq!(
            doc.comment_anchor_text(0).unwrap().as_deref(),
            Some(CORPUS_ANCHOR),
            "{name}"
        );
        assert_eq!(comment(&doc, 0).text(), CORPUS_TEXT, "{name}");
    }
}

#[test]
fn a_body_edit_keeps_the_producer_comment_parts_byte_for_byte() {
    let parts = [
        "/word/comments.xml",
        "/word/commentsExtended.xml",
        "/word/commentsIds.xml",
        "/word/commentsExtensible.xml",
    ];
    for name in ["word/report.docx", "google/report.docx"] {
        let mut doc = corpus_doc(name);
        doc.add_paragraph("Added after the review.");
        let saved = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
        for part in parts {
            assert_eq!(
                saved.get_part(part).map(<[u8]>::to_vec),
                corpus_part(name, part),
                "{name}: {part}"
            );
        }
    }
}

#[test]
fn a_comment_added_to_a_localized_word_file_uses_its_comment_styles() {
    let mut doc = corpus_doc("word/report.docx");
    let names = |doc: &Document| -> Vec<(String, Option<String>)> {
        doc.styles()
            .iter()
            .map(|style| (style.style_id().to_owned(), style.name().map(str::to_owned)))
            .collect()
    };
    let before = names(&doc);
    assert!(before.contains(&("Commentaire".to_owned(), Some("annotation text".to_owned()))));

    doc.add_comment_on_text(
        "January",
        "Ada",
        "Check the date",
        TextCommentOptions {
            date: CommentDate::Undated,
            ..Default::default()
        },
    )
    .unwrap();
    let doc = reopen(&mut doc);
    let added: Vec<_> = names(&doc)
        .into_iter()
        .filter(|style| !before.contains(style))
        .collect();
    assert!(
        added.is_empty(),
        "styles added beside Word's comment styles: {added:?}"
    );
    // And the new comment takes Word's own styles. Word marks a comment
    // with "Marquedecommentaire" (once in the untouched report, on the
    // reference run of the body); every style the comment parts name exists.
    let ids: Vec<String> = names(&doc).into_iter().map(|(id, _)| id).collect();
    let mut marks = 0;
    for part in ["word/comments.xml", "word/document.xml"] {
        let xml = doc.part_xml(part).unwrap();
        marks += xml.matches("w:val=\"Marquedecommentaire\"").count();
        for tag in ["<w:pStyle w:val=\"", "<w:rStyle w:val=\""] {
            for named in xml.split(tag).skip(1) {
                let id = &named[..named.find('"').unwrap()];
                assert!(
                    ids.iter().any(|known| known == id),
                    "{part} names the undefined style {id}"
                );
            }
        }
    }
    assert!(
        marks >= 2,
        "the new comment is marked with Word's comment mark style"
    );
}

#[test]
fn a_comment_added_to_any_corpus_report_takes_word_built_in_comment_styles() {
    for name in [
        "word/report.docx",
        "google/report.docx",
        "libreoffice/report.docx",
        "authored/report.docx",
    ] {
        let mut doc = corpus_doc(name);
        doc.add_comment_on_text(
            "January",
            "Ada",
            "Check the date",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
        let doc = reopen(&mut doc);
        let xml = doc.part_xml("word/comments.xml").unwrap();
        let last = &xml[xml.rfind("<w:comment ").unwrap()..];
        for (tag, built_in) in [
            ("<w:pStyle w:val=\"", "annotation text"),
            ("<w:rStyle w:val=\"", "annotation reference"),
        ] {
            let at = last.find(tag).unwrap_or_else(|| panic!("{name}: no {tag}"));
            let id = &last[at + tag.len()..];
            let id = &id[..id.find('"').unwrap()];
            let style_name = doc
                .style(id)
                .and_then(|style| style.name().map(str::to_owned));
            assert_eq!(
                style_name.map(|name| name.to_ascii_lowercase()).as_deref(),
                Some(built_in),
                "{name}: the new comment's {id} is Word's {built_in:?}"
            );
        }
    }
}

#[test]
fn a_comment_reuses_comment_styles_found_by_id_and_refuses_one_of_the_wrong_type() {
    // A file written before comment styles carried Word's names: they are
    // found by their ids and kept as they are.
    let mut doc = doc_with(&["review this"]);
    doc.add_style(StyleBuilder::paragraph("CommentText", "CommentText"))
        .unwrap();
    doc.add_style(StyleBuilder::character(
        "CommentReference",
        "CommentReference",
    ))
    .unwrap();
    let style_count = doc.styles().len();
    let id = doc
        .add_comment_on_text(
            "review",
            "Ada",
            "Please review",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.styles().len(), style_count, "no style is created");
    for id in ["CommentText", "CommentReference"] {
        assert_eq!(
            doc.style(id)
                .and_then(|style| style.name().map(str::to_owned))
                .as_deref(),
            Some(id),
            "the existing style {id} is reused, not renamed"
        );
    }
    let xml = doc.part_xml("word/comments.xml").unwrap();
    assert!(xml.contains(r#"<w:pStyle w:val="CommentText"/>"#), "{xml}");
    assert!(
        xml.contains(r#"<w:rStyle w:val="CommentReference"/>"#),
        "{xml}"
    );
    let body = doc.part_xml("word/document.xml").unwrap();
    assert_eq!(
        comment_reference_style(&body, id).as_deref(),
        Some("CommentReference"),
        "the body mark:\n{body}"
    );

    let mut doc = doc_with(&["review this"]);
    doc.add_style(StyleBuilder::character("CommentText", "CommentText"))
        .unwrap();
    let error = doc
        .add_comment_on_text(
            "review",
            "Ada",
            "Please review",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("wrong type"), "{error}");
}

#[test]
fn a_comment_mark_style_of_the_wrong_type_leaves_no_comment_text_style_behind() {
    // The comment text style is created before the mark's style is found
    // to be a paragraph style: the refused add takes it back.
    type Add = fn(&mut Document) -> oxoffice_docx::Result<i32>;
    let routes: [(&str, Add); 3] = [
        ("on text", |doc| {
            doc.add_comment_on_text(
                "review",
                "Ada",
                "Please review",
                TextCommentOptions {
                    date: CommentDate::Undated,
                    ..Default::default()
                },
            )
        }),
        ("on runs", |doc| {
            doc.add_comment_with_date(
                run_range(0, 0, 0, 1),
                "Ada",
                None,
                "Please review",
                CommentDate::Undated,
            )
        }),
        ("on a story range", |doc| {
            let first = story_paragraph(doc, StoryKind::Body, 0);
            let second = doc.story_items(first.story()).unwrap()[1]
                .location()
                .clone();
            let mut range = story_range(&first, 0, 1);
            range.end = StoryRunPosition {
                location: second,
                run_index: 1,
            };
            doc.add_story_comment_with_date(
                range,
                "Ada",
                None,
                "Please review",
                CommentDate::Undated,
            )
        }),
    ];
    for (route, add) in routes {
        let mut doc = doc_with(&["review this", "and this"]);
        doc.add_style(StyleBuilder::paragraph(
            "CommentReference",
            "CommentReference",
        ))
        .unwrap();
        let before = doc.to_bytes().unwrap();
        let error = add(&mut doc).unwrap_err().to_string();
        assert!(
            error.contains("comment style CommentReference has the wrong type"),
            "{route}: {error}"
        );
        assert!(
            doc.style("CommentText").is_none(),
            "{route}: no comment text style is left behind"
        );
        assert_eq!(doc.to_bytes().unwrap(), before, "{route}: nothing changed");
    }
}

/// `doc` saved, its `word/styles.xml` rewritten by `edit`, and reopened:
/// styles as another producer wrote them, which the facade would not write.
fn with_styles_xml(doc: &mut Document, edit: impl FnOnce(String) -> String) -> Document {
    let bytes = doc.to_bytes().expect("document saves");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let xml = String::from_utf8(
        package
            .get_part("/word/styles.xml")
            .expect("styles part")
            .to_vec(),
    )
    .expect("styles are UTF-8");
    package.set_part("/word/styles.xml", edit(xml).into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    Document::from_bytes(&out.into_inner()).expect("document with these styles opens")
}

/// The styles the comment text and comment reference styles that a new
/// comment on "review" creates are based on, after a save.
fn comment_style_bases(doc: &mut Document) -> [Option<String>; 2] {
    doc.add_comment_on_text(
        "review",
        "Ada",
        "Please review",
        TextCommentOptions {
            date: CommentDate::Undated,
            ..Default::default()
        },
    )
    .unwrap();
    let doc = reopen(doc);
    ["CommentText", "CommentReference"].map(|id| {
        doc.style(id)
            .unwrap_or_else(|| panic!("{id} is created"))
            .based_on()
            .map(str::to_owned)
    })
}

#[test]
fn new_comment_styles_fall_back_to_normal_and_default_paragraph_font_by_id() {
    // A file that marks no style default: the created comment styles stand
    // on Word's usual base styles, found by their ids.
    let mut doc = doc_with(&["review this"]);
    doc.add_style(StyleBuilder::character(
        "DefaultParagraphFont",
        "Default Paragraph Font",
    ))
    .unwrap();
    let mut doc = with_styles_xml(&mut doc, |xml| xml.replace(r#" w:default="1""#, ""));
    assert!(
        doc.styles().iter().all(|style| !style.is_default()),
        "no style is marked default"
    );
    assert_eq!(
        comment_style_bases(&mut doc),
        [
            Some("Normal".to_owned()),
            Some("DefaultParagraphFont".to_owned())
        ]
    );
}

#[test]
fn a_base_id_naming_a_style_of_the_other_type_is_no_comment_style_base() {
    // No style is marked default, and Word's usual base ids name styles of
    // the other type: the created comment styles are based on nothing.
    let mut doc = doc_with(&["review this"]);
    doc.add_style(StyleBuilder::paragraph(
        "DefaultParagraphFont",
        "Default Paragraph Font",
    ))
    .unwrap();
    let mut doc = with_styles_xml(&mut doc, |xml| {
        xml.replace(r#" w:default="1""#, "").replace(
            r#"w:type="paragraph" w:styleId="Normal""#,
            r#"w:type="character" w:styleId="Normal""#,
        )
    });
    assert_eq!(
        doc.style("Normal").map(|style| style.style_type()),
        Some(StyleType::Character)
    );
    assert_eq!(comment_style_bases(&mut doc), [None, None]);
}

#[test]
fn the_comment_styles_a_new_document_gets_are_words_built_in_ones() {
    let mut doc = doc_with(&["review this"]);
    doc.add_comment_on_text(
        "review",
        "Ada",
        "Please review",
        TextCommentOptions {
            date: CommentDate::Undated,
            ..Default::default()
        },
    )
    .unwrap();
    let doc = reopen(&mut doc);
    let name = |id: &str| {
        doc.style(id)
            .and_then(|style| style.name().map(str::to_owned))
    };
    for (id, built_in) in [
        ("CommentText", "annotation text"),
        ("CommentReference", "annotation reference"),
    ] {
        assert_eq!(
            name(id).as_deref(),
            Some(built_in),
            "{id} is Word's built-in {built_in:?}"
        );
    }
}

/// The character style of the run that holds the reference of comment `id`
/// in `xml`, a story part as saved.
fn comment_reference_style(xml: &str, id: i32) -> Option<String> {
    let at = xml.find(&format!("<w:commentReference w:id=\"{id}\""))?;
    let before = &xml[..at];
    let run = before.rfind("<w:r>").max(before.rfind("<w:r "))?;
    let tag = "<w:rStyle w:val=\"";
    let style = before[run..].find(tag)? + run + tag.len();
    Some(xml[style..style + xml[style..].find('"')?].to_owned())
}

#[test]
fn new_comment_styles_have_words_sizes_and_every_new_comment_mark_takes_the_reference_style() {
    let mut doc = doc_with(&["review this", "and this run", "and this story range"]);
    doc.set_header("Header words");
    // The first comment, in a header, comes with the comment parts.
    let header = story_paragraph(&doc, StoryKind::Header, 0);
    let in_header = doc
        .add_story_comment_with_date(
            story_range(&header, 0, 1),
            "Ada",
            None,
            "On the header",
            CommentDate::Undated,
        )
        .unwrap();
    let on_text = doc
        .add_comment_on_text(
            "review",
            "Ada",
            "Please review",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
    let on_runs = doc
        .add_comment_with_date(
            run_range(1, 0, 1, 1),
            "Ada",
            None,
            "On the run",
            CommentDate::Undated,
        )
        .unwrap();
    let reply = doc
        .reply_to_with_date(on_text, "Ben", "Agreed", CommentDate::Undated)
        .unwrap();
    let body = story_paragraph(&doc, StoryKind::Body, 0).story().clone();
    let third = doc.story_items(&body).unwrap()[2].location().clone();
    let on_story_range = doc
        .add_story_comment_with_date(
            story_range(&third, 0, 1),
            "Ada",
            None,
            "On the story range",
            CommentDate::Undated,
        )
        .unwrap();
    let doc = reopen(&mut doc);

    // Word's built-in "annotation text" is 10 pt on single lines, its
    // "annotation reference" 8 pt.
    let text = doc.style("CommentText").expect("comment text style");
    let run = text.run_properties().expect("comment text run properties");
    assert_eq!(
        (run.sz, run.sz_cs),
        (Some(HalfPoint(20)), Some(HalfPoint(20)))
    );
    let paragraph = text
        .paragraph_properties()
        .expect("comment text paragraph properties");
    assert_eq!(
        (paragraph.line_spacing, paragraph.line_rule.as_deref()),
        (Some(Twips(240)), Some("auto"))
    );
    let reference = doc
        .style("CommentReference")
        .expect("comment reference style");
    let mark = reference
        .run_properties()
        .expect("comment reference run properties");
    assert_eq!(
        (mark.sz, mark.sz_cs),
        (Some(HalfPoint(16)), Some(HalfPoint(16)))
    );
    // Both are kept out of the style gallery until used, as Word's are.
    for style in [&text, &reference] {
        assert_eq!(
            (
                style.priority(),
                style.semi_hidden(),
                style.unhide_when_used()
            ),
            (Some(99), Some(true), Some(true)),
            "{}",
            style.style_id()
        );
    }

    let body = doc.part_xml("word/document.xml").unwrap();
    for id in [on_text, on_runs, reply, on_story_range] {
        assert_eq!(
            comment_reference_style(&body, id).as_deref(),
            Some("CommentReference"),
            "the body mark of comment {id}:\n{body}"
        );
    }
    let header = doc.part_xml("word/header1.xml").unwrap();
    assert_eq!(
        comment_reference_style(&header, in_header).as_deref(),
        Some("CommentReference"),
        "the header mark:\n{header}"
    );
}

#[test]
fn a_comment_mark_takes_the_reference_style_in_every_story_owner_a_range_can_name() {
    let mut doc = doc_with_body(concat!(
        "<w:p><w:r><w:t>first of two</w:t></w:r></w:p>",
        "<w:p><w:r><w:t>second of two</w:t></w:r></w:p>",
        "<w:sdt><w:sdtPr/><w:sdtContent>",
        "<w:p><w:r><w:t>in a control</w:t></w:r></w:p>",
        "</w:sdtContent></w:sdt>",
    ));
    let mut table = doc.add_table(1, 1);
    table.cell(0, 0).expect("cell").set_text("in a cell");
    let first = doc.paragraph_story_location(0).unwrap().unwrap();
    doc.create_footnote(&first, "in a note").unwrap();
    let add = |doc: &mut Document, range: StoryRunRange| {
        doc.add_story_comment_with_date(range, "Ada", None, "Here", CommentDate::Undated)
            .unwrap()
    };

    // Taken again: the note's reference rewrote the first paragraph.
    let first = doc.paragraph_story_location(0).unwrap().unwrap();
    let second = doc.paragraph_story_location(1).unwrap().unwrap();
    let mut over_two = story_range(&first, 0, 1);
    over_two.end = StoryRunPosition {
        location: second,
        run_index: 1,
    };
    let over_two = add(&mut doc, over_two);
    let control = doc.paragraph_story_location(2).unwrap().unwrap();
    assert_eq!(control.index_path().len(), 2, "a paragraph in a control");
    let in_control = add(&mut doc, story_range(&control, 0, 1));
    let cell = story_paragraph(&doc, StoryKind::TableCell, 0);
    let in_cell = add(&mut doc, story_range(&cell, 0, 1));
    let note = story_paragraph(&doc, StoryKind::Footnote, 0);
    // The note's text run, after the run of its own reference mark.
    let in_note = add(&mut doc, story_range(&note, 1, 2));
    let doc = reopen(&mut doc);

    let body = doc.part_xml("word/document.xml").unwrap();
    for (owner, id) in [
        ("a range over two paragraphs", over_two),
        ("a content control", in_control),
        ("a table cell", in_cell),
    ] {
        assert_eq!(
            comment_reference_style(&body, id).as_deref(),
            Some("CommentReference"),
            "the mark in {owner}:\n{body}"
        );
    }
    let notes = doc.part_xml("word/footnotes.xml").unwrap();
    assert_eq!(
        comment_reference_style(&notes, in_note).as_deref(),
        Some("CommentReference"),
        "the mark in a note:\n{notes}"
    );
}

#[test]
fn the_body_mark_of_a_comment_added_to_a_localized_word_file_takes_its_comment_reference_style() {
    // Word for Mac in French names the comment reference style
    // "Marquedecommentaire".
    let mut doc = corpus_doc("word/report.docx");
    let root = doc
        .add_comment_on_text(
            "January",
            "Ada",
            "Check the date",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
    let reply = doc
        .reply_to_with_date(root, "Ben", "Checked", CommentDate::Undated)
        .unwrap();
    let doc = reopen(&mut doc);
    let body = doc.part_xml("word/document.xml").unwrap();
    for id in [root, reply] {
        assert_eq!(
            comment_reference_style(&body, id).as_deref(),
            Some("Marquedecommentaire"),
            "the body mark of comment {id}"
        );
    }
}

#[test]
fn removing_a_comment_takes_its_styled_reference_run_with_it() {
    // Word and oxoffice give the comment mark a character style: its run
    // holds nothing but the reference, and goes with the comment.
    let mut doc = doc_with(&["review this"]);
    let id = doc
        .add_comment_on_text(
            "review",
            "Ada",
            "Please review",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(doc.remove_comment(id).unwrap());
    let doc = reopen(&mut doc);
    let body = doc.part_xml("word/document.xml").unwrap();
    assert!(!body.contains("CommentReference"), "{body}");

    let mut doc = corpus_doc("word/report.docx");
    let id = doc.comments()[0].id();
    assert!(doc.remove_comment(id).unwrap());
    let doc = reopen(&mut doc);
    let body = doc.part_xml("word/document.xml").unwrap();
    assert!(!body.contains("Marquedecommentaire"), "{body}");
}

/// `doc` has no comment yet: no comments part, nothing listed.
fn assert_no_comments_yet(doc: &Document) {
    assert!(doc.comments().is_empty(), "no comment yet");
    assert!(
        doc.part_xml("word/comments.xml").is_err(),
        "no comments part yet"
    );
}

/// After a save, comment `id` reads back `text`, and its mark in `part`
/// carries the comment reference style.
fn assert_first_comment_reads_back(doc: &mut Document, id: i32, part: &str, text: &str) {
    let doc = reopen(doc);
    assert_eq!(comment(&doc, id).text(), text);
    let xml = doc.part_xml(part).unwrap();
    assert_eq!(
        comment_reference_style(&xml, id).as_deref(),
        Some("CommentReference"),
        "the mark of comment {id} in {part}:\n{xml}"
    );
}

#[test]
fn a_first_story_comment_on_a_body_paragraph_comes_with_the_comment_parts() {
    // #372: the story-range route wrote the commented paragraph back through
    // the stories after adding the relationship to a comments part not
    // written yet, and the stories followed it.
    let mut doc = doc_with(&["Body"]);
    assert_no_comments_yet(&doc);
    let paragraph = story_paragraph(&doc, StoryKind::Body, 0);
    let id = doc
        .add_story_comment(story_range(&paragraph, 0, 1), "Ada", None, "Thread")
        .unwrap();
    assert_first_comment_reads_back(&mut doc, id, "word/document.xml", "Thread");
}

#[test]
fn a_first_story_comment_in_a_table_cell_comes_with_the_comment_parts() {
    let mut doc = table_doc();
    assert_no_comments_yet(&doc);
    let cell = story_paragraph(&doc, StoryKind::TableCell, 0);
    let id = doc
        .add_story_comment(story_range(&cell, 0, 1), "Ada", None, "In a cell")
        .unwrap();
    assert_first_comment_reads_back(&mut doc, id, "word/document.xml", "In a cell");
}

#[test]
fn a_first_story_comment_in_a_block_content_control_comes_with_the_comment_parts() {
    let mut doc = doc_with_body(concat!(
        "<w:p><w:r><w:t>before</w:t></w:r></w:p>",
        "<w:sdt><w:sdtPr/><w:sdtContent>",
        "<w:p><w:r><w:t>in a control</w:t></w:r></w:p>",
        "</w:sdtContent></w:sdt>",
    ));
    assert_no_comments_yet(&doc);
    let control = doc.paragraph_story_location(1).unwrap().unwrap();
    assert_eq!(control.index_path().len(), 2, "a paragraph in a control");
    let id = doc
        .add_story_comment(story_range(&control, 0, 1), "Ada", None, "In a control")
        .unwrap();
    assert_first_comment_reads_back(&mut doc, id, "word/document.xml", "In a control");
}

#[test]
fn a_first_story_comment_in_a_header_or_a_footer_comes_with_the_comment_parts() {
    for (kind, part) in [
        (StoryKind::Header, "word/header1.xml"),
        (StoryKind::Footer, "word/footer1.xml"),
    ] {
        let mut doc = doc_with(&["Body"]);
        doc.set_header("Header words");
        doc.set_footer("Footer words");
        assert_no_comments_yet(&doc);
        let paragraph = story_paragraph(&doc, kind, 0);
        let id = doc
            .add_story_comment(story_range(&paragraph, 0, 1), "Ada", None, "Up here")
            .unwrap();
        assert_eq!(anchor_kind(&doc, id), kind);
        assert_first_comment_reads_back(&mut doc, id, part, "Up here");
    }
}

#[test]
fn a_first_comment_on_text_or_on_runs_comes_with_the_comment_parts() {
    let mut doc = doc_with(&["review this"]);
    assert_no_comments_yet(&doc);
    let id = doc
        .add_comment_on_text(
            "review",
            "Ada",
            "On text",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
    assert_first_comment_reads_back(&mut doc, id, "word/document.xml", "On text");

    let mut doc = doc_with(&["review this"]);
    assert_no_comments_yet(&doc);
    let id = doc
        .add_comment(run_range(0, 0, 0, 1), "Ada", None, "On runs")
        .unwrap();
    assert_first_comment_reads_back(&mut doc, id, "word/document.xml", "On runs");
}

#[test]
fn a_reply_on_the_first_comment_of_a_document_takes_the_reference_style() {
    type First = fn(&mut Document) -> i32;
    let routes: [(&str, First); 3] = [
        ("on text", |doc| {
            doc.add_comment_on_text(
                "Body",
                "Ada",
                "Thread",
                TextCommentOptions {
                    date: CommentDate::Undated,
                    ..Default::default()
                },
            )
            .unwrap()
        }),
        ("on runs", |doc| {
            doc.add_comment(run_range(0, 0, 0, 1), "Ada", None, "Thread")
                .unwrap()
        }),
        ("on a story range", |doc| {
            let paragraph = story_paragraph(doc, StoryKind::Body, 0);
            doc.add_story_comment(story_range(&paragraph, 0, 1), "Ada", None, "Thread")
                .unwrap()
        }),
    ];
    for (route, first) in routes {
        let mut doc = doc_with(&["Body"]);
        assert_no_comments_yet(&doc);
        let root = first(&mut doc);
        let reply = doc.reply_to(root, "Ben", "Reply").unwrap();
        let mut doc = reopen(&mut doc);
        assert_eq!(comment(&doc, reply).parent_id(), Some(root), "{route}");
        assert_first_comment_reads_back(&mut doc, root, "word/document.xml", "Thread");
        assert_first_comment_reads_back(&mut doc, reply, "word/document.xml", "Reply");
    }
}

// ---- Range edges inside tracked changes ----

/// A document whose one body paragraph is `paragraph`, with a comment `id`
/// whose range markers `paragraph` places itself.
fn commented_paragraph(paragraph: &str) -> (Document, i32) {
    let mut doc = doc_with(&["Placeholder"]);
    let id = doc
        .add_comment_on_text("Placeholder", "Ada", "Note", TextCommentOptions::default())
        .unwrap();
    let bytes = doc.to_bytes().unwrap();
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let main = package.main_document_part().unwrap();
    let xml = String::from_utf8(package.get_part(&main).unwrap().to_vec()).unwrap();
    let start = xml.find("<w:p>").or_else(|| xml.find("<w:p ")).unwrap();
    let end = xml[start..].find("</w:p>").unwrap() + start + "</w:p>".len();
    let paragraph = paragraph.replace("ID", &id.to_string());
    package.set_part(
        &main,
        format!("{}{paragraph}{}", &xml[..start], &xml[end..]).into_bytes(),
    );
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    (Document::from_bytes(&out.into_inner()).unwrap(), id)
}

const DEL: &str = r#"w:id="91" w:author="Bo" w:date="2026-11-21T00:00:00Z""#;
const INS: &str = r#"w:id="92" w:author="Bo" w:date="2026-11-21T00:00:00Z""#;

#[test]
fn a_range_edge_inside_tracked_deleted_or_moved_text_reads_at_the_next_accepted_position() {
    let start = r#"<w:commentRangeStart w:id="ID"/>"#;
    let end = r#"<w:commentRangeEnd w:id="ID"/><w:r><w:commentReference w:id="ID"/></w:r>"#;
    let run = |text: &str| format!("<w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r>");
    let deleted = |inner: &str| {
        format!(r#"<w:del {DEL}><w:r><w:delText>ninety (90)</w:delText></w:r>{inner}</w:del>"#)
    };
    let moved = |inner: &str| {
        format!(r#"<w:moveFrom {DEL}><w:r><w:delText>old</w:delText></w:r>{inner}</w:moveFrom>"#)
    };
    let inserted = |inner: &str| format!(r#"<w:ins {INS}>{inner}</w:ins>"#);
    for (paragraph, expected) in [
        // The start inside the deletion, as Google Docs writes it.
        (
            format!(
                "<w:p>{}{}{}{end}</w:p>",
                run("Within "),
                deleted(start),
                inserted(&run("120 days")),
            ),
            "120 days",
        ),
        // The end inside the deletion.
        (
            format!(
                "<w:p>{start}{}{}{}</w:p>",
                run("Within "),
                deleted(end),
                run(" days"),
            ),
            "Within ",
        ),
        // The start inside moved-away text.
        (
            format!("<w:p>{}{}{end}</w:p>", moved(start), run("kept")),
            "kept",
        ),
        // The end inside moved-away text.
        (
            format!("<w:p>{start}{}{}</w:p>", run("kept"), moved(end)),
            "kept",
        ),
        // Both edges inside a tracked insertion.
        (
            format!(
                "<w:p>{}{}</w:p>",
                run("Pay "),
                inserted(&format!("{}{start}{}{end}", run("within "), run("30 days"))),
            ),
            "30 days",
        ),
    ] {
        let (doc, id) = commented_paragraph(&paragraph);
        assert_eq!(
            doc.comment_anchor_text(id).unwrap().as_deref(),
            Some(expected),
            "{paragraph}"
        );
    }
}

#[test]
fn a_comment_starting_inside_a_deletion_can_be_moved_and_removed() {
    let (mut doc, id) = commented_paragraph(&format!(
        concat!(
            r#"<w:p><w:r><w:t xml:space="preserve">Within </w:t></w:r>"#,
            r#"<w:del {}><w:r><w:delText>ninety (90)</w:delText></w:r><w:commentRangeStart w:id="ID"/></w:del>"#,
            r#"<w:ins {}><w:r><w:t>120 days</w:t></w:r></w:ins>"#,
            r#"<w:commentRangeEnd w:id="ID"/><w:r><w:commentReference w:id="ID"/></w:r></w:p>"#
        ),
        DEL, INS
    ));
    doc.move_comment_to_text(id, "Within", 0).unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(
        doc.comment_anchor_text(id).unwrap().as_deref(),
        Some("Within")
    );
    assert!(doc.remove_comment(id).unwrap());
    let doc = reopen(&mut doc);
    assert!(doc.comments().is_empty());
    let main = doc.part_xml("word/document.xml").unwrap();
    assert!(!main.contains("commentRange"), "{main}");
    assert!(main.contains("ninety (90)"), "the deletion stays: {main}");
}

#[test]
fn the_google_copy_of_the_contract_reads_the_anchors_of_the_word_copy() {
    let anchors = |file: &str| {
        let doc = corpus_doc(file);
        let mut anchors = doc
            .comments()
            .iter()
            .map(|comment| comment.anchor_text().unwrap().unwrap_or_default())
            .collect::<Vec<_>>();
        anchors.sort();
        anchors
    };
    let mut word = anchors("word/contract.docx");
    let google = anchors("google/contract.docx");
    // Google drops the resolved comment; every anchor it keeps reads as Word's.
    let resolved = corpus_doc("word/contract.docx")
        .comments()
        .iter()
        .find(|comment| comment.resolved() == Some(true))
        .map(|comment| comment.anchor_text().unwrap().unwrap_or_default())
        .expect("the Word copy keeps the resolved comment");
    word.remove(word.iter().position(|anchor| *anchor == resolved).unwrap());
    assert_eq!(google, word);
}

#[test]
fn the_google_thread_starting_inside_a_deletion_moves_with_its_reply() {
    let mut doc = corpus_doc("google/contract.docx");
    let (root, reply) = {
        let comments = doc.comments();
        let reply = comments
            .iter()
            .find(|comment| comment.parent_id().is_some())
            .expect("a reply");
        (reply.parent_id().unwrap(), reply.id())
    };
    doc.move_comment_to_text(root, "Confidential Information", 0)
        .unwrap();
    let doc = reopen(&mut doc);
    for id in [root, reply] {
        assert_eq!(
            doc.comment_anchor_text(id).unwrap().as_deref(),
            Some("Confidential Information"),
            "comment {id}"
        );
    }
}

#[test]
fn a_comment_inside_a_tracked_insertion_can_be_removed() {
    let (mut doc, id) = commented_paragraph(&format!(
        concat!(
            r#"<w:p><w:r><w:t xml:space="preserve">Pay </w:t></w:r><w:ins {}>"#,
            r#"<w:r><w:t xml:space="preserve">within </w:t></w:r><w:commentRangeStart w:id="ID"/>"#,
            r#"<w:r><w:t>30 days</w:t></w:r><w:commentRangeEnd w:id="ID"/>"#,
            r#"<w:r><w:commentReference w:id="ID"/></w:r></w:ins></w:p>"#
        ),
        INS
    ));
    assert_eq!(
        doc.comment_anchor_text(id).unwrap().as_deref(),
        Some("30 days")
    );
    assert!(doc.remove_comment(id).unwrap());
    let doc = reopen(&mut doc);
    assert!(doc.comments().is_empty());
    let main = doc.part_xml("word/document.xml").unwrap();
    assert!(!main.contains("comment"), "{main}");
    assert!(main.contains("30 days"), "{main}");
}

#[test]
fn accepting_or_rejecting_the_change_a_comment_sits_on_keeps_the_comment() {
    // Google Docs starts the first thread of its contract copy inside the
    // deletion of the tracked change it discusses.
    let mut doc = corpus_doc("google/contract.docx");
    let before = doc
        .comments()
        .iter()
        .map(|comment| comment.anchor_text().unwrap().unwrap_or_default())
        .collect::<Vec<_>>();
    doc.accept_all().unwrap();
    let doc = reopen(&mut doc);
    let after = doc
        .comments()
        .iter()
        .map(|comment| comment.anchor_text().unwrap().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(after, before);

    // A comment inside a tracked insertion, the insertion rejected.
    let (mut doc, id) = commented_paragraph(&format!(
        concat!(
            r#"<w:p><w:r><w:t xml:space="preserve">Pay </w:t></w:r><w:ins {}>"#,
            r#"<w:r><w:t xml:space="preserve">within </w:t></w:r><w:commentRangeStart w:id="ID"/>"#,
            r#"<w:r><w:t>30 days</w:t></w:r><w:commentRangeEnd w:id="ID"/></w:ins>"#,
            r#"<w:r><w:t xml:space="preserve"> now</w:t></w:r>"#,
            r#"<w:r><w:commentReference w:id="ID"/></w:r></w:p>"#
        ),
        INS
    ));
    doc.reject_all().unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.comments().len(), 1);
    assert_eq!(doc.comment_anchor_text(id).unwrap().as_deref(), Some(""));
    assert_eq!(texts(&doc), ["Pay  now"]);
}

#[test]
fn accepting_or_rejecting_a_change_that_holds_a_whole_comment_removes_the_comment() {
    let whole = |wrapper: &str, attributes: &str, text: &str| {
        format!(
            concat!(
                r#"<w:p><w:r><w:t xml:space="preserve">Pay </w:t></w:r><w:{0} {1}>"#,
                r#"<w:commentRangeStart w:id="ID"/><w:r><w:{2}>30 days</w:{2}></w:r>"#,
                r#"<w:commentRangeEnd w:id="ID"/><w:r><w:commentReference w:id="ID"/></w:r></w:{0}></w:p>"#
            ),
            wrapper, attributes, text
        )
    };
    for (paragraph, accept) in [
        (whole("ins", INS, "t"), false),
        (whole("del", DEL, "delText"), true),
    ] {
        let (mut doc, _) = commented_paragraph(&paragraph);
        if accept {
            doc.accept_all().unwrap();
        } else {
            doc.reject_all().unwrap();
        }
        let doc = reopen(&mut doc);
        assert!(doc.comments().is_empty(), "{paragraph}");
        let main = doc.part_xml("word/document.xml").unwrap();
        assert!(!main.contains("commentRange"), "{main}");
        assert_eq!(texts(&doc), ["Pay "]);
    }
}

/// Author, date, text, parent's text, resolved state and anchor of every
/// comment, as a tool reads a review back.
fn review_of(doc: &Document) -> Vec<String> {
    let comments = doc.comments();
    let text_of = |id: i32| {
        comments
            .iter()
            .find(|comment| comment.id() == id)
            .map(CommentRef::text)
    };
    comments
        .iter()
        .map(|comment| {
            format!(
                "{:?} {:?} {:?} parent {:?} resolved {:?} on {:?}",
                comment.author(),
                comment.date(),
                comment.text(),
                comment.parent_id().and_then(text_of),
                comment.resolved(),
                comment.anchor_text().ok().flatten(),
            )
        })
        .collect()
}

const REVIEWER: &str = "Reviewer";

/// The comments and tracked changes of everyone but [`REVIEWER`].
fn other_reviewers(doc: &Document) -> (Vec<String>, Vec<String>) {
    let comments = review_of(doc)
        .into_iter()
        .filter(|line| !line.starts_with(&format!("Some({REVIEWER:?})")))
        .collect::<Vec<_>>();
    let revisions = doc
        .story_revisions()
        .unwrap()
        .iter()
        .filter(|revision| revision.author() != REVIEWER)
        .map(|revision| {
            format!(
                "{:?} {} {:?} {:?}",
                revision.kind(),
                revision.author(),
                revision.timestamp(),
                revision.text()
            )
        })
        .collect::<Vec<_>>();
    (comments, revisions)
}

/// Comments in one person's name on a contract other people already
/// reviewed, one of them on a table cell: they read back dated and open, and
/// the other reviewers' comments, replies, resolved state and tracked changes
/// stay as they were.
#[test]
fn comments_in_one_name_leave_the_other_reviewers_alone() {
    for producer in crate::support::workflow_copies(&["word", "google", "libreoffice"]) {
        let path = format!("{producer}/contract.docx");
        let before = corpus_doc(&path);
        let mut doc = corpus_doc(&path);
        for (anchor, occurrence, tables_only) in [
            ("Effective Date", 0, false),
            ("Business Day", 1, false),
            ("Schedule 2", 0, false),
            ("material breach", 0, false),
            ("Signature:", 1, true),
        ] {
            doc.add_comment_on_text(
                anchor,
                REVIEWER,
                &format!("Check \"{anchor}\""),
                TextCommentOptions {
                    occurrence,
                    tables_only,
                    date: CommentDate::At("2026-11-21T10:05:00Z"),
                    ..Default::default()
                },
            )
            .unwrap_or_else(|e| panic!("{path}: comment on {anchor}: {e}"));
        }
        let doc = reopen(&mut doc);
        assert_eq!(other_reviewers(&doc), other_reviewers(&before), "{path}");
        let mine = review_of(&doc)
            .into_iter()
            .filter(|line| line.starts_with(&format!("Some({REVIEWER:?})")))
            .collect::<Vec<_>>();
        assert_eq!(mine.len(), 5, "{path}: {mine:#?}");
        assert!(
            mine.iter().all(|line| line.contains("2026-11-21T10:05:00Z")
                && line.contains("parent None")
                && !line.contains("resolved Some(true)")),
            "{path}: {mine:#?}"
        );
        assert!(
            mine.iter()
                .any(|line| line.ends_with("on Some(\"Signature:\")")),
            "{path}: {mine:#?}"
        );
    }
}

/// A redline in one person's name on that contract, then one of their
/// changes accepted and one rejected by id (a replaced phrase is a deletion
/// and an insertion, each with its id): the text shows the accepted change
/// only, and the other reviewers' tracked changes and comments stay.
#[test]
fn a_redline_in_one_name_leaves_the_other_reviewers_alone() {
    redline_in_one_name("word/contract.docx");
}

// Known failure #366, left by its fix: on the Google and LibreOffice copies
// the comparison still refuses the renewal clause, which holds another
// reviewer's change. When one passes, drop its `should_panic`.
#[test]
#[should_panic(expected = "google/contract.docx: the redline of a reviewed contract is made")]
fn a_redline_in_one_name_on_the_google_copy_leaves_the_other_reviewers_alone() {
    redline_in_one_name("google/contract.docx");
}

#[test]
#[should_panic(expected = "libreoffice/contract.docx: the redline of a reviewed contract is made")]
fn a_redline_in_one_name_on_the_libreoffice_copy_leaves_the_other_reviewers_alone() {
    redline_in_one_name("libreoffice/contract.docx");
}

fn redline_in_one_name(path: &str) {
    {
        let before = corpus_doc(path);
        let mut edited = corpus_doc(path);
        let pairs = [
            ("thirty-six (36) months", "forty-eight (48) months", Some(1)),
            ("appropriately qualified staff", "trained staff", Some(1)),
        ];
        edited.try_replace_all_expected(&pairs).unwrap().unwrap();
        let mut doc = corpus_doc(path);
        doc.compare(&edited, REVIEWER, "2026-11-21T10:00:00Z")
            .unwrap_or_else(|e| {
                assert!(
                    e.to_string().contains("holds an existing tracked revision"),
                    "{path}: unexpected refusal: {e}"
                );
                panic!("{path}: the redline of a reviewed contract is made: {e}")
            });
        let mut doc = reopen(&mut doc);
        let mine = doc
            .story_revisions()
            .unwrap()
            .into_iter()
            .filter(|revision| revision.author() == REVIEWER)
            .collect::<Vec<_>>();
        let ids_of = |texts: [&str; 2]| {
            let ids = mine
                .iter()
                .filter(|revision| texts.iter().any(|text| revision.text().contains(text)))
                .map(|revision| revision.id())
                .collect::<Vec<_>>();
            assert!(
                !ids.is_empty(),
                "{path}: a revision of {REVIEWER} holds {texts:?}: {mine:?}"
            );
            ids
        };
        let kept = ids_of(["forty-eight", "thirty-six"]);
        let refused = ids_of(["trained", "appropriately qualified"]);
        for id in kept {
            assert!(doc.accept_revision_id(id).unwrap() > 0, "{path}");
        }
        for id in refused {
            assert!(doc.reject_revision_id(id).unwrap() > 0, "{path}");
        }
        let doc = reopen(&mut doc);
        assert_eq!(other_reviewers(&doc), other_reviewers(&before), "{path}");
        assert!(
            doc.story_revisions()
                .unwrap()
                .iter()
                .all(|revision| revision.author() != REVIEWER),
            "{path}: every change of {REVIEWER} is resolved"
        );
        let text = (0..doc.paragraph_count())
            .map(|index| doc.paragraph(index).unwrap().text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("forty-eight (48) months"), "{path}");
        assert!(!text.contains("thirty-six (36) months"), "{path}");
        assert!(text.contains("appropriately qualified staff"), "{path}");
        assert!(!text.contains("trained staff"), "{path}");
    }
}

/// A tool rebases a copy someone annotated in Google Docs onto the last clean
/// write: it reads every thread of the annotated copy and writes it again on
/// the clean one, anchor, author, date, replies and resolved state included.
#[test]
fn the_threads_of_a_google_copy_carry_over_to_a_clean_copy() {
    let annotated = corpus_doc("google/contract.docx");
    let mut clean = corpus_doc("authored/contract.docx");
    let ids = clean
        .comments()
        .iter()
        .map(CommentRef::id)
        .collect::<Vec<_>>();
    for id in ids.into_iter().rev() {
        clean.remove_comment(id).unwrap();
    }
    assert!(reopen(&mut clean).comments().is_empty());

    let mut carried = std::collections::HashMap::new();
    for comment in annotated.comments() {
        let date = CommentDate::At(comment.date().expect("Google dates its comments"));
        let author = comment.author().unwrap();
        let id = match comment.parent_id() {
            Some(parent) => clean
                .reply_to_with_date(carried[&parent], author, &comment.text(), date)
                .unwrap(),
            None => {
                let anchor = match comment.anchor_text() {
                    Ok(Some(anchor)) => anchor,
                    other => {
                        // The #364 refusal, and no other, is the known failure.
                        assert!(
                            format!("{other:?}")
                                .contains("selected source is outside the accepted paragraph view"),
                            "unexpected anchor: {other:?}"
                        );
                        panic!("the anchor of a Google thread reads: {other:?}")
                    }
                };
                clean
                    .add_comment_on_text(
                        &anchor,
                        author,
                        &comment.text(),
                        TextCommentOptions {
                            date,
                            ..Default::default()
                        },
                    )
                    .unwrap()
            }
        };
        if comment.resolved() == Some(true) {
            clean.resolve_comment(id, true).unwrap();
        }
        carried.insert(comment.id(), id);
    }
    let clean = reopen(&mut clean);
    let normalise = |lines: Vec<String>| {
        lines
            .into_iter()
            .map(|line| line.replace("resolved None", "resolved Some(false)"))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        normalise(review_of(&clean)),
        normalise(review_of(&annotated))
    );
    assert!(
        annotated
            .comments()
            .iter()
            .any(|comment| comment.parent_id().is_some()),
        "the Google copy keeps reply threads"
    );
}

/// A comment id and the ids of its paragraphs.
type CommentParagraphs = (i32, Vec<String>);
/// A `commentEx` entry: paraId, paraIdParent, done.
type CommentEntry = (String, Option<String>, bool);

/// The ids of the paragraphs of each comment in `comments.xml`, by comment
/// id, and the `commentEx` entries of `commentsExtended.xml` as (paraId,
/// paraIdParent, done).
fn comment_paragraph_ids(bytes: &[u8]) -> (Vec<CommentParagraphs>, Vec<CommentEntry>) {
    let parts = crate::support::package_parts(bytes);
    let comments = String::from_utf8(parts["word/comments.xml"].clone()).unwrap();
    let attribute = |tag: &str, name: &str| {
        tag.split(&format!("{name}=\""))
            .nth(1)
            .map(|rest| rest.split('"').next().unwrap().to_owned())
    };
    let by_comment = comments
        .split("<w:comment ")
        .skip(1)
        .map(|chunk| {
            let id = attribute(chunk, "w:id").unwrap().parse().unwrap();
            let body = chunk.split("</w:comment>").next().unwrap();
            let ids = body
                .split("<w:p ")
                .skip(1)
                .filter_map(|tag| attribute(tag.split('>').next().unwrap(), "w14:paraId"))
                .collect();
            (id, ids)
        })
        .collect();
    let extended = String::from_utf8(parts["word/commentsExtended.xml"].clone()).unwrap();
    let entries = extended
        .split("<w15:commentEx ")
        .skip(1)
        .map(|tag| {
            let tag = tag.split("/>").next().unwrap();
            (
                attribute(tag, "w15:paraId").unwrap(),
                attribute(tag, "w15:paraIdParent"),
                attribute(tag, "w15:done").as_deref() == Some("1"),
            )
        })
        .collect();
    (by_comment, entries)
}

/// Word and Google Docs key a comment's thread and resolved state on its
/// last paragraph: a reply names its parent's last paragraph, and the done
/// mark sits on the entry of the resolved comment's last paragraph.
#[test]
fn threads_and_resolved_state_are_written_on_the_last_paragraph_of_a_comment() {
    let mut doc = doc_with(&["The fees are due within thirty days."]);
    let parent = doc
        .add_comment_on_text(
            "thirty days",
            "Ann",
            "first line\nsecond line",
            TextCommentOptions {
                date: CommentDate::Undated,
                ..Default::default()
            },
        )
        .unwrap();
    let reply = doc
        .reply_to_with_date(parent, "Ben", "agreed\nwith a note", CommentDate::Undated)
        .unwrap();
    doc.resolve_comment(parent, true).unwrap();
    let (by_comment, entries) = comment_paragraph_ids(&doc.to_bytes().unwrap());

    let last = |id: i32| {
        let (_, ids) = by_comment
            .iter()
            .find(|(comment, _)| *comment == id)
            .unwrap();
        assert_eq!(
            ids.len(),
            2,
            "comment {id} keeps its two lines as paragraphs"
        );
        ids[1].clone()
    };
    let (parent_last, reply_last) = (last(parent), last(reply));
    assert!(
        entries.contains(&(parent_last.clone(), None, true)),
        "{entries:?}"
    );
    assert!(
        entries.contains(&(reply_last, Some(parent_last), false)),
        "{entries:?}"
    );
    let firsts = by_comment
        .iter()
        .map(|(_, ids)| ids[0].clone())
        .collect::<Vec<_>>();
    assert!(
        entries.iter().all(|(id, _, _)| !firsts.contains(id)),
        "no entry names a first paragraph: {entries:?}"
    );
}

/// Word, Google Docs and LibreOffice keep the contract's reply threads: a
/// reply reads its parent, and the parent of each reply is a top-level
/// comment.
#[test]
fn every_producer_copy_of_the_contract_keeps_its_reply_threads() {
    // Each reply of the source, with the comment it answers.
    let threads = [
        ("Agreed for trade secrets", "Should trade secrets stay"),
        ("Fine with that split", "Should trade secrets stay"),
        ("We need the extra month", "120 days is longer"),
    ];
    for producer in ["authored", "word", "google", "libreoffice"] {
        let doc = corpus_doc(&format!("{producer}/contract.docx"));
        let comments = doc.comments();
        let mut replies = comments
            .iter()
            .filter_map(|comment| {
                let parent = comment.parent_id()?;
                let parent = comments
                    .iter()
                    .find(|other| other.id() == parent)
                    .unwrap_or_else(|| {
                        panic!("{producer}: the parent of {} is there", comment.text())
                    });
                assert_eq!(
                    parent.parent_id(),
                    None,
                    "{producer}: a parent is top-level"
                );
                Some((comment.text(), parent.text()))
            })
            .collect::<Vec<_>>();
        replies.sort();
        assert_eq!(replies.len(), threads.len(), "{producer}: {replies:?}");
        for ((reply, parent), (reply_start, parent_start)) in replies.iter().zip(threads) {
            assert!(
                reply.starts_with(reply_start) && parent.starts_with(parent_start),
                "{producer}: {reply:?} answers {parent:?}"
            );
        }
    }
}
