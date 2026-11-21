//! Conversions to and from the web formats: `to_html`, `to_html_fragment` and
//! `to_markdown` with their diagnostics twins, and the `from_html` and
//! `from_markdown` readers with their template and image forms. Outputs are
//! read back the way a browser or a CommonMark reader would read them.

use std::collections::HashMap;
use std::io::Cursor;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use oxoffice_docx::paragraph::ParagraphRef;
use oxoffice_docx::run::RunRef;
use oxoffice_docx::{
    BreakKind, CT_PPr, ChartData, ChartKind, Document, HtmlImageResource, Length, PictureOptions,
    StoryKind, StyleBuilder,
};
use oxoffice_docx_oxml::table::VMerge;
use oxoffice_opc::OpcPackage;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use scraper::{ElementRef, Html, Selector};

use crate::support::{corpus_doc, doc_with, doc_with_body, reopen, texts};

/// A one-pixel PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

/// One block as a CommonMark reader with GFM tables and strikethrough reads
/// it: the text of its inlines, delimiters and escapes resolved, raw HTML kept
/// as written.
#[derive(Debug, Clone, PartialEq)]
enum Read {
    Heading(usize, String),
    Paragraph(String),
    /// A list item whose text is not wrapped in a paragraph.
    Item(String),
    Code(String),
    Rule,
    Table(Vec<Vec<String>>),
}

/// The blocks a CommonMark reader reads from `markdown`, in order. Block
/// quotes are transparent: their paragraphs are listed.
fn read_markdown(markdown: &str) -> Vec<Read> {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let mut blocks = Vec::new();
    let mut text = String::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Text(t) | Event::Code(t) | Event::InlineHtml(t) | Event::Html(t) => {
                text.push_str(&t);
            }
            Event::SoftBreak | Event::HardBreak => text.push('\n'),
            Event::Rule => blocks.push(Read::Rule),
            Event::Start(Tag::TableHead | Tag::TableRow) => rows.push(Vec::new()),
            Event::End(TagEnd::TableCell) => rows
                .last_mut()
                .expect("a cell is in a row")
                .push(std::mem::take(&mut text)),
            Event::End(TagEnd::Table) => blocks.push(Read::Table(std::mem::take(&mut rows))),
            Event::End(TagEnd::Heading(level)) => {
                blocks.push(Read::Heading(level as usize, std::mem::take(&mut text)));
            }
            Event::End(TagEnd::Paragraph) => {
                blocks.push(Read::Paragraph(std::mem::take(&mut text)));
            }
            Event::End(TagEnd::Item) if !text.is_empty() => {
                blocks.push(Read::Item(std::mem::take(&mut text)));
            }
            Event::End(TagEnd::CodeBlock) => blocks.push(Read::Code(std::mem::take(&mut text))),
            _ => {}
        }
    }
    blocks
}

/// The marks a CommonMark reader puts on a stretch of text.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Marks {
    strong: bool,
    emphasis: bool,
    strike: bool,
}

/// The texts a CommonMark reader reads from `markdown`, each with its marks,
/// neighbours with the same marks joined.
fn markdown_marks(markdown: &str) -> Vec<(String, Marks)> {
    let mut marks = Marks::default();
    let mut texts: Vec<(String, Marks)> = Vec::new();
    for event in Parser::new_ext(markdown, Options::ENABLE_STRIKETHROUGH) {
        match event {
            Event::Start(Tag::Strong) => marks.strong = true,
            Event::End(TagEnd::Strong) => marks.strong = false,
            Event::Start(Tag::Emphasis) => marks.emphasis = true,
            Event::End(TagEnd::Emphasis) => marks.emphasis = false,
            Event::Start(Tag::Strikethrough) => marks.strike = true,
            Event::End(TagEnd::Strikethrough) => marks.strike = false,
            Event::Text(text) => match texts.last_mut() {
                Some((last, last_marks)) if *last_marks == marks => last.push_str(&text),
                _ => texts.push((text.to_string(), marks)),
            },
            _ => {}
        }
    }
    texts
}

/// The links and images a CommonMark reader reads from `markdown`, each as
/// `(is_image, text or alt text, destination)`.
fn markdown_targets(markdown: &str) -> Vec<(bool, String, String)> {
    let mut targets = Vec::new();
    let mut open: Option<(bool, String, String)> = None;
    for event in Parser::new_ext(markdown, Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::Link { dest_url, .. }) => {
                open = Some((false, String::new(), dest_url.to_string()));
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                open = Some((true, String::new(), dest_url.to_string()));
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some((_, text, _)) = open.as_mut() {
                    text.push_str(&t);
                }
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                targets.push(open.take().expect("a link or image is open"));
            }
            _ => {}
        }
    }
    targets
}

