//! Word content controls exported as fillable PDF form fields.

use lopdf::{Dictionary, Document as Pdf, Object};
use oxoffice_docx::{Document, RenderOptions};
use oxoffice_layout::{FormFieldKind, PositionedElement};

use crate::support::{corpus_doc, doc_with_body};

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
fn word_content_controls_become_named_fields_with_their_values() {
    let doc = form();
    let layout = doc.layout_with_options(fillable()).unwrap();
    let names: Vec<&str> = layout
        .layout
        .form_fields
        .iter()
        .map(|field| field.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "name",
            "agree",
            "country",
            "notes",
            "city",
            "signature:client"
        ]
    );
    let kinds: Vec<&FormFieldKind> = layout
        .layout
        .form_fields
        .iter()
        .map(|field| &field.kind)
        .collect();
    assert!(matches!(kinds[0], FormFieldKind::Text { value, .. } if value == "Jane Doe"));
    assert!(matches!(
        kinds[1],
        FormFieldKind::CheckBox { checked: true }
    ));
    assert!(
        matches!(kinds[2], FormFieldKind::Choice { value: Some(value), editable: false, .. } if value == "fr")
    );
    assert!(matches!(
        kinds[3],
        FormFieldKind::Text {
            multiline: true,
            ..
        }
    ));
    assert!(
        layout.layout.form_fields[4].placeholder,
        "the city control shows its placeholder"
    );
    assert!(matches!(kinds[5], FormFieldKind::Signature));

    let bytes = doc.to_pdf_with_options(fillable()).unwrap();
    let (_, fields) = fields(&bytes);
    let by_name = |name: &str| {
        fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, dict)| dict.clone())
            .unwrap_or_else(|| panic!("no field {name}"))
    };
    assert_eq!(text(by_name("name").get(b"V").unwrap()), "Jane Doe");
    assert_eq!(text(by_name("name").get(b"TU").unwrap()), "Full name");
    assert_eq!(
        by_name("agree").get(b"V").unwrap().as_name().unwrap(),
        b"Yes"
    );
    assert_eq!(text(by_name("country").get(b"V").unwrap()), "fr");
    assert!(
        by_name("city").get(b"V").is_err(),
        "a placeholder is no value"
    );
    assert_eq!(
        text(by_name("city").get(b"TU").unwrap()),
        "Click here to enter text."
    );
    assert_eq!(
        by_name("signature:client")
            .get(b"FT")
            .unwrap()
            .as_name()
            .unwrap(),
        b"Sig"
    );
}

#[test]
fn without_the_switch_the_layout_has_no_form_field() {
    let doc = form();
    let layout = doc.layout_with_options(RenderOptions::default()).unwrap();
    assert!(layout.layout.form_fields.is_empty());
    let mut fragments = 0;
    for page in &layout.layout.pages {
        oxoffice_layout::walk(&page.elements, &mut |_, _| {});
        count_fragments(&page.elements, &mut fragments);
    }
    assert_eq!(fragments, 0);
    let bytes = doc.to_pdf_with_options(RenderOptions::default()).unwrap();
    let pdf = Pdf::load_mem(&bytes).unwrap();
    assert!(pdf.catalog().unwrap().get(b"AcroForm").is_err());
}

fn count_fragments(elements: &[PositionedElement], count: &mut usize) {
    for element in elements {
        match element {
            PositionedElement::FormField(fragment) => {
                assert!(fragment.mark.is_none(), "no marker survives the layout");
                *count += 1;
            }
            PositionedElement::MarkedContent { children, .. } => count_fragments(children, count),
            PositionedElement::Group(group) => count_fragments(&group.children, count),
            _ => {}
        }
    }
}

#[test]
fn the_same_document_switches_fields_on_and_off() {
    let doc = form();
    assert_eq!(
        doc.layout_with_options(fillable())
            .unwrap()
            .layout
            .form_fields
            .len(),
        6
    );
    assert!(
        doc.layout_with_options(RenderOptions::default())
            .unwrap()
            .layout
            .form_fields
            .is_empty()
    );
    let again = doc.layout_with_options(fillable()).unwrap();
    let mut fragments = 0;
    for page in &again.layout.pages {
        count_fragments(&page.elements, &mut fragments);
    }
    assert!(fragments >= 6, "every control drew a fragment: {fragments}");
}

