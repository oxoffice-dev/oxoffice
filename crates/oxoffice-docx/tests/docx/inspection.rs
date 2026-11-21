//! Reading a document without changing it: headings and outlines, pictures,
//! links, word and character counts, the accessibility audit, validation, and
//! the stories and text report, on new documents and on the corpus, before
//! and after a save.

use std::collections::HashMap;
use std::fs;
use std::io::Cursor;

use oxoffice_docx::{
    AccessibilityIssue, CommentDate, Document, HdrFtrType, HeaderFooterKind, IssueSeverity, Length,
    LinkInfo, NumberedOutlineNode, OutlineNode, PictureOptions, RunPosition, RunRange, StoryId,
    StoryKind, TextCommentOptions, ValidationFinding, ValidationReport, ValidationSeverity,
    validate_document_bytes, validate_document_file,
};
use oxoffice_opc::OpcPackage;
use regex::Regex;

use crate::support::{corpus_doc, corpus_root, doc_with, doc_with_body, reopen};

/// A one-pixel PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

/// The corpus report as each producer saved it.
const REPORTS: [&str; 4] = [
    "word/report.docx",
    "google/report.docx",
    "libreoffice/report.docx",
    "authored/report.docx",
];

/// EMU in one centimetre.
const CM: i64 = 360_000;

