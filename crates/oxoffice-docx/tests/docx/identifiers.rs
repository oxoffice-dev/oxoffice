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

/// The relationship ids `document.xml.rels` gives each part type, as saved.
fn main_relationships(doc: &Document) -> Vec<(String, String)> {
    let rels = doc.part_xml("word/_rels/document.xml.rels").unwrap();
    rels.split("<Relationship ")
        .skip(1)
        .map(|element| {
            let attribute = |name: &str| {
                let start = element.find(&format!("{name}=\"")).unwrap() + name.len() + 2;
                element[start..].split('"').next().unwrap().to_owned()
            };
            let kind = attribute("Type");
            (kind.rsplit('/').next().unwrap().to_owned(), attribute("Id"))
        })
        .collect()
}

#[test]
fn parts_a_document_gains_take_relationship_ids_in_the_order_they_were_added() {
    let mut doc = doc_with(&["first", "second"]);
    let first = doc.paragraph_story_location(0).unwrap().unwrap();
    doc.create_endnote(&first, "an endnote").unwrap();
    doc.add_numbering_definition(&[ListLevel::new(ListNumberFormat::Decimal)])
        .unwrap();
    let second = doc.paragraph_story_location(1).unwrap().unwrap();
    doc.create_footnote(&second, "a footnote").unwrap();

    let relationships = main_relationships(&doc);
    let id = |kind: &str| {
        relationships
            .iter()
            .find(|(each, _)| each == kind)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("no {kind} relationship in {relationships:?}"))
    };
    let number = |id: String| -> u32 {
        id.strip_prefix("rId")
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("{id} is not an rIdN relationship id"))
    };
    let (endnotes, numbering, footnotes) = (
        number(id("endnotes")),
        number(id("numbering")),
        number(id("footnotes")),
    );
    assert!(
        endnotes < numbering && numbering < footnotes,
        "{relationships:?}"
    );

    let saved = saved(&doc);
    assert_eq!(main_relationships(&saved), relationships);
}

#[test]
fn a_picture_a_run_handle_adds_takes_a_drawing_id_no_other_picture_holds() {
    let (a, b) = (png(2, 70), png(3, 80));
    let mut doc = doc_with(&["first"]);
    doc.add_picture(&a, "a.png", Length::inches(1.0), Length::inches(1.0));
    let relationship = doc.embed_image(&b, "b.png");
    doc.add_paragraph("").add_run("").add_picture(
        &relationship,
        Length::inches(1.0),
        Length::inches(1.0),
    );
    let first = drawing_ids(&doc, 1);

    let saved = saved(&doc);

    assert_eq!(
        drawing_ids(&saved, 1),
        first,
        "the facade picture keeps its id"
    );
    let handle = drawing_ids(&saved, 2);
    assert_eq!(handle.len(), 1);
    assert_ne!(handle, first, "the handle picture takes an id of its own");
    assert_ne!(handle[0], "0");
}

/// One inline picture paragraph whose drawing has `wp:docPr id="0"`, as some
/// producers write every picture, showing the image of `relationship`.
fn producer_picture(relationship: &str, name: &str) -> String {
    format!(
        concat!(
            r#"<w:p xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing""#,
            r#" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main""#,
            r#" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture""#,
            r#" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#,
            r#"<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0">"#,
            r#"<wp:extent cx="914400" cy="914400"/><wp:docPr id="0" name="{name}"/>"#,
            r#"<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">"#,
            r#"<pic:pic><pic:nvPicPr><pic:cNvPr id="0" name="{name}"/><pic:cNvPicPr/></pic:nvPicPr>"#,
            r#"<pic:blipFill><a:blip r:embed="{relationship}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>"#,
            r#"<pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/></a:xfrm>"#,
            r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>"#,
            r#"</a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
        ),
        relationship = relationship,
        name = name
    )
}

