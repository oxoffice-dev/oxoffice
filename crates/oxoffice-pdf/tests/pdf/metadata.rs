//! Document metadata: the catalog's language.

use lopdf::Object;
use oxoffice_layout::{DocumentMetadata, Point};

use crate::support::{Pdf, layout, run};

fn language(pdf: &Pdf) -> Option<Vec<u8>> {
    pdf.catalog()
        .get(b"Lang")
        .ok()
        .map(|lang| pdf.resolve(lang).as_str().unwrap().to_vec())
}

#[test]
fn an_untagged_pdf_names_the_language_of_its_document_when_it_is_a_language_tag() {
    let with = |value: Option<&str>| {
        let mut result = layout(vec![vec![run(
            "Guten Tag",
            Point { x: 72.0, y: 72.0 },
            12.0,
        )]]);
        result.metadata = Some(DocumentMetadata {
            language: value.map(str::to_owned),
            ..Default::default()
        });
        assert!(result.structure.is_none(), "the layout is untagged");
        Pdf::read(&oxoffice_pdf::render_to_pdf(&result))
    };
    assert_eq!(
        language(&with(Some("de-DE"))).as_deref(),
        Some(&b"de-DE"[..])
    );
    // Without a language, or with a value that names none, an untagged file
    // says nothing.
    for value in [None, Some("x-none"), Some("0409"), Some("de DE"), Some("")] {
        assert_eq!(language(&with(value)), None, "{value:?}");
    }
    assert!(matches!(
        with(Some("de-DE")).catalog().get(b"Lang"),
        Ok(Object::String(..))
    ));
}
