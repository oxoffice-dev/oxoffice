//! Block locations: every operation that inserts, copies, moves or removes
//! whole blocks reads a location the same way, in every story, inside block
//! content controls too, and a refusal names the route that works.

use std::io::Cursor;
use std::sync::OnceLock;

use oxoffice_docx::{
    CaptionOptions, ContentControlPlacement, ContentFragment, ContentLocation, Document,
    DocumentFragment, FragmentConflictPolicy, RunPosition, RunRange, StoryId, StoryItemKind,
    StoryKind,
};
use oxoffice_docx_oxml::text::CT_P;
use oxoffice_opc::OpcPackage;

use crate::support::{
    corpus_doc, doc_with_body, map_in_parallel, package_parts, reopen, text_box_run,
};

const W15: &str = "http://schemas.microsoft.com/office/word/2012/wordml";

/// A one-pixel PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

fn p(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

fn sdt(tag: &str, properties: &str, content: &str) -> String {
    format!(
        "<w:sdt><w:sdtPr><w:tag w:val=\"{tag}\"/>{properties}</w:sdtPr><w:sdtContent>{content}</w:sdtContent></w:sdt>"
    )
}

fn table(cell: &str) -> String {
    format!(
        "<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w=\"4000\"/></w:tblGrid><w:tr><w:tc><w:tcPr>\
         <w:tcW w:w=\"4000\" w:type=\"dxa\"/></w:tcPr>{cell}</w:tc></w:tr></w:tbl>"
    )
}

fn item(tag: &str, text: &str) -> String {
    let properties = format!("<w15:repeatingSectionItem xmlns:w15=\"{W15}\"/>");
    sdt(tag, &properties, &p(text))
}

/// `table` in a block control as Google Docs writes one: locked, alone.
fn google_table(n: usize, table: &str) -> String {
    format!(
        "<w:sdt><w:sdtPr><w:lock w:val=\"contentLocked\"/><w:id w:val=\"{n}\"/>\
         <w:tag w:val=\"goog_rdk_{n}\"/></w:sdtPr><w:sdtContent>{table}</w:sdtContent></w:sdt>"
    )
}

/// Every container a location can name: block controls three levels deep,
/// a cell holding a control and one whose only paragraph is in a control, a
/// text box, a plain text control, a repeating section, an empty control, an
/// inline control, a table alone in a locked control, a table then a
/// paragraph in one control, a table in a nested control; a header whose only
/// block is a control; a footnote and a comment.
fn fixture() -> Document {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    let bytes =
        BYTES.get_or_init(|| {
            let inner = [p("Inner 1"), sdt("deep", "", &p("Deep 1"))].concat();
            let section = format!("<w15:repeatingSection xmlns:w15=\"{W15}\"/>");
            let body = [
            p("Before"),
            sdt("box", "", &[p("Box 1"), sdt("inner", "", &inner), p("Box 2")].concat()),
            p("After"),
            table(&[p("Cell"), sdt("cellbox", "", &p("Cell box 1")), p("Cell end")].concat()),
            table(&sdt("onlybox", "", &p("Only"))),
            table(&[sdt("twoa", "", &p("A")), sdt("twob", "", &p("B"))].concat()),
            table(&[p("P"), sdt("qbox", "", &p("Q"))].concat()),
            format!("<w:p><w:r><w:t>Holder</w:t></w:r>{}</w:p>", text_box_run(7, &p("Text box"))),
            sdt("typed", "<w:text/>", &p("Plain")),
            sdt("section", &section, &[item("item", "Item 1"), item("item2", "Item 2")].concat()),
            "<w:sdt><w:sdtPr><w:tag w:val=\"empty\"/></w:sdtPr><w:sdtContent/></w:sdt>".to_owned(),
            "<w:p><w:r><w:t>Tail</w:t></w:r><w:sdt><w:sdtPr><w:tag w:val=\"inline\"/></w:sdtPr>\
             <w:sdtContent><w:r><w:t>!</w:t></w:r></w:sdtContent></w:sdt></w:p>"
                .to_owned(),
            google_table(1, &table(&p("Boxed"))),
            sdt("tpara", "", &[table(&p("Tab")), p("Para")].concat()),
            sdt("outer", "", &sdt("nest", "", &table(&p("Nested")))),
        ]
        .concat();
            let mut doc = doc_with_body(&body);
            doc.create_footnote(&location(&doc, "p:Tail!"), "Note")
                .expect("footnote");
            let start = RunPosition {
                body_index: 0,
                run_index: 0,
            };
            let end = RunPosition {
                run_index: 1,
                ..start
            };
            doc.add_comment(RunRange { start, end }, "Ada", None, "Remark")
                .expect("comment");
            doc.set_header("Head");
            with_header(
                doc,
                &sdt("headbox", "", &[p("Head box 1"), p("Head box 2")].concat()),
            )
        });
    Document::from_bytes(bytes).expect("fixture opens")
}

/// `doc` saved with the content of its header replaced by `content`.
fn with_header(doc: Document, content: &str) -> Vec<u8> {
    with_story(doc, StoryKind::Header, content)
}

/// `doc` saved with the content of its header or footer `kind` replaced by
/// `content`.
fn with_story(mut doc: Document, kind: StoryKind, content: &str) -> Vec<u8> {
    let header = story(&doc, kind).part_name().to_owned();
    let bytes = doc.to_bytes().expect("saves");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let xml = String::from_utf8(package.get_part(&header).expect("header").to_vec()).unwrap();
    let start = xml
        .find("<w:p>")
        .or_else(|| xml.find("<w:p "))
        .expect("a paragraph");
    let end = xml
        .rfind("</w:hdr>")
        .or_else(|| xml.rfind("</w:ftr>"))
        .expect("the part's end");
    package.set_part(
        &header,
        format!("{}{content}{}", &xml[..start], &xml[end..]).into_bytes(),
    );
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    let bytes = out.into_inner();
    Document::from_bytes(&bytes).expect("reopens");
    bytes
}

fn story(doc: &Document, kind: StoryKind) -> StoryId {
    doc.stories()
        .expect("stories")
        .into_iter()
        .find(|story| story.kind() == kind)
        .unwrap_or_else(|| panic!("a {kind:?} story"))
}

fn body(doc: &Document) -> StoryId {
    story(doc, StoryKind::Body)
}

fn same_story(left: &StoryId, right: &StoryId) -> bool {
    left.kind() == right.kind()
        && left.part_name() == right.part_name()
        && left.owner_index() == right.owner_index()
}

/// The blocks of `story` in order: the text of each paragraph, and
/// `table:TEXT` for a table outside its cells, so that a table copied, moved
/// or removed shows.
fn texts_of(doc: &Document, story: &StoryId) -> Vec<String> {
    use quick_xml::events::Event;
    let mut paragraphs = doc
        .story_paragraphs()
        .expect("story paragraphs")
        .into_iter()
        .filter(|paragraph| same_story(paragraph.location.story(), story))
        .map(|paragraph| paragraph.text.trim().to_owned());
    let xml = doc
        .element_extent(story, None)
        .expect("the story reads")
        .expect("the story owner")
        .xml;
    let mut reader = quick_xml::Reader::from_reader(xml.as_slice());
    let (mut texts, mut open, mut table) = (Vec::new(), Vec::<Vec<u8>>::new(), None::<String>);
    let mut buffer = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buffer).expect("story XML");
        // Below the owner, outside a paragraph, table, cell or text box.
        let block = |open: &[Vec<u8>]| {
            !open.is_empty()
                && !open[1..]
                    .iter()
                    .any(|name| matches!(name.as_slice(), b"p" | b"tbl" | b"tc" | b"txbxContent"))
        };
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                let name = element.local_name().as_ref().to_vec();
                if block(&open) && name == b"p" {
                    texts.push(paragraphs.next().expect("a story paragraph"));
                } else if block(&open) && name == b"tbl" {
                    table = Some(String::new());
                }
                if matches!(event, Event::Start(_)) {
                    open.push(name);
                }
            }
            Event::Text(text) if table.is_some() => {
                table
                    .as_mut()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(text));
            }
            Event::End(_) => {
                if open.pop().as_deref() == Some(b"tbl") && block(&open) {
                    texts.push(format!("table:{}", table.take().unwrap_or_default()));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    texts
}

fn body_texts(doc: &Document) -> Vec<String> {
    texts_of(doc, &body(doc))
}

/// The story and item index of the content control tagged `tag`.
fn control_index(doc: &Document, tag: &str) -> (StoryId, usize) {
    let needle = format!("<w:tag w:val=\"{tag}\"/>");
    doc.story_item_snapshots()
        .expect("story items")
        .into_iter()
        .find(|item| {
            let xml = String::from_utf8_lossy(item.xml());
            item.location().item_kind() == StoryItemKind::ContentControl
                && xml
                    .find("<w:tag ")
                    .is_some_and(|at| xml[at..].starts_with(&needle))
        })
        .map(|item| {
            (
                item.location().story().clone(),
                item.location().index_path()[0],
            )
        })
        .unwrap_or_else(|| panic!("control {tag}"))
}

/// The location `spec` names: `p:TEXT` the story paragraph whose text is
/// TEXT; `c:TAG` the control tagged TAG, `c:TAG/K` its paragraph K, `t:TAG/K`
/// its table K, `e:TAG` its end; `end@TEXT` the end of the story holding
/// paragraph TEXT; `after:TEXT` the story item after paragraph TEXT.
fn location(doc: &Document, spec: &str) -> ContentLocation {
    let (kind, rest) = spec.split_once(':').unwrap_or((spec, ""));
    let paragraph = |text: &str| {
        let found = doc
            .story_paragraphs()
            .expect("story paragraphs")
            .into_iter()
            .filter(|paragraph| paragraph.text.trim() == text)
            .map(|paragraph| paragraph.location)
            .collect::<Vec<_>>();
        assert_eq!(found.len(), 1, "one paragraph {text:?}");
        found.into_iter().next().unwrap()
    };
    match kind {
        "p" => paragraph(rest),
        "e" => {
            let (story, index) = control_index(doc, rest);
            ContentLocation::control_end(story, index)
        }
        "c" | "t" => {
            let (tag, k) = rest
                .split_once('/')
                .map_or((rest, None), |(t, k)| (t, k.parse().ok()));
            let (story, index) = control_index(doc, tag);
            match (kind, k) {
                ("c", None) => {
                    ContentLocation::new(story, StoryItemKind::ContentControl, vec![index])
                }
                ("c", Some(k)) => {
                    ContentLocation::new(story, StoryItemKind::Paragraph, vec![index, k])
                }
                (_, k) => {
                    ContentLocation::new(story, StoryItemKind::Table, vec![index, k.unwrap()])
                }
            }
        }
        "after" => {
            let at = paragraph(rest);
            let next = at.index_path()[0] + 1;
            let kind = doc
                .story_item_snapshots()
                .expect("story items")
                .into_iter()
                .find(|item| {
                    same_story(item.location().story(), at.story())
                        && item.location().index_path() == [next]
                })
                .expect("an item after it")
                .location()
                .item_kind();
            ContentLocation::new(at.story().clone(), kind, vec![next])
        }
        _ => {
            let text = spec.strip_prefix("end@").expect("a location spec");
            ContentLocation::end(paragraph(text).story().clone())
        }
    }
}

fn new_paragraph(text: &str) -> ContentFragment {
    let mut paragraph = CT_P::new();
    paragraph.add_run(text);
    ContentFragment::paragraph(paragraph).expect("paragraph fragment")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Insert,
    Import,
    Html,
    Caption,
    Picture,
    CloneFrom,
    CloneTo,
    Move,
    MoveTo,
    Remove,
    RangeFrom,
    RangeTo,
    /// Every operation, in a row's refusals.
    All,
}

const OPS: [Op; 12] = [
    Op::Insert,
    Op::Import,
    Op::Html,
    Op::Caption,
    Op::Picture,
    Op::CloneFrom,
    Op::CloneTo,
    Op::Move,
    Op::MoveTo,
    Op::Remove,
    Op::RangeFrom,
    Op::RangeTo,
];

type Refusals = &'static [(Op, &'static str)];

