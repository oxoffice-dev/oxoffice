//! Core, application and custom properties refuse a character XML cannot
//! carry instead of making every later save fail.

use oxoffice_docx::{AppProperties, CoreProperties, CustomProperty, CustomPropertyValue, Document};

fn custom(name: &str, value: CustomPropertyValue) -> CustomProperty {
    CustomProperty {
        fmtid: "{D5CDD505-2E9C-101B-9397-08002B2CF9AE}".into(),
        pid: 2,
        name: Some(name.into()),
        value,
    }
}

#[test]
fn a_property_xml_cannot_carry_is_refused_and_the_document_still_saves() {
    let mut doc = Document::new();
    doc.set_custom_property(custom("Kept", CustomPropertyValue::Lpwstr("fine".into())))
        .unwrap();
    let before = doc.to_bytes().unwrap();
    let refusals = [
        (
            "title",
            doc.set_core_properties(CoreProperties {
                title: Some("Q3\u{7}".into()),
                ..Default::default()
            }),
        ),
        (
            "keywords",
            doc.set_core_properties(CoreProperties {
                keywords: Some("a\u{FFFF}".into()),
                ..Default::default()
            }),
        ),
        (
            "company",
            doc.set_application_properties({
                let mut app = AppProperties::default();
                app.company = Some("\u{FFFE}Corp".into());
                app
            }),
        ),
        (
            "\"Bell\"",
            doc.set_custom_property(custom(
                "Bell",
                CustomPropertyValue::Lpwstr("ring\u{7}".into()),
            )),
        ),
        (
            "name",
            doc.set_custom_property(custom("Bad\u{1}", CustomPropertyValue::Lpstr("x".into()))),
        ),
    ];
    for (field, refusal) in refusals {
        let message = refusal.expect_err(field).to_string();
        assert!(
            message.contains(field) && message.contains("a character XML 1.0 cannot carry"),
            "{field}: {message}"
        );
    }
    assert_eq!(
        doc.to_bytes().unwrap(),
        before,
        "nothing changed, and it still saves"
    );
}

#[test]
fn line_feeds_and_tabs_in_properties_are_accepted() {
    let mut doc = Document::new();
    doc.set_core_properties(CoreProperties {
        description: Some("first\nsecond\tthird".into()),
        ..Default::default()
    })
    .unwrap();
    doc.set_custom_property(custom(
        "Address",
        CustomPropertyValue::Lpwstr("1 Main St\nTown".into()),
    ))
    .unwrap();
    let doc = Document::from_bytes(&doc.to_bytes().unwrap()).unwrap();
    assert_eq!(
        doc.core_properties()
            .and_then(|core| core.description.as_deref()),
        Some("first\nsecond\tthird")
    );
}

#[test]
fn a_one_field_setter_takes_the_value_and_the_save_names_the_part_and_the_character() {
    // The one-field setters cannot refuse: the save does, through the package
    // writer, and set_core_properties refuses the value at once.
    let mut doc = Document::new();
    doc.set_title("Q3\u{7}");
    let message = doc.to_bytes().expect_err("the save fails").to_string();
    assert!(
        message.contains("/docProps/core.xml") && message.contains("U+0007"),
        "{message}"
    );
    doc.set_title("Q3");
    assert!(doc.to_bytes().is_ok(), "a clean value saves again");
}

#[test]
fn every_application_and_custom_string_xml_cannot_carry_is_refused() {
    let mut doc = Document::new();
    let before = doc.to_bytes().unwrap();
    for field in [
        "template",
        "manager",
        "company",
        "presentation format",
        "application",
    ] {
        let mut app = AppProperties::default();
        let value = Some("a\u{1}".to_owned());
        match field {
            "template" => app.template = value,
            "manager" => app.manager = value,
            "company" => app.company = value,
            "presentation format" => app.presentation_format = value,
            _ => app.application = value,
        }
        let message = doc
            .set_application_properties(app)
            .expect_err(field)
            .to_string();
        assert!(message.contains(field), "{field}: {message}");
    }
    let mut bad_fmtid = custom("Fmt", CustomPropertyValue::Lpwstr("x".into()));
    bad_fmtid.fmtid = "{D5CDD505-2E9C-101B-9397-08002B2CF9AE}\u{7}".into();
    for property in [
        custom("Ansi", CustomPropertyValue::Lpstr("a\u{2}".into())),
        custom("When", CustomPropertyValue::FileTime("2026\u{3}".into())),
        custom(
            "Raw",
            CustomPropertyValue::Raw(b"<vt:lpstr>a\x07</vt:lpstr>".to_vec()),
        ),
        bad_fmtid,
    ] {
        let name = property.name.clone().unwrap();
        assert!(doc.set_custom_property(property).is_err(), "{name}");
    }
    assert_eq!(doc.to_bytes().unwrap(), before, "nothing changed");
}

#[test]
fn a_file_whose_core_part_already_holds_a_bad_character_reference_still_edits_and_saves() {
    // Another producer wrote a character reference XML 1.0 forbids; the file
    // opens, and the part is kept as it was through edits and saves.
    let bytes = Document::new().to_bytes().unwrap();
    let mut package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).unwrap();
    let core = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Report</dc:title><cp:lastModifiedBy>J&#11;Doe</cp:lastModifiedBy></cp:coreProperties>"#;
    let part = if package.contains_part("/docProps/core.xml") {
        "/docProps/core.xml"
    } else {
        panic!("a new document has a core part")
    };
    package.set_part(part, core.as_bytes().to_vec());
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut doc = Document::from_bytes(&out.into_inner()).unwrap();
    doc.add_paragraph("an edit");
    let saved = doc.to_bytes().expect("the file still saves");
    let reopened = Document::from_bytes(&saved).unwrap();
    assert_eq!(
        reopened
            .core_properties()
            .and_then(|core| core.last_modified_by.as_deref()),
        Some("J\u{b}Doe")
    );
}
