//! Comments: adding them on text, on run ranges and in other stories, author,
//! initials and date, reply threads, the done state, editing, moving and
//! removing them, and the comments Word, Google Docs and LibreOffice saved.

use std::fs;
use std::io::Cursor;
use std::path::Path;

use oxoffice_docx::paragraph::ParagraphItemRef;
use oxoffice_docx::{
    CommentRef, ContentLocation, Document, RunItemRef, RunPosition, RunRange, StoryKind,
    StoryRunPosition, StoryRunRange, StyleType,
};
use oxoffice_opc::OpcPackage;

use crate::support::{corpus_doc, doc_with, reopen, texts};

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
            0,
            "Ada Lovelace",
            Some("AL"),
            "Which one?",
            Some("2026-11-20T10:00:00Z"),
        )
        .unwrap();
    let bare = doc
        .add_comment_on_text("world", 0, "", None, "No initials, no date", None)
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
fn each_line_of_a_comment_becomes_one_of_its_paragraphs() {
    let mut doc = doc_with(&["Hello brave world"]);
    let id = doc
        .add_comment_on_text(
            "brave",
            0,
            "Ada",
            None,
            "  padded\tline  \r\nsecond\n\nfourth",
            None,
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
        .add_comment_on_text("llo wo", 0, "Ada", None, "Split me", None)
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
            doc.add_comment_on_text("word", occurrence, "Ada", None, "Here", None)
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
        .add_comment_on_text("text", 2, "Ada", None, "Third?", None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("it occurs 2 times"), "{error}");
    // Matches do not overlap: "aaa" holds "aa" once.
    let error = doc
        .add_comment_on_text("aa", 1, "Ada", None, "Second?", None)
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
        .add_comment_on_text("", 0, "Ada", None, "Nothing", None)
        .unwrap_err();
    assert!(empty.to_string().contains("must not be empty"), "{empty}");
    // One match never spans two paragraphs.
    assert!(
        doc.add_comment_on_text("one start", 0, "Ada", None, "Across", None)
            .is_err()
    );
    // Headers, footers and notes are not searched: comment them by range.
    let header = doc
        .add_comment_on_text("Header", 0, "Ada", None, "In the header", None)
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
            Some("2026-11-20T10:00:00Z"),
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
            doc.add_comment_on_text(anchor, 0, author, *initials, text, None)
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
        .add_comment_on_text("text", 0, "Ada", None, "bell\u{7}", None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("U+0007"), "{error}");
    assert!(
        doc.add_comment_on_text("text", 0, "Ada\u{1}", None, "Fine", None)
            .is_err()
    );
    assert!(doc.comments().is_empty());
}

#[test]
fn a_date_is_kept_as_written_and_a_date_that_is_not_rfc_3339_is_refused() {
    let mut doc = doc_with(&["text"]);
    for date in ["2026-02-30", "yesterday", "2026-11-20 10:00"] {
        let error = doc
            .add_comment_on_text("text", 0, "Ada", None, "Dated", Some(date))
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid RFC 3339"), "{date}: {error}");
    }
    let id = doc
        .add_comment_on_text(
            "text",
            0,
            "Ada",
            None,
            "Dated",
            Some("2026-09-16T11:00:00+01:00"),
        )
        .unwrap();
    let reply = doc
        .reply_to_with_date(id, "Ben", "Later", Some("2026-09-16T12:30:15.250Z"))
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
        .add_comment_on_text("Second", 0, "Ada", None, "On the second", None)
        .unwrap();
    let first = doc
        .add_comment_on_text("First", 0, "Ben", None, "On the first", None)
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
        .add_comment_on_text("review", 0, "Ada", None, "Please review", None)
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
        .add_comment_on_text("thirty days", 0, "Ada", Some("AL"), "Too short?", None)
        .unwrap();
    let reply = doc
        .reply_to_with_initials(
            root,
            "Ben",
            Some("BN"),
            "Agreed.\nMake it sixty.",
            Some("2026-11-21T09:30:00Z"),
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
        .add_comment_on_text("brave", 0, "Ada", None, "root", None)
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
    doc.add_comment_on_text("text", 0, "Ada", None, "Known", None)
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
        .add_comment_on_text("brave", 0, "Ada", None, "Please review", None)
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
        .add_comment_on_text("brave", 0, "Ada", None, "root", None)
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
        .add_comment_on_text("brave", 0, "Ada", None, "first line\nsecond line", None)
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
        .add_comment_on_text("brave", 0, "Ada", None, "root", None)
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
        .add_comment_on_text("commented", 0, "Ada", None, "Whole paragraph", None)
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
fn moving_a_comment_to_other_text_brings_its_replies() {
    let mut doc = doc_with(&["alpha beta", "gamma delta"]);
    let root = doc
        .add_comment_on_text("beta", 0, "Ada", None, "root", None)
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
        .add_comment_on_text("brave", 0, "Ada", None, "Which one?", None)
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
        .add_comment_on_text("south", 0, "Ada", None, "On text in a cell", None)
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
            Some("2026-11-20T10:00:00Z"),
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
            0,
            "Ada",
            Some("AL"),
            "Which one?",
            Some("2026-11-20T10:00:00Z"),
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
                Some("2026-11-21T09:00:00Z"),
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
            .add_comment_on_text("from 6 h 40 to 3", 0, "Ada", Some("AL"), "Check", None)
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

// Fails today: a comment added to a Word file whose comment styles have
// localized ids ("Commentaire", "Marquedecommentaire") brings its own
// CommentText and CommentReference styles beside them. Issue: #27
#[test]
#[should_panic(expected = "styles added beside Word's comment styles")]
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

    doc.add_comment_on_text("January", 0, "Ada", None, "Check the date", None)
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
        for named in xml.split("Style w:val=\"").skip(1) {
            let id = &named[..named.find('"').unwrap()];
            assert!(
                ids.iter().any(|known| known == id),
                "{part} names the undefined style {id}"
            );
        }
    }
    assert!(
        marks >= 2,
        "the new comment is marked with Word's comment mark style"
    );
}

// Fails today: the comment styles a new document gets are named after their
// ids, so Word reads them as custom styles, not its built-in "annotation
// text" and "annotation reference". Issue: #27
#[test]
#[should_panic(expected = "is Word's built-in")]
fn the_comment_styles_a_new_document_gets_are_words_built_in_ones() {
    let mut doc = doc_with(&["review this"]);
    doc.add_comment_on_text("review", 0, "Ada", None, "Please review", None)
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