const END: &str = "an end is a boundary between blocks";
const ONLY: &str = "only block of content control";
const TYPED: &str = "set the control's value";
const SECTION: &str = "whose content holds only its items";
const ITEM: &str = "a repeating section item stays directly in the content of a repeating section";
const HTML: &str = "from_html";
const ENDS: Refusals = &[
    (Op::CloneFrom, END),
    (Op::Move, END),
    (Op::Remove, END),
    (Op::RangeFrom, END),
    (Op::Picture, END),
];
const INSERTS: [Op; 7] = [
    Op::Insert,
    Op::Import,
    Op::Html,
    Op::Caption,
    Op::Picture,
    Op::CloneTo,
    Op::MoveTo,
];

/// One location shape: its name, its location, the story whose paragraphs an
/// operation there changes, where an inserted paragraph lands among them, the
/// paragraphs of the block it names, the block before it in its container
/// and its paragraphs, the boundary after the block, and the operations
/// refused there with a part of each message.
type Shape = (
    &'static str,
    &'static str,
    &'static str,
    usize,
    Option<(usize, usize)>,
    Option<(&'static str, usize, usize)>,
    Option<&'static str>,
    Refusals,
);

#[rustfmt::skip]
fn shapes() -> Vec<Shape> {
    let refusals = |ops: &[Op], message: &'static str, more: &[(Op, &'static str)]| -> Refusals {
        Box::leak(ops.iter().map(|op| (*op, message)).chain(more.iter().copied()).collect::<Vec<_>>().into_boxed_slice())
    };
    let only: Refusals = &[(Op::Move, ONLY), (Op::Remove, ONLY)];
    let typed = refusals(&INSERTS, TYPED, only);
    let item = refusals(&INSERTS, SECTION, &[(Op::CloneFrom, ITEM), (Op::Move, ITEM), (Op::RangeFrom, ITEM), (Op::RangeTo, ITEM)]);
    let section_end = refusals(&INSERTS[..4], SECTION, &[(Op::CloneTo, SECTION), (Op::MoveTo, SECTION), (Op::RangeTo, ITEM)]);
    let section_end = refusals(&[], "", &[section_end, ENDS].concat());
    // Body blocks: Before, Box 1, Inner 1, Deep 1, Box 2, After, four cell tables, Holder, Plain, Item 1, Item 2,
    // Tail!, the tables Boxed and Tab, Para, the table Nested.
    vec![
        ("a body paragraph [i]", "p:After", "body", 5, Some((5, 6)), Some(("c:box", 1, 5)), Some("after:After"), &[]),
        ("a paragraph [c, k] of a body control", "c:box/1", "body", 4, Some((4, 5)), Some(("c:inner", 2, 4)), Some("e:box"), &[]),
        ("the end of a body control", "e:box", "body", 5, None, Some(("c:box/1", 4, 5)), None, ENDS),
        ("a paragraph [c, k] of a nested control", "c:inner/0", "body", 2, Some((2, 3)), None, Some("c:deep"), &[]),
        ("a nested block control [i]", "c:inner", "body", 2, Some((2, 4)), Some(("c:box/0", 1, 2)), Some("c:box/1"), &[]),
        ("the end of a nested control", "e:inner", "body", 4, None, Some(("c:deep", 3, 4)), None, ENDS),
        ("a control three levels deep [i]", "c:deep", "body", 3, Some((3, 4)), Some(("c:inner/0", 2, 3)), Some("e:inner"), &[]),
        ("a paragraph of a control three levels deep", "c:deep/0", "body", 3, Some((3, 4)), None, Some("e:deep"), only),
        ("the end of a control three levels deep", "e:deep", "body", 4, None, Some(("c:deep/0", 3, 4)), None, ENDS),
        ("the end of the body", "end@Before", "body", 19, None, Some(("c:outer", 18, 19)), None, ENDS),
        ("[c, n] past a control's paragraphs", "c:box/2", "body", 0, None, None, None, &[(Op::All, "control_end(story, ")]),
        ("a cell paragraph [i]", "p:Cell", "@Cell", 0, Some((0, 1)), None, Some("c:cellbox"), &[]),
        ("a paragraph of a control in a cell", "c:cellbox/0", "@Cell", 1, Some((1, 2)), None, Some("e:cellbox"), only),
        ("the end of a control in a cell", "e:cellbox", "@Cell", 2, None, Some(("c:cellbox/0", 1, 2)), None, ENDS),
        ("a control holding its cell's only paragraph", "c:onlybox", "@Only", 0, Some((0, 1)), None, Some("end@Only"), &[(Op::Remove, "only paragraph of its table cell"), (Op::MoveTo, ONLY)]),
        ("a control of a cell that holds two", "c:twoa", "@A", 0, Some((0, 1)), None, Some("c:twob"), &[(Op::MoveTo, ONLY)]),
        ("the end of a control after its cell's paragraph", "e:qbox", "@P", 2, None, Some(("c:qbox/0", 1, 2)), None, ENDS),
        ("a control that is its header's only block", "c:headbox", "@Head box 1", 0, Some((0, 2)), None, Some("end@Head box 1"), &[(Op::Remove, "only block of its header")]),
        ("a paragraph of a header control", "c:headbox/0", "@Head box 1", 0, Some((0, 1)), None, Some("c:headbox/1"), &[]),
        ("the end of a header control", "e:headbox", "@Head box 1", 2, None, Some(("c:headbox/1", 1, 2)), None, ENDS),
        ("a footnote paragraph [i]", "p:Note", "@Note", 0, Some((0, 1)), None, Some("end@Note"), &[(Op::Html, HTML), (Op::Remove, "only block of its footnote")]),
        ("a text box paragraph [i]", "p:Text box", "@Text box", 0, Some((0, 1)), None, Some("end@Text box"), &[(Op::Html, HTML), (Op::Remove, "only block of its text box")]),
        ("a comment paragraph [i]", "p:Remark", "@Remark", 0, Some((0, 1)), None, Some("end@Remark"), &[(Op::Html, HTML), (Op::Remove, "only block of its comment")]),
        ("the paragraph of a plain text control", "c:typed/0", "body", 11, Some((11, 12)), None, Some("e:typed"), typed),
        ("a repeating section item [i]", "c:item2", "body", 13, Some((13, 14)), Some(("c:item", 12, 13)), Some("e:section"), item),
        ("the end of a repeating section", "e:section", "body", 14, None, Some(("c:item2", 13, 14)), None, section_end),
        ("[c, 0] of a repeating section", "c:section/0", "body", 0, None, None, None, &[(Op::All, "name a paragraph of an item")]),
        ("the end of a repeating section item", "e:item", "body", 13, None, Some(("c:item/0", 12, 13)), None, ENDS),
        ("the end of an empty control", "e:empty", "body", 14, None, None, None, ENDS),
        ("[c, k] of an inline control", "c:inline/0", "body", 0, None, None, None, &[(Op::All, "inside a paragraph and holds no block")]),
        ("an inline control [i]", "c:inline", "body", 0, None, None, None, &[(Op::All, "not a block")]),
        ("a table [c, k] alone in a locked control", "t:goog_rdk_1/0", "body", 15, Some((15, 16)), None, Some("e:goog_rdk_1"), only),
        ("a table [c, k] then a paragraph in one control", "t:tpara/0", "body", 16, Some((16, 17)), None, Some("c:tpara/0"), &[]),
        ("a table [c, k] of a control nested in a control", "t:nest/0", "body", 18, Some((18, 19)), None, Some("e:nest"), only),
        ("[c, n] of kind table past a control's tables", "t:box/0", "body", 0, None, None, None, &[(Op::All, "control_end(story, ")]),
    ]
}

