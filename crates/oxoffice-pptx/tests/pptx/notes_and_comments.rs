//! Speaker notes (set, read, edit, clear, through slide reorder, duplication
//! and removal, on the decks of every corpus editor) and slide comments
//! (authors, comments, replies, resolution, removal, anchors, legacy parts).

use std::io::Cursor;
use std::sync::OnceLock;

use oxoffice_opc::OpcPackage;
use oxoffice_opc::relationship::rel_types;
use oxoffice_pptx::{Comment, CommentAuthor, CommentReply, Error, Presentation};

use crate::support::{corpus_deck, new_deck, reopen, slide_texts};

/// The speaker notes of the Highlights slide in every corpus deck.
const HIGHLIGHTS_NOTES: &str = "Start with the revenue story, then churn.";
const CORPUS_DECKS: [&str; 4] = [
    "powerpoint/deck.pptx",
    "google/deck.pptx",
    "libreoffice/deck.pptx",
    "authored/deck.pptx",
];

const ADA: &str = "{11111111-1111-1111-1111-111111111111}";
const GRACE: &str = "{22222222-2222-2222-2222-222222222222}";
const MONDAY: &str = "2026-11-16T09:30:00Z";
const TUESDAY: &str = "2026-11-17T14:05:10.250+01:00";

/// A braced GUID for comment and reply ids: `guid(7)`.
fn guid(n: u32) -> String {
    format!("{{00000000-0000-4000-8000-{n:012}}}")
}

/// A new deck with `count` blank slides.
fn blank_deck(count: usize) -> Presentation {
    let mut prs = new_deck();
    let blank = (0..prs.layout_count())
        .find(|&index| prs.layout_name(index) == Some("Blank"))
        .expect("the default template has a Blank layout");
    for _ in 0..count {
        prs.add_slide(blank).expect("blank slide");
    }
    prs
}

/// The notes of every slide, in order.
fn all_notes(prs: &Presentation) -> Vec<Option<String>> {
    prs.slides().map(|slide| slide.notes_text()).collect()
}

fn package_of(prs: &Presentation) -> OpcPackage {
    let bytes = prs.to_bytes().expect("deck saves");
    OpcPackage::from_reader(Cursor::new(bytes)).expect("saved package opens")
}

fn bytes_of(package: &OpcPackage) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    out.into_inner()
}

/// The zip entry names of the saved deck that start with `prefix` and end in
/// `.xml`, so relationship parts are left out.
fn parts_under(prs: &Presentation, prefix: &str) -> Vec<String> {
    package_of(prs)
        .zip_entries()
        .expect("package serialises")
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| name.starts_with(prefix) && name.ends_with(".xml"))
        .collect()
}

fn part_xml(prs: &Presentation, part_name: &str) -> String {
    let package = package_of(prs);
    let bytes = package
        .get_part(part_name)
        .unwrap_or_else(|| panic!("{part_name} is in the saved deck"));
    String::from_utf8(bytes.to_vec()).expect("part is UTF-8")
}

/// A deck with one blank slide and no notes master, as some generators write.
fn deck_without_notes_master() -> Presentation {
    let mut package = package_of(&blank_deck(1));
    let master = "/ppt/notesMasters/notesMaster1.xml";
    let master_theme = "/ppt/theme/theme2.xml";
    for part in [master, master_theme] {
        package.remove_part(part).expect("template part exists");
        package.remove_part_rels(part);
        package.content_types.remove_override(part);
    }
    package
        .get_part_rels_mut("/ppt/presentation.xml")
        .expect("presentation relationships")
        .items
        .retain(|relationship| relationship.rel_type != rel_types::NOTES_MASTER);
    Presentation::from_bytes(&bytes_of(&package)).expect("deck without notes master opens")
}

fn author(id: &str, name: &str, initials: Option<&str>) -> CommentAuthor {
    let user = name
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_lowercase();
    CommentAuthor::new(id, name, initials, format!("{user}@example.com"), "None")
        .expect("valid author")
}

fn comment(id: &str, author_id: &str, created: &str, text: &str) -> Comment {
    Comment::new(id, author_id, created, text).expect("valid comment")
}

fn reply(id: &str, author_id: &str, created: &str, text: &str) -> CommentReply {
    CommentReply::new(id, author_id, created, text).expect("valid reply")
}

/// A deck of three blank slides with Ada and Grace as comment authors and no
/// comment yet. Built once: every comment edit saves and reopens the deck.
fn reviewed_deck() -> Presentation {
    static DECK: OnceLock<Presentation> = OnceLock::new();
    DECK.get_or_init(|| {
        let mut prs = blank_deck(3);
        prs.add_comment_author(author(ADA, "Ada Lovelace", Some("AL")))
            .unwrap();
        prs.add_comment_author(author(GRACE, "Grace Hopper", Some("GH")))
            .unwrap();
        prs
    })
    .clone()
}

fn comment_texts(prs: &Presentation, slide: usize) -> Vec<String> {
    prs.comments(slide)
        .unwrap_or_default()
        .iter()
        .map(Comment::text)
        .collect()
}

fn reply_texts(comment: &Comment) -> Vec<String> {
    comment.replies().iter().map(CommentReply::text).collect()
}

