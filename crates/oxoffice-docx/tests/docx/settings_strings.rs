//! The strings of `word/settings.xml`: document variables, mail-merge
//! settings and the other string settings, as read back after a save.

use std::io::Cursor;

use oxoffice_docx::{Document, MailMerge, MathProperties, ThemeFontLanguage};
use oxoffice_opc::OpcPackage;

use crate::support::reopen;

/// A new document whose `word/settings.xml` holds `children` inside
/// `w:settings`.
fn doc_with_settings(children: &str) -> Document {
    let bytes = Document::new().to_bytes().expect("new document saves");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let xml = String::from_utf8(
        package
            .get_part("/word/settings.xml")
            .expect("settings part")
            .to_vec(),
    )
    .expect("settings are UTF-8");
    let at = xml.find("</w:settings>").expect("settings end");
    let xml = format!("{}{children}{}", &xml[..at], &xml[at..]);
    package.set_part("/word/settings.xml", xml.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    Document::from_bytes(&out.into_inner()).expect("document with these settings opens")
}

#[test]
fn a_line_feed_or_a_tab_in_a_document_variable_survives_a_save() {
    let mut doc = Document::new();
    doc.set_document_variable("Address", "1 Main Street\nSpringfield\tUSA")
        .unwrap();
    doc.set_document_variable("Line", "first\r\nsecond")
        .unwrap();
    doc.set_document_variable("Return", "first\rsecond")
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.document_variable("Address"),
        Some("1 Main Street\nSpringfield\tUSA")
    );
    assert_eq!(doc.document_variable("Line"), Some("first\r\nsecond"));
    assert_eq!(doc.document_variable("Return"), Some("first\rsecond"));
}

#[test]
fn setting_a_variable_keeps_the_line_feeds_another_producer_wrote_in_the_others() {
    let mut doc = doc_with_settings(
        r#"<w:docVars><w:docVar w:name="Kept" w:val="one&#xA;two&#9;three"/></w:docVars>"#,
    );
    assert_eq!(doc.document_variable("Kept"), Some("one\ntwo\tthree"));
    doc.set_document_variable("Added", "x").unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.document_variable("Kept"), Some("one\ntwo\tthree"));
    assert_eq!(doc.document_variable("Added"), Some("x"));
}

#[test]
fn a_line_feed_in_a_merge_query_and_other_string_settings_survives_a_save() {
    let mut doc = Document::new();
    doc.set_mail_merge_settings(MailMerge {
        query: Some("SELECT *\nFROM clients".into()),
        connect_string: Some("DSN=a;\tUID=b".into()),
        ..Default::default()
    })
    .unwrap();
    doc.set_compatibility_setting("probe", "http://example.com/compat", "first\nsecond")
        .unwrap();
    doc.set_theme_font_language(ThemeFontLanguage {
        latin: Some("en\tUS".into()),
        ..Default::default()
    })
    .unwrap();
    doc.set_list_separator("\t").unwrap();
    let doc = reopen(&mut doc);
    let merge = doc.mail_merge_settings().expect("mail merge settings");
    assert_eq!(merge.query.as_deref(), Some("SELECT *\nFROM clients"));
    assert_eq!(merge.connect_string.as_deref(), Some("DSN=a;\tUID=b"));
    let compat = doc
        .compatibility_settings()
        .iter()
        .find(|setting| setting.name == "probe")
        .expect("compatibility setting");
    assert_eq!(compat.value, "first\nsecond");
    assert_eq!(
        doc.theme_font_language()
            .and_then(|language| language.latin.as_deref()),
        Some("en\tUS")
    );
    assert_eq!(doc.list_separator(), Some("\t"));
}

/// Math properties with only a math font.
fn math_font(font: &str) -> MathProperties {
    let mut properties = MathProperties::default();
    properties.math_font = Some(font.into());
    properties
}

#[test]
fn a_settings_string_xml_cannot_carry_is_refused_and_the_document_still_saves() {
    let mut doc = Document::new();
    doc.set_document_variable("Kept", "fine").unwrap();
    let before = doc.to_bytes().unwrap();
    let refusals = [
        doc.set_document_variable("Bell", "ring\u{7}"),
        doc.set_document_variable("Bad\u{1}name", "value"),
        doc.set_mail_merge_settings(MailMerge {
            query: Some("a\u{7}".into()),
            ..Default::default()
        }),
        doc.set_compatibility_setting("probe", "http://example.com/compat", "\u{FFFE}"),
        doc.set_theme_font_language(ThemeFontLanguage {
            latin: Some("en\u{FFFF}".into()),
            ..Default::default()
        }),
        doc.set_decimal_symbol("\u{8}"),
        doc.set_list_separator("\u{1B}"),
        doc.set_default_table_style("Grid\u{0}"),
        doc.set_math_properties(math_font("Cambria\u{2}Math")),
    ];
    for (index, refusal) in refusals.into_iter().enumerate() {
        let message = refusal
            .expect_err(&format!("setter {index} refuses"))
            .to_string();
        assert!(
            message.contains("a character XML 1.0 cannot carry"),
            "setter {index}: {message}"
        );
    }
    assert_eq!(
        doc.to_bytes().unwrap(),
        before,
        "nothing changed, and it still saves"
    );
}

#[test]
fn an_opened_document_refuses_a_bad_settings_string_and_names_the_variable() {
    let mut doc =
        doc_with_settings(r#"<w:docVars><w:docVar w:name="Kept" w:val="fine"/></w:docVars>"#);
    let before = doc.to_bytes().unwrap();
    let message = doc
        .set_document_variable("Bell", "ring\u{7}")
        .expect_err("refused")
        .to_string();
    assert!(
        message.contains("\"Bell\"") && message.contains("U+0007"),
        "{message}"
    );
    assert!(
        doc.set_mail_merge_settings(MailMerge {
            query: Some("a\u{7}".into()),
            ..Default::default()
        })
        .is_err()
    );
    assert_eq!(doc.to_bytes().unwrap(), before, "nothing changed");
}

#[test]
fn a_variable_another_producer_wrote_with_a_bad_character_is_named_when_another_is_set() {
    // XML readers take a character reference to U+0007, which no document
    // may hold: setting another variable rewrites the group and names it.
    let mut doc =
        doc_with_settings(r#"<w:docVars><w:docVar w:name="Bell" w:val="ring&#7;"/></w:docVars>"#);
    let message = doc
        .set_document_variable("Added", "x")
        .expect_err("refused")
        .to_string();
    assert!(message.contains("\"Bell\""), "{message}");
}
