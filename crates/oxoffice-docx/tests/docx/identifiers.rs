//! Identifiers an edit adds: a numbering instance, a bookmark, a picture's
//! relationship, drawing id and media part take their value when the edit is
//! made, and the save writes that value. Reads before a save and after a
//! reopen agree, the values follow the order of the edits, not of the
//! document, and removing content renumbers nothing.

use oxoffice_docx::{Document, Length, ListLevel, ListNumberFormat, StoryRunPosition};

use crate::support::{corpus_doc, doc_with};

/// A valid one-pixel-per-side grey PNG of `side` pixels, `grey` making the
/// bytes of two pictures differ.
fn png(side: u32, grey: u8) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFF_u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend(u32::try_from(data.len()).unwrap().to_be_bytes());
        let start = out.len();
        out.extend(kind);
        out.extend(data);
        let crc = crc32(&out[start..]);
        out.extend(crc.to_be_bytes());
    }
    let mut header = Vec::new();
    header.extend(side.to_be_bytes());
    header.extend(side.to_be_bytes());
    header.extend([8, 0, 0, 0, 0]);
    let mut rows = Vec::new();
    for _ in 0..side {
        rows.push(0);
        rows.extend(std::iter::repeat_n(grey, side as usize));
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &header);
    chunk(
        &mut out,
        b"IDAT",
        &miniz_oxide::deflate::compress_to_vec_zlib(&rows, 6),
    );
    chunk(&mut out, b"IEND", &[]);
    out
}

/// The document a save of `doc` writes, opened, saved and opened again.
/// `doc` itself is left as the edits made it.
fn saved(doc: &Document) -> Document {
    let bytes = doc.clone_for_staging().to_bytes().expect("document saves");
    let mut reopened = Document::from_bytes(&bytes).expect("saved document reopens");
    let bytes = reopened.to_bytes().expect("reopened document saves");
    Document::from_bytes(&bytes).expect("saved twice, the document reopens")
}

/// Every picture as (image bytes, relationship id, media part name), in
/// story order.
fn picture_ids(doc: &Document) -> Vec<(Vec<u8>, String, Option<String>)> {
    doc.pictures()
        .expect("pictures read")
        .into_iter()
        .map(|picture| {
            (
                picture.bytes,
                picture.relationship_id,
                picture.media_part_name,
            )
        })
        .collect()
}

/// The `wp:docPr` ids of the body paragraph at `index`, as its XML reads.
fn drawing_ids(doc: &Document, index: usize) -> Vec<String> {
    let location = doc
        .paragraph_story_location(index)
        .unwrap()
        .expect("paragraph exists");
    let xml = String::from_utf8(doc.paragraph_element_xml(&location).unwrap()).unwrap();
    xml.match_indices("<wp:docPr")
        .map(|(at, _)| {
            let element = &xml[at..at + xml[at..].find('>').expect("closed docPr")];
            element
                .split_whitespace()
                .find_map(|attribute| attribute.strip_prefix("id=\""))
                .expect("docPr has an id")
                .trim_end_matches(['/', '"'])
                .to_owned()
        })
        .collect()
}

#[test]
fn numbering_instances_read_before_a_save_are_the_ones_it_writes() {
    let mut doc = Document::new();
    let definition = doc
        .add_numbering_definition(&[ListLevel::new(ListNumberFormat::Decimal)])
        .unwrap();
    let first = doc.add_numbering_instance(definition, &[]).unwrap();
    let second = doc.add_numbering_instance(definition, &[]).unwrap();
    // The second list comes first in the body: a save that numbered lists in
    // document order would swap the two.
    doc.add_paragraph("on the second list").numbering(second, 0);
    doc.add_paragraph("on the first list").numbering(first, 0);
    let read = |doc: &Document| {
        (0..2)
            .map(|index| doc.paragraph(index).unwrap().numbering())
            .collect::<Vec<_>>()
    };
    let before = read(&doc);
    assert_eq!(before, [Some((second, 0)), Some((first, 0))]);

    let saved = saved(&doc);

    assert_eq!(read(&saved), before);
    for id in [first, second] {
        assert_eq!(
            saved.numbering_instance(id).unwrap().definition_id,
            definition,
            "instance {id} keeps its definition"
        );
    }
}