/// The bytes of a one-slide deck whose comment parts are shaped like
/// PowerPoint 365's: a `modernComment_*` part with a slide anchor, a position
/// and a reply, and an author with a PowerPoint user id. `created` is the
/// date written on the comment and its reply.
fn powerpoint_comment_deck(created: &str) -> Vec<u8> {
    const AUTHOR: &str = "{CD37207E-7903-4ED4-8AE8-017538D2DF7E}";
    const COMMENT: &str = "{62A8A96D-E5A8-4BFC-B993-A6EAE3907CAD}";
    const REPLY: &str = "{0F8C8C0A-5D35-4F7E-9B53-3C1D2A9A6E11}";
    let mut prs = blank_deck(1);
    prs.add_comment_author(author(AUTHOR, "placeholder", None))
        .unwrap();
    prs.add_comment(0, comment(COMMENT, AUTHOR, MONDAY, "placeholder"))
        .unwrap();
    let slide_id = prs.slide(0).unwrap().id();
    let mut package = package_of(&prs);

    package.set_part(
        "/ppt/authors.xml",
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p188:authorLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p188="http://schemas.microsoft.com/office/powerpoint/2018/8/main"><p188:author id="{AUTHOR}" name="Grace Hopper" initials="GH" userId="S::grace@example.com::5b7f2c9e-1d4a-4c39-8f0e-2a6b1c3d4e5f" providerId="AD"/></p188:authorLst>"#
        )
        .into_bytes(),
    );
    let modern = "/ppt/comments/modernComment_100_4F1B2C3D.xml";
    package.remove_part("/ppt/comments/comment1.xml");
    package
        .content_types
        .remove_override("/ppt/comments/comment1.xml");
    package
        .content_types
        .add_override(modern, "application/vnd.ms-powerpoint.comments+xml");
    package
        .get_part_rels_mut("/ppt/slides/slide1.xml")
        .expect("slide relationships")
        .items
        .iter_mut()
        .find(|relationship| relationship.rel_type == rel_types::POWERPOINT_COMMENTS)
        .expect("comment relationship")
        .target = "../comments/modernComment_100_4F1B2C3D.xml".to_owned();
    package.set_part(
        modern,
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p188:cmLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p188="http://schemas.microsoft.com/office/powerpoint/2018/8/main"><p188:cm id="{COMMENT}" authorId="{AUTHOR}" created="{created}"><pc:sldMkLst xmlns:pc="http://schemas.microsoft.com/office/powerpoint/2013/main/command"><pc:docMk/><pc:sldMk cId="3456789012" sldId="{slide_id}"/></pc:sldMkLst><p188:pos x="10668000" y="152400"/><p188:replyLst><p188:reply id="{REPLY}" authorId="{AUTHOR}" created="{created}"><p188:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>Done in v2.</a:t></a:r></a:p></p188:txBody></p188:reply></p188:replyLst><p188:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>Needs a source for this figure.</a:t></a:r></a:p></p188:txBody></p188:cm></p188:cmLst>"#
        )
        .into_bytes(),
    );
    bytes_of(&package)
}

/// A deck of `slides` blank slides whose first slide carries one legacy
/// comment (`p:cmLst`, as PowerPoint 2007 to 2019 wrote them) by a legacy
/// author.
fn deck_with_legacy_comments(slides: usize) -> Presentation {
    let mut package = package_of(&blank_deck(slides));
    package.set_part(
        "/ppt/commentAuthors.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:cmAuthorLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cmAuthor id="1" name="Grace Hopper" initials="GH" lastIdx="1" clrIdx="0"/></p:cmAuthorLst>"#
            .to_vec(),
    );
    package.set_part(
        "/ppt/comments/comment1.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:cmLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cm authorId="1" dt="2016-03-04T10:20:30.000" idx="1"><p:pos x="10" y="10"/><p:text>Legacy remark on the opening slide</p:text></p:cm></p:cmLst>"#
            .to_vec(),
    );
    package.content_types.add_override(
        "/ppt/commentAuthors.xml",
        "application/vnd.openxmlformats-officedocument.presentationml.commentAuthors+xml",
    );
    package.content_types.add_override(
        "/ppt/comments/comment1.xml",
        "application/vnd.openxmlformats-officedocument.presentationml.comments+xml",
    );
    package
        .get_or_create_part_rels("/ppt/presentation.xml")
        .add(
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/commentAuthors",
            "commentAuthors.xml",
        );
    package
        .get_or_create_part_rels("/ppt/slides/slide1.xml")
        .add(rel_types::COMMENTS, "../comments/comment1.xml");
    Presentation::from_bytes(&bytes_of(&package)).expect("deck with legacy comments opens")
}

// Speaker notes.

#[test]
fn a_new_slide_has_no_notes_until_some_are_set() {
    let mut prs = blank_deck(2);
    assert_eq!(all_notes(&prs), [None, None]);

    prs.set_notes_text(1, "Thank the team.").unwrap();
    assert_eq!(all_notes(&prs), [None, Some("Thank the team.".to_owned())]);

    let reopened = reopen(&prs);
    assert_eq!(
        all_notes(&reopened),
        [None, Some("Thank the team.".to_owned())]
    );
    assert!(reopened.validate().is_empty(), "{:?}", reopened.validate());
}

