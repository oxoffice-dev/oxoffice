//! Comments: adding them on text, on run ranges and in other stories, author,
//! initials and date, reply threads, the done state, editing, moving and
//! removing them, and the comments Word, Google Docs and LibreOffice saved.

use std::io::Cursor;

use oxoffice_docx::paragraph::ParagraphItemRef;
use oxoffice_docx::{
    CommentDate, CommentRef, ContentLocation, Document, HalfPoint, RunItemRef, RunPosition,
    RunRange, StoryKind, StoryRunPosition, StoryRunRange, StyleBuilder, StyleType,
    TextCommentOptions, Twips,
};
use oxoffice_opc::OpcPackage;

use crate::support::{doc_with, doc_with_body, reopen, texts};

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
fn writing_each_run_text_back_keeps_the_comment_reference() {
    let mut doc = doc_with(&["Commented"]);
    let id = doc
        .add_comment(run_range(0, 0, 0, 1), "Ada", None, "Note")
        .unwrap();
    let mut doc = reopen(&mut doc);
    {
        // The find-and-replace loop over every run, the reference run included.
        let mut paragraph = doc.paragraph_mut(0).unwrap();
        let count = paragraph.runs().count();
        for index in 0..count {
            let mut run = paragraph.run_mut(index).unwrap();
            let text = run.text().replace("Commented", "Reviewed");
            run.set_text(&text);
        }
    }
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Reviewed"]);
    assert_eq!(
        doc.comment_anchor_text(id).unwrap().as_deref(),
        Some("Reviewed")
    );
    assert!(
        doc.part_xml("word/document.xml")
            .unwrap()
            .contains("<w:commentReference")
    );
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

const REVIEWER: &str = "Reviewer";

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

// Known failure #443: a comment's edge inside a tracked insertion is refused.
// When it passes, drop the `should_panic`.
#[test]
#[should_panic(expected = "falls inside a tracked insertion or move")]
fn a_comment_on_part_of_a_tracked_insertion_stays_inside_it() {
    let mut doc = doc_with_body(
        r#"<w:p><w:r><w:t>Kept </w:t></w:r><w:ins w:id="1" w:author="A" w:date="2026-01-01T00:00:00Z"><w:r><w:t>new words here</w:t></w:r></w:ins></w:p>"#,
    );
    doc.add_comment_on_text(
        "words",
        REVIEWER,
        "Check",
        TextCommentOptions {
            date: CommentDate::Undated,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.comments()[0].anchor_text().unwrap().as_deref(),
        Some("words")
    );
    let revisions = doc.story_revisions().unwrap();
    assert_eq!(revisions.len(), 1);
    assert_eq!(revisions[0].text(), "new words here");
}
