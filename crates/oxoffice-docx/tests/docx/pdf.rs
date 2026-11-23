//! Converting documents to PDF, read back with a real PDF parser: pages and
//! their sizes, where text lands, headers, footers, notes, tables, pictures,
//! fonts, links, outline, tags, PDF/A, metadata, determinism and the corpus.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use lopdf::Object;
use oxoffice_docx::paragraph::Alignment;
use oxoffice_docx::{Document, Length, PdfConformance, PictureOptions, RenderOptions, StoryKind};
use oxoffice_render_test::fingerprint::Pixels;
use oxoffice_render_test::pdf::{Drawn, Element, Pdf};

use crate::support::{corpus_root, doc_with, doc_with_body, reopen};

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

// ---- Pages ----

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

// ---- Headers, footers and notes ----

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

// ---- Pictures ----

/// A paragraph that starts with a text box it must clear, Word's "top and
/// bottom" wrapping, anchored at the paragraph's top.
fn paragraph_below_its_text_box(text: &str) -> String {
    format!(
        concat!(
            r#"<w:p><w:r><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" "#,
            r#"relativeHeight="1" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1">"#,
            r#"<wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="column"><wp:posOffset>0</wp:posOffset></wp:positionH>"#,
            r#"<wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV>"#,
            r#"<wp:extent cx="1828800" cy="1371600"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapTopAndBottom/>"#,
            r#"<wp:docPr id="7" name="Text Box"/><wp:cNvGraphicFramePr/>"#,
            r#"<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">"#,
            r#"<a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape">"#,
            r#"<wps:wsp xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:cNvSpPr txBox="1"/>"#,
            r#"<wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1828800" cy="1371600"/></a:xfrm>"#,
            r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></wps:spPr><wps:txbx><w:txbxContent>"#,
            r#"<w:p><w:r><w:t>Boxed</w:t></w:r></w:p></w:txbxContent></wps:txbx><wps:bodyPr/></wps:wsp>"#,
            r#"</a:graphicData></a:graphic></wp:anchor></w:drawing></w:r><w:r><w:t>{}</w:t></w:r></w:p>"#,
        ),
        text
    )
}

// The text box stays with the first part of a paragraph split across pages;
// the rest starts at the top of the next page, where the cursor is, so the
// paragraph after it is drawn below it, not over it. The newsletter's
// three-column pages showed it.
#[test]
fn the_rest_of_a_paragraph_below_a_text_box_starts_the_next_page() {
    let fillers = (1..=20)
        .map(|number| format!("<w:p><w:r><w:t>Filler {number}</w:t></w:r></w:p>"))
        .collect::<String>();
    let words = (1..=160)
        .map(|number| format!("word{number}"))
        .collect::<Vec<_>>()
        .join(" ");
    let body = format!(
        "{fillers}{}<w:p><w:r><w:t>After</w:t></w:r></w:p>",
        paragraph_below_its_text_box(&words)
    );
    let pdf = pdf_of(&doc_with_body(&body));
    assert_eq!(
        page_with(&pdf, "word1 "),
        0,
        "the paragraph starts on page 1"
    );
    assert_eq!(page_with(&pdf, "word160"), 1, "and ends on page 2");
    let rest = lines(&pdf.drawn(1));
    let after = line_of(&pdf, 1, "After");
    for line in rest.iter().filter(|line| line.text.contains("word")) {
        assert!(
            line.y > after.y + 1.0,
            "{:?} at {:.1} is not above the next paragraph at {:.1}",
            line.text,
            line.y,
            after.y
        );
    }
}

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

/// A document whose first paragraph links to bookmark `target`, and whose
/// third page starts with a "Results" heading bookmarked `results`.
fn doc_linking_to(target: &str) -> Document {
    let mut doc = Document::new();
    doc.add_paragraph("See ")
        .add_internal_hyperlink("the results", target, None);
    doc.add_paragraph("Method").page_break_before(true);
    doc.add_paragraph("Results")
        .style("Heading1")
        .page_break_before(true);
    let index = doc.content_count() - 1;
    let range = oxoffice_docx::RunRange {
        start: oxoffice_docx::RunPosition {
            body_index: index,
            run_index: 0,
        },
        end: oxoffice_docx::RunPosition {
            body_index: index,
            run_index: 1,
        },
    };
    doc.add_bookmark("results", range).unwrap();
    doc
}

