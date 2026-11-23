//! Bibliography authoring as the CLI and Python use it: a source added with
//! no GUID gets a derived one, and a citation placed after a piece of text.

use oxoffice_docx::{
    BibliographyAuthor, BibliographyContributor, BibliographyContributorRole, BibliographyOptions,
    BibliographyPerson, BibliographyProperty, BibliographySource, BibliographySourceField,
    BibliographySourceKind, BibliographyStyle, CitationOptions, CitationSourceOptions, Document,
};

use crate::support::{doc_with, reopen, texts};

fn book(tag: &str, last: &str, year: &str) -> BibliographySource {
    BibliographySource {
        tag: tag.to_owned(),
        guid: String::new(),
        kind: BibliographySourceKind::Book,
        locale: None,
        contributors: vec![BibliographyContributor {
            role: BibliographyContributorRole::Author,
            value: BibliographyAuthor::People(vec![BibliographyPerson {
                first: vec!["Ada".to_owned()],
                middle: Vec::new(),
                last: vec![last.to_owned()],
            }]),
        }],
        properties: [
            (BibliographySourceField::Title, "A history"),
            (BibliographySourceField::Year, year),
            (BibliographySourceField::Publisher, "Press"),
        ]
        .into_iter()
        .map(|(field, value)| BibliographyProperty {
            field,
            value: value.to_owned(),
        })
        .collect(),
    }
}

fn cite(tag: &str) -> CitationOptions {
    CitationOptions {
        sources: vec![CitationSourceOptions {
            tag: tag.to_owned(),
            pages: None,
            volume: None,
            prefix: None,
            suffix: None,
            suppress_author: false,
            suppress_year: false,
            suppress_title: false,
        }],
        locale: Some(1033),
    }
}

fn apa() -> BibliographyOptions {
    BibliographyOptions {
        style: BibliographyStyle::ApaSixthEdition,
        locale: None,
        locale_filter: None,
        tags: Vec::new(),
    }
}

fn guids(doc: &Document) -> Vec<(String, Option<String>)> {
    doc.bibliography_sources()
        .unwrap()
        .into_iter()
        .map(|source| (source.tag, source.guid))
        .collect()
}

fn is_version_4_guid(guid: &str) -> bool {
    let Some(inner) = guid.strip_prefix('{').and_then(|g| g.strip_suffix('}')) else {
        return false;
    };
    let groups = inner.split('-').map(str::len).collect::<Vec<_>>();
    groups == [8, 4, 4, 4, 12]
        && inner
            .chars()
            .all(|c| c == '-' || c.is_ascii_digit() || c.is_ascii_uppercase())
        && inner.as_bytes()[14] == b'4'
        && matches!(inner.as_bytes()[19], b'8' | b'9' | b'A' | b'B')
}

#[test]
fn a_source_without_a_guid_gets_one_derived_from_its_content() {
    let mut first = Document::new();
    let guid = first
        .add_bibliography_source(&book("Smith20", "Smith", "2020"))
        .unwrap();
    assert!(is_version_4_guid(&guid), "{guid}");
    assert_eq!(
        guids(&reopen(&mut first)),
        [("Smith20".to_owned(), Some(guid.clone()))]
    );

    // The same source gets the same GUID in another document, a different
    // source another one.
    let mut second = Document::new();
    assert_eq!(
        second
            .add_bibliography_source(&book("Smith20", "Smith", "2020"))
            .unwrap(),
        guid
    );
    let other = second
        .add_bibliography_source(&book("Smith21", "Smith", "2021"))
        .unwrap();
    assert_ne!(other, guid);
    assert!(is_version_4_guid(&other), "{other}");
}