/// Run `op` at `shape` on `doc`. `None` when it does not apply: a range
/// from a block with no boundary after it in its container, or to one with
/// no block before it. A range at a shape refused everywhere starts and ends
/// at the shape. For a control, the content control call of the same
/// operation is run on a copy and must agree.
fn run(op: Op, shape: &Shape, doc: &mut Document, refused: bool) -> Option<Result<(), String>> {
    let at = location(doc, shape.1);
    let story = story_of(doc, shape.2);
    let end = ContentLocation::end(story.clone());
    // A block control: the content control call must agree.
    let control = at.item_kind() == StoryItemKind::ContentControl && shape.4.is_some();
    let twin = |call: &dyn Fn(&mut Document) -> oxoffice_docx::Result<()>| {
        control.then(|| {
            let mut twin = doc.clone_for_staging();
            call(&mut twin)
                .ok()
                .map(|()| texts_of(&twin, &refreshed(&twin, &story)))
        })
    };
    let twin = match op {
        Op::CloneFrom => twin(&|d| d.clone_content_control(&at, &end)),
        Op::Move => twin(&|d| d.move_content_control(&at, &end)),
        Op::Remove => twin(&|d| d.remove_content_control(&at)),
        _ => None,
    };
    let result = match op {
        Op::Insert => doc.insert_content(&at, new_paragraph("New")),
        Op::Import => {
            let start = location(doc, "p:Before");
            let fragment =
                DocumentFragment::from_range(doc, &start, &location(doc, "c:box"), false)
                    .expect("the fragment of Before");
            doc.import_fragment(&at, &fragment, FragmentConflictPolicy::default())
        }
        Op::Html => doc
            .insert_html_fragment(&at, "<p>Html</p>", &[])
            .map(|_| ()),
        Op::Caption | Op::Picture => {
            let inserted = if op == Op::Caption {
                let options = CaptionOptions {
                    text: "Cap".into(),
                    ..CaptionOptions::default()
                };
                doc.insert_caption(&at, &options)
                    .map(|target| target.paragraph)
            } else {
                doc.insert_picture_to_story(&story, Some(&at), PNG, "p.png", None, None)
            };
            // The location returned reads the paragraph inserted.
            inserted.map(|location| {
                let xml = doc
                    .paragraph_element_xml(&location)
                    .expect("the location reads");
                let xml = String::from_utf8_lossy(&xml);
                assert!(
                    xml.contains("Cap") || xml.contains("<w:drawing"),
                    "{}: {xml}",
                    shape.0
                );
            })
        }
        Op::CloneFrom => doc.clone_content(&at, &end),
        Op::CloneTo => doc.clone_content(&location(doc, "p:Before"), &at),
        Op::Move => doc.move_content(&at, &end),
        Op::MoveTo => {
            let first = doc
                .story_paragraphs()
                .expect("story paragraphs")
                .into_iter()
                .find(|paragraph| same_story(paragraph.location.story(), &story))
                .expect("a paragraph of the story");
            doc.move_content(&first.location, &at)
        }
        Op::Remove => doc.remove_content_at(&at).map(|_| ()),
        Op::RangeFrom | Op::RangeTo => {
            let other = match (op, shape.6, shape.5) {
                (Op::RangeFrom, Some(next), _) | (Op::RangeTo, _, Some((next, _, _))) => {
                    location(doc, next)
                }
                _ if refused => at.clone(),
                _ => return None,
            };
            let (start, stop) = if op == Op::RangeFrom {
                (&at, &other)
            } else {
                (&other, &at)
            };
            DocumentFragment::from_range(doc, start, stop, false).and_then(|fragment| {
                doc.import_fragment(&end, &fragment, FragmentConflictPolicy::default())
            })
        }
        Op::All => unreachable!(),
    };
    if let Some(twin) = twin {
        let ours = result
            .as_ref()
            .ok()
            .map(|()| texts_of(doc, &refreshed(doc, &story)));
        if twin != ours {
            return Some(Err(format!(
                "the content control call disagrees: {twin:?} and {ours:?}"
            )));
        }
    }
    Some(result.map_err(|error| error.to_string()))
}

