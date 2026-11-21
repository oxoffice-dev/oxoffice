//! Converting documents to PDF, read back with a real PDF parser: pages and
//! their sizes, where text lands, headers, footers, notes, tables, pictures,
//! fonts, links, outline, tags, PDF/A, metadata, determinism and the corpus.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use lopdf::Object;
use oxoffice_docx::paragraph::Alignment;
use oxoffice_docx::{
    ContentLocation, Document, Field, Length, PdfConformance, PictureOptions, RenderOptions,
    RevisionView, StoryItemKind, StoryKind, StoryRunPosition,
};
use oxoffice_docx_oxml::shared::ST_PageOrientation;
use oxoffice_render_test::fingerprint::Pixels;
use oxoffice_render_test::pdf::{Drawn, Element, Pdf, Picture};

use crate::support::{corpus_doc, corpus_root, doc_with, doc_with_body, reopen, texts};

/// One line of a page: the pieces on one baseline, in stream order, from
/// the left edge of its leftmost glyph to the right edge of its rightmost.
#[derive(Debug)]
struct Line {
    y: f64,
    text: String,
    left: f64,
    right: f64,
}

fn lines(pieces: &[Drawn]) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    for piece in pieces {
        match lines.last_mut() {
            Some(line) if (line.y - piece.y).abs() < 0.5 => {
                line.text.push_str(&piece.text);
                line.left = line.left.min(piece.x);
                line.right = line.right.max(piece.end);
            }
            _ => lines.push(Line {
                y: piece.y,
                text: piece.text.clone(),
                left: piece.x,
                right: piece.end,
            }),
        }
    }
    lines
}

/// The line of a page that holds `text`.
fn line_of(pdf: &Pdf, page: usize, text: &str) -> Line {
    lines(&pdf.drawn(page))
        .into_iter()
        .find(|line| line.text.contains(text))
        .unwrap_or_else(|| panic!("page {page} draws no line with {text:?}"))
}

/// Text without its white space, to compare text across line breaks and tabs.
fn squeeze(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

/// The first page whose text holds `text`, white space aside.
fn page_with(pdf: &Pdf, text: &str) -> usize {
    (0..pdf.page_count())
        .find(|page| squeeze(&pdf.page_text(*page)).contains(&squeeze(text)))
        .unwrap_or_else(|| panic!("no page draws {text:?}"))
}

/// The text of a structure element and of the elements under it, read
/// through their marked content.
fn tagged_text(pdf: &Pdf, elements: &[Element], index: usize) -> String {
    let depth = elements[index].1;
    let below = elements[index + 1..]
        .iter()
        .take_while(|element| element.1 > depth);
    let mut pages: HashMap<usize, Vec<Drawn>> = HashMap::new();
    let mut text = String::new();
    for (page, mcid) in std::iter::once(&elements[index])
        .chain(below)
        .flat_map(|element| element.3.iter())
    {
        let drawn = pages.entry(*page).or_insert_with(|| pdf.drawn(*page));
        for piece in drawn.iter().filter(|piece| piece.mcid == Some(*mcid)) {
            text.push_str(&piece.text);
        }
    }
    text
}

// ---- Builders ----

/// The document converted with the bundled fonts only, whatever fonts the
/// machine has.
fn pdf_of(doc: &Document) -> Pdf {
    Pdf::read(
        &doc.to_pdf_deterministic()
            .expect("document converts to PDF"),
    )
}

/// Whether the glyphs of a piece draw its text: the same characters, in any
/// order, since a right-to-left run is drawn in visual order. An Arabic face
/// draws letter skeletons and dots as glyphs several letters share, and a
/// ToUnicode map gives each glyph one text for the whole file, so the glyphs
/// of an Arabic word only map to Arabic characters; its ActualText spells it.
fn glyphs_draw_text(piece: &Drawn) -> bool {
    let arabic = |ch: char| ('\u{0600}'..='\u{06FF}').contains(&ch);
    if piece.text.chars().any(arabic) {
        return !piece.glyphs.is_empty()
            && piece
                .glyphs
                .chars()
                .all(|ch| arabic(ch) || piece.text.contains(ch));
    }
    let sorted = |text: &str| {
        let mut chars: Vec<char> = text.chars().collect();
        chars.sort_unstable();
        chars
    };
    sorted(&piece.text) == sorted(&piece.glyphs)
}

/// A PNG of `width` by `height` RGB pixels, rows top first.
fn png(width: u32, height: u32, pixels: &[[u8; 3]]) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFF_u32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    0xEDB8_8320 ^ (crc >> 1)
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend(kind);
        out.extend(data);
        let crc = crc32(&out[start..]);
        out.extend(crc.to_be_bytes());
    }
    let mut samples = Vec::new();
    for row in pixels.chunks(width as usize) {
        samples.push(0);
        samples.extend(row.iter().flatten());
    }
    let mut header = Vec::new();
    header.extend(width.to_be_bytes());
    header.extend(height.to_be_bytes());
    header.extend([8, 2, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &header);
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&samples, 6);
    chunk(&mut out, b"IDAT", &compressed);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// A part of a corpus file, by its ZIP entry name.
fn corpus_part(name: &str, part: &str) -> Vec<u8> {
    let bytes = std::fs::read(corpus_root().join("files").join(name)).expect("corpus file");
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("package is a ZIP");
    let mut data = Vec::new();
    zip.by_name(part)
        .expect("part is in the package")
        .read_to_end(&mut data)
        .expect("part inflates");
    data
}

/// The location of the first paragraph of the first story of this kind.
fn first_paragraph_of(doc: &Document, kind: StoryKind) -> ContentLocation {
    let stories = doc.stories().expect("stories are listed");
    let story = stories
        .into_iter()
        .find(|story| story.kind() == kind)
        .expect("the story exists");
    doc.story_items(&story)
        .expect("story reads")
        .iter()
        .find(|item| item.kind() == StoryItemKind::Paragraph)
        .expect("the story has a paragraph")
        .location()
        .clone()
}

const LETTER: (f64, f64) = (612.0, 792.0);
const A4: (f64, f64) = (595.3, 841.9);
const PRODUCERS: [&str; 4] = ["word", "google", "libreoffice", "authored"];

fn assert_size(found: (f64, f64), expected: (f64, f64), what: &str) {
    assert!(
        (found.0 - expected.0).abs() < 0.5 && (found.1 - expected.1).abs() < 0.5,
        "{what}: page is {found:?}, expected {expected:?}"
    );
}

// ---- Pages ----

#[test]
fn a_document_converts_to_letter_pages_even_when_empty() {
    let pdf = pdf_of(&doc_with(&["Hello world"]));
    assert_eq!(pdf.page_count(), 1);
    assert_size(pdf.page_size(0), LETTER, "page 1");
    assert_eq!(pdf.page_text(0), "Hello world");

    // A document with no paragraph still prints one blank page.
    let pdf = pdf_of(&Document::new());
    assert_eq!(pdf.page_count(), 1);
    assert_size(pdf.page_size(0), LETTER, "empty page");
    assert!(pdf.drawn(0).is_empty(), "{:?}", pdf.drawn(0));
}

#[test]
fn long_text_flows_onto_further_pages_in_reading_order() {
    let paragraphs: Vec<String> = (1..=150).map(|n| format!("Paragraph {n:03}")).collect();
    let doc = doc_with(&paragraphs.iter().map(String::as_str).collect::<Vec<_>>());
    let pdf = pdf_of(&doc);
    assert!(pdf.page_count() >= 3, "{} pages", pdf.page_count());
    let mut read = Vec::new();
    for page in 0..pdf.page_count() {
        let lines = lines(&pdf.drawn(page));
        assert!(!lines.is_empty(), "page {page} is blank");
        assert!(
            lines.windows(2).all(|pair| pair[0].y > pair[1].y),
            "page {page}: lines are not drawn top to bottom"
        );
        read.extend(lines.into_iter().map(|line| line.text));
    }
    // Every paragraph once, in order, across the pages.
    assert_eq!(read, paragraphs);
}

#[test]
fn each_section_keeps_its_page_size_and_orientation() {
    let mut doc = doc_with(&["On an A4 page"]);
    doc.section_mut(0)
        .unwrap()
        .set_page_size(Length::twips(11906), Length::twips(16838))
        .unwrap();
    doc.insert_section(1).unwrap();
    doc.add_paragraph("On a landscape page");
    doc.section_mut(1)
        .unwrap()
        .set_orientation(ST_PageOrientation::Landscape);
    let pdf = pdf_of(&reopen(&mut doc));

    assert_eq!(pdf.page_count(), 2);
    assert_size(pdf.page_size(0), A4, "A4 portrait");
    assert_size(pdf.page_size(1), (A4.1, A4.0), "A4 landscape");
    assert_eq!(pdf.page_text(0), "On an A4 page");
    assert_eq!(pdf.page_text(1), "On a landscape page");
    // Each page is laid out at its own height: the first baseline sits the
    // top margin (one inch) and less than a line below its top edge.
    for page in 0..2 {
        let (_, height) = pdf.page_size(page);
        let below_top = height - pdf.drawn(page)[0].y;
        assert!(
            (72.0..86.0).contains(&below_top),
            "page {page}: first baseline {below_top} pt below the top"
        );
    }
}

// Word (Pages = 3 in the Word file) and LibreOffice print each report on 3
// pages, though it ends its first section with a page break.
#[test]
fn no_page_of_a_corpus_report_is_blank() {
    for producer in PRODUCERS {
        let pdf = pdf_of(&corpus_doc(&format!("{producer}/report.docx")));
        for page in 0..pdf.page_count() {
            assert!(
                !pdf.body_text(page, 72.0).trim().is_empty(),
                "page {} of the {producer} report draws only its header and footer",
                page + 1
            );
        }
    }
}

#[test]
fn a_table_of_contents_entry_saved_by_libreoffice_keeps_its_title() {
    // LibreOffice writes the whole entry (number, tab, title, tab, page) as
    // the result of its PAGEREF; Word writes the page number alone.
    let entry = |result: &str| {
        format!(
            r#"<w:p><w:pPr><w:tabs><w:tab w:val="left" w:pos="720"/><w:tab w:val="right" w:pos="9000"/></w:tabs></w:pPr><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve">PAGEREF _Toc1 \h</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r>{result}</w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
        )
    };
    let heading = r#"<w:p><w:bookmarkStart w:id="1" w:name="_Toc1"/><w:r><w:t>Motivation</w:t></w:r><w:bookmarkEnd w:id="1"/></w:p>"#;
    let libreoffice = entry("<w:t>1.1</w:t><w:tab/><w:t>Motivation</w:t><w:tab/><w:t>7</w:t>");
    let word = entry("<w:t>7</w:t>");
    // A level whose entry has no tab stop separates its parts with spaces.
    let spaced = entry("<w:t xml:space=\"preserve\">1.2 Research questions 9</w:t>");
    // The entry inside another field's result, closed in the same paragraph.
    let nested = format!(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> QUOTE "x" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{}<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        entry("<w:t>1.3</w:t><w:tab/><w:t>Scope</w:t><w:tab/><w:t>8</w:t>")
            .split_once("</w:pPr>")
            .expect("an entry has paragraph properties")
            .1
            .trim_end_matches("</w:p>")
    );
    let pdf = pdf_of(&doc_with_body(&format!(
        "{libreoffice}{word}{spaced}{nested}{heading}"
    )));
    let lines: Vec<String> = lines(&pdf.drawn(0))
        .into_iter()
        .map(|line| squeeze(&line.text))
        .collect();
    assert_eq!(
        lines[..4],
        ["1.1Motivation7", "1", "1.2Researchquestions9", "1.3Scope8"],
        "the LibreOffice entries keep their stored text, the Word one gets the page"
    );
}

