//! A direct body paragraph edited on a copy of itself, swapped in on success.

use oxoffice_docx::{Document, Error};

use crate::support::corpus_root;

fn word_report() -> Document {
    let path = corpus_root().join("files/word/report.docx");
    Document::open(&path).expect("the Word report opens")
}

/// The first paragraph of the Word report that is a direct body child and
/// holds text.
fn direct_paragraph(document: &Document) -> usize {
    (0..document.paragraph_count())
        .find(|index| {
            document.content_index_of_paragraph(*index).is_some()
                && !document
                    .paragraph(*index)
                    .expect("paragraph")
                    .text()
                    .is_empty()
        })
        .expect("the report has a direct body paragraph with text")
}

#[test]
fn an_edit_in_place_replaces_the_paragraph_and_returns_the_old_one() {
    let mut document = word_report();
    let index = direct_paragraph(&document);
    let before = document.paragraph(index).expect("paragraph").text();
    let (value, old) = document
        .edit_body_paragraph_in_place(index, |paragraph| {
            paragraph.set_text("changed")?;
            Ok(7)
        })
        .expect("edits")
        .expect("a direct body paragraph");
    assert_eq!(value, 7);
    assert_eq!(
        document.paragraph(index).expect("paragraph").text(),
        "changed"
    );
    document
        .restore_body_paragraph(index, old)
        .expect("restores");
    assert_eq!(document.paragraph(index).expect("paragraph").text(), before);
}

#[test]
fn a_refused_edit_in_place_leaves_the_saved_bytes_unchanged() {
    let mut document = word_report();
    let index = direct_paragraph(&document);
    let before = document.to_bytes().expect("saves");
    let refused = document.edit_body_paragraph_in_place(index, |paragraph| {
        paragraph.add_run("half done");
        Err::<(), _>(Error::Other("refused".to_owned()))
    });
    assert!(refused.is_err());
    assert_eq!(document.to_bytes().expect("saves"), before);
}

#[test]
fn a_panicking_edit_in_place_leaves_the_saved_bytes_unchanged() {
    let mut document = word_report();
    let index = direct_paragraph(&document);
    let before = document.to_bytes().expect("saves");
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        document.edit_body_paragraph_in_place(index, |paragraph| -> oxoffice_docx::Result<()> {
            paragraph.add_run("half done");
            panic!("an edit that panics");
        })
    }));
    assert!(panicked.is_err());
    assert_eq!(document.to_bytes().expect("saves"), before);
}

#[test]
fn a_restored_paragraph_saves_the_bytes_of_before_the_edit() {
    let mut document = word_report();
    let index = direct_paragraph(&document);
    let before = document.to_bytes().expect("saves");
    let (_, old) = document
        .edit_body_paragraph_in_place(index, |paragraph| paragraph.set_text("undone"))
        .expect("edits")
        .expect("a direct body paragraph");
    assert_ne!(document.to_bytes().expect("saves"), before);
    document
        .restore_body_paragraph(index, old)
        .expect("restores");
    assert_eq!(document.to_bytes().expect("saves"), before);
}

#[test]
fn a_save_after_an_edit_in_place_writes_the_new_paragraph() {
    let mut document = word_report();
    let index = direct_paragraph(&document);
    // Reads that fill the save and layout caches before the edit.
    let _ = document.to_bytes().expect("saves");
    let _ = document.text();
    document
        .edit_body_paragraph_in_place(index, |paragraph| paragraph.set_text("rewritten"))
        .expect("edits")
        .expect("a direct body paragraph");
    assert!(document.text().contains("rewritten"));
    let reopened = Document::from_bytes(&document.to_bytes().expect("saves")).expect("reopens");
    assert_eq!(
        reopened.paragraph(index).expect("paragraph").text(),
        "rewritten"
    );
    assert_eq!(reopened.paragraph_count(), document.paragraph_count());
    for other in (0..document.paragraph_count()).filter(|other| *other != index) {
        assert_eq!(
            reopened.paragraph(other).expect("paragraph").text(),
            document.paragraph(other).expect("paragraph").text()
        );
    }
}