#[test]
fn the_tracked_view_refuses_fillable_forms() {
    let doc = form();
    let options = fillable().with_revision_view(oxoffice_docx::RevisionView::Tracked);
    assert!(doc.to_pdf_with_options(options).is_err());
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

/// The content controls of the corpus forms, in document order.
const CORPUS_FORM_CONTROLS: [&str; 14] = [
    "company_name",
    "registered_address",
    "date_of_incorporation",
    "company_type",
    "country",
    "vat_registered",
    "iso_9001",
    "contact_person",
    "signatory",
    "signature_date",
    "signature",
    "supplier_reference",
    "category",
    "reviewed",
];

/// The legacy form fields of the Word-shaped corpus form, after its
/// `contact_person` control. LibreOffice's copy keeps them in a shape the
/// model does not read as fields (#139). Its `PaymentTerms` drop-down stores
/// no result, as Word saves one, and draws its selected entry.
const CORPUS_LEGACY_FIELDS: [&str; 6] = [
    "BankName",
    "IBAN",
    "BIC",
    "PaymentTerms",
    "RemittanceEmail",
    "Declaration",
];

#[test]
fn the_corpus_forms_export_their_controls_as_fields() {
    for name in ["authored/form.docx", "libreoffice/form.docx"] {
        let doc = corpus_doc(name);
        let with_fields = doc.layout_with_options(fillable()).unwrap();
        let built = &with_fields.layout.form_fields;
        let names: Vec<&str> = built.iter().map(|field| field.name.as_str()).collect();
        let mut expected = CORPUS_FORM_CONTROLS.to_vec();
        if name.starts_with("authored/") {
            expected.splice(8..8, CORPUS_LEGACY_FIELDS);
        }
        assert_eq!(names, expected, "{name}");
        let field = |wanted: &str| built.iter().find(|field| field.name == wanted).unwrap();
        assert!(
            matches!(field("signature").kind, FormFieldKind::Signature),
            "{name}"
        );
        assert!(
            field("supplier_reference").read_only,
            "{name}: its content is locked"
        );
        let said: Vec<&String> = with_fields
            .layout
            .diagnostics
            .iter()
            .map(|diagnostic| &diagnostic.message)
            .filter(|message| message.contains("stays static"))
            .collect();
        assert!(said.is_empty(), "{name}: {said:?}");
        let without = doc.layout_with_options(RenderOptions::default()).unwrap();
        assert_eq!(
            drawing(&with_fields.layout.pages),
            drawing(&without.layout.pages),
            "{name}"
        );
        let (_, exported) = fields(&doc.to_pdf_with_options(fillable()).unwrap());
        assert_eq!(
            exported.len(),
            names.len() - said.len(),
            "{name}: every field but the static ones has a widget"
        );
        let signature = &exported
            .iter()
            .find(|(field, _)| field == "signature")
            .unwrap()
            .1;
        assert_eq!(signature.get(b"FT").unwrap().as_name().unwrap(), b"Sig");
    }
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

#[test]
fn a_block_field_takes_only_its_own_paragraphs() {
    let doc = doc_with_body(
        r#"<w:sdt><w:sdtPr><w:tag w:val="title"/><w:richText/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Title here</w:t></w:r></w:p></w:sdtContent></w:sdt><w:p><w:r><w:t>Title here</w:t></w:r></w:p><w:p><w:r><w:t>Plain two</w:t></w:r></w:p>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert_eq!(
        fragment_texts(&layout.layout.pages),
        [(0, "Title here".to_owned())]
    );
    assert_eq!(page_text(&layout.layout.pages), "Title herePlain two");
}

#[test]
fn a_header_line_is_never_part_of_a_body_field() {
    let mut doc = doc_with_body(
        r#"<w:sdt><w:sdtPr><w:tag w:val="title"/><w:richText/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Title here</w:t></w:r></w:p></w:sdtContent></w:sdt><w:p><w:r><w:t>Body text</w:t></w:r></w:p>"#,
    );
    doc.set_header("Header line");
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert_eq!(
        fragment_texts(&layout.layout.pages),
        [(0, "Title here".to_owned())]
    );
    assert!(page_text(&layout.layout.pages).contains("Header line"));
}

#[test]
fn a_control_in_a_text_box_is_no_field_and_takes_nothing() {
    let doc = doc_with_body(
        r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="body"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Body value</w:t></w:r></w:sdtContent></w:sdt></w:p><w:p><w:r><w:t xml:space="preserve">anchor </w:t></w:r><w:r><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:v="urn:schemas-microsoft-com:vml"><mc:Choice Requires="wps"><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="1" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="column"><wp:posOffset>0</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="1828800" cy="914400"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapNone/><wp:docPr id="77" name="Text Box 77"/><wp:cNvGraphicFramePr/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp><wps:cNvSpPr txBox="1"/><wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1828800" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></wps:spPr><wps:txbx><w:txbxContent><w:p><w:sdt><w:sdtPr><w:tag w:val="boxed"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Box value</w:t></w:r></w:sdtContent></w:sdt></w:p></w:txbxContent></wps:txbx><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing></mc:Choice><mc:Fallback><w:pict><v:shape id="tb77" style="width:144pt;height:72pt"><v:textbox><w:txbxContent><w:p><w:r><w:t>Box value</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape></w:pict></mc:Fallback></mc:AlternateContent></w:r></w:p>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    let names: Vec<&str> = layout
        .layout
        .form_fields
        .iter()
        .map(|field| field.name.as_str())
        .collect();
    assert_eq!(names, ["body"]);
    assert_eq!(
        fragment_texts(&layout.layout.pages),
        [(0, "Body value".to_owned())]
    );
    assert!(page_text(&layout.layout.pages).contains("Box value"));
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
fn a_control_that_wraps_alone_is_one_multiline_field() {
    let words = "wrapping words ".repeat(30);
    let doc = doc_with_body(&format!(
        r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="long"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t xml:space="preserve">{words}</w:t></w:r></w:sdtContent></w:sdt></w:p>"#
    ));
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert!(matches!(
        layout.layout.form_fields[0].kind,
        FormFieldKind::Text {
            multiline: true,
            ..
        }
    ));
    let drawn = fragment_texts(&layout.layout.pages);
    assert_eq!(drawn.len(), 1, "one field: {drawn:?}");
    let letters = |text: &str| text.split_whitespace().collect::<String>();
    assert_eq!(
        letters(&drawn[0].1),
        letters(&words),
        "every line is the field's"
    );
    assert!(diagnostics_about(&layout.layout, "long").is_empty());
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
fn a_control_beside_a_page_count_on_a_right_tab_keeps_its_glyphs() {
    // Right-aligned text holding a control, then the page count, which
    // takes two digits: what is substituted after layout around a control
    // stays with it.
    let breaks = r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#.repeat(10);
    let doc = doc_with_body(&format!(
        r#"{breaks}<w:p><w:pPr><w:tabs><w:tab w:val="right" w:pos="9000"/></w:tabs></w:pPr><w:r><w:tab/></w:r><w:sdt><w:sdtPr><w:tag w:val="reference"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>R-12</w:t></w:r></w:sdtContent></w:sdt><w:r><w:t xml:space="preserve"> page </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> NUMPAGES </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
    ));
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert_eq!(
        fragment_texts(&layout.layout.pages),
        [(0, "R-12".to_owned())]
    );
    assert!(!page_text(&layout.layout.pages).contains("R-12"));
    let without = doc.layout_with_options(RenderOptions::default()).unwrap();
    assert_eq!(
        drawing(&layout.layout.pages),
        drawing(&without.layout.pages)
    );
}

#[test]
fn every_field_takes_its_own_value_off_the_page() {
    let layout = form().layout_with_options(fillable()).unwrap();
    let pages = &layout.layout.pages;
    assert_eq!(
        fragment_texts(pages),
        [
            (0, "Jane Doe".to_owned()),
            (1, "\u{2612}".to_owned()),
            (2, "France".to_owned()),
            (3, "Some notes".to_owned()),
            (4, "Click here to enter text.".to_owned()),
            (5, "Sign here".to_owned()),
        ]
    );
    let page = page_text(pages);
    assert_eq!(
        page, "Name:  I agreeCountry: City: ",
        "labels stay, values go"
    );
}

#[test]
fn a_paragraph_inserted_first_moves_every_field_with_its_control() {
    let mut doc = form();
    let before = doc.layout_with_options(fillable()).unwrap();
    doc.insert_paragraph(0, "Intro");
    let after = doc.layout_with_options(fillable()).unwrap();
    assert_eq!(field_names(&after.layout), field_names(&before.layout));
    assert_eq!(
        fragment_texts(&after.layout.pages),
        fragment_texts(&before.layout.pages)
    );
    assert!(page_text(&after.layout.pages).starts_with("Intro"));
}

#[test]
fn a_style_edit_reaches_the_next_field_layout() {
    let mut doc = form();
    let before = doc.layout_with_options(fillable()).unwrap();
    doc.paragraph_mut(0).unwrap().set_style("Heading1");
    let after = doc.layout_with_options(fillable()).unwrap();
    assert!(
        after.layout.form_fields[0].font_size > before.layout.form_fields[0].font_size,
        "{} then {}",
        before.layout.form_fields[0].font_size,
        after.layout.form_fields[0].font_size
    );
    assert_eq!(
        fragment_texts(&after.layout.pages),
        fragment_texts(&before.layout.pages)
    );
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

// Word reads left and right as start and end in a right-to-left paragraph,
// and so does the field's alignment.
#[test]
fn a_control_in_a_right_to_left_paragraph_reads_left_and_right_as_start_and_end() {
    for (jc, expected) in [
        ("left", oxoffice_layout::FieldAlignment::Right),
        ("start", oxoffice_layout::FieldAlignment::Right),
        ("right", oxoffice_layout::FieldAlignment::Left),
        ("end", oxoffice_layout::FieldAlignment::Left),
    ] {
        let doc = doc_with_body(&format!(
            r#"<w:p><w:pPr><w:bidi/><w:jc w:val="{jc}"/></w:pPr><w:r><w:rPr><w:rtl/></w:rPr><w:t xml:space="preserve">שם: </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="name"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Jane</w:t></w:r></w:sdtContent></w:sdt></w:p>"#
        ));
        let layout = doc.layout_with_options(fillable()).unwrap();
        assert_eq!(layout.layout.form_fields[0].alignment, expected, "jc={jc}");
    }
}

#[test]
fn a_check_box_widget_covers_its_symbol_and_draws_its_other_state() {
    fn check_box(elements: &[PositionedElement]) -> Option<&oxoffice_layout::FormFieldFragment> {
        elements.iter().find_map(|element| match element {
            PositionedElement::FormField(fragment) if fragment.field.0 == 1 => Some(fragment),
            PositionedElement::MarkedContent { children, .. } => check_box(children),
            _ => None,
        })
    }
    let layout = form().layout_with_options(fillable()).unwrap();
    let fragment = check_box(&layout.layout.pages[0].elements).expect("the check box fragment");
    let symbol = fragment
        .children
        .iter()
        .find_map(|child| match child {
            PositionedElement::Text(run) => Some(run),
            PositionedElement::MarkedContent { children, .. } => {
                children.iter().find_map(|child| match child {
                    PositionedElement::Text(run) => Some(run),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("the checked symbol");
    assert_eq!(symbol.text, "\u{2612}");
    let width: f64 = symbol.advances.iter().sum();
    assert!(
        (fragment.rect.x - symbol.origin.x).abs() < 0.01,
        "{:?}",
        fragment.rect
    );
    assert!(
        (fragment.rect.width - width).abs() < 0.01,
        "{:?}",
        fragment.rect
    );
    let other = match fragment.alternate.as_slice() {
        [PositionedElement::Text(run)] => run,
        other => panic!("one other symbol: {other:?}"),
    };
    assert_eq!(other.text, "\u{2610}");
    assert_eq!(other.font_id, symbol.font_id);
    assert_eq!(layout.layout.form_fields[1].font_size, symbol.font_size);
}

#[test]
fn a_value_is_what_the_accepted_view_shows() {
    // A tracked insertion is part of the value and a deletion is not, and a
    // line feed inside a text node shows as a space, as in Word.
    let doc = doc_with_body(
        r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="tracked"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t xml:space="preserve">Ada </w:t></w:r><w:ins w:id="1" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:t xml:space="preserve">Lovelace </w:t></w:r></w:ins><w:del w:id="2" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:delText>Byron </w:delText></w:r></w:del><w:r><w:t>London</w:t></w:r></w:sdtContent></w:sdt></w:p><w:p><w:sdt><w:sdtPr><w:tag w:val="feed"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t xml:space="preserve">Ada Lovelace&#10;London</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    for field in &layout.layout.form_fields {
        assert!(
            matches!(
                &field.kind,
                FormFieldKind::Text { value, multiline: false, .. } if value == "Ada Lovelace London"
            ),
            "{}: {:?}",
            field.name,
            field.kind
        );
    }
    assert_eq!(layout.layout.form_fields.len(), 2);
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

/// How many pictures the pages draw outside any fragment.
fn page_pictures(pages: &[std::sync::Arc<oxoffice_layout::PageFrame>]) -> usize {
    fn visit(elements: &[PositionedElement]) -> usize {
        elements
            .iter()
            .map(|element| match element {
                PositionedElement::Image { .. } => 1,
                PositionedElement::MarkedContent { children, .. } => visit(children),
                PositionedElement::Group(group) => visit(&group.children),
                _ => 0,
            })
            .sum()
    }
    pages.iter().map(|page| visit(&page.elements)).sum()
}

#[test]
fn a_floating_picture_over_a_control_stays_on_the_page() {
    // A small picture anchored at the paragraph's start lies inside the
    // box of the control's first word.
    let doc = doc_with_picture(|relationship| {
        format!(
            r#"<w:p><w:r><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="1" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="column"><wp:posOffset>12700</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>25400</wp:posOffset></wp:positionV><wp:extent cx="50800" cy="50800"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapNone/><wp:docPr id="11" name="Dot"/><wp:cNvGraphicFramePr/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:nvPicPr><pic:cNvPr id="11" name="picture.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{relationship}" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="50800" cy="50800"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:anchor></w:drawing></w:r><w:sdt><w:sdtPr><w:tag w:val="name"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Wolfeschlegelsteinhausen</w:t></w:r></w:sdtContent></w:sdt></w:p>"#
        )
    });
    let layout = doc.layout_with_options(fillable()).unwrap();
    let pages = &layout.layout.pages;
    assert_eq!(
        fragment_texts(pages),
        [(0, "Wolfeschlegelsteinhausen".to_owned())]
    );
    assert_eq!(page_pictures(pages), 1, "the floating picture stays");
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
fn an_empty_control_says_it_stays_static() {
    let doc = doc_with_body(
        r#"<w:p><w:r><w:t xml:space="preserve">Notes: </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="empty"/><w:text/></w:sdtPr><w:sdtContent/></w:sdt></w:p>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert!(
        layout.layout.form_fields.is_empty() || fragment_texts(&layout.layout.pages).is_empty()
    );
    let said = diagnostics_about(&layout.layout, "empty");
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("stays static"), "{said:?}");
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
fn a_section_break_inside_a_block_control_makes_its_section_vertical() {
    let doc = doc_with_body(
        r#"<w:sdt><w:sdtPr><w:tag w:val="wrapper"/><w:group/></w:sdtPr><w:sdtContent><w:p><w:sdt><w:sdtPr><w:tag w:val="upright"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Down</w:t></w:r></w:sdtContent></w:sdt></w:p><w:p><w:pPr><w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/><w:textDirection w:val="tbRl"/></w:sectPr></w:pPr></w:p></w:sdtContent></w:sdt>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert!(
        layout.layout.form_fields.is_empty() || fragment_texts(&layout.layout.pages).is_empty()
    );
    let said = diagnostics_about(&layout.layout, "upright");
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].contains("its section's text is vertical"),
        "{said:?}"
    );
}

#[test]
fn a_control_around_a_deleted_control_stays_a_field() {
    let doc = doc_with_body(
        r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="outer"/><w:richText/></w:sdtPr><w:sdtContent><w:r><w:t xml:space="preserve">Kept </w:t></w:r><w:del w:id="1" w:author="a" w:date="2026-01-01T00:00:00Z"><w:sdt><w:sdtPr><w:tag w:val="gone"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:delText>Gone</w:delText></w:r></w:sdtContent></w:sdt></w:del><w:r><w:t>text</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert_eq!(field_names(&layout.layout), ["outer"]);
    assert!(matches!(
        &layout.layout.form_fields[0].kind,
        FormFieldKind::Text { value, .. } if value == "Kept text"
    ));
}

#[test]
fn a_repeated_name_follows_document_order_through_tracked_changes() {
    let doc = doc_with_body(
        r#"<w:p><w:ins w:id="1" w:author="a" w:date="2026-01-01T00:00:00Z"><w:sdt><w:sdtPr><w:tag w:val="name"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Jane</w:t></w:r></w:sdtContent></w:sdt></w:ins><w:r><w:t xml:space="preserve"> </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="name"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Doe</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    let named: Vec<(&str, &str)> = layout
        .layout
        .form_fields
        .iter()
        .map(|field| match &field.kind {
            FormFieldKind::Text { value, .. } => (field.name.as_str(), value.as_str()),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(named, [("name", "Jane"), ("name_2", "Doe")]);
}

#[test]
fn a_control_in_a_vertical_section_inside_a_block_control_stays_static() {
    // The block control holds a horizontal section's break, then a control
    // that belongs to the vertical section the next break ends.
    let doc = doc_with_body(
        r#"<w:sdt><w:sdtPr><w:tag w:val="wrapper"/><w:group/></w:sdtPr><w:sdtContent><w:p><w:pPr><w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr></w:pPr></w:p><w:p><w:sdt><w:sdtPr><w:tag w:val="after"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Down</w:t></w:r></w:sdtContent></w:sdt></w:p></w:sdtContent></w:sdt><w:p><w:pPr><w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/><w:textDirection w:val="tbRl"/></w:sectPr></w:pPr></w:p>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert!(fragment_texts(&layout.layout.pages).is_empty());
    let said = diagnostics_about(&layout.layout, "after");
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].contains("its section's text is vertical"),
        "{said:?}"
    );
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

#[test]
fn a_floating_text_box_over_a_value_stays_on_the_page() {
    let doc = doc_with_body(
        r#"<w:p><w:r><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><mc:Choice Requires="wps"><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="1" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="column"><wp:posOffset>0</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="2286000" cy="228600"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapNone/><wp:docPr id="71" name="Stamp"/><wp:cNvGraphicFramePr/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp><wps:cNvSpPr txBox="1"/><wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2286000" cy="228600"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></wps:spPr><wps:txbx><w:txbxContent><w:p><w:r><w:t>APPROVED</w:t></w:r></w:p></w:txbxContent></wps:txbx><wps:bodyPr lIns="0" tIns="0" rIns="0" bIns="0"/></wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing></mc:Choice></mc:AlternateContent></w:r><w:sdt><w:sdtPr><w:tag w:val="name"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Wolfeschlegelsteinhausenbergerdorff</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    let pages = &layout.layout.pages;
    assert_eq!(
        fragment_texts(pages),
        [(0, "Wolfeschlegelsteinhausenbergerdorff".to_owned())]
    );
    assert!(page_text(pages).contains("APPROVED"), "the stamp stays");
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
fn fields_draw_exactly_what_the_static_layout_draws() {
    let doc = form();
    let with_fields = doc.layout_with_options(fillable()).unwrap();
    let without = doc.layout_with_options(RenderOptions::default()).unwrap();
    assert_eq!(
        drawing(&with_fields.layout.pages),
        drawing(&without.layout.pages)
    );
}

#[test]
fn controls_in_table_cells_become_fields() {
    let doc = doc_with_body(
        r#"<w:tbl><w:tblPr><w:tblW w:w="6000" w:type="dxa"/></w:tblPr><w:tblGrid><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:w="3000" w:type="dxa"/></w:tcPr><w:p><w:r><w:t>Name</w:t></w:r></w:p></w:tc><w:tc><w:tcPr><w:tcW w:w="3000" w:type="dxa"/></w:tcPr><w:p><w:sdt><w:sdtPr><w:tag w:val="cell_name"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Jane</w:t></w:r></w:sdtContent></w:sdt></w:p></w:tc></w:tr><w:tr><w:tc><w:tcPr><w:tcW w:w="3000" w:type="dxa"/></w:tcPr><w:p><w:r><w:t>Notes</w:t></w:r></w:p></w:tc><w:tc><w:tcPr><w:tcW w:w="3000" w:type="dxa"/></w:tcPr><w:sdt><w:sdtPr><w:tag w:val="cell_notes"/><w:richText/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Block in a cell</w:t></w:r></w:p></w:sdtContent></w:sdt></w:tc></w:tr></w:tbl><w:p/>"#,
    );
    let layout = doc.layout_with_options(fillable()).unwrap();
    let names: Vec<&str> = layout
        .layout
        .form_fields
        .iter()
        .map(|field| field.name.as_str())
        .collect();
    assert_eq!(names, ["cell_name", "cell_notes"]);
    let (_, fields) = fields(&doc.to_pdf_with_options(fillable()).unwrap());
    let found: Vec<&str> = fields.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(found, ["cell_name", "cell_notes"], "both get a widget");
}

#[test]
fn a_control_split_across_pages_stays_static_and_says_so() {
    let paragraphs: String = (0..90)
        .map(|index| format!("<w:p><w:r><w:t>Line {index}</w:t></w:r></w:p>"))
        .collect();
    let doc = doc_with_body(&format!(
        r#"<w:sdt><w:sdtPr><w:tag w:val="long"/><w:richText/></w:sdtPr><w:sdtContent>{paragraphs}</w:sdtContent></w:sdt>"#
    ));
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert!(layout.layout.pages.len() > 1, "the control spans pages");
    let mut fragments = 0;
    for page in &layout.layout.pages {
        count_fragments(&page.elements, &mut fragments);
    }
    assert_eq!(fragments, 0, "no fragment for a split control");
    assert!(
        layout
            .layout
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("\"long\"")
                && diagnostic.message.contains("split across pages")),
        "{:?}",
        layout.layout.diagnostics
    );
    let bytes = doc.to_pdf_with_options(fillable()).unwrap();
    assert!(
        Pdf::load_mem(&bytes)
            .unwrap()
            .catalog()
            .unwrap()
            .get(b"AcroForm")
            .is_err()
    );
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

#[test]
fn legacy_fields_draw_exactly_what_the_static_layout_draws() {
    let doc = legacy_form();
    let with_fields = doc.layout_with_options(fillable()).unwrap();
    let without = doc.layout_with_options(RenderOptions::default()).unwrap();
    assert_eq!(
        drawing(&with_fields.layout.pages),
        drawing(&without.layout.pages)
    );
    let mut fragments = 0;
    for page in &with_fields.layout.pages {
        count_fragments(&page.elements, &mut fragments);
    }
    assert_eq!(fragments, 6, "every legacy field has its widget");
    let texts: Vec<String> = fragment_texts(&with_fields.layout.pages)
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    assert_eq!(
        texts,
        [
            "Acme",
            "\u{2612}",
            "Green",
            "R-12",
            "21/11/2026",
            "\u{2002}\u{2002}\u{2002}\u{2002}\u{2002}"
        ],
        "each fragment holds its field's result and nothing beside it"
    );
    let outside = page_text(&with_fields.layout.pages);
    for label in [
        "Company: ",
        " Subscribe",
        "Colour: ",
        "Reference: ",
        " Date: ",
        "City: ",
    ] {
        assert!(
            outside.contains(label),
            "{label:?} stays on the page: {outside:?}"
        );
    }
}

/// A legacy text field inside a tracked deletion, its instruction and result
/// in plain runs. Word writes `delInstrText` and `delText` there (checked
/// below): the model reads both shapes as the same field.
const DELETED_LEGACY: &str = r#"<w:del w:id="90" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:fldChar w:fldCharType="begin"><w:ffData><w:name w:val="Old"/><w:enabled/><w:calcOnExit w:val="0"/><w:textInput/></w:ffData></w:fldChar></w:r><w:r><w:instrText xml:space="preserve"> FORMTEXT </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Old value</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:del>"#;

#[test]
fn a_legacy_field_in_a_tracked_deletion_is_no_field_and_says_so() {
    let doc = doc_with_body(&format!(
        r#"<w:p><w:r><w:t xml:space="preserve">Kept </w:t></w:r>{DELETED_LEGACY}</w:p>"#
    ));
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert!(field_names(&layout.layout).is_empty());
    assert_eq!(
        diagnostics_about(&layout.layout, "Old"),
        ["form field \"Old\" stays static: a tracked change deletes it"]
    );

    // Around it, a rich text control holds no live field: it is the field.
    let doc = doc_with_body(&format!(
        r#"<w:sdt><w:sdtPr><w:tag w:val="notes"/><w:richText/></w:sdtPr><w:sdtContent><w:p><w:r><w:t xml:space="preserve">Kept </w:t></w:r>{DELETED_LEGACY}</w:p></w:sdtContent></w:sdt>"#
    ));
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert_eq!(field_names(&layout.layout), ["notes"]);

    // Word's own shape of a deleted field says so too.
    let word_shape = DELETED_LEGACY
        .replace("w:instrText", "w:delInstrText")
        .replace("<w:t>Old value</w:t>", "<w:delText>Old value</w:delText>");
    let doc = doc_with_body(&format!(
        r#"<w:p><w:r><w:t xml:space="preserve">Kept </w:t></w:r>{word_shape}</w:p>"#
    ));
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert!(field_names(&layout.layout).is_empty());
    assert_eq!(
        diagnostics_about(&layout.layout, "Old"),
        ["form field \"Old\" stays static: a tracked change deletes it"]
    );
}

#[test]
fn a_legacy_field_whose_form_data_does_not_read_stays_static_and_says_so() {
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
        ["form field \"untitled\" stays static: its form data does not read"]
    );
}

#[test]
fn a_legacy_check_box_draws_its_other_state() {
    fn alternates(elements: &[PositionedElement], out: &mut Vec<String>) {
        for element in elements {
            match element {
                PositionedElement::FormField(fragment) => {
                    for alternate in &fragment.alternate {
                        if let PositionedElement::Text(run) = alternate {
                            out.push(run.text.clone());
                        }
                    }
                }
                PositionedElement::MarkedContent { children, .. } => alternates(children, out),
                PositionedElement::Group(group) => alternates(&group.children, out),
                _ => {}
            }
        }
    }
    let layout = legacy_form().layout_with_options(fillable()).unwrap();
    let mut found = Vec::new();
    for page in &layout.layout.pages {
        alternates(&page.elements, &mut found);
    }
    assert_eq!(found, ["\u{2610}"], "the checked box can draw unchecked");
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
fn legacy_fields_and_controls_are_numbered_in_document_order() {
    let first = legacy(
        0,
        "First",
        r#"<w:enabled/><w:calcOnExit w:val="0"/><w:textInput/>"#,
        "FORMTEXT",
        Some("one"),
    );
    let last = legacy(
        1,
        "Last",
        r#"<w:enabled/><w:calcOnExit w:val="0"/><w:textInput/>"#,
        "FORMTEXT",
        Some("four"),
    );
    let doc = doc_with_body(&format!(
        r#"<w:p>{first}<w:r><w:t xml:space="preserve"> </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="second"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>two</w:t></w:r></w:sdtContent></w:sdt></w:p>
<w:p><w:sdt><w:sdtPr><w:tag w:val="third"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>three</w:t></w:r></w:sdtContent></w:sdt><w:r><w:t xml:space="preserve"> </w:t></w:r>{last}</w:p>"#
    ));
    assert_eq!(names_of(&doc), ["First", "second", "third", "Last"]);
}

#[test]
fn a_legacy_field_in_a_rotated_cell_stays_static_and_says_so() {
    let field = legacy(
        0,
        "Turned",
        r#"<w:enabled/><w:calcOnExit w:val="0"/><w:textInput/>"#,
        "FORMTEXT",
        Some("up"),
    );
    let doc = doc_with_body(&format!(
        r#"<w:tbl><w:tblPr><w:tblW w:w="3000" w:type="dxa"/></w:tblPr><w:tblGrid><w:gridCol w:w="3000"/></w:tblGrid><w:tr><w:trPr><w:trHeight w:val="1500"/></w:trPr><w:tc><w:tcPr><w:tcW w:w="3000" w:type="dxa"/><w:textDirection w:val="btLr"/></w:tcPr><w:p>{field}</w:p></w:tc></w:tr></w:tbl><w:p/>"#
    ));
    let layout = doc.layout_with_options(fillable()).unwrap();
    assert!(layout.layout.form_fields.is_empty());
    let said = diagnostics_about(&layout.layout, "Turned");
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("its cell's text is rotated"), "{said:?}");
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
fn controls_inside_a_container_follow_document_order_through_its_tracked_changes() {
    // A tracked insertion inside the container comes before a later
    // control: the repeated name goes to the later one.
    let doc = doc_with_body(
        r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="person"/><w:richText/></w:sdtPr><w:sdtContent><w:ins w:id="1" w:author="a" w:date="2026-01-01T00:00:00Z"><w:sdt><w:sdtPr><w:tag w:val="name"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Jane</w:t></w:r></w:sdtContent></w:sdt></w:ins><w:r><w:t xml:space="preserve"> </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="name"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Doe</w:t></w:r></w:sdtContent></w:sdt></w:sdtContent></w:sdt></w:p>"#,
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

#[test]
fn an_empty_control_before_a_drawn_one_takes_the_first_name() {
    let doc = doc_with_body(
        r#"<w:p><w:r><w:t xml:space="preserve">Label </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="F"/><w:text/></w:sdtPr><w:sdtContent/></w:sdt><w:sdt><w:sdtPr><w:tag w:val="F"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>x</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
    );
    let named = names_and_values(&doc);
    assert_eq!(named[1], ("F_2".to_owned(), "x".to_owned()), "{named:?}");
}

#[test]
fn a_legacy_field_that_wraps_beside_text_says_form_field() {
    let words = "wrapping words ".repeat(30);
    let field = legacy(
        0,
        "Beside",
        r#"<w:enabled/><w:calcOnExit w:val="0"/><w:textInput/>"#,
        "FORMTEXT",
        Some(&words),
    );
    let doc = doc_with_body(&format!(
        r#"<w:p><w:r><w:t xml:space="preserve">Label: </w:t></w:r>{field}<w:r><w:t xml:space="preserve"> after</w:t></w:r></w:p>"#
    ));
    let layout = doc.layout_with_options(fillable()).unwrap();
    let said = diagnostics_about(&layout.layout, "Beside");
    assert_eq!(
        said,
        ["form field \"Beside\": other text on its lines sits under its box"]
    );
}
