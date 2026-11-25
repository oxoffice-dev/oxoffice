//! Block locations: every operation that inserts, copies, moves or removes
//! whole blocks reads a location the same way, in every story, inside block
//! content controls too, and a refusal names the route that works.

use std::io::Cursor;
use std::ops::Range;
use std::sync::OnceLock;

use oxoffice_docx::{
    CaptionOptions, ContentFragment, ContentLocation, Document, DocumentFragment,
    FragmentConflictPolicy, RunPosition, RunRange, StoryId, StoryItemKind, StoryKind,
};
use oxoffice_docx_oxml::text::CT_P;
use oxoffice_opc::OpcPackage;

use crate::support::{corpus_doc, doc_with_body, map_in_parallel, reopen, text_box_run};

const W15: &str = "http://schemas.microsoft.com/office/word/2012/wordml";

fn p(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

fn sdt(tag: &str, properties: &str, content: &str) -> String {
    format!(
        "<w:sdt><w:sdtPr><w:tag w:val=\"{tag}\"/>{properties}</w:sdtPr><w:sdtContent>{content}</w:sdtContent></w:sdt>"
    )
}

/// A body with a block control holding a nested one, a table whose cell holds
/// a control, a text box, a plain text control, a repeating section, an
/// inline control; a header holding a control, a footnote and a comment.
fn fixture_bytes() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| {
        let body = [
            p("Before"),
            sdt(
                "box",
                "",
                &[p("Box 1"), sdt("inner", "", &p("Inner 1")), p("Box 2")].concat(),
            ),
            p("After"),
            format!(
                "<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w=\"4000\"/></w:tblGrid><w:tr><w:tc>\
                 <w:tcPr><w:tcW w:w=\"4000\" w:type=\"dxa\"/></w:tcPr>{}{}{}</w:tc></w:tr></w:tbl>",
                p("Cell"),
                sdt("cellbox", "", &p("Cell box 1")),
                p("Cell end"),
            ),
            format!(
                "<w:p><w:r><w:t>Holder</w:t></w:r>{}</w:p>",
                text_box_run(7, &p("Text box"))
            ),
            sdt("typed", "<w:text/>", &p("Plain")),
            sdt(
                "section",
                &format!("<w15:repeatingSection xmlns:w15=\"{W15}\"/>"),
                &sdt(
                    "item",
                    &format!("<w15:repeatingSectionItem xmlns:w15=\"{W15}\"/>"),
                    &p("Item 1"),
                ),
            ),
            "<w:p><w:r><w:t>Tail</w:t></w:r><w:sdt><w:sdtPr><w:tag w:val=\"inline\"/></w:sdtPr>\
             <w:sdtContent><w:r><w:t>!</w:t></w:r></w:sdtContent></w:sdt></w:p>"
                .to_owned(),
        ]
        .concat();
        let mut doc = doc_with_body(&body);
        let tail = paragraph(&doc, "Tail!");
        doc.create_footnote(&tail, "Note").expect("footnote");
        let first_run = RunPosition {
            body_index: 0,
            run_index: 0,
        };
        let range = RunRange {
            start: first_run,
            end: RunPosition {
                run_index: 1,
                ..first_run
            },
        };
        doc.add_comment(range, "Ada", None, "Remark")
            .expect("comment");
        doc.set_header("Head");
        let header = story(&doc, StoryKind::Header).part_name().to_owned();
        let bytes = doc.to_bytes().expect("fixture saves");
        let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
        let xml = String::from_utf8(package.get_part(&header).expect("header").to_vec())
            .expect("header is UTF-8");
        let xml = xml.replace(
            "</w:hdr>",
            &format!("{}</w:hdr>", sdt("headbox", "", &p("Head box 1"))),
        );
        package.set_part(&header, xml.into_bytes());
        let mut out = Cursor::new(Vec::new());
        package.write_to(&mut out).expect("package writes");
        let bytes = out.into_inner();
        Document::from_bytes(&bytes).expect("fixture reopens");
        bytes
    })
}

fn fixture() -> Document {
    Document::from_bytes(fixture_bytes()).expect("fixture opens")
}

fn story(doc: &Document, kind: StoryKind) -> StoryId {
    doc.stories()
        .expect("stories")
        .into_iter()
        .find(|story| story.kind() == kind)
        .unwrap_or_else(|| panic!("a {kind:?} story"))
}

fn same_story(left: &StoryId, right: &StoryId) -> bool {
    left.kind() == right.kind()
        && left.part_name() == right.part_name()
        && left.owner_index() == right.owner_index()
}

/// The texts of the paragraphs of `story`, in order.
fn texts_of(doc: &Document, story: &StoryId) -> Vec<String> {
    doc.story_paragraphs()
        .expect("story paragraphs")
        .into_iter()
        .filter(|paragraph| same_story(paragraph.location.story(), story))
        .map(|paragraph| paragraph.text)
        .collect()
}