#[test]
fn a_paragraph_that_is_not_a_direct_body_child_is_not_edited_in_place() {
    let path = corpus_root().join("files/word/record.docx");
    let mut document = Document::open(&path).expect("the Word record opens");
    let index = (0..document.paragraph_count())
        .find(|index| document.content_index_of_paragraph(*index).is_none())
        .expect("a paragraph inside the control");
    let before = document.to_bytes().expect("saves");
    let mut ran = false;
    let outcome = document
        .edit_body_paragraph_in_place(index, |paragraph| {
            ran = true;
            paragraph.set_text("never")
        })
        .expect("no error");
    assert!(outcome.is_none());
    assert!(!ran);
    assert_eq!(document.to_bytes().expect("saves"), before);
}

#[test]
fn an_index_out_of_range_is_an_error() {
    let mut document = word_report();
    let count = document.paragraph_count();
    assert!(
        document
            .edit_body_paragraph_in_place(count, |paragraph| paragraph.set_text("x"))
            .is_err()
    );
}

#[test]
fn an_edit_in_place_drops_the_layout_of_before_the_edit() {
    let mut document = Document::new();
    document.add_paragraph("first");
    document.add_paragraph("second");
    let mut document = Document::from_bytes(&document.to_bytes().expect("saves")).expect("reopens");
    let before = document.layout_deterministic().expect("lays out");
    document
        .edit_body_paragraph_in_place(1, |paragraph| paragraph.set_text("a longer second"))
        .expect("edits")
        .expect("a direct body paragraph");
    let after = document.layout_deterministic().expect("lays out");
    assert!(!std::sync::Arc::ptr_eq(&before, &after));
}

#[test]
fn a_swapped_block_saves_the_bytes_of_before_the_edit() {
    let mut document = word_report();
    let index = direct_paragraph(&document);
    let slot = document
        .content_index_of_paragraph(index)
        .expect("a direct body paragraph");
    let before = document.to_bytes().expect("saves");
    let (_, old) = document
        .edit_body_paragraph_in_place(index, |paragraph| paragraph.set_text("undone"))
        .expect("edits")
        .expect("a direct body paragraph");
    document
        .swap_body_block(
            slot,
            oxoffice_docx_oxml::BodyContent::Paragraph(Box::new(old)),
        )
        .expect("swaps");
    assert_eq!(document.to_bytes().expect("saves"), before);
    assert!(
        document
            .swap_body_block(
                document.content_count(),
                oxoffice_docx_oxml::BodyContent::RawXml(Box::default())
            )
            .is_err()
    );
}

/// A new document of three short paragraphs and the body slot of the second.
fn three_paragraphs() -> (Document, usize) {
    let mut document = Document::new();
    for index in 0..3 {
        document.add_paragraph(&format!("para {index}"));
    }
    let slot = document
        .content_index_of_paragraph(1)
        .expect("a direct body paragraph");
    (document, slot)
}

#[test]
fn a_block_edited_in_place_reads_back_as_a_reopen_reads_it() {
    let (mut document, slot) = three_paragraphs();
    document
        .edit_body_paragraph_in_place(1, |paragraph| {
            paragraph.run_mut(0).expect("a run").add_field("PAGE", "1")
        })
        .expect("edits")
        .expect("a direct body paragraph");
    // The edit leaves the field in the run; the reader splits it out.
    assert!(
        document
            .reread_body_blocks_in_place(slot, 1)
            .expect("rereads")
    );
    let reopened = Document::from_bytes(&document.to_bytes().expect("saves")).expect("reopens");
    assert_eq!(
        document.body_children()[slot],
        reopened.body_children()[slot]
    );
    assert!(
        !document
            .reread_body_blocks_in_place(slot, 1)
            .expect("rereads")
    );
    assert!(
        document
            .reread_body_blocks_in_place(document.content_count(), 1)
            .is_err()
    );
}