/// The text of an element as a reader sees it: its lines trimmed, the blank
/// ones between its blocks left out.
fn element_text(element: ElementRef<'_>) -> String {
    element
        .text()
        .collect::<String>()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn attributes(element: ElementRef<'_>) -> HashMap<String, String> {
    element
        .value()
        .attrs()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

/// The elements `selector` picks in `html`, each as its text and its
/// attributes, as a browser parses them.
fn html_elements(html: &str, selector: &str) -> Vec<(String, HashMap<String, String>)> {
    let dom = Html::parse_fragment(html);
    let selector = Selector::parse(selector).expect("a valid selector");
    dom.select(&selector)
        .map(|element| (element_text(element), attributes(element)))
        .collect()
}

/// The texts of the elements `selector` picks in `html`.
fn html_texts(html: &str, selector: &str) -> Vec<String> {
    html_elements(html, selector)
        .into_iter()
        .map(|(text, _)| text)
        .collect()
}

/// The cell texts of each row of the first top-level table of `html`, as a
/// browser parses them.
fn html_table_rows(html: &str) -> Vec<Vec<(String, HashMap<String, String>)>> {
    let dom = Html::parse_fragment(html);
    let table = dom
        .select(&Selector::parse("table").unwrap())
        .next()
        .expect("the HTML holds a table");
    let rows = Selector::parse(":scope > tbody > tr, :scope > tr").unwrap();
    let cells = Selector::parse(":scope > td, :scope > th").unwrap();
    table
        .select(&rows)
        .map(|row| {
            row.select(&cells)
                .map(|cell| (element_text(cell), attributes(cell)))
                .collect()
        })
        .collect()
}

/// The first run of `paragraph` whose text is `text`.
fn run_with_text<'a>(paragraph: &'a ParagraphRef<'a>, text: &str) -> RunRef<'a> {
    paragraph
        .runs()
        .find(|run| run.text() == text)
        .unwrap_or_else(|| panic!("a run reads {text:?}"))
}

/// A 3x3 table whose cells read `r{row}{column}`, in a new document.
fn three_by_three(doc: &mut Document) -> oxoffice_docx::Table<'_> {
    let mut table = doc.add_table(3, 3);
    for row in 0..3 {
        for column in 0..3 {
            table
                .cell(row, column)
                .unwrap()
                .set_text(&format!("r{row}{column}"));
        }
    }
    table
}

/// Picture options of a 10 pt square with this alt text.
fn picture_options(description: Option<&str>) -> PictureOptions {
    PictureOptions {
        width: Length::pt(10.0),
        height: Length::pt(10.0),
        crop: None,
        anchor: None,
        name: None,
        description: description.map(str::to_owned),
        title: None,
        decorative: false,
    }
}

fn body_story(doc: &Document) -> oxoffice_docx::StoryId {
    doc.stories()
        .expect("stories are listed")
        .into_iter()
        .find(|story| story.kind() == StoryKind::Body)
        .expect("the document has a body")
}

// ---- Exports: page, fragment and Markdown ----

#[test]
fn the_html_page_wraps_the_fragment_and_each_diagnostics_twin_returns_the_same_text() {
    let doc = reopen(&mut doc_with(&["First", "Second"]));
    let fragment = doc.to_html_fragment();
    assert_eq!(fragment, "<p>First</p>\n<p>Second</p>\n");
    let page = doc.to_html();
    assert!(
        page.starts_with("<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"UTF-8\">"),
        "{page}"
    );
    assert!(
        page.ends_with(&format!("<body>\n{fragment}\n</body>\n</html>")),
        "{page}"
    );
    assert_eq!(doc.to_markdown(), "First\n\nSecond\n\n");
    assert_eq!(doc.to_html_with_diagnostics(), (page, Vec::new()));
    assert_eq!(
        doc.to_html_fragment_with_diagnostics(),
        (fragment, Vec::new())
    );
    assert_eq!(
        doc.to_markdown_with_diagnostics(),
        (doc.to_markdown(), Vec::new())
    );

    let empty = reopen(&mut Document::new());
    assert_eq!(empty.to_html_fragment(), "");
    assert_eq!(empty.to_markdown(), "");
    assert!(
        empty.to_html().ends_with("<body>\n\n</body>\n</html>"),
        "{}",
        empty.to_html()
    );
}

#[test]
fn headings_export_at_the_level_their_style_or_outline_level_gives() {
    let mut body = String::new();
    for level in 1..=6 {
        body.push_str(&format!(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading{level}"/></w:pPr><w:r><w:t>Level {level}</w:t></w:r></w:p>"#
        ));
    }
    body.push_str(
        r#"<w:p><w:pPr><w:outlineLvl w:val="2"/></w:pPr><w:r><w:t>Outline three</w:t></w:r></w:p>"#,
    );
    body.push_str(r#"<w:p><w:r><w:t>Body</w:t></w:r></w:p>"#);
    let doc = reopen(&mut doc_with_body(&body));

    let html = doc.to_html_fragment();
    let mut expected = String::new();
    for level in 1..=6 {
        expected.push_str(&format!("<h{level}>Level {level}</h{level}>\n"));
    }
    expected.push_str("<h3>Outline three</h3>\n<p>Body</p>\n");
    assert_eq!(html, expected);

    let mut expected: Vec<Read> = (1..=6)
        .map(|level| Read::Heading(level, format!("Level {level}")))
        .collect();
    expected.push(Read::Heading(3, "Outline three".to_owned()));
    expected.push(Read::Paragraph("Body".to_owned()));
    assert_eq!(read_markdown(&doc.to_markdown()), expected);
}

#[test]
fn a_paragraph_at_the_body_text_outline_level_is_not_a_markdown_heading() {
    // The French Word report names its TOC Heading style, outline level 9,
    // "En-ttedetabledesmatires".
    let mut doc = corpus_doc("word/report.docx");
    doc.insert_paragraph(0, "Contents")
        .style("En-ttedetabledesmatires");
    let doc = reopen(&mut doc);
    assert!(
        doc.headings().iter().all(|(_, text)| text != "Contents"),
        "headings() leaves the TOC title out"
    );
    assert!(
        doc.to_html_fragment().starts_with("<p>Contents</p>\n"),
        "HTML writes the TOC title as a paragraph"
    );
    let markdown = doc.to_markdown();
    assert_eq!(
        read_markdown(&markdown)[0],
        Read::Paragraph("Contents".to_owned()),
        "a body-text paragraph is written as a Markdown heading: {:?}",
        markdown.lines().next()
    );
}

#[test]
fn inline_formats_become_html_elements_and_markdown_spells_bold_italic_and_strike() {
    let mut doc = Document::new();
    {
        let mut paragraph = doc.add_paragraph("");
        paragraph.add_run("bold").bold(true);
        paragraph.add_run(" ");
        paragraph.add_run("italic").italic(true);
        paragraph.add_run(" ");
        paragraph.add_run("both").bold(true).italic(true);
        paragraph.add_run(" ");
        paragraph.add_run("under").underline(true);
        paragraph.add_run(" ");
        paragraph.add_run("strike").strike(true);
        paragraph.add_run(" x");
        paragraph.add_run("2").superscript();
        paragraph.add_run(" H");
        paragraph.add_run("2").subscript();
        paragraph.add_run("O ");
        paragraph.add_run("red").color("FF0000");
    }
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.to_html_fragment(),
        "<p><strong>bold</strong> <em>italic</em> <strong><em>both</em></strong> <u>under</u> \
         <s>strike</s> x<sup>2</sup> H<sub>2</sub>O <span style=\"color:#FF0000\">red</span></p>\n"
    );
    // Markdown spells bold, italic and strikethrough; the other formats are
    // written as plain text.
    let markdown = doc.to_markdown();
    assert_eq!(
        read_markdown(&markdown),
        [Read::Paragraph(
            "bold italic both under strike x2 H2O red".to_owned()
        )]
    );
    // A space between two marked runs may take either side's marks.
    let marked: Vec<(String, Marks)> = markdown_marks(&markdown)
        .into_iter()
        .filter(|(text, _)| !text.trim().is_empty())
        .map(|(text, marks)| (text.trim().to_owned(), marks))
        .collect();
    let plain = Marks::default();
    assert_eq!(
        marked,
        [
            (
                "bold",
                Marks {
                    strong: true,
                    ..plain
                }
            ),
            (
                "italic",
                Marks {
                    emphasis: true,
                    ..plain
                }
            ),
            (
                "both",
                Marks {
                    strong: true,
                    emphasis: true,
                    ..plain
                }
            ),
            ("under", plain),
            (
                "strike",
                Marks {
                    strike: true,
                    ..plain
                }
            ),
            ("x2 H2O red", plain),
        ]
        .map(|(text, marks)| (text.to_owned(), marks)),
        "{markdown}"
    );
}

#[test]
fn text_whose_underline_is_turned_off_is_not_underlined_in_html() {
    let mut doc = Document::new();
    doc.add_paragraph("").add_run("plain").underline(false);
    let doc = reopen(&mut doc);
    let paragraph = doc.paragraph(0).unwrap();
    assert!(!run_with_text(&paragraph, "plain").is_underline());
    let underlined = html_texts(&doc.to_html_fragment(), "u");
    assert!(
        underlined.is_empty(),
        "{underlined:?} is underlined in HTML although its underline is off"
    );

    // Google Docs writes w:u w:val="none" on the runs of its list items.
    let google = corpus_doc("google/report.docx");
    let item = "Subscription revenue reached 4.2 million euros, up from 3.7 million.";
    let paragraph = google
        .paragraphs()
        .into_iter()
        .find(|paragraph| paragraph.text() == item)
        .expect("the Google report holds the item");
    assert!(!run_with_text(&paragraph, item).is_underline());
    let underlined = html_texts(&google.to_html_fragment(), "u");
    assert!(
        !underlined.iter().any(|text| text == item),
        "the Google item is underlined in HTML although its underline is off"
    );
}

// ---- Escaping and sanitizing ----

#[test]
fn html_escapes_markup_so_a_browser_reads_back_the_text() {
    let lines = [
        "a < b && c > d",
        "\"double\" and 'single' quotes",
        "<script>alert(1)</script>",
        "&copy; is written as text",
    ];
    let doc = reopen(&mut doc_with(&lines));
    let html = doc.to_html_fragment();
    assert_eq!(html_texts(&html, "p"), lines);
    assert!(html_elements(&html, "script").is_empty(), "{html}");
    assert!(
        html.contains("<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>"),
        "{html}"
    );
}

#[test]
fn markdown_escapes_specials_so_a_commonmark_reader_reads_back_the_text() {
    let lines = [
        "*not emphasis* and _not either_",
        "a < b and `not code`",
        "[not a link](https://example.com)",
        "~~not struck~~ and a back\\slash",
        "# not a heading",
        "1. not a list",
        "- not a bullet",
        "+ not a bullet either",
        "> not a quote",
        "AT&T, 5 * 3 and a | b",
    ];
    let doc = reopen(&mut doc_with(&lines));
    let markdown = doc.to_markdown();
    let expected: Vec<Read> = lines
        .iter()
        .map(|line| Read::Paragraph((*line).to_owned()))
        .collect();
    assert_eq!(read_markdown(&markdown), expected, "{markdown}");
}

#[test]
fn markdown_escapes_an_entity_reference_written_as_text() {
    let line = "&copy; 2026, &#42; and &amp;";
    let doc = reopen(&mut doc_with(&[line]));
    let markdown = doc.to_markdown();
    assert_eq!(
        read_markdown(&markdown),
        [Read::Paragraph(line.to_owned())],
        "a CommonMark reader reads an entity reference written as text as another character: \
         {markdown:?}"
    );
    // An `&` that starts no reference is left as it is, at the end of a
    // paragraph too.
    let plain = reopen(&mut doc_with(&["R&D & sales", "Head of R&D"])).to_markdown();
    assert_eq!(plain.trim_end(), "R&D & sales\n\nHead of R&D");
}

#[test]
fn a_paragraph_starting_with_a_tab_stays_a_paragraph_in_markdown() {
    let doc = reopen(&mut doc_with(&[
        "\tIndented by a tab",
        "    Indented by four spaces",
        "Plain",
    ]));
    assert_eq!(texts(&doc)[0], "\tIndented by a tab");
    let markdown = doc.to_markdown();
    let read = read_markdown(&markdown);
    let paragraphs: Vec<String> = read
        .iter()
        .filter_map(|block| match block {
            Read::Paragraph(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        paragraphs,
        [
            "\tIndented by a tab",
            "    Indented by four spaces",
            "Plain"
        ],
        "a paragraph that starts with a tab is read as a code block: {read:?}"
    );
}

#[test]
fn markdown_escapes_a_character_reference_in_split_runs_alt_text_and_links() {
    // Word splits runs around a word it flags: "&", "nbsp" and ";" in runs
    // of their own still make a reference once written one after another.
    let doc = reopen(&mut doc_with_body(
        r#"<w:p><w:r><w:t>&amp;</w:t></w:r><w:r><w:t>nbsp</w:t></w:r><w:r><w:t>;</w:t></w:r></w:p>"#,
    ));
    assert_eq!(
        read_markdown(&doc.to_markdown()),
        [Read::Paragraph("&nbsp;".to_owned())]
    );

    let mut doc = Document::new();
    let link = doc.add_hyperlink_relationship("https://example.com/?a=1&amp;b=2");
    doc.add_paragraph("See ")
        .add_hyperlink("&copy; terms", &link);
    let body = body_story(&doc);
    doc.add_picture_with_options(&body, PNG, "dot.png", picture_options(Some("&copy; logo")))
        .unwrap();
    let doc = reopen(&mut doc);
    let targets = markdown_targets(&doc.to_markdown());
    assert_eq!(
        targets[0],
        (
            false,
            "&copy; terms".to_owned(),
            "https://example.com/?a=1&amp;b=2".to_owned()
        )
    );
    assert_eq!(targets[1].1, "&copy; logo");
}

#[test]
fn two_line_breaks_in_a_row_stay_one_paragraph_in_markdown() {
    let mut doc = Document::new();
    {
        let mut paragraph = doc.add_paragraph("");
        let mut run = paragraph.add_run("a");
        run.add_break(BreakKind::Line);
        run.add_break(BreakKind::Line);
        run.add_text("b");
    }
    let doc = reopen(&mut doc);
    let read = read_markdown(&doc.to_markdown());
    assert_eq!(read, [Read::Paragraph("a\n\nb".to_owned())]);
}

#[test]
fn a_body_text_paragraph_takes_no_numbering_from_its_style_in_markdown() {
    // The French Word thesis numbers its headings through Titre1, on which
    // its TOC Heading style, outline level 9, is based.
    let mut doc = corpus_doc("word/thesis.docx");
    doc.insert_paragraph(0, "Contents")
        .style("En-ttedetabledesmatires");
    let doc = reopen(&mut doc);
    let html = doc.to_html_fragment();
    assert!(html.starts_with("<p>Contents</p>\n"), "{html:.80}");
    let markdown = doc.to_markdown();
    assert_eq!(
        read_markdown(&markdown)[0],
        Read::Paragraph("Contents".to_owned()),
        "{:?}",
        markdown.lines().next()
    );
}

#[test]
fn a_described_picture_has_one_alt_attribute_in_epub() {
    let mut doc = Document::new();
    let body = body_story(&doc);
    doc.add_picture_with_options(&body, PNG, "dot.png", picture_options(Some("A red dot")))
        .unwrap();
    let body = body_story(&doc);
    doc.add_picture_with_options(&body, PNG, "plain.png", picture_options(Some("")))
        .unwrap();
    let doc = reopen(&mut doc);
    let epub = doc.to_epub_bytes().unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(epub.bytes)).unwrap();
    let mut images = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().unwrap().into_owned();
        if !name.ends_with(".xhtml") {
            continue;
        }
        let mut xhtml = String::new();
        std::io::Read::read_to_string(&mut entry, &mut xhtml).unwrap();
        images.extend(xhtml.match_indices("<img").map(|(at, _)| {
            let end = xhtml[at..].find('>').unwrap() + at;
            xhtml[at..=end].to_owned()
        }));
    }
    assert_eq!(images.len(), 2, "{images:?}");
    for (image, alt) in images.iter().zip(["alt=\"A red dot\"", "alt=\"\""]) {
        assert_eq!(image.matches(" alt=").count(), 1, "{image}");
        assert!(image.contains(alt), "{image}");
        assert!(image.ends_with(" style=\"max-width:100%\"/>"), "{image}");
    }
}

#[test]
fn a_paragraph_right_to_left_through_its_style_chain_keeps_its_direction_in_html() {
    let mut doc = Document::new();
    let right_to_left = CT_PPr {
        bidi: Some(true),
        ..CT_PPr::default()
    };
    doc.add_style(StyleBuilder::paragraph("Arabic", "Arabic").paragraph_properties(right_to_left))
        .unwrap();
    doc.add_style(StyleBuilder::paragraph("ArabicQuote", "Arabic quote").based_on("Arabic"))
        .unwrap();
    doc.add_paragraph("نص").style("ArabicQuote");
    // A direct `w:bidi w:val="0"` wins over the style.
    doc.add_paragraph("text")
        .style("Arabic")
        .right_to_left(false);
    let doc = reopen(&mut doc);
    let html = doc.to_html_fragment();
    assert!(html.contains("<p dir=\"rtl\">نص</p>"), "{html}");
    assert!(html.contains("<p>text</p>"), "{html}");

    // A paragraph without a style follows the default paragraph style.
    let mut doc = Document::new();
    doc.add_paragraph("نص");
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let styles = String::from_utf8(package.get_part("/word/styles.xml").unwrap().to_vec()).unwrap();
    let start = styles
        .find("w:styleId=\"Normal\"")
        .expect("the template has Normal");
    let end = styles[start..].find("</w:style>").unwrap() + start;
    let normal = &styles[start..end];
    let edited = match normal.find("<w:rPr>") {
        Some(at) => format!("{}<w:pPr><w:bidi/></w:pPr>{}", &normal[..at], &normal[at..]),
        None => format!("{normal}<w:pPr><w:bidi/></w:pPr>"),
    };
    assert!(!normal.contains("<w:pPr>"), "{normal}");
    package.set_part(
        "/word/styles.xml",
        format!("{}{edited}{}", &styles[..start], &styles[end..]).into_bytes(),
    );
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let doc = Document::from_bytes(&bytes.into_inner()).unwrap();
    let html = doc.to_html_fragment();
    assert!(html.contains("<p dir=\"rtl\">نص</p>"), "{html}");

    // `start` follows the direction rather than meaning left.
    let doc = reopen(&mut doc_with_body(
        r#"<w:p><w:pPr><w:bidi/><w:jc w:val="start"/></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#,
    ));
    let html = doc.to_html_fragment();
    assert!(
        html.contains("<p dir=\"rtl\" style=\"text-align:start\">x</p>"),
        "{html}"
    );
}

#[test]
fn links_keep_their_url_and_an_unsafe_scheme_keeps_only_its_text() {
    let mut doc = Document::new();
    let query = doc.add_hyperlink_relationship("https://example.com/a?b=c&d=e");
    doc.add_paragraph("See ").add_hyperlink("the query", &query);
    let parenthesis = doc.add_hyperlink_relationship("https://example.com/a_(b)");
    doc.add_paragraph("See ")
        .add_hyperlink("the parenthesis", &parenthesis);
    let mail = doc.add_hyperlink_relationship("mailto:team@example.com");
    doc.add_paragraph("Write to ")
        .add_hyperlink("the team", &mail);
    let script = doc.add_hyperlink_relationship("javascript:alert(1)");
    doc.add_paragraph("Do not ")
        .add_hyperlink("run this", &script);
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.hyperlink_url(&script).as_deref(),
        Some("javascript:alert(1)")
    );

    let html = doc.to_html_fragment();
    let links: Vec<(String, Option<String>)> = html_elements(&html, "a")
        .into_iter()
        .map(|(text, attributes)| (text, attributes.get("href").cloned()))
        .collect();
    let expected = [
        ("the query", "https://example.com/a?b=c&d=e"),
        ("the parenthesis", "https://example.com/a_(b)"),
        ("the team", "mailto:team@example.com"),
    ];
    assert_eq!(
        links,
        expected.map(|(text, url)| (text.to_owned(), Some(url.to_owned())))
    );
    assert_eq!(html_texts(&html, "p")[3], "Do not run this");
    assert!(!html.contains("javascript"), "{html}");

    let markdown = doc.to_markdown();
    assert_eq!(
        markdown_targets(&markdown),
        expected.map(|(text, url)| (false, text.to_owned(), url.to_owned()))
    );
    assert_eq!(
        read_markdown(&markdown)[3],
        Read::Paragraph("Do not run this".to_owned())
    );
    assert!(!markdown.contains("javascript"), "{markdown}");
}