#[test]
fn notes_on_a_deck_without_a_notes_master_bring_one() {
    let mut prs = deck_without_notes_master();
    assert!(parts_under(&prs, "ppt/notesMasters/").is_empty());

    prs.set_notes_text(0, "Speak slowly.").unwrap();

    let reopened = reopen(&prs);
    assert_eq!(
        reopened.slide(0).unwrap().notes_text().as_deref(),
        Some("Speak slowly.")
    );
    assert_eq!(parts_under(&reopened, "ppt/notesMasters/").len(), 1);
    assert!(reopened.validate().is_empty(), "{:?}", reopened.validate());
}

#[test]
fn editing_notes_replaces_them_in_the_same_notes_page() {
    let mut prs = blank_deck(1);
    prs.set_notes_text(0, "Draft speaker note").unwrap();
    let mut prs = reopen(&prs);

    prs.set_notes_text(0, "Final speaker note").unwrap();

    let reopened = reopen(&prs);
    assert_eq!(
        reopened.slide(0).unwrap().notes_text().as_deref(),
        Some("Final speaker note")
    );
    assert_eq!(parts_under(&reopened, "ppt/notesSlides/").len(), 1);
}

#[test]
fn cleared_notes_read_back_empty_not_missing() {
    let mut prs = blank_deck(1);
    prs.set_notes_text(0, "Remove me").unwrap();

    prs.set_notes_text(0, "").unwrap();

    assert_eq!(
        reopen(&prs).slide(0).unwrap().notes_text().as_deref(),
        Some("")
    );
}

#[test]
fn multi_paragraph_notes_keep_every_line_and_blank_line() {
    let text = "Opening\n\nFirst point\nSecond point\n";
    let mut prs = blank_deck(1);

    prs.set_notes_text(0, text).unwrap();

    assert_eq!(
        reopen(&prs).slide(0).unwrap().notes_text().as_deref(),
        Some(text)
    );
}

#[test]
fn notes_keep_unicode_markup_characters_and_spacing_as_typed() {
    let texts = [
        "Café, naïve, Œuvre, Straße",
        "مرحبا بالعالم: اعرض الإيرادات أولا",
        "שלום, תודה על ההקשבה",
        "先讲收入，再讲流失。売上から始めます。",
        "Rocket 🚀 and family 👩‍👩‍👧",
        "<b>bold?</b> & \"quotes\" 'too' &amp;",
        "  indented\tand tabbed  ",
    ];
    let mut prs = blank_deck(1);
    prs.set_notes_text(0, "").unwrap();
    for text in texts {
        prs.set_notes_text(0, text).unwrap();
        assert_eq!(
            reopen(&prs).slide(0).unwrap().notes_text().as_deref(),
            Some(text),
            "{text:?}"
        );
    }
}

// A raw carriage return would reach a conforming XML reader (PowerPoint's,
// python-pptx's) as a second line feed.
#[test]
fn notes_with_a_windows_line_ending_are_saved_so_that_other_readers_keep_them() {
    let mut prs = blank_deck(1);
    prs.set_notes_text(0, "Windows line\r\nending").unwrap();
    assert_eq!(
        reopen(&prs).slide(0).unwrap().notes_text().as_deref(),
        Some("Windows line\r\nending")
    );
    let notes = parts_under(&prs, "ppt/notesSlides/");
    assert_eq!(notes.len(), 1);
    let xml = part_xml(&prs, &format!("/{}", notes[0]));
    assert!(
        !xml.contains('\r'),
        "the carriage return is escaped in the saved notes"
    );
}

#[test]
fn the_slide_handle_edits_notes_but_only_the_presentation_creates_them() {
    let mut prs = blank_deck(2);
    let before = prs.to_bytes().unwrap();

    let refused = prs
        .slide_mut(0)
        .unwrap()
        .set_notes_text("Created through the slide")
        .unwrap_err();
    assert!(refused.to_string().contains("no notes part"), "{refused}");
    assert!(matches!(
        prs.set_notes_text(2, "No such slide"),
        Err(Error::UnknownSlideIndex {
            index: 2,
            slide_count: 2
        })
    ));
    assert_eq!(prs.to_bytes().unwrap(), before, "refusals change nothing");

    prs.set_notes_text(0, "Created").unwrap();
    prs.slide_mut(0)
        .unwrap()
        .set_notes_text("Edited through the slide")
        .unwrap();
    assert_eq!(
        reopen(&prs).slide(0).unwrap().notes_text().as_deref(),
        Some("Edited through the slide")
    );
}

#[test]
fn notes_follow_their_slide_when_slides_are_reordered() {
    let mut prs = blank_deck(3);
    prs.set_notes_text(0, "Notes of the first slide").unwrap();
    prs.set_notes_text(2, "Notes of the last slide").unwrap();

    prs.move_slide(0, 2).unwrap();

    assert_eq!(
        all_notes(&reopen(&prs)),
        [
            None,
            Some("Notes of the last slide".to_owned()),
            Some("Notes of the first slide".to_owned()),
        ]
    );
}

#[test]
fn a_duplicated_slide_gets_its_own_copy_of_the_notes() {
    for name in CORPUS_DECKS {
        let mut prs = corpus_deck(name);
        let mut expected = all_notes(&prs);

        prs.duplicate_slide(1).unwrap();
        assert_eq!(prs.slide(2).unwrap().notes_text(), expected[1], "{name}");
        prs.set_notes_text(2, "Only the copy").unwrap();
        expected.insert(2, Some("Only the copy".to_owned()));

        let reopened = reopen(&prs);
        assert_eq!(all_notes(&reopened), expected, "{name}");
        assert!(
            reopened.validate().is_empty(),
            "{name}: {:?}",
            reopened.validate()
        );
    }
}