/// `story` in `doc` after an edit, which changed its fingerprint.
fn refreshed(doc: &Document, story: &StoryId) -> StoryId {
    doc.stories()
        .expect("stories")
        .into_iter()
        .find(|candidate| same_story(candidate, story))
        .expect("the story")
}

fn story_of(doc: &Document, spec: &str) -> StoryId {
    match spec.strip_prefix('@') {
        Some(text) => location(doc, &format!("p:{text}")).story().clone(),
        None => body(doc),
    }
}

/// The story's paragraphs `op` at `shape` leaves.
fn expected(op: Op, shape: &Shape, before: &[String]) -> Vec<String> {
    let mut after = before.to_vec();
    let block = shape.4.map(|(start, end)| start..end).unwrap_or(0..0);
    let at = shape.3;
    match op {
        Op::Insert => after.insert(at, "New".into()),
        Op::Import | Op::CloneTo => after.insert(at, "Before".into()),
        Op::Html => after.insert(at, "Html".into()),
        Op::Caption => after.insert(at, String::new()),
        Op::Picture => after.insert(block.end, String::new()),
        Op::CloneFrom | Op::RangeFrom => after.extend(before[block].to_vec()),
        Op::Move => {
            after.drain(block.clone());
            after.extend(before[block].to_vec());
        }
        Op::MoveTo => {
            let first = after.remove(0);
            after.insert(at.saturating_sub(1), first);
        }
        Op::Remove => {
            after.drain(block);
        }
        Op::RangeTo => {
            let (_, start, end) = shape.5.expect("a block before");
            after.extend(before[start..end].to_vec());
        }
        Op::All => unreachable!(),
    }
    after
}