#[test]
fn values_from_the_file_cannot_inject_markup_into_html_attributes() {
    let doc = reopen(&mut doc_with_body(concat!(
        r#"<w:p><w:pPr><w:shd w:val="clear" w:fill="00FF00;background:url(x)"/></w:pPr>"#,
        r#"<w:r><w:rPr><w:rFonts w:ascii="Arial&quot; onmouseover=&quot;alert(1)"/>"#,
        r#"<w:color w:val="red;} body{display:none}"/>"#,
        r#"<w:shd w:val="clear" w:fill="FF0000&quot; onclick=&quot;steal()"/></w:rPr><w:t>hostile</w:t></w:r>"#,
        r#"<w:r><w:rPr><w:rFonts w:ascii="Arial"/><w:color w:val="FF0000"/>"#,
        r#"<w:shd w:val="clear" w:fill="FFFF00"/></w:rPr><w:t>safe</w:t></w:r></w:p>"#,
    )));
    // The hostile values reach the document model...
    let paragraph = doc.paragraph(0).unwrap();
    let hostile = run_with_text(&paragraph, "hostile");
    assert!(hostile.font_name().unwrap().contains("onmouseover"));
    assert!(hostile.color().unwrap().contains("display:none"));
    assert!(hostile.shading_fill().unwrap().contains("onclick"));
    let html = doc.to_html_fragment();
    // ...the well-formed values are written...
    assert!(
        html.contains(
            "<span style=\"font-family:&#39;Arial&#39;;color:#FF0000;background-color:#FFFF00\">safe</span>"
        ),
        "{html}"
    );
    // ...and the hostile ones add no attribute and no declaration.
    for (text, attributes) in html_elements(&html, "*") {
        for (name, value) in &attributes {
            assert!(
                name == "style" && !value.contains("url(") && !value.contains("display"),
                "{text:?} carries {name}={value:?}"
            );
        }
    }
    assert_eq!(html_texts(&html, "p"), ["hostilesafe"]);

    // A single-quoted attribute hands a literal quote to the reader (and no
    // other character the font check refuses); a save writes it as &quot;,
    // which reads back as the same quote.
    let mut quoted = doc_with_body(concat!(
        r#"<w:p><w:r><w:rPr><w:rFonts w:ascii='Arial" onmouseover="steal'/></w:rPr>"#,
        r#"<w:t>quoted</w:t></w:r></w:p>"#,
    ));
    let paragraph = quoted.paragraph(0).unwrap();
    assert_eq!(
        run_with_text(&paragraph, "quoted").font_name(),
        Some("Arial\" onmouseover=\"steal")
    );
    let html = quoted.to_html_fragment();
    for (text, attributes) in html_elements(&html, "*") {
        for name in attributes.keys() {
            assert_eq!(name, "style", "{text:?} carries {name}: {html}");
        }
    }
    assert_eq!(html_texts(&html, "p"), ["quoted"]);
    let quoted = reopen(&mut quoted);
    let paragraph = quoted.paragraph(0).unwrap();
    assert_eq!(
        run_with_text(&paragraph, "quoted").font_name(),
        Some("Arial\" onmouseover=\"steal")
    );
}

#[test]
fn tracked_changes_and_content_controls_export_as_what_the_text_readers_read() {
    let tracked = concat!(
        r#"<w:p><w:r><w:t xml:space="preserve">Keep </w:t></w:r>"#,
        r#"<w:ins w:id="1" w:author="Ada"><w:r><w:t>added</w:t></w:r></w:ins>"#,
        r#"<w:del w:id="2" w:author="Ada"><w:r><w:delText>removed</w:delText></w:r></w:del></w:p>"#,
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid>"#,
        r#"<w:tr><w:tc><w:p><w:r><w:t>KEPT</w:t></w:r></w:p></w:tc></w:tr>"#,
        r#"<w:tr><w:trPr><w:del w:id="3" w:author="Ada"/></w:trPr><w:tc><w:p><w:r><w:t>GONE</w:t></w:r></w:p></w:tc></w:tr>"#,
        r#"</w:tbl><w:p><w:r><w:t>End</w:t></w:r></w:p>"#,
    );
    let doc = reopen(&mut doc_with_body(tracked));
    let mut accepted = reopen(&mut doc_with_body(tracked));
    assert_eq!(accepted.accept_all().unwrap(), 3);
    let accepted = reopen(&mut accepted);
    let html = doc.to_html_fragment();
    let markdown = doc.to_markdown();
    assert_eq!(html, accepted.to_html_fragment());
    assert_eq!(markdown, accepted.to_markdown());
    assert!(html.starts_with("<p>Keep added</p>\n"), "{html}");
    assert_eq!(html_texts(&html, "td"), ["KEPT"]);
    for gone in ["removed", "GONE"] {
        assert!(!html.contains(gone) && !markdown.contains(gone), "{gone}");
    }

    // A block, a run and a cell control are written as what they wrap.
    let controlled = doc_with_body(concat!(
        r#"<w:sdt><w:sdtPr><w:alias w:val="Block"/><w:tag w:val="block"/></w:sdtPr><w:sdtContent>"#,
        r#"<w:p><w:r><w:t>In a block control</w:t></w:r></w:p></w:sdtContent></w:sdt>"#,
        r#"<w:p><w:r><w:t xml:space="preserve">Inline </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="run"/></w:sdtPr>"#,
        r#"<w:sdtContent><w:r><w:rPr><w:b/></w:rPr><w:t>control</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:sdt><w:sdtPr><w:tag w:val="cell"/></w:sdtPr>"#,
        r#"<w:sdtContent><w:tc><w:p><w:r><w:t>In a cell control</w:t></w:r></w:p></w:tc></w:sdtContent></w:sdt></w:tr></w:tbl>"#,
    ));
    let plain = doc_with_body(concat!(
        r#"<w:p><w:r><w:t>In a block control</w:t></w:r></w:p>"#,
        r#"<w:p><w:r><w:t xml:space="preserve">Inline </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>control</w:t></w:r></w:p>"#,
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr>"#,
        r#"<w:tc><w:p><w:r><w:t>In a cell control</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
    ));
    let controlled = reopen(&mut { controlled });
    let html = controlled.to_html_fragment();
    assert_eq!(html, reopen(&mut { plain }).to_html_fragment());
    assert!(
        html.starts_with("<p>In a block control</p>\n<p>Inline <strong>control</strong></p>\n"),
        "{html}"
    );
    assert_eq!(
        read_markdown(&controlled.to_markdown()),
        [
            Read::Paragraph("In a block control".to_owned()),
            Read::Paragraph("Inline control".to_owned()),
            Read::Table(vec![vec!["In a cell control".to_owned()]]),
        ]
    );
}

