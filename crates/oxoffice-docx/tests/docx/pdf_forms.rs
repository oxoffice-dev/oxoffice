//! Word content controls exported as fillable PDF form fields.

use lopdf::{Dictionary, Document as Pdf, Object};
use oxoffice_docx::{Document, LegacyFormFieldValue, RenderOptions};
use oxoffice_layout::{FormFieldKind, PositionedElement};

use crate::support::{corpus_doc, doc_with_body, reopen};

const W14: &str = r#"xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml""#;

/// A form as Word saves it: inline text, check box and list controls, a
/// block control, a signature control and one showing its placeholder.
fn form() -> Document {
    doc_with_body(&format!(
        r#"<w:p><w:r><w:t xml:space="preserve">Name: </w:t></w:r><w:sdt><w:sdtPr><w:alias w:val="Full name"/><w:tag w:val="name"/><w:id w:val="1"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Jane Doe</w:t></w:r></w:sdtContent></w:sdt></w:p>
<w:p><w:sdt {W14}><w:sdtPr><w:tag w:val="agree"/><w:id w:val="2"/><w14:checkbox><w14:checked w14:val="1"/><w14:checkedState w14:val="2612" w14:font="MS Gothic"/><w14:uncheckedState w14:val="2610" w14:font="MS Gothic"/></w14:checkbox></w:sdtPr><w:sdtContent><w:r><w:rPr><w:rFonts w:ascii="MS Gothic" w:hAnsi="MS Gothic" w:eastAsia="MS Gothic"/></w:rPr><w:t>&#x2612;</w:t></w:r></w:sdtContent></w:sdt><w:r><w:t xml:space="preserve"> I agree</w:t></w:r></w:p>
<w:p><w:r><w:t xml:space="preserve">Country: </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="country"/><w:id w:val="3"/><w:dropDownList><w:listItem w:displayText="Choose an item." w:value=""/><w:listItem w:displayText="France" w:value="fr"/><w:listItem w:displayText="Spain" w:value="es"/></w:dropDownList></w:sdtPr><w:sdtContent><w:r><w:t>France</w:t></w:r></w:sdtContent></w:sdt></w:p>
<w:sdt><w:sdtPr><w:tag w:val="notes"/><w:id w:val="4"/><w:text w:multiLine="1"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Some notes</w:t></w:r></w:p></w:sdtContent></w:sdt>
<w:p><w:r><w:t xml:space="preserve">City: </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="city"/><w:id w:val="5"/><w:showingPlcHdr/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Click here to enter text.</w:t></w:r></w:sdtContent></w:sdt></w:p>
<w:p><w:sdt><w:sdtPr><w:tag w:val="signature:client"/><w:id w:val="6"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Sign here</w:t></w:r></w:sdtContent></w:sdt></w:p>"#
    ))
}

fn fillable() -> RenderOptions {
    RenderOptions::default().with_fillable_forms(true)
}

/// The PDF's AcroForm fields by name.
fn fields(bytes: &[u8]) -> (Pdf, Vec<(String, Dictionary)>) {
    let pdf = Pdf::load_mem(bytes).expect("the PDF parses");
    let catalog = pdf.catalog().unwrap().clone();
    let form = pdf
        .dereference(catalog.get(b"AcroForm").unwrap())
        .unwrap()
        .1
        .as_dict()
        .unwrap()
        .clone();
    let fields = form
        .get(b"Fields")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|field| {
            let field = pdf.dereference(field).unwrap().1.as_dict().unwrap().clone();
            let name =
                String::from_utf8_lossy(field.get(b"T").unwrap().as_str().unwrap()).into_owned();
            (name, field)
        })
        .collect();
    (pdf, fields)
}

fn text(object: &Object) -> String {
    String::from_utf8_lossy(object.as_str().unwrap()).into_owned()
}

#[test]
fn a_control_in_a_header_stays_page_content() {
    let mut doc = doc_with_body(
        r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="body"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>In the body</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
    );
    doc.set_raw_header_with_images(
        format!(
            r#"<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" {W14}><w:p><w:sdt><w:sdtPr><w:tag w:val="approved"/><w14:checkbox><w14:checked w14:val="1"/><w14:checkedState w14:val="2612" w14:font="MS Gothic"/><w14:uncheckedState w14:val="2610" w14:font="MS Gothic"/></w14:checkbox></w:sdtPr><w:sdtContent><w:r><w:t>&#x2612;</w:t></w:r></w:sdtContent></w:sdt><w:r><w:t xml:space="preserve"> Approved</w:t></w:r></w:p></w:hdr>"#
        )
        .into_bytes(),
        &[],
        oxoffice_docx::HdrFtrType::Default,
    );
    let with_fields = doc.layout_with_options(fillable()).unwrap();
    let names: Vec<&str> = with_fields
        .layout
        .form_fields
        .iter()
        .map(|field| field.name.as_str())
        .collect();
    assert_eq!(names, ["body"], "the header control is no field");
    let without = doc.layout_with_options(RenderOptions::default()).unwrap();
    let drawn = drawing(&with_fields.layout.pages);
    assert_eq!(drawn, drawing(&without.layout.pages));
    assert!(
        drawn
            .iter()
            .any(|element| element.starts_with("text \"\u{2612}\"")),
        "the header still draws its box"
    );
}

/// The text runs of elements, wrappers looked through, joined.
fn joined_text(elements: &[PositionedElement]) -> String {
    let mut text = String::new();
    for element in elements {
        match element {
            PositionedElement::Text(run) => text.push_str(&run.text),
            PositionedElement::MultilingualText(run) => text.push_str(&run.logical_text),
            PositionedElement::MarkedContent { children, .. } => {
                text.push_str(&joined_text(children))
            }
            PositionedElement::Group(group) => text.push_str(&joined_text(&group.children)),
            PositionedElement::FormField(fragment) => {
                text.push_str(&joined_text(&fragment.children))
            }
            _ => {}
        }
    }
    text
}

