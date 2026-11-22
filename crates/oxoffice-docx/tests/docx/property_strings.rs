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
        let message = doc
            .set_custom_property(property)
            .expect_err(&name)
            .to_string();
        let named = if name == "Fmt" {
            "the format id of custom property \"Fmt\""
        } else {
            &format!("custom property {name:?}")
        };
        assert!(
            message.contains(named) && message.contains("a character XML 1.0 cannot carry"),
            "{name}: {message}"
        );
    }
    assert_eq!(doc.to_bytes().unwrap(), before, "nothing changed");
}

#[test]
fn a_raw_custom_value_with_a_character_reference_xml_cannot_carry_is_refused() {
    let mut doc = Document::new();
    let before = doc.to_bytes().unwrap();
    for raw in [
        &b"<vt:lpstr>a&#7;b</vt:lpstr>"[..],
        b"<vt:lpstr>a&#x1F;b</vt:lpstr>",
        b"<vt:lpstr>&#0;</vt:lpstr>",
        b"<vt:lpstr>a&#xD800;</vt:lpstr>",
        "<vt:lpstr><![CDATA[a]]>\u{e9}&#7;</vt:lpstr>".as_bytes(),
    ] {
        let message = doc
            .set_custom_property(custom("Raw", CustomPropertyValue::Raw(raw.to_vec())))
            .expect_err("a reference to a control character is refused")
            .to_string();
        assert!(
            message.contains("custom property \"Raw\"")
                && message.contains("a character XML 1.0 cannot carry"),
            "{message}"
        );
    }
    doc.set_custom_property(custom(
        "Raw",
        CustomPropertyValue::Raw(
            b"<vt:lpstr>a&#9;b&amp;c<![CDATA[&#7;]]><!-- &#7; --><!--x--><![CDATA[&#7;]]></vt:lpstr>"
                .to_vec(),
        ),
    ))
    .expect("a tab reference, an escaped ampersand and literal text are fine");
    assert_ne!(doc.to_bytes().unwrap(), before);
}

/// `doc` saved and reopened after `plain` in `part` became `written`: what
/// another producer wrote, such as a character reference XML 1.0 forbids.
fn written_by_another_producer(
    doc: &mut Document,
    part: &str,
    plain: &str,
    written: &str,
) -> Document {
    let bytes = doc.to_bytes().unwrap();
    let mut package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).unwrap();
    let xml = String::from_utf8(package.get_part(part).expect("the part exists").to_vec()).unwrap();
    assert!(xml.contains(plain), "{xml}");
    package.set_part(part, xml.replace(plain, written).into_bytes());
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    Document::from_bytes(&out.into_inner()).unwrap()
}

/// A document whose core part holds the title "Report" and a
/// `lastModifiedBy` with a character reference XML 1.0 forbids.
fn with_bad_last_modified_by() -> Document {
    let mut doc = Document::new();
    doc.set_core_properties(CoreProperties {
        title: Some("Report".into()),
        last_modified_by: Some("J_Doe".into()),
        ..Default::default()
    })
    .unwrap();
    written_by_another_producer(&mut doc, "/docProps/core.xml", "J_Doe", "J&#11;Doe")
}

#[test]
fn setting_core_properties_drops_a_bad_character_the_file_already_held() {
    // The part is written again from the properties, so the character the
    // file held cannot stay; the caller, CLI or Python, is not stuck on it.
    let mut doc = with_bad_last_modified_by();
    assert_eq!(
        doc.core_properties().unwrap().last_modified_by.as_deref(),
        Some("J\u{b}Doe")
    );
    let mut core = doc.core_properties().cloned().unwrap();
    core.title = Some("Q3\u{7}".into());
    let message = doc
        .set_core_properties(core.clone())
        .expect_err("a new bad value is still refused")
        .to_string();
    assert!(message.contains("core property title"), "{message}");
    core.title = Some("Q3".into());
    doc.set_core_properties(core)
        .expect("the held value is cleaned");
    let doc = Document::from_bytes(&doc.to_bytes().expect("it saves")).unwrap();
    let core = doc.core_properties().unwrap();
    assert_eq!(core.title.as_deref(), Some("Q3"));
    assert_eq!(core.last_modified_by.as_deref(), Some("JDoe"));

    let mut doc = with_bad_last_modified_by();
    doc.set_title("Q4");
    let doc = Document::from_bytes(&doc.to_bytes().expect("a one-field setter saves")).unwrap();
    assert_eq!(doc.title(), Some("Q4"));
    assert_eq!(
        doc.core_properties().unwrap().last_modified_by.as_deref(),
        Some("JDoe")
    );
}
