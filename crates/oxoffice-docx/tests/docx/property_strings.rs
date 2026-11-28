//! Core, application and custom properties refuse a character XML cannot
//! carry instead of making every later save fail.

use oxoffice_docx::{AppProperties, CustomProperty, CustomPropertyValue, Document};

fn custom(name: &str, value: CustomPropertyValue) -> CustomProperty {
    CustomProperty {
        fmtid: "{D5CDD505-2E9C-101B-9397-08002B2CF9AE}".into(),
        pid: 2,
        name: Some(name.into()),
        value,
    }
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

#[test]
fn every_refusal_of_a_character_xml_cannot_carry_holds_the_shared_words() {
    // The Python bindings tell such a refusal apart by these words, wherever
    // it was wrapped; the package crate spells them out on its own.
    let words = oxoffice_core::xml::NON_XML_CHARACTER_REFUSAL;
    let package = oxoffice_opc::OpcError::InvalidXmlCharacter {
        part: "/word/document.xml".to_owned(),
        code_point: 7,
        line: 1,
        column: 1,
    };
    assert!(package.to_string().contains(words), "{package}");
    let value = oxoffice_core::xml::reject_non_xml_characters("value", "\u{7}").unwrap_err();
    assert!(value.to_string().contains(words), "{value}");
}