/// What each field's fragments hold, fields in the order they first draw:
/// the field's number and the text its fragments draw, joined, as the
/// writer joins a field's fragments into one widget.
fn fragment_texts(pages: &[std::sync::Arc<oxoffice_layout::PageFrame>]) -> Vec<(u32, String)> {
    fn visit(elements: &[PositionedElement], out: &mut Vec<(u32, String)>) {
        for element in elements {
            match element {
                PositionedElement::FormField(fragment) => {
                    let text = joined_text(&fragment.children);
                    match out.iter_mut().find(|(field, _)| *field == fragment.field.0) {
                        Some((_, joined)) => joined.push_str(&text),
                        None => out.push((fragment.field.0, text)),
                    }
                }
                PositionedElement::MarkedContent { children, .. } => visit(children, out),
                PositionedElement::Group(group) => visit(&group.children, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    for page in pages {
        visit(&page.elements, &mut out);
    }
    out
}

/// The text the pages draw outside any fragment.
fn page_text(pages: &[std::sync::Arc<oxoffice_layout::PageFrame>]) -> String {
    fn visit(elements: &[PositionedElement], out: &mut String) {
        for element in elements {
            match element {
                PositionedElement::Text(run) => out.push_str(&run.text),
                PositionedElement::MultilingualText(run) => out.push_str(&run.logical_text),
                PositionedElement::MarkedContent { children, .. } => visit(children, out),
                PositionedElement::Group(group) => visit(&group.children, out),
                _ => {}
            }
        }
    }
    let mut out = String::new();
    for page in pages {
        visit(&page.elements, &mut out);
    }
    out
}

/// Every fragment's rectangle, in page order.
fn fragment_rects(
    pages: &[std::sync::Arc<oxoffice_layout::PageFrame>],
) -> Vec<oxoffice_layout::Rect> {
    fn visit(elements: &[PositionedElement], out: &mut Vec<oxoffice_layout::Rect>) {
        for element in elements {
            match element {
                PositionedElement::FormField(fragment) => out.push(fragment.rect),
                PositionedElement::MarkedContent { children, .. } => visit(children, out),
                PositionedElement::Group(group) => visit(&group.children, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    for page in pages {
        visit(&page.elements, &mut out);
    }
    out
}

/// How many filled rectangles the pages draw outside any fragment.
fn page_rects(pages: &[std::sync::Arc<oxoffice_layout::PageFrame>]) -> usize {
    fn visit(elements: &[PositionedElement]) -> usize {
        elements
            .iter()
            .map(|element| match element {
                PositionedElement::FilledRect { .. } => 1,
                PositionedElement::MarkedContent { children, .. } => visit(children),
                PositionedElement::Group(group) => visit(&group.children),
                _ => 0,
            })
            .sum()
    }
    pages.iter().map(|page| visit(&page.elements)).sum()
}

#[test]
fn a_hanging_first_line_gives_the_field_its_glyphs_and_keeps_its_shading() {
    let paragraph = |control: &str| {
        format!(
            r#"<w:p><w:pPr><w:shd w:val="clear" w:color="auto" w:fill="FFFF00"/><w:ind w:left="720" w:hanging="360"/></w:pPr>{control}</w:p>"#
        )
    };
    let doc = doc_with_body(&paragraph(
        r#"<w:sdt><w:sdtPr><w:tag w:val="hanging"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Hanging value</w:t></w:r></w:sdtContent></w:sdt>"#,
    ));
    let layout = doc.layout_with_options(fillable()).unwrap();
    let pages = &layout.layout.pages;
    assert_eq!(fragment_texts(pages), [(0, "Hanging value".to_owned())]);
    assert_eq!(page_text(pages), "", "the value draws in its widget only");
    let without = doc.layout_with_options(RenderOptions::default()).unwrap();
    assert!(
        page_rects(&without.layout.pages) > 0,
        "the paragraph is shaded"
    );
    assert_eq!(
        page_rects(pages),
        page_rects(&without.layout.pages),
        "the shading stays on the page"
    );
    // One field alone on its line: the widget spans the line, from the
    // first line's indent (1 inch margin + 0.25 inch) to the right margin.
    for rect in fragment_rects(pages) {
        assert!((rect.x - 90.0).abs() < 0.01, "{rect:?}");
        assert!((rect.x + rect.width - 540.0).abs() < 0.01, "{rect:?}");
    }
}

#[test]
fn a_tab_leader_inside_a_control_goes_with_it() {
    let doc = doc_with_body(
        r#"<w:p><w:pPr><w:tabs><w:tab w:val="left" w:leader="dot" w:pos="4320"/></w:tabs></w:pPr><w:r><w:t xml:space="preserve">Label </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="row"/><w:richText/></w:sdtPr><w:sdtContent><w:r><w:t>Name</w:t></w:r><w:r><w:tab/></w:r><w:r><w:t>Value</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    let pages = &layout.layout.pages;
    let fields = fragment_texts(pages);
    assert_eq!(fields.len(), 1);
    assert!(fields[0].1.starts_with("Name"), "{fields:?}");
    assert!(fields[0].1.ends_with("Value"), "{fields:?}");
    assert!(
        fields[0].1.contains('.'),
        "the leader is the field's: {fields:?}"
    );
    assert_eq!(
        page_text(pages),
        "Label ",
        "the label stays, the leader goes"
    );
}

fn diagnostics_about(layout: &oxoffice_layout::LayoutResult, name: &str) -> Vec<String> {
    layout
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.clone())
        .filter(|message| message.contains(&format!("{name:?}")))
        .collect()
}

#[test]
fn an_inline_control_that_wraps_beside_text_is_multiline_and_says_so() {
    let words = "wrapping words ".repeat(30);
    let doc = doc_with_body(&format!(
        r#"<w:p><w:r><w:t xml:space="preserve">Label: </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="beside"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t xml:space="preserve">{words}</w:t></w:r></w:sdtContent></w:sdt><w:r><w:t xml:space="preserve"> after</w:t></w:r></w:p>"#
    ));
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert!(matches!(
        layout.layout.form_fields[0].kind,
        FormFieldKind::Text {
            multiline: true,
            ..
        }
    ));
    let said = diagnostics_about(&layout.layout, "beside");
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("sits under its box"), "{said:?}");
    assert!(page_text(&layout.layout.pages).contains("Label:"));
}

#[test]
fn a_block_control_split_across_columns_stays_static_and_says_so() {
    // About 30 lines fill a column: 40 start the second on the same page.
    let paragraphs: String = (0..40)
        .map(|index| format!("<w:p><w:r><w:t>Line {index}</w:t></w:r></w:p>"))
        .collect();
    let doc = doc_with_body(&format!(
        r#"<w:sdt><w:sdtPr><w:tag w:val="columns"/><w:richText/></w:sdtPr><w:sdtContent>{paragraphs}</w:sdtContent></w:sdt><w:p><w:pPr><w:sectPr><w:type w:val="continuous"/><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/><w:cols w:num="2" w:space="720"/></w:sectPr></w:pPr></w:p>"#
    ));
    let layout = doc.layout_with_options(fillable()).unwrap();
    let first_page = page_text(&layout.layout.pages[..1]);
    assert!(
        first_page.contains("Line 0") && first_page.contains("Line 39"),
        "both columns on the first page: {first_page}"
    );
    assert!(fragment_texts(&layout.layout.pages).is_empty());
    let said = diagnostics_about(&layout.layout, "columns");
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("split across columns"), "{said:?}");
    let without = doc.layout_with_options(RenderOptions::default()).unwrap();
    assert_eq!(
        drawing(&layout.layout.pages),
        drawing(&without.layout.pages)
    );
}

fn field_names(layout: &oxoffice_layout::LayoutResult) -> Vec<&str> {
    layout
        .form_fields
        .iter()
        .map(|field| field.name.as_str())
        .collect()
}

/// One control of each place the spec keeps static, beside a control that
/// becomes a field, and what each one says.
#[test]
fn controls_in_rotated_cells_vertical_sections_or_around_floating_pictures_stay_static_and_say_so()
{
    let inline = |tag: &str, text: &str| {
        format!(
            r#"<w:sdt><w:sdtPr><w:tag w:val="{tag}"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>{text}</w:t></w:r></w:sdtContent></w:sdt>"#
        )
    };
    let rotated = format!(
        r#"<w:tbl><w:tblPr><w:tblW w:w="3000" w:type="dxa"/></w:tblPr><w:tblGrid><w:gridCol w:w="3000"/></w:tblGrid><w:tr><w:trPr><w:trHeight w:val="1500"/></w:trPr><w:tc><w:tcPr><w:tcW w:w="3000" w:type="dxa"/><w:textDirection w:val="btLr"/></w:tcPr><w:p>{}</w:p></w:tc></w:tr></w:tbl><w:p/>"#,
        inline("rotated", "Up")
    );
    let floating = r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="floating"/><w:richText/></w:sdtPr><w:sdtContent><w:r><w:t xml:space="preserve">Beside </w:t></w:r><w:r><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><mc:Choice Requires="wps"><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="1" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="column"><wp:posOffset>2743200</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="914400" cy="457200"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapNone/><wp:docPr id="70" name="Text Box 70"/><wp:cNvGraphicFramePr/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp><wps:cNvSpPr txBox="1"/><wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></wps:spPr><wps:txbx><w:txbxContent><w:p><w:r><w:t>Floating</w:t></w:r></w:p></w:txbxContent></wps:txbx><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing></mc:Choice></mc:AlternateContent></w:r></w:sdtContent></w:sdt></w:p>"#;
    let vertical = format!(
        r#"<w:p>{}</w:p><w:p><w:pPr><w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/><w:textDirection w:val="tbRl"/></w:sectPr></w:pPr></w:p>"#,
        inline("vertical", "Down")
    );
    // The section break ends a vertical section holding what comes before
    // it; the rest is in the document's last, horizontal, section.
    let doc = doc_with_body(&format!(
        r#"{vertical}<w:p>{}</w:p>{rotated}{floating}"#,
        inline("plain", "Value")
    ));
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert_eq!(field_names(&layout.layout), ["plain"]);
    for (name, reason) in [
        ("vertical", "its section's text is vertical"),
        ("rotated", "its cell's text is rotated"),
        ("floating", "it holds a floating picture"),
    ] {
        let said = diagnostics_about(&layout.layout, name);
        assert_eq!(said.len(), 1, "{name}: {said:?}");
        assert!(said[0].contains(reason), "{name}: {said:?}");
    }
    let without = doc.layout_with_options(RenderOptions::default()).unwrap();
    assert_eq!(
        drawing(&layout.layout.pages),
        drawing(&without.layout.pages)
    );
}

#[test]
fn a_block_field_may_hold_bookmarks_and_unmapped_wrappers() {
    let doc = doc_with_body(
        r#"<w:sdt><w:sdtPr><w:tag w:val="marked"/><w:richText/></w:sdtPr><w:sdtContent><w:bookmarkStart w:id="0" w:name="start"/><w:p><w:r><w:t>First</w:t></w:r></w:p><w:sdt><w:sdtPr><w:group/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Second</w:t></w:r></w:p></w:sdtContent></w:sdt><w:bookmarkEnd w:id="0"/></w:sdtContent></w:sdt>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert_eq!(field_names(&layout.layout), ["marked"]);
    assert!(matches!(
        &layout.layout.form_fields[0].kind,
        FormFieldKind::Text { value, multiline: true, .. } if value == "First\nSecond"
    ));
    assert_eq!(
        fragment_texts(&layout.layout.pages),
        [(0, "FirstSecond".to_owned())]
    );
}

#[test]
fn a_block_control_inside_a_cell_level_control_becomes_a_field() {
    let doc = doc_with_body(
        r#"<w:tbl><w:tblPr><w:tblW w:w="6000" w:type="dxa"/></w:tblPr><w:tblGrid><w:gridCol w:w="6000"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:w="6000" w:type="dxa"/></w:tcPr><w:sdt><w:sdtPr><w:tag w:val="outer"/><w:richText/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Outer text</w:t></w:r></w:p><w:sdt><w:sdtPr><w:tag w:val="inner"/><w:richText/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Inner text</w:t></w:r></w:p></w:sdtContent></w:sdt></w:sdtContent></w:sdt></w:tc></w:tr></w:tbl><w:p/>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert_eq!(
        field_names(&layout.layout),
        ["inner"],
        "the outer control holds a field, so it is a container"
    );
    assert_eq!(
        fragment_texts(&layout.layout.pages),
        [(0, "Inner text".to_owned())]
    );
    assert!(page_text(&layout.layout.pages).contains("Outer text"));
}

#[test]
fn a_control_in_a_right_to_left_paragraph_takes_only_its_text() {
    let doc = doc_with_body(
        r#"<w:p><w:pPr><w:bidi/></w:pPr><w:r><w:rPr><w:rtl/></w:rPr><w:t xml:space="preserve">שם: </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="name"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Jane</w:t></w:r></w:sdtContent></w:sdt><w:r><w:rPr><w:rtl/></w:rPr><w:t xml:space="preserve"> סוף</w:t></w:r></w:p>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert_eq!(
        fragment_texts(&layout.layout.pages),
        [(0, "Jane".to_owned())]
    );
    let page = page_text(&layout.layout.pages);
    assert!(
        !page.contains("Jane") && page.contains('ש') && page.contains('ס'),
        "{page}"
    );
    assert_eq!(
        layout.layout.form_fields[0].alignment,
        oxoffice_layout::FieldAlignment::Right,
        "a right-to-left paragraph starts on the right"
    );
    let without = doc.layout_with_options(RenderOptions::default()).unwrap();
    assert_eq!(
        drawing(&layout.layout.pages),
        drawing(&without.layout.pages)
    );
}

/// A one-pixel PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

/// A document holding a one-pixel picture, its body built around the
/// picture's relationship id.
fn doc_with_picture(body: impl Fn(&str) -> String) -> Document {
    use std::io::Cursor;
    let mut doc = Document::new();
    let relationship = doc.embed_image(PNG, "picture.png");
    let bytes = doc.to_bytes().expect("document saves");
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let part = package.main_document_part().expect("main document part");
    let xml = String::from_utf8(package.get_part(&part).expect("main part").to_vec()).unwrap();
    let start = xml.find("<w:body>").unwrap() + "<w:body>".len();
    let end = xml
        .find("<w:sectPr")
        .or_else(|| xml.find("</w:body>"))
        .unwrap();
    let xml = format!("{}{}{}", &xml[..start], body(&relationship), &xml[end..]);
    package.set_part(&part, xml.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    Document::from_bytes(&out.into_inner()).expect("document opens")
}

/// An inline picture of this size in points, with alternative text when
/// `description` is given.
fn inline_picture(relationship: &str, width: u32, height: u32, description: &str) -> String {
    let (cx, cy) = (width * 12700, height * 12700);
    format!(
        r#"<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="10" name="Picture 10" descr="{description}"/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:nvPicPr><pic:cNvPr id="10" name="picture.png" descr="{description}"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{relationship}" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#
    )
}

#[test]
fn a_signature_over_a_described_picture_is_a_signature_field() {
    let doc = doc_with_picture(|relationship| {
        format!(
            r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="signature"/><w:picture/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt></w:p>"#,
            inline_picture(relationship, 144, 36, "Sign here")
        )
    });
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert_eq!(field_names(&layout.layout), ["signature"]);
    assert!(matches!(
        layout.layout.form_fields[0].kind,
        FormFieldKind::Signature
    ));
    assert!(diagnostics_about(&layout.layout, "signature").is_empty());
    let (_, exported) = fields(&doc.to_pdf_with_options(fillable()).unwrap());
    assert_eq!(exported[0].1.get(b"FT").unwrap().as_name().unwrap(), b"Sig");
}

#[test]
fn a_field_in_a_vertically_centred_section_keeps_its_widget_on_its_text() {
    let doc = doc_with_body(
        r#"<w:sdt><w:sdtPr><w:tag w:val="title"/><w:richText/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Centred title</w:t></w:r></w:p></w:sdtContent></w:sdt><w:p><w:pPr><w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/><w:vAlign w:val="center"/></w:sectPr></w:pPr></w:p>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    fn glyphs(elements: &[PositionedElement]) -> Vec<f64> {
        elements
            .iter()
            .flat_map(|element| match element {
                PositionedElement::Text(run) => vec![run.origin.y],
                PositionedElement::MarkedContent { children, .. } => glyphs(children),
                _ => Vec::new(),
            })
            .collect()
    }
    fn fragments(elements: &[PositionedElement]) -> Vec<&oxoffice_layout::FormFieldFragment> {
        elements
            .iter()
            .flat_map(|element| match element {
                PositionedElement::FormField(fragment) => vec![fragment],
                PositionedElement::MarkedContent { children, .. } => fragments(children),
                _ => Vec::new(),
            })
            .collect()
    }
    let found = fragments(&layout.layout.pages[0].elements);
    assert!(!found.is_empty());
    for fragment in found {
        let rect = fragment.rect;
        assert!(rect.y > 300.0, "centred, not at the top: {rect:?}");
        for baseline in glyphs(&fragment.children) {
            assert!(
                baseline >= rect.y && baseline <= rect.y + rect.height,
                "the widget covers its glyphs: {rect:?}, baseline {baseline}"
            );
        }
    }
}

#[test]
fn controls_in_tracked_changes_follow_the_accepted_view() {
    let control = |tag: &str, text: &str| {
        format!(
            r#"<w:sdt><w:sdtPr><w:tag w:val="{tag}"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>{text}</w:t></w:r></w:sdtContent></w:sdt>"#
        )
    };
    let deleted =
        control("deleted", "Gone").replace("<w:t>Gone</w:t>", "<w:delText>Gone</w:delText>");
    let doc = doc_with_body(&format!(
        r#"<w:p><w:ins w:id="1" w:author="a" w:date="2026-01-01T00:00:00Z">{}</w:ins><w:r><w:t xml:space="preserve"> and </w:t></w:r><w:del w:id="2" w:author="a" w:date="2026-01-01T00:00:00Z">{deleted}</w:del></w:p>"#,
        control("inserted", "Kept")
    ));
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert_eq!(field_names(&layout.layout), ["inserted"]);
    assert_eq!(
        fragment_texts(&layout.layout.pages),
        [(0, "Kept".to_owned())]
    );
    let said = diagnostics_about(&layout.layout, "deleted");
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("a tracked change deletes it"), "{said:?}");
}

#[test]
fn a_block_control_around_a_table_is_a_container() {
    let doc = doc_with_body(
        r#"<w:sdt><w:sdtPr><w:tag w:val="around"/><w:richText/></w:sdtPr><w:sdtContent><w:tbl><w:tblPr><w:tblW w:w="6000" w:type="dxa"/></w:tblPr><w:tblGrid><w:gridCol w:w="6000"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:w="6000" w:type="dxa"/></w:tcPr><w:p><w:r><w:t xml:space="preserve">Cell: </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="inside"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Value</w:t></w:r></w:sdtContent></w:sdt></w:p></w:tc></w:tr></w:tbl></w:sdtContent></w:sdt><w:p/>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert_eq!(field_names(&layout.layout), ["inside"]);
    assert_eq!(
        fragment_texts(&layout.layout.pages),
        [(0, "Value".to_owned())]
    );
    assert_eq!(page_text(&layout.layout.pages), "Cell: ");
}

#[test]
fn a_leader_before_a_control_keeps_the_widget_to_the_control() {
    let doc = doc_with_body(
        r#"<w:p><w:pPr><w:tabs><w:tab w:val="left" w:leader="dot" w:pos="4320"/></w:tabs></w:pPr><w:r><w:tab/></w:r><w:sdt><w:sdtPr><w:tag w:val="after"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Value</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    let pages = &layout.layout.pages;
    assert_eq!(fragment_texts(pages), [(0, "Value".to_owned())]);
    assert!(
        page_text(pages).contains('.'),
        "the leader stays on the page"
    );
    for rect in fragment_rects(pages) {
        assert!(
            rect.x > 72.0 + 200.0,
            "the widget starts at the control, not over the leader: {rect:?}"
        );
    }
}

#[test]
fn a_block_field_straddling_a_vertical_section_stays_static_as_a_whole() {
    // The first paragraph ends a vertical section, the second is in the
    // document's last, horizontal, section.
    let doc = doc_with_body(
        r#"<w:sdt><w:sdtPr><w:tag w:val="straddle"/><w:richText/></w:sdtPr><w:sdtContent><w:p><w:pPr><w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/><w:textDirection w:val="tbRl"/></w:sectPr></w:pPr><w:r><w:t>Down</w:t></w:r></w:p><w:p><w:r><w:t>Across</w:t></w:r></w:p></w:sdtContent></w:sdt>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert!(fragment_texts(&layout.layout.pages).is_empty());
    let said = diagnostics_about(&layout.layout, "straddle");
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].contains("its section's text is vertical"),
        "{said:?}"
    );
}

/// What a page draws, fragments looked through and sources ignored: the
/// static drawing to compare with and without fields.
fn drawing(pages: &[std::sync::Arc<oxoffice_layout::PageFrame>]) -> Vec<String> {
    fn visit(elements: &[PositionedElement], out: &mut Vec<String>) {
        for element in elements {
            match element {
                PositionedElement::Text(run) => out.push(format!(
                    "text {:?} {:.2},{:.2} {} {:?}",
                    run.text, run.origin.x, run.origin.y, run.font_size, run.glyph_ids
                )),
                PositionedElement::Line { start, end, .. } => out.push(format!(
                    "line {:.2},{:.2} {:.2},{:.2}",
                    start.x, start.y, end.x, end.y
                )),
                PositionedElement::FilledRect { rect, .. }
                | PositionedElement::Image { rect, .. } => out.push(format!(
                    "rect {:.2},{:.2} {:.2}x{:.2}",
                    rect.x, rect.y, rect.width, rect.height
                )),
                PositionedElement::MarkedContent { children, .. } => visit(children, out),
                PositionedElement::FormField(fragment) => visit(&fragment.children, out),
                PositionedElement::Group(group) => visit(&group.children, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    for page in pages {
        visit(&page.elements, &mut out);
        out.push("page".to_owned());
    }
    out
}

#[test]
fn an_edited_value_is_the_next_field_value() {
    let mut doc = form();
    assert!(matches!(
        &doc.layout_with_options(fillable()).unwrap().layout.form_fields[0].kind,
        FormFieldKind::Text { value, .. } if value == "Jane Doe"
    ));
    doc.set_content_control_value_by_tag("name", "Bob").unwrap();
    assert!(matches!(
        &doc.layout_with_options(fillable()).unwrap().layout.form_fields[0].kind,
        FormFieldKind::Text { value, .. } if value == "Bob"
    ));
}

#[test]
fn a_pdfa_export_keeps_the_fields() {
    let doc = form();
    let bytes = doc
        .to_pdfa_deterministic_with_options(oxoffice_docx::PdfConformance::PdfA2b, fillable())
        .unwrap();
    let (_, fields) = fields(&bytes);
    assert_eq!(fields.len(), 6);
    assert!(
        fields
            .iter()
            .all(|(_, field)| field.get(b"F").unwrap().as_i64().unwrap() == 4)
    );
}

#[test]
fn a_control_whose_properties_do_not_decode_stays_static_and_says_so() {
    let doc = doc_with_body(&format!(
        r#"<w:p><w:sdt {W14}><w:sdtPr><w:tag w:val="broken"/><w14:checkbox><w14:checked w14:val="maybe"/><w14:checkedState w14:val="2612" w14:font="MS Gothic"/><w14:uncheckedState w14:val="2610" w14:font="MS Gothic"/></w14:checkbox></w:sdtPr><w:sdtContent><w:r><w:t>&#x2610;</w:t></w:r></w:sdtContent></w:sdt></w:p><w:p><w:sdt><w:sdtPr><w:tag w:val="fine"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Value</w:t></w:r></w:sdtContent></w:sdt></w:p>"#
    ));
    let layout = doc.layout_with_options(fillable()).unwrap();
    let names: Vec<&str> = layout
        .layout
        .form_fields
        .iter()
        .map(|field| field.name.as_str())
        .collect();
    assert_eq!(names, ["fine"], "the broken control is no field");
    let said: Vec<&String> = layout
        .layout
        .diagnostics
        .iter()
        .map(|diagnostic| &diagnostic.message)
        .filter(|message| message.contains("\"broken\""))
        .collect();
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        doc.to_pdf_with_options(fillable()).is_ok(),
        "the export still succeeds"
    );
}

/// A legacy form field as Word saves it: the form data on the field's begin
/// mark, a bookmark of the same name around it, and its result, if any.
fn legacy(id: u32, name: &str, data: &str, instruction: &str, result: Option<&str>) -> String {
    let result = result.map_or_else(String::new, |text| {
        format!(
            r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#
        )
    });
    format!(
        r#"<w:bookmarkStart w:id="{id}" w:name="{name}"/><w:r><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="{name}"/>{data}</w:ffData></w:fldChar></w:r><w:r><w:instrText xml:space="preserve"> {instruction} </w:instrText></w:r>{result}<w:r><w:fldChar w:fldCharType="end"/></w:r><w:bookmarkEnd w:id="{id}"/>"#
    )
}

/// A legacy form shaped as Word saves it: a text field with help text and a
/// maximum length, a check box, a drop-down, a disabled field, a computed
/// date and an empty text field. The drop-down stores its result; the
/// corpus form's has none, as Word saves it.
fn legacy_form() -> Document {
    let company = legacy(
        0,
        "Company",
        r#"<w:enabled/><w:calcOnExit w:val="0"/><w:helpText w:type="text" w:val="Legal name"/><w:textInput><w:maxLength w:val="40"/></w:textInput>"#,
        "FORMTEXT",
        Some("Acme"),
    );
    let check = legacy(
        1,
        "Check1",
        r#"<w:enabled/><w:calcOnExit w:val="0"/><w:checkBox><w:sizeAuto/><w:default w:val="1"/></w:checkBox>"#,
        "FORMCHECKBOX",
        None,
    );
    let colour = legacy(
        2,
        "Colour",
        r#"<w:enabled/><w:calcOnExit w:val="0"/><w:ddList><w:result w:val="1"/><w:listEntry w:val="Red"/><w:listEntry w:val="Green"/><w:listEntry w:val="Blue"/></w:ddList>"#,
        "FORMDROPDOWN",
        Some("Green"),
    );
    let reference = legacy(
        3,
        "Reference",
        r#"<w:enabled w:val="0"/><w:calcOnExit w:val="0"/><w:textInput/>"#,
        "FORMTEXT",
        Some("R-12"),
    );
    let today = legacy(
        4,
        "Today",
        r#"<w:enabled/><w:calcOnExit w:val="0"/><w:textInput><w:type w:val="currentDate"/><w:format w:val="dd/MM/yyyy"/></w:textInput>"#,
        "FORMTEXT",
        Some("21/11/2026"),
    );
    let city = legacy(
        5,
        "City",
        r#"<w:enabled/><w:calcOnExit w:val="0"/><w:textInput/>"#,
        "FORMTEXT",
        Some("&#8194;&#8194;&#8194;&#8194;&#8194;"),
    );
    doc_with_body(&format!(
        r#"<w:p><w:r><w:t xml:space="preserve">Company: </w:t></w:r>{company}</w:p>
<w:p>{check}<w:r><w:t xml:space="preserve"> Subscribe</w:t></w:r></w:p>
<w:p><w:r><w:t xml:space="preserve">Colour: </w:t></w:r>{colour}</w:p>
<w:p><w:r><w:t xml:space="preserve">Reference: </w:t></w:r>{reference}<w:r><w:t xml:space="preserve"> Date: </w:t></w:r>{today}</w:p>
<w:p><w:r><w:t xml:space="preserve">City: </w:t></w:r>{city}</w:p>"#
    ))
}

fn names_of(doc: &Document) -> Vec<String> {
    doc.layout_with_options(fillable())
        .unwrap()
        .layout
        .form_fields
        .iter()
        .map(|field| field.name.clone())
        .collect()
}

#[test]
fn legacy_form_fields_become_named_fields_with_their_values() {
    let doc = legacy_form();
    let layout = doc.layout_with_options(fillable()).unwrap();
    let fields_out = &layout.layout.form_fields;
    let names: Vec<&str> = fields_out.iter().map(|field| field.name.as_str()).collect();
    assert_eq!(
        names,
        ["Company", "Check1", "Colour", "Reference", "Today", "City"]
    );
    assert!(matches!(
        &fields_out[0].kind,
        FormFieldKind::Text { value, max_len: Some(40), multiline: false } if value == "Acme"
    ));
    assert_eq!(fields_out[0].tooltip.as_deref(), Some("Legal name"));
    assert!(matches!(
        fields_out[1].kind,
        FormFieldKind::CheckBox { checked: true }
    ));
    assert!(matches!(
        &fields_out[2].kind,
        FormFieldKind::Choice { options, value: Some(value), editable: false }
            if value == "Green" && options.len() == 3 && options[0].value == "Red"
    ));
    assert!(
        matches!(&fields_out[5].kind, FormFieldKind::Text { value, .. } if value.is_empty()),
        "five en spaces are Word's empty text field"
    );

    let (_, fields) = fields(&doc.to_pdf_with_options(fillable()).unwrap());
    let by_name = |name: &str| {
        fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, dict)| dict.clone())
            .unwrap_or_else(|| panic!("no field {name}"))
    };
    assert_eq!(text(by_name("Company").get(b"V").unwrap()), "Acme");
    assert_eq!(
        by_name("Company").get(b"MaxLen").unwrap().as_i64().unwrap(),
        40
    );
    assert_eq!(text(by_name("Company").get(b"TU").unwrap()), "Legal name");
    assert_eq!(
        by_name("Check1").get(b"V").unwrap().as_name().unwrap(),
        b"Yes"
    );
    assert_eq!(text(by_name("Colour").get(b"V").unwrap()), "Green");
    let read_only = |name: &str| {
        by_name(name)
            .get(b"Ff")
            .ok()
            .and_then(|flags| flags.as_i64().ok())
            .is_some_and(|flags| flags & 1 == 1)
    };
    assert!(read_only("Reference"), "a disabled field is read only");
    assert!(read_only("Today"), "a computed date is read only");
    assert!(!read_only("Company"));
}

/// Name and kind of the fillable fields made from the corpus form's legacy
/// fields, as the layout gives them.
fn legacy_kinds(doc: &Document) -> Vec<(String, FormFieldKind)> {
    const LEGACY: [&str; 6] = [
        "BankName",
        "IBAN",
        "BIC",
        "PaymentTerms",
        "RemittanceEmail",
        "Declaration",
    ];
    doc.layout_with_options(fillable())
        .unwrap()
        .layout
        .form_fields
        .iter()
        .filter(|field| LEGACY.contains(&field.name.as_str()))
        .map(|field| (field.name.clone(), field.kind.clone()))
        .collect()
}

#[test]
fn legacy_form_fields_saved_by_libreoffice_become_the_fields_word_saves() {
    // LibreOffice puts each field's bookmark among its instruction runs or at
    // the start of its result, where Word puts it around the field.
    let word = legacy_kinds(&corpus_doc("authored/form.docx"));
    assert_eq!(word.len(), 6, "{word:?}");
    let mut doc = corpus_doc("libreoffice/form.docx");
    assert_eq!(legacy_kinds(&doc), word);

    // Filled, saved and opened again, they keep LibreOffice's bookmarks.
    let bookmarks = |doc: &Document| {
        doc.bookmarks()
            .iter()
            .map(|bookmark| bookmark.name().map(str::to_owned))
            .collect::<Vec<_>>()
    };
    let before = bookmarks(&doc);
    let part = doc
        .legacy_form_fields()
        .unwrap()
        .remove(0)
        .source_part
        .clone();
    for (ordinal, value) in [
        (0, LegacyFormFieldValue::Text("HSBC UK".into())),
        (3, LegacyFormFieldValue::SelectedIndex(2)),
    ] {
        doc.set_legacy_form_field_value(&part, ordinal, value)
            .unwrap();
    }
    // An edit elsewhere in the document, then the saved XML: each bookmark
    // is still where LibreOffice put it, inside its field.
    doc.add_paragraph("Edited elsewhere");
    let saved = doc.to_bytes().unwrap();
    let package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(saved)).unwrap();
    let part = package.main_document_part().unwrap();
    let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
    for (filled, placed) in [
        (
            true,
            r#"FORMTEXT </w:instrText></w:r><w:bookmarkStart w:id="0" w:name="BankName_Copy_1"/>"#,
        ),
        (
            false,
            r#"FORMTEXT </w:instrText></w:r><w:bookmarkStart w:id="1" w:name="IBAN_Copy_1"/>"#,
        ),
        (
            true,
            r#"<w:fldChar w:fldCharType="separate"/></w:r><w:bookmarkStart w:id="3" w:name="PaymentTerms_Copy_1"/>"#,
        ),
    ] {
        assert!(xml.contains(placed), "filled {filled}: {placed}");
    }
    let doc = reopen(&mut doc);
    assert_eq!(bookmarks(&doc), before);
    let values = legacy_kinds(&doc);
    assert!(
        matches!(&values[0].1, FormFieldKind::Text { value, .. } if value == "HSBC UK"),
        "{values:?}"
    );
    assert!(
        matches!(&values[3].1, FormFieldKind::Choice { value: Some(value), .. } if value == "60 days"),
        "{values:?}"
    );
}

#[test]
fn a_legacy_field_whose_form_data_does_not_read_stays_static_and_says_its_name() {
    // The selected entry is past the end of the list.
    let size = legacy(
        0,
        "Size",
        r#"<w:enabled/><w:calcOnExit w:val="0"/><w:ddList><w:result w:val="5"/><w:listEntry w:val="S"/><w:listEntry w:val="M"/></w:ddList>"#,
        "FORMDROPDOWN",
        Some("M"),
    );
    let doc = doc_with_body(&format!("<w:p>{size}</w:p>"));
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert!(field_names(&layout.layout).is_empty());
    let said: Vec<&str> = layout
        .layout
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .filter(|message| message.contains("stays static"))
        .collect();
    assert_eq!(
        said,
        ["form field \"Size\" stays static: its form data does not read"]
    );
}

#[test]
fn a_control_around_a_legacy_field_is_a_container() {
    let inner = legacy(
        0,
        "Inner",
        r#"<w:enabled/><w:calcOnExit w:val="0"/><w:textInput/>"#,
        "FORMTEXT",
        Some("typed"),
    );
    let doc = doc_with_body(&format!(
        r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="wrapper"/><w:richText/></w:sdtPr><w:sdtContent><w:r><w:t xml:space="preserve">Name: </w:t></w:r>{inner}</w:sdtContent></w:sdt></w:p>
<w:sdt><w:sdtPr><w:tag w:val="block"/><w:richText/></w:sdtPr><w:sdtContent><w:p>{}</w:p></w:sdtContent></w:sdt>"#,
        legacy(
            1,
            "InBlock",
            r#"<w:enabled/><w:calcOnExit w:val="0"/><w:textInput/>"#,
            "FORMTEXT",
            Some("more"),
        )
    ));
    assert_eq!(names_of(&doc), ["Inner", "InBlock"]);
    let (_, fields) = fields(&doc.to_pdf_with_options(fillable()).unwrap());
    let found: Vec<&str> = fields.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(found, ["Inner", "InBlock"], "both get a widget");
}

#[test]
fn a_legacy_field_inside_another_field_stays_static_and_says_so() {
    let inner = r#"<w:r><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="Nested"/><w:enabled/><w:calcOnExit w:val="0"/><w:textInput/></w:ffData></w:fldChar></w:r><w:r><w:instrText xml:space="preserve"> FORMTEXT </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>inner</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#;
    let doc = doc_with_body(&format!(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> QUOTE "x" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{inner}<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
    ));
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert!(layout.layout.form_fields.is_empty());
    let said = layout
        .layout
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.message.contains("\"Nested\" stays static"))
        .count();
    assert_eq!(said, 1);
}

fn named_values(doc: &Document) -> Vec<(String, String)> {
    doc.layout_with_options(fillable())
        .unwrap()
        .layout
        .form_fields
        .iter()
        .map(|field| match &field.kind {
            FormFieldKind::Text { value, .. } => (field.name.clone(), value.clone()),
            other => panic!("{other:?}"),
        })
        .collect()
}

#[test]
fn a_control_in_a_tracked_change_inside_a_hyperlink_follows_document_order() {
    let doc = doc_with_body(
        r#"<w:p><w:hyperlink w:anchor="top"><w:ins w:id="1" w:author="a" w:date="2026-01-01T00:00:00Z"><w:sdt><w:sdtPr><w:tag w:val="name"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Jane</w:t></w:r></w:sdtContent></w:sdt></w:ins></w:hyperlink><w:r><w:t xml:space="preserve"> </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="name"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Doe</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
    );
    assert_eq!(
        named_values(&doc),
        [
            ("name".to_owned(), "Jane".to_owned()),
            ("name_2".to_owned(), "Doe".to_owned())
        ]
    );
}

#[test]
fn a_control_whose_text_is_only_a_tracked_insertion_follows_a_legacy_field_before_it() {
    let first = legacy(
        0,
        "First",
        r#"<w:enabled/><w:calcOnExit w:val="0"/><w:textInput/>"#,
        "FORMTEXT",
        Some("one"),
    );
    let doc = doc_with_body(&format!(
        r#"<w:p>{first}<w:r><w:t xml:space="preserve"> </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="late"/><w:text/></w:sdtPr><w:sdtContent><w:ins w:id="9" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:t>two</w:t></w:r></w:ins></w:sdtContent></w:sdt></w:p>"#
    ));
    assert_eq!(names_of(&doc), ["First", "late"]);
}

#[test]
fn a_control_with_no_run_after_a_legacy_field_takes_the_later_name() {
    let first = legacy(
        0,
        "First",
        r#"<w:enabled/><w:calcOnExit w:val="0"/><w:textInput/>"#,
        "FORMTEXT",
        Some("one"),
    );
    let doc = doc_with_body(&format!(
        r#"<w:p>{first}<w:r><w:t xml:space="preserve"> </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="later"/><w:text/></w:sdtPr><w:sdtContent/></w:sdt></w:p>"#
    ));
    assert_eq!(names_of(&doc), ["First", "later"]);
}

/// The fields' names with the value each shows, or the kind of a field
/// that is not text.
fn names_and_values(doc: &Document) -> Vec<(String, String)> {
    doc.layout_with_options(fillable())
        .unwrap()
        .layout
        .form_fields
        .iter()
        .map(|field| match &field.kind {
            FormFieldKind::Text { value, .. } => (field.name.clone(), value.clone()),
            other => (field.name.clone(), format!("{other:?}")),
        })
        .collect()
}

#[test]
fn an_empty_control_after_a_tracked_insertion_in_a_container_takes_the_later_name() {
    // The empty control has no run: it orders by the controls listed
    // before it, the inserted one included.
    let doc = doc_with_body(
        r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="person"/><w:richText/></w:sdtPr><w:sdtContent><w:ins w:id="1" w:author="a" w:date="2026-01-01T00:00:00Z"><w:sdt><w:sdtPr><w:tag w:val="name"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Jane</w:t></w:r></w:sdtContent></w:sdt></w:ins><w:sdt><w:sdtPr><w:tag w:val="name"/><w:text/></w:sdtPr><w:sdtContent/></w:sdt></w:sdtContent></w:sdt></w:p>"#,
    );
    let named = names_and_values(&doc);
    assert_eq!(
        named[0],
        ("name".to_owned(), "Jane".to_owned()),
        "{named:?}"
    );
}

fn typing_diagnostics(layout: &oxoffice_layout::LayoutResult) -> Vec<String> {
    layout
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.clone())
        .filter(|message| message.contains("typing font"))
        .collect()
}

/// Text controls holding `values`, tagged `field0`, `field1`...
fn text_controls(values: &[&str]) -> Document {
    doc_with_body(
        &values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                format!(
                    r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="field{index}"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t xml:space="preserve">{value}</w:t></w:r></w:sdtContent></w:sdt></w:p>"#
                )
            })
            .collect::<String>(),
    )
}

#[test]
fn a_field_holding_text_the_typing_font_cannot_encode_names_its_scripts() {
    let doc = text_controls(&["Москва", "Łódź 東京", "Crème brûlée, 5 € – œuvre"]);
    let layout = doc.layout_with_options(fillable()).unwrap();
    let said = typing_diagnostics(&layout.layout);
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(
        said[0].contains("\"field0\" holds Cyrillic text"),
        "{said:?}"
    );
    assert!(said[0].contains("6 distinct characters"), "{said:?}");
    // Ł and ź are Latin letters WinAnsi lacks; ó is in WinAnsi.
    assert!(
        said[1].contains("\"field1\" holds Han, Latin text"),
        "{said:?}"
    );
    // The sample is in code point order, like the glyph coverage ones.
    assert!(said[1].contains("such as \"Łź京東\""), "{said:?}");

    let (_, diagnostics) = doc.to_pdf_with_diagnostics(&[], fillable()).unwrap();
    let from_pdf: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.message.contains("typing font"))
        .collect();
    assert_eq!(from_pdf.len(), 2, "{diagnostics:?}");

    let static_export = doc.layout_with_options(RenderOptions::default()).unwrap();
    assert!(typing_diagnostics(&static_export.layout).is_empty());
}

#[test]
fn a_field_names_the_typing_font_gap_under_its_pdf_name() {
    let doc = doc_with_body(
        r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="a.b"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Ωμέγα</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    let said = typing_diagnostics(&layout.layout);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("\"a_b\" holds Greek text"), "{said:?}");
}

#[test]
fn list_options_the_typing_font_cannot_encode_are_named_even_when_not_chosen() {
    let doc = doc_with_body(
        r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="country"/><w:dropDownList><w:listItem w:displayText="France" w:value="fr"/><w:listItem w:displayText="Ελλάδα" w:value="gr"/></w:dropDownList></w:sdtPr><w:sdtContent><w:r><w:t>France</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    let said = typing_diagnostics(&layout.layout);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("\"country\" holds Greek text"), "{said:?}");
}

#[test]
fn locked_fields_and_placeholders_say_nothing_about_the_typing_font() {
    let doc = doc_with_body(
        r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="locked"/><w:lock w:val="sdtContentLocked"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Москва</w:t></w:r></w:sdtContent></w:sdt></w:p>
<w:p><w:sdt><w:sdtPr><w:tag w:val="prompt"/><w:showingPlcHdr/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>点击这里</w:t></w:r></w:sdtContent></w:sdt></w:p>
<w:p><w:sdt><w:sdtPr><w:tag w:val="notes"/><w:text w:multiLine="1"/></w:sdtPr><w:sdtContent><w:r><w:t>one</w:t><w:br/><w:t>two</w:t><w:tab/><w:t>three</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert_eq!(field_names(&layout.layout).len(), 3);
    assert!(
        typing_diagnostics(&layout.layout).is_empty(),
        "{:?}",
        typing_diagnostics(&layout.layout)
    );
}

#[test]
fn a_list_value_counts_only_when_typed_not_when_it_is_an_option_export_value() {
    let doc = doc_with_body(
        r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="exported"/><w:dropDownList><w:listItem w:displayText="Greece" w:value="Ελλάδα"/><w:listItem w:displayText="France" w:value="fr"/></w:dropDownList></w:sdtPr><w:sdtContent><w:r><w:t>Greece</w:t></w:r></w:sdtContent></w:sdt></w:p>
<w:p><w:sdt><w:sdtPr><w:tag w:val="typed"/><w:comboBox><w:listItem w:displayText="Paris" w:value="paris"/></w:comboBox></w:sdtPr><w:sdtContent><w:r><w:t>Αθήνα</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    let said = typing_diagnostics(&layout.layout);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("\"typed\" holds Greek text"), "{said:?}");
}

/// A text control tagged `tag` showing `value`.
fn text_control(tag: &str, value: &str) -> String {
    format!(
        r#"<w:sdt><w:sdtPr><w:tag w:val="{tag}"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>{value}</w:t></w:r></w:sdtContent></w:sdt>"#
    )
}

/// A document whose first paragraph references an endnote, then a footnote,
/// each holding a text control after its own text, and whose second
/// paragraph holds a control of the body. All three are tagged `name`.
fn doc_with_note_controls() -> Document {
    doc_with_notes(|xml, marker, value| {
        xml.replace(
            &format!("{marker}</w:t></w:r>"),
            &format!("{marker} </w:t></w:r>{}", text_control("name", value)),
        )
    })
}

/// A document whose first paragraph references an endnote, then a footnote,
/// and whose second paragraph holds a control of the body tagged `name`.
/// `note` rewrites each notes part, given the part, the text of its note
/// and the value a control there shows.
fn doc_with_notes(note: impl Fn(&str, &str, &str) -> String) -> Document {
    let mut doc = doc_with_body(&format!(
        r#"<w:p><w:r><w:t>Intro</w:t></w:r></w:p><w:p>{}</w:p>"#,
        text_control("name", "Body value")
    ));
    let body = doc.paragraph_story_location(0).unwrap().unwrap();
    doc.create_endnote(&body, "ENDNOTE").unwrap();
    let body = doc.paragraph_story_location(0).unwrap().unwrap();
    doc.create_footnote(&body, "FOOTNOTE").unwrap();
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(doc.to_bytes().unwrap()))
            .unwrap();
    for (part, marker, value) in [
        ("/word/footnotes.xml", "FOOTNOTE", "Foot value"),
        ("/word/endnotes.xml", "ENDNOTE", "End value"),
    ] {
        let xml = String::from_utf8(package.get_part(part).unwrap().to_vec()).unwrap();
        assert!(xml.contains(&format!("{marker}</w:t></w:r>")), "{xml}");
        let xml = note(&xml, marker, value);
        package.set_part(part, xml.into_bytes());
    }
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    Document::from_bytes(&out.into_inner()).unwrap()
}

#[test]
fn controls_in_notes_become_fields_after_the_body_footnotes_first() {
    let doc = doc_with_note_controls();
    assert_eq!(
        named_values(&doc),
        [
            ("name".to_owned(), "Body value".to_owned()),
            ("name_2".to_owned(), "Foot value".to_owned()),
            ("name_3".to_owned(), "End value".to_owned()),
        ]
    );
    let result = doc.layout_with_options(fillable()).unwrap();
    let layout = &result.layout;
    let mut texts: Vec<String> = fragment_texts(&layout.pages)
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    texts.sort();
    assert_eq!(texts, ["Body value", "End value", "Foot value"]);
    // The notes' own text and marks stay on the page.
    let page = page_text(&layout.pages);
    assert!(
        page.contains("FOOTNOTE") && page.contains("ENDNOTE"),
        "{page}"
    );
    assert!(
        layout
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("stays static")),
        "{:?}",
        layout.diagnostics
    );
    let (_, fields) = fields(&doc.to_pdf_with_options(fillable()).unwrap());
    assert_eq!(fields.len(), 3);
}

#[test]
fn a_control_in_a_table_of_a_note_becomes_a_field() {
    // The table follows the note's paragraph, inside the note.
    let doc = doc_with_notes(|xml, marker, value| {
        let table = format!(
            r#"{marker}</w:t></w:r></w:p><w:tbl><w:tblPr><w:tblW w:w="0" w:type="auto"/></w:tblPr><w:tblGrid><w:gridCol w:w="4000"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:w="4000" w:type="dxa"/></w:tcPr><w:p>{}</w:p></w:tc></w:tr></w:tbl><w:p><w:r><w:t></w:t></w:r>"#,
            text_control("name", value)
        );
        xml.replace(&format!("{marker}</w:t></w:r>"), &table)
    });
    assert_eq!(
        named_values(&doc),
        [
            ("name".to_owned(), "Body value".to_owned()),
            ("name_2".to_owned(), "Foot value".to_owned()),
            ("name_3".to_owned(), "End value".to_owned()),
        ]
    );
    let result = doc.layout_with_options(fillable()).unwrap();
    let mut texts: Vec<String> = fragment_texts(&result.layout.pages)
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    texts.sort();
    assert_eq!(texts, ["Body value", "End value", "Foot value"]);
}

#[test]
fn a_block_control_around_a_note_leaves_the_note_number_on_the_page() {
    // Word wraps the whole note text, its number included, when the whole
    // note is selected. The endnote holds a legacy text field instead.
    let doc = doc_with_notes(|xml, marker, value| {
        if marker == "ENDNOTE" {
            let field = legacy(
                9,
                "Legacy",
                r#"<w:enabled/><w:calcOnExit w:val="0"/><w:textInput/>"#,
                "FORMTEXT",
                Some(value),
            );
            return xml.replace(
                &format!("{marker}</w:t></w:r>"),
                &format!("{marker} </w:t></w:r>{field}"),
            );
        }
        let end = xml.find(&format!("{marker}</w:t></w:r>")).unwrap();
        let start = xml[..end]
            .rfind("<w:p>")
            .or_else(|| xml[..end].rfind("<w:p "))
            .unwrap();
        let close = end + xml[end..].find("</w:p>").unwrap() + "</w:p>".len();
        let paragraph = xml[start..close].replace(marker, value);
        format!(
            r#"{}<w:sdt><w:sdtPr><w:tag w:val="name"/><w:richText/></w:sdtPr><w:sdtContent>{paragraph}</w:sdtContent></w:sdt>{}"#,
            &xml[..start],
            &xml[close..]
        )
    });
    let result = doc.layout_with_options(fillable()).unwrap();
    let layout = &result.layout;
    let mut texts: Vec<String> = fragment_texts(&layout.pages)
        .into_iter()
        .map(|(_, text)| text.trim().to_owned())
        .collect();
    texts.sort();
    // The space after the number is the note's text, inside the control.
    assert_eq!(texts, ["Body value", "End value", "Foot value"]);
    let named: Vec<(String, String)> = names_and_values(&doc)
        .into_iter()
        .map(|(name, value)| (name, value.trim().to_owned()))
        .collect();
    assert_eq!(
        named,
        [
            ("name".to_owned(), "Body value".to_owned()),
            ("name_2".to_owned(), "Foot value".to_owned()),
            ("Legacy".to_owned(), "End value".to_owned()),
        ]
    );
    // The note's number, beside the body's reference to it.
    assert_eq!(page_text(&layout.pages).matches('1').count(), 2);
    assert!(
        layout
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("stays static")),
        "{:?}",
        layout.diagnostics
    );
}

/// A floating text box holding `content`, as Word writes it, with extra
/// attributes on its `a:xfrm` (a turn) and its `wps:bodyPr` (vertical text).
fn floating_text_box(id: u32, content: &str, xfrm: &str, body: &str) -> String {
    format!(
        r#"<w:r><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><mc:Choice Requires="wps"><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="1" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="column"><wp:posOffset>2743200</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="1828800" cy="457200"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapNone/><wp:docPr id="{id}" name="Text Box {id}"/><wp:cNvGraphicFramePr/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp><wps:cNvSpPr txBox="1"/><wps:spPr><a:xfrm{xfrm}><a:off x="0" y="0"/><a:ext cx="1828800" cy="457200"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></wps:spPr><wps:txbx><w:txbxContent>{content}</w:txbxContent></wps:txbx><wps:bodyPr{body}/></wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing></mc:Choice></mc:AlternateContent></w:r>"#
    )
}

#[test]
fn controls_in_a_text_box_become_fields_where_the_box_is_anchored() {
    let boxed = floating_text_box(
        70,
        &format!("<w:p>{}</w:p>", text_control("name", "Box value")),
        "",
        "",
    );
    let doc = doc_with_body(&format!(
        r#"<w:p>{}<w:r><w:t xml:space="preserve"> and </w:t></w:r>{boxed}</w:p><w:p>{}</w:p>"#,
        text_control("name", "First value"),
        text_control("name", "Body value"),
    ));
    assert_eq!(
        named_values(&doc),
        [
            ("name".to_owned(), "First value".to_owned()),
            ("name_2".to_owned(), "Box value".to_owned()),
            ("name_3".to_owned(), "Body value".to_owned()),
        ]
    );
    let result = doc.layout_with_options(fillable()).unwrap();
    let layout = &result.layout;
    let mut texts: Vec<String> = fragment_texts(&layout.pages)
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    texts.sort();
    assert_eq!(texts, ["Body value", "Box value", "First value"]);
    assert!(page_text(&layout.pages).contains("and"));
    assert!(
        layout
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("stays static")),
        "{:?}",
        layout.diagnostics
    );
    let (_, exported) = fields(&doc.to_pdf_with_options(fillable()).unwrap());
    assert_eq!(exported.len(), 3);
    // Tagged output too, where the box's marked content has a structure.
    let tagged = doc
        .to_pdfa_deterministic_with_options(oxoffice_docx::PdfConformance::PdfA2b, fillable())
        .unwrap();
    assert_eq!(fields(&tagged).1.len(), 3);
}