/// The location of the one story paragraph whose text is `text`.
fn paragraph(doc: &Document, text: &str) -> ContentLocation {
    let found = doc
        .story_paragraphs()
        .expect("story paragraphs")
        .into_iter()
        .filter(|paragraph| paragraph.text == text)
        .map(|paragraph| paragraph.location)
        .collect::<Vec<_>>();
    assert_eq!(found.len(), 1, "one paragraph {text:?}");
    found.into_iter().next().unwrap()
}

/// The story and item index of the content control tagged `tag`.
fn control_index(doc: &Document, tag: &str) -> (StoryId, usize) {
    let needle = format!("<w:tag w:val=\"{tag}\"/>");
    doc.story_item_snapshots()
        .expect("story items")
        .into_iter()
        .find(|item| {
            item.location().item_kind() == StoryItemKind::ContentControl
                && String::from_utf8_lossy(item.xml())
                    .find("<w:tag ")
                    .is_some_and(|at| {
                        String::from_utf8_lossy(item.xml())[at..].starts_with(&needle)
                    })
        })
        .map(|item| {
            (
                item.location().story().clone(),
                item.location().index_path()[0],
            )
        })
        .unwrap_or_else(|| panic!("control {tag}"))
}

fn control(doc: &Document, tag: &str) -> ContentLocation {
    let (story, index) = control_index(doc, tag);
    ContentLocation::new(story, StoryItemKind::ContentControl, vec![index])
}

fn control_end(doc: &Document, tag: &str) -> ContentLocation {
    let (story, index) = control_index(doc, tag);
    ContentLocation::control_end(story, index)
}

fn control_paragraph(doc: &Document, tag: &str, k: usize) -> ContentLocation {
    let (story, index) = control_index(doc, tag);
    ContentLocation::new(story, StoryItemKind::Paragraph, vec![index, k])
}

fn new_paragraph(text: &str) -> ContentFragment {
    let mut paragraph = CT_P::new();
    paragraph.add_run(text);
    ContentFragment::paragraph(paragraph).expect("paragraph fragment")
}

type Locate = fn(&Document) -> ContentLocation;

/// One location shape and what the operations do with it.
struct Shape {
    name: &'static str,
    at: Locate,
    /// The story whose paragraphs an operation at the shape changes.
    story: fn(&Document) -> StoryId,
    /// Where a paragraph inserted at the shape lands among those paragraphs.
    insert_at: usize,
    /// The paragraphs of the block the shape names; `None` at an end.
    block: Option<Range<usize>>,
    /// The block before it in its container, and its paragraphs.
    previous: Option<(Locate, Range<usize>)>,
    /// The boundary after the block in its container.
    next: Option<Locate>,
    /// The operations refused at the shape, each with a part of its message.
    refused: &'static [(Op, &'static str)],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Insert,
    Import,
    Html,
    Caption,
    CloneFrom,
    CloneTo,
    Move,
    Remove,
    RangeFrom,
    RangeTo,
}

const OPS: [Op; 10] = [
    Op::Insert,
    Op::Import,
    Op::Html,
    Op::Caption,
    Op::CloneFrom,
    Op::CloneTo,
    Op::Move,
    Op::Remove,
    Op::RangeFrom,
    Op::RangeTo,
];

const END: &str = "an end is a boundary between blocks";
const ONLY: &str = "only block of content control";
const HTML: &str = "from_html";
const TYPED: &str = "set the control's value";
const SECTION: &str = "repeat_content_control";
const SOURCES_AT_AN_END: [(Op, &str); 4] = [
    (Op::CloneFrom, END),
    (Op::Move, END),
    (Op::Remove, END),
    (Op::RangeFrom, END),
];
const PAST: &str = "control_end(story, ";
const INLINE: &str = "inside a paragraph and holds no block";
const TABLE: &str = "#660";
const NOT_A_BLOCK: &str = "not a block";

