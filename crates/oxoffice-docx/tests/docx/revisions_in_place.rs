//! Accepting or rejecting one revision id in the body paragraphs that hold
//! it, in place, and the shapes that keep the staged route. Debug builds
//! compare each in-place call with the staged route on a copy.

use oxoffice_docx::Document;

use crate::support::{doc_with_body, reopen, texts};

const MARKS: &str = r#"w:author="A" w:date="2026-11-25T09:00:00Z""#;

fn run(text: &str) -> String {
    format!(r#"<w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

fn ins(id: i32, text: &str) -> String {
    format!(r#"<w:ins w:id="{id}" {MARKS}>{}</w:ins>"#, run(text))
}

fn del(id: i32, text: &str) -> String {
    format!(
        r#"<w:del w:id="{id}" {MARKS}><w:r><w:delText xml:space="preserve">{text}</w:delText></w:r></w:del>"#
    )
}

fn paragraph(children: &str) -> String {
    format!("<w:p>{children}</w:p>")
}

/// Accept or reject `id` on a fresh copy of `body`: the route the call
/// takes (its in-place slots), its count and the texts after a reopen.
fn resolve(body: &str, id: i32, accept: bool) -> (Option<Vec<usize>>, usize, Vec<String>) {
    let mut doc = doc_with_body(body);
    let slots = doc.revision_id_in_place_slots(id, accept);
    let count = if accept {
        doc.accept_revision_id(id)
    } else {
        doc.reject_revision_id(id)
    }
    .unwrap_or_else(|error| panic!("revision {id} resolves: {error}"));
    assert!(
        doc.revisions().iter().all(|revision| revision.id() != id),
        "revision {id} is resolved whole"
    );
    (slots, count, texts(&reopen(&mut doc)))
}

fn three_paragraphs(middle: &str) -> String {
    [
        paragraph(&run("first")),
        paragraph(middle),
        paragraph(&run("last")),
    ]
    .concat()
}

#[test]
fn an_inline_insertion_or_deletion_resolves_in_its_paragraph_in_place() {
    let body = three_paragraphs(&[run("keep "), ins(5, "added"), del(6, "gone")].concat());
    let (slots, count, after) = resolve(&body, 5, true);
    assert_eq!((slots, count), (Some(vec![1]), 1));
    assert_eq!(after, ["first", "keep added", "last"]);
    let (_, count, after) = resolve(&body, 5, false);
    assert_eq!(count, 1);
    assert_eq!(after, ["first", "keep ", "last"]);
    let (slots, count, after) = resolve(&body, 6, true);
    assert_eq!((slots, count), (Some(vec![1]), 1));
    assert_eq!(after, ["first", "keep added", "last"]);
    let (_, count, after) = resolve(&body, 6, false);
    assert_eq!(count, 1);
    assert_eq!(after, ["first", "keep addedgone", "last"]);
}

#[test]
fn run_and_paragraph_property_changes_resolve_in_place() {
    let run_change = format!(
        r#"<w:r><w:rPr><w:b/><w:rPrChange w:id="7" {MARKS}><w:rPr><w:i/></w:rPr></w:rPrChange></w:rPr><w:t>bold</w:t></w:r>"#
    );
    let paragraph_change = format!(
        r#"<w:pPr><w:jc w:val="center"/><w:pPrChange w:id="8" {MARKS}><w:pPr><w:jc w:val="right"/></w:pPr></w:pPrChange></w:pPr>{}"#,
        run("centred")
    );
    let body = [
        paragraph(&run_change),
        paragraph(&paragraph_change),
        paragraph(&run("last")),
    ]
    .concat();
    for accept in [true, false] {
        let (slots, count, _) = resolve(&body, 7, accept);
        assert_eq!((slots, count), (Some(vec![0]), 1), "accept {accept}");
        let (slots, count, _) = resolve(&body, 8, accept);
        assert_eq!((slots, count), (Some(vec![1]), 1), "accept {accept}");
    }
    let mut doc = doc_with_body(&body);
    doc.reject_revision_id(7).unwrap();
    doc.reject_revision_id(8).unwrap();
    let doc = reopen(&mut doc);
    assert!(doc.revisions().is_empty());
}

#[test]
fn a_rejected_change_from_no_run_properties_keeps_the_staged_route() {
    // The resolver writes the empty prior properties (`<w:rPr/>`), which
    // the staged route saves as they are and the model would not write.
    let body = three_paragraphs(&format!(
        r#"<w:r><w:rPr><w:b/><w:rPrChange w:id="7" {MARKS}><w:rPr/></w:rPrChange></w:rPr><w:t>bold</w:t></w:r>"#
    ));
    let (slots, count, _) = resolve(&body, 7, true);
    assert_eq!((slots, count), (Some(vec![1]), 1));
    let (slots, count, _) = resolve(&body, 7, false);
    assert_eq!((slots, count), (None, 1));
}

#[test]
fn one_side_of_a_move_resolves_in_place_and_leaves_the_other() {
    let from = format!(
        r#"<w:moveFrom w:id="11" {MARKS}><w:r><w:delText>moved</w:delText></w:r></w:moveFrom>"#
    );
    let to = format!(r#"<w:moveTo w:id="12" {MARKS}>{}</w:moveTo>"#, run("moved"));
    let body = [
        paragraph(&[run("a "), from].concat()),
        paragraph(&run("middle")),
        paragraph(&[run("b "), to].concat()),
    ]
    .concat();
    let (slots, count, after) = resolve(&body, 11, true);
    assert_eq!((slots, count), (Some(vec![0]), 1));
    assert_eq!(after, ["a ", "middle", "b moved"]);
    let (slots, count, after) = resolve(&body, 12, false);
    assert_eq!((slots, count), (Some(vec![2]), 1));
    assert_eq!(after, ["a ", "middle", "b "]);
    let mut doc = doc_with_body(&body);
    doc.reject_revision_id(12).unwrap();
    assert_eq!(doc.revisions().len(), 1, "the move away stays tracked");
}

#[test]
fn one_id_in_two_body_paragraphs_resolves_both_in_place() {
    let body = [
        paragraph(&[run("one "), ins(9, "x")].concat()),
        paragraph(&run("between")),
        paragraph(&[run("two "), ins(9, "y")].concat()),
    ]
    .concat();
    let (slots, count, after) = resolve(&body, 9, false);
    assert_eq!((slots, count), (Some(vec![0, 2]), 2));
    assert_eq!(after, ["one ", "between", "two "]);
    let (_, count, after) = resolve(&body, 9, true);
    assert_eq!(count, 2);
    assert_eq!(after, ["one x", "between", "two y"]);
}

#[test]
fn an_unknown_id_resolves_nothing_and_changes_nothing() {
    let body = three_paragraphs(&[run("keep "), ins(5, "added")].concat());
    let mut doc = doc_with_body(&body);
    let saved = doc.to_bytes().unwrap();
    assert_eq!(doc.revision_id_in_place_slots(77, true), None);
    assert_eq!(doc.accept_revision_id(77).unwrap(), 0);
    assert_eq!(doc.reject_revision_id(77).unwrap(), 0);
    assert_eq!(doc.to_bytes().unwrap(), saved);
}

#[test]
fn a_deleted_paragraph_mark_keeps_the_staged_route_and_joins_the_paragraphs() {
    let marked = format!(
        r#"<w:pPr><w:rPr><w:del w:id="13" {MARKS}/></w:rPr></w:pPr>{}{}"#,
        run("joined "),
        del(13, "x")
    );
    let body = [
        paragraph(&marked),
        paragraph(&run("next")),
        paragraph(&run("last")),
    ]
    .concat();
    let (slots, count, after) = resolve(&body, 13, true);
    assert_eq!(slots, None);
    assert_eq!(count, 2);
    assert_eq!(after, ["joined next", "last"]);
    // A paragraph mark inserted alone: rejecting it joins the paragraph
    // with the next one, keeping the paragraph's text.
    let inserted = format!(
        r#"<w:pPr><w:rPr><w:ins w:id="20" {MARKS}/></w:rPr></w:pPr>{}"#,
        run("split ")
    );
    let body = [
        paragraph(&run("first")),
        paragraph(&inserted),
        paragraph(&run("next")),
    ]
    .concat();
    let (slots, count, after) = resolve(&body, 20, false);
    assert_eq!(slots, None);
    assert_eq!(count, 1);
    assert_eq!(after, ["first", "split next"]);
}

#[test]
fn an_inserted_table_row_keeps_the_staged_route() {
    let row = |text: &str, marker: &str| {
        format!(
            "<w:tr><w:trPr>{marker}</w:trPr><w:tc><w:tcPr><w:tcW w:w=\"2000\" w:type=\"dxa\"/></w:tcPr>{}</w:tc></w:tr>",
            paragraph(&run(text))
        )
    };
    let body = [
        paragraph(&run("before")),
        format!(
            "<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/></w:tblPr><w:tblGrid><w:gridCol w:w=\"2000\"/></w:tblGrid>{}{}</w:tbl>",
            row("kept", ""),
            row("added", &format!(r#"<w:ins w:id="14" {MARKS}/>"#)),
        ),
        paragraph(&run("after")),
    ]
    .concat();
    let mut doc = doc_with_body(&body);
    assert_eq!(doc.revision_id_in_place_slots(14, true), None);
    assert_eq!(doc.reject_revision_id(14).unwrap(), 1);
    assert!(doc.revisions().iter().all(|revision| revision.id() != 14));
}

/// `doc` saved, with the first part whose name holds `name` edited by
/// `edit`, reopened.
fn with_part_edit(doc: &mut Document, name: &str, edit: impl Fn(&str) -> String) -> Document {
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(doc.to_bytes().unwrap()))
            .unwrap();
    let part = package
        .zip_entries()
        .unwrap()
        .into_iter()
        .map(|(entry, _)| format!("/{entry}"))
        .find(|entry| entry.contains(name))
        .unwrap_or_else(|| panic!("a {name} part"));
    let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
    let edited = edit(&xml);
    assert_ne!(edited, xml, "the {name} part was edited");
    package.set_part(&part, edited.into_bytes());
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    Document::from_bytes(&out.into_inner()).unwrap()
}

/// `xml` with an insertion of `id` added after the run that shows `text`.
fn with_insertion_after(xml: &str, text: &str, id: i32) -> String {
    let at = xml.find(text).expect("the text") + text.len();
    let end = at + xml[at..].find("</w:r>").expect("its run's end") + "</w:r>".len();
    format!("{}{}{}", &xml[..end], ins(id, " more"), &xml[end..])
}

#[test]
fn an_id_also_in_a_header_keeps_the_staged_route() {
    let body = three_paragraphs(&[run("keep "), ins(15, "added")].concat());
    let mut doc = doc_with_body(&body);
    doc.set_header("Header words");
    let mut doc = with_part_edit(&mut doc, "header", |xml| {
        with_insertion_after(xml, "Header words", 15)
    });
    assert_eq!(doc.revision_id_in_place_slots(15, true), None);
    assert_eq!(doc.reject_revision_id(15).unwrap(), 2);
    assert!(doc.revisions().iter().all(|revision| revision.id() != 15));
    // Another id of the body still resolves in place.
    let body = three_paragraphs(&[run("keep "), ins(16, "added")].concat());
    let mut doc = doc_with_body(&body);
    doc.set_header("Header words");
    let mut doc = with_part_edit(&mut doc, "header", |xml| {
        with_insertion_after(xml, "Header words", 15)
    });
    assert_eq!(doc.revision_id_in_place_slots(16, true), Some(vec![1]));
    assert_eq!(doc.accept_revision_id(16).unwrap(), 1);
}

#[test]
fn an_id_also_in_a_footnote_keeps_the_staged_route() {
    let body = three_paragraphs(&[run("keep "), ins(17, "added")].concat());
    let mut doc = doc_with_body(&body);
    doc.add_footnote("Note words");
    let mut doc = with_part_edit(&mut doc, "footnotes", |xml| {
        with_insertion_after(xml, "Note words", 17)
    });
    assert_eq!(doc.revision_id_in_place_slots(17, true), None);
    assert_eq!(doc.accept_revision_id(17).unwrap(), 2);
    assert!(doc.revisions().iter().all(|revision| revision.id() != 17));
}

#[test]
fn a_paragraph_with_a_comment_or_a_field_keeps_the_staged_route() {
    let comment = three_paragraphs(
        &[
            r#"<w:commentRangeStart w:id="0"/>"#.to_owned(),
            ins(18, "added"),
            r#"<w:commentRangeEnd w:id="0"/>"#.to_owned(),
        ]
        .concat(),
    );
    let doc = doc_with_body(&comment);
    assert_eq!(doc.revision_id_in_place_slots(18, true), None);
    let field = three_paragraphs(
        &[
            ins(19, "added"),
            r#"<w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple>"#.to_owned(),
        ]
        .concat(),
    );
    let mut doc = doc_with_body(&field);
    assert_eq!(doc.revision_id_in_place_slots(19, true), None);
    assert_eq!(doc.reject_revision_id(19).unwrap(), 1);
}

#[test]
fn an_in_place_resolution_leaves_an_undo_the_binding_takes() {
    let body = three_paragraphs(&[run("keep "), ins(5, "added")].concat());
    let mut doc = doc_with_body(&body);
    let saved = doc.to_bytes().unwrap();
    doc.hold_edit_undo();
    assert_eq!(doc.reject_revision_id(5).unwrap(), 1);
    let undo = doc.take_edit_undo().expect("the edit is held");
    assert_eq!(
        undo.report().body_blocks,
        [oxoffice_docx::BodyBlocksChange {
            slot: 1,
            removed: 1,
            inserted: 1
        }]
    );
    doc.undo_edit(undo);
    assert_eq!(doc.to_bytes().unwrap(), saved);
}

#[test]
fn a_main_part_another_producer_framed_keeps_the_staged_route() {
    // The XML declaration as python-docx writes it: a typed edit would
    // write the frame again, the staged route keeps it.
    let body = three_paragraphs(&[run("keep "), ins(5, "added")].concat());
    let bytes = crate::support::docx_with_body(&body);
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).expect("package opens");
    let part = package.main_document_part().expect("main document part");
    let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
    let end = xml.find("?>").unwrap() + 2;
    let framed = format!(
        "<?xml version='1.0' encoding='UTF-8' standalone='yes'?>\n{}",
        &xml[end..]
    );
    package.set_part(&part, framed.into_bytes());
    let mut saved = Vec::new();
    package
        .write_to(std::io::Cursor::new(&mut saved))
        .expect("package saves");
    for accept in [true, false] {
        let mut doc = Document::from_bytes(&saved).unwrap();
        assert_eq!(doc.revision_id_in_place_slots(5, accept), None);
        let count = if accept {
            doc.accept_revision_id(5)
        } else {
            doc.reject_revision_id(5)
        }
        .unwrap();
        assert_eq!(count, 1);
        let expected = if accept { "keep added" } else { "keep " };
        assert_eq!(texts(&reopen(&mut doc)), ["first", expected, "last"]);
    }
}