#[test]
fn fields_export_their_cached_results() {
    let mut doc = doc_with_body(concat!(
        r#"<w:p><w:r><w:t xml:space="preserve">Page </w:t></w:r>"#,
        r#"<w:fldSimple w:instr=" PAGE "><w:r><w:t>7</w:t></w:r></w:fldSimple>"#,
        r#"<w:r><w:t xml:space="preserve"> of </w:t></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> NUMPAGES </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>9</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    ));
    doc.add_paragraph("By ")
        .add_run("")
        .add_field("AUTHOR", "Ada Lovelace")
        .unwrap();
    let doc = reopen(&mut doc);
    let html = doc.to_html_fragment();
    assert_eq!(html, "<p>Page 7 of 9</p>\n<p>By Ada Lovelace</p>\n");
    let markdown = doc.to_markdown();
    assert_eq!(
        read_markdown(&markdown),
        [
            Read::Paragraph("Page 7 of 9".to_owned()),
            Read::Paragraph("By Ada Lovelace".to_owned()),
        ]
    );
    for instruction in ["PAGE", "NUMPAGES", "AUTHOR"] {
        assert!(!markdown.contains(instruction), "{markdown}");
    }
}

#[test]
fn line_breaks_tabs_and_page_breaks_keep_their_place() {
    let mut doc = Document::new();
    {
        let mut paragraph = doc.add_paragraph("");
        let mut run = paragraph.add_run("line1");
        run.add_break(BreakKind::Line);
        run.add_text("line2");
        run.add_tab();
        run.add_text("tabbed");
    }
    {
        let mut paragraph = doc.add_paragraph("");
        let mut run = paragraph.add_run("before break");
        run.add_break(BreakKind::Page);
        run.add_text("after break");
    }
    let doc = reopen(&mut doc);

    let html = doc.to_html_fragment();
    assert!(html.starts_with("<p>line1<br>line2"), "{html}");
    let first = &html_texts(&html, "p")[0];
    let between = first
        .strip_prefix("line1line2")
        .and_then(|rest| rest.strip_suffix("tabbed"))
        .unwrap_or_else(|| panic!("{first:?}"));
    assert!(
        !between.is_empty() && between.chars().all(char::is_whitespace),
        "the tab is written as space: {between:?}"
    );
    let before = html
        .find("before break")
        .expect("text before the page break");
    let rule = html.find("<hr").expect("a rule for the page break");
    let after = html.find("after break").expect("text after the page break");
    assert!(before < rule && rule < after, "{html}");

    // A hard line break is two spaces and a new line, a page break a
    // thematic break between blank lines.
    let markdown = doc.to_markdown();
    assert_eq!(
        markdown,
        "line1  \nline2\ttabbed\n\nbefore break\n\n---\n\nafter break\n\n"
    );
    assert_eq!(
        read_markdown(&markdown),
        [
            Read::Paragraph("line1\nline2\ttabbed".to_owned()),
            Read::Paragraph("before break".to_owned()),
            Read::Rule,
            Read::Paragraph("after break".to_owned()),
        ]
    );
}

// ---- Pictures ----

#[test]
fn pictures_are_embedded_as_data_uris_of_their_bytes() {
    let mut doc = doc_with(&["Before"]);
    let body = body_story(&doc);
    doc.add_picture_with_options(&body, PNG, "dot.png", picture_options(Some("A *red* dot")))
        .unwrap();
    doc.add_picture(PNG, "plain.png", Length::pt(10.0), Length::pt(10.0));
    let doc = reopen(&mut doc);
    let pictures = doc.pictures().unwrap();
    assert_eq!(pictures.len(), 2);
    assert!(pictures.iter().all(|picture| picture.bytes == PNG));

    let uri = format!("data:image/png;base64,{}", BASE64.encode(PNG));
    let sources: Vec<Option<String>> = html_elements(&doc.to_html_fragment(), "img")
        .into_iter()
        .map(|(_, attributes)| attributes.get("src").cloned())
        .collect();
    assert_eq!(sources, [Some(uri.clone()), Some(uri.clone())]);

    // Markdown writes the alt text, escaped.
    let markdown = doc.to_markdown();
    assert!(
        markdown.contains(&format!("![A \\*red\\* dot]({uri})")),
        "{markdown}"
    );
    assert_eq!(
        markdown_targets(&markdown),
        [
            (true, "A *red* dot".to_owned(), uri.clone()),
            (true, String::new(), uri),
        ]
    );
}

#[test]
fn a_pictures_alt_text_reaches_the_html() {
    let mut doc = Document::new();
    let body = body_story(&doc);
    doc.add_picture_with_options(&body, PNG, "dot.png", picture_options(Some("A red dot")))
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.pictures().unwrap()[0].description.as_deref(),
        Some("A red dot")
    );
    assert_eq!(markdown_targets(&doc.to_markdown())[0].1, "A red dot");
    let alts: Vec<Option<String>> = html_elements(&doc.to_html_fragment(), "img")
        .into_iter()
        .map(|(_, attributes)| attributes.get("alt").cloned())
        .collect();
    assert_eq!(
        alts,
        [Some("A red dot".to_owned())],
        "the picture's alt text is missing from the HTML"
    );
}

// ---- Tables ----

#[test]
fn merged_cells_become_spans_in_html_and_keep_their_text_in_markdown() {
    let mut doc = Document::new();
    {
        let mut table = three_by_three(&mut doc);
        table.merge_cells((0, 1), (0, 2)).unwrap();
        table.merge_cells((1, 0), (2, 0)).unwrap();
    }
    {
        let mut table = doc.add_table(1, 2);
        table.cell(0, 0).unwrap().set_text("outer");
        table
            .cell(0, 1)
            .unwrap()
            .add_table(1, 1)
            .cell(0, 0)
            .unwrap()
            .set_text("inner");
    }
    let doc = reopen(&mut doc);
    let table = doc.table(0).unwrap();
    assert_eq!(table.cell(0, 1).unwrap().grid_span(), Some(2));
    assert!(matches!(
        table.row(2).unwrap().cell(0).unwrap().v_merge(),
        Some(VMerge::Continue)
    ));

    let html = doc.to_html_fragment();
    // Each cell as its text, its column span and its row span.
    type Spanned = (String, Option<String>, Option<String>);
    let cells: Vec<Vec<Spanned>> = html_table_rows(&html)
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|(text, attributes)| {
                    (
                        text,
                        attributes.get("colspan").cloned(),
                        attributes.get("rowspan").cloned(),
                    )
                })
                .collect()
        })
        .collect();
    let cell = |text: &str, colspan: Option<&str>, rowspan: Option<&str>| {
        (
            text.to_owned(),
            colspan.map(str::to_owned),
            rowspan.map(str::to_owned),
        )
    };
    assert_eq!(
        cells,
        [
            vec![cell("r00", None, None), cell("r01\nr02", Some("2"), None)],
            vec![
                cell("r10\nr20", None, Some("2")),
                cell("r11", None, None),
                cell("r12", None, None),
            ],
            vec![cell("r21", None, None), cell("r22", None, None)],
        ]
    );
    // A nested table stays inside its cell.
    assert_eq!(html_texts(&html, "td td"), ["inner"]);

    // Markdown keeps a merged cell's text in its first column.
    let read = read_markdown(&doc.to_markdown());
    let Read::Table(rows) = &read[0] else {
        panic!("{read:?}")
    };
    let rows: Vec<Vec<&str>> = rows
        .iter()
        .map(|row| row.iter().map(String::as_str).collect())
        .collect();
    assert_eq!(
        rows,
        [
            ["r00", "r01 r02", ""],
            ["r10 r20", "r11", "r12"],
            ["", "r21", "r22"],
        ]
    );
}

#[test]
fn html_spans_a_cell_merged_down_beside_a_horizontal_merge() {
    let mut doc = Document::new();
    {
        let mut table = three_by_three(&mut doc);
        table.merge_cells((0, 2), (1, 2)).unwrap();
        table.merge_cells((1, 0), (1, 1)).unwrap();
    }
    let doc = reopen(&mut doc);
    let table = doc.table(0).unwrap();
    assert!(matches!(
        table.cell(0, 2).unwrap().v_merge(),
        Some(VMerge::Restart)
    ));
    let second = table.row(1).unwrap();
    assert_eq!(second.cell(0).unwrap().grid_span(), Some(2));
    assert!(matches!(
        second.cell(1).unwrap().v_merge(),
        Some(VMerge::Continue)
    ));

    let rows = html_table_rows(&doc.to_html_fragment());
    let (text, attributes) = &rows[0][2];
    assert_eq!(text, "r02\nr12");
    assert_eq!(
        attributes.get("rowspan").map(String::as_str),
        Some("2"),
        "a cell merged down beside a horizontal merge loses its row span: {rows:?}"
    );
}

/// A table of three grid columns whose rows are `rows`, each a row's XML
/// inside `w:tr`.
fn three_column_table(rows: &[&str]) -> String {
    format!(
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid>{}</w:tbl><w:p/>"#,
        rows.iter()
            .map(|row| format!("<w:tr>{row}</w:tr>"))
            .collect::<String>()
    )
}