fn check(op: Op, shape: &Shape) -> Result<bool, String> {
    let mut doc = fixture();
    let story = story_of(&doc, shape.2);
    let before = texts_of(&doc, &story);
    let refusal = shape
        .7
        .iter()
        .find(|(refused, _)| *refused == op || *refused == Op::All)
        .map(|(_, message)| *message);
    let Some(result) = run(op, shape, &mut doc, refusal.is_some()) else {
        return Ok(false);
    };
    let story = refreshed(&doc, &story);
    match (result, refusal) {
        (Err(error), Some(message)) if error.contains(message) => (texts_of(&doc, &story)
            == before)
            .then_some(true)
            .ok_or_else(|| "a refusal changed the story".to_owned()),
        (Err(error), _) => Err(format!("refused: {error}")),
        (Ok(()), Some(message)) => Err(format!("works, expected a refusal naming {message:?}")),
        (Ok(()), None) => {
            let mut want = expected(op, shape, &before);
            let got = texts_of(&doc, &story);
            if op == Op::Caption && got.get(shape.3).is_some_and(|text| text.ends_with("Cap")) {
                want[shape.3] = got[shape.3].clone();
            }
            if got != want {
                return Err(format!("paragraphs {got:?}, expected {want:?}"));
            }
            let saved = doc.to_bytes().map_err(|error| format!("save: {error}"))?;
            let mut reopened = Document::from_bytes(&saved).map_err(|e| format!("reopen: {e}"))?;
            if texts_of(&reopened, &refreshed(&reopened, &story)) != want {
                return Err("the reopened document reads other paragraphs".to_owned());
            }
            if reopened
                .to_bytes()
                .map_err(|e| format!("second save: {e}"))?
                != saved
            {
                return Err("the second save writes other bytes".to_owned());
            }
            Ok(true)
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
    let (mut done, mut skipped, mut failures) = (0, 0, Vec::new());
    for ((op, shape), outcome) in cells.iter().zip(outcomes) {
        match outcome {
            Some(Ok(true)) => done += 1,
            Some(Ok(false)) => skipped += 1,
            Some(Err(failure)) => failures.push(format!("{op:?} at {}: {failure}", shape.0)),
            None => failures.push(format!("{op:?} at {}: panicked", shape.0)),
        }
    }
    let refused = shapes
        .iter()
        .flat_map(|shape| {
            OPS.iter()
                .filter(move |op| shape.7.iter().any(|(r, _)| r == *op || *r == Op::All))
        })
        .count();
    println!(
        "{} cells: {} work, {refused} refused with their route, {skipped} not applicable",
        cells.len(),
        done - refused
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The issue's body: one paragraph, a block control of two, one paragraph.
const ISSUE_BODY: &str = "<w:p><w:r><w:t>Before</w:t></w:r></w:p><w:sdt><w:sdtPr><w:tag w:val=\"box\"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Inside 1</w:t></w:r></w:p><w:p><w:r><w:t>Inside 2</w:t></w:r></w:p></w:sdtContent></w:sdt><w:p><w:r><w:t>After</w:t></w:r></w:p>";

fn refusal<T>(result: oxoffice_docx::Result<T>) -> String {
    result.map(|_| ()).expect_err("refused").to_string()
}

#[test]
fn a_range_in_a_block_control_ends_at_its_end_and_names_the_route() {
    let mut doc = doc_with_body(ISSUE_BODY);
    let range = |doc: &Document, start: &str, end: &str| {
        DocumentFragment::from_range(doc, &location(doc, start), &location(doc, end), false)
    };
    let routes = [
        (
            refusal(range(&doc, "c:box/1", "c:box/2")),
            "control_end(story, 1)",
        ),
        (
            refusal(range(&doc, "c:box/1", "p:After")),
            "control_end(story, 1)",
        ),
        (
            refusal(range(&doc, "p:Before", "c:box/1")),
            "the control that holds that end ([1])",
        ),
        (
            refusal(
                doc.clone_for_staging()
                    .move_content(&location(&doc, "c:box"), &location(&doc, "e:box")),
            ),
            "cannot move into itself",
        ),
    ];
    for (error, route) in routes {
        assert!(error.contains(route), "{error}");
    }
    // The routes work.
    range(&doc, "p:Before", "c:box").expect("up to the control");
    let fragment = range(&doc, "c:box/1", "e:box").expect("the last paragraph to the end");
    let at = location(&doc, "e:box");
    doc.import_fragment(&at, &fragment, FragmentConflictPolicy::default())
        .expect("imported");
    assert_eq!(
        body_texts(&reopen(&mut doc)),
        ["Before", "Inside 1", "Inside 2", "Inside 2", "After"]
    );
    // A fragment with the final section properties goes only to the body.
    let start = location(&doc, "p:Before");
    let whole = DocumentFragment::from_range(&doc, &start, &ContentLocation::end(body(&doc)), true)
        .expect("the body with its section");
    let at = location(&doc, "e:box");
    let error = refusal(doc.import_fragment(&at, &whole, FragmentConflictPolicy::default()));
    assert!(error.contains("at end(body)"), "{error}");
    let start = location(&doc, "c:box/0");
    let error = refusal(DocumentFragment::from_range(
        &doc,
        &start,
        &location(&doc, "e:box"),
        true,
    ));
    assert!(error.contains("main-body boundary"), "{error}");
    // An item is copied within its section.
    let mut doc = fixture();
    doc.clone_content(&location(&doc, "c:item"), &location(&doc, "e:section"))
        .expect("an item copied to its section's end");
    assert_eq!(&body_texts(&doc)[12..15], ["Item 1", "Item 2", "Item 1"]);
}

#[test]
fn bookmark_markers_inside_a_block_control_go_with_their_paragraph() {
    let marked = format!(
        "{}<w:bookmarkStart w:id=\"3\" w:name=\"heading\"/>{}<w:bookmarkEnd w:id=\"3\"/>",
        p("One"),
        p("Two")
    );
    let mut doc = doc_with_body(&format!("{}{}", sdt("box", "", &marked), p("After")));
    let names = |doc: &Document| {
        let bytes = doc.clone_for_staging().to_bytes().expect("saves");
        let xml = String::from_utf8(package_parts(&bytes)["word/document.xml"].clone()).unwrap();
        xml.match_indices("w:name=\"")
            .map(|(at, _)| xml[at + 8..].split('"').next().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    doc.clone_content(&location(&doc, "c:box/1"), &location(&doc, "e:box"))
        .expect("cloned with its bookmark");
    let cloned = names(&doc);
    assert!(cloned.len() == 2 && cloned[0] != cloned[1], "{cloned:?}");
    // An insertion goes before the bookmark start; a removal takes it.
    doc.insert_content(&location(&doc, "c:box/1"), new_paragraph("New"))
        .expect("inserted");
    doc.remove_content_at(&location(&doc, "c:box/2"))
        .expect("removed");
    assert_eq!(
        names(&doc).len(),
        1,
        "the removed paragraph took its bookmark"
    );
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
        let mut doc = doc_with_body(&format!(
            "{}{}",
            sdt("typed", &properties, &p("Value")),
            p("After")
        ));
        let result = doc.insert_content(&location(&doc, "e:typed"), new_paragraph("Second"));
        match result {
            Ok(()) => assert!(takes, "{kind}"),
            Err(error) => assert!(
                !takes && error.to_string().contains(&format!("is a {kind} control")),
                "{error}"
            ),
        }
    }
}

/// The table of contents control of a corpus thesis: its story item index,
/// its paragraphs' texts and its XML.
fn toc(doc: &Document) -> (usize, Vec<String>, String) {
    let item = doc
        .story_item_snapshots()
        .expect("story items")
        .into_iter()
        .find(|item| {
            String::from_utf8_lossy(item.xml()).contains("Table of Contents")
                && item.location().item_kind() == StoryItemKind::ContentControl
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
        let count =
            |xml: &str, kind: &str| xml.matches(&format!("w:fldCharType=\"{kind}\"")).count();
        // The first entry begins the TOC field and the last paragraph ends it.
        let first = usize::from(file.starts_with("word"));
        for k in [first, n - 1] {
            let error = refusal(doc.clone_content(
                &toc_paragraph(&doc, control, k),
                &toc_paragraph(&doc, control, n - 1),
            ));
            assert!(error.contains("of a complex field"), "{file} {k}: {error}");
        }
        // Before the paragraph that ends the field.
        doc.clone_content(
            &toc_paragraph(&doc, control, n - 2),
            &toc_paragraph(&doc, control, n - 1),
        )
        .unwrap_or_else(|error| panic!("{file}: {error}"));
        let mut reopened = reopen(&mut doc);
        let (_, after, xml_after) = toc(&reopened);
        assert_eq!(after, [&texts[..n - 1], &texts[n - 2..]].concat(), "{file}");
        assert_eq!(
            count(&xml_after, "begin") - count(&xml, "begin"),
            count(&xml_after, "end") - count(&xml, "end")
        );
        // The copy drops its paragraph identity, and every identity stays unique.
        let bytes = reopened.to_bytes().expect("saves");
        let document =
            String::from_utf8(package_parts(&bytes)["word/document.xml"].clone()).unwrap();
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
    for (xml, after_end) in [
        (field.to_owned(), "p:Tail"),
        (sdt("box", "", field), "e:box"),
    ] {
        let mut doc = doc_with_body(&format!("{xml}{}", p("Tail")));
        let before = body_texts(&doc);
        let end = ContentLocation::end(body(&doc));
        for (entry, held) in [("p:Entry 1", "the begin"), ("p:Entry 2", "the end")] {
            let at = location(&doc, entry);
            let clone = refusal(doc.clone_content(&at, &location(&doc, "p:Tail")));
            let moved = refusal(doc.move_content(&at, &end));
            for error in [clone, moved] {
                assert!(
                    error.contains(&format!("holds {held} of a complex field"))
                        && error.contains("from_range"),
                    "{error}"
                );
            }
        }
        let range = |doc: &Document, start: &str| {
            DocumentFragment::from_range(
                doc,
                &location(doc, start),
                &location(doc, after_end),
                false,
            )
        };
        assert!(refusal(range(&doc, "p:Entry 2")).contains("the range holds the end"));
        assert_eq!(body_texts(&doc), before, "the refusals change nothing");
        let fragment = range(&doc, "p:Entry 1").expect("the whole field");
        doc.import_fragment(&end, &fragment, FragmentConflictPolicy::default())
            .expect("imported");
        assert_eq!(
            body_texts(&reopen(&mut doc)),
            ["Entry 1", "Entry 2", "Tail", "Entry 1", "Entry 2"]
        );
    }
}

#[test]
fn a_table_of_authorities_in_a_block_control_keeps_its_categories_in_order() {
    let ta = |category: u8| {
        format!(
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> TA \l "Case {category}" \s "C{category}" \c {category} </w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
        )
    };
    let mut doc = doc_with_body(&format!(
        "<w:p>{}{}</w:p>{}",
        ta(1),
        ta(2),
        sdt("box", "", &[p("Inside 1"), p("Inside 2")].concat())
    ));
    let at = location(&doc, "c:box/1");
    let first = doc
        .insert_table_of_authorities(&at, &oxoffice_docx::TableOfAuthoritiesOptions::default())
        .expect("inserted in the control");
    assert_eq!(first.index_path(), at.index_path());
    let bytes = doc.to_bytes().expect("saves");
    let xml = String::from_utf8(package_parts(&bytes)["word/document.xml"].clone()).unwrap();
    let order = ["Inside 1", r"TOA \c 1", r"TOA \c 2", "Inside 2"]
        .map(|needle| xml.find(needle).unwrap_or_else(|| panic!("{needle}")));
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
}

#[test]
fn a_table_of_a_block_control_is_read_and_edited_by_its_location() {
    // Paragraph [c, 0] and table [c, 0] are both there: a reader counting
    // the wrong kind edits "Under". A direct table follows.
    let fixture = format!(
        "{}{}{}",
        p("Before"),
        sdt(
            "box",
            "",
            &format!(
                "{}<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w=\"2000\"/><w:gridCol w:w=\"2000\"/>\
                 </w:tblGrid><w:tr><w:tc>{}</w:tc><w:tc>{}</w:tc></w:tr></w:tbl>",
                p("Under"),
                p("Boxed"),
                p("Two")
            )
        ),
        table(&p("Direct"))
    );
    type Edit = fn(&mut Document, &ContentLocation) -> oxoffice_docx::Result<()>;
    let cases: [(&str, Edit, &str); 13] = [
        (
            "read",
            |doc, at| {
                assert_eq!(doc.read_story_table(at, |table| table.row_count())?, 1);
                let cell = doc.cell_element_xml(at, 0, 1)?;
                assert!(String::from_utf8_lossy(&cell).contains("Two"));
                let options = oxoffice_docx::RenderOptions::default();
                doc.measure_content(at, oxoffice_docx::Length::cm(10.0), options)?;
                let paragraph = ContentLocation::new(
                    at.story().clone(),
                    StoryItemKind::Paragraph,
                    at.index_path().to_vec(),
                );
                doc.measure_content(&paragraph, oxoffice_docx::Length::cm(10.0), options)
                    .map(|_| ())
            },
            "Boxed",
        ),
        (
            "scoped replacement",
            |doc, at| {
                assert_eq!(
                    doc.try_replace_text_at(at, "Boxed", "Swapped", Some(1))?,
                    Ok(1)
                );
                Ok(())
            },
            "Swapped",
        ),
        (
            "table XML",
            |doc, at| {
                let xml = String::from_utf8(doc.table_element_xml(at)?).unwrap();
                doc.replace_table_xml(at, &xml.replace("Boxed", "Swapped"))
            },
            "Swapped",
        ),
        (
            "cell XML",
            |doc, at| {
                let xml = String::from_utf8(doc.cell_element_xml(at, 0, 0)?).unwrap();
                doc.replace_cell_xml(at, 0, 0, &xml.replace("Boxed", "Swapped"))
            },
            "Swapped",
        ),
        (
            "table edit",
            |doc, at| doc.edit_story_table(at, |table| table.add_row().map(|_| ())),
            "</w:tr><w:tr><w:tc><w:p/>",
        ),
        (
            "row copy",
            |doc, at| doc.clone_story_table_row(at, 0, 1).map(|_| ()),
            "Boxed</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Two</w:t></w:r></w:p></w:tc></w:tr><w:tr>",
        ),
        (
            "column removal",
            |doc, at| doc.delete_story_table_column(at, 1),
            "Boxed</w:t></w:r></w:p></w:tc></w:tr>",
        ),
        (
            "row control",
            |doc, at| doc.insert_content_control(at, control_spec(ContentControlPlacement::Row)),
            "<w:tag w:val=\"placed\"/>",
        ),
        (
            "cell control",
            |doc, at| {
                let row = ContentLocation::new(
                    at.story().clone(),
                    StoryItemKind::Table,
                    [at.index_path(), &[0]].concat(),
                );
                doc.insert_content_control(&row, control_spec(ContentControlPlacement::Cell))
            },
            "<w:tag w:val=\"placed\"/>",
        ),
        (
            "row control clone",
            |doc, at| {
                doc.insert_content_control(at, control_spec(ContentControlPlacement::Row))?;
                let source = location(doc, "c:placed");
                doc.clone_content_control(&source, &location(doc, "t:box/0"))
            },
            "</w:sdtContent></w:sdt><w:sdt",
        ),
        (
            "cell control clone into a row, [c, k, r]",
            |doc, _| {
                let row = |doc: &Document| {
                    let table = location(doc, "t:box/0");
                    ContentLocation::new(
                        body(doc),
                        StoryItemKind::Table,
                        [table.index_path(), &[0]].concat(),
                    )
                };
                doc.insert_content_control(&row(doc), control_spec(ContentControlPlacement::Cell))?;
                let source = location(doc, "c:placed");
                doc.clone_content_control(&source, &row(doc))
            },
            "</w:sdtContent></w:sdt><w:sdt",
        ),
        (
            "row control moved in from a direct table",
            |doc, _| {
                let direct = doc
                    .story_tables(&body(doc))?
                    .last()
                    .cloned()
                    .expect("a table");
                doc.insert_content_control(&direct, control_spec(ContentControlPlacement::Row))?;
                let source = location(doc, "c:placed");
                doc.move_content_control(&source, &location(doc, "t:box/0"))
            },
            "<w:tag w:val=\"placed\"/>",
        ),
        (
            "cell control cloned to a direct table's row, [t, r]",
            |doc, at| {
                let row = ContentLocation::new(
                    body(doc),
                    StoryItemKind::Table,
                    [at.index_path(), &[0]].concat(),
                );
                doc.insert_content_control(&row, control_spec(ContentControlPlacement::Cell))?;
                let source = location(doc, "c:placed");
                let direct = doc
                    .story_tables(&body(doc))?
                    .last()
                    .cloned()
                    .expect("a table");
                let row = ContentLocation::new(
                    body(doc),
                    StoryItemKind::Table,
                    [direct.index_path(), &[0]].concat(),
                );
                doc.clone_content_control(&source, &row)?;
                let direct = doc
                    .story_tables(&body(doc))?
                    .last()
                    .cloned()
                    .expect("a table");
                let xml = String::from_utf8(doc.table_element_xml(&direct)?).unwrap();
                assert!(xml.contains("<w:tag w:val=\"placed\"/>"), "{xml}");
                Ok(())
            },
            "<w:tag w:val=\"placed\"/>",
        ),
    ];
    for (name, edit, expected) in cases {
        let mut doc = doc_with_body(&fixture);
        let at = location(&doc, "t:box/0");
        edit(&mut doc, &at).unwrap_or_else(|error| panic!("{name}: {error}"));
        let doc = reopen(&mut doc);
        let table =
            String::from_utf8(doc.table_element_xml(&location(&doc, "t:box/0")).unwrap()).unwrap();
        assert!(table.contains(expected), "{name}: {table}");
        assert_eq!(body_texts(&doc)[1], "Under", "{name}");
    }
    // A row is its table's location and the row index.
    let mut doc = doc_with_body(&fixture);
    let error = refusal(doc.insert_content_control(
        &location(&doc, "t:box/0"),
        control_spec(ContentControlPlacement::Cell),
    ));
    assert!(error.contains("[1, k, row]"), "{error}");
}

fn control_spec(placement: ContentControlPlacement) -> oxoffice_docx::ContentControlSpec {
    oxoffice_docx::ContentControlSpec {
        placement,
        kind: oxoffice_docx_oxml::content_control::SdtType::RichText,
        tag: Some("placed".to_owned()),
        alias: None,
        lock: None,
        appearance: None,
        placeholder: None,
    }
}

#[test]
fn a_table_in_a_block_control_is_listed_as_a_table_in_every_story() {
    // Google Docs tables: in the body, 2x2 in a body table's cell, in a footer
    // after a table.
    let two_by_two = format!(
        "<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w=\"2000\"/><w:gridCol w:w=\"2000\"/>\
         </w:tblGrid><w:tr><w:tc>{}</w:tc><w:tc>{}</w:tc></w:tr><w:tr><w:tc>{}</w:tc>\
         <w:tc>{}</w:tc></w:tr></w:tbl>",
        p("Inner {name}"),
        p("B"),
        p("C"),
        p("D")
    );
    let mut doc = doc_with_body(&format!(
        "{}{}{}{}",
        p("Before {name}"),
        google_table(1, &table(&p("A {name}"))),
        table(&format!("{}{}", p("Cell"), google_table(2, &two_by_two))),
        p("After {name}")
    ));
    doc.set_footer("Foot");
    let mut doc = Document::from_bytes(&with_story(
        doc,
        StoryKind::Footer,
        &format!(
            "{}{}",
            table(&p("Plain")),
            google_table(3, &table(&p("F {name}")))
        ),
    ))
    .expect("reopens");
    // Its cells are no paragraphs of the story around it.
    let paragraphs = doc
        .paragraphs()
        .iter()
        .map(|paragraph| paragraph.text())
        .collect::<Vec<_>>();
    assert_eq!(paragraphs, ["Before {name}", "After {name}"]);
    let visibility = doc.paragraph_visibility().expect("visibility");
    assert_eq!(
        visibility.len(),
        doc.paragraph_count(),
        "one entry a paragraph"
    );
    assert_eq!(
        body_texts(&doc),
        [
            "Before {name}",
            "table:A {name}",
            "table:CellInner {name}BCD",
            "After {name}"
        ]
    );
    let cell: Vec<String> = doc.tables()[1]
        .cell(0, 0)
        .expect("a cell")
        .paragraphs()
        .map(|paragraph| paragraph.text())
        .collect();
    assert_eq!(cell, ["Cell"]);
    // Every story lists it among its tables (a cell's item 0 is its `w:tcPr`),
    // and its location edits it.
    let body = body(&doc);
    let direct = ContentLocation::new(body.clone(), StoryItemKind::Table, vec![2]);
    let cell = doc.story_table_cell(&direct, 0, 0).expect("the cell story");
    let footer = story(&doc, StoryKind::Footer);
    let listed = [&body, &cell, &footer].map(|story| {
        doc.story_tables(story)
            .expect("tables")
            .iter()
            .map(|table| table.index_path().to_vec())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        listed,
        [
            vec![vec![1, 0], vec![2]],
            vec![vec![2, 0]],
            vec![vec![0], vec![1, 0]]
        ]
    );
    // The owner snapshots hold each one, placed among its story's tables by the
    // story-item tables before it, with its own cells (none under the table
    // around it), and its bytes among the items.
    let owners = doc.story_owner_snapshots().expect("owner snapshots");
    let elements = doc.owner_element_xml().expect("owner elements");
    for story in [&body, &cell, &footer] {
        let mut before = 0;
        let mut expected = Vec::new();
        for table in doc.story_tables(story).expect("tables") {
            match table.index_path() {
                [_] => before += 1,
                path => expected.push((path.to_vec(), before)),
            }
        }
        let held = owners
            .control_tables
            .iter()
            .filter(|(table, _, _)| same_story(table.story(), story))
            .map(|(table, before, _)| (table.index_path().to_vec(), *before))
            .collect::<Vec<_>>();
        assert_eq!(held, expected);
        for (path, _) in expected {
            let key = (
                story.kind(),
                story.part_name().to_owned(),
                story.owner_index(),
                path,
            );
            assert!(elements.items[&key].starts_with(b"<w:tbl"));
        }
    }
    let cells_of = |story: &StoryId, path: &[usize]| {
        owners
            .cells
            .iter()
            .filter(|(table, _, _, _)| {
                same_story(table.story(), story) && table.index_path() == path
            })
            .map(|(_, row, cell, _)| (*row, *cell))
            .collect::<Vec<_>>()
    };
    assert_eq!(cells_of(&body, &[2]), [(0, 0)]);
    assert_eq!(cells_of(&body, &[1, 0]), [(0, 0)]);
    assert_eq!(cells_of(&cell, &[2, 0]), [(0, 0), (0, 1), (1, 0), (1, 1)]);
    assert_eq!(cells_of(&footer, &[1, 0]), [(0, 0)]);
    assert_eq!(
        doc.table_story_location(0).unwrap().unwrap().index_path(),
        [1, 0]
    );
    assert_eq!(
        doc.table_story_location(1).unwrap().unwrap().index_path(),
        [2]
    );
    for (story, path) in [(body, vec![1, 0]), (cell, vec![2, 0]), (footer, vec![1, 0])] {
        let at = ContentLocation::new(refreshed(&doc, &story), StoryItemKind::Table, path);
        let replaced = doc.try_replace_text_at(&at, "{name}", "Ada", Some(1));
        assert_eq!(replaced.expect("replaced"), Ok(1));
    }
    // The lock stays as written.
    let bytes = reopen(&mut doc).to_bytes().expect("saves");
    let parts = package_parts(&bytes);
    let xml = parts
        .values()
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect::<String>();
    assert_eq!(xml.matches("<w:lock w:val=\"contentLocked\"/>").count(), 3);
    assert!(
        xml.contains("F Ada") && !xml.contains("{name}</w:t></w:r></w:p></w:tc>"),
        "{xml}"
    );
    let doc = Document::from_bytes(&bytes).expect("reopens");
    assert_eq!(
        body_texts(&doc),
        [
            "Before {name}",
            "table:A Ada",
            "table:CellInner AdaBCD",
            "After {name}"
        ]
    );
}