#[test]
fn removing_a_slide_removes_its_notes_and_keeps_the_others() {
    let mut prs = blank_deck(3);
    for (index, text) in ["one", "two", "three"].into_iter().enumerate() {
        prs.set_notes_text(index, text).unwrap();
    }

    prs.remove_slide(1).unwrap();

    let reopened = reopen(&prs);
    assert_eq!(
        all_notes(&reopened),
        [Some("one".to_owned()), Some("three".to_owned())]
    );
    assert_eq!(parts_under(&reopened, "ppt/notesSlides/").len(), 2);
    assert!(reopened.validate().is_empty(), "{:?}", reopened.validate());
}

#[test]
fn find_and_replace_reaches_speaker_notes() {
    let mut prs = corpus_deck("powerpoint/deck.pptx");

    let count = prs.replace_text("revenue story", "growth story");

    assert_eq!(count, 1);
    assert_eq!(
        reopen(&prs).slide(1).unwrap().notes_text().as_deref(),
        Some("Start with the growth story, then churn.")
    );
}

#[test]
fn the_highlights_notes_read_the_same_from_every_editor() {
    let highlights = Some(HIGHLIGHTS_NOTES.to_owned());
    for name in CORPUS_DECKS {
        let prs = corpus_deck(name);
        // Google Slides writes a notes page, empty or not, for every slide.
        let other = (name == "google/deck.pptx").then(String::new);
        assert_eq!(
            all_notes(&prs),
            [other.clone(), highlights.clone(), other.clone(), other],
            "{name}"
        );
    }
}

#[test]
fn edited_highlights_notes_round_trip_for_every_editor() {
    let edited = "Lead with churn, then revenue.\nClose on the Lyon warehouse.";
    for name in CORPUS_DECKS {
        let mut prs = corpus_deck(name);
        let mut expected = all_notes(&prs);
        let slides = slide_texts(&prs);

        prs.set_notes_text(1, edited).unwrap();
        expected[1] = Some(edited.to_owned());

        let reopened = reopen(&prs);
        assert_eq!(all_notes(&reopened), expected, "{name}");
        assert_eq!(slide_texts(&reopened), slides, "{name}: slide text kept");
    }
}

#[test]
fn notes_added_to_a_corpus_slide_reuse_the_deck_notes_master() {
    for name in [
        "powerpoint/deck.pptx",
        "libreoffice/deck.pptx",
        "authored/deck.pptx",
    ] {
        let mut prs = corpus_deck(name);
        assert_eq!(prs.slide(0).unwrap().notes_text(), None, "{name}");

        prs.set_notes_text(0, "Welcome everyone.").unwrap();

        let reopened = reopen(&prs);
        assert_eq!(
            all_notes(&reopened)[..2],
            [
                Some("Welcome everyone.".to_owned()),
                Some(HIGHLIGHTS_NOTES.to_owned())
            ],
            "{name}"
        );
        assert_eq!(
            parts_under(&reopened, "ppt/notesMasters/").len(),
            1,
            "{name}"
        );
    }
}