#[test]
fn a_page_break_just_before_a_next_page_section_break_adds_no_blank_page() {
    let section = r#"<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr>"#;
    let page_break = r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#;
    let doc = doc_with_body(&format!(
        r#"<w:p><w:r><w:t>First section</w:t></w:r></w:p>{page_break}<w:p><w:pPr>{section}</w:pPr></w:p><w:p><w:r><w:t>Second section</w:t></w:r></w:p>"#
    ));
    let pdf = pdf_of(&doc);
    assert_eq!(pdf.page_count(), 2, "the section break starts the page");
    assert_eq!(pdf.page_text(0), "First section");
    assert_eq!(pdf.page_text(1), "Second section");

    // Before a continuous section, which shares the page, the break keeps
    // its page.
    let continuous = section.replace("<w:pgSz", r#"<w:type w:val="continuous"/><w:pgSz"#);
    let doc = doc_with_body(&format!(
        r#"<w:p><w:r><w:t>First page</w:t></w:r></w:p>{page_break}<w:p><w:pPr>{section}</w:pPr></w:p><w:p><w:r><w:t>Second page</w:t></w:r></w:p><w:p><w:pPr>{continuous}</w:pPr></w:p><w:p><w:r><w:t>Third page</w:t></w:r></w:p>"#
    ));
    let pdf = pdf_of(&doc);
    assert_eq!(pdf.page_count(), 3);
    assert_eq!(pdf.page_text(0), "First page");
    assert_eq!(pdf.page_text(1), "Second page");
    assert_eq!(pdf.page_text(2), "Third page");

    // A section printing its endnotes at its end, with none to print, drops
    // the break too.
    let endnotes_at_end = section.replace(
        "<w:pgSz",
        r#"<w:endnotePr><w:pos w:val="sectEnd"/></w:endnotePr><w:pgSz"#,
    );
    let doc = doc_with_body(&format!(
        r#"<w:p><w:r><w:t>First section</w:t></w:r></w:p>{page_break}<w:p><w:pPr>{endnotes_at_end}</w:pPr></w:p><w:p><w:r><w:t>Second section</w:t></w:r></w:p>"#
    ));
    let pdf = pdf_of(&doc);
    assert_eq!(pdf.page_count(), 2);
    assert_eq!(pdf.page_text(1), "Second section");

    // Text after the page break, in the same section, still goes to the next
    // page, then the section break starts a third one.
    let doc = doc_with_body(&format!(
        r#"<w:p><w:r><w:t>First</w:t></w:r></w:p>{page_break}<w:p><w:r><w:t>After the break</w:t></w:r></w:p><w:p><w:pPr>{section}</w:pPr></w:p><w:p><w:r><w:t>Next section</w:t></w:r></w:p>"#
    ));
    let pdf = pdf_of(&doc);
    assert_eq!(pdf.page_count(), 3);
    assert_eq!(pdf.page_text(1), "After the break");
    assert_eq!(pdf.page_text(2), "Next section");
}

#[test]
fn a_page_break_before_a_page_break_before_heading_adds_no_blank_page() {
    // A paragraph ending in a page break, then a heading with page break
    // before: one new page, in any script. Hebrew and Japanese text is laid
    // out by the multilingual path, whose empty end of paragraph is a
    // segment of its own.
    for text in ["Body", "שלום", "日本語"] {
        let doc = doc_with_body(&format!(
            r#"<w:p><w:r><w:t>{text}</w:t><w:br w:type="page"/></w:r></w:p><w:p><w:pPr><w:pageBreakBefore/></w:pPr><w:r><w:t>Heading</w:t></w:r></w:p>"#
        ));
        let pdf = pdf_of(&doc);
        assert_eq!(pdf.page_count(), 2, "{text}");
        assert_eq!(pdf.page_text(1), "Heading", "{text}");
    }
}

#[test]
fn an_odd_or_even_page_section_starts_on_a_page_of_its_parity() {
    let section = r#"<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/><w:pgBorders w:display="firstPage"><w:top w:val="single" w:sz="24" w:space="24" w:color="FF0000"/></w:pgBorders></w:sectPr>"#;
    let page_break = r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#;
    let text = |text: &str| format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>");
    // The section after the break is the last one: its type is in the body's
    // own section properties, which `doc_with_body` keeps as Word's default,
    // so the type goes on a section break of its own after it.
    let pages = |first: &str, kind: &str, numbering: &str| {
        let next = section.replace("<w:pgSz", &format!(r#"<w:type w:val="{kind}"/><w:pgSz"#));
        let next = next.replace("</w:sectPr>", &format!("{numbering}</w:sectPr>"));
        let doc = doc_with_body(&format!(
            r#"{first}<w:p><w:pPr>{section}</w:pPr></w:p>{}<w:p><w:pPr>{next}</w:pPr></w:p>{}"#,
            text("Next"),
            text("Last"),
        ));
        let mut doc = doc;
        doc.set_footer("Footer");
        let pdf = pdf_of(&reopen(&mut doc));
        (0..pdf.page_count())
            .map(|page| pdf.page_text(page).replace("Footer", "").trim().to_owned())
            .collect::<Vec<_>>()
    };
    let one = text("Body");
    let two = format!("{}{page_break}{}", text("Body"), text("Two"));
    // Page 2 is even: an odd-page section skips it, an even-page one takes it.
    assert_eq!(pages(&one, "oddPage", ""), ["Body", "", "Next", "Last"]);
    assert_eq!(pages(&one, "evenPage", ""), ["Body", "Next", "Last"]);
    // Page 3 is odd.
    assert_eq!(pages(&two, "oddPage", ""), ["Body", "Two", "Next", "Last"]);
    assert_eq!(
        pages(&two, "evenPage", ""),
        ["Body", "Two", "", "Next", "Last"]
    );
    // The physical page decides, whatever number the section asks for, as
    // LibreOffice prints it (Word was not run): an odd section restarting at
    // 1 or at 2 still skips page 2, an even one restarting at 1 takes it.
    assert_eq!(
        pages(&one, "oddPage", r#"<w:pgNumType w:start="1"/>"#),
        ["Body", "", "Next", "Last"]
    );
    assert_eq!(
        pages(&one, "oddPage", r#"<w:pgNumType w:start="2"/>"#),
        ["Body", "", "Next", "Last"]
    );
    assert_eq!(
        pages(&one, "evenPage", r#"<w:pgNumType w:start="1"/>"#),
        ["Body", "Next", "Last"]
    );
}

#[test]
fn the_blank_page_before_an_odd_page_section_draws_nothing() {
    // The first section has a footer on every page and a red border on its
    // first page; the blank page that sends the next section to page 3
    // shows neither.
    let section = r#"<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/><w:pgBorders w:display="firstPage"><w:top w:val="single" w:sz="24" w:space="24" w:color="FF0000"/><w:bottom w:val="single" w:sz="24" w:space="24" w:color="FF0000"/></w:pgBorders></w:sectPr>"#;
    let odd = section.replace("<w:pgSz", r#"<w:type w:val="oddPage"/><w:pgSz"#);
    let mut doc = doc_with_body(&format!(
        r#"<w:p><w:r><w:t>Body</w:t></w:r></w:p><w:p><w:pPr>{section}</w:pPr></w:p><w:p><w:r><w:t>Next</w:t></w:r></w:p><w:p><w:pPr>{odd}</w:pPr></w:p><w:p><w:r><w:t>Last</w:t></w:r></w:p>"#
    ));
    doc.set_footer("Footer");
    let doc = reopen(&mut doc);
    let pdf = pdf_of(&doc);
    assert_eq!(pdf.page_count(), 4);
    assert_eq!(pdf.page_text(1), "");
    let red = |page: usize| {
        let image = Pixels::decode(&doc.render_page_to_png(page, 72.0).unwrap().expect("page"));
        image
            .rgba
            .chunks(4)
            .filter(|pixel| pixel[..3] == [0xFF, 0, 0])
            .count()
    };
    assert!(red(0) > 0, "the first page has its border");
    assert_eq!(red(1), 0, "the blank page has none");
}

#[test]
fn the_blank_page_before_an_odd_page_section_has_its_size_and_no_page_colour() {
    let section = r#"<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr>"#;
    let landscape = r#"<w:sectPr><w:type w:val="oddPage"/><w:pgSz w:w="15840" w:h="12240" w:orient="landscape"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr>"#;
    let mut doc = doc_with_body(&format!(
        r#"<w:p><w:r><w:t>Body</w:t></w:r></w:p><w:p><w:pPr>{section}</w:pPr></w:p><w:p><w:r><w:t>Next</w:t></w:r></w:p><w:p><w:pPr>{landscape}</w:pPr></w:p><w:p><w:r><w:t>Last</w:t></w:r></w:p>"#
    ));
    // A green page colour on every page.
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let xml = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    let xml = xml.replacen("<w:body>", r#"<w:background w:color="00FF00"/><w:body>"#, 1);
    package.set_part("/word/document.xml", xml.into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let doc = Document::from_bytes(&bytes.into_inner()).unwrap();
    let pdf = pdf_of(&doc);
    assert_eq!(pdf.page_count(), 4);
    // The blank page takes the size of the section it comes before.
    assert_eq!(pdf.page_size(1), (792.0, 612.0));
    let green = |page: usize| {
        Pixels::decode(&doc.render_page_to_png(page, 36.0).unwrap().expect("page"))
            .rgba
            .chunks(4)
            .filter(|pixel| pixel[..3] == [0, 0xFF, 0])
            .count()
    };
    assert!(
        green(0) > 0 && green(2) > 0,
        "the pages around it are green"
    );
    assert_eq!(green(1), 0, "the blank page is not");
}

#[test]
fn a_page_break_at_the_foot_of_a_full_page_before_a_section_break_stays_there() {
    // Lines of exactly 12 pt with no spacing: 53 of them leave room for one
    // more on a Letter page with one-inch margins.
    let exact =
        r#"<w:pPr><w:spacing w:before="0" w:after="0" w:line="240" w:lineRule="exact"/></w:pPr>"#;
    let filler: String = (1..=53)
        .map(|line| format!("<w:p>{exact}<w:r><w:t>Line {line}</w:t></w:r></w:p>"))
        .collect();
    let section = r#"<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr>"#;
    let doc = doc_with_body(&format!(
        r#"{filler}<w:p>{exact}<w:r><w:t>Last line</w:t><w:br w:type="page"/></w:r></w:p><w:p><w:pPr>{section}</w:pPr></w:p><w:p><w:r><w:t>Next section</w:t></w:r></w:p>"#
    ));
    let pdf = pdf_of(&doc);
    assert_eq!(pdf.page_count(), 2);
    assert!(
        pdf.page_text(0).ends_with("Last line"),
        "{}",
        pdf.page_text(0)
    );
    assert_eq!(pdf.page_text(1), "Next section");

    // The space after the last line, Normal's in a Word document, would end
    // below the foot: it goes with the break.
    let spaced = exact.replace(r#"w:after="0""#, r#"w:after="160""#);
    let doc = doc_with_body(&format!(
        r#"{filler}<w:p>{spaced}<w:r><w:t>Last line</w:t><w:br w:type="page"/></w:r></w:p><w:p><w:pPr>{section}</w:pPr></w:p><w:p><w:r><w:t>Next section</w:t></w:r></w:p>"#
    ));
    let pdf = pdf_of(&doc);
    assert_eq!(pdf.page_count(), 2);
    assert_eq!(pdf.page_text(1), "Next section");

    // With line numbers, what follows the break gets no number: the last
    // number is the last line's, drawn once.
    let numbered = section.replace("</w:sectPr>", r#"<w:lnNumType w:countBy="1"/></w:sectPr>"#);
    let doc = doc_with_body(&format!(
        r#"{filler}<w:p>{exact}<w:r><w:t>Last line</w:t><w:br w:type="page"/></w:r></w:p><w:p><w:pPr>{numbered}</w:pPr></w:p><w:p><w:r><w:t>Next section</w:t></w:r></w:p>"#
    ));
    let pdf = pdf_of(&doc);
    assert_eq!(pdf.page_count(), 2);
    let numbers: Vec<String> = lines(&pdf.drawn(0))
        .into_iter()
        .filter(|line| line.text.chars().all(|c| c.is_ascii_digit()))
        .map(|line| line.text)
        .collect();
    assert_eq!(
        numbers.last().map(String::as_str),
        Some("54"),
        "{numbers:?}"
    );
    assert_eq!(numbers.len(), 54, "{numbers:?}");
}

// ---- Where text lands ----

#[test]
fn text_lands_inside_the_margins_and_aligns_on_them() {
    let long = "Margins decide where every line of a paragraph starts and ends. ".repeat(5);
    let mut doc = doc_with(&["First line", long.trim_end()]);
    doc.add_paragraph("Right aligned")
        .alignment(Alignment::Right);
    doc.add_paragraph("Centred").alignment(Alignment::Center);
    // Top 1.5 in, right 3 in, bottom 1 in, left 2 in.
    doc.set_margins(
        Length::inches(1.5),
        Length::inches(3.0),
        Length::inches(1.0),
        Length::inches(2.0),
    );
    let pdf = pdf_of(&reopen(&mut doc));
    let (left, right) = (144.0, 612.0 - 216.0);
    let lines = lines(&pdf.drawn(0));

    assert_eq!(lines[0].text, "First line");
    assert!(
        (lines[0].left - left).abs() < 0.01,
        "starts at {}",
        lines[0].left
    );
    let below_top = 792.0 - 108.0 - lines[0].y;
    assert!(
        (0.0..14.0).contains(&below_top),
        "{below_top} pt below the top margin"
    );

    // The long paragraph wraps inside both margins and uses the width.
    let wrapped = &lines[1..lines.len() - 2];
    assert!(wrapped.len() >= 3, "{wrapped:?}");
    for line in wrapped {
        assert!((line.left - left).abs() < 0.01, "{line:?}");
        assert!(line.right <= right + 0.5, "{line:?}");
    }
    assert!(wrapped[0].right > right - 40.0, "{:?}", wrapped[0]);
    assert_eq!(
        squeeze(
            &wrapped
                .iter()
                .map(|line| line.text.as_str())
                .collect::<String>()
        ),
        squeeze(&long)
    );

    let aligned = &lines[lines.len() - 2];
    assert_eq!(aligned.text, "Right aligned");
    assert!((aligned.right - right).abs() < 0.5, "{aligned:?}");
    let centred = &lines[lines.len() - 1];
    assert_eq!(centred.text, "Centred");
    let middle = (centred.left + centred.right) / 2.0;
    assert!((middle - (left + right) / 2.0).abs() < 0.5, "{centred:?}");
}

// Word 16.115 reads left and right as start and end in a `w:bidi`
// paragraph: `jc="left"` and `jc="start"` end it at the right margin,
// `jc="right"` and `jc="end"` start it at the left one, `w:ind w:left` and
// `w:ind w:start` both indent it from the right, and a justified paragraph
// ends its last line on the right. Without `w:bidi`, Arabic text keeps the
// plain sides. The first-line indent is on the start side, the right.
#[test]
fn a_right_to_left_paragraph_reads_left_and_right_as_start_and_end() {
    let arabic = "مرحبا بالعالم";
    let long =
        "هذا نص عربي طويل يملأ أكثر من سطر واحد في الفقرة حتى نرى أين يبدأ كل سطر وأين ينتهي "
            .repeat(3);
    let cases = [
        ("left", "<w:bidi/><w:jc w:val=\"left\"/>", arabic),
        ("start", "<w:bidi/><w:jc w:val=\"start\"/>", arabic),
        ("right", "<w:bidi/><w:jc w:val=\"right\"/>", arabic),
        ("end", "<w:bidi/><w:jc w:val=\"end\"/>", arabic),
        ("plain", "", arabic),
        ("plain right", "<w:jc w:val=\"right\"/>", arabic),
        ("ind start", "<w:bidi/><w:ind w:start=\"1440\"/>", arabic),
        ("ind left", "<w:bidi/><w:ind w:left=\"1440\"/>", arabic),
        (
            "justified",
            "<w:bidi/><w:jc w:val=\"both\"/>",
            long.trim_end(),
        ),
        (
            "first line",
            "<w:bidi/><w:ind w:firstLine=\"720\"/>",
            long.trim_end(),
        ),
    ];
    // A list number in a bidi paragraph: Word's default list indent, the
    // number at 18 pt from the right margin, its text at 36 pt, as the
    // following lines.
    let numbered = format!(
        r#"<w:p><w:pPr><w:bidi/><w:ind w:left="720" w:hanging="360"/></w:pPr><w:r><w:rPr><w:rtl/></w:rPr><w:t>1.</w:t></w:r><w:r><w:rPr><w:rtl/></w:rPr><w:tab/><w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,
        long.trim_end()
    );
    let mut body = String::new();
    for (_, ppr, text) in cases {
        body.push_str(&format!(
            r#"<w:p><w:pPr>{ppr}</w:pPr><w:r><w:rPr><w:rtl/></w:rPr><w:t xml:space="preserve">{text}</w:t></w:r></w:p>"#
        ));
    }
    body.push_str(&numbered);
    let pdf = pdf_of(&doc_with_body(&body));
    let lines = lines(&pdf.drawn(0));
    let (left, right) = (72.0, 540.0);
    let at = |index: usize| &lines[index];
    let near = |value: f64, expected: f64| (value - expected).abs() < 1.0;
    for (index, (name, ends_right)) in [
        ("left", true),
        ("start", true),
        ("right", false),
        ("end", false),
        ("plain", false),
        ("plain right", true),
    ]
    .into_iter()
    .enumerate()
    {
        let line = at(index);
        let edge = if ends_right {
            near(line.right, right)
        } else {
            near(line.left, left)
        };
        assert!(edge, "{name}: {line:?}");
    }
    for index in [6, 7] {
        assert!(
            near(at(index).right, right - 72.0),
            "{:?}: {:?}",
            cases[index].0,
            at(index)
        );
    }
    // The justified paragraph fills its lines, and its last line, wherever
    // it is in `lines`, ends on the right without reaching the left margin.
    let justified: Vec<&Line> = lines[8..]
        .iter()
        .take_while(|line| near(line.right, right))
        .collect();
    assert!(justified.len() >= 2, "{lines:#?}");
    // Its first line's trailing space hangs past the left margin.
    assert!(justified[0].left < left + 3.0, "{:?}", justified[0]);
    let last = justified.last().unwrap();
    assert!(last.left > left + 40.0, "the last justified line: {last:?}");
    // The first-line indent leaves 36 pt on the right of the first line.
    let first = &lines[8 + justified.len()];
    assert!(near(first.right, right - 36.0), "first line: {first:?}");
    assert!(near(lines[9 + justified.len()].right, right), "second line");
    // The numbered paragraph: the number at the hanging indent, its text
    // and the next line at the left (start) indent.
    let page = pdf.page_count() - 1;
    let drawn = pdf.drawn(page);
    let lines = self::lines(&drawn);
    // The number "1." may be drawn as two pieces, the digit and the stop.
    let number = drawn
        .iter()
        .rfind(|piece| piece.text == "1")
        .unwrap_or_else(|| panic!("the list number: {drawn:#?}"));
    let on_its_line = |piece: &&Drawn| (piece.y - number.y).abs() < 0.5;
    let is_number = |piece: &&Drawn| matches!(piece.text.trim(), "1" | "." | "1.");
    let number_end = drawn
        .iter()
        .filter(on_its_line)
        .filter(is_number)
        .map(|piece| piece.end)
        .fold(f64::MIN, f64::max);
    assert!(
        near(number_end, right - 18.0),
        "the number ends at {number_end:.1}"
    );
    let first_text = drawn
        .iter()
        .filter(on_its_line)
        .filter(|piece| !is_number(piece) && !piece.text.trim().is_empty())
        .map(|piece| piece.end)
        .fold(f64::MIN, f64::max);
    assert!(
        near(first_text, right - 36.0),
        "text after the number ends at {first_text:.1}"
    );
    let next = lines
        .iter()
        .find(|line| line.y < number.y - 0.5)
        .expect("a second numbered line");
    assert!(near(next.right, right - 36.0), "{next:?}");
}

// Word and LibreOffice align a right-to-left paragraph on the right margin,
// with no alignment set and with the `jc="left"` (start) LibreOffice writes.
/// A paragraph holding one display equation, alone, under `ppr`.
fn display_equation(ppr: &str, equation: &str) -> String {
    format!(
        r#"<w:p>{ppr}<m:oMathPara xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math"><m:oMath>{equation}</m:oMath></m:oMathPara></w:p>"#
    )
}

/// `S = Σ_{i=1}^{N} x_i`, a sum with limits over and under it.
const SUM: &str = r#"<m:r><m:t>S=</m:t></m:r><m:nary><m:naryPr><m:chr m:val="∑"/><m:limLoc m:val="undOvr"/></m:naryPr><m:sub><m:r><m:t>i=1</m:t></m:r></m:sub><m:sup><m:r><m:t>N</m:t></m:r></m:sup><m:e><m:sSub><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sub><m:r><m:t>i</m:t></m:r></m:sub></m:sSub></m:e></m:nary>"#;

/// Single line spacing and no space between paragraphs, so that baselines
/// step by line heights alone.
const TIGHT: &str =
    r#"<w:pPr><w:spacing w:before="0" w:after="0" w:line="240" w:lineRule="auto"/></w:pPr>"#;

fn tight(text: &str) -> String {
    format!(r#"<w:p>{TIGHT}<w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p>"#)
}

/// The distance between the baselines of the lines holding `from` and `to`
/// on the first page.
fn baseline_step(pdf: &Pdf, from: &str, to: &str) -> f64 {
    line_of(pdf, 0, from).y - line_of(pdf, 0, to).y
}

/// The same body under math settings with this `m:mathPr` content.
fn with_math_settings(body: &str, math_properties: &str) -> Document {
    let bytes = doc_with_body(body).to_bytes().expect("document saves");
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let part = "/word/settings.xml";
    let xml = String::from_utf8(package.get_part(part).expect("settings part").to_vec())
        .expect("settings are UTF-8");
    let end = xml.rfind("</w:settings>").expect("settings end");
    let xml = format!(
        r#"{}<m:mathPr xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math">{math_properties}</m:mathPr>{}"#,
        &xml[..end],
        &xml[end..]
    );
    package.set_part(part, xml.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    Document::from_bytes(&out.into_inner()).expect("document opens")
}

// Word 16.115 gives a display equation in a paragraph of its own one line,
// as tall as the equation: "C=x" between two Cambria 11 paragraphs steps
// 13.0 and 12.75 pt, where two text lines step 12.75.
#[test]
fn a_display_equation_alone_in_its_paragraph_takes_one_line() {
    let body = [
        tight("Before"),
        display_equation(TIGHT, "<m:r><m:t>C=x</m:t></m:r>"),
        tight("After"),
        tight("Plain one"),
        tight("Plain two"),
        // A bookmark a PAGEREF targets leaves a marker after the equation,
        // or before it (the empty run beside it makes no item).
        tight("Marked before"),
        format!(
            r#"<w:p>{TIGHT}<m:oMathPara xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math"><m:oMath><m:r><m:t>C=x</m:t></m:r></m:oMath></m:oMathPara><w:bookmarkStart w:id="7" w:name="equation"/><w:bookmarkEnd w:id="7"/></w:p>"#
        ),
        tight("Marked after"),
        tight("Led before"),
        format!(
            r#"<w:p>{TIGHT}<w:bookmarkStart w:id="8" w:name="led"/><w:r><w:t></w:t></w:r><w:bookmarkEnd w:id="8"/><m:oMathPara xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math"><m:oMath><m:r><m:t>C=x</m:t></m:r></m:oMath></m:oMathPara></w:p>"#
        ),
        tight("Led after"),
        format!(
            r#"<w:p>{TIGHT}<w:r><w:t xml:space="preserve">See page </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGEREF equation \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>9</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve"> and </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGEREF led \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>9</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
        ),
    ]
    .concat();
    let pdf = pdf_of(&doc_with_body(&body));
    let line = baseline_step(&pdf, "Plain one", "Plain two");
    let step = baseline_step(&pdf, "Before", "After");
    assert!(
        (step - 2.0 * line).abs() < 1.0,
        "the equation paragraph takes {:.2} pt, a text line {line:.2}",
        step - line
    );
    let marked = baseline_step(&pdf, "Marked before", "Marked after");
    assert!(
        (marked - step).abs() < 0.01,
        "with a marker after it, the equation paragraph takes {:.2} pt, {:.2} without",
        marked - line,
        step - line
    );
    let led = baseline_step(&pdf, "Led before", "Led after");
    assert!(
        (led - step).abs() < 0.01,
        "with a marker before it, the equation paragraph takes {:.2} pt, {:.2} without",
        led - line,
        step - line
    );
    assert!(
        pdf.page_text(0).contains("See page 1 and 1"),
        "{}",
        pdf.page_text(0)
    );
}

// Text after a display equation in the same paragraph goes on a line of its
// own, below the equation, and so does text before it.
#[test]
fn text_around_a_display_equation_in_its_paragraph_takes_lines_of_its_own() {
    let body = [
        tight("Before"),
        format!(
            r#"<w:p>{TIGHT}<w:r><w:t>Lead</w:t></w:r><m:oMathPara xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math"><m:oMath><m:r><m:t>C=x</m:t></m:r></m:oMath></m:oMathPara><w:r><w:t>Tail</w:t></w:r></w:p>"#
        ),
        tight("After"),
        tight("Plain one"),
        tight("Plain two"),
    ]
    .concat();
    let pdf = pdf_of(&doc_with_body(&body));
    let line = baseline_step(&pdf, "Plain one", "Plain two");
    for (from, to, lines) in [
        ("Before", "Lead", 1.0),
        ("Lead", "Tail", 2.0),
        ("Tail", "After", 1.0),
    ] {
        let step = baseline_step(&pdf, from, to);
        assert!(
            (step - lines * line).abs() < 1.0,
            "{from} to {to}: {step:.2} pt, a text line {line:.2}"
        );
    }
}

// Word 16.115 scales the text of an equation line by proportional spacing,
// not the equation: at 1.15 lines, "Before" to "After" around a display sum
// grows by 7.75 and 8.22 pt (6 pt after, plus 15 % of a Cambria 11 line), as
// it does around a one-line "C=x" (8.0 and 8.0), and around a line holding
// an inline fraction by 7.74 and 7.98.
#[test]
fn proportional_spacing_scales_the_text_of_an_equation_line_not_the_equation() {
    let spaced =
        r#"<w:pPr><w:spacing w:before="0" w:after="0" w:line="276" w:lineRule="auto"/></w:pPr>"#;
    let inline = |ppr: &str| {
        format!(
            r#"<w:p>{ppr}<w:r><w:t xml:space="preserve">Ratio </w:t></w:r><m:oMath xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math"><m:f><m:num><m:r><m:t>a</m:t></m:r></m:num><m:den><m:r><m:t>b</m:t></m:r></m:den></m:f></m:oMath><w:r><w:t xml:space="preserve"> inline</w:t></w:r></w:p>"#
        )
    };
    let body = [
        tight("Single before"),
        display_equation(TIGHT, SUM),
        tight("Single after"),
        tight("Spaced before"),
        display_equation(spaced, SUM),
        tight("Spaced after"),
        tight("Inline single before"),
        inline(TIGHT),
        tight("Inline single after"),
        tight("Inline spaced before"),
        inline(spaced),
        tight("Inline spaced after"),
        tight("Plain one"),
        tight("Plain two"),
    ]
    .concat();
    let pdf = pdf_of(&doc_with_body(&body));
    let line = baseline_step(&pdf, "Plain one", "Plain two");
    for (shape, single, spaced) in [
        (
            "display sum",
            ("Single before", "Single after"),
            ("Spaced before", "Spaced after"),
        ),
        (
            "inline fraction",
            ("Inline single before", "Inline single after"),
            ("Inline spaced before", "Inline spaced after"),
        ),
    ] {
        let single = baseline_step(&pdf, single.0, single.1);
        let spaced = baseline_step(&pdf, spaced.0, spaced.1);
        assert!(
            single > 2.0 * line + 8.0,
            "the {shape} is taller than a line: {single:.2} pt"
        );
        assert!(
            (spaced - single - 0.15 * line).abs() < 0.4,
            "1.15 lines add {:.2} pt to the {shape}'s line, 15 % of a text line is {:.2}",
            spaced - single,
            0.15 * line
        );
    }
}

// Word 16.115 adds no space for m:preSp and m:postSp around a display
// equation alone in its paragraph, with m:dispDef on or off.
#[test]
fn math_pre_and_post_spacing_add_nothing_around_a_display_equation() {
    let body = [
        tight("Before"),
        display_equation(TIGHT, SUM),
        tight("After"),
    ]
    .concat();
    let plain = baseline_step(&pdf_of(&doc_with_body(&body)), "Before", "After");
    for display_defaults in [r#"<m:dispDef/>"#, r#"<m:dispDef m:val="0"/>"#] {
        let doc = with_math_settings(
            &body,
            &format!(r#"{display_defaults}<m:preSp m:val="240"/><m:postSp m:val="120"/>"#),
        );
        assert_eq!(
            doc.math_properties()
                .map(|properties| (properties.pre_spacing, properties.post_spacing)),
            Some((Some(240), Some(120))),
            "the spacing is read"
        );
        let spaced = baseline_step(&pdf_of(&doc), "Before", "After");
        assert!(
            (spaced - plain).abs() < 0.01,
            "{display_defaults}: {spaced:.2} pt with the spacing, {plain:.2} without"
        );
    }
}

// Fails today: a right-to-left paragraph is drawn against the left margin;
// Word and LibreOffice align it on the right margin, with no alignment set
// and with the `jc="left"` (start) LibreOffice writes. Issue: #157
#[test]
fn a_right_to_left_paragraph_ends_at_the_right_margin() {
    let mut doc = Document::new();
    doc.add_paragraph("مرحبا بالعالم").right_to_left(true);
    doc.add_paragraph("שלום עולם").right_to_left(true);
    let pdf = pdf_of(&reopen(&mut doc));
    let lines = lines(&pdf.drawn(0));
    assert_eq!(lines.len(), 2);
    for line in lines {
        assert!(
            (line.right - 540.0).abs() < 1.0,
            "right-to-left line ends at {:.1}, not at the right margin (540): {:?}",
            line.right,
            line.text
        );
    }

    // LibreOffice saves its right-aligned Arabic heading as bidi with
    // jc="left", which a bidi paragraph reads as its start, the right. A fix
    // of the default alone stops at this assertion, with its own message.
    let doc = corpus_doc("libreoffice/minutes.docx");
    let (_, right, _, _) = doc.section(0).unwrap().margins().unwrap();
    let pdf = pdf_of(&doc);
    let page = page_with(&pdf, "ملخص الاجتماع");
    let margin = pdf.page_size(page).0 - right.to_pt();
    let heading = line_of(&pdf, page, "ملخص الاجتماع");
    assert!(
        (heading.right - margin).abs() < 1.0,
        "the jc=left Arabic heading of the LibreOffice minutes ends at {:.1}, not at its right margin ({margin:.1})",
        heading.right
    );
}

// ---- Headers, footers and notes ----

#[test]
fn headers_and_footers_with_page_numbers_are_drawn_on_every_page() {
    let mut doc = doc_with(&["One", "Two", "Three"]);
    for index in [1, 2] {
        doc.paragraph_mut(index)
            .unwrap()
            .set_page_break_before(true);
    }
    doc.set_header("Running header");
    doc.set_footer("Page ");
    let footer = first_paragraph_of(&doc, StoryKind::Footer);
    let position = StoryRunPosition {
        location: footer,
        run_index: 1,
    };
    doc.insert_field(&position, Field::new("PAGE", "9"))
        .unwrap();
    let pdf = pdf_of(&reopen(&mut doc));

    assert_eq!(pdf.page_count(), 3);
    for (page, body) in ["One", "Two", "Three"].into_iter().enumerate() {
        let lines = lines(&pdf.drawn(page));
        let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
        // Not the stored result 9: each page shows its own number.
        let number = format!("Page {}", page + 1);
        assert_eq!(texts, ["Running header", body, &number], "page {page}");
        // The header sits in the top margin, the footer in the bottom one.
        assert!(lines[0].y > 792.0 - 72.0, "{:?}", lines[0]);
        assert!(lines[2].y < 72.0, "{:?}", lines[2]);
    }
}

// A page field whose page value is wider or narrower than its stored result
// keeps a right-aligned header on the margin and a centred one centred: the
// line moves by the change, as Word draws it (the Word thesis' running
// header, right-aligned STYLEREF fields, ends at its margin on every page).
#[test]
fn a_right_aligned_or_centred_header_stays_aligned_with_its_page_value() {
    for (jc, edge) in [("right", 540.0), ("center", 306.0)] {
        let paragraphs: Vec<String> = (1..=12).map(|page| format!("Body {page}")).collect();
        let paragraphs: Vec<&str> = paragraphs.iter().map(String::as_str).collect();
        let mut doc = doc_with(&paragraphs);
        for index in 1..12 {
            doc.paragraph_mut(index)
                .unwrap()
                .set_page_break_before(true);
        }
        doc.set_header("Page ");
        let header = first_paragraph_of(&doc, StoryKind::Header);
        let position = StoryRunPosition {
            location: header,
            run_index: 1,
        };
        doc.insert_field(&position, Field::new("PAGE", "5"))
            .unwrap();
        // The facade sets no alignment on a header paragraph: write it.
        let bytes = doc.to_bytes().unwrap();
        let mut package = oxoffice_opc::OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
        let headers: Vec<String> = (1..=3)
            .map(|index| format!("/word/header{index}.xml"))
            .filter(|name| package.contains_part(name))
            .collect();
        assert!(!headers.is_empty());
        for part in headers {
            let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
            // The paragraph tag may carry namespace declarations.
            let tag = xml
                .match_indices("<w:p")
                .map(|(at, _)| at)
                .find(|at| matches!(xml.as_bytes()[at + 4], b' ' | b'>'))
                .expect("a header paragraph");
            let end = tag + xml[tag..].find('>').unwrap() + 1;
            let xml = format!(
                r#"{}<w:pPr><w:jc w:val="{jc}"/></w:pPr>{}"#,
                &xml[..end],
                &xml[end..]
            );
            package.set_part(&part, xml.into_bytes());
        }
        let mut out = Cursor::new(Vec::new());
        package.write_to(&mut out).unwrap();
        let pdf = pdf_of(&Document::from_bytes(&out.into_inner()).unwrap());
        assert_eq!(pdf.page_count(), 12);
        for page in [0, 11] {
            let drawn = pdf.drawn(page);
            assert_contiguous(&drawn, &format!("{jc}, page {page}"));
            let header = &lines(&drawn)[0];
            assert_eq!(header.text, format!("Page {}", page + 1));
            let at = match jc {
                "right" => header.right,
                _ => (header.left + header.right) / 2.0,
            };
            assert!((at - edge).abs() < 0.5, "{jc}, page {page}: {header:?}");
        }
    }
}

/// The pieces of the first line of a page touch, left to right: nothing on
/// it was left behind or drawn over its neighbour.
fn assert_contiguous(drawn: &[Drawn], context: &str) {
    let first = &lines(drawn)[0];
    let mut pieces: Vec<&Drawn> = drawn
        .iter()
        .filter(|piece| (piece.y - first.y).abs() < 0.5)
        .collect();
    pieces.sort_by(|a, b| a.x.total_cmp(&b.x));
    for pair in pieces.windows(2) {
        assert!(
            (pair[1].x - pair[0].end).abs() < 0.5,
            "{context}: {:?} then {:?}",
            pair[0],
            pair[1]
        );
    }
}

// A STYLEREF and a PAGE field on one right-aligned line: the chapter title
// is longer and the page number wider than their stored results, and the
// line still ends at the margin, its pieces side by side. (This checks the
// result; that the text moves once for both kinds of field holds by
// construction, as no document reached here makes STYLEREF change width
// after PAGE moved the line.)
#[test]
fn a_right_aligned_header_with_a_styleref_and_a_page_field_ends_at_the_margin() {
    // Pages 3 to 12 hold no heading: their STYLEREF reads the chapter title
    // of page 2, which it takes after layout, with PAGE (as the thesis'
    // header on the second page of a chapter).
    let mut doc = Document::new();
    doc.add_paragraph("Intro").style("Heading1");
    doc.add_paragraph("A chapter title much longer than the stored one")
        .style("Heading1")
        .page_break_before(true);
    for page in 3..=12 {
        doc.add_paragraph(&format!("Body {page}"))
            .page_break_before(true);
    }
    doc.set_header("placeholder");
    let bytes = doc.to_bytes().unwrap();
    let mut package = oxoffice_opc::OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let headers: Vec<String> = (1..=3)
        .map(|index| format!("/word/header{index}.xml"))
        .filter(|name| package.contains_part(name))
        .collect();
    for part in headers {
        let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
        let tag = xml
            .match_indices("<w:p")
            .map(|(at, _)| at)
            .find(|at| matches!(xml.as_bytes()[at + 4], b' ' | b'>'))
            .expect("a header paragraph");
        let end = tag + xml[tag..].find('>').unwrap() + 1;
        let close = xml.rfind("</w:p>").expect("the paragraph end");
        // Complex fields, as Word writes them (the thesis header).
        let field = |instruction: &str, result: &str| {
            format!(
                r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> {instruction} </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{result}</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#
            )
        };
        let xml = format!(
            r#"{}<w:pPr><w:jc w:val="right"/></w:pPr>{}<w:r><w:t xml:space="preserve"> p. </w:t></w:r>{}{}"#,
            &xml[..end],
            field(r#"STYLEREF "Heading 1""#, "Intro"),
            field("PAGE", "5"),
            &xml[close..]
        );
        package.set_part(&part, xml.into_bytes());
    }
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let pdf = pdf_of(&Document::from_bytes(&out.into_inner()).unwrap());
    assert_eq!(pdf.page_count(), 12);
    for (page, title) in [
        (0, "Intro"),
        (11, "A chapter title much longer than the stored one"),
    ] {
        let drawn = pdf.drawn(page);
        let header = &lines(&drawn)[0];
        assert_eq!(
            header.text,
            format!("{title} p. {}", page + 1),
            "page {page}"
        );
        assert!(
            (header.right - 540.0).abs() < 0.5,
            "page {page}: {header:?}"
        );
        assert_contiguous(&drawn, &format!("page {page}"));
    }
}

#[test]
fn a_first_page_header_replaces_the_header_on_the_first_page_only() {
    let mut doc = doc_with(&["Cover", "Content"]);
    doc.paragraph_mut(1).unwrap().set_page_break_before(true);
    doc.set_header("Running header");
    doc.set_first_page_header("Cover header");
    let pdf = pdf_of(&reopen(&mut doc));
    assert_eq!(pdf.page_count(), 2);
    assert_eq!(pdf.page_text(0), "Cover headerCover");
    assert_eq!(pdf.page_text(1), "Running headerContent");
}

#[test]
fn notes_are_drawn_where_word_prints_them() {
    let mut doc = doc_with(&[
        "First page",
        "Cited on the second page",
        "Closing paragraph",
    ]);
    doc.paragraph_mut(1).unwrap().set_page_break_before(true);
    let cited = doc.paragraph_story_location(1).unwrap().unwrap();
    doc.create_footnote(&cited, "The footnote.").unwrap();
    let cited = doc.paragraph_story_location(1).unwrap().unwrap();
    doc.create_endnote(&cited, "The endnote.").unwrap();
    let pdf = pdf_of(&reopen(&mut doc));
    assert_eq!(pdf.page_count(), 2);
    assert_eq!(pdf.page_text(0), "First page");

    let page = lines(&pdf.drawn(1));
    let texts: Vec<&str> = page.iter().map(|line| line.text.as_str()).collect();
    // The reference marks (the footnote's number first) follow the cited
    // text, raised above its baseline.
    assert_eq!(texts[0], "Cited on the second page");
    assert!(
        texts[1].starts_with('1') && texts[1].chars().count() == 2,
        "{texts:?}"
    );
    assert_eq!(texts[2], "Closing paragraph");
    assert!(
        page[1].y > page[0].y && page[1].y < page[0].y + 6.0,
        "{page:?}"
    );
    // Endnotes follow the last paragraph; footnotes sit at the foot of the
    // page of their reference, above the bottom margin.
    let endnote = page
        .iter()
        .find(|line| line.text.ends_with("The endnote."))
        .unwrap();
    let note = page
        .iter()
        .find(|line| line.text.ends_with("The footnote."))
        .unwrap();
    assert!(endnote.y < page[2].y, "{page:?}");
    assert!(
        note.y < endnote.y && note.y > 72.0 && note.y < 100.0,
        "{page:?}"
    );
}

// Fails today: with no number format set, endnotes are numbered 1, 2, 3 like
// footnotes; Word and LibreOffice number them i, ii, iii. Issue: #158
#[test]
#[should_panic(expected = "the endnote marks read")]
fn endnotes_are_numbered_in_lower_roman_by_default() {
    let mut doc = doc_with(&["First claim", "Second claim"]);
    for index in 0..2 {
        let cited = doc.paragraph_story_location(index).unwrap().unwrap();
        doc.create_endnote(&cited, "Source.").unwrap();
    }
    let pdf = pdf_of(&reopen(&mut doc));
    let lines = lines(&pdf.drawn(0));
    let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
    // Each claim, then its raised mark.
    assert_eq!(
        [texts[0], texts[2]],
        ["First claim", "Second claim"],
        "{texts:?}"
    );
    let marks = [texts[1], texts[3]];
    assert_eq!(marks, ["i", "ii"], "the endnote marks read {marks:?}");
}

// Fails today: endnotes are drawn over a table that ends the document, at
// the height of its rows. Issue: #156
#[test]
#[should_panic(expected = "endnote drawn over the table")]
fn endnotes_are_drawn_below_a_table_that_ends_the_document() {
    let mut doc = doc_with(&["Has a note"]);
    let cited = doc.paragraph_story_location(0).unwrap().unwrap();
    doc.create_endnote(&cited, "Endnote text.").unwrap();
    {
        let mut table = doc.add_table(3, 2);
        for row in 0..3 {
            for column in 0..2 {
                let text = format!("cell {row}{column}");
                table.cell(row, column).unwrap().set_text(&text);
            }
        }
    }
    let pdf = pdf_of(&reopen(&mut doc));
    let last_row = line_of(&pdf, 0, "cell 20");
    let note = line_of(&pdf, 0, "Endnote text.");
    assert!(
        note.y < last_row.y,
        "endnote drawn over the table: its baseline {:.1} is above the last row at {:.1}",
        note.y,
        last_row.y
    );
}

// ---- Tables ----

#[test]
fn table_cells_are_drawn_row_by_row_in_their_columns() {
    let mut doc = doc_with(&["Before"]);
    {
        let mut table = doc.add_table(3, 3);
        for row in 0..3 {
            for column in 0..3 {
                let text = format!("R{row}C{column}");
                table.cell(row, column).unwrap().set_text(&text);
            }
        }
    }
    doc.add_paragraph("After");
    let pdf = pdf_of(&reopen(&mut doc));
    let drawn = pdf.drawn(0);
    let read: Vec<&str> = drawn.iter().map(|piece| piece.text.as_str()).collect();
    assert_eq!(
        read,
        [
            "Before", "R0C0", "R0C1", "R0C2", "R1C0", "R1C1", "R1C2", "R2C0", "R2C1", "R2C2",
            "After"
        ]
    );
    let cells = &drawn[1..10];
    for row in cells.chunks(3) {
        // One baseline per row, the columns left to right.
        assert!(
            row.iter().all(|cell| (cell.y - row[0].y).abs() < 0.01),
            "{row:?}"
        );
        assert!(row[0].end < row[1].x && row[1].end < row[2].x, "{row:?}");
    }
    for column in 0..3 {
        // The columns line up from row to row, the rows go down the page.
        let (top, middle, bottom) = (&cells[column], &cells[column + 3], &cells[column + 6]);
        assert!((top.x - bottom.x).abs() < 0.01 && (top.x - middle.x).abs() < 0.01);
        assert!(top.y > middle.y && middle.y > bottom.y);
    }
    assert!(drawn[0].y > cells[0].y && cells[8].y > drawn[10].y);
}

#[test]
fn a_header_row_is_repeated_at_the_top_of_every_page_of_a_long_table() {
    let mut doc = doc_with(&["Inventory"]);
    {
        let mut table = doc.add_table(90, 2);
        table.row(0).unwrap().header();
        table.cell(0, 0).unwrap().set_text("Item");
        table.cell(0, 1).unwrap().set_text("Count");
        for row in 1..90 {
            table.cell(row, 0).unwrap().set_text(&format!("Item {row}"));
            table
                .cell(row, 1)
                .unwrap()
                .set_text(&format!("{}", row * 10));
        }
    }
    let pdf = pdf_of(&reopen(&mut doc));
    assert!(pdf.page_count() >= 3, "{} pages", pdf.page_count());
    let mut items = Vec::new();
    for page in 0..pdf.page_count() {
        let mut lines = lines(&pdf.drawn(page)).into_iter();
        if page == 0 {
            assert_eq!(lines.next().unwrap().text, "Inventory");
        }
        assert_eq!(
            lines.next().unwrap().text,
            "ItemCount",
            "header row on page {page}"
        );
        items.extend(lines.map(|line| line.text));
    }
    // Every row once, in order, below the repeated headers.
    let expected: Vec<String> = (1..90)
        .map(|row| format!("Item {row}{}", row * 10))
        .collect();
    assert_eq!(items, expected);
}

// ---- Pictures ----

#[test]
fn pictures_are_drawn_once_at_their_size_with_their_pixels() {
    let pixels = [
        [255, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
        [255, 255, 0],
        [0, 255, 255],
        [255, 0, 255],
    ];
    let jpeg = corpus_part("authored/contract.docx", "docProps/thumbnail.jpeg");
    let mut doc = doc_with(&["Above"]);
    let swatch = png(3, 2, &pixels);
    doc.add_picture(
        &swatch,
        "swatch.png",
        Length::inches(2.0),
        Length::inches(1.0),
    );
    doc.add_paragraph("Between");
    doc.add_picture(
        &jpeg,
        "photo.jpeg",
        Length::inches(1.0),
        Length::inches(1.0),
    );
    doc.add_paragraph("Below");
    let pdf = pdf_of(&reopen(&mut doc));
    assert_eq!(pdf.page_count(), 1);

    let pictures = pdf.pictures(0);
    assert_eq!(pictures.len(), 2, "{pictures:?}");
    let lines = lines(&pdf.drawn(0));
    let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(texts, ["Above", "Between", "Below"]);

    // In the flow at the left margin, between the paragraphs around it.
    let [x, bottom, width, height] = pictures[0].rect;
    assert!((x - 72.0).abs() < 0.01, "{:?}", pictures[0].rect);
    assert!((width - 144.0).abs() < 0.01 && (height - 72.0).abs() < 0.01);
    assert!(bottom + height < lines[0].y && bottom > lines[1].y);
    assert_eq!(pictures[0].pixels, (3, 2));
    assert_eq!(
        pictures[0].data,
        pixels.concat(),
        "the samples are the PNG's"
    );

    // A JPEG keeps its bytes: no second lossy compression.
    assert_eq!(pictures[1].filter, "DCTDecode");
    assert!(
        pictures[1].data == jpeg,
        "the JPEG is stored as it was given"
    );
    let [_, bottom, width, height] = pictures[1].rect;
    assert!((width - 72.0).abs() < 0.01 && (height - 72.0).abs() < 0.01);
    assert!(bottom + height < lines[1].y && bottom > lines[2].y);
}

// ---- Links and outline ----

#[test]
fn an_external_hyperlink_becomes_a_link_annotation_over_its_text() {
    let url = "https://example.com/reports?year=2026&q=3";
    let mut doc = Document::new();
    let relationship = doc.add_hyperlink_relationship(url);
    {
        let mut paragraph = doc.add_paragraph("Read ");
        paragraph.add_hyperlink("the full report", &relationship);
        paragraph.add_run(" online.");
    }
    doc.add_paragraph("No link here.");
    let pdf = pdf_of(&reopen(&mut doc));

    let links = pdf.links(0);
    assert!(!links.is_empty());
    assert!(links.iter().all(|(uri, _)| uri == url), "{links:?}");
    let covered = |piece: &Drawn| {
        links.iter().any(|(_, [left, bottom, right, top])| {
            piece.x >= left - 0.01
                && piece.end <= right + 0.01
                && piece.y > *bottom
                && piece.y < *top
        })
    };
    let linked: Vec<String> = pdf
        .drawn(0)
        .into_iter()
        .filter(|piece| covered(piece))
        .map(|piece| piece.text)
        .collect();
    assert_eq!(linked.concat(), "the full report");
}

#[test]
fn headings_become_the_outline_with_their_levels_and_pages() {
    let mut doc = Document::new();
    doc.add_paragraph("Introduction").style("Heading1");
    doc.add_paragraph("Body text");
    doc.add_paragraph("Background").style("Heading2");
    doc.add_paragraph("Results")
        .style("Heading1")
        .page_break_before(true);
    doc.add_paragraph("Numbers").style("Heading3");
    let pdf = pdf_of(&reopen(&mut doc));
    let outline = pdf.outline();
    let outline: Vec<(&str, usize, usize)> = outline
        .iter()
        .map(|(title, depth, page)| (title.as_str(), *depth, *page))
        .collect();
    let expected = [
        ("Introduction", 0, 0),
        ("Background", 1, 0),
        ("Results", 0, 1),
        ("Numbers", 1, 1),
    ];
    assert_eq!(outline, expected);

    // Without a heading there is no outline at all.
    let pdf = pdf_of(&doc_with(&["Plain", "Paragraphs"]));
    assert!(pdf.catalog().get(b"Outlines").is_err());
}

// Fails today: the outline keeps the place a heading had before keep with
// next moved it to the next page, so the bookmark opens the page before; the
// four corpus reports show it on "3. Operations", the thesis on "1.2 Research
// questions". Issue: #156
#[test]
#[should_panic(expected = "but the heading is drawn on page")]
fn an_outline_entry_opens_the_page_its_heading_moved_to() {
    let mut doc = Document::new();
    for number in 1..=27 {
        doc.add_paragraph(&format!("Filler {number}"));
    }
    // Too little room is left for the heading and the paragraph it keeps
    // with, so both start the next page.
    doc.add_paragraph("Moved heading")
        .style("Heading1")
        .keep_with_next(true);
    doc.add_paragraph(&"A paragraph kept on one page. ".repeat(12))
        .keep_together(true);
    let pdf = pdf_of(&reopen(&mut doc));
    let drawn_on = page_with(&pdf, "Moved heading");
    assert_eq!(drawn_on, 1, "the heading starts the second page");
    let outline = pdf.outline();
    assert_eq!(outline.len(), 1, "{outline:?}");
    let (title, _, opens) = &outline[0];
    assert_eq!(title, "Moved heading");
    assert_eq!(
        *opens, drawn_on,
        "the outline opens page {opens} but the heading is drawn on page {drawn_on}"
    );
}

#[test]
fn the_links_of_a_libreoffice_letter_cover_their_text() {
    let pdf = pdf_of(&corpus_doc("libreoffice/letter.docx"));
    let pages = 0..pdf.page_count();
    let links: Vec<(String, [f64; 4])> = pages.clone().flat_map(|page| pdf.links(page)).collect();
    let drawn: Vec<Drawn> = pages.flat_map(|page| pdf.drawn(page)).collect();
    let mut uris: Vec<&str> = links.iter().map(|(uri, _)| uri.as_str()).collect();
    uris.sort();
    uris.dedup();
    assert_eq!(uris.len(), 3, "{uris:?}");
    for (uri, text) in [
        ("https://www.northwind.example/pricing/2027", "our website"),
        (
            "https://portal.northwind.example/renewals?ref=NW-2026-114",
            "customer portal",
        ),
        (
            "mailto:julien.moreau@northwind.example",
            "julien.moreau@northwind.example",
        ),
    ] {
        let rects: Vec<&[f64; 4]> = links
            .iter()
            .filter(|(found, _)| found == uri)
            .map(|(_, rect)| rect)
            .collect();
        assert!(!rects.is_empty(), "no link to {uri}");
        let covered: String = drawn
            .iter()
            .filter(|piece| {
                rects.iter().any(|[left, bottom, right, top]| {
                    piece.x >= left - 0.01
                        && piece.end <= right + 0.01
                        && piece.y > *bottom
                        && piece.y < *top
                })
            })
            .map(|piece| piece.text.as_str())
            .collect();
        assert_eq!(covered.trim(), text, "{uri}");
    }
}

// ---- Fonts and scripts ----

#[test]
fn every_face_is_embedded_with_a_map_back_to_text() {
    let mut doc = Document::new();
    {
        let mut paragraph = doc.add_paragraph("Plain ");
        paragraph.add_run("bold").bold(true);
        paragraph.add_run(" and ");
        paragraph.add_run("italic").italic(true);
    }
    let pdf = pdf_of(&reopen(&mut doc));
    assert_eq!(pdf.page_text(0), "Plain bold and italic");
    let fonts = pdf.embedded_fonts();
    let expected = [
        ("Carlito".to_owned(), true, true),
        ("Carlito-Bold".to_owned(), true, true),
        ("Carlito-Italic".to_owned(), true, true),
    ];
    assert_eq!(fonts, expected);
}

#[test]
fn a_caller_font_draws_the_runs_of_its_family() {
    let mono = oxoffice_layout::bundled_fonts::bundled_font_data()
        .into_iter()
        .find(|(family, _)| *family == "Liberation Mono")
        .expect("Liberation Mono is bundled")
        .1;
    let mut doc = Document::new();
    doc.add_paragraph("Body ")
        .add_run("brand text")
        .font("Brand Sans");
    let doc = reopen(&mut doc);

    // Without the font, the run falls back to a bundled face, and says so.
    let (bytes, diagnostics) = doc
        .to_pdf_with_diagnostics(&[], RenderOptions::default())
        .unwrap();
    let fonts = Pdf::read(&bytes).embedded_fonts();
    assert!(
        fonts.iter().all(|font| font.0 != "LiberationMono"),
        "{fonts:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("'Brand Sans' substituted")),
        "{diagnostics:?}"
    );

    let fonts = [("Brand Sans", mono)];
    let options = RenderOptions::default();
    let pdf = Pdf::read(
        &doc.to_pdf_with_fonts_deterministic_and_options(&fonts, options)
            .unwrap(),
    );
    assert_eq!(pdf.page_text(0), "Body brand text");
    let fonts: Vec<String> = pdf
        .embedded_fonts()
        .into_iter()
        .map(|font| font.0)
        .collect();
    assert_eq!(fonts, ["Carlito", "LiberationMono"]);
}

#[test]
fn accented_and_symbol_text_reads_back_and_hidden_text_is_not_drawn() {
    let text = "Café, naïve façade — 12 € œuvre « déjà » Ångström ß";
    let mut doc = doc_with(&[text]);
    doc.add_paragraph("Shown ")
        .add_run("hidden words")
        .hidden(true);
    let pdf = pdf_of(&reopen(&mut doc));
    let drawn = pdf.drawn(0);
    let lines = lines(&drawn);
    let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(texts, [text, "Shown "]);
    assert!(drawn.iter().all(|piece| piece.notdef == 0), "{drawn:?}");
}

#[test]
fn arabic_and_hebrew_text_reads_back_in_logical_order() {
    let arabic = "مرحبا بالعالم الجميل";
    let hebrew = "שלום עולם יפה";
    let mut doc = Document::new();
    doc.add_paragraph(arabic).right_to_left(true);
    doc.add_paragraph(hebrew).right_to_left(true);
    let pdf = pdf_of(&reopen(&mut doc));
    let drawn = pdf.drawn(0);
    // Real glyphs that map back to the characters of their words.
    for piece in &drawn {
        assert!(piece.notdef == 0 && glyphs_draw_text(piece), "{piece:?}");
    }
    let lines = lines(&drawn);
    for (line, text) in lines.iter().zip([arabic, hebrew]) {
        assert_eq!(line.text, text);
        // The first word is drawn rightmost, the last word leftmost.
        let words: Vec<&Drawn> = drawn
            .iter()
            .filter(|piece| (piece.y - line.y).abs() < 0.5 && !piece.text.trim().is_empty())
            .collect();
        assert_eq!(words.len(), 3, "{words:?}");
        assert!(
            words[0].x > words[1].x && words[1].x > words[2].x,
            "{words:?}"
        );
    }
    let fonts: Vec<String> = pdf
        .embedded_fonts()
        .into_iter()
        .map(|font| font.0)
        .collect();
    assert!(fonts.contains(&"NotoSansArabic".to_owned()), "{fonts:?}");
}

/// Whether `text` holds Chinese, Japanese or Korean characters.
fn has_cjk(text: &str) -> bool {
    text.chars().any(|ch| {
        matches!(ch, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{9fff}' | '\u{ac00}'..='\u{d7af}')
    })
}

// The default render resolves families in the bundled fonts and takes a
// system font for the characters they lack. What it draws depends on the
// machine's fonts, so this holds on any machine: each CJK piece is either drawn
// with real glyphs while a diagnostic names the system font, or drawn as boxes
// while a diagnostic says no system font has them. The fallback itself is
// proved with fixed fonts in oxoffice-layout's system_fallback tests.
#[test]
fn chinese_japanese_and_korean_text_takes_a_system_font_or_says_none_was_found() {
    let sentences = [
        "我们的公司在北京有很多客户。",
        "東京の会社は大きいです。",
        "한국어 문장입니다.",
    ];
    let doc = doc_with(&sentences);
    let (bytes, diagnostics) = doc
        .to_pdf_with_diagnostics(&[], RenderOptions::default())
        .expect("document converts to PDF");
    let messages: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
    let pdf = Pdf::read(&bytes);
    let drawn = pdf.drawn(0);
    let texts: Vec<String> = lines(&drawn).into_iter().map(|line| line.text).collect();
    assert_eq!(texts, sentences);

    let cjk: Vec<&Drawn> = drawn.iter().filter(|piece| has_cjk(&piece.text)).collect();
    assert!(!cjk.is_empty());
    for piece in cjk {
        if piece.notdef == 0 {
            assert!(glyphs_draw_text(piece), "{piece:?}");
            assert!(
                messages
                    .iter()
                    .any(|message| message.contains("is drawn with the system font '")),
                "{:?} is drawn with real glyphs, but no diagnostic names the system font: \
                 {messages:#?}",
                piece.text
            );
        } else {
            assert!(
                messages
                    .iter()
                    .any(|message| message.contains("drawn as empty boxes")
                        && message.contains("no system font on this machine has them")),
                "{:?} is drawn as boxes, but no diagnostic says no system font has them: \
                 {messages:#?}",
                piece.text
            );
        }
    }
}

#[test]
fn bundled_fonts_only_draws_chinese_japanese_and_korean_text_as_named_boxes() {
    let doc = doc_with(&["我们的公司在北京", "東京のかいしゃ", "Latin stays drawn."]);
    let bundled_only = RenderOptions::default().with_bundled_fonts_only(true);
    let (bytes, diagnostics) = doc
        .to_pdf_with_diagnostics(&[], bundled_only)
        .expect("document converts to PDF");
    let boxes: Vec<&str> = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .filter(|message| message.contains("drawn as empty boxes"))
        .collect();
    assert_eq!(boxes.len(), 2, "{boxes:#?}");
    assert!(
        boxes[0].starts_with("Chinese, Japanese or Korean ideograph (Han) text has no glyph"),
        "{}",
        boxes[0]
    );
    assert!(
        boxes[1].starts_with("Japanese (kana) text has no glyph"),
        "{}",
        boxes[1]
    );
    for message in &boxes {
        for way_out in [
            "bundled fonts only",
            "--bundled-fonts-only",
            "bundled_fonts_only=False",
            "with_bundled_fonts_only(false)",
            "font file",
        ] {
            assert!(message.contains(way_out), "{message}");
        }
    }
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("system font '")),
        "{diagnostics:?}"
    );
    let pdf = Pdf::read(&bytes);
    assert!(pdf.drawn(0).iter().any(|piece| piece.notdef > 0));
    // The same on every run, as on every machine.
    let (again, _) = doc
        .to_pdf_with_diagnostics(&[], bundled_only)
        .expect("document converts to PDF");
    assert_eq!(bytes, again);

    // Text the bundled fonts draw says nothing.
    let (_, diagnostics) = doc_with(&["Plain Latin, مرحبا, שלום"])
        .to_pdf_with_diagnostics(&[], bundled_only)
        .expect("document converts to PDF");
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("empty boxes")),
        "{diagnostics:?}"
    );
}

// Families resolve in the bundled fonts in both modes, so a document they
// cover is the same file with or without the system fallback, on any machine.
#[test]
fn text_the_bundled_fonts_cover_renders_the_same_with_the_system_fallback() {
    let doc = doc_with(&[
        "Plain Latin in the default face, with digits 0123456789.",
        "مرحبا بالعالم",
        "שלום עולם",
        "Ελληνικά και кириллица.",
        // One run whose scripts no single bundled face has.
        "Plain Latin, مرحبا, שלום",
    ]);
    let default = doc.to_pdf().expect("document converts to PDF");
    let bundled_only = doc
        .to_pdf_with_options(RenderOptions::default().with_bundled_fonts_only(true))
        .expect("document converts to PDF");
    assert_eq!(default, bundled_only);
    assert_eq!(default, doc.to_pdf_deterministic().unwrap());
}

// A run the bundled fonts partly draw keeps its bundled face for that part:
// only the characters they lack may take a system font, so the Latin words
// are drawn alike in both modes, on any machine (the line's height may grow
// with a taller system face, so only the horizontal placement is compared).
#[test]
fn latin_words_beside_text_the_bundled_fonts_lack_are_drawn_alike_in_both_modes() {
    let doc = doc_with(&["Version 2 of the plan 计划书已经批准"]);
    let latin = |options: RenderOptions| {
        let pdf = Pdf::read(&doc.to_pdf_with_options(options).unwrap());
        pdf.drawn(0)
            .into_iter()
            .filter(|piece| !has_cjk(&piece.text) && !piece.text.trim().is_empty())
            .map(|piece| (piece.text, piece.glyphs, piece.x, piece.end, piece.size))
            .collect::<Vec<_>>()
    };
    let default = latin(RenderOptions::default());
    assert!(!default.is_empty());
    assert_eq!(
        default,
        latin(RenderOptions::default().with_bundled_fonts_only(true))
    );
}

#[test]
fn system_fonts_first_and_bundled_fonts_only_are_refused_together() {
    let options = RenderOptions::default()
        .with_system_fonts(true)
        .with_bundled_fonts_only(true);
    let error = doc_with(&["Text"])
        .to_pdf_with_options(options)
        .expect_err("the two switches exclude each other");
    assert!(error.to_string().contains("exclude each other"), "{error}");

    // The caller-font renders follow the same switches.
    let doc = doc_with(&["Text"]);
    let fonts: [(&str, &[u8]); 1] = [(
        "Caller",
        oxoffice_layout::bundled_fonts::LIBERATION_SANS_REGULAR,
    )];
    let raster = oxoffice_pdf::RasterOptions {
        dpi: 36.0,
        format: oxoffice_pdf::RasterFormat::Png {
            transparent_background: false,
        },
    };
    assert!(doc.to_pdf_with_fonts_and_options(&fonts, options).is_err());
    assert!(
        doc.render_page_to_png_with_fonts_and_options(0, 36.0, &fonts, options)
            .is_err()
    );
    assert!(
        doc.render_page_to_svg_with_fonts_and_options(0, &fonts, options)
            .is_err()
    );
    assert!(
        doc.render_pages_with_fonts_and_options(&[0], raster, &fonts, options)
            .is_err()
    );
    assert!(
        doc.render_all_pages_with_fonts_and_options(36.0, &fonts, options)
            .is_err()
    );
}

// ---- Tags, PDF/A and metadata ----

#[test]
fn ordinary_output_is_tagged_with_headings_paragraphs_tables_lists_and_figures() {
    let mut doc = Document::new();
    doc.add_paragraph("Quarterly results").style("Heading1");
    doc.add_paragraph("Revenue grew.");
    {
        let mut table = doc.add_table(2, 2);
        table.row(0).unwrap().header();
        table.cell(0, 0).unwrap().set_text("Region");
        table.cell(0, 1).unwrap().set_text("Revenue");
        table.cell(1, 0).unwrap().set_text("North");
        table.cell(1, 1).unwrap().set_text("1,402");
    }
    doc.add_bullet_list_item("First point", 0);
    let stories = doc.stories().unwrap();
    let body = stories
        .into_iter()
        .find(|story| story.kind() == StoryKind::Body)
        .unwrap();
    let options = PictureOptions {
        width: Length::inches(1.0),
        height: Length::inches(1.0),
        crop: None,
        anchor: None,
        name: None,
        description: Some("Revenue chart".to_owned()),
        title: None,
        decorative: false,
    };
    doc.add_picture_with_options(&body, &png(1, 1, &[[0, 0, 0]]), "dot.png", options)
        .unwrap();
    let pdf = pdf_of(&reopen(&mut doc));

    let mark_info = pdf.dict(pdf.catalog().get(b"MarkInfo").expect("MarkInfo"));
    assert_eq!(
        mark_info.get(b"Marked").and_then(Object::as_bool).ok(),
        Some(true)
    );
    assert!(pdf.page(0).get(b"StructParents").is_ok());

    let elements = pdf.structure();
    let roles: Vec<&str> = elements
        .iter()
        .map(|element| element.0.as_str())
        .filter(|role| !["P", "LBody"].contains(role))
        .collect();
    assert_eq!(
        roles,
        [
            "Document", "H1", "Table", "TR", "TH", "TH", "TR", "TD", "TD", "L", "LI", "Figure"
        ]
    );
    let text_of = |role: &str| -> Vec<String> {
        (0..elements.len())
            .filter(|index| elements[*index].0 == role)
            .map(|index| tagged_text(&pdf, &elements, index))
            .collect()
    };
    assert_eq!(text_of("H1"), ["Quarterly results"]);
    assert_eq!(text_of("TH"), ["Region", "Revenue"]);
    assert_eq!(text_of("TD"), ["North", "1,402"]);
    assert!(
        text_of("LI")[0].ends_with("First point"),
        "{:?}",
        text_of("LI")
    );
    let figure = elements
        .iter()
        .find(|element| element.0 == "Figure")
        .unwrap();
    assert_eq!(figure.2.as_deref(), Some("Revenue chart"));
    assert_eq!(
        figure.3.len(),
        1,
        "the figure owns the picture's marked content"
    );
}

#[test]
fn pdfa_output_declares_its_part_and_carries_an_output_intent() {
    let doc = doc_with(&["Archived"]);
    for (profile, part) in [(PdfConformance::PdfA2b, 2), (PdfConformance::PdfA3b, 3)] {
        let pdf = Pdf::read(&doc.to_pdfa_deterministic(profile).unwrap());
        assert_eq!(pdf.page_text(0), "Archived");
        let xmp = pdf.xmp().expect("PDF/A carries XMP metadata");
        assert!(
            xmp.contains(&format!("<pdfaid:part>{part}</pdfaid:part>")),
            "{xmp}"
        );
        assert!(
            xmp.contains("<pdfaid:conformance>B</pdfaid:conformance>"),
            "{xmp}"
        );
        let intents = pdf.resolve(pdf.catalog().get(b"OutputIntents").expect("OutputIntents"));
        let intent = pdf.dict(&intents.as_array().expect("OutputIntents array")[0]);
        let kind = intent
            .get(b"S")
            .and_then(Object::as_name)
            .expect("intent kind");
        assert_eq!(kind, b"GTS_PDFA1");
        let icc = pdf.resolve(intent.get(b"DestOutputProfile").expect("ICC profile"));
        let channels = icc.as_stream().expect("ICC stream").dict.get(b"N");
        assert_eq!(channels.and_then(Object::as_i64).ok(), Some(3));
        assert!(
            pdf.doc.trailer.get(b"ID").is_ok(),
            "PDF/A needs a file identifier"
        );
        assert!(pdf.embedded_fonts().iter().all(|font| font.1));
    }
    // An ordinary conversion claims no archival profile.
    let plain = pdf_of(&doc);
    assert!(plain.catalog().get(b"OutputIntents").is_err());
    assert!(!plain.xmp().unwrap_or_default().contains("pdfaid"));
}

#[test]
fn pdfa_output_refuses_a_link_it_cannot_archive() {
    let target = "file:///srv/share/budget.xlsx";
    let mut doc = Document::new();
    let relationship = doc.add_hyperlink_relationship(target);
    doc.add_paragraph("See ")
        .add_hyperlink("the budget", &relationship);
    let doc = reopen(&mut doc);
    match doc.to_pdfa_deterministic(PdfConformance::PdfA2b) {
        Err(oxoffice_docx::Error::Pdf(oxoffice_pdf::PdfError::ForbiddenAction { action })) => {
            assert_eq!(action, target)
        }
        Err(error) => panic!("expected a refusal of the file link, got {error}"),
        Ok(_) => panic!("expected a refusal of the file link, got a PDF"),
    }
    // An ordinary PDF keeps the link.
    let links = pdf_of(&doc).links(0);
    assert!(
        !links.is_empty() && links.iter().all(|(uri, _)| uri == target),
        "{links:?}"
    );
}

#[test]
fn title_author_subject_and_keywords_reach_the_document_information() {
    let title = "Rapport été 2026 – 東京";
    let mut doc = doc_with(&["Body"]);
    doc.set_title(title);
    doc.set_author("Zoë Martin");
    doc.set_subject("Quarterly review");
    doc.set_keywords("revenue, churn, Lyon");
    let pdf = pdf_of(&reopen(&mut doc));
    assert_eq!(pdf.info(b"Title").as_deref(), Some(title));
    assert_eq!(pdf.info(b"Author").as_deref(), Some("Zoë Martin"));
    assert_eq!(pdf.info(b"Subject").as_deref(), Some("Quarterly review"));
    assert_eq!(
        pdf.info(b"Keywords").as_deref(),
        Some("revenue, churn, Lyon")
    );
    assert_eq!(pdf.info(b"Producer").as_deref(), Some("oxoffice"));
    // Viewers show the title, not the file name, in their title bar.
    let preferences = pdf.dict(
        pdf.catalog()
            .get(b"ViewerPreferences")
            .expect("preferences"),
    );
    let display = preferences
        .get(b"DisplayDocTitle")
        .and_then(Object::as_bool);
    assert_eq!(display.ok(), Some(true));
    let xmp = pdf.xmp().expect("tagged output carries XMP metadata");
    let dc_title = format!("<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{title}</rdf:li>");
    assert!(xmp.contains(&dc_title), "{xmp}");
}

#[test]
fn tracked_changes_are_drawn_in_the_tracked_view_only() {
    let mut original = doc_with(&["Keep OLDWORD here."]);
    let edited = doc_with(&["Keep NEWWORD here."]);
    original
        .compare(&edited, "Reviewer", "2026-11-21T10:00:00Z")
        .unwrap();
    let redline = reopen(&mut original);

    assert_eq!(pdf_of(&redline).page_text(0), "Keep NEWWORD here.");
    let options = RenderOptions::default().with_revision_view(RevisionView::Tracked);
    let tracked = Pdf::read(&redline.to_pdf_deterministic_with_options(options).unwrap());
    let text = tracked.page_text(0);
    assert!(
        text.contains("OLDWORD") && text.contains("NEWWORD"),
        "{text}"
    );
}

// ---- Determinism and other outputs ----

#[test]
fn the_same_document_converts_to_the_same_bytes() {
    let build = || {
        let mut doc = Document::new();
        doc.set_title("Same bytes");
        doc.add_paragraph("Heading").style("Heading1");
        doc.add_paragraph("Plain and ").add_run("bold").bold(true);
        doc.add_table(2, 2).cell(1, 1).unwrap().set_text("cell");
        let dot = png(1, 1, &[[9, 9, 9]]);
        doc.add_picture(&dot, "dot.png", Length::inches(0.5), Length::inches(0.5));
        doc
    };
    // Two documents built apart, and the next session's copy.
    let first = build().to_pdf_deterministic().unwrap();
    assert!(
        first == build().to_pdf_deterministic().unwrap(),
        "two builds convert differently"
    );
    let mut doc = build();
    let reopened = reopen(&mut doc).to_pdf_deterministic().unwrap();
    assert!(first == reopened, "a reopened copy converts differently");
    let archived = doc.to_pdfa_deterministic(PdfConformance::PdfA2b).unwrap();
    let again = build()
        .to_pdfa_deterministic(PdfConformance::PdfA2b)
        .unwrap();
    assert!(archived == again, "two PDF/A builds differ");

    // to_pdf repeats itself on one machine too, and save_pdf writes its bytes.
    let plain = doc.to_pdf().unwrap();
    assert!(
        plain == build().to_pdf().unwrap(),
        "two builds convert differently through to_pdf"
    );
    let name = format!("oxoffice-docx-pdf-{}.pdf", std::process::id());
    let path = std::env::temp_dir().join(name);
    doc.save_pdf(&path).unwrap();
    let saved = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(saved == plain, "save_pdf wrote other bytes");
}

/// How many pixels of `rgb` the 72 DPI image of page 0 holds inside the box
/// of the drawn text `text`, from its baseline to its font size above it.
fn pixels_in_text_box(doc: &Document, text: &str, rgb: [u8; 3]) -> usize {
    let pdf = pdf_of(doc);
    let piece = pdf
        .drawn(0)
        .into_iter()
        .find(|piece| piece.text.contains(text))
        .unwrap_or_else(|| panic!("{text} is drawn"));
    let image = Pixels::decode(&doc.render_page_to_png(0, 72.0).unwrap().expect("page 0"));
    let height = f64::from(image.height);
    let mut count = 0;
    for y in (height - piece.y - piece.size * 0.7) as u32..(height - piece.y) as u32 {
        for x in piece.x as u32..piece.end as u32 {
            let at = ((y * image.width + x) * 4) as usize;
            if image.rgba[at..at + 3] == rgb {
                count += 1;
            }
        }
    }
    count
}

#[test]
fn a_run_shading_is_drawn_behind_its_text() {
    let run = |fill: &str, text: &str| {
        format!(
            r#"<w:r><w:rPr><w:shd w:val="clear" w:color="auto" w:fill="{fill}"/></w:rPr><w:t xml:space="preserve">{text} </w:t></w:r>"#
        )
    };
    // Word's pale green, red and grey cell styles applied to text, then a
    // highlight over a shading, and a dark fill under automatic text.
    let doc = doc_with_body(&format!(
        r#"<w:p>{}{}{}<w:r><w:rPr><w:highlight w:val="yellow"/><w:shd w:val="clear" w:fill="C6EFCE"/></w:rPr><w:t xml:space="preserve">Both </w:t></w:r>{}</w:p>"#,
        run("C6EFCE", "Done"),
        run("FFC7CE", "Late"),
        run("E7E6E6", "Open"),
        run("1F3864", "Dark"),
    ));
    for (text, rgb) in [
        ("Done", [0xC6, 0xEF, 0xCE]),
        ("Late", [0xFF, 0xC7, 0xCE]),
        ("Open", [0xE7, 0xE6, 0xE6]),
        ("Dark", [0x1F, 0x38, 0x64]),
    ] {
        assert!(
            pixels_in_text_box(&doc, text, rgb) > 20,
            "{text} has its fill behind it"
        );
    }
    // Word draws the highlight over the shading.
    assert!(pixels_in_text_box(&doc, "Both", [0xFF, 0xFF, 0x00]) > 20);
    assert_eq!(pixels_in_text_box(&doc, "Both", [0xC6, 0xEF, 0xCE]), 0);
    // Automatic text turns white on a dark fill, as on a dark paragraph.
    assert!(pixels_in_text_box(&doc, "Dark", [0xFF, 0xFF, 0xFF]) > 0);
    assert_eq!(pixels_in_text_box(&doc, "Dark", [0, 0, 0]), 0);
}

#[test]
fn a_solid_or_percentage_shading_paints_its_pattern_colour() {
    // A solid pattern paints its foreground colour, a percentage mixes it
    // into the fill: 25 % black over white is (191, 191, 191).
    let doc = doc_with_body(concat!(
        r#"<w:p><w:pPr><w:shd w:val="solid" w:color="C00000" w:fill="FFFFFF"/></w:pPr><w:r><w:t>Paragraph</w:t></w:r></w:p>"#,
        r#"<w:p><w:r><w:rPr><w:shd w:val="pct25" w:color="000000" w:fill="FFFFFF"/></w:rPr><w:t>Quarter</w:t></w:r></w:p>"#,
        r#"<w:tbl><w:tblPr><w:tblW w:w="5000" w:type="pct"/></w:tblPr><w:tblGrid><w:gridCol w:w="9000"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:shd w:val="solid" w:color="00B050" w:fill="auto"/></w:tcPr><w:p><w:r><w:t>Cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
        r#"<w:p><w:r><w:rPr><w:shd w:val="pct12" w:color="000000" w:fill="FFFFFF"/></w:rPr><w:t>Eighth</w:t></w:r></w:p>"#,
        r#"<w:tbl><w:tblPr><w:tblW w:w="5000" w:type="pct"/><w:shd w:val="clear" w:fill="FF00FF"/></w:tblPr><w:tblGrid><w:gridCol w:w="9000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>Table</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
        r#"<w:p/>"#,
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="5000" w:type="pct"/><w:shd w:val="clear" w:fill="00FFFF"/></w:tblPr><w:tblGrid><w:gridCol w:w="9000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>Styled</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
        r#"<w:p/>"#,
    ));
    assert!(pixels_in_text_box(&doc, "Paragraph", [0xC0, 0x00, 0x00]) > 20);
    assert!(pixels_in_text_box(&doc, "Quarter", [191, 191, 191]) > 20);
    assert!(pixels_in_text_box(&doc, "Cell", [0x00, 0xB0, 0x50]) > 20);
    // pct12 stands for 12.5 %: 255 * 0.875 is 223.
    assert!(pixels_in_text_box(&doc, "Eighth", [223, 223, 223]) > 20);
    // The table's own shading fills a cell that has none.
    assert!(pixels_in_text_box(&doc, "Table", [0xFF, 0x00, 0xFF]) > 20);
    // So does it under a table style, over the style's own.
    assert!(pixels_in_text_box(&doc, "Styled", [0x00, 0xFF, 0xFF]) > 20);
}

#[test]
fn a_list_number_takes_the_shading_of_its_level() {
    let mut doc = Document::new();
    doc.add_numbered_list_item("Item", 0);
    let mut package = oxoffice_opc::OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap()))
        .expect("package opens");
    let xml = String::from_utf8(package.get_part("/word/numbering.xml").unwrap().to_vec()).unwrap();
    // At 24 pt the number shows ink pixels of its own colour.
    let shading = r#"<w:sz w:val="48"/><w:shd w:val="clear" w:color="auto" w:fill="0000FF"/>"#;
    let level = xml.find(r#"<w:lvl w:ilvl="0""#).expect("level 0");
    let end = level + xml[level..].find("</w:lvl>").expect("level end");
    let xml = match xml[level..end].find("<w:rPr>") {
        Some(at) => {
            let at = level + at + "<w:rPr>".len();
            format!("{}{shading}{}", &xml[..at], &xml[at..])
        }
        None => format!("{}<w:rPr>{shading}</w:rPr>{}", &xml[..end], &xml[end..]),
    };
    package.set_part("/word/numbering.xml", xml.into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let doc = Document::from_bytes(&bytes.into_inner()).unwrap();
    assert!(pixels_in_text_box(&doc, "1.", [0, 0, 0xFF]) > 10);
    // Its automatic ink contrasts with the dark fill.
    assert!(pixels_in_text_box(&doc, "1.", [0xFF, 0xFF, 0xFF]) > 0);
    assert_eq!(pixels_in_text_box(&doc, "1.", [0, 0, 0]), 0);
}

#[test]
fn a_single_page_renders_to_png_at_the_size_of_its_pdf_page() {
    let mut doc = doc_with(&["Portrait"]);
    doc.insert_section(1).unwrap();
    doc.add_paragraph("Landscape");
    doc.section_mut(1)
        .unwrap()
        .set_orientation(ST_PageOrientation::Landscape);
    let doc = reopen(&mut doc);
    let pdf = pdf_of(&doc);
    assert_eq!(pdf.page_count(), 2);
    for page in 0..2 {
        let image = doc
            .render_page_to_png(page, 36.0)
            .unwrap()
            .expect("page renders");
        assert!(image.starts_with(b"\x89PNG\r\n\x1a\n"));
        let width = u32::from_be_bytes(image[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(image[20..24].try_into().unwrap());
        // 36 dpi is half a pixel per point.
        let (pdf_width, pdf_height) = pdf.page_size(page);
        let expected = ((pdf_width / 2.0) as u32, (pdf_height / 2.0) as u32);
        assert_eq!((width, height), expected, "page {page}");
    }
    assert!(doc.render_page_to_png(2, 36.0).unwrap().is_none());
}

// ---- Corpus ----

#[test]
fn every_corpus_report_draws_its_text_header_footer_picture_and_outline() {
    const HEADER: &str = "Northwind Analytics: Quarterly Operations Review";
    const FOOTER: &str = "Internal. Prepared by the operations team.";
    for producer in PRODUCERS {
        let doc = corpus_doc(&format!("{producer}/report.docx"));
        let pdf = pdf_of(&doc);
        let last = pdf.page_count() - 1;
        assert_size(pdf.page_size(0), LETTER, producer);
        assert_size(pdf.page_size(last), (792.0, 612.0), producer);

        for page in 0..=last {
            let lines = lines(&pdf.drawn(page));
            assert_eq!(lines[0].text, HEADER, "{producer} page {page}");
            assert_eq!(
                lines[lines.len() - 1].text,
                FOOTER,
                "{producer} page {page}"
            );
        }

        // Every body paragraph, in document order, across the pages.
        let body: String = (0..=last).map(|page| pdf.body_text(page, 72.0)).collect();
        let body = squeeze(&body);
        let mut from = 0;
        for paragraph in texts(&doc) {
            let paragraph = squeeze(&paragraph);
            let found = body[from..]
                .find(&paragraph)
                .unwrap_or_else(|| panic!("{producer}: {paragraph:?} is missing or out of order"));
            from += found + paragraph.len();
        }
        let appendix = pdf.body_text(last, 72.0);
        assert!(
            appendix.starts_with("Appendix A. Regional detail"),
            "{producer}"
        );

        // The picture is drawn once, above its caption.
        let pictures: Vec<(usize, Picture)> = (0..=last)
            .flat_map(|page| {
                pdf.pictures(page)
                    .into_iter()
                    .map(move |picture| (page, picture))
            })
            .collect();
        assert_eq!(pictures.len(), 1, "{producer}");
        let (page, picture) = &pictures[0];
        assert_eq!(picture.pixels, (900, 450), "{producer}");
        let caption = line_of(&pdf, *page, "Figure 1.");
        assert!(
            picture.rect[1] > caption.y,
            "{producer}: picture below its caption"
        );

        // The headings make the outline.
        let outline = pdf.outline();
        let titles: Vec<(&str, usize)> = outline
            .iter()
            .map(|(title, depth, _)| (title.as_str(), *depth))
            .collect();
        let expected = [
            ("1. Highlights", 0),
            ("2. Financial overview", 0),
            ("3. Operations", 0),
            ("3.1 Support", 1),
            ("3.2 Next steps", 1),
            ("Appendix A. Regional detail", 0),
        ];
        assert_eq!(titles, expected, "{producer}");
    }
}

#[test]
fn the_minutes_keep_their_arabic_hebrew_chinese_and_japanese_text() {
    for name in ["authored/minutes.docx", "libreoffice/minutes.docx"] {
        let doc = corpus_doc(name);
        let pdf = pdf_of(&doc);
        // LibreOffice lays the minutes out on three A4 pages too.
        assert_eq!(pdf.page_count(), 3, "{name}");
        for page in 0..3 {
            assert_size(pdf.page_size(page), A4, name);
        }
        let all: String = (0..3).map(|page| pdf.page_text(page)).collect();
        let all = squeeze(&all);
        for paragraph in texts(&doc) {
            assert!(
                all.contains(&squeeze(&paragraph)),
                "{name}: {paragraph:?} is not drawn"
            );
        }
        // The Arabic summary reads in logical order, spaces and all.
        let summary = texts(&doc)
            .into_iter()
            .find(|text| text.starts_with("وافقت اللجنة"))
            .expect("the Arabic summary");
        let page = page_with(&pdf, &summary);
        assert!(pdf.page_text(page).contains(&summary), "{name}");
        // Every piece but the Chinese and Japanese ones (see the known
        // failure on them above) is drawn with real glyphs that map back to
        // its characters, Arabic and Hebrew included.
        let cjk = |ch: char| {
            ('\u{3000}'..='\u{9FFF}').contains(&ch) || ('\u{FF00}'..='\u{FFEF}').contains(&ch)
        };
        let mut right_to_left = 0;
        for page in 0..3 {
            for piece in pdf.drawn(page) {
                if piece.text.chars().any(cjk) {
                    continue;
                }
                assert!(
                    piece.notdef == 0 && glyphs_draw_text(&piece),
                    "{name}: {piece:?}"
                );
                let script = |ch: char| ('\u{0590}'..='\u{06FF}').contains(&ch);
                right_to_left += usize::from(piece.text.chars().any(script));
            }
        }
        assert!(
            right_to_left > 50,
            "{name}: {right_to_left} right-to-left pieces"
        );
        let outline = pdf.outline();
        let arabic = outline
            .iter()
            .any(|(title, depth, _)| title == "ملخص الاجتماع" && *depth == 1);
        assert!(arabic, "{name}: {outline:?}");
    }
}

#[test]
fn the_manual_repeats_its_table_header_flows_into_columns_and_draws_its_chart() {
    let pdf = pdf_of(&corpus_doc("authored/manual.docx"));
    // LibreOffice lays the manual out on seven A4 pages too.
    assert_eq!(pdf.page_count(), 7);
    for page in 0..7 {
        let footer = lines(&pdf.drawn(page)).pop().unwrap();
        let expected = format!(
            "SC-40 User and Maintenance Manual, version 2.3 - page {}",
            page + 1
        );
        assert_eq!(footer.text, expected);
    }

    // The error code table runs over pages 2 to 5, each opening with its
    // header row.
    let codes: Vec<usize> = (0..7)
        .filter(|page| pdf.page_text(*page).contains("Battery too hot"))
        .collect();
    assert_eq!(codes, [1, 2, 3, 4]);
    for page in &codes[1..] {
        let first = lines(&pdf.drawn(*page)).remove(0);
        assert_eq!(first.text, "CodeMessageCauseWhat to do", "page {page}");
    }

    // The maintenance section flows into a second column, from the top.
    let page = page_with(&pdf, "5. Maintenance");
    let lines = lines(&pdf.drawn(page));
    let body: Vec<&Line> = lines.iter().filter(|line| line.y > 60.0).collect();
    let second = body
        .iter()
        .position(|line| line.left > 297.0)
        .expect("a second column");
    assert!(second > 10 && body[..second].iter().all(|line| line.left < 297.0));
    assert!(
        body[second].y > body[second - 1].y + 500.0,
        "{:?}",
        body[second]
    );

    // The chart draws its axis labels and its legend.
    let chart = pdf.page_text(page_with(&pdf, "6. Battery performance"));
    for label in [
        "-20 °C",
        "40 °C",
        "Standard model (h)",
        "Extended battery (h)",
    ] {
        assert!(chart.contains(label), "{label:?} in {chart:?}");
    }
}

#[test]
fn the_thesis_numbers_its_pages_and_keeps_its_landscape_pages_figure_notes_and_outline() {
    let doc = corpus_doc("authored/thesis.docx");
    let pdf = pdf_of(&doc);
    let count = pdf.page_count();
    // LibreOffice lays it out on 28 pages.
    assert!((22..=32).contains(&count), "{count} pages");
    let margin = 1417.0 / 20.0;
    // Each page read once: its lines, and its text without white space.
    let pages: Vec<Vec<Line>> = (0..count).map(|page| lines(&pdf.drawn(page))).collect();
    let page_with = |text: &str| {
        let text = squeeze(text);
        pages
            .iter()
            .position(|lines| {
                squeeze(
                    &lines
                        .iter()
                        .map(|line| line.text.as_str())
                        .collect::<String>(),
                )
                .contains(&text)
            })
            .unwrap_or_else(|| panic!("no page draws {text:?}"))
    };

    // The title page has no number, the front matter counts in roman
    // numerals, the chapters from 1 again; odd numbers sit on the right.
    // The odd-page first chapter starts on a right-hand page, after a blank
    // page that draws nothing when the front matter ends on one.
    let footer = |page: usize| pages[page].iter().find(|line| line.y < margin);
    assert!(footer(0).is_none(), "{:?}", footer(0));
    let chapter = page_with("Regional distributors keep stock");
    assert_eq!(chapter % 2, 0, "chapter 1 is on an odd physical page");
    let roman = ["ii", "iii", "iv", "v", "vi", "vii"];
    for page in 1..count {
        if page + 1 == chapter && pages[page].is_empty() {
            continue;
        }
        let (number, value) = match page < chapter {
            true => (roman[page - 1].to_owned(), page + 1),
            false => ((page - chapter + 1).to_string(), page - chapter + 1),
        };
        let line = footer(page).unwrap_or_else(|| panic!("page {page} has no number"));
        assert_eq!(line.text, number, "page {page}");
        assert_eq!(line.left > 300.0, value % 2 == 1, "page {page}: {line:?}");
    }

    // The landscape section is one run of pages, holding the results.
    let landscape: Vec<usize> = (0..count)
        .filter(|page| pdf.page_size(*page).0 > pdf.page_size(*page).1)
        .collect();
    assert!(landscape.len() >= 2, "{landscape:?}");
    assert!(
        landscape.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "{landscape:?}"
    );
    assert!(landscape.contains(&page_with("Each scenario simulates 730 days")));
    for page in (0..count).filter(|page| !landscape.contains(page)) {
        assert_size(pdf.page_size(page), A4, &format!("page {page}"));
    }

    // The figure is drawn above its caption.
    let figures: Vec<(usize, Picture)> = (0..count)
        .flat_map(|page| {
            pdf.pictures(page)
                .into_iter()
                .map(move |picture| (page, picture))
        })
        .collect();
    assert_eq!(figures.len(), 1);
    let (page, figure) = &figures[0];
    let caption = pages[*page]
        .iter()
        .find(|line| line.text.contains("Weekly demand at the central warehouse"))
        .expect("the caption is on the page of the figure");
    assert!(figure.rect[1] > caption.y, "{:?} {caption:?}", figure.rect);

    // Each footnote is drawn in the lower part of a page.
    let footnotes = doc.footnotes();
    assert!(!footnotes.is_empty());
    for (_, note) in footnotes {
        let page = page_with(&note);
        let start: String = squeeze(&note).chars().take(16).collect();
        let line = pages[page]
            .iter()
            .find(|line| squeeze(&line.text).contains(&start))
            .unwrap();
        assert!(line.y < A4.1 / 3.0, "{note:?} on page {page}: {line:?}");
    }

    // The outline lists the headings in order, at their levels.
    let headings: Vec<(String, usize)> = doc
        .paragraphs()
        .iter()
        .filter_map(|paragraph| {
            let level = paragraph
                .style_id()?
                .strip_prefix("Heading")?
                .parse::<usize>()
                .ok()?;
            Some((paragraph.text(), level - 1))
        })
        .collect();
    let outline = pdf.outline();
    assert!(headings.len() > 20, "{headings:?}");
    assert_eq!(outline.len(), headings.len(), "{outline:?}");
    for ((title, depth, _), (heading, level)) in outline.iter().zip(&headings) {
        assert!(
            title.ends_with(heading.as_str()),
            "{title:?} for {heading:?}"
        );
        assert_eq!(depth, level, "{title}");
    }
}

// Fails today: the number of a caption made with a SEQ field is drawn, and
// tagged, before its label, so text extraction and screen readers read
// "1Table : Notation". Issue: #158
#[test]
#[should_panic(expected = "in the tagged order")]
fn a_caption_number_is_read_after_its_label() {
    let doc = doc_with_body(concat!(
        r#"<w:p><w:r><w:t xml:space="preserve">Table </w:t></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        r#"<w:r><w:instrText xml:space="preserve"> SEQ Table \* ARABIC </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
        r#"<w:r><w:t xml:space="preserve">: Notation</w:t></w:r></w:p>"#,
    ));
    let pdf = pdf_of(&doc);
    // The number is drawn where it belongs on the line, after the label.
    let drawn = pdf.drawn(0);
    let label = drawn.iter().find(|piece| piece.text == "Table ").unwrap();
    let number = drawn.iter().find(|piece| piece.text == "1").unwrap();
    assert!(number.x >= label.end - 0.01, "{drawn:?}");
    let elements = pdf.structure();
    let paragraph = elements
        .iter()
        .position(|element| element.0 == "P")
        .unwrap();
    let read = tagged_text(&pdf, &elements, paragraph);
    assert_eq!(
        read, "Table 1: Notation",
        "the caption reads {read:?} in the tagged order"
    );
}