/// The bytes of a corpus file, by its path under `tests/corpus/files`.
fn corpus_bytes(name: &str) -> Vec<u8> {
    let path = corpus_root().join("files").join(name);
    fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The bytes of a package after `edit` changed it: a file as another tool,
/// or a damaged copy, would leave it.
fn edit_package(bytes: &[u8], edit: impl FnOnce(&mut OpcPackage)) -> Vec<u8> {
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    edit(&mut package);
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    out.into_inner()
}

/// One part of a package, as text.
fn package_part(bytes: &[u8], part_name: &str) -> Option<String> {
    let package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    package
        .get_part(part_name)
        .map(|data| String::from_utf8(data.to_vec()).expect("part is UTF-8"))
}

/// The relationship id and part name of the only header of a package.
fn header_relationship(bytes: &[u8]) -> (String, String) {
    let package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let main = package.main_document_part().expect("main part");
    let rels = package
        .get_part_rels(&main)
        .expect("main part relationships");
    let headers: Vec<_> = rels
        .items
        .iter()
        .filter(|rel| rel.rel_type.ends_with("/header"))
        .map(|rel| {
            (
                rel.id.clone(),
                OpcPackage::resolve_rel_target(&main, &rel.target),
            )
        })
        .collect();
    assert_eq!(headers.len(), 1, "one header: {headers:?}");
    headers.into_iter().next().unwrap()
}

/// The bytes of a package whose header part `header` stops in the middle of
/// its text "Head", as an interrupted copy leaves it.
fn truncate_header(bytes: &[u8], header: &str) -> Vec<u8> {
    edit_package(bytes, |package| {
        let xml = package.get_part(header).expect("header part").to_vec();
        let text = b"Head</w:t>";
        let at = xml
            .windows(text.len())
            .position(|window| window == text)
            .expect("the header text");
        package.set_part(header, xml[..at + 2].to_vec());
    })
}

/// The first story of this kind.
fn story(doc: &Document, kind: StoryKind) -> StoryId {
    doc.stories()
        .expect("stories are listed")
        .into_iter()
        .find(|story| story.kind() == kind)
        .unwrap_or_else(|| panic!("the document has a {kind:?} story"))
}

/// Whether two EMU sizes agree to a hundredth of a millimetre, the unit
/// LibreOffice rounds picture sizes to.
fn near(found: i64, expected: i64) -> bool {
    (found - expected).abs() <= 360
}

/// Plain options for a picture of this size and metadata.
fn picture(
    width: Length,
    height: Length,
    name: &str,
    description: Option<&str>,
    decorative: bool,
) -> PictureOptions {
    PictureOptions {
        width,
        height,
        crop: None,
        anchor: None,
        name: Some(name.to_owned()),
        description: description.map(str::to_owned),
        title: None,
        decorative,
    }
}

/// The outline as one line per heading, `level text`, indented two spaces
/// per depth.
fn outline_lines(nodes: &[OutlineNode]) -> Vec<String> {
    fn walk(nodes: &[OutlineNode], depth: usize, out: &mut Vec<String>) {
        for node in nodes {
            out.push(format!(
                "{}{} {}",
                "  ".repeat(depth),
                node.level,
                node.text
            ));
            walk(&node.children, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(nodes, 0, &mut out);
    out
}

/// The numbered outline as one line per heading, `level display text`,
/// indented two spaces per depth.
fn numbered_lines(nodes: &[NumberedOutlineNode]) -> Vec<String> {
    fn walk(nodes: &[NumberedOutlineNode], depth: usize, out: &mut Vec<String>) {
        for node in nodes {
            out.push(format!(
                "{}{} {}",
                "  ".repeat(depth),
                node.level,
                node.display_text
            ));
            walk(&node.children, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(nodes, 0, &mut out);
    out
}

fn headings(items: &[(u32, &str)]) -> Vec<(u32, String)> {
    items
        .iter()
        .map(|(level, text)| (*level, (*text).to_owned()))
        .collect()
}

fn issue(severity: IssueSeverity, message: &str) -> AccessibilityIssue {
    AccessibilityIssue {
        severity,
        message: message.to_owned(),
    }
}

fn warning(message: &str) -> ValidationFinding {
    ValidationFinding {
        severity: ValidationSeverity::Warning,
        message: message.to_owned(),
    }
}

fn error(message: &str) -> ValidationFinding {
    ValidationFinding {
        severity: ValidationSeverity::Error,
        message: message.to_owned(),
    }
}

/// The messages of the errors of a report.
fn errors(report: &ValidationReport) -> Vec<String> {
    report
        .findings
        .iter()
        .filter(|finding| finding.severity == ValidationSeverity::Error)
        .map(|finding| finding.message.clone())
        .collect()
}

// ---- Headings and outlines ----

#[test]
fn headings_in_a_block_control_are_listed_and_those_in_table_cells_only_in_the_numbered_outline() {
    let mut doc = doc_with_body(concat!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Overview</w:t></w:r></w:p>"#,
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="4000"/></w:tblGrid><w:tr><w:tc>"#,
        r#"<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>In a cell</w:t></w:r></w:p>"#,
        r#"</w:tc></w:tr></w:tbl>"#,
        r#"<w:sdt><w:sdtPr><w:id w:val="7"/></w:sdtPr><w:sdtContent>"#,
        r#"<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>In a control</w:t></w:r></w:p>"#,
        r#"</w:sdtContent></w:sdt>"#,
    ));
    let doc = reopen(&mut doc);
    // The title-only readers search the body and its block controls.
    assert_eq!(
        doc.headings(),
        headings(&[(1, "Overview"), (2, "In a control")])
    );
    assert_eq!(
        outline_lines(&doc.document_outline()),
        ["1 Overview", "  2 In a control"]
    );
    // The numbered outline also reads table cells, and says where each
    // heading sits: body child, then row, cell and paragraph.
    let outline = doc.numbered_document_outline().unwrap();
    assert_eq!(
        numbered_lines(&outline),
        ["1 Overview", "  2 In a cell", "  2 In a control"]
    );
    let sources: Vec<&[usize]> = outline[0]
        .children
        .iter()
        .map(|node| node.source.children.as_slice())
        .collect();
    assert_eq!(sources, [&[1, 0, 0, 0][..], &[2, 0][..]]);
    assert!(
        outline[0]
            .children
            .iter()
            .all(|node| node.number.is_none() && node.display_text == node.text)
    );
}

#[test]
fn the_thesis_and_newsletter_outlines_nest_as_their_heading_levels() {
    // From the corpus generator: unnumbered chapters, the contents heading
    // inside the TOC content control, and chapters three levels deep.
    let thesis = [
        "1 Abstract",
        "1 Acknowledgements",
        "1 Contents",
        "1 List of Figures and Tables",
        "1 Introduction",
        "  2 Motivation",
        "  2 Research questions",
        "  2 Contributions and structure",
        "1 Literature review",
        "  2 Inventory models",
        "    3 Single-echelon models",
        "    3 Multi-echelon models",
        "  2 Demand forecasting",
        "  2 Research gap",
        "  2 Summary",
        "1 Model and policies",
        "  2 Notation",
        "  2 Cost function",
        "  2 Adaptive base-stock policy",
        "1 Results",
        "  2 Experimental setup",
        "  2 Discussion",
        "  2 Threats to validity",
        "    3 Internal validity",
        "    3 External validity",
        "1 Conclusion",
        "  2 Future work",
        "1 References",
        "1 Appendix A. Simulation parameters",
    ];
    let mut authored = corpus_doc("authored/thesis.docx");
    for (name, doc) in [
        ("authored", corpus_doc("authored/thesis.docx")),
        ("libreoffice", corpus_doc("libreoffice/thesis.docx")),
        ("authored, saved", reopen(&mut authored)),
    ] {
        assert_eq!(outline_lines(&doc.document_outline()), thesis, "{name}");
    }
    // A newsletter whose articles all start at Heading 2: each is a root.
    for name in ["authored/newsletter.docx", "libreoffice/newsletter.docx"] {
        assert_eq!(
            outline_lines(&corpus_doc(name).document_outline()),
            [
                "2 Lyon warehouse doubles its capacity",
                "2 Meet the support night shift",
                "2 A greener fleet",
                "2 New faces",
                "2 Survey results",
            ],
            "{name}"
        );
    }
}

#[test]
fn right_to_left_headings_and_a_new_cjk_heading_survive_a_save() {
    let mut doc = corpus_doc("authored/minutes.docx");
    let before = doc.headings();
    assert!(
        before.contains(&(2, "ملخص الاجتماع".to_owned())),
        "the Arabic summary heading: {before:?}"
    );
    assert_eq!(
        corpus_doc("libreoffice/minutes.docx").headings(),
        before,
        "LibreOffice keeps every heading"
    );
    doc.add_paragraph("第八章 会议纪要").style("Heading1");
    let doc = reopen(&mut doc);
    let mut expected = before;
    expected.push((1, "第八章 会议纪要".to_owned()));
    assert_eq!(doc.headings(), expected);
    let outline = doc.document_outline();
    assert_eq!(outline.last().unwrap().text, "第八章 会议纪要");
}

#[test]
fn the_numbered_outline_of_the_word_report_keeps_its_typed_numbers_in_the_text() {
    // The numbers of the report's headings are typed, not list numbers, and
    // Word saved the heading styles under French ids (Titre1, Titre2).
    let mut doc = corpus_doc("word/report.docx");
    let saved = reopen(&mut doc);
    for (name, doc) in [("as opened", doc), ("after a save", saved)] {
        let outline = doc.numbered_document_outline().unwrap();
        assert_eq!(
            numbered_lines(&outline),
            [
                "1 1. Highlights",
                "1 2. Financial overview",
                "1 3. Operations",
                "  2 3.1 Support",
                "  2 3.2 Next steps",
                "1 Appendix A. Regional detail",
            ],
            "{name}"
        );
        let mut nodes: Vec<&NumberedOutlineNode> = outline.iter().collect();
        nodes.extend(outline.iter().flat_map(|node| &node.children));
        assert!(
            nodes
                .iter()
                .all(|node| node.number.is_none() && node.display_text == node.text),
            "{name}: no list number"
        );
    }
}

#[test]
fn a_heading_typed_with_tracked_changes_on_reads_its_inserted_text() {
    let mut doc = doc_with_body(concat!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t xml:space="preserve">Annual </w:t></w:r>"#,
        r#"<w:ins w:id="1" w:author="Ada" w:date="2026-01-01T00:00:00Z"><w:r><w:t>results</w:t></w:r></w:ins>"#,
        r#"<w:del w:id="2" w:author="Ada" w:date="2026-01-01T00:00:00Z"><w:r><w:delText>plan</w:delText></w:r></w:del></w:p>"#,
        r#"<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr>"#,
        r#"<w:ins w:id="3" w:author="Ada" w:date="2026-01-01T00:00:00Z"><w:r><w:t>Typed with tracking</w:t></w:r></w:ins></w:p>"#,
    ));
    let doc = reopen(&mut doc);
    assert_eq!(doc.text(), "Annual results\nTyped with tracking\n");
    // The numbered outline reads the accepted text already.
    assert_eq!(
        numbered_lines(&doc.numbered_document_outline().unwrap()),
        ["1 Annual results", "  2 Typed with tracking"]
    );
    assert_eq!(
        doc.headings(),
        headings(&[(1, "Annual results"), (2, "Typed with tracking")]),
        "the heading reads its tracked insertion"
    );
}

// ---- Pictures ----

#[test]
fn inline_and_floating_pictures_are_listed_with_their_size_and_bytes_after_a_save() {
    let mut doc = doc_with(&["Body"]);
    doc.add_picture(PNG, "inline.png", Length::cm(4.0), Length::cm(1.0));
    // A floating picture goes before the first paragraph.
    doc.add_anchored_image(PNG, "floating.png", Length::cm(3.0), Length::cm(2.0), true);
    let doc = reopen(&mut doc);

    let images = doc.images();
    let sizes: Vec<_> = images
        .iter()
        .map(|image| (image.is_anchor, image.width_emu, image.height_emu))
        .collect();
    assert_eq!(sizes, [(true, 3 * CM, 2 * CM), (false, 4 * CM, CM)]);
    for image in &images {
        assert_eq!(
            doc.image_data(&image.embed_id).as_deref(),
            Some(PNG),
            "{image:?}"
        );
    }

    let pictures = doc.pictures().unwrap();
    let found: Vec<_> = pictures
        .iter()
        .map(|picture| {
            (
                picture.location.story().kind(),
                picture.anchored,
                picture.width,
                picture.height,
                picture.content_type.as_str(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (StoryKind::Body, true, 3 * CM, 2 * CM, "image/png"),
            (StoryKind::Body, false, 4 * CM, CM, "image/png"),
        ]
    );
    assert!(pictures.iter().all(|picture| picture.bytes == PNG));
    assert_eq!(
        pictures
            .iter()
            .map(|picture| picture.relationship_id.as_str())
            .collect::<Vec<_>>(),
        images
            .iter()
            .map(|image| image.embed_id.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn alt_text_title_and_decorative_flag_of_a_picture_survive_a_save() {
    let mut doc = doc_with(&["Sales"]);
    doc.add_picture_with_options(
        &story(&doc, StoryKind::Body),
        PNG,
        "chart.png",
        PictureOptions {
            title: Some("Q3 sales".to_owned()),
            ..picture(
                Length::cm(5.0),
                Length::cm(2.5),
                "Sales chart",
                Some("Ventes par trimestre : +12 % <T3> & 売上"),
                false,
            )
        },
    )
    .unwrap();
    doc.add_picture_with_options(
        &story(&doc, StoryKind::Body),
        PNG,
        "rule.png",
        picture(Length::cm(5.0), Length::mm(1.0), "Divider", None, true),
    )
    .unwrap();
    let doc = reopen(&mut doc);

    let found: Vec<_> = doc
        .pictures()
        .unwrap()
        .into_iter()
        .map(|picture| {
            (
                picture.name,
                picture.description,
                picture.title,
                picture.decorative,
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (
                Some("Sales chart".to_owned()),
                Some("Ventes par trimestre : +12 % <T3> & 売上".to_owned()),
                Some("Q3 sales".to_owned()),
                false,
            ),
            (Some("Divider".to_owned()), None, None, true),
        ]
    );
    let descriptions: Vec<_> = doc
        .images()
        .into_iter()
        .map(|image| image.description)
        .collect();
    assert_eq!(
        descriptions,
        [
            Some("Ventes par trimestre : +12 % <T3> & 売上".to_owned()),
            None
        ]
    );
}

#[test]
fn a_floating_picture_added_without_alt_text_has_none() {
    let mut doc = doc_with(&["Body"]);
    doc.add_anchored_image(PNG, "photo.png", Length::cm(3.0), Length::cm(2.0), false);
    let doc = reopen(&mut doc);
    let images = doc.images();
    assert_eq!(images.len(), 1);
    assert!(images[0].is_anchor);
    assert_eq!(
        images[0].description, None,
        "a floating picture added without alt text has none"
    );
}

#[test]
fn pictures_in_a_table_cell_and_a_header_are_listed_with_their_story() {
    let mut doc = doc_with(&["Body"]);
    doc.add_table(1, 1);
    let rel_id = doc.embed_image(PNG, "cell.png");
    doc.table_mut(0).unwrap().cell(0, 0).unwrap().add_picture(
        &rel_id,
        Length::cm(2.0),
        Length::cm(4.0),
    );
    doc.set_header_image(PNG, "logo.png", Length::cm(3.0), Length::cm(1.0));
    let doc = reopen(&mut doc);

    // `images` reads the body and its tables, `pictures` every story.
    let images: Vec<_> = doc
        .images()
        .iter()
        .map(|image| (image.width_emu, image.height_emu))
        .collect();
    assert_eq!(images, [(2 * CM, 4 * CM)]);
    let listed = doc.pictures().unwrap();
    let pictures: Vec<_> = listed
        .iter()
        .map(|picture| {
            (
                picture.location.story().kind(),
                picture.width,
                picture.height,
                picture.bytes.as_slice(),
            )
        })
        .collect();
    // A header picture's bytes resolve through the header's own relationships.
    assert_eq!(
        pictures,
        [
            (StoryKind::TableCell, 2 * CM, 4 * CM, PNG),
            (StoryKind::Header, 3 * CM, CM, PNG),
        ]
    );

    // The letter's logo sits in the header of its first page, 2 cm wide.
    for name in ["authored/letter.docx", "libreoffice/letter.docx"] {
        let doc = corpus_doc(name);
        assert!(doc.images().is_empty(), "{name}: nothing in the body");
        let pictures = doc.pictures().unwrap();
        assert_eq!(pictures.len(), 1, "{name}");
        let first_page = doc
            .section_story(0, HeaderFooterKind::Header, HdrFtrType::First)
            .unwrap()
            .expect("a first-page header");
        assert_eq!(pictures[0].location.story(), first_page.story(), "{name}");
        let bytes = corpus_bytes(name);
        let package = OpcPackage::from_reader(Cursor::new(&bytes)).unwrap();
        let entries = package.zip_entries().unwrap();
        let media: Vec<_> = entries
            .iter()
            .filter(|(entry, _)| entry.trim_start_matches('/').starts_with("word/media/"))
            .collect();
        assert_eq!(media.len(), 1, "{name}");
        assert_eq!(pictures[0].bytes.as_slice(), &*media[0].1, "{name}");
        assert!(
            near(pictures[0].width, 2 * CM) && near(pictures[0].height, 2 * CM),
            "{name}: {} x {}",
            pictures[0].width,
            pictures[0].height
        );
    }
}

#[test]
fn every_report_lists_its_one_inline_picture_with_the_bytes_of_its_media_part() {
    // The generator adds the chart picture 15 cm wide; it is twice as wide
    // as tall.
    let mut word = corpus_doc("word/report.docx");
    let saved = reopen(&mut word);
    let word_picture = &word.pictures().unwrap()[0];
    assert_eq!(saved.pictures().unwrap()[0].bytes, word_picture.bytes);
    for name in REPORTS {
        let bytes = corpus_bytes(name);
        let doc = Document::from_bytes(&bytes).unwrap();
        let pictures = doc.pictures().unwrap();
        assert_eq!(pictures.len(), 1, "{name}");
        let picture = &pictures[0];
        assert_eq!(picture.location.story().kind(), StoryKind::Body, "{name}");
        assert!(!picture.anchored, "{name}");
        assert!(
            near(picture.width, 15 * CM) && near(picture.height, 15 * CM / 2),
            "{name}: {} x {}",
            picture.width,
            picture.height
        );
        let media = picture
            .media_part_name
            .as_deref()
            .expect("an internal picture");
        let package = OpcPackage::from_reader(Cursor::new(&bytes)).unwrap();
        assert_eq!(picture.bytes, package.get_part(media).unwrap(), "{name}");
        assert!(picture.bytes.starts_with(b"\x89PNG"), "{name}");
        assert_eq!(picture.content_type, "image/png", "{name}");

        let images = doc.images();
        assert_eq!(images.len(), 1, "{name}");
        assert_eq!(images[0].embed_id, picture.relationship_id, "{name}");
        assert_eq!(
            (images[0].width_emu, images[0].height_emu),
            (picture.width, picture.height),
            "{name}"
        );
    }
}

#[test]
fn the_newsletter_lists_its_floating_pictures_but_not_its_shapes_or_text_boxes() {
    let mut authored = corpus_doc("authored/newsletter.docx");
    for (name, doc) in [
        ("authored", corpus_doc("authored/newsletter.docx")),
        ("libreoffice", corpus_doc("libreoffice/newsletter.docx")),
        ("authored, saved", reopen(&mut authored)),
    ] {
        // Two pictures, three shapes and two text boxes, by the generator.
        let xml = doc.part_xml("word/document.xml").unwrap();
        assert_eq!(xml.matches("<wp:docPr ").count(), 7, "{name}");

        let images = doc.images();
        let names: Vec<_> = images.iter().map(|image| image.name.as_deref()).collect();
        assert_eq!(names, [Some("Picture 3"), Some("Picture 4")], "{name}");
        assert!(images.iter().all(|image| image.is_anchor), "{name}");
        // 3.6 cm and 2.4 cm wide.
        assert!(near(images[0].width_emu, 36 * CM / 10), "{name}");
        assert!(near(images[1].width_emu, 24 * CM / 10), "{name}");

        let pictures = doc.pictures().unwrap();
        assert_eq!(pictures.len(), 2, "{name}");
        assert!(pictures.iter().all(|picture| picture.anchored), "{name}");
    }
}

#[test]
fn a_picture_repeated_in_the_long_report_shares_one_media_part() {
    // The generator adds the same chart to chapters 1, 13 and 25.
    let doc = corpus_doc("authored/long-report.docx");
    let images = doc.images();
    let names: Vec<_> = images.iter().map(|image| image.name.as_deref()).collect();
    assert_eq!(
        names,
        [Some("Picture 1"), Some("Picture 2"), Some("Picture 3")]
    );
    assert!(
        images
            .iter()
            .all(|image| image.embed_id == images[0].embed_id)
    );
    let pictures = doc.pictures().unwrap();
    assert_eq!(pictures.len(), 3);
    assert!(pictures.iter().all(
        |picture| picture.media_part_name == pictures[0].media_part_name
            && picture.bytes == pictures[0].bytes
    ));
    let paths: Vec<_> = pictures
        .iter()
        .map(|picture| picture.location.index_path().to_vec())
        .collect();
    assert!(paths[0] < paths[1] && paths[1] < paths[2], "{paths:?}");
}

#[test]
fn a_chart_is_not_listed_as_an_image() {
    let doc = corpus_doc("authored/manual.docx");
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(
        xml.contains("drawingml/2006/chart"),
        "the manual has a chart"
    );
    assert!(doc.pictures().unwrap().is_empty(), "and no picture");
    for image in doc.images() {
        assert!(
            doc.image_data(&image.embed_id).is_some(),
            "every listed image has its bytes: {image:?}"
        );
    }
}

// ---- Links ----

#[test]
fn an_internal_link_names_its_bookmark_and_no_url() {
    let mut doc = Document::new();
    doc.add_paragraph("Results").style("Heading1");
    doc.add_bookmark(
        "Results",
        RunRange {
            start: RunPosition {
                body_index: 0,
                run_index: 0,
            },
            end: RunPosition {
                body_index: 0,
                run_index: 1,
            },
        },
    )
    .unwrap();
    doc.add_paragraph("See ").add_internal_hyperlink(
        "the results",
        "Results",
        Some("Go to the results"),
    );
    let doc = reopen(&mut doc);

    assert_eq!(
        doc.links(),
        [LinkInfo {
            text: "the results".to_owned(),
            url: None,
            anchor: Some("Results".to_owned()),
            rel_id: None,
        }]
    );
    let bookmarks: Vec<_> = doc
        .bookmarks()
        .iter()
        .map(|bookmark| {
            (
                bookmark.name().map(str::to_owned),
                bookmark.text().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        bookmarks,
        [(Some("Results".to_owned()), "Results".to_owned())]
    );
    assert_eq!(doc.text(), "Results\nSee the results\n");
}

#[test]
fn a_link_written_as_a_hyperlink_field_reads_its_url() {
    // Word may write a link as a HYPERLINK field instead of w:hyperlink.
    let mut doc = doc_with_body(concat!(
        r#"<w:p><w:r><w:t xml:space="preserve">Visit </w:t></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        r#"<w:r><w:instrText xml:space="preserve"> HYPERLINK "https://example.com/a?b=1&amp;c=2" </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>our site</w:t></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t>.</w:t></w:r></w:p>"#,
    ));
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.links(),
        [LinkInfo {
            text: "our site".to_owned(),
            url: Some("https://example.com/a?b=1&c=2".to_owned()),
            anchor: None,
            rel_id: None,
        }]
    );
    assert_eq!(doc.text(), "Visit our site.\n");
}

#[test]
fn links_in_a_header_and_a_table_cell_are_found_story_by_story() {
    let mut doc = doc_with(&["Body "]);
    doc.append_hyperlink("home", "https://example.com/");
    doc.add_table(1, 1);
    let header = doc
        .create_section_story(0, HeaderFooterKind::Header, HdrFtrType::Default)
        .unwrap();
    doc.add_hyperlink_to_story(
        &header,
        "Menu du café",
        "https://exemple.fr/café?jour=lundi&plat=crêpe",
    )
    .unwrap();
    let cell = story(&doc, StoryKind::TableCell);
    doc.add_hyperlink_to_story(&cell, "cell link", "https://example.com/cell")
        .unwrap();
    let doc = reopen(&mut doc);

    let found: Vec<_> = doc
        .story_link_snapshots()
        .unwrap()
        .into_iter()
        .map(|(location, link)| (location.story().kind(), link.text, link.url))
        .collect();
    let link = |kind, text: &str, url: &str| (kind, text.to_owned(), Some(url.to_owned()));
    assert_eq!(
        found,
        [
            link(StoryKind::Body, "home", "https://example.com/"),
            link(
                StoryKind::TableCell,
                "cell link",
                "https://example.com/cell"
            ),
            link(
                StoryKind::Header,
                "Menu du café",
                "https://exemple.fr/café?jour=lundi&plat=crêpe"
            ),
        ]
    );
    let header = story(&doc, StoryKind::Header);
    let in_header: Vec<_> = doc
        .story_links(&header)
        .unwrap()
        .into_iter()
        .map(|(_, link)| link.text)
        .collect();
    assert_eq!(in_header, ["Menu du café"]);
    // `links` reads the body paragraphs only.
    let body: Vec<_> = doc.links().into_iter().map(|link| link.text).collect();
    assert_eq!(body, ["home"]);
}

#[test]
fn the_letter_and_cv_links_read_back_with_their_urls_in_both_producers() {
    let letter = [
        ("our website", "https://www.northwind.example/pricing/2027"),
        (
            "customer portal",
            "https://portal.northwind.example/renewals?ref=NW-2026-114",
        ),
        (
            "julien.moreau@northwind.example",
            "mailto:julien.moreau@northwind.example",
        ),
    ];
    let pairs = |links: Vec<LinkInfo>| -> Vec<(String, String)> {
        links
            .into_iter()
            .map(|link| (link.text, link.url.unwrap_or_default()))
            .collect()
    };
    let expected: Vec<_> = letter
        .iter()
        .map(|(text, url)| ((*text).to_owned(), (*url).to_owned()))
        .collect();
    let mut authored = corpus_doc("authored/letter.docx");
    for (name, doc) in [
        ("authored", corpus_doc("authored/letter.docx")),
        ("libreoffice", corpus_doc("libreoffice/letter.docx")),
        ("authored, saved", reopen(&mut authored)),
    ] {
        assert_eq!(pairs(doc.links()), expected, "{name}");
    }

    // The CV's links sit in the cells of its layout table.
    let cv = [
        (
            "elena.marchetti@example.com",
            "mailto:elena.marchetti@example.com",
        ),
        (
            "linkedin.com/in/elena-marchetti",
            "https://www.linkedin.com/in/elena-marchetti",
        ),
        ("github.com/emarchetti", "https://github.com/emarchetti"),
        ("video", "https://www.youtube.com/watch?v=example-talk"),
    ];
    for name in ["authored/cv.docx", "libreoffice/cv.docx"] {
        let found: Vec<_> = corpus_doc(name)
            .story_link_snapshots()
            .unwrap()
            .into_iter()
            .map(|(location, link)| {
                assert_eq!(location.story().kind(), StoryKind::TableCell, "{name}");
                (link.text, link.url.unwrap_or_default())
            })
            .collect();
        let expected: Vec<_> = cv
            .iter()
            .map(|(text, url)| ((*text).to_owned(), (*url).to_owned()))
            .collect();
        assert_eq!(found, expected, "{name}");
    }
}

#[test]
fn every_table_of_contents_link_of_the_thesis_points_at_its_heading_bookmark() {
    let mut authored = corpus_doc("authored/thesis.docx");
    for (name, doc) in [
        ("authored", corpus_doc("authored/thesis.docx")),
        ("libreoffice", corpus_doc("libreoffice/thesis.docx")),
        ("authored, saved", reopen(&mut authored)),
    ] {
        let bookmarks: HashMap<String, String> = doc
            .bookmarks()
            .iter()
            .filter_map(|bookmark| Some((bookmark.name()?.to_owned(), bookmark.text().to_owned())))
            .collect();
        let links = doc.links();
        // 23 numbered headings and 3 captions, in the content control of
        // the contents and in the list of figures.
        assert_eq!(links.len(), 26, "{name}");
        for link in links {
            assert_eq!((link.url.as_deref(), link.rel_id.as_deref()), (None, None));
            let anchor = link.anchor.expect("an internal link");
            let target = bookmarks
                .get(&anchor)
                .unwrap_or_else(|| panic!("{name}: no bookmark {anchor}"));
            // An entry reads number, tab, title, tab, page; the bookmark
            // holds the title.
            assert!(
                link.text.split('\t').any(|field| field == target),
                "{name}: {:?} does not show {target:?}",
                link.text
            );
        }
    }
}

// ---- Word and character counts ----

#[test]
fn words_and_characters_are_counted_in_paragraphs_cells_and_controls() {
    let mut doc = doc_with_body(concat!(
        r#"<w:p><w:r><w:t>Hello wide world</w:t></w:r></w:p>"#,
        r#"<w:p><w:r><w:t>a</w:t><w:tab/><w:t>b</w:t></w:r></w:p>"#,
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="4000"/></w:tblGrid><w:tr><w:tc>"#,
        r#"<w:p><w:r><w:t>cell one</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
        r#"<w:sdt><w:sdtPr><w:id w:val="7"/></w:sdtPr><w:sdtContent>"#,
        r#"<w:p><w:r><w:t>in control</w:t></w:r></w:p></w:sdtContent></w:sdt>"#,
        r#"<w:p><w:r><w:t>Été 😀 مرحبا</w:t></w:r></w:p>"#,
    ));
    let doc = reopen(&mut doc);
    // 3 + 2 + 2 + 2 + 3 words; a tab separates words; an emoji or an
    // Arabic letter is one character.
    assert_eq!(doc.word_count(), 12);
    assert_eq!(doc.character_count(true), 16 + 3 + 8 + 10 + 11);
    assert_eq!(doc.character_count(false), 14 + 2 + 7 + 9 + 9);

    let empty = Document::new();
    assert_eq!(
        (
            empty.word_count(),
            empty.character_count(true),
            empty.character_count(false)
        ),
        (0, 0, 0)
    );
}

#[test]
fn deleted_text_and_deleted_rows_are_not_counted() {
    let mut doc = doc_with_body(concat!(
        r#"<w:p><w:r><w:t xml:space="preserve">one two </w:t></w:r>"#,
        r#"<w:del w:id="1" w:author="Ada" w:date="2026-01-01T00:00:00Z"><w:r><w:delText>gone words</w:delText></w:r></w:del></w:p>"#,
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="4000"/></w:tblGrid>"#,
        r#"<w:tr><w:tc><w:p><w:r><w:t>kept row</w:t></w:r></w:p></w:tc></w:tr>"#,
        // A deleted row's text is plain: only the row's mark removes it.
        r#"<w:tr><w:trPr><w:del w:id="2" w:author="Ada" w:date="2026-01-01T00:00:00Z"/></w:trPr><w:tc><w:p>"#,
        r#"<w:r><w:t>deleted row</w:t></w:r>"#,
        r#"</w:p></w:tc></w:tr></w:tbl><w:p/>"#,
    ));
    let doc = reopen(&mut doc);
    assert_eq!(doc.word_count(), 4, "one two kept row");
    assert_eq!(doc.character_count(true), 8 + 8);
    assert_eq!(doc.character_count(false), 6 + 7);
}

#[test]
fn text_inserted_with_tracked_changes_is_counted() {
    let mut doc = doc_with_body(concat!(
        r#"<w:p><w:r><w:t xml:space="preserve">one two </w:t></w:r>"#,
        r#"<w:ins w:id="1" w:author="Ada" w:date="2026-01-01T00:00:00Z"><w:r><w:t>three</w:t></w:r></w:ins></w:p>"#,
    ));
    let doc = reopen(&mut doc);
    assert_eq!(doc.text(), "one two three\n");
    assert_eq!(doc.word_count(), 3, "the inserted word is counted");
    assert_eq!(doc.character_count(true), 13);
    assert_eq!(doc.character_count(false), 11);
}

/// Words, non-space characters and all characters of the body as
/// `document.xml` holds it: the text of `w:t`, with `w:tab` and `w:br` as one
/// white space each, paragraph by paragraph. Deleted text (`w:delText`) and
/// field codes are not `w:t`.
fn xml_counts(xml: &str) -> (usize, usize, usize) {
    let token =
        Regex::new(r#"<w:t(?: [^>]*)?>([^<]*)</w:t>|<w:tab/>|<w:br(?: [^>]*)?/>|</w:p>"#).unwrap();
    let (mut words, mut visible, mut all) = (0, 0, 0);
    let mut paragraph = String::new();
    for found in token.captures_iter(xml) {
        match found.get(1) {
            Some(text) => paragraph.push_str(
                &text
                    .as_str()
                    .replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&quot;", "\"")
                    .replace("&apos;", "'")
                    .replace("&amp;", "&"),
            ),
            None if found[0].starts_with("</w:p") => {
                words += paragraph.split_whitespace().count();
                visible += paragraph.chars().filter(|c| !c.is_whitespace()).count();
                all += paragraph.chars().count();
                paragraph.clear();
            }
            None => paragraph.push(' '),
        }
    }
    (words, visible, all)
}

#[test]
fn the_report_counts_the_same_in_every_producer_as_its_saved_xml() {
    let stored = package_part(&corpus_bytes("word/report.docx"), "/word/document.xml").unwrap();
    let (words, visible, with_spaces) = xml_counts(&stored);
    assert!(words > 200, "{words}");
    let mut word = corpus_doc("word/report.docx");
    let saved = reopen(&mut word);
    let mut docs = vec![("word, saved".to_owned(), saved)];
    docs.extend(REPORTS.map(|name| (name.to_owned(), corpus_doc(name))));
    for (name, doc) in docs {
        assert_eq!(
            (
                doc.word_count(),
                doc.character_count(false),
                doc.character_count(true)
            ),
            (words, visible, with_spaces),
            "{name}"
        );
    }
}

// ---- Accessibility audit ----

#[test]
fn a_document_with_metadata_alt_text_and_ordered_headings_passes_the_audit() {
    let mut doc = Document::new();
    doc.set_title("Annual report");
    doc.set_author("Ada");
    doc.add_paragraph("Overview").style("Heading1");
    doc.add_paragraph("Text");
    doc.add_paragraph("Details").style("Heading2");
    doc.add_picture_with_options(
        &story(&doc, StoryKind::Body),
        PNG,
        "chart.png",
        picture(
            Length::cm(4.0),
            Length::cm(2.0),
            "Chart",
            Some("Bar chart of sales by quarter"),
            false,
        ),
    )
    .unwrap();
    // Two empty paragraphs are spacing, not a problem.
    doc.add_paragraph("");
    doc.add_paragraph("");
    doc.add_paragraph("Summary").style("Heading1");
    let doc = reopen(&mut doc);
    assert_eq!(doc.audit_accessibility(), []);
}

#[test]
fn each_accessibility_problem_is_reported_with_its_severity() {
    let mut doc = Document::new();
    doc.add_paragraph("Scope").style("Heading1");
    doc.add_paragraph("Methods in detail").style("Heading3");
    for (name, description) in [("Sales chart", None), ("Logo", Some(""))] {
        doc.add_picture_with_options(
            &story(&doc, StoryKind::Body),
            PNG,
            "picture.png",
            picture(Length::cm(4.0), Length::cm(2.0), name, description, false),
        )
        .unwrap();
    }
    for _ in 0..3 {
        doc.add_paragraph("");
    }
    doc.add_paragraph("End");
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.audit_accessibility(),
        [
            issue(IssueSeverity::Warning, "Document has no title"),
            issue(IssueSeverity::Info, "Document has no author"),
            issue(
                IssueSeverity::Error,
                "Image \"Sales chart\" has no alt text"
            ),
            issue(IssueSeverity::Error, "Image \"Logo\" has no alt text"),
            issue(
                IssueSeverity::Warning,
                "Heading level gap: h1 -> h3 (\"Methods in detail\")"
            ),
            issue(
                IssueSeverity::Info,
                "3 consecutive empty paragraphs (consider using spacing instead)"
            ),
        ]
    );
}

#[test]
fn the_audit_of_the_corpus_names_every_picture_without_alt_text() {
    let docpr = Regex::new(r#"<wp:docPr [^>]*>"#).unwrap();
    let name = Regex::new(r#" name="([^"]*)""#).unwrap();
    for file in REPORTS {
        let bytes = corpus_bytes(file);
        let doc = Document::from_bytes(&bytes).unwrap();
        // What the package says: its core properties (Google writes none)
        // and the drawing of its one picture, without a description.
        let core = package_part(&bytes, "/docProps/core.xml").unwrap_or_default();
        let xml = doc.part_xml("word/document.xml").unwrap();
        let drawings: Vec<_> = docpr.find_iter(&xml).map(|m| m.as_str()).collect();
        assert_eq!(drawings.len(), 1, "{file}");
        assert!(!drawings[0].contains("descr="), "{file}");
        let picture = &name.captures(drawings[0]).unwrap()[1];

        let mut expected = Vec::new();
        if !core.contains("<dc:title>") {
            expected.push(issue(IssueSeverity::Warning, "Document has no title"));
        }
        if !core.contains("<dc:creator>") {
            expected.push(issue(IssueSeverity::Info, "Document has no author"));
        }
        expected.push(issue(
            IssueSeverity::Error,
            &format!("Image \"{picture}\" has no alt text"),
        ));
        assert_eq!(doc.audit_accessibility(), expected, "{file}");
    }
    // The floating pictures of the newsletter, not its shapes.
    assert_eq!(
        corpus_doc("authored/newsletter.docx").audit_accessibility(),
        [
            issue(IssueSeverity::Error, "Image \"Picture 3\" has no alt text"),
            issue(IssueSeverity::Error, "Image \"Picture 4\" has no alt text"),
        ]
    );
}

#[test]
fn a_decorative_picture_needs_no_alt_text() {
    let mut doc = doc_with(&["Body"]);
    doc.set_title("Title");
    doc.set_author("Ada");
    doc.add_picture_with_options(
        &story(&doc, StoryKind::Body),
        PNG,
        "rule.png",
        picture(Length::cm(5.0), Length::mm(1.0), "Divider", None, true),
    )
    .unwrap();
    let doc = reopen(&mut doc);
    assert!(doc.pictures().unwrap()[0].decorative);
    assert_eq!(
        doc.audit_accessibility(),
        [],
        "a decorative picture needs no alt text"
    );
}

#[test]
fn a_streak_of_empty_paragraphs_is_one_issue() {
    let mut doc = doc_with(&["Before", "", "", "", "", "After"]);
    doc.set_title("Title");
    doc.set_author("Ada");
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.audit_accessibility(),
        [issue(
            IssueSeverity::Info,
            "4 consecutive empty paragraphs (consider using spacing instead)"
        )],
        "one issue for one streak of empty paragraphs"
    );
}

// ---- Validation ----

#[test]
fn validation_warns_about_content_metadata_and_headings_and_changes_nothing() {
    let mut doc = Document::new();
    let before = doc.to_bytes().unwrap();
    let report = doc.validate().unwrap();
    assert_eq!(
        report.findings,
        [
            warning("Document has no content (no paragraphs or tables)"),
            warning("Missing document title"),
            warning("Missing document author"),
        ]
    );
    assert!(report.is_valid(), "warnings do not invalidate");
    assert_eq!(
        doc.to_bytes().unwrap(),
        before,
        "validation leaves the document as it was"
    );

    let mut doc = Document::new();
    doc.set_title("Title");
    doc.set_author("Ada");
    doc.add_paragraph("Scope").style("Heading1");
    doc.add_paragraph("");
    doc.add_paragraph("");
    doc.add_paragraph("Detail").style("Heading3");
    let doc = reopen(&mut doc);
    let report = doc.validate().unwrap();
    assert_eq!(
        report.findings,
        [
            warning("2 empty paragraph(s) found"),
            warning("Heading level gap: Heading1 -> Heading3 (skipped level(s))"),
        ]
    );
    assert!(report.is_valid());
}

#[test]
fn validation_names_a_missing_part_an_undeclared_part_and_malformed_xml() {
    let mut doc = doc_with(&["Body"]);
    doc.set_title("Title");
    doc.set_author("Ada");
    doc.set_header("Head");
    let bytes = doc.to_bytes().unwrap();
    assert!(validate_document_bytes(&bytes).unwrap().findings.is_empty());
    let (rel_id, header) = header_relationship(&bytes);

    let missing = edit_package(&bytes, |package| {
        package.remove_part(&header);
    });
    let report = validate_document_bytes(&missing).unwrap();
    assert!(!report.is_valid());
    assert_eq!(
        errors(&report),
        [format!(
            "relationship {rel_id} points at missing part {header}"
        )]
    );

    let undeclared = edit_package(&bytes, |package| {
        package.set_part("/word/notes.bin", b"notes".to_vec());
    });
    assert_eq!(
        errors(&validate_document_bytes(&undeclared).unwrap()),
        ["part /word/notes.bin has no declared content type"]
    );

    let truncated = truncate_header(&bytes, &header);
    let report = validate_document_bytes(&truncated).unwrap();
    let found = errors(&report);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].starts_with(&format!("part {header} is not well-formed XML")),
        "{found:?}"
    );
    // The opened document and the file on disk say the same.
    assert_eq!(
        Document::from_bytes(&truncated)
            .unwrap()
            .validate()
            .unwrap(),
        report
    );
    let path = std::env::temp_dir().join(format!(
        "oxoffice-docx-inspection-{}.docx",
        std::process::id()
    ));
    fs::write(&path, &truncated).unwrap();
    let from_file = validate_document_file(&path);
    let _ = fs::remove_file(&path);
    assert_eq!(from_file.unwrap(), report);

    assert!(validate_document_bytes(b"not a package").is_err());
}

#[test]
fn section_validation_reports_bad_page_sizes_and_validate_includes_them() {
    let mut doc = doc_with_body(concat!(
        r#"<w:p><w:pPr><w:sectPr><w:pgSz w:w="12240"/></w:sectPr></w:pPr><w:r><w:t>one</w:t></w:r></w:p>"#,
        r#"<w:p><w:pPr><w:sectPr><w:pgSz w:w="0" w:h="15840"/></w:sectPr></w:pPr><w:r><w:t>two</w:t></w:r></w:p>"#,
        r#"<w:p><w:r><w:t>three</w:t></w:r></w:p>"#,
    ));
    let expected = [
        warning("section 0 has incomplete page size: width or height is missing"),
        error("section 1 has nonpositive page size"),
    ];
    assert_eq!(doc.validate_sections().findings, expected);
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.validate_sections().findings,
        expected,
        "a save keeps the sizes as written"
    );
    let report = doc.validate().unwrap();
    assert!(!report.is_valid());
    for finding in &expected {
        assert!(report.findings.contains(finding), "{:?}", report.findings);
    }

    assert!(Document::new().validate_sections().findings.is_empty());
    // A portrait then a landscape section.
    assert!(
        corpus_doc("word/report.docx")
            .validate_sections()
            .findings
            .is_empty()
    );
}

#[test]
fn files_saved_by_word_google_and_libreoffice_validate_without_errors() {
    for name in [
        "word/report.docx",
        "google/report.docx",
        "libreoffice/report.docx",
    ] {
        let report = validate_document_bytes(&corpus_bytes(name)).unwrap();
        assert!(errors(&report).is_empty(), "{name}: {:?}", report.findings);
    }
    // python-docx names comment styles that its template does not define.
    let bytes = corpus_bytes("authored/report.docx");
    let styles = package_part(&bytes, "/word/styles.xml").unwrap();
    assert!(!styles.contains(r#"w:styleId="CommentText""#));
    assert!(!styles.contains(r#"w:styleId="CommentReference""#));
    assert_eq!(
        errors(&validate_document_bytes(&bytes).unwrap()),
        [
            "paragraph style \"CommentText\" used in /word/comments.xml is not defined",
            "character style \"CommentReference\" used in /word/comments.xml is not defined",
            "character style \"CommentReference\" used in /word/document.xml is not defined",
        ]
    );
    // An edited Word file still validates.
    let mut doc = corpus_doc("word/report.docx");
    doc.add_paragraph("Added after the review.");
    let report = reopen(&mut doc).validate().unwrap();
    assert!(errors(&report).is_empty(), "{:?}", report.findings);
}

// ---- Stories and the text report ----

#[test]
fn the_text_report_lists_the_body_then_each_story_part_under_its_label() {
    let mut doc = doc_with(&["Body one", "Body two"]);
    doc.add_table(1, 2);
    doc.table_mut(0)
        .unwrap()
        .cell(0, 0)
        .unwrap()
        .set_text("left");
    doc.table_mut(0)
        .unwrap()
        .cell(0, 1)
        .unwrap()
        .set_text("right");
    doc.set_header("Head line");
    doc.set_footer("Foot line");
    doc.add_comment_on_text(
        "two",
        "Ada",
        "Look here",
        TextCommentOptions {
            date: CommentDate::Undated,
            ..Default::default()
        },
    )
    .unwrap();
    let first = doc.paragraph_story_location(0).unwrap().unwrap();
    doc.create_footnote(&first, "Note text").unwrap();
    let doc = reopen(&mut doc);

    let stories = doc.stories().unwrap();
    let kinds: Vec<_> = stories.iter().map(StoryId::kind).collect();
    assert_eq!(
        kinds,
        [
            StoryKind::Body,
            StoryKind::TableCell,
            StoryKind::TableCell,
            StoryKind::Header,
            StoryKind::Footer,
            StoryKind::Footnote,
            StoryKind::Comment,
        ]
    );
    let part = |kind| story(&doc, kind).part_name().to_owned();
    let report = doc.text_report();
    assert_eq!(
        report.text,
        format!(
            "Body one\nBody two\nleft\tright\t\n\
             --- header ({}) ---\nHead line\n\
             --- footer ({}) ---\nFoot line\n\
             --- footnote ({}) ---\n Note text\n\
             --- comment ({}) ---\nLook here\n",
            part(StoryKind::Header),
            part(StoryKind::Footer),
            part(StoryKind::Footnote),
            part(StoryKind::Comment),
        )
    );
    assert_eq!(
        (report.scope, report.revision_view),
        ("all-supported-stories", "accepted")
    );
    assert!(report.diagnostics.is_empty());

    // The body and its cells are the body view: the other stories are the rest.
    let others: Vec<_> = doc
        .other_story_texts()
        .unwrap()
        .into_iter()
        .map(|text| {
            let items: Vec<_> = text
                .items
                .into_iter()
                .map(|item| (item.index_path, item.text))
                .collect();
            (text.story.kind(), items)
        })
        .collect();
    let item = |text: &str| vec![(vec![0], text.to_owned())];
    assert_eq!(
        others,
        [
            (StoryKind::Header, item("Head line")),
            (StoryKind::Footer, item("Foot line")),
            (StoryKind::Footnote, item(" Note text")),
            (StoryKind::Comment, item("Look here")),
        ]
    );
}

#[test]
fn the_text_report_keeps_a_header_table_in_reading_order() {
    let mut doc = doc_with(&["Body"]);
    let header = concat!(
        r#"<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
        r#"<w:p><w:r><w:t>Northwind Analytics</w:t></w:r></w:p>"#,
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="4000"/><w:gridCol w:w="4000"/></w:tblGrid>"#,
        r#"<w:tr><w:tc><w:p><w:r><w:t>Reference</w:t></w:r></w:p></w:tc>"#,
        r#"<w:tc><w:p><w:r><w:t>NW-2026-114</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
        r#"<w:p><w:r><w:t>Confidential</w:t></w:r></w:p></w:hdr>"#,
    );
    doc.set_raw_header_with_images(header.as_bytes().to_vec(), &[], HdrFtrType::Default);
    let doc = reopen(&mut doc);
    let text = doc.text_report().text;
    let label = text.find("--- header (").expect("a header section");
    let at = |needle: &str| {
        text[label..]
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} is in the header section: {text}"))
    };
    // Each header line is reported once.
    assert_eq!(text.matches("NW-2026-114").count(), 1, "{text}");
    assert!(at("Northwind Analytics") < at("Reference"), "{text}");
    assert!(
        at("NW-2026-114") < at("Confidential"),
        "the header table reads before the header's last line: {text}"
    );
}

#[test]
fn the_text_report_keeps_nested_and_successive_header_tables_in_reading_order() {
    let mut doc = doc_with(&["Body"]);
    let cell = |text: &str| format!("<w:tc><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc>");
    let paragraph = |text: &str| format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>");
    let grid =
        "<w:tblPr/><w:tblGrid><w:gridCol w:w=\"4000\"/><w:gridCol w:w=\"4000\"/></w:tblGrid>";
    // First table: a cell holding a nested table, then a line after it in
    // the same cell; second table right after a line of the header.
    let header = format!(
        concat!(
            r#"<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
            "{}<w:tbl>{grid}<w:tr><w:tc>{}<w:tbl>{grid}<w:tr>{}{}</w:tr></w:tbl>{}</w:tc>{}</w:tr></w:tbl>",
            "{}<w:tbl>{grid}<w:tr>{}{}</w:tr></w:tbl>{}</w:hdr>"
        ),
        paragraph("H1"),
        paragraph("A1"),
        cell("N1"),
        cell("N2"),
        paragraph("A2"),
        cell("B1"),
        paragraph("H2"),
        cell("C1"),
        cell("C2"),
        paragraph("H3"),
        grid = grid,
    );
    doc.set_raw_header_with_images(header.into_bytes(), &[], HdrFtrType::Default);
    let doc = reopen(&mut doc);
    let text = doc.text_report().text;
    let label = text.find("--- header (").expect("a header section");
    let lines: Vec<&str> = text[label..]
        .lines()
        .skip(1)
        .take_while(|line| !line.starts_with("--- "))
        .collect();
    assert_eq!(
        lines,
        ["H1", "A1", "N1", "N2", "A2", "B1", "H2", "C1", "C2", "H3"],
        "{text}"
    );
}

#[test]
fn a_deleted_paragraph_mark_joins_two_words_into_one_as_accepting_does() {
    let doc = doc_with_body(concat!(
        r#"<w:p><w:pPr><w:rPr><w:del w:id="1" w:author="Ana" w:date="2026-11-21T00:00:00Z"/></w:rPr></w:pPr>"#,
        r#"<w:r><w:t>end</w:t></w:r></w:p><w:p><w:r><w:t>start here</w:t></w:r></w:p>"#,
    ));
    assert_eq!(doc.text(), "endstart here\n");
    assert_eq!(doc.word_count(), 2);
    assert_eq!(doc.character_count(false), 12);
}

/// The lines of the text report's part whose label starts with `label`.
fn report_lines(doc: &Document, label: &str) -> Vec<String> {
    let text = doc.text_report().text;
    let at = text
        .find(label)
        .unwrap_or_else(|| panic!("{label} is in the report: {text}"));
    text[at..]
        .lines()
        .skip(1)
        .take_while(|line| !line.starts_with("--- "))
        .map(str::to_owned)
        .collect()
}

/// A floating text box holding `content`, as Word writes it: DrawingML in
/// `mc:Choice` and a VML copy in `mc:Fallback`.
fn text_box_run(id: u32, content: &str) -> String {
    format!(
        concat!(
            r#"<w:r><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" "#,
            r#"xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" "#,
            r#"xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" "#,
            r#"xmlns:v="urn:schemas-microsoft-com:vml"><mc:Choice Requires="wps"><w:drawing>"#,
            r#"<wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="914400" cy="914400"/>"#,
            r#"<wp:docPr id="{id}" name="Text Box {id}"/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">"#,
            r#"<a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp>"#,
            r#"<wps:cNvSpPr txBox="1"/><wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/></a:xfrm>"#,
            r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></wps:spPr><wps:txbx><w:txbxContent>{content}"#,
            r#"</w:txbxContent></wps:txbx><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:inline>"#,
            r#"</w:drawing></mc:Choice><mc:Fallback><w:pict><v:shape id="tb{id}"><v:textbox><w:txbxContent>"#,
            r#"{content}</w:txbxContent></v:textbox></v:shape></w:pict></mc:Fallback></mc:AlternateContent></w:r>"#,
        ),
        id = id,
        content = content,
    )
}

fn line(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

fn one_row_table(cells: &[&str]) -> String {
    let grid: String = cells.iter().map(|_| r#"<w:gridCol w:w="2000"/>"#).collect();
    let row: String = cells
        .iter()
        .map(|text| format!("<w:tc>{}</w:tc>", line(text)))
        .collect();
    format!("<w:tbl><w:tblPr/><w:tblGrid>{grid}</w:tblGrid><w:tr>{row}</w:tr></w:tbl>")
}

#[test]
fn the_text_report_reads_a_body_text_box_and_its_table_in_reading_order() {
    let first = format!(
        "{}{}{}",
        line("Intro"),
        one_row_table(&["C1", "C2"]),
        line("After")
    );
    let doc = doc_with_body(&format!(
        "<w:p><w:r><w:t>Body</w:t></w:r>{}</w:p><w:p>{}</w:p>",
        text_box_run(81, &first),
        text_box_run(82, &line("Second")),
    ));
    assert_eq!(doc.text(), "Body\n\n");
    // The body view leaves text boxes out: the report reads them once, the
    // cells of the table where it sits.
    assert_eq!(
        report_lines(&doc, "--- text_box ("),
        ["Intro", "C1", "C2", "After", "Second"]
    );
}

#[test]
fn the_text_report_keeps_a_table_inside_a_content_control_in_reading_order() {
    let mut doc = doc_with(&["Body"]);
    let header = format!(
        concat!(
            r#"<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
            "{}<w:sdt><w:sdtPr><w:alias w:val=\"Letterhead\"/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt>",
            "{}{}{}</w:hdr>"
        ),
        line("H1"),
        one_row_table(&["A", "B"]),
        line("H2"),
        one_row_table(&["C", "D"]),
        line("H3"),
    );
    doc.set_raw_header_with_images(header.into_bytes(), &[], HdrFtrType::Default);
    let doc = reopen(&mut doc);
    let lines = report_lines(&doc, "--- header (");
    // The control is a line of its own (it holds no paragraph of its own).
    assert_eq!(lines, ["H1", "", "A", "B", "H2", "C", "D", "H3"]);
}

#[test]
fn the_text_report_keeps_a_table_inside_a_footnote_before_the_next_note() {
    let mut doc = doc_with(&["Body"]);
    for text in ["first", "second"] {
        let location = doc.paragraph_story_location(0).unwrap().unwrap();
        doc.create_footnote(&location, text).unwrap();
    }
    let bytes = doc.to_bytes().unwrap();
    let edited = edit_package(&bytes, |package| {
        let xml =
            String::from_utf8(package.get_part("/word/footnotes.xml").unwrap().to_vec()).unwrap();
        let end = xml.find("first</w:t>").unwrap();
        let at = end + xml[end..].find("</w:p>").unwrap() + "</w:p>".len();
        let inserted = format!("{}{}", one_row_table(&["X1", "X2"]), line("then"));
        package.set_part(
            "/word/footnotes.xml",
            format!("{}{inserted}{}", &xml[..at], &xml[at..]).into_bytes(),
        );
    });
    let doc = Document::from_bytes(&edited).unwrap();
    let lines = report_lines(&doc, "--- footnote (");
    let at = |needle: &str| {
        lines
            .iter()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("{needle} is reported: {lines:?}"))
    };
    assert!(
        at("first") < at("X1")
            && at("X1") < at("X2")
            && at("X2") < at("then")
            && at("then") < at("second"),
        "{lines:?}"
    );
}

#[test]
fn a_missing_header_part_leaves_the_other_stories_out_with_a_diagnostic() {
    let mut doc = doc_with(&["Body"]);
    doc.set_header("Head");
    let bytes = doc.to_bytes().unwrap();
    let (_, header) = header_relationship(&bytes);
    let broken = edit_package(&bytes, |package| {
        package.remove_part(&header);
    });
    let doc = Document::from_bytes(&broken).expect("the body still opens");

    let reason = doc.other_story_texts().unwrap_err();
    assert!(reason.contains(&header), "{reason}");
    let report = doc.text_report();
    assert_eq!(report.text, "Body\n");
    assert_eq!(report.scope, "main");
    assert_eq!(report.diagnostics.len(), 1);
    assert!(
        report.diagnostics[0]
            .message
            .starts_with("other stories left out: ")
            && report.diagnostics[0].message.contains(&header),
        "{:?}",
        report.diagnostics
    );
    assert!(
        errors(&doc.validate().unwrap())
            .iter()
            .any(|error| error.contains(&format!("missing part {header}")))
    );
}

#[test]
fn a_truncated_header_is_reported_not_skipped() {
    let mut doc = doc_with(&["Body"]);
    doc.set_header("Head");
    let bytes = doc.to_bytes().unwrap();
    let (_, header) = header_relationship(&bytes);
    let truncated = truncate_header(&bytes, &header);
    let doc = Document::from_bytes(&truncated).expect("the body still opens");
    assert!(
        errors(&doc.validate().unwrap())
            .iter()
            .any(|error| error.starts_with(&format!("part {header} is not well-formed XML")))
    );
    let report = doc.text_report();
    assert!(
        report.scope == "main"
            && report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(&header)),
        "the truncated header is reported: {report:?}"
    );
}

#[test]
fn every_report_text_holds_its_header_footer_and_comment_after_the_body() {
    // From the corpus generator.
    let stories = [
        (
            "--- header (",
            "Northwind Analytics: Quarterly Operations Review",
        ),
        ("--- footer (", "Internal. Prepared by the operations team."),
        ("--- comment (", "Confirm with finance before sending."),
    ];
    let mut word = corpus_doc("word/report.docx");
    let mut docs = vec![("word, saved".to_owned(), reopen(&mut word))];
    docs.extend(REPORTS.map(|name| (name.to_owned(), corpus_doc(name))));
    for (name, doc) in docs {
        let report = doc.text_report();
        assert_eq!(report.scope, "all-supported-stories", "{name}");
        assert!(report.diagnostics.is_empty(), "{name}");
        let body = doc.text();
        assert!(
            report.text.starts_with(&body),
            "{name}: the body comes first"
        );
        let rest = &report.text[body.len()..];
        for (label, text) in stories {
            // The lines between this label and the next one.
            let start = rest
                .find(label)
                .unwrap_or_else(|| panic!("{name}: no {label}"));
            let section = &rest[start..];
            let end = section[1..].find("--- ").map_or(section.len(), |at| at + 1);
            assert!(
                section[..end].contains(&format!("\n{text}\n")),
                "{name}: {text:?} under {label}:\n{rest}"
            );
        }
    }
}

const ONE_LEVEL_NUMBERING: &str = r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#;

/// A document whose numbering part defines numbering instance 1 with level 0
/// only, and whose body holds `body`.
fn doc_with_one_level_numbering(body: &str) -> Vec<u8> {
    doc_with_numbering(body, Some(ONE_LEVEL_NUMBERING), "")
}

/// A document whose body holds `body`, whose numbering part holds
/// `numbering` (none when `None`), and whose styles part also holds `styles`.
fn doc_with_numbering(body: &str, numbering: Option<&str>, styles: &str) -> Vec<u8> {
    let bytes = doc_with_body(body).to_bytes().unwrap();
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    if let Some(numbering) = numbering {
        let numbering = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">{numbering}</w:numbering>"#
        );
        if !package.contains_part("/word/numbering.xml") {
            package.get_or_create_part_rels("/word/document.xml").add(
                oxoffice_opc::relationship::rel_types::NUMBERING,
                "numbering.xml",
            );
            package.content_types.add_override(
                "/word/numbering.xml",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml",
            );
        }
        package.set_part("/word/numbering.xml", numbering.into_bytes());
    } else {
        assert!(!package.contains_part("/word/numbering.xml"));
    }
    let xml = String::from_utf8(package.get_part("/word/styles.xml").unwrap().to_vec()).unwrap();
    let at = xml.rfind("</w:styles>").unwrap();
    package.set_part(
        "/word/styles.xml",
        format!("{}{styles}{}", &xml[..at], &xml[at..]).into_bytes(),
    );
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    out.into_inner()
}

/// The validation warnings of `bytes` that say Word shows a paragraph without
/// a number.
fn numbering_warnings(bytes: &[u8]) -> Vec<String> {
    validate_document_bytes(bytes)
        .unwrap()
        .findings
        .into_iter()
        .filter(|finding| finding.message.contains("Word shows it without a number"))
        .map(|finding| finding.message)
        .collect()
}

fn numbered(text: &str, num_id: u32, level: u32) -> String {
    format!(
        r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="{level}"/><w:numId w:val="{num_id}"/></w:numPr></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#
    )
}

#[test]
fn validation_warns_once_per_paragraph_numbered_by_a_missing_instance_or_level() {
    let body = [
        numbered("missing instance", 42, 0),
        numbered("missing level", 1, 3),
        numbered("fine", 1, 0),
        numbered("missing instance again", 42, 0),
    ]
    .concat();
    let bytes = doc_with_one_level_numbering(&body);
    let messages = |report: ValidationReport| {
        report
            .findings
            .into_iter()
            .filter(|finding| finding.message.contains("numbering"))
            .map(|finding| (finding.severity, finding.message))
            .collect::<Vec<_>>()
    };
    let expected = vec![
        (
            ValidationSeverity::Warning,
            "a paragraph in /word/document.xml is numbered by numbering instance 42, which the \
             document does not define: Word shows it without a number"
                .to_owned(),
        ),
        (
            ValidationSeverity::Warning,
            "a paragraph in /word/document.xml is numbered at level 3 of numbering instance 1, \
             a level its definition does not have: Word shows it without a number"
                .to_owned(),
        ),
        (
            ValidationSeverity::Warning,
            "a paragraph in /word/document.xml is numbered by numbering instance 42, which the \
             document does not define: Word shows it without a number"
                .to_owned(),
        ),
    ];
    assert_eq!(messages(validate_document_bytes(&bytes).unwrap()), expected);
    let doc = Document::from_bytes(&bytes).unwrap();
    let report = doc.validate().unwrap();
    assert!(report.is_valid(), "warnings only: Word opens the file");
    assert_eq!(messages(report), expected);
}

#[test]
fn numbered_contracts_of_every_producer_have_no_dangling_numbering_reference() {
    for producer in ["word", "google", "libreoffice", "authored"] {
        let name = format!("{producer}/contract.docx");
        let report = validate_document_bytes(&corpus_bytes(&name)).unwrap();
        let dangling = report
            .findings
            .iter()
            .filter(|finding| finding.message.contains("Word shows it without a number"))
            .collect::<Vec<_>>();
        assert!(dangling.is_empty(), "{name}: {dangling:?}");
    }
}

#[test]
fn validation_follows_a_list_style_link_and_does_not_warn() {
    // Word writes a list that uses a list style as a definition holding only
    // w:numStyleLink; the levels are those of the definition the style uses.
    let numbering = r#"<w:abstractNum w:abstractNumId="0"><w:styleLink w:val="MyList"/><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl><w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%2."/></w:lvl></w:abstractNum><w:abstractNum w:abstractNumId="1"><w:numStyleLink w:val="MyList"/></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num><w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>"#;
    let styles = r#"<w:style w:type="numbering" w:styleId="MyList"><w:name w:val="My List"/><w:pPr><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr></w:style>"#;
    let body = [
        numbered("first", 2, 0),
        numbered("second", 2, 1),
        numbered("missing", 2, 4),
    ]
    .concat();
    let warnings = numbering_warnings(&doc_with_numbering(&body, Some(numbering), styles));
    assert_eq!(
        warnings,
        [
            "a paragraph in /word/document.xml is numbered at level 4 of numbering instance 2, \
          a level its definition does not have: Word shows it without a number"
        ],
        "only the level the linked definition lacks"
    );
}

#[test]
fn validation_resolves_numbering_through_the_paragraph_style_and_skips_number_zero() {
    let styles = r#"<w:style w:type="paragraph" w:styleId="Dangling"><w:name w:val="Dangling"/><w:pPr><w:numPr><w:numId w:val="42"/></w:numPr></w:pPr></w:style>"#;
    let body = format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Dangling"/></w:pPr><w:r><w:t>styled</w:t></w:r></w:p>{}{}"#,
        numbered("not numbered", 0, 0),
        numbered("fine", 1, 0),
    );
    let warnings = numbering_warnings(&doc_with_numbering(
        &body,
        Some(ONE_LEVEL_NUMBERING),
        styles,
    ));
    assert_eq!(
        warnings,
        [
            "a paragraph in /word/document.xml is numbered by numbering instance 42, which the \
          document does not define: Word shows it without a number"
        ]
    );
}

#[test]
fn validation_warns_without_a_numbering_part_and_for_a_missing_definition() {
    let warnings = numbering_warnings(&doc_with_numbering(&numbered("orphan", 5, 0), None, ""));
    assert_eq!(
        warnings,
        [
            "a paragraph in /word/document.xml is numbered by numbering instance 5, which the \
          document does not define: Word shows it without a number"
        ]
    );
    let numbering =
        format!(r#"{ONE_LEVEL_NUMBERING}<w:num w:numId="3"><w:abstractNumId w:val="9"/></w:num>"#);
    let warnings = numbering_warnings(&doc_with_numbering(
        &numbered("x", 3, 0),
        Some(&numbering),
        "",
    ));
    assert_eq!(
        warnings,
        [
            "a paragraph in /word/document.xml is numbered by numbering instance 3, whose \
          definition 9 the document does not define: Word shows it without a number"
        ]
    );
}

#[test]
fn validation_warns_about_a_dangling_numbering_reference_in_a_header() {
    let bytes = corpus_bytes("word/report.docx");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let header = String::from_utf8(package.get_part("/word/header1.xml").unwrap().to_vec())
        .unwrap()
        .replacen(
            "<w:pPr><w:pStyle w:val=\"En-tte\"/>",
            "<w:pPr><w:pStyle w:val=\"En-tte\"/><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"4242\"/></w:numPr>",
            1,
        );
    assert!(header.contains("4242"), "the header is numbered");
    package.set_part("/word/header1.xml", header.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    assert_eq!(
        numbering_warnings(&out.into_inner()),
        [
            "a paragraph in /word/header1.xml is numbered by numbering instance 4242, which the \
          document does not define: Word shows it without a number"
        ]
    );
}

#[test]
fn a_numbered_paragraph_in_a_text_box_word_writes_twice_warns_once() {
    // Word writes each text box in `mc:Choice` and again in `mc:Fallback`.
    let paragraph = numbered("in the box", 42, 0);
    let body = format!(
        concat!(
            r#"<w:p><w:r><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">"#,
            r#"<mc:Choice xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" Requires="wps">"#,
            r#"<wps:txbx><w:txbxContent>{0}</w:txbxContent></wps:txbx></mc:Choice>"#,
            r#"<mc:Fallback><w:pict><v:shape xmlns:v="urn:schemas-microsoft-com:vml"><v:textbox>"#,
            r#"<w:txbxContent>{0}</w:txbxContent></v:textbox></v:shape></w:pict></mc:Fallback>"#,
            r#"</mc:AlternateContent></w:r></w:p>"#
        ),
        paragraph
    );
    let warnings = numbering_warnings(&doc_with_one_level_numbering(&body));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
}

#[test]
fn a_glossary_paragraph_is_checked_against_the_glossary_numbering() {
    use oxoffice_docx::{BuildingBlock, BuildingBlockKind};
    use oxoffice_docx_oxml::document::{BodyContent, CT_Body};
    use oxoffice_docx_oxml::text::CT_P;
    let mut paragraph = CT_P::new();
    paragraph.add_run("entry");
    let mut body = CT_Body::new();
    body.sect_pr = None;
    body.content
        .push(BodyContent::Paragraph(Box::new(paragraph)));
    let mut doc = doc_with_body("<w:p/>");
    doc.create_building_block(BuildingBlock {
        name: "Entry".to_owned(),
        kind: BuildingBlockKind::AutoText,
        category: Some("General".to_owned()),
        description: None,
        guid: None,
        gallery: Some("autoTxt".to_owned()),
        behaviors: Vec::new(),
        body,
    })
    .unwrap();
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let glossary = "/word/glossary/document.xml";
    let xml = String::from_utf8(package.get_part(glossary).unwrap().to_vec()).unwrap();
    assert!(xml.contains("entry"), "{xml}");
    // Instance 5 is the glossary's own, instance 7 nobody's; the main
    // document defines no numbering at all.
    let start = xml.find("<w:p>").or_else(|| xml.find("<w:p ")).unwrap();
    let end = xml[start..].find("</w:p>").unwrap() + start + "</w:p>".len();
    let paragraphs = format!("{}{}", numbered("own", 5, 0), numbered("dangling", 7, 0));
    package.set_part(
        glossary,
        format!("{}{paragraphs}{}", &xml[..start], &xml[end..]).into_bytes(),
    );
    package.set_part(
        "/word/glossary/numbering.xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum><w:num w:numId="5"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#
        .as_bytes()
            .to_vec(),
    );
    package.get_or_create_part_rels(glossary).add(
        oxoffice_opc::relationship::rel_types::NUMBERING,
        "numbering.xml",
    );
    package.content_types.add_override(
        "/word/glossary/numbering.xml",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml",
    );
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let bytes = out.into_inner();
    // The graph check resolves the glossary the same way: instance 5 is
    // found in the glossary's numbering, instance 7 is the dangling one.
    let error = Document::from_bytes(&bytes)
        .unwrap()
        .validate_numbering_graph()
        .unwrap_err()
        .to_string();
    assert!(error.contains("instance 7"), "{error}");
    assert_eq!(
        numbering_warnings(&bytes),
        [
            "a paragraph in /word/glossary/document.xml is numbered by numbering instance 7, \
             which the document does not define: Word shows it without a number"
        ]
    );
}

#[test]
fn a_full_page_background_picture_is_decorative_and_needs_no_alt_text() {
    let mut doc = doc_with(&["Body"]);
    doc.set_title("Title");
    doc.set_author("Ada");
    doc.add_background_image(PNG, "paper.png");
    let doc = reopen(&mut doc);
    let pictures = doc.pictures().unwrap();
    assert_eq!(pictures.len(), 1);
    assert!(pictures[0].decorative);
    assert_eq!(pictures[0].description, None);
    assert_eq!(doc.audit_accessibility(), []);
}
