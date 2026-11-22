//! Reading a document without changing it: headings and outlines, pictures,
//! links, word and character counts, the accessibility audit, validation, and
//! the stories and text report, on new documents and on the corpus, before
//! and after a save.

use std::fs;
use std::io::Cursor;

use oxoffice_docx::{
    AccessibilityIssue, Document, HdrFtrType, HeaderFooterKind, IssueSeverity, Length, LinkInfo,
    NumberedOutlineNode, OutlineNode, PictureOptions, RunPosition, RunRange, StoryId, StoryKind,
    ValidationFinding, ValidationReport, ValidationSeverity, validate_document_bytes,
    validate_document_file,
};
use oxoffice_opc::OpcPackage;

use crate::support::{corpus_doc, corpus_root, doc_with, doc_with_body, reopen};

/// A one-pixel PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

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

// ---- Pictures ----

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
        // A place in this document, its name unquoted: not a URL.
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        r#"<w:r><w:instrText xml:space="preserve"> HYPERLINK \l Results </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Results</w:t></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
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
    assert_eq!(doc.text(), "Visit our site.\nResults\n");
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

// ---- Accessibility audit ----

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

// ---- Stories and the text report ----

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

#[test]
fn only_the_first_branch_of_an_alternate_content_is_checked_for_numbering() {
    let alternate = |choice: &str, fallback: &str| {
        format!(
            concat!(
                r#"<w:p><w:r><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">"#,
                r#"<mc:Choice Requires="wps">{}</mc:Choice><mc:Fallback>{}</mc:Fallback>"#,
                r#"</mc:AlternateContent></w:r></w:p>"#
            ),
            choice, fallback
        )
    };
    let boxed = |num_id: u32| {
        format!(
            "<w:pict><w:txbxContent>{}</w:txbxContent></w:pict>",
            numbered("boxed", num_id, 0)
        )
    };
    let warnings = numbering_warnings(&doc_with_one_level_numbering(&alternate(
        &boxed(42),
        &boxed(43),
    )));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("instance 42"), "{warnings:?}");
    // An empty Choice is the branch Word shows: the Fallback is not read.
    let body = alternate("", &boxed(43)).replace(
        r#"<mc:Choice Requires="wps"></mc:Choice>"#,
        r#"<mc:Choice Requires="wps"/>"#,
    );
    assert!(numbering_warnings(&doc_with_one_level_numbering(&body)).is_empty());
}