#[test]
fn edited_libreoffice_notes_keep_their_font() {
    let mut prs = corpus_deck("libreoffice/deck.pptx");

    prs.set_notes_text(1, "First line\nSecond line").unwrap();

    // The facade has no reader for notes formatting: every new run keeps the
    // 20 pt Arial that LibreOffice wrote on the original one.
    let xml = part_xml(&prs, "/ppt/notesSlides/notesSlide2.xml");
    for line in ["First line", "Second line"] {
        let run = xml
            .split("<a:r>")
            .find(|run| run.contains(&format!("<a:t>{line}</a:t>")))
            .unwrap_or_else(|| panic!("a run holds {line:?}: {xml}"));
        assert!(run.contains(r#"sz="2000""#), "{run}");
        assert!(run.contains(r#"typeface="Arial""#), "{run}");
    }
}

// Comments.

#[test]
fn a_comment_reads_back_with_its_author_initials_text_and_date() {
    let mut prs = blank_deck(2);
    assert!(prs.comment_authors().is_empty());
    assert!(prs.comments(0).is_none());

    prs.add_comment_author(author(ADA, "Ada Lovelace", Some("AL")))
        .unwrap();
    prs.add_comment(1, comment(&guid(1), ADA, MONDAY, "Is this figure final?"))
        .unwrap();

    let reopened = reopen(&prs);
    assert!(reopened.comments(0).unwrap_or_default().is_empty());
    let [remark] = reopened.comments(1).unwrap() else {
        panic!("one comment on the second slide");
    };
    assert_eq!(remark.id, guid(1));
    assert_eq!(remark.text(), "Is this figure final?");
    assert_eq!(remark.created, MONDAY);
    assert_eq!(remark.status, None);
    assert!(remark.replies().is_empty());
    let writer = reopened
        .comment_authors()
        .iter()
        .find(|listed| listed.id == remark.author_id)
        .expect("the comment author is listed");
    assert_eq!(writer.name, "Ada Lovelace");
    assert_eq!(writer.initials.as_deref(), Some("AL"));
    assert_eq!(writer.user_id, "ada@example.com");
    assert_eq!(writer.provider_id, "None");
}

#[test]
fn comment_authors_keep_their_order_and_details() {
    let mut prs = blank_deck(1);
    let authors = [
        author(GRACE, "Grace Hopper", Some("GH")),
        author(ADA, "Zoë Ångström", None),
        author(&guid(3), "李雷", Some("李")),
    ];
    for author in &authors {
        prs.add_comment_author(author.clone()).unwrap();
    }

    assert_eq!(reopen(&prs).comment_authors(), authors);
}

#[test]
fn refused_comment_edits_leave_the_deck_as_it_was() {
    let mut prs = reviewed_deck();
    prs.add_comment(0, comment(&guid(1), ADA, MONDAY, "First"))
        .unwrap();
    let before = prs.to_bytes().unwrap();

    let refusals = [
        prs.add_comment_author(author(ADA, "Another Ada", None)),
        prs.add_comment(0, comment(&guid(2), &guid(99), MONDAY, "Unknown author")),
        prs.add_comment(1, comment(&guid(1), ADA, MONDAY, "Id taken on slide 0")),
        prs.reply_to_comment(0, &guid(1), reply(&guid(1), GRACE, MONDAY, "Same id")),
        prs.reply_to_comment(0, &guid(42), reply(&guid(3), GRACE, MONDAY, "No thread")),
        prs.remove_comment(0, &guid(42)),
        prs.remove_comment(1, &guid(1)),
    ];
    for (index, refusal) in refusals.into_iter().enumerate() {
        assert!(
            matches!(refusal, Err(Error::InvalidPresentationMutation { .. })),
            "refusal {index}: {refusal:?}"
        );
    }
    assert!(matches!(
        prs.add_comment(3, comment(&guid(5), ADA, MONDAY, "No slide")),
        Err(Error::UnknownSlideIndex { index: 3, .. })
    ));
    assert_eq!(prs.to_bytes().unwrap(), before);
}

#[test]
fn comment_ids_authors_and_dates_must_be_well_formed() {
    assert!(CommentAuthor::new("ada", "Ada", None, "ada", "None").is_err());
    assert!(CommentAuthor::new(ADA, "Ada\u{7}", None, "ada", "None").is_err());
    assert!(Comment::new("comment-1", ADA, MONDAY, "text").is_err());
    assert!(Comment::new(guid(1), ADA, "2026-11-16", "text").is_err());
    assert!(Comment::new(guid(1), ADA, "16/11/2026 09:30", "text").is_err());
    assert!(CommentReply::new(guid(1), ADA, "yesterday", "text").is_err());
    assert!(Comment::new(guid(1), ADA, TUESDAY, "an offset date is fine").is_ok());
}

#[test]
fn replies_keep_their_order_authors_and_dates() {
    let mut prs = reviewed_deck();
    prs.add_comment(0, comment(&guid(1), ADA, MONDAY, "Check the totals"))
        .unwrap();
    prs.reply_to_comment(0, &guid(1), reply(&guid(2), GRACE, TUESDAY, "Checked"))
        .unwrap();
    prs.reply_to_comment(0, &guid(1), reply(&guid(3), ADA, TUESDAY, "Thanks!"))
        .unwrap();

    let mut prs = reopen(&prs);
    let thread = &prs.comments(0).unwrap()[0];
    assert_eq!(reply_texts(thread), ["Checked", "Thanks!"]);
    let first = &thread.replies()[0];
    assert_eq!(
        (
            first.id.as_str(),
            first.author_id.as_str(),
            first.created.as_str()
        ),
        (guid(2).as_str(), GRACE, TUESDAY)
    );

    prs.move_reply(0, &guid(1), 1, 0).unwrap();
    assert_eq!(
        reply_texts(&reopen(&prs).comments(0).unwrap()[0]),
        ["Thanks!", "Checked"]
    );
}

#[test]
fn comments_on_a_slide_keep_their_order_and_can_be_reordered() {
    let mut prs = reviewed_deck();
    for (n, text) in [(1, "first"), (2, "second"), (3, "third")] {
        prs.add_comment(0, comment(&guid(n), ADA, MONDAY, text))
            .unwrap();
    }
    let mut prs = reopen(&prs);
    assert_eq!(comment_texts(&prs, 0), ["first", "second", "third"]);

    prs.move_comment(0, 2, 0).unwrap();

    assert_eq!(
        comment_texts(&reopen(&prs), 0),
        ["third", "first", "second"]
    );
}

#[test]
fn resolving_a_comment_marks_its_thread_and_nothing_else() {
    let mut prs = reviewed_deck();
    prs.add_comment(0, comment(&guid(1), ADA, MONDAY, "Fix the legend"))
        .unwrap();
    prs.add_comment(0, comment(&guid(2), ADA, MONDAY, "Open question"))
        .unwrap();
    prs.reply_to_comment(0, &guid(1), reply(&guid(3), GRACE, TUESDAY, "Fixed"))
        .unwrap();

    assert!(
        prs.resolve_comment(0, &guid(3)).is_err(),
        "a reply has no status of its own"
    );
    prs.resolve_comment(0, &guid(1)).unwrap();

    let reopened = reopen(&prs);
    let comments = reopened.comments(0).unwrap();
    assert_eq!(comments[0].status.as_deref(), Some("resolved"));
    assert_eq!(comments[0].replies()[0].status, None);
    assert_eq!(comments[1].status, None);
}

#[test]
fn removing_a_reply_or_a_thread_keeps_the_rest() {
    let mut prs = reviewed_deck();
    prs.add_comment(0, comment(&guid(1), ADA, MONDAY, "Thread"))
        .unwrap();
    prs.add_comment(0, comment(&guid(2), GRACE, MONDAY, "Other"))
        .unwrap();
    for (n, text) in [(3, "keep me"), (4, "drop me")] {
        prs.reply_to_comment(0, &guid(1), reply(&guid(n), GRACE, TUESDAY, text))
            .unwrap();
    }

    prs.remove_comment(0, &guid(4)).unwrap();
    let mut prs = reopen(&prs);
    assert_eq!(comment_texts(&prs, 0), ["Thread", "Other"]);
    assert_eq!(reply_texts(&prs.comments(0).unwrap()[0]), ["keep me"]);

    prs.remove_comment(0, &guid(1)).unwrap();
    assert!(
        prs.remove_comment(0, &guid(3)).is_err(),
        "the replies went with their thread"
    );
    let mut prs = reopen(&prs);
    assert_eq!(comment_texts(&prs, 0), ["Other"]);

    prs.remove_comment(0, &guid(2)).unwrap();
    let reopened = reopen(&prs);
    assert!(reopened.comments(0).unwrap_or_default().is_empty());
    assert!(reopened.validate().is_empty(), "{:?}", reopened.validate());
}

#[test]
fn comment_text_keeps_unicode_and_line_breaks() {
    let texts = [
        "Vérifier le chiffre d'affaires",
        "يرجى مراجعة هذا الرقم",
        "这个数字需要来源。",
        "Ship it 🚢",
        "Two points:\n\n1. sources\n2. dates",
        "",
    ];
    let mut prs = reviewed_deck();
    for (n, text) in (1..).zip(texts) {
        prs.add_comment(0, comment(&guid(n), ADA, MONDAY, text))
            .unwrap();
    }

    assert_eq!(comment_texts(&reopen(&prs), 0), texts);
}

#[test]
fn comments_stay_with_their_slide_through_a_reorder() {
    let mut prs = reviewed_deck();
    prs.add_comment(0, comment(&guid(1), ADA, MONDAY, "On the title"))
        .unwrap();
    prs.add_comment(2, comment(&guid(2), GRACE, MONDAY, "On the end"))
        .unwrap();
    let reopened = reopen(&prs);
    assert_eq!(comment_texts(&reopened, 0), ["On the title"]);
    assert!(reopened.comments(1).is_none());
    assert_eq!(comment_texts(&reopened, 2), ["On the end"]);

    prs.move_slide(2, 0).unwrap();

    let reopened = reopen(&prs);
    assert_eq!(comment_texts(&reopened, 0), ["On the end"]);
    assert_eq!(comment_texts(&reopened, 1), ["On the title"]);
    assert!(reopened.comments(2).is_none());
}

#[test]
fn removing_a_commented_slide_removes_only_its_comments() {
    let mut prs = reviewed_deck();
    prs.add_comment(0, comment(&guid(1), ADA, MONDAY, "Goes with the slide"))
        .unwrap();
    prs.add_comment(1, comment(&guid(2), GRACE, MONDAY, "Stays"))
        .unwrap();

    prs.remove_slide(0).unwrap();

    let reopened = reopen(&prs);
    assert_eq!(comment_texts(&reopened, 0), ["Stays"]);
    assert_eq!(parts_under(&reopened, "ppt/comments/").len(), 1);
    assert_eq!(reopened.comment_authors().len(), 2);
    assert!(reopened.validate().is_empty(), "{:?}", reopened.validate());
}

// Duplicating a commented slide is refused today (a strict known failure in
// slides_and_layouts); importing it from a clone is the workaround.
#[test]
fn a_commented_slide_can_be_copied_by_importing_it_from_a_clone() {
    let mut prs = reviewed_deck();
    prs.add_comment(0, comment(&guid(1), GRACE, MONDAY, "Travels with the copy"))
        .unwrap();
    prs.reply_to_comment(0, &guid(1), reply(&guid(2), ADA, TUESDAY, "Noted"))
        .unwrap();
    let source = prs.clone();

    prs.import_slide(&source, 0, None, Some(1)).unwrap();

    let reopened = reopen(&prs);
    assert_eq!(reopened.len(), 4);
    let original = &reopened.comments(0).unwrap()[0];
    let copy = &reopened.comments(1).unwrap()[0];
    assert_eq!(original.id, guid(1));
    assert_ne!(copy.id, original.id, "the copy has a fresh id");
    assert_eq!(
        (copy.author_id.as_str(), copy.text(), reply_texts(copy)),
        (GRACE, original.text(), vec!["Noted".to_owned()])
    );
    assert_eq!(
        reopened.comment_authors().len(),
        2,
        "authors are not doubled"
    );
    assert!(reopened.comments(2).is_none());
    assert!(reopened.comments(3).is_none());
}

#[test]
fn a_comment_can_point_at_a_shape_or_at_a_span_of_its_text() {
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    let body = prs
        .slide(1)
        .unwrap()
        .shapes()
        .find(|shape| {
            shape
                .text()
                .is_some_and(|text| text.starts_with("Subscription"))
        })
        .and_then(|shape| shape.non_visual_id())
        .expect("the Highlights body has an id");
    prs.add_comment_author(author(ADA, "Ada Lovelace", Some("AL")))
        .unwrap();

    prs.add_comment_at_shape(1, comment(&guid(1), ADA, MONDAY, "Whole list"), body, None)
        .unwrap();
    // "Subscription" is the first 12 UTF-16 units of the body text.
    prs.add_comment_at_shape(
        1,
        comment(&guid(2), ADA, MONDAY, "Which plans?"),
        body,
        Some((0, 12)),
    )
    .unwrap();
    assert!(
        prs.add_comment_at_shape(1, comment(&guid(3), ADA, MONDAY, "x"), 9999, None)
            .is_err(),
        "a missing shape is refused"
    );
    assert!(
        prs.add_comment_at_shape(
            1,
            comment(&guid(3), ADA, MONDAY, "x"),
            body,
            Some((0, 10_000))
        )
        .is_err(),
        "a span past the end of the text is refused"
    );

    let reopened = reopen(&prs);
    let comments = reopened.comments(1).unwrap();
    let slide = format!(r#"sldId="{}""#, reopened.slide(1).unwrap().id());
    let on_shape = String::from_utf8_lossy(comments[0].anchor_xml()).into_owned();
    assert!(on_shape.contains(&slide), "{on_shape}");
    assert!(
        on_shape.contains(&format!(r#"spMk id="{body}""#)),
        "{on_shape}"
    );
    let on_text = String::from_utf8_lossy(comments[1].anchor_xml()).into_owned();
    assert!(on_text.contains(&slide), "{on_text}");
    assert!(on_text.contains(r#"cp="0" len="12""#), "{on_text}");
}

#[test]
fn a_comment_added_to_a_powerpoint_or_google_deck_round_trips() {
    for name in ["powerpoint/deck.pptx", "google/deck.pptx"] {
        let mut prs = corpus_deck(name);
        let slides = slide_texts(&prs);
        let notes = all_notes(&prs);
        prs.add_comment_author(author(ADA, "Ada Lovelace", Some("AL")))
            .unwrap();

        prs.add_comment(2, comment(&guid(1), ADA, MONDAY, "Add last year's column"))
            .unwrap();

        let reopened = reopen(&prs);
        assert_eq!(
            comment_texts(&reopened, 2),
            ["Add last year's column"],
            "{name}"
        );
        assert_eq!(reopened.comment_authors()[0].name, "Ada Lovelace", "{name}");
        assert_eq!(slide_texts(&reopened), slides, "{name}");
        assert_eq!(all_notes(&reopened), notes, "{name}");
    }
}

#[test]
fn a_comment_saved_by_powerpoint_reads_back_and_keeps_its_position() {
    let mut prs = Presentation::from_bytes(&powerpoint_comment_deck("2026-11-16T09:30:00.503Z"))
        .expect("the PowerPoint-shaped deck opens");
    let [thread] = prs.comments(0).unwrap() else {
        panic!("one PowerPoint comment");
    };
    assert_eq!(thread.text(), "Needs a source for this figure.");
    assert_eq!(thread.created, "2026-11-16T09:30:00.503Z");
    assert_eq!(reply_texts(thread), ["Done in v2."]);
    let reviewer = &prs.comment_authors()[0];
    assert_eq!(
        (reviewer.name.as_str(), reviewer.initials.as_deref()),
        ("Grace Hopper", Some("GH"))
    );
    let thread_id = thread.id.clone();

    prs.add_comment_author(author(ADA, "Ada Lovelace", None))
        .unwrap();
    prs.reply_to_comment(0, &thread_id, reply(&guid(1), ADA, TUESDAY, "Source added"))
        .unwrap();
    prs.add_comment(0, comment(&guid(2), ADA, TUESDAY, "Second thread"))
        .unwrap();

    let reopened = reopen(&prs);
    assert_eq!(
        comment_texts(&reopened, 0),
        ["Needs a source for this figure.", "Second thread"]
    );
    assert_eq!(
        reply_texts(&reopened.comments(0).unwrap()[0]),
        ["Done in v2.", "Source added"]
    );
    // The facade has no reader for the position PowerPoint drew the comment at.
    let xml = part_xml(&reopened, "/ppt/comments/modernComment_100_4F1B2C3D.xml");
    assert!(
        xml.contains(r#"<p188:pos x="10668000" y="152400"/>"#),
        "{xml}"
    );
    assert!(xml.contains("sldMkLst"), "{xml}");
}

// A comment date is an xsd:dateTime (MS-PPTX), whose time zone is optional.
#[test]
fn a_comment_dated_in_local_time_opens() {
    let mut prs = Presentation::from_bytes(&powerpoint_comment_deck("2026-11-16T09:30:00.503"))
        .expect("a deck with a comment dated in local time opens");

    let thread = &prs.comments(0).unwrap()[0];
    assert_eq!(thread.created, "2026-11-16T09:30:00.503");
    assert_eq!(thread.replies()[0].created, "2026-11-16T09:30:00.503");
    let thread_id = thread.id.clone();
    prs.add_comment_author(author(ADA, "Ada Lovelace", None))
        .unwrap();
    prs.reply_to_comment(0, &thread_id, reply(&guid(1), ADA, TUESDAY, "Seen"))
        .unwrap();

    let reopened = reopen(&prs);
    let thread = &reopened.comments(0).unwrap()[0];
    assert_eq!(thread.created, "2026-11-16T09:30:00.503");
    let dates: Vec<_> = thread
        .replies()
        .iter()
        .map(|reply| reply.created.as_str())
        .collect();
    assert_eq!(dates, ["2026-11-16T09:30:00.503", TUESDAY]);
}

#[test]
fn a_comment_dated_in_local_time_is_added_to_every_editors_deck_as_written() {
    const LOCAL: &str = "2026-11-21T10:00:00";
    for name in CORPUS_DECKS {
        let mut prs = corpus_deck(name);
        prs.add_comment_author(author(ADA, "Ada Lovelace", Some("AL")))
            .unwrap();

        prs.add_comment(1, comment(&guid(1), ADA, LOCAL, "Dated in local time"))
            .unwrap();
        prs.reply_to_comment(
            1,
            &guid(1),
            reply(&guid(2), ADA, "2026-11-21T10:05:30.25", "Same"),
        )
        .unwrap();

        let reopened = reopen(&prs);
        let thread = &reopened.comments(1).unwrap()[0];
        assert_eq!(thread.created, LOCAL, "{name}");
        assert_eq!(
            thread.replies()[0].created,
            "2026-11-21T10:05:30.25",
            "{name}"
        );
        assert!(
            reopened.validate().is_empty(),
            "{name}: {:?}",
            reopened.validate()
        );
    }
}

#[test]
fn a_comment_date_may_omit_its_time_zone_but_not_its_shape() {
    for date in [
        "2026-11-16T09:30:00",
        "2026-11-16T09:30:00.5",
        "2026-11-16T09:30:00Z",
        "2026-11-16T09:30:00-05:00",
    ] {
        assert!(Comment::new(guid(1), ADA, date, "text").is_ok(), "{date}");
        assert!(
            CommentReply::new(guid(1), ADA, date, "text").is_ok(),
            "{date}"
        );
    }
    for date in [
        "2026-11-16T09:30",
        "2026-11-16 09:30:00",
        "2026-11-16T09:30:00.",
        "2026-11-16T09:30:00+0100",
        "2026-11-16T09:30:00 ",
        "2026-02-30T09:30:00",
        "2026-11-16T24:30:00",
    ] {
        assert!(Comment::new(guid(1), ADA, date, "text").is_err(), "{date}");
        assert!(
            CommentReply::new(guid(1), ADA, date, "text").is_err(),
            "{date}"
        );
    }
}

#[test]
fn legacy_comments_are_kept_through_edits_and_a_save() {
    let mut prs = deck_with_legacy_comments(2);

    prs.set_notes_text(0, "Notes next to a legacy comment")
        .unwrap();
    prs.move_slide(0, 1).unwrap();

    // The facade has no reader for legacy comments.
    let reopened = reopen(&prs);
    let xml = part_xml(&reopened, "/ppt/comments/comment1.xml");
    assert!(xml.contains("Legacy remark on the opening slide"), "{xml}");
    assert!(part_xml(&reopened, "/ppt/commentAuthors.xml").contains("Grace Hopper"));
    let package = package_of(&reopened);
    let owner = package
        .get_part_rels("/ppt/slides/slide1.xml")
        .and_then(|relationships| relationships.get_by_type(rel_types::COMMENTS))
        .expect("the slide still points at its legacy comments");
    assert_eq!(owner.target, "../comments/comment1.xml");
    assert_eq!(
        reopened.slide(1).unwrap().notes_text().as_deref(),
        Some("Notes next to a legacy comment")
    );
}

// Fails today: remove_slide drops the slide's notes and modern comments but
// leaves its legacy comment part, text included, in the saved file. Issue: #38
#[test]
#[should_panic(expected = "the legacy comments of the removed slide are still in the file")]
fn removing_a_slide_removes_its_legacy_comments() {
    let mut prs = deck_with_legacy_comments(2);

    prs.remove_slide(0).unwrap();

    let comment_parts = parts_under(&reopen(&prs), "ppt/comments/");
    assert!(
        comment_parts.is_empty(),
        "the legacy comments of the removed slide are still in the file: {comment_parts:?}"
    );
}

// Fails today: the legacy comment part sits at /ppt/comments/comment1.xml,
// the name add_comment guards, so any modern comment is refused. Issue: #39
#[test]
#[should_panic(expected = "CollaborationPartCollision")]
fn a_modern_comment_can_be_added_to_a_deck_with_legacy_comments() {
    let mut prs = deck_with_legacy_comments(2);
    prs.add_comment_author(author(ADA, "Ada Lovelace", Some("AL")))
        .unwrap();

    prs.add_comment(1, comment(&guid(1), ADA, MONDAY, "A modern comment"))
        .expect("a modern comment is added next to legacy comments");

    let reopened = reopen(&prs);
    assert_eq!(comment_texts(&reopened, 1), ["A modern comment"]);
    assert!(part_xml(&reopened, "/ppt/comments/comment1.xml").contains("Legacy remark"));
}

#[test]
fn a_comment_author_keeps_the_tabs_and_line_feeds_of_its_name() {
    let mut prs = blank_deck(1);
    prs.add_comment_author(author(ADA, "Ada\tLovelace\nR&D", Some("A\tL")))
        .unwrap();
    prs.add_comment(0, comment(&guid(1), ADA, MONDAY, "Signed"))
        .unwrap();

    let reopened = reopen(&prs);
    let ada = &reopened.comment_authors()[0];
    assert_eq!(
        (ada.name.as_str(), ada.initials.as_deref()),
        ("Ada\tLovelace\nR&D", Some("A\tL"))
    );
}