#[test]
fn a_given_guid_is_kept_and_a_derived_one_skips_the_guids_already_held() {
    let mut probe = Document::new();
    let derived = probe
        .add_bibliography_source(&book("Smith20", "Smith", "2020"))
        .unwrap();

    // Another source already holds the GUID Smith20 would get.
    let mut doc = Document::new();
    let mut holder = book("Doe19", "Doe", "2019");
    holder.guid = derived.to_lowercase();
    assert_eq!(doc.add_bibliography_source(&holder).unwrap(), holder.guid);
    let next = doc
        .add_bibliography_source(&book("Smith20", "Smith", "2020"))
        .unwrap();
    assert_ne!(next.to_lowercase(), derived.to_lowercase());
    assert!(is_version_4_guid(&next), "{next}");
    assert_eq!(
        guids(&reopen(&mut doc)),
        [
            ("Doe19".to_owned(), Some(holder.guid.clone())),
            ("Smith20".to_owned(), Some(next)),
        ]
    );

    // A given GUID that is already held is still refused.
    let mut clash = book("Roe18", "Roe", "2018");
    clash.guid = derived;
    let error = doc.add_bibliography_source(&clash).unwrap_err().to_string();
    assert!(error.contains("source identity already exists"), "{error}");
}

#[test]
fn a_citation_goes_right_after_the_chosen_occurrence_of_the_text() {
    let mut doc = doc_with(&["One brave step", "Two brave words and brave deeds"]);
    doc.add_bibliography_source(&book("Smith20", "Smith", "2020"))
        .unwrap();
    doc.insert_citation_after_text("brave", 2, &cite("Smith20"))
        .unwrap();
    let mut doc = reopen(&mut doc);
    doc.set_bibliography_options(&apa()).unwrap();
    doc.update_bibliography().unwrap();
    assert_eq!(
        texts(&reopen(&mut doc)),
        [
            "One brave step",
            "Two brave words and brave (Smith, 2020) deeds"
        ]
    );
}

#[test]
fn a_citation_after_text_reaches_table_cells() {
    let mut doc = doc_with(&["Before"]);
    let mut table = doc.add_table(1, 1);
    table.cell(0, 0).expect("cell").set_text("In the cell");
    doc.add_bibliography_source(&book("Smith20", "Smith", "2020"))
        .unwrap();
    doc.insert_citation_after_text("the cell", 0, &cite("Smith20"))
        .unwrap();
    let mut doc = reopen(&mut doc);
    doc.set_bibliography_options(&apa()).unwrap();
    doc.update_bibliography().unwrap();
    let doc = reopen(&mut doc);
    let cell = doc.tables()[0].cell(0, 0).expect("cell").text();
    assert_eq!(cell, "In the cell (Smith, 2020)");
}

#[test]
fn a_missing_occurrence_leaves_the_document_unchanged() {
    let mut doc = doc_with(&["text and text"]);
    doc.add_bibliography_source(&book("Smith20", "Smith", "2020"))
        .unwrap();
    let before = doc.to_bytes().unwrap();
    let error = doc
        .insert_citation_after_text("text", 2, &cite("Smith20"))
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "bibliography: citation anchor text \"text\" has no occurrence 2: it occurs 2 times in the main story"
    );
    let error = doc
        .insert_citation_after_text("", 0, &cite("Smith20"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("must not be empty"), "{error}");
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn a_replacement_names_its_source_guid_in_any_case_and_keeps_the_saved_one() {
    let mut doc = Document::new();
    let guid = doc
        .add_bibliography_source(&book("Smith20", "Smith", "2020"))
        .unwrap();
    let mut replacement = book("Smith20", "Smith", "2021");
    replacement.guid = guid.to_lowercase();
    doc.replace_bibliography_source(&replacement).unwrap();
    let source = reopen(&mut doc).bibliography_sources().unwrap().remove(0);
    assert_eq!(source.guid, Some(guid.clone()));
    let year = source.supported.unwrap().properties[1].value.clone();
    assert_eq!(year, "2021");

    replacement.guid = "{00000000-0000-4000-8000-000000000000}".to_owned();
    let error = doc.replace_bibliography_source(&replacement).unwrap_err();
    assert_eq!(error.to_string(), "bibliography: source GUID cannot change");
}