#[test]
fn bookmark_ids_an_edit_returns_are_the_ones_a_save_writes() {
    let mut doc = doc_with(&["first", "second"]);
    let range = |paragraph| oxoffice_docx::RunRange {
        start: oxoffice_docx::RunPosition {
            body_index: paragraph,
            run_index: 0,
        },
        end: oxoffice_docx::RunPosition {
            body_index: paragraph,
            run_index: 1,
        },
    };
    // Bookmarked in the reverse of their document order.
    let later = doc.add_bookmark("later", range(1)).unwrap();
    let earlier = doc.add_bookmark("earlier", range(0)).unwrap();
    let ids = |doc: &Document| {
        doc.bookmarks()
            .iter()
            .map(|bookmark| (bookmark.name().unwrap().to_owned(), bookmark.id().unwrap()))
            .collect::<Vec<_>>()
    };
    let before = ids(&doc);
    assert_eq!(
        before,
        [("earlier".to_owned(), earlier), ("later".to_owned(), later)]
    );
    assert!(later < earlier, "ids follow the order of the edits");

    assert_eq!(ids(&saved(&doc)), before);
}

#[test]
fn a_picture_keeps_its_ids_when_another_is_inserted_before_it() {
    let (a, b) = (png(2, 10), png(3, 20));
    let mut doc = doc_with(&["first"]);
    doc.add_picture(&a, "a.png", Length::inches(1.0), Length::inches(1.0));
    let a_ids = picture_ids(&doc);
    let a_drawing = drawing_ids(&doc, 1);
    assert_eq!(a_ids.len(), 1);

    let position = StoryRunPosition {
        location: doc.paragraph_story_location(0).unwrap().unwrap(),
        run_index: 1,
    };
    doc.insert_picture_at(&position, &b, "b.png", None, None)
        .unwrap();

    let ids = picture_ids(&doc);
    assert_eq!(ids[0].0, b, "the new picture comes first in the body");
    assert_eq!(ids[1], a_ids[0], "the earlier picture keeps its ids");
    assert_eq!(drawing_ids(&doc, 1), a_drawing);
    // Ids added by edits follow the order of the edits.
    assert_ne!(ids[0].1, ids[1].1);
    assert_ne!(ids[0].2, ids[1].2);
    assert!(drawing_ids(&doc, 0)[0].parse::<u32>().unwrap() > a_drawing[0].parse().unwrap());

    let saved = saved(&doc);
    assert_eq!(picture_ids(&saved), ids);
    assert_eq!(drawing_ids(&saved, 0), drawing_ids(&doc, 0));
    assert_eq!(drawing_ids(&saved, 1), a_drawing);
}

#[test]
fn removing_a_picture_renumbers_none_added_after_it() {
    let (a, b) = (png(2, 30), png(3, 40));
    let mut doc = doc_with(&["first"]);
    doc.add_picture(&a, "a.png", Length::inches(1.0), Length::inches(1.0));
    doc.add_picture(&b, "b.png", Length::inches(1.0), Length::inches(1.0));
    let before = picture_ids(&doc);
    let b_drawing = drawing_ids(&doc, 2);

    let first = doc.pictures().unwrap().remove(0);
    doc.remove_content_at(&first.location).unwrap();

    let ids = picture_ids(&doc);
    assert_eq!(ids, before[1..]);
    let saved = saved(&doc);
    assert_eq!(picture_ids(&saved), ids);
    let index = saved.paragraph_count() - 1;
    assert_eq!(drawing_ids(&saved, index), b_drawing);
}

#[test]
fn pictures_added_to_word_and_google_files_save_the_ids_they_read() {
    for name in ["word/report.docx", "google/report.docx"] {
        let mut doc = corpus_doc(name);
        let existing = picture_ids(&doc);
        let end = png(2, 50);
        doc.add_picture(&end, "end.png", Length::inches(1.0), Length::inches(1.0));
        let added = picture_ids(&doc)
            .into_iter()
            .find(|picture| picture.0 == end)
            .expect("the added picture reads");
        let position = StoryRunPosition {
            location: doc.paragraph_story_location(0).unwrap().unwrap(),
            run_index: 0,
        };
        doc.insert_picture_at(&position, &png(3, 60), "start.png", None, None)
            .unwrap();
        let ids = picture_ids(&doc);
        for picture in existing.iter().chain([&added]) {
            assert!(ids.contains(picture), "{name}: {:?} kept", &picture.1);
        }

        assert_eq!(picture_ids(&saved(&doc)), ids, "{name}");
    }
}