fn all(message: &'static str) -> &'static [(Op, &'static str)] {
    Box::leak(
        OPS.iter()
            .map(|op| (*op, message))
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    )
}

fn body(doc: &Document) -> StoryId {
    story(doc, StoryKind::Body)
}

fn cell(doc: &Document) -> StoryId {
    story(doc, StoryKind::TableCell)
}

fn header(doc: &Document) -> StoryId {
    story(doc, StoryKind::Header)
}

fn shapes() -> Vec<Shape> {
    // Body paragraphs: Before, Box 1, Inner 1, Box 2, After, Holder, Plain,
    // Item 1, Tail!
    vec![
        Shape {
            name: "a body paragraph [i]",
            at: |doc| paragraph(doc, "After"),
            story: body,
            insert_at: 4,
            block: Some(4..5),
            previous: Some((|doc| control(doc, "box"), 1..4)),
            next: Some(|doc| {
                let (story, _) = control_index(doc, "box");
                let after = paragraph(doc, "After").index_path()[0];
                ContentLocation::new(story, StoryItemKind::Table, vec![after + 1])
            }),
            refused: &[],
        },
        Shape {
            name: "a paragraph [c, k] of a body control",
            at: |doc| control_paragraph(doc, "box", 1),
            story: body,
            insert_at: 3,
            block: Some(3..4),
            previous: Some((|doc| control(doc, "inner"), 2..3)),
            next: Some(|doc| control_end(doc, "box")),
            refused: &[],
        },
        Shape {
            name: "the end of a body control",
            at: |doc| control_end(doc, "box"),
            story: body,
            insert_at: 4,
            block: None,
            previous: Some((|doc| control_paragraph(doc, "box", 1), 3..4)),
            next: None,
            refused: &SOURCES_AT_AN_END,
        },
        Shape {
            name: "a paragraph [c, k] of a nested control",
            at: |doc| control_paragraph(doc, "inner", 0),
            story: body,
            insert_at: 2,
            block: Some(2..3),
            previous: None,
            next: Some(|doc| control_end(doc, "inner")),
            refused: &[(Op::Move, ONLY), (Op::Remove, ONLY)],
        },
        Shape {
            name: "the end of a nested control",
            at: |doc| control_end(doc, "inner"),
            story: body,
            insert_at: 3,
            block: None,
            previous: Some((|doc| control_paragraph(doc, "inner", 0), 2..3)),
            next: None,
            refused: &SOURCES_AT_AN_END,
        },
        Shape {
            name: "a nested block control [i]",
            at: |doc| control(doc, "inner"),
            story: body,
            insert_at: 2,
            block: Some(2..3),
            previous: Some((|doc| control_paragraph(doc, "box", 0), 1..2)),
            next: Some(|doc| control_paragraph(doc, "box", 1)),
            refused: &[],
        },
        Shape {
            name: "the end of the body",
            at: |doc| ContentLocation::end(body(doc)),
            story: body,
            insert_at: 9,
            block: None,
            previous: Some((|doc| paragraph(doc, "Tail!"), 8..9)),
            next: None,
            refused: &SOURCES_AT_AN_END,
        },
        Shape {
            name: "[c, n] past a control's paragraphs",
            at: |doc| control_paragraph(doc, "box", 2),
            story: body,
            insert_at: 0,
            block: None,
            previous: None,
            next: None,
            refused: all(PAST),
        },
        Shape {
            name: "a cell paragraph [i]",
            at: |doc| paragraph(doc, "Cell"),
            story: cell,
            insert_at: 0,
            block: Some(0..1),
            previous: None,
            next: Some(|doc| control(doc, "cellbox")),
            refused: &[],
        },
        Shape {
            name: "a paragraph of a control in a cell",
            at: |doc| control_paragraph(doc, "cellbox", 0),
            story: cell,
            insert_at: 1,
            block: Some(1..2),
            previous: None,
            next: Some(|doc| control_end(doc, "cellbox")),
            refused: &[(Op::Move, ONLY), (Op::Remove, ONLY)],
        },
        Shape {
            name: "the end of a control in a cell",
            at: |doc| control_end(doc, "cellbox"),
            story: cell,
            insert_at: 2,
            block: None,
            previous: Some((|doc| control_paragraph(doc, "cellbox", 0), 1..2)),
            next: None,
            refused: &SOURCES_AT_AN_END,
        },
        Shape {
            name: "a header paragraph [i]",
            at: |doc| paragraph(doc, "Head"),
            story: header,
            insert_at: 0,
            block: Some(0..1),
            previous: None,
            next: Some(|doc| control(doc, "headbox")),
            refused: &[],
        },
        Shape {
            name: "a paragraph of a control in a header",
            at: |doc| control_paragraph(doc, "headbox", 0),
            story: header,
            insert_at: 1,
            block: Some(1..2),
            previous: None,
            next: Some(|doc| control_end(doc, "headbox")),
            refused: &[(Op::Move, ONLY), (Op::Remove, ONLY)],
        },
        Shape {
            name: "the end of a control in a header",
            at: |doc| control_end(doc, "headbox"),
            story: header,
            insert_at: 2,
            block: None,
            previous: Some((|doc| control_paragraph(doc, "headbox", 0), 1..2)),
            next: None,
            refused: &SOURCES_AT_AN_END,
        },
        Shape {
            name: "a footnote paragraph [i]",
            at: |doc| note_paragraph(doc),
            story: |doc| story(doc, StoryKind::Footnote),
            insert_at: 0,
            block: Some(0..1),
            previous: None,
            next: Some(|doc| ContentLocation::end(story(doc, StoryKind::Footnote))),
            refused: &[(Op::Html, HTML), (Op::Remove, "only block of its note")],
        },
        Shape {
            name: "a text box paragraph [i]",
            at: |doc| paragraph(doc, "Text box"),
            story: |doc| story(doc, StoryKind::TextBox),
            insert_at: 0,
            block: Some(0..1),
            previous: None,
            next: Some(|doc| ContentLocation::end(story(doc, StoryKind::TextBox))),
            refused: &[(Op::Html, HTML), (Op::Remove, "only block of its text box")],
        },
        Shape {
            name: "a comment paragraph [i]",
            at: |doc| paragraph(doc, "Remark"),
            story: |doc| story(doc, StoryKind::Comment),
            insert_at: 0,
            block: Some(0..1),
            previous: None,
            next: Some(|doc| ContentLocation::end(story(doc, StoryKind::Comment))),
            refused: &[(Op::Html, HTML), (Op::Remove, "only block of its comment")],
        },
        Shape {
            name: "the paragraph of a plain text control",
            at: |doc| control_paragraph(doc, "typed", 0),
            story: body,
            insert_at: 6,
            block: Some(6..7),
            previous: None,
            next: Some(|doc| control_end(doc, "typed")),
            refused: &[
                (Op::Insert, TYPED),
                (Op::Import, TYPED),
                (Op::Html, TYPED),
                (Op::Caption, TYPED),
                (Op::CloneTo, TYPED),
                (Op::Move, ONLY),
                (Op::Remove, ONLY),
            ],
        },
        Shape {
            name: "the end of a repeating section",
            at: |doc| control_end(doc, "section"),
            story: body,
            insert_at: 8,
            block: None,
            previous: Some((|doc| control(doc, "item"), 7..8)),
            next: None,
            refused: &[
                (Op::Insert, SECTION),
                (Op::Import, SECTION),
                (Op::Html, SECTION),
                (Op::Caption, SECTION),
                (Op::CloneTo, SECTION),
                (Op::CloneFrom, END),
                (Op::Move, END),
                (Op::Remove, END),
                (Op::RangeFrom, END),
            ],
        },
        Shape {
            name: "the end of a repeating section item",
            at: |doc| control_end(doc, "item"),
            story: body,
            insert_at: 8,
            block: None,
            previous: Some((|doc| control_paragraph(doc, "item", 0), 7..8)),
            next: None,
            refused: &SOURCES_AT_AN_END,
        },
        Shape {
            name: "[c, k] of an inline control",
            at: |doc| control_paragraph(doc, "inline", 0),
            story: body,
            insert_at: 0,
            block: None,
            previous: None,
            next: None,
            refused: all(INLINE),
        },
        Shape {
            name: "an inline control [i]",
            at: |doc| control(doc, "inline"),
            story: body,
            insert_at: 0,
            block: None,
            previous: None,
            next: None,
            refused: all(NOT_A_BLOCK),
        },
        Shape {
            name: "a table [c, k] in a block control",
            at: |doc| {
                let (story, index) = control_index(doc, "box");
                ContentLocation::new(story, StoryItemKind::Table, vec![index, 0])
            },
            story: body,
            insert_at: 0,
            block: None,
            previous: None,
            next: None,
            refused: all(TABLE),
        },
    ]
}

fn note_paragraph(doc: &Document) -> ContentLocation {
    let note = story(doc, StoryKind::Footnote);
    doc.story_paragraphs()
        .expect("story paragraphs")
        .into_iter()
        .find(|paragraph| same_story(paragraph.location.story(), &note))
        .expect("a footnote paragraph")
        .location
}

/// The fragment of the body's first paragraph, `Before`.
fn before_fragment(doc: &Document) -> DocumentFragment {
    let start = paragraph(doc, "Before");
    let end = control(doc, "box");
    DocumentFragment::from_range(doc, &start, &end, false).expect("fragment of Before")
}

/// Run `op` at `shape` on a fresh document. `None` when the operation does
/// not apply to the shape: a range from a block that has no next boundary in
/// its container, or to one with no block before it. A range at a shape
/// refused everywhere starts and ends at the shape.
fn run(op: Op, shape: &Shape, doc: &mut Document, refused: bool) -> Option<Result<(), String>> {
    let at = (shape.at)(doc);
    let story = (shape.story)(doc);
    let result = match op {
        Op::Insert => doc.insert_content(&at, new_paragraph("New")),
        Op::Import => {
            let fragment = before_fragment(doc);
            doc.import_fragment(&at, &fragment, FragmentConflictPolicy::default())
        }
        Op::Html => doc
            .insert_html_fragment(&at, "<p>Html</p>", &[])
            .map(|_| ()),
        Op::Caption => {
            let options = CaptionOptions {
                text: "Cap".into(),
                ..CaptionOptions::default()
            };
            match doc.insert_caption(&at, &options) {
                Ok(target) => {
                    // The location returned is the caption's.
                    let read = doc
                        .story_paragraphs()
                        .expect("story paragraphs")
                        .into_iter()
                        .find(|paragraph| {
                            same_story(paragraph.location.story(), target.paragraph.story())
                                && paragraph.location.index_path() == target.paragraph.index_path()
                        })
                        .map(|paragraph| paragraph.text);
                    if !read.as_deref().is_some_and(|text| text.ends_with("Cap")) {
                        return Some(Err(format!(
                            "the caption location {:?} reads {read:?}",
                            target.paragraph.index_path()
                        )));
                    }
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
        Op::CloneFrom => doc.clone_content(&at, &ContentLocation::end(story)),
        Op::CloneTo => {
            let source = paragraph(doc, "Before");
            doc.clone_content(&source, &at)
        }
        Op::Move => doc.move_content(&at, &ContentLocation::end(story)),
        Op::Remove => doc.remove_content_at(&at).map(|_| ()),
        Op::RangeFrom => {
            let next = match shape.next {
                Some(next) => next(doc),
                None if refused => at.clone(),
                None => return None,
            };
            DocumentFragment::from_range(doc, &at, &next, false).and_then(|fragment| {
                let end = ContentLocation::end((shape.story)(doc));
                doc.import_fragment(&end, &fragment, FragmentConflictPolicy::default())
            })
        }
        Op::RangeTo => {
            let previous = match &shape.previous {
                Some((previous, _)) => previous(doc),
                None if refused => at.clone(),
                None => return None,
            };
            DocumentFragment::from_range(doc, &previous, &at, false).and_then(|fragment| {
                let end = ContentLocation::end((shape.story)(doc));
                doc.import_fragment(&end, &fragment, FragmentConflictPolicy::default())
            })
        }
    };
    Some(result.map_err(|error| error.to_string()))
}

/// The story's paragraphs `op` at `shape` leaves.
fn expected(op: Op, shape: &Shape, before: &[String]) -> Vec<String> {
    let mut after = before.to_vec();
    let block = || before[shape.block.clone().expect("a block")].to_vec();
    let mut insert = |text: &str| after.insert(shape.insert_at, text.to_owned());
    match op {
        Op::Insert => insert("New"),
        Op::Import | Op::CloneTo => insert("Before"),
        Op::Html => insert("Html"),
        Op::Caption => {
            // The caption's text depends on its number's result.
            let text = String::new();
            insert(&text);
        }
        Op::CloneFrom | Op::RangeFrom => after.extend(block()),
        Op::Move => {
            let range = shape.block.clone().expect("a block");
            after.drain(range);
            after.extend(block());
        }
        Op::Remove => {
            after.drain(shape.block.clone().expect("a block"));
        }
        Op::RangeTo => {
            let (_, range) = shape.previous.as_ref().expect("a previous block");
            after.extend(before[range.clone()].to_vec());
        }
    }
    after
}

/// The outcome of one cell, for the summary.
enum Cell {
    Works,
    Refused,
    NotApplicable,
}

fn check(op: Op, shape: &Shape) -> Result<Cell, String> {
    let mut doc = fixture();
    let story = (shape.story)(&doc);
    let before = texts_of(&doc, &story);
    let refusal = shape
        .refused
        .iter()
        .find(|(refused, _)| *refused == op)
        .map(|(_, message)| *message);
    let Some(result) = run(op, shape, &mut doc, refusal.is_some()) else {
        return Ok(Cell::NotApplicable);
    };
    match (result, refusal) {
        (Err(error), Some(message)) => {
            if !error.contains(message) {
                return Err(format!(
                    "refused with {error:?}, which does not name {message:?}"
                ));
            }
            let story = (shape.story)(&doc);
            if texts_of(&doc, &story) != before {
                return Err("a refusal changed the story".to_owned());
            }
            Ok(Cell::Refused)
        }
        (Err(error), None) => Err(format!("refused: {error}")),
        (Ok(()), Some(message)) => Err(format!("works, expected a refusal naming {message:?}")),
        (Ok(()), None) => {
            let story = (shape.story)(&doc);
            let mut want = expected(op, shape, &before);
            let got = texts_of(&doc, &story);
            if op == Op::Caption
                && let Some(text) = got.get(shape.insert_at)
                && text.ends_with("Cap")
            {
                want[shape.insert_at] = text.clone();
            }
            if got != want {
                return Err(format!("paragraphs {got:?}, expected {want:?}"));
            }
            let saved = doc.to_bytes().map_err(|error| format!("save: {error}"))?;
            let mut reopened =
                Document::from_bytes(&saved).map_err(|error| format!("reopen: {error}"))?;
            let story = (shape.story)(&reopened);
            if texts_of(&reopened, &story) != want {
                return Err("the reopened document reads other paragraphs".to_owned());
            }
            let again = reopened
                .to_bytes()
                .map_err(|error| format!("second save: {error}"))?;
            if again != saved {
                return Err("the second save writes other bytes".to_owned());
            }
            Ok(Cell::Works)
        }
    }
}

#[test]
fn every_block_operation_takes_every_location_shape_or_names_the_working_route() {
    let shapes = shapes();
    let cells = shapes
        .iter()
        .flat_map(|shape| OPS.iter().map(move |op| (*op, shape)))
        .collect::<Vec<_>>();
    let outcomes = map_in_parallel(&cells, |(op, shape)| check(*op, shape));
    let (mut works, mut refused, mut skipped) = (0, 0, 0);
    let mut failures = Vec::new();
    for ((op, shape), outcome) in cells.iter().zip(outcomes) {
        match outcome {
            Some(Ok(Cell::Works)) => works += 1,
            Some(Ok(Cell::Refused)) => refused += 1,
            Some(Ok(Cell::NotApplicable)) => skipped += 1,
            Some(Err(failure)) => failures.push(format!("{op:?} at {}: {failure}", shape.name)),
            None => failures.push(format!("{op:?} at {}: panicked", shape.name)),
        }
    }
    println!(
        "{} cells: {works} work, {refused} refused with their route, {skipped} not applicable",
        cells.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The issue's body: one paragraph, a block control of two, one paragraph.
const ISSUE_BODY: &str = "<w:p><w:r><w:t>Before</w:t></w:r></w:p><w:sdt><w:sdtPr><w:tag w:val=\"box\"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Inside 1</w:t></w:r></w:p><w:p><w:r><w:t>Inside 2</w:t></w:r></w:p></w:sdtContent></w:sdt><w:p><w:r><w:t>After</w:t></w:r></w:p>";

fn body_texts(doc: &Document) -> Vec<String> {
    texts_of(doc, &body(doc))
}

#[test]
fn the_last_paragraph_of_a_block_control_is_copied_up_to_its_end() {
    let mut doc = doc_with_body(ISSUE_BODY);
    let error = DocumentFragment::from_range(
        &doc,
        &control_paragraph(&doc, "box", 1),
        &control_paragraph(&doc, "box", 2),
        false,
    )
    .map(|_| ())
    .expect_err("[c, n] is past the control's paragraphs")
    .to_string();
    assert!(error.contains("control_end(story, 1)"), "{error}");
    let error = DocumentFragment::from_range(
        &doc,
        &control_paragraph(&doc, "box", 1),
        &paragraph(&doc, "After"),
        false,
    )
    .map(|_| ())
    .expect_err("a range across containers")
    .to_string();
    assert!(error.contains("control_end(story, 1)"), "{error}");
    let fragment = DocumentFragment::from_range(
        &doc,
        &control_paragraph(&doc, "box", 1),
        &control_end(&doc, "box"),
        false,
    )
    .expect("the last paragraph up to the control's end");
    let at = control_end(&doc, "box");
    doc.import_fragment(&at, &fragment, FragmentConflictPolicy::default())
        .expect("imported at the control's end");
    assert_eq!(
        body_texts(&doc),
        ["Before", "Inside 1", "Inside 2", "Inside 2", "After"]
    );
    assert_eq!(
        control_paragraph(&doc, "box", 2).index_path(),
        paragraph(&doc, "After")
            .index_path()
            .first()
            .map(|_| [1, 2])
            .unwrap()
    );
    let reopened = reopen(&mut doc);
    assert_eq!(
        body_texts(&reopened),
        ["Before", "Inside 1", "Inside 2", "Inside 2", "After"]
    );
}

#[test]
fn a_paragraph_is_appended_at_the_end_of_a_block_control_and_the_story_end_stays_after_it() {
    // The body ends with the control, before its section properties.
    let mut doc = doc_with_body(
        "<w:p><w:r><w:t>Before</w:t></w:r></w:p><w:sdt><w:sdtPr><w:tag w:val=\"box\"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Inside</w:t></w:r></w:p><w:bookmarkStart w:id=\"5\" w:name=\"around\"/><w:p><w:r><w:t>Marked</w:t></w:r></w:p><w:bookmarkEnd w:id=\"5\"/></w:sdtContent></w:sdt>",
    );
    let at = control_end(&doc, "box");
    assert!(at.is_end());
    assert_eq!(at.index_path(), control(&doc, "box").index_path());
    doc.insert_content(&at, new_paragraph("In the control"))
        .expect("inserted at the control's end");
    let end = ContentLocation::end(body(&doc));
    doc.insert_content(&end, new_paragraph("After the control"))
        .expect("inserted at the body's end");
    let mut reopened = reopen(&mut doc);
    assert_eq!(
        body_texts(&reopened),
        [
            "Before",
            "Inside",
            "Marked",
            "In the control",
            "After the control"
        ]
    );
    let bytes = reopened.to_bytes().expect("saves");
    let xml = crate::support::package_parts(&bytes)["word/document.xml"].clone();
    let xml = String::from_utf8(xml).expect("UTF-8");
    let order = [
        "Marked",
        "<w:bookmarkEnd w:id=\"5\"/>",
        "In the control",
        "</w:sdtContent>",
        "After the control",
        "<w:sectPr",
    ]
    .map(|needle| {
        xml.find(needle)
            .unwrap_or_else(|| panic!("{needle} written"))
    });
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
}

#[test]
fn a_paragraph_is_cloned_into_and_moved_out_of_a_block_control() {
    let mut doc = doc_with_body(ISSUE_BODY);
    let source = paragraph(&doc, "Inside 2");
    let destination = control_paragraph(&doc, "box", 1);
    doc.clone_content(&source, &destination)
        .expect("cloned before itself, in the control");
    assert_eq!(
        body_texts(&doc),
        ["Before", "Inside 1", "Inside 2", "Inside 2", "After"]
    );
    let source = control_paragraph(&doc, "box", 0);
    let destination = paragraph(&doc, "After");
    doc.move_content(&source, &destination)
        .expect("moved out of the control");
    let source = paragraph(&doc, "Before");
    let destination = control_end(&doc, "box");
    doc.move_content(&source, &destination)
        .expect("moved into the control");
    assert_eq!(
        body_texts(&doc),
        ["Inside 2", "Inside 2", "Before", "Inside 1", "After"]
    );
    assert_eq!(
        paragraph(&doc, "Before").index_path(),
        control_paragraph(&doc, "box", 2).index_path()
    );
    let error = doc
        .move_content(&control(&doc, "box"), &control_end(&doc, "box"))
        .map(|_| ())
        .expect_err("a control into itself")
        .to_string();
    assert!(error.contains("cannot move into itself"), "{error}");
    let reopened = reopen(&mut doc);
    assert_eq!(
        body_texts(&reopened),
        ["Inside 2", "Inside 2", "Before", "Inside 1", "After"]
    );
}

#[test]
fn bookmark_markers_inside_a_block_control_go_with_their_paragraph() {
    let mut doc = doc_with_body(
        "<w:sdt><w:sdtPr><w:tag w:val=\"box\"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>One</w:t></w:r></w:p><w:bookmarkStart w:id=\"3\" w:name=\"heading\"/><w:p><w:r><w:t>Two</w:t></w:r></w:p><w:bookmarkEnd w:id=\"3\"/></w:sdtContent></w:sdt><w:p><w:r><w:t>After</w:t></w:r></w:p>",
    );
    let marked = control_paragraph(&doc, "box", 1);
    doc.clone_content(&marked, &control_end(&doc, "box"))
        .expect("cloned with its bookmark");
    let names = |doc: &Document| -> Vec<String> {
        let bytes = doc.clone_for_staging().to_bytes().expect("saves");
        let xml =
            String::from_utf8(crate::support::package_parts(&bytes)["word/document.xml"].clone())
                .expect("UTF-8");
        xml.match_indices("w:name=\"")
            .map(|(at, _)| xml[at + 8..].split('"').next().unwrap().to_owned())
            .collect()
    };
    let cloned = names(&doc);
    assert_eq!(cloned.len(), 2, "{cloned:?}");
    assert_ne!(cloned[0], cloned[1], "the copy's bookmark has its own name");
    // An insertion before the marked paragraph goes before its bookmark
    // start, and its removal takes the bookmark with it.
    doc.insert_content(&control_paragraph(&doc, "box", 1), new_paragraph("New"))
        .expect("inserted");
    let removed = doc
        .remove_content_at(&control_paragraph(&doc, "box", 2))
        .expect("removed");
    assert_eq!(
        names(&doc).len(),
        1,
        "the removed paragraph took its bookmark"
    );
    drop(removed);
    assert_eq!(body_texts(&doc), ["One", "New", "Two", "After"]);
}

#[test]
fn a_typed_control_takes_no_second_block() {
    let w14 = "http://schemas.microsoft.com/office/word/2010/wordml";
    for (kind, properties, takes) in [
        ("plain text", "<w:text/>".to_owned(), false),
        ("date", "<w:date/>".to_owned(), false),
        ("drop-down list", "<w:dropDownList/>".to_owned(), false),
        ("combo box", "<w:comboBox/>".to_owned(), false),
        ("picture", "<w:picture/>".to_owned(), false),
        (
            "check box",
            format!("<w14:checkbox xmlns:w14=\"{w14}\"/>"),
            false,
        ),
        ("rich text", "<w:richText/>".to_owned(), true),
        ("untyped", String::new(), true),
        ("group", "<w:group/>".to_owned(), true),
    ] {
        let body = format!(
            "{}<w:p><w:r><w:t>After</w:t></w:r></w:p>",
            sdt("typed", &properties, &p("Value"))
        );
        let mut doc = doc_with_body(&body);
        let at = control_end(&doc, "typed");
        let result = doc.insert_content(&at, new_paragraph("Second"));
        assert_eq!(result.is_ok(), takes, "{kind}: {result:?}");
        if let Err(error) = result {
            let error = error.to_string();
            assert!(
                error.contains(&format!("is a {kind} control")) && error.contains(TYPED),
                "{error}"
            );
        }
    }
}

/// The paragraphs of the table of contents control of a corpus thesis, its
/// story item index and its XML.
fn toc(doc: &Document) -> (usize, Vec<String>, String) {
    let item = doc
        .story_item_snapshots()
        .expect("story items")
        .into_iter()
        .find(|item| {
            item.location().item_kind() == StoryItemKind::ContentControl
                && String::from_utf8_lossy(item.xml()).contains("Table of Contents")
        })
        .expect("a table of contents control");
    let index = item.location().index_path()[0];
    let texts = doc
        .story_paragraphs()
        .expect("story paragraphs")
        .into_iter()
        .filter(|paragraph| {
            paragraph.location.index_path().len() == 2
                && paragraph.location.index_path()[0] == index
        })
        .map(|paragraph| paragraph.text)
        .collect();
    (
        index,
        texts,
        String::from_utf8_lossy(item.xml()).into_owned(),
    )
}

fn toc_paragraph(doc: &Document, control: usize, k: usize) -> ContentLocation {
    ContentLocation::new(body(doc), StoryItemKind::Paragraph, vec![control, k])
}

#[test]
fn a_toc_entry_is_added_after_the_last_one_by_cloning_it() {
    for file in ["word/thesis.docx", "google/thesis.docx"] {
        let mut doc = corpus_doc(file);
        let (control, texts, xml) = toc(&doc);
        let n = texts.len();
        let last_entry = texts[n - 2].clone();
        let field_ends = xml.matches("w:fldCharType=\"end\"").count();
        let begins = xml.matches("w:fldCharType=\"begin\"").count();
        // Before the paragraph that ends the TOC field.
        doc.clone_content(
            &toc_paragraph(&doc, control, n - 2),
            &toc_paragraph(&doc, control, n - 1),
        )
        .unwrap_or_else(|error| panic!("{file}: {error}"));
        let mut reopened = reopen(&mut doc);
        let (_, after, xml_after) = toc(&reopened);
        assert_eq!(after.len(), n + 1, "{file}");
        assert_eq!(after[n - 1], last_entry, "{file}: the copy");
        assert_eq!(&after[..n - 1], &texts[..n - 1], "{file}");
        assert_eq!(after[n], texts[n - 1], "{file}: the field end stays last");
        // The copy holds whole fields: as many begins as ends were added.
        let added_begins = xml_after.matches("w:fldCharType=\"begin\"").count() - begins;
        let added_ends = xml_after.matches("w:fldCharType=\"end\"").count() - field_ends;
        assert_eq!(added_begins, added_ends, "{file}");
        // The copy drops its paragraph identity, and every identity stays unique.
        let bytes = reopened.to_bytes().expect("saves");
        let document =
            String::from_utf8(crate::support::package_parts(&bytes)["word/document.xml"].clone())
                .expect("UTF-8");
        let ids = document
            .match_indices("w14:paraId=\"")
            .map(|(at, _)| &document[at + 12..at + 20])
            .collect::<Vec<_>>();
        let unique = ids.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), ids.len(), "{file}: unique paragraph ids");
    }
}

#[test]
fn a_copy_never_leaves_a_complex_field_unbalanced() {
    // A field begun in "Entry 1" and ended in "Entry 2", at story level and
    // inside a block control.
    let field = "<w:p><w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText> TOC \\o </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>Entry 1</w:t></w:r></w:p><w:p><w:r><w:t>Entry 2</w:t></w:r><w:r><w:fldChar w:fldCharType=\"end\"/></w:r></w:p>";
    for xml in [
        format!("{field}<w:p><w:r><w:t>Tail</w:t></w:r></w:p>"),
        format!(
            "{}<w:p><w:r><w:t>Tail</w:t></w:r></w:p>",
            sdt("box", "", field)
        ),
    ] {
        let mut doc = doc_with_body(&xml);
        let begin = paragraph(&doc, "Entry 1");
        let end = paragraph(&doc, "Entry 2");
        let tail = paragraph(&doc, "Tail");
        let before = body_texts(&doc);
        for (location, held) in [(&begin, "the begin"), (&end, "the end")] {
            let error = doc
                .clone_content(location, &tail)
                .map(|_| ())
                .expect_err("a clone of part of a field")
                .to_string();
            assert!(
                error.contains(&format!("holds {held} of a complex field"))
                    && error.contains("from_range"),
                "{error}"
            );
            let error = doc
                .move_content(location, &ContentLocation::end(body(&doc)))
                .map(|_| ())
                .expect_err("a move of part of a field")
                .to_string();
            assert!(error.contains(&format!("holds {held}")), "{error}");
        }
        // After "Entry 2": the control's end, or "Tail".
        let whole_end = if end.index_path().len() == 2 {
            control_end(&doc, "box")
        } else {
            tail.clone()
        };
        let error = DocumentFragment::from_range(&doc, &end, &whole_end, false)
            .map(|_| ())
            .expect_err("a range of part of a field")
            .to_string();
        assert!(error.contains("the range holds the end"), "{error}");
        assert_eq!(body_texts(&doc), before, "the refusals change nothing");
        // The whole field is copied.
        let fragment =
            DocumentFragment::from_range(&doc, &begin, &whole_end, false).expect("the whole field");
        doc.import_fragment(
            &ContentLocation::end(body(&doc)),
            &fragment,
            FragmentConflictPolicy::default(),
        )
        .expect("imported");
        assert_eq!(
            body_texts(&reopen(&mut doc)),
            ["Entry 1", "Entry 2", "Tail", "Entry 1", "Entry 2"]
        );
    }
    // In a table of contents, the first entry (which begins the field) and
    // the paragraph that ends it are refused; any other entry is copied.
    let mut doc = corpus_doc("word/thesis.docx");
    let (control, texts, _) = toc(&doc);
    let n = texts.len();
    let destination = toc_paragraph(&doc, control, n - 1);
    for k in [1, n - 1] {
        let error = doc
            .clone_content(&toc_paragraph(&doc, control, k), &destination)
            .err()
            .unwrap_or_else(|| panic!("entry {k} holds part of the TOC field"))
            .to_string();
        assert!(error.contains("of a complex field"), "{error}");
    }
    doc.clone_content(&toc_paragraph(&doc, control, n / 2), &destination)
        .expect("a middle entry");
}