const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

#[test]
fn a_reread_block_leaves_the_save_to_give_its_pictures_their_drawing_ids() {
    use oxoffice_docx::Length;
    let (mut document, _) = three_paragraphs();
    let relationship = document.embed_image(PNG, "dot.png");
    // Two pictures added by handle, each in a block reread before the save.
    for index in 0..2 {
        document
            .edit_body_paragraph_in_place(index, |paragraph| {
                let mut run = paragraph.run_mut(0).expect("a run");
                run.add_picture(&relationship, Length::inches(1.0), Length::inches(1.0));
                run.add_field("PAGE", "1")
            })
            .expect("edits")
            .expect("a direct body paragraph");
        let slot = document
            .content_index_of_paragraph(index)
            .expect("a direct body paragraph");
        assert!(
            document
                .reread_body_blocks_in_place(slot, 1)
                .expect("rereads")
        );
    }
    let reopened = Document::from_bytes(&document.to_bytes().expect("saves")).expect("reopens");
    let mut ids = Vec::new();
    for block in reopened.body_children() {
        let oxoffice_docx_oxml::BodyContent::Paragraph(paragraph) = block else {
            continue;
        };
        for content in paragraph.runs.iter().flat_map(|run| &run.content) {
            if let oxoffice_docx_oxml::text::RunContent::Drawing(drawing) = content {
                ids.push(
                    drawing
                        .inline
                        .as_ref()
                        .expect("an inline picture")
                        .doc_pr_id,
                );
            }
        }
    }
    assert_eq!(ids.len(), 2);
    assert_ne!(
        ids[0], ids[1],
        "the save gives each picture its own drawing id"
    );
}

#[test]
fn an_edited_paragraph_binds_every_prefix_it_writes_when_the_document_names_them_otherwise() {
    use oxoffice_xml_test::{Writing, canonical, rewrite_package};

    let mut doc = Document::new();
    let relationship = doc.add_hyperlink_relationship("https://example.com/schedule");
    let mut paragraph = doc.add_paragraph("Read the delivery ");
    paragraph.add_hyperlink("schedule", &relationship);
    paragraph.add_run(" before the meeting.");
    let bytes = doc.to_bytes().expect("saves");
    // The main namespaces under other prefixes (`ox0:p`, `ox3:id`), as a
    // generated writer names them: no `w` or `r` binding left in the part.
    let bytes = rewrite_package(&bytes, Writing::OtherPrefixes)
        .expect("rewrites")
        .bytes;
    let mut doc = Document::from_bytes(&bytes).expect("opens");
    let paragraphs = doc.story_paragraphs().expect("reads");
    let edited = paragraphs
        .iter()
        .find(|paragraph| paragraph.text.contains("delivery"))
        .expect("the paragraph");
    assert_eq!(
        doc.try_replace_text_at(&edited.location, "delivery", "DELIVERY", Some(1))
            .expect("edits"),
        Ok(1)
    );
    let saved = doc.to_bytes().expect("saves");
    let part = crate::support::package_parts(&saved)
        .remove("word/document.xml")
        .expect("main part");
    // Every prefix the edited paragraph writes is bound: the part is XML.
    canonical(&part).expect("the edited part binds every prefix it uses");
    let mut reopened = Document::from_bytes(&saved).expect("reopens");
    let text = reopened
        .story_paragraphs()
        .expect("reads")
        .into_iter()
        .map(|paragraph| paragraph.text)
        .collect::<Vec<_>>();
    assert!(
        text.iter().any(|text| text.contains("DELIVERY schedule")),
        "{text:?}"
    );
    let reopened = reopened.to_bytes().expect("saves again");
    assert!(
        String::from_utf8_lossy(
            &crate::support::package_parts(&reopened)["word/_rels/document.xml.rels"]
        )
        .contains("https://example.com/schedule"),
        "the hyperlink keeps its target"
    );
}