#[test]
fn controls_in_turned_vertical_or_nested_text_boxes_stay_static_and_say_so() {
    let control = |tag: &str| format!("<w:p>{}</w:p>", text_control(tag, "Value"));
    let turned = floating_text_box(71, &control("turned"), r#" rot="1800000""#, "");
    let vertical = floating_text_box(72, &control("vertical"), "", r#" vert="vert""#);
    let inner = floating_text_box(74, &control("nested"), "", "");
    let outer = floating_text_box(73, &format!("<w:p>{inner}</w:p>"), "", "");
    let doc = doc_with_body(&format!(
        r#"<w:p>{turned}</w:p><w:p>{vertical}</w:p><w:p>{outer}</w:p><w:p>{}</w:p>"#,
        text_control("plain", "Value")
    ));
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert_eq!(field_names(&layout.layout), ["plain"]);
    let said: Vec<&str> = layout
        .layout
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .filter(|message| message.contains("stays static"))
        .collect();
    assert_eq!(
        said,
        [
            "form control \"turned\" stays static: its text box is turned",
            "form control \"vertical\" stays static: its text box's text is vertical",
            "form control \"nested\" stays static: its text box sits in another text box",
        ]
    );
}

#[test]
fn a_wrapped_block_control_around_a_note_says_its_number_sits_under_its_box() {
    // The later lines start at the margin, so the widget reaches back over
    // the number on the first line.
    let long = "Foot value that runs on long enough to wrap onto a second line of the note area, and then on again for good measure, well past the right margin of the page";
    let doc = doc_with_notes(|xml, marker, _| {
        let end = xml.find(&format!("{marker}</w:t></w:r>")).unwrap();
        let start = xml[..end]
            .rfind("<w:p>")
            .or_else(|| xml[..end].rfind("<w:p "))
            .unwrap();
        let close = end + xml[end..].find("</w:p>").unwrap() + "</w:p>".len();
        let paragraph = xml[start..close].replace(marker, long);
        format!(
            r#"{}<w:sdt><w:sdtPr><w:tag w:val="note"/><w:richText/></w:sdtPr><w:sdtContent>{paragraph}</w:sdtContent></w:sdt>{}"#,
            &xml[..start],
            &xml[close..]
        )
    });
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert!(
        layout
            .layout
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message
                == "form control \"note\": other text on its lines sits under its box"),
        "{:?}",
        layout.layout.diagnostics
    );
}
