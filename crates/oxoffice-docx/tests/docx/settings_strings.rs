//! The strings of `word/settings.xml`: document variables, mail-merge
//! settings and the other string settings, as read back after a save.

use oxoffice_docx::{Document, MathProperties};

use crate::support::reopen;

/// Math properties with only a math font.
fn math_font(font: &str) -> MathProperties {
    let mut properties = MathProperties::default();
    properties.math_font = Some(font.into());
    properties
}

#[test]
fn a_tab_a_line_feed_or_a_carriage_return_in_the_math_font_survives_a_save() {
    let font = "Cambria\tMath\nBold\rItalic";
    let mut doc = Document::new();
    doc.set_math_properties(math_font(font)).unwrap();
    let mut doc = reopen(&mut doc);
    let read = |doc: &Document| {
        doc.math_properties()
            .and_then(|properties| properties.math_font.clone())
    };
    assert_eq!(read(&doc).as_deref(), Some(font));
    // The properties now come from the file: setting them again rewrites
    // each `m:` element read from the source, the font among them.
    let edited = "Asana\tMath\nLight\rWide";
    let mut properties = doc
        .math_properties()
        .cloned()
        .expect("math properties read from the file");
    properties.math_font = Some(edited.into());
    properties.small_fraction = Some(true);
    doc.set_math_properties(properties).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(read(&doc).as_deref(), Some(edited));
    assert_eq!(
        doc.math_properties()
            .and_then(|properties| properties.small_fraction),
        Some(true)
    );
}