#[test]
fn an_internal_hyperlink_goes_to_the_top_of_its_bookmark() {
    let doc = reopen(&mut doc_linking_to("results"));
    // One page's SVG has nowhere to land, so it draws no dead anchor.
    let svg = doc
        .render_page_to_svg_deterministic(0)
        .unwrap()
        .unwrap()
        .svg;
    assert!(!svg.contains("href=\"#"), "{svg}");
    // The bookmark's page draws its SVG with nothing left out.
    let bookmarked = doc.render_page_to_svg_deterministic(2).unwrap().unwrap();
    assert!(
        bookmarked
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("omitted")),
        "{:?}",
        bookmarked.diagnostics
    );
    let pdf = pdf_of(&doc);
    assert!(pdf.links(0).is_empty(), "no URI link");
    let links = pdf.internal_links(0);
    assert!(!links.is_empty());
    let results = page_with(&pdf, "Results");
    assert_eq!(results, 2);
    let heading = line_of(&pdf, results, "Results");
    for (page, top, _) in &links {
        assert_eq!(*page, results, "{links:?}");
        assert!(
            *top > heading.y && *top < heading.y + 30.0,
            "the destination {top} is the top of the heading's line at {}",
            heading.y
        );
    }
    let covered: String = pdf
        .drawn(0)
        .into_iter()
        .filter(|piece| {
            links.iter().any(|(_, _, [left, bottom, right, top])| {
                piece.x >= left - 0.01
                    && piece.end <= right + 0.01
                    && piece.y > *bottom
                    && piece.y < *top
            })
        })
        .map(|piece| piece.text)
        .collect();
    assert_eq!(covered, "the results");
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

// ---- Fonts and scripts ----

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

/// Whether `text` holds Chinese, Japanese or Korean characters.
fn has_cjk(text: &str) -> bool {
    text.chars().any(|ch| {
        matches!(ch, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{9fff}' | '\u{ac00}'..='\u{d7af}')
    })
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
fn a_dark_highlight_paints_words_palette() {
    // Word's dark highlights are half intensity, not the CSS colours of the
    // same names (dark blue is 000080, not 00008B), and its greys are
    // 808080 and C0C0C0.
    let run = |value: &str| {
        format!(
            r#"<w:p><w:r><w:rPr><w:highlight w:val="{value}"/></w:rPr><w:t>{value}</w:t></w:r></w:p>"#
        )
    };
    let doc = doc_with_body(&[run("darkBlue"), run("darkGray"), run("lightGray")].concat());
    assert!(pixels_in_text_box(&doc, "darkBlue", [0x00, 0x00, 0x80]) > 20);
    assert!(pixels_in_text_box(&doc, "darkGray", [0x80, 0x80, 0x80]) > 20);
    assert!(pixels_in_text_box(&doc, "lightGray", [0xC0, 0xC0, 0xC0]) > 20);
}

#[test]
fn a_colour_value_that_is_not_hex_digits_paints_nothing_and_never_stops_the_render() {
    // A shading or a text colour read from the file unchecked, with a
    // character of more than one byte where a digit belongs.
    let doc = doc_with_body(concat!(
        r#"<w:p><w:r><w:rPr><w:shd w:val="clear" w:fill="Aé0000"/></w:rPr><w:t>Fill</w:t></w:r></w:p>"#,
        r#"<w:p><w:r><w:rPr><w:color w:val="Aé0000"/></w:rPr><w:t>Colour</w:t></w:r></w:p>"#,
        r#"<w:p><w:r><w:rPr><w:shd w:val="pct25" w:color="00FF00;x" w:fill="FF0000"/></w:rPr><w:t>Pattern</w:t></w:r></w:p>"#,
    ));
    // The page stays white behind the fill that is not a colour.
    assert_eq!(pixels_in_text_box(&doc, "Fill", [0xA0, 0x00, 0x00]), 0);
    assert!(pixels_in_text_box(&doc, "Fill", [0xFF, 0xFF, 0xFF]) > 20);
    // A foreground that is not a colour is automatic: black under a
    // pattern, so 25 % of it over red is BF0000 (not BF4000, a quarter of
    // the green the value starts with).
    assert!(pixels_in_text_box(&doc, "Pattern", [0xBF, 0x00, 0x00]) > 20);
    let html = doc.to_html_fragment();
    assert!(!html.contains("background-color:#A0"), "{html}");
}

// ---- Corpus ----

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