/// The bytes of a producer's file whose two pictures both have
/// `wp:docPr id="0"` and show one image part.
fn producer_file_with_zero_drawing_ids() -> Vec<u8> {
    let body = format!(
        concat!(
            r#"<w:p><w:r><w:t>Two pictures</w:t></w:r></w:p>"#,
            "{first}{second}"
        ),
        first = producer_picture("rIdShared", "first"),
        second = producer_picture("rIdShared", "second")
    );
    let bytes = crate::support::doc_with_body(&body)
        .to_bytes()
        .expect("document saves");
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).expect("package opens");
    package.set_part("/word/media/shared.png", png(2, 90));
    package.content_types.add_default("png", "image/png");
    package
        .get_or_create_part_rels("/word/document.xml")
        .add_with_id(
            "rIdShared",
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image",
            "media/shared.png",
        );
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    out.into_inner()
}

/// Every `wp:docPr` element of the main part as written, in document order.
fn doc_pr_elements(doc: &Document) -> Vec<String> {
    let xml = doc.part_xml("word/document.xml").unwrap();
    xml.match_indices("<wp:docPr ")
        .map(|(at, _)| xml[at..at + xml[at..].find('>').unwrap() + 1].to_owned())
        .collect()
}

#[test]
fn source_drawings_keep_a_zero_id_and_only_a_handle_picture_gets_one() {
    let source = producer_file_with_zero_drawing_ids();
    let unedited = Document::from_bytes(&source).unwrap();
    let source_elements = doc_pr_elements(&unedited);
    assert_eq!(source_elements.len(), 2);
    assert!(
        source_elements
            .iter()
            .all(|element| element.contains(r#"id="0""#))
    );
    let opened =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(source.clone())).unwrap();
    let saved_unedited = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(
        saved(&unedited)
            .to_bytes_as(unedited.package_class().unwrap())
            .unwrap(),
    ))
    .unwrap();
    assert_eq!(
        saved_unedited.get_part("/word/document.xml"),
        opened.get_part("/word/document.xml"),
        "opened and saved without an edit, the main part is byte for byte the same"
    );

    let mut doc = Document::from_bytes(&source).unwrap();
    // Both source pictures are edited, and keep their id.
    assert_eq!(
        doc.set_picture_size("rIdShared", Length::inches(2.0), Length::inches(2.0))
            .unwrap(),
        2
    );
    let relationship = doc.embed_image(&png(3, 100), "added.png");
    doc.add_paragraph("").add_run("").add_picture(
        &relationship,
        Length::inches(1.0),
        Length::inches(1.0),
    );
    doc.add_table(1, 1);
    doc.table_mut(0).unwrap().cell(0, 0).unwrap().add_picture(
        &relationship,
        Length::inches(1.0),
        Length::inches(1.0),
    );

    let elements = doc_pr_elements(&saved(&doc));
    assert_eq!(elements.len(), 4, "{elements:?}");
    assert_eq!(
        elements[..2],
        source_elements[..],
        "the source pictures keep their drawing ids, byte for byte"
    );
    let added = elements[2..]
        .iter()
        .map(|element| element.split('"').nth(1).unwrap().to_owned())
        .collect::<Vec<_>>();
    assert!(
        added.iter().all(|id| id != "0") && added[0] != added[1],
        "each handle picture gets an id of its own: {elements:?}"
    );
}

#[test]
fn a_google_file_keeps_its_repeated_drawing_ids_beside_a_handle_picture() {
    let id = |element: &String| element.split('"').nth(1).unwrap().to_owned();
    let mut unedited = corpus_doc("google/newsletter.docx");
    let source_elements = doc_pr_elements(&unedited);
    let source_ids = source_elements.iter().map(id).collect::<Vec<_>>();
    let distinct = source_ids
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len();
    assert!(
        source_ids.len() > distinct,
        "the file repeats drawing ids: {source_ids:?}"
    );
    assert_eq!(doc_pr_elements(&saved(&unedited)), source_elements);

    let relationship = unedited.embed_image(&png(3, 110), "added.png");
    unedited.add_paragraph("").add_run("").add_picture(
        &relationship,
        Length::inches(1.0),
        Length::inches(1.0),
    );
    let elements = doc_pr_elements(&saved(&unedited));
    assert_eq!(
        &elements[..source_elements.len()],
        source_elements.as_slice()
    );
    let added = id(elements.last().unwrap());
    assert!(
        !source_ids.contains(&added),
        "the handle picture takes an id no source picture holds: {added}"
    );
}