/// A cell holding `text`, with `properties` inside its `w:tcPr`.
fn table_cell(properties: &str, text: &str) -> String {
    format!(r#"<w:tc><w:tcPr>{properties}</w:tcPr><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc>"#)
}

#[test]
fn markdown_places_cells_by_grid_column_and_aligns_columns_by_their_header() {
    let header = format!(
        "{}{}",
        table_cell(r#"<w:gridSpan w:val="2"/>"#, "A")
            .replace("<w:p>", r#"<w:p><w:pPr><w:jc w:val="center"/></w:pPr>"#),
        table_cell("", "B").replace("<w:p>", r#"<w:p><w:pPr><w:jc w:val="right"/></w:pPr>"#)
    );
    let omitted = format!(
        r#"<w:trPr><w:gridBefore w:val="1"/></w:trPr>{}{}"#,
        table_cell("", "x"),
        table_cell("", "y")
    );
    let doc = reopen(&mut doc_with_body(&three_column_table(&[
        &header, &omitted,
    ])));
    let markdown = doc.to_markdown();
    let read = read_markdown(&markdown);
    let Read::Table(rows) = &read[0] else {
        panic!("{read:?}")
    };
    assert_eq!(rows[0], ["A", "", "B"]);
    assert_eq!(rows[1], ["", "x", "y"]);
    assert_eq!(
        markdown.lines().nth(1),
        Some("| :---: | --- | ---: |"),
        "{markdown}"
    );
}

#[test]
fn a_huge_grid_span_from_the_file_stays_within_the_table_in_both_exports() {
    // Two cells past the table's width, as a generator that sets gridSpan
    // without removing the cells it covers writes them: both keep their text.
    let row = format!(
        "{}{}{}",
        table_cell(r#"<w:gridSpan w:val="4000000000"/>"#, "wide"),
        table_cell("", "after"),
        table_cell("", "then")
    );
    let before = format!(
        r#"<w:trPr><w:gridBefore w:val="4000000000"/></w:trPr>{}{}"#,
        table_cell("", "late"),
        table_cell("", "later")
    );
    let doc = reopen(&mut doc_with_body(&three_column_table(&[&row, &before])));
    let markdown = doc.to_markdown();
    assert!(markdown.len() < 1000, "{} bytes", markdown.len());
    assert!(
        ["wide", "after", "then", "late", "later"]
            .iter()
            .all(|text| markdown.contains(text)),
        "{markdown}"
    );
    let html = doc.to_html_fragment();
    assert!(html.contains("after") && html.contains("late"), "{html}");
}

#[test]
fn markdown_keeps_the_cells_after_a_horizontal_merge_in_their_columns() {
    let mut doc = Document::new();
    three_by_three(&mut doc)
        .merge_cells((1, 0), (1, 1))
        .unwrap();
    let doc = reopen(&mut doc);
    let html_rows = html_table_rows(&doc.to_html_fragment());
    assert_eq!(html_rows[1][1].0, "r12");
    let read = read_markdown(&doc.to_markdown());
    let Read::Table(rows) = &read[0] else {
        panic!("{read:?}")
    };
    assert_eq!(
        rows[1][2], "r12",
        "a cell after a horizontal merge moves to another column in Markdown: {rows:?}"
    );
}

#[test]
fn markdown_tables_take_the_first_row_as_header_and_escape_cell_text() {
    let mut doc = Document::new();
    {
        let mut table = doc.add_table(3, 2);
        table.cell(0, 0).unwrap().set_text("Name");
        table.cell(0, 1).unwrap().set_text("Value");
        table.cell(1, 0).unwrap().set_text("a|b");
        let mut cell = table.cell(1, 1).unwrap();
        cell.set_text("first");
        cell.add_paragraph("second");
        table.cell(2, 0).unwrap().set_text("*x*");
        table.cell(2, 1).unwrap().set_text("`y`");
    }
    let doc = reopen(&mut doc);
    let markdown = doc.to_markdown();
    let read = read_markdown(&markdown);
    let [Read::Table(rows)] = read.as_slice() else {
        panic!("{markdown}")
    };
    assert_eq!(rows[0], ["Name", "Value"]);
    assert_eq!(rows[1][0], "a|b");
    assert!(
        rows[1][1].starts_with("first") && rows[1][1].ends_with("second"),
        "both paragraphs stay in their cell: {rows:?}"
    );
    assert_eq!(rows[2], ["*x*", "`y`"]);
}

#[test]
fn markdown_keeps_the_text_of_a_nested_table() {
    let mut doc = Document::new();
    {
        let mut table = doc.add_table(1, 2);
        table.cell(0, 0).unwrap().set_text("Supported codes");
        let mut cell = table.cell(0, 1).unwrap();
        let mut nested = cell.add_table(2, 2);
        for (row, texts) in [["EAN-13", "13 digits"], ["UPC-A", "12 digits"]]
            .iter()
            .enumerate()
        {
            for (column, text) in texts.iter().enumerate() {
                nested.cell(row, column).unwrap().set_text(text);
            }
        }
    }
    let doc = reopen(&mut doc);
    assert_eq!(
        html_texts(&doc.to_html_fragment(), "td td"),
        ["EAN-13", "13 digits", "UPC-A", "12 digits"]
    );
    // GFM has no nested tables: the nested table's rows go into the cell,
    // each on a line of its own, its cells between pipes.
    let markdown = doc.to_markdown();
    assert_eq!(
        read_markdown(&markdown),
        [Read::Table(vec![vec![
            "Supported codes".to_owned(),
            "EAN-13 | 13 digits<br>UPC-A | 12 digits".to_owned()
        ]])],
        "the nested table's text is missing from the Markdown: {markdown}"
    );
}

// ---- Charts ----

#[test]
fn charts_export_as_data_tables_and_an_unreadable_chart_is_diagnosed() {
    // Word's manual: a line chart with a title.
    let manual = corpus_doc("word/manual.docx");
    let (html, diagnostics) = manual.to_html_fragment_with_diagnostics();
    assert_eq!(diagnostics, Vec::<String>::new());
    assert_eq!(
        html_texts(&html, "p.chart-title"),
        ["Runtime by temperature"]
    );
    let data = html_elements(&html, "table.chart-data tr");
    assert_eq!(data.len(), 8);
    assert_eq!(
        html_texts(&html, "table.chart-data th"),
        ["", "Standard model (h)", "Extended battery (h)"]
    );
    assert_eq!(
        html_texts(&html, "table.chart-data td")[..3],
        ["-20 °C", "5.1", "9"]
    );
    let read = read_markdown(&manual.to_markdown());
    let title = read
        .iter()
        .position(|block| *block == Read::Paragraph("Runtime by temperature".to_owned()))
        .expect("the chart title is a paragraph");
    let Read::Table(rows) = &read[title + 1] else {
        panic!("{read:?}")
    };
    assert_eq!(rows[0], ["", "Standard model (h)", "Extended battery (h)"]);
    assert_eq!(rows[1], ["-20 °C", "5.1", "9"]);
    assert_eq!(rows.len(), 8);

    // A chart part without a chart is left out and reported.
    let mut doc = doc_with(&["Intro"]);
    let data = ChartData {
        categories: vec!["Q1".into()],
        series: vec![("Sales".into(), vec![1.5])],
        ..Default::default()
    };
    doc.add_chart(ChartKind::Bar, Length::pt(200.0), Length::pt(100.0), &data)
        .unwrap();
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let relationship = package
        .get_part_rels("/word/document.xml")
        .unwrap()
        .items
        .iter()
        .find(|rel| rel.rel_type.ends_with("/chart"))
        .expect("a chart relationship")
        .clone();
    package.set_part(
        &OpcPackage::resolve_rel_target("/word/document.xml", &relationship.target),
        br#"<?xml version="1.0" encoding="UTF-8"?><c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"/>"#.to_vec(),
    );
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let broken = Document::from_bytes(&bytes.into_inner()).unwrap();
    let (html, diagnostics) = broken.to_html_with_diagnostics();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(
        diagnostics[0].starts_with(&format!(
            "chart relationship {}: chart table omitted",
            relationship.id
        )),
        "{diagnostics:?}"
    );
    assert!(!html.contains("chart-data"), "{html}");
    assert_eq!(html_texts(&html, "p")[0], "Intro");
    assert_eq!(broken.to_html_fragment_with_diagnostics().1, diagnostics);
    assert_eq!(broken.to_markdown_with_diagnostics().1, diagnostics);
}

// ---- Stories other than the body ----

#[test]
fn internal_links_reach_their_bookmark_in_both_exports() {
    let doc = reopen(&mut doc_with_body(concat!(
        r#"<w:p><w:bookmarkStart w:id="1" w:name="goal"/><w:r><w:t>Goal</w:t></w:r><w:bookmarkEnd w:id="1"/>"#,
        r#"<w:bookmarkStart w:id="2" w:name="_GoBack"/><w:bookmarkEnd w:id="2"/></w:p>"#,
        r#"<w:p><w:r><w:t xml:space="preserve">See </w:t></w:r><w:hyperlink w:anchor="goal">"#,
        r#"<w:r><w:t>the goal</w:t></w:r></w:hyperlink></w:p>"#,
    )));
    let html = doc.to_html_fragment();
    assert!(html.contains(r##"<a id="goal"></a>Goal"##), "{html}");
    assert!(
        html.contains(r##"See <a href="#goal">the goal</a>"##),
        "{html}"
    );
    // A bookmark no link targets gets no anchor.
    assert!(!html.contains("_GoBack"), "{html}");
    let markdown = doc.to_markdown();
    assert!(
        markdown.contains(r##"<a id="goal"></a>Goal"##),
        "{markdown}"
    );
    assert!(markdown.contains("See [the goal](#goal)"), "{markdown}");
    assert!(!markdown.contains("_GoBack"), "{markdown}");

    // EPUB parts chapters and drops the anchor, as it reports: no dead link.
    let epub = doc.to_epub_bytes().unwrap();
    assert!(
        epub.diagnostics
            .iter()
            .any(|d| d.message == "internal hyperlink anchor was dropped during EPUB export"),
        "{:?}",
        epub.diagnostics
            .iter()
            .map(|d| &d.message)
            .collect::<Vec<_>>()
    );
    let mut archive = zip::ZipArchive::new(Cursor::new(epub.bytes)).unwrap();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let mut text = String::new();
        std::io::Read::read_to_string(&mut entry, &mut text).ok();
        assert!(!text.contains("href=\"#goal\""), "{text}");
    }
}

#[test]
fn links_to_bookmarks_in_cells_after_links_and_missing_ones_read_right() {
    let mut doc = doc_with_body(concat!(
        // A bookmark right after an external link, and a link to a
        // bookmark that does not exist.
        r#"<w:p><w:hyperlink r:id="rIdSite"><w:r><w:t>site</w:t></w:r></w:hyperlink>"#,
        r#"<w:bookmarkStart w:id="1" w:name="after"/><w:r><w:t>After</w:t></w:r><w:bookmarkEnd w:id="1"/></w:p>"#,
        r#"<w:p><w:hyperlink w:anchor="after"><w:r><w:t>to after</w:t></w:r></w:hyperlink>"#,
        r#"<w:hyperlink w:anchor="nowhere"><w:r><w:t xml:space="preserve"> lost</w:t></w:r></w:hyperlink></w:p>"#,
        // A link and its bookmark in table cells.
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc>"#,
        r#"<w:p><w:bookmarkStart w:id="2" w:name="cell"/><w:r><w:t>In cell</w:t></w:r><w:bookmarkEnd w:id="2"/></w:p>"#,
        r#"<w:p><w:hyperlink w:anchor="cell"><w:r><w:t>to cell</w:t></w:r></w:hyperlink></w:p></w:tc></w:tr></w:tbl>"#,
        // An external link with an anchor keeps it as its fragment.
        r#"<w:p><w:hyperlink r:id="rIdSite" w:anchor="part"><w:r><w:t>deep</w:t></w:r></w:hyperlink></w:p>"#,
    ));
    let bytes = doc.to_bytes().unwrap();
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let part = package.main_document_part().unwrap();
    let mut rels = package.get_part_rels(&part).cloned().unwrap_or_default();
    rels.items.push(oxoffice_opc::relationship::Relationship {
        id: "rIdSite".to_owned(),
        rel_type: oxoffice_opc::relationship::rel_types::HYPERLINK.to_owned(),
        target: "https://example.com/".to_owned(),
        target_mode: Some("External".to_owned()),
    });
    package.set_part_rels(&part, rels);
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let doc = Document::from_bytes(&out.into_inner()).unwrap();

    let html = doc.to_html_fragment();
    assert!(
        html.contains(r##"<a href="https://example.com/">site</a><a id="after"></a>After"##),
        "the anchor is not nested in the link: {html}"
    );
    assert!(
        html.contains(r##"<a href="#after">to after</a>"##),
        "{html}"
    );
    assert!(
        !html.contains("#nowhere") && html.contains(" lost"),
        "{html}"
    );
    assert!(html.contains(r##"<a id="cell"></a>In cell"##), "{html}");
    assert!(html.contains(r##"<a href="#cell">to cell</a>"##), "{html}");
    assert!(
        html.contains(r##"<a href="https://example.com/#part">deep</a>"##),
        "{html}"
    );
    let markdown = doc.to_markdown();
    assert!(markdown.contains("[to after](#after)"), "{markdown}");
    assert!(!markdown.contains("#nowhere"), "{markdown}");
    assert!(
        markdown.contains("[deep](https://example.com/#part)"),
        "{markdown}"
    );
}

#[test]
fn a_header_row_holds_header_cells_in_html() {
    let doc = reopen(&mut doc_with_body(concat!(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid>"#,
        r#"<w:tr><w:trPr><w:tblHeader/></w:trPr><w:tc><w:p><w:r><w:t>Name</w:t></w:r></w:p></w:tc>"#,
        r#"<w:tc><w:p><w:r><w:t>Total</w:t></w:r></w:p></w:tc></w:tr>"#,
        r#"<w:tr><w:tc><w:p><w:r><w:t>Ann</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>3</w:t></w:r></w:p></w:tc></w:tr>"#,
        r#"</w:tbl><w:p/>"#,
    )));
    let html = doc.to_html_fragment();
    assert_eq!(html.matches("<th ").count(), 2, "{html}");
    // Drawn like the cell Word draws, not bold and centred.
    assert!(
        html.contains("<th style=\"font-weight:inherit;text-align:inherit;\"><p>Name</p>\n</th>"),
        "{html}"
    );
    assert!(html.contains("<td><p>Ann</p>\n</td>"), "{html}");
    // An HTML reader marks the row as a header again.
    let reread = Document::from_html(&doc.to_html()).unwrap().document;
    assert!(reread.table(0).unwrap().row(0).unwrap().is_header());
}

#[test]
fn the_exports_hold_the_body_only() {
    // The emitter converts "body content only"; the CLI convert route
    // appends the other stories itself.
    let mut doc = doc_with(&["Commented text here", "Noted here"]);
    doc.add_comment_on_text("text", "Ann", "A comment", Default::default())
        .unwrap();
    doc.set_header("Header text");
    doc.set_footer("Footer text");
    let location = doc.paragraph_story_location(1).unwrap().unwrap();
    let note = doc.create_footnote(&location, "Note text").unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.comments().len(), 1);
    assert_eq!(doc.header_text().as_deref(), Some("Header text"));
    assert_eq!(doc.footer_text().as_deref(), Some("Footer text"));
    assert!(doc.footnote_story(note).unwrap().is_some());

    // The body text is there, the other stories' text is not. How a note
    // reference or a comment mark shows is not pinned: the facade docs do
    // not state it.
    for (format, output) in [
        ("HTML", doc.to_html_fragment()),
        ("Markdown", doc.to_markdown()),
    ] {
        for body in ["Commented text here", "Noted here"] {
            assert!(output.contains(body), "{format} lacks {body:?}: {output}");
        }
        for story in ["A comment", "Header text", "Footer text", "Note text"] {
            assert!(
                !output.contains(story),
                "{format} holds {story:?}: {output}"
            );
        }
    }
}

// ---- Equations ----

const MATH_NAMESPACE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";

#[test]
fn equations_reach_both_exports_in_their_place() {
    let doc = reopen(&mut doc_with_body(&format!(
        concat!(
            r#"<w:p><w:r><w:t xml:space="preserve">Let </w:t></w:r>"#,
            r#"<m:oMath xmlns:m="{ns}"><m:r><m:t>x=1</m:t></m:r></m:oMath>"#,
            r#"<w:r><w:t xml:space="preserve"> hold.</w:t></w:r></w:p>"#,
            r#"<w:p><m:oMathPara xmlns:m="{ns}"><m:oMath><m:f><m:num><m:r><m:t>a</m:t></m:r></m:num>"#,
            r#"<m:den><m:r><m:t>b</m:t></m:r></m:den></m:f></m:oMath></m:oMathPara></w:p>"#,
        ),
        ns = MATH_NAMESPACE
    )));
    assert_eq!(doc.paragraph(0).unwrap().equations().count(), 1);

    let (html, diagnostics) = doc.to_html_fragment_with_diagnostics();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let inline = html.find("<math").expect("the inline equation is MathML");
    assert!(
        html.find("Let ").unwrap() < inline && inline < html.find(" hold.").unwrap(),
        "the equation stays between the words around it: {html}"
    );
    assert!(
        html.contains("<p><math display=\"block\""),
        "the display equation is a block: {html}"
    );
    assert!(html.contains("<mfrac>"), "{html}");

    let (markdown, diagnostics) = doc.to_markdown_with_diagnostics();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert!(
        markdown.contains("Let $x=1$ hold.") || markdown.contains("Let $x = 1$ hold."),
        "{markdown}"
    );
    assert!(markdown.contains("$$\\frac{a}{b}$$"), "{markdown}");

    // EPUB writes MathML too, and names the documents that hold it.
    let epub = doc.to_epub_bytes().unwrap();
    assert!(
        !epub.diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains("equation") || diagnostic.path.contains("/xml[")
        }),
        "{:?}",
        epub.diagnostics
            .iter()
            .map(|diagnostic| format!("{} {}", diagnostic.path, diagnostic.message))
            .collect::<Vec<_>>()
    );
    let mut archive = zip::ZipArchive::new(Cursor::new(epub.bytes)).unwrap();
    let mut chapters = String::new();
    let mut package = String::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().unwrap().into_owned();
        let mut text = String::new();
        if name.ends_with(".opf") {
            std::io::Read::read_to_string(&mut entry, &mut package).unwrap();
        } else if name.ends_with(".xhtml") && !name.ends_with("nav.xhtml") {
            std::io::Read::read_to_string(&mut entry, &mut text).unwrap();
            chapters.push_str(&text);
        }
    }
    assert_eq!(chapters.matches("<math").count(), 2, "{chapters}");
    assert!(chapters.contains("<math display=\"block\""), "{chapters}");
    assert!(package.contains("properties=\"mathml\""), "{package}");

    // RTF and ODT cannot write an equation yet: each says so.
    let rtf = doc.to_rtf_bytes().unwrap();
    let dropped = |messages: Vec<&str>, format: &str| {
        assert_eq!(
            messages
                .iter()
                .filter(|message| **message
                    == format!("Word equation was dropped during {format} export"))
                .count(),
            2,
            "{messages:?}"
        );
    };
    dropped(
        rtf.diagnostics.iter().map(|d| d.message.as_str()).collect(),
        "RTF",
    );
    let odt = doc.to_odt_bytes().unwrap();
    dropped(
        odt.diagnostics.iter().map(|d| d.message.as_str()).collect(),
        "ODT",
    );

    // MHTML holds the same HTML and no longer reports the equations dropped.
    let mhtml = doc.to_mhtml_bytes().unwrap();
    assert!(
        !mhtml
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("equation")),
        "{:?}",
        mhtml
            .diagnostics
            .iter()
            .map(|diagnostic| &diagnostic.message)
            .collect::<Vec<_>>()
    );
}

#[test]
fn equations_of_tracked_and_table_paragraphs_reach_both_exports() {
    let equation =
        format!(r#"<m:oMath xmlns:m="{MATH_NAMESPACE}"><m:r><m:t>y</m:t></m:r></m:oMath>"#);
    let doc = reopen(&mut doc_with_body(&format!(
        concat!(
            // A tracked insertion before the equation: the accepted view
            // keeps the equation after the inserted text.
            r#"<w:p><w:ins w:id="1" w:author="A" w:date="2026-01-01T00:00:00Z"><w:r><w:t xml:space="preserve">Then </w:t></w:r></w:ins>"#,
            "{eq}",
            r#"<w:r><w:t xml:space="preserve"> grows.</w:t></w:r></w:p>"#,
            r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t xml:space="preserve">cell </w:t></w:r>{eq}</w:p></w:tc></w:tr></w:tbl>"#,
            r#"<w:p/>"#,
        ),
        eq = equation
    )));
    let html = doc.to_html_fragment();
    let math = html.find("<math").expect("{html}");
    assert!(
        html.find("Then ").unwrap() < math && math < html.find(" grows.").unwrap(),
        "{html}"
    );
    assert_eq!(html.matches("<math").count(), 2, "{html}");
    let markdown = doc.to_markdown();
    assert!(markdown.contains("Then $y$ grows."), "{markdown}");
    assert!(markdown.contains("| cell $y$ |"), "{markdown}");
}

#[test]
fn every_equation_of_the_word_paper_reaches_both_exports() {
    let doc = corpus_doc("word/paper.docx");
    let count: usize = doc
        .paragraphs()
        .iter()
        .map(|paragraph| {
            paragraph
                .equations()
                .map(|equation| match equation {
                    oxoffice_docx_oxml::math::OfficeMath::Inline(_) => 1,
                    oxoffice_docx_oxml::math::OfficeMath::Display(display) => {
                        display.equations.len()
                    }
                })
                .sum::<usize>()
        })
        .sum();
    assert!(count >= 11, "{count}");
    let (html, diagnostics) = doc.to_html_fragment_with_diagnostics();
    assert_eq!(html.matches("<math").count(), count, "{diagnostics:?}");
    let (markdown, _) = doc.to_markdown_with_diagnostics();
    let (inline, display): (usize, usize) = doc
        .paragraphs()
        .iter()
        .flat_map(|paragraph| paragraph.equations().cloned().collect::<Vec<_>>())
        .map(|equation| match equation {
            oxoffice_docx_oxml::math::OfficeMath::Inline(_) => (1, 0),
            oxoffice_docx_oxml::math::OfficeMath::Display(display) => (0, display.equations.len()),
        })
        .fold((0, 0), |(a, b), (c, d)| (a + c, b + d));
    // Unescaped dollars: two per inline equation, four per display one.
    let dollars = markdown
        .match_indices('$')
        .filter(|(at, _)| !markdown[..*at].ends_with('\\'))
        .count();
    assert_eq!(dollars, 2 * inline + 4 * display, "{markdown}");
}

#[test]
fn equation_content_outside_the_supported_subset_is_reported() {
    // m:func (sin x) is kept as preserved XML, not as a typed expression.
    let func = concat!(
        r#"<m:func><m:funcPr/><m:fName><m:r><m:t>sin</m:t></m:r></m:fName>"#,
        r#"<m:e><m:r><m:t>x</m:t></m:r></m:e></m:func>"#
    );
    let doc = reopen(&mut doc_with_body(&format!(
        concat!(
            r#"<w:p><m:oMath xmlns:m="{ns}">{func}<m:r><m:t>=0</m:t></m:r></m:oMath></w:p>"#,
            r#"<w:p><m:oMath xmlns:m="{ns}">{func}</m:oMath></w:p>"#,
        ),
        ns = MATH_NAMESPACE,
        func = func
    )));
    let (html, diagnostics) = doc.to_html_fragment_with_diagnostics();
    assert!(
        diagnostics.iter().any(|message| message
            == "equation 1 written as MathML without its content outside the supported subset"),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|message| message.starts_with("equation 2 left out of the MathML")),
        "{diagnostics:?}"
    );
    assert_eq!(html.matches("<math").count(), 1, "{html}");
    let (markdown, diagnostics) = doc.to_markdown_with_diagnostics();
    assert!(
        diagnostics
            .iter()
            .any(|message| message.starts_with("equation 2 left out of the LaTeX")),
        "{diagnostics:?}"
    );
    assert_eq!(markdown.matches('$').count(), 2, "{markdown}");
}

#[test]
fn equations_in_wrappers_and_tracked_changes_reach_both_exports_in_their_place() {
    let equation = |text: &str| {
        format!(r#"<m:oMath xmlns:m="{MATH_NAMESPACE}"><m:r><m:t>{text}</m:t></m:r></m:oMath>"#)
    };
    let date = r#"w:author="A" w:date="2026-01-01T00:00:00Z""#;
    let doc = reopen(&mut doc_with_body(&format!(
        concat!(
            // In a smart tag.
            r#"<w:p><w:smartTag w:uri="urn:u" w:element="e"><w:r><w:t xml:space="preserve">tag </w:t></w:r>{y}</w:smartTag>"#,
            r#"<w:r><w:t xml:space="preserve"> end</w:t></w:r></w:p>"#,
            // In a tracked insertion, and a deleted one left out.
            r#"<w:p><w:ins w:id="1" {date}><w:r><w:t xml:space="preserve">new </w:t></w:r>{z}</w:ins>"#,
            r#"<w:del w:id="2" {date}>{q}</w:del><w:r><w:t xml:space="preserve"> kept</w:t></w:r></w:p>"#,
            // In a paragraph whose mark is deleted: it joins the next one.
            r#"<w:p><w:pPr><w:rPr><w:del w:id="3" {date}/></w:rPr></w:pPr>"#,
            r#"<w:r><w:t xml:space="preserve">first </w:t></w:r>{a}</w:p>"#,
            r#"<w:p><w:r><w:t xml:space="preserve"> second</w:t></w:r></w:p>"#,
            // Document text with a dollar.
            r#"<w:p><w:r><w:t xml:space="preserve">costs $5 if </w:t></w:r>{x}</w:p>"#,
        ),
        y = equation("y"),
        z = equation("z"),
        q = equation("q"),
        a = equation("a"),
        x = equation("x"),
        date = date,
    )));
    let markdown = doc.to_markdown();
    for line in [
        "tag $y$ end",
        "new $z$ kept",
        "first $a$ second",
        "costs \\$5 if $x$",
    ] {
        assert!(markdown.contains(line), "{line}: {markdown}");
    }
    assert!(!markdown.contains("$q$"), "{markdown}");
    let html = doc.to_html_fragment();
    assert_eq!(html.matches("<math").count(), 4, "{html}");
    assert!(
        html.find("tag ").unwrap() < html.find("<mi>y</mi>").unwrap(),
        "{html}"
    );
    assert!(
        html.find("first ").unwrap() < html.find("<mi>a</mi>").unwrap(),
        "{html}"
    );
}

/// A document whose body holds this raw XML, with the math namespace
/// declared on the document root, as Word writes it, not on each equation.
fn doc_with_math_body(body: &str) -> Document {
    let mut doc = doc_with_body(body);
    let bytes = doc.to_bytes().unwrap();
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let part = package.main_document_part().unwrap();
    let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
    let xml = xml.replacen(
        "<w:document ",
        &format!(r#"<w:document xmlns:m="{MATH_NAMESPACE}" "#),
        1,
    );
    package.set_part(&part, xml.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    Document::from_bytes(&out.into_inner()).unwrap()
}

#[test]
fn equations_word_writes_in_wrappers_fields_and_display_groups_reach_both_exports() {
    let doc = doc_with_math_body(concat!(
        // In a smart tag, `m` declared on the root only.
        r#"<w:p><w:smartTag w:uri="urn:u" w:element="e"><w:r><w:t xml:space="preserve">tag </w:t></w:r>"#,
        r#"<m:oMath><m:r><m:t>y</m:t></m:r></m:oMath></w:smartTag></w:p>"#,
        // In a simple field's result.
        r#"<w:p><w:fldSimple w:instr=" QUOTE x "><w:r><w:t xml:space="preserve">field </w:t></w:r>"#,
        r#"<m:oMath><m:r><m:t>f</m:t></m:r></m:oMath></w:fldSimple></w:p>"#,
        // A centred display group.
        r#"<w:p><m:oMathPara><m:oMathParaPr><m:jc m:val="center"/></m:oMathParaPr>"#,
        r#"<m:oMath><m:r><m:t>d</m:t></m:r></m:oMath></m:oMathPara></w:p>"#,
        // After a two-run field, in a tracked insertion (the field's result
        // text is lost there: #392).
        r#"<w:p><w:ins w:id="5" w:author="A" w:date="2026-01-01T00:00:00Z"><w:fldSimple w:instr=" QUOTE a ">"#,
        r#"<w:r><w:t>1</w:t></w:r><w:r><w:t>2</w:t></w:r></w:fldSimple>"#,
        r#"<m:oMath><m:r><m:t>z</m:t></m:r></m:oMath><w:r><w:t>C</w:t></w:r></w:ins></w:p>"#,
    ));
    let (markdown, diagnostics) = doc.to_markdown_with_diagnostics();
    assert!(markdown.contains("tag $y$"), "{markdown}");
    assert!(markdown.contains("field $f$"), "{markdown}");
    assert!(markdown.contains("$$d$$"), "{markdown}");
    assert!(
        diagnostics
            .iter()
            .any(|message| message == "display group of equation 3: its justification was left out"),
        "{diagnostics:?}"
    );
    // Both exports place it before the run that follows it.
    assert!(markdown.contains("$z$C"), "{markdown}");
    let html = doc.to_html_fragment();
    assert_eq!(html.matches("<math").count(), 4, "{html}");
    assert!(html.contains("<mi>z</mi></math>C</p>"), "{html}");
}

#[test]
fn an_equation_after_a_link_is_not_part_of_the_link() {
    let mut doc = Document::new();
    let link = doc.add_hyperlink_relationship("https://example.com/");
    let mut paragraph = doc.add_paragraph("");
    paragraph.add_hyperlink("see", &link);
    let expressions = oxoffice_docx::equation_from_latex("y").unwrap().value;
    paragraph
        .add_equation(oxoffice_docx_oxml::math::OfficeMath::inline(
            expressions.expressions.clone(),
        ))
        .unwrap();
    paragraph.add_run(" here");
    let html = doc.to_html_fragment();
    assert!(html.contains("see</a><math"), "{html}");
    let markdown = doc.to_markdown();
    assert!(
        markdown.contains("[see](https://example.com/)$y$ here"),
        "{markdown}"
    );
}

// ---- Corpus ----

#[test]
fn report_headings_and_merged_note_row_of_every_producer_reach_both_exports() {
    let note = "Operating margin excludes one-off costs of the Lyon opening (212).";
    for producer in ["word", "google", "libreoffice", "authored"] {
        let doc = corpus_doc(&format!("{producer}/report.docx"));
        let headings: Vec<(usize, String)> = doc
            .headings()
            .into_iter()
            .map(|(level, text)| (level as usize, text))
            .collect();
        assert_eq!(headings.len(), 6, "{producer}");

        let html = doc.to_html_fragment();
        let mut html_headings = Vec::new();
        for (text, _) in html_elements(&html, "h1, h2, h3, h4, h5, h6") {
            let level = (1..=6)
                .find(|level| html.contains(&format!("<h{level}>{text}</h{level}>")))
                .unwrap_or_else(|| panic!("{producer}: {text}"));
            html_headings.push((level, text));
        }
        assert_eq!(html_headings, headings, "{producer}");
        let (_, attributes) = html_elements(&html, "td")
            .into_iter()
            .find(|(text, _)| text == note)
            .unwrap_or_else(|| panic!("{producer}: the note row is a cell"));
        assert_eq!(
            attributes.get("colspan").map(String::as_str),
            Some("4"),
            "{producer}"
        );

        let read = read_markdown(&doc.to_markdown());
        let markdown_headings: Vec<(usize, String)> = read
            .iter()
            .filter_map(|block| match block {
                Read::Heading(level, text) => Some((*level, text.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(markdown_headings, headings, "{producer}");
        let first_table = read
            .iter()
            .find_map(|block| match block {
                Read::Table(rows) => Some(rows),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{producer}: a table"));
        assert_eq!(
            first_table.last().unwrap(),
            &[note, "", "", ""],
            "{producer}"
        );
    }
}

/// Whether `text` holds Arabic, Hebrew or CJK characters.
fn rtl_or_cjk(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c, '\u{0590}'..='\u{06FF}' | '\u{3040}'..='\u{30FF}' | '\u{4E00}'..='\u{9FFF}')
    })
}

#[test]
fn rtl_and_cjk_text_of_the_minutes_survives_both_exports() {
    let doc = corpus_doc("word/minutes.docx");
    let expected: Vec<String> = texts(&doc)
        .into_iter()
        .map(|text| text.trim().to_owned())
        .filter(|text| rtl_or_cjk(text))
        .collect();
    assert!(expected.len() >= 6, "{expected:?}");

    let html = doc.to_html_fragment();
    let html_blocks = html_texts(&html, "p, li, h1, h2, h3");
    let markdown = doc.to_markdown();
    let markdown_blocks: Vec<String> = read_markdown(&markdown)
        .into_iter()
        .filter_map(|block| match block {
            Read::Heading(_, text) | Read::Paragraph(text) | Read::Item(text) => Some(text),
            _ => None,
        })
        .collect();
    for text in &expected {
        assert!(
            html_blocks.iter().any(|block| block.trim() == text),
            "HTML: {text}"
        );
        assert!(markdown_blocks.contains(text), "Markdown: {text}");
    }

    // The Arabic and Chinese cells of the tables.
    for cell in ["إعداد خطة افتتاح مستودع شنغهاي.", "配送时间"] {
        assert!(
            html_texts(&html, "td").iter().any(|text| text == cell),
            "{cell}"
        );
        assert!(
            read_markdown(&markdown).iter().any(|block| matches!(
                block,
                Read::Table(rows) if rows.iter().flatten().any(|text| text == cell)
            )),
            "{cell}"
        );
    }
}

#[test]
fn a_right_to_left_paragraph_keeps_its_direction_in_html() {
    let doc = corpus_doc("word/minutes.docx");
    let paragraph = doc
        .paragraphs()
        .into_iter()
        .find(|paragraph| paragraph.text().starts_with("رحّب"))
        .expect("the Arabic statement");
    assert_eq!(paragraph.right_to_left_value(), Some(true));
    let text = paragraph.text();
    let (_, attributes) = html_elements(&doc.to_html_fragment(), "p")
        .into_iter()
        .find(|(html_text, _)| *html_text == text)
        .expect("the statement is a paragraph");
    let rtl = attributes.get("dir").map(String::as_str) == Some("rtl")
        || attributes
            .get("style")
            .is_some_and(|style| style.contains("direction:rtl"));
    assert!(
        rtl,
        "the right-to-left paragraph is written without its direction: {attributes:?}"
    );
}

#[test]
fn exporting_leaves_the_document_unchanged() {
    let mut doc = corpus_doc("word/report.docx");
    let before = doc.to_bytes().unwrap();
    let _ = doc.to_html();
    let _ = doc.to_html_with_diagnostics();
    let _ = doc.to_html_fragment();
    let _ = doc.to_html_fragment_with_diagnostics();
    let _ = doc.to_markdown();
    let _ = doc.to_markdown_with_diagnostics();
    assert!(
        doc.to_bytes().unwrap() == before,
        "an export changed the document"
    );
}

// ---- Round trips ----

/// A document with a heading, bold, italic and struck runs, a link and a
/// table, saved and reopened.
fn rich_document() -> Document {
    let mut doc = Document::new();
    doc.add_paragraph("Quarterly report").style("Heading1");
    {
        let mut paragraph = doc.add_paragraph("Sales ");
        paragraph.add_run("rose").bold(true);
        paragraph.add_run(" by ");
        paragraph.add_run("ten").italic(true);
        paragraph.add_run(" percent, ");
        paragraph.add_run("not").strike(true);
        paragraph.add_run(" less.");
    }
    let link = doc.add_hyperlink_relationship("https://example.com/report");
    doc.add_paragraph("See ").add_hyperlink("the report", &link);
    {
        let mut table = doc.add_table(2, 2);
        table.cell(0, 0).unwrap().set_text("Region");
        table.cell(0, 1).unwrap().set_text("Revenue");
        table.cell(1, 0).unwrap().set_text("North");
        table.cell(1, 1).unwrap().set_text("1,402");
    }
    doc.add_paragraph("End.");
    reopen(&mut doc)
}

/// Check that `doc` reads as [`rich_document`] through the facade.
fn assert_reads_as_the_rich_document(doc: &Document) {
    assert_eq!(
        texts(doc),
        [
            "Quarterly report",
            "Sales rose by ten percent, not less.",
            "See the report",
            "End."
        ]
    );
    assert_eq!(doc.paragraph(0).unwrap().style_id(), Some("Heading1"));
    let sales = doc.paragraph(1).unwrap();
    assert!(run_with_text(&sales, "rose").is_bold());
    assert!(run_with_text(&sales, "ten").is_italic());
    assert!(run_with_text(&sales, "not").is_strike());
    assert!(!run_with_text(&sales, " by ").is_bold());
    let see = doc.paragraph(2).unwrap();
    let spans = see.hyperlink_spans();
    assert_eq!(spans.len(), 1);
    let relationship = spans[0].2.expect("an external link");
    assert_eq!(
        doc.hyperlink_url(relationship).as_deref(),
        Some("https://example.com/report")
    );
    let table = doc.table(0).unwrap();
    assert_eq!(table.cell(0, 1).unwrap().text(), "Revenue");
    assert_eq!(table.cell(1, 1).unwrap().text(), "1,402");
}

#[test]
fn markdown_export_reads_back_as_the_same_document_and_the_same_markdown() {
    let doc = rich_document();
    let markdown = doc.to_markdown();
    let imported = Document::from_markdown(&markdown).unwrap();
    assert_eq!(imported.diagnostics, []);
    let imported = reopen(&mut { imported.document });
    assert_reads_as_the_rich_document(&imported);
    assert_eq!(imported.to_markdown(), markdown);
}

#[test]
fn html_export_reads_back_as_the_same_document() {
    let doc = rich_document();
    let imported = Document::from_html(&doc.to_html()).unwrap();
    // The reader reports only rules of the page's own style sheet it cannot
    // apply; it applies the others (the link rule gives the link run its
    // colour and underline), so the content and its formatting read back.
    assert!(
        imported.diagnostics.iter().all(|diagnostic| {
            diagnostic.location.contains("style") && diagnostic.message.contains("CSS")
        }),
        "{:?}",
        imported.diagnostics
    );
    let imported = reopen(&mut { imported.document });
    assert_reads_as_the_rich_document(&imported);
}

// ---- Imports ----

#[test]
fn html_import_maps_headings_formats_and_table_spans() {
    let result = Document::from_html(concat!(
        "<h1>Title</h1><h3>Section</h3>",
        "<p>a <b>bold</b> <i>italic</i> <u>under</u> <s>struck</s> x<sup>2</sup> H<sub>2</sub>O ",
        "<span style=\"color:#ff0000\">red</span> <mark>marked</mark></p>",
        "<table><tr><td rowspan=\"2\">a</td><td colspan=\"2\">b</td></tr><tr><td>c</td><td>e</td></tr></table>",
    ))
    .unwrap();
    assert_eq!(result.diagnostics, []);
    let doc = reopen(&mut { result.document });
    assert_eq!(texts(&doc)[..2], ["Title", "Section"]);
    assert_eq!(doc.paragraph(0).unwrap().style_id(), Some("Heading1"));
    assert_eq!(doc.paragraph(1).unwrap().style_id(), Some("Heading3"));
    let paragraph = doc.paragraph(2).unwrap();
    assert!(run_with_text(&paragraph, "bold").is_bold());
    assert!(run_with_text(&paragraph, "italic").is_italic());
    assert!(run_with_text(&paragraph, "under").is_underline());
    assert!(run_with_text(&paragraph, "struck").is_strike());
    let raised: Vec<Option<String>> = paragraph
        .runs()
        .filter(|run| run.text() == "2")
        .map(|run| run.vert_align().map(str::to_owned))
        .collect();
    assert_eq!(
        raised,
        [Some("superscript".to_owned()), Some("subscript".to_owned())]
    );
    assert_eq!(run_with_text(&paragraph, "red").color(), Some("FF0000"));
    assert_eq!(
        run_with_text(&paragraph, "marked").shading_fill(),
        Some("FFFF00")
    );
    assert!(!run_with_text(&paragraph, "a ").is_bold());

    let table = doc.table(0).unwrap();
    let first = table.row(0).unwrap();
    assert!(matches!(
        first.cell(0).unwrap().v_merge(),
        Some(VMerge::Restart)
    ));
    assert_eq!(first.cell(1).unwrap().grid_span(), Some(2));
    let second = table.row(1).unwrap();
    assert!(matches!(
        second.cell(0).unwrap().v_merge(),
        Some(VMerge::Continue)
    ));
    let second_texts: Vec<String> = (0..second.cell_count())
        .map(|index| second.cell(index).unwrap().text())
        .collect();
    assert_eq!(second_texts, ["", "c", "e"]);
}

#[test]
fn html_import_drops_scripts_handlers_and_unsafe_links_and_says_so() {
    let result = Document::from_html(concat!(
        "<p onclick=\"steal()\">Hello <script>evil()</script>",
        "<a href=\"javascript:alert(2)\">js</a> <a href=\"https://example.com/x\">ok</a></p>",
        "<iframe src=\"https://example.com/frame\">framed</iframe>",
    ))
    .unwrap();
    let reported: Vec<(&str, &str)> = result
        .diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.location.as_str(), diagnostic.message.as_str()))
        .collect();
    assert_eq!(
        reported,
        [
            ("html/body/p[1]/script[1]", "dropped HTML script"),
            (
                "html/body/p[1]/a[1]",
                "dropped HTML link target and retained anchor text"
            ),
            ("html/body/iframe[1]", "dropped HTML iframe content"),
        ]
    );
    let doc = reopen(&mut { result.document });
    assert_eq!(texts(&doc), ["Hello js ok"]);
    let xml = doc.part_xml("word/document.xml").unwrap();
    for dropped in ["evil", "steal", "javascript", "framed"] {
        assert!(!xml.contains(dropped), "{dropped} reached the document");
    }
    let paragraph = doc.paragraph(0).unwrap();
    let urls: Vec<Option<String>> = paragraph
        .hyperlink_spans()
        .into_iter()
        .map(|(_, _, relationship)| doc.hyperlink_url(relationship.expect("a relationship")))
        .collect();
    assert_eq!(urls, [Some("https://example.com/x".to_owned())]);
}

#[test]
fn html_import_embeds_inline_and_supplied_images_and_keeps_the_alt_text_of_missing_ones() {
    let html = format!(
        "<p>a <img src=\"data:image/png;base64,{}\" alt=\"inline dot\"> b \
         <img src=\"dot.png\" alt=\"named dot\" width=\"20\" height=\"20\"> c \
         <img src=\"missing.png\" alt=\"gone\"></p>",
        BASE64.encode(PNG)
    );
    let supplied = [HtmlImageResource {
        source: "dot.png",
        bytes: PNG,
        filename: "dot.png",
    }];
    let result = Document::from_html_with_options(&html, None, &supplied).unwrap();
    let reported: Vec<(&str, &str)> = result
        .diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.location.as_str(), diagnostic.message.as_str()))
        .collect();
    assert_eq!(
        reported,
        [(
            "html/body/p[1]/img[3]",
            "dropped unresolved HTML image `missing.png` and retained alternate text"
        )]
    );
    let doc = reopen(&mut { result.document });
    let pictures: Vec<(Option<String>, bool, String)> = doc
        .pictures()
        .unwrap()
        .into_iter()
        .map(|picture| {
            (
                picture.description,
                picture.bytes == PNG,
                picture.content_type,
            )
        })
        .collect();
    assert_eq!(
        pictures,
        [
            (Some("inline dot".to_owned()), true, "image/png".to_owned()),
            (Some("named dot".to_owned()), true, "image/png".to_owned()),
        ]
    );
    assert_eq!(
        texts(&doc)[0].split_whitespace().collect::<Vec<_>>(),
        ["a", "b", "c", "gone"]
    );

    // Without the supplied bytes, a named image is dropped the same way.
    let plain = Document::from_html("<p><img src=\"dot.png\" alt=\"named dot\"></p>").unwrap();
    assert_eq!(texts(&plain.document), ["named dot"]);
    assert!(plain.document.pictures().unwrap().is_empty());
    assert_eq!(plain.diagnostics.len(), 1);
}

#[test]
fn markdown_import_reads_commonmark_and_locates_what_it_drops_in_the_source() {
    let markdown = concat!(
        "# Head\n\n",
        "Some **bold**, *it*, ~~gone~~ and `code` with [a link](https://example.com/x), ",
        "[bad](javascript:alert(1)) and <span>raw</span>.\n\n",
        "> quoted\n\n",
        "```\nfenced\n```\n\n",
        "| A | B |\n| --- | --- |\n| 1 | x\\|y |\n",
    );
    let result = Document::from_markdown(markdown).unwrap();
    let dropped: Vec<(&str, &str)> = result
        .diagnostics
        .iter()
        .map(|diagnostic| {
            let (start, end) = diagnostic
                .location
                .strip_prefix("bytes ")
                .and_then(|range| range.split_once(".."))
                .unwrap_or_else(|| panic!("{diagnostic:?}"));
            (
                &markdown[start.parse::<usize>().unwrap()..end.parse::<usize>().unwrap()],
                diagnostic.message.as_str(),
            )
        })
        .collect();
    assert_eq!(
        dropped,
        [
            (
                "[bad](javascript:alert(1))",
                "dropped HTML link target and retained anchor text"
            ),
            ("<span>", "dropped raw Markdown HTML"),
            ("</span>", "dropped raw Markdown HTML"),
        ]
    );

    let doc = reopen(&mut { result.document });
    assert_eq!(
        texts(&doc),
        [
            "Head",
            "Some bold, it, gone and code with a link, bad and raw.",
            "quoted",
            "fenced\n"
        ]
    );
    let styles: Vec<Option<String>> = doc
        .paragraphs()
        .iter()
        .map(|paragraph| paragraph.style_id().map(str::to_owned))
        .collect();
    assert_eq!(
        styles,
        [
            Some("Heading1".to_owned()),
            None,
            Some("Quote".to_owned()),
            Some("HTMLPreformatted".to_owned()),
        ]
    );
    let body = doc.paragraph(1).unwrap();
    assert!(run_with_text(&body, "bold").is_bold());
    assert!(run_with_text(&body, "it").is_italic());
    assert!(run_with_text(&body, "gone").is_strike());
    assert_eq!(run_with_text(&body, "code").style_id(), Some("HTMLCode"));
    let urls: Vec<Option<String>> = body
        .hyperlink_spans()
        .into_iter()
        .map(|(_, _, relationship)| doc.hyperlink_url(relationship.expect("a relationship")))
        .collect();
    assert_eq!(urls, [Some("https://example.com/x".to_owned())]);
    let table = doc.table(0).unwrap();
    assert_eq!(table.cell(0, 1).unwrap().text(), "B");
    assert_eq!(table.cell(1, 1).unwrap().text(), "x|y");
}

#[test]
fn imports_into_a_template_keep_its_header_and_footer_and_replace_its_body() {
    let mut template = doc_with(&["Discard this template body"]);
    template.set_header("Keep header");
    template.set_footer("Keep footer");
    let mut template = reopen(&mut template);
    let before = template.to_bytes().unwrap();

    let from_markdown =
        Document::from_markdown_with_options("# Replacement\n\nBody", Some(&template), &[])
            .unwrap();
    let from_html =
        Document::from_html_with_options("<h2>Replacement</h2><p>Body</p>", Some(&template), &[])
            .unwrap();
    for (imported, heading) in [
        (from_markdown.document, "Heading1"),
        (from_html.document, "Heading2"),
    ] {
        let doc = reopen(&mut { imported });
        assert_eq!(texts(&doc), ["Replacement", "Body"]);
        assert_eq!(doc.paragraph(0).unwrap().style_id(), Some(heading));
        assert_eq!(doc.header_text().as_deref(), Some("Keep header"));
        assert_eq!(doc.footer_text().as_deref(), Some("Keep footer"));
    }
    assert!(
        template.to_bytes().unwrap() == before,
        "the template is left as it was"
    );
}
