//! Citations and the bibliography: the source collection (add, replace,
//! remove, imported XML kept verbatim), the style and selectors, citation
//! and bibliography fields, and the results an update writes, refused whole
//! when one field is not covered.

use oxoffice_docx::{
    BibliographyAuthor, BibliographyContributor, BibliographyContributorRole, BibliographyOptions,
    BibliographyPerson, BibliographyProperty, BibliographySource, BibliographySourceField,
    BibliographySourceKind, BibliographyStyle, CitationOptions, CitationSourceOptions, Document,
    StoryRunPosition,
};

use crate::support::{doc_with, package_parts, reopen, texts};

const GUID: &str = "{1A2B3C4D-2222-3333-4444-55555555EEEE}";
const OTHER_GUID: &str = "{66666666-7777-8888-9999-000000000000}";

fn person(first: &str, last: &str) -> BibliographyPerson {
    BibliographyPerson {
        first: vec![first.into()],
        middle: Vec::new(),
        last: vec![last.into()],
    }
}

fn people(role: BibliographyContributorRole, names: &[(&str, &str)]) -> BibliographyContributor {
    BibliographyContributor {
        role,
        value: BibliographyAuthor::People(
            names
                .iter()
                .map(|(first, last)| person(first, last))
                .collect(),
        ),
    }
}

fn fields(pairs: &[(BibliographySourceField, &str)]) -> Vec<BibliographyProperty> {
    pairs
        .iter()
        .map(|(field, value)| BibliographyProperty {
            field: *field,
            value: (*value).into(),
        })
        .collect()
}

/// A source of `kind` by John Smith, 2020, with the fields its kind shows.
fn source(tag: &str, guid: &str, kind: BibliographySourceKind) -> BibliographySource {
    use BibliographySourceField as F;
    use BibliographySourceKind as K;
    let role = match kind {
        K::Patent => BibliographyContributorRole::Inventor,
        K::SoundRecording => BibliographyContributorRole::Composer,
        K::Film => BibliographyContributorRole::Director,
        K::Interview => BibliographyContributorRole::Interviewee,
        K::Art => BibliographyContributorRole::Artist,
        K::Performance => BibliographyContributorRole::Writer,
        _ => BibliographyContributorRole::Author,
    };
    let mut pairs = vec![
        (F::Title, "A study of things"),
        (F::Year, "2020"),
        (F::City, "Bordeaux"),
        (F::Publisher, "Acme"),
    ];
    pairs.extend_from_slice(match kind {
        K::JournalArticle => &[(F::JournalName, "Journal"), (F::Pages, "1-10")][..],
        K::ArticleInAPeriodical => &[(F::PeriodicalTitle, "Weekly")],
        K::BookSection => &[(F::BookTitle, "Big book"), (F::Pages, "3-9")],
        K::ConferenceProceedings => &[(F::ConferenceName, "Conf")],
        K::Report => &[(F::Institution, "Inst")],
        K::Patent => &[(F::CountryRegion, "France"), (F::PatentNumber, "123")],
        K::Case => &[(F::ShortTitle, "Case"), (F::Court, "Court")],
        K::InternetSite | K::DocumentFromInternetSite => &[
            (F::InternetSiteTitle, "Site"),
            (F::Url, "https://example.org"),
        ],
        _ => &[],
    });
    if kind != K::Case {
        pairs.push((F::ShortTitle, "Study"));
    }
    BibliographySource {
        tag: tag.into(),
        guid: guid.into(),
        kind,
        locale: None,
        contributors: vec![people(role, &[("John", "Smith")])],
        properties: fields(&pairs),
    }
}

fn member(tag: &str) -> CitationSourceOptions {
    CitationSourceOptions {
        tag: tag.into(),
        pages: None,
        volume: None,
        prefix: None,
        suffix: None,
        suppress_author: false,
        suppress_year: false,
        suppress_title: false,
    }
}

fn options(style: BibliographyStyle) -> BibliographyOptions {
    BibliographyOptions {
        style,
        locale: None,
        locale_filter: None,
        tags: Vec::new(),
    }
}

/// "Cited here " followed by a citation of `members`, then an empty paragraph
/// before which a bibliography in `style` is inserted.
fn cited(
    sources: &[BibliographySource],
    members: Vec<CitationSourceOptions>,
    style: BibliographyStyle,
) -> Document {
    let mut doc = doc_with(&["Cited here ", ""]);
    for source in sources {
        doc.add_bibliography_source(source)
            .expect("source is added");
    }
    let location = doc
        .direct_body_item_location(0)
        .expect("first paragraph is located")
        .expect("first paragraph exists");
    doc.insert_citation(
        &StoryRunPosition {
            location,
            run_index: 1,
        },
        &CitationOptions {
            sources: members,
            locale: None,
        },
    )
    .expect("citation is inserted");
    let end = doc
        .direct_body_item_location(1)
        .expect("second paragraph is located")
        .expect("second paragraph exists");
    doc.insert_bibliography(&end, &options(style))
        .expect("bibliography is inserted");
    doc
}

fn main_xml(doc: &mut Document) -> String {
    let bytes = doc.to_bytes().expect("document saves");
    String::from_utf8(
        package_parts(&bytes)
            .remove("word/document.xml")
            .expect("main part"),
    )
    .expect("main part is UTF-8")
}

fn source_xml(doc: &mut Document) -> String {
    let bytes = doc.to_bytes().expect("document saves");
    package_parts(&bytes)
        .into_iter()
        .filter(|(name, _)| name.starts_with("customXml/item") && !name.contains("Props"))
        .map(|(_, xml)| String::from_utf8(xml).expect("source part is UTF-8"))
        .find(|xml| xml.contains("bibliography"))
        .expect("source collection part")
}

fn refusal<T: std::fmt::Debug>(result: oxoffice_docx::Result<T>) -> String {
    result
        .expect_err("the call was expected to fail")
        .to_string()
}

#[test]
fn a_source_reads_back_as_written_after_a_save() {
    let mut book = source("Smith20", GUID, BibliographySourceKind::Book);
    book.locale = Some(1036);
    book.contributors.push(people(
        BibliographyContributorRole::Editor,
        &[("Ann", "Doe"), ("Bo", "Roe")],
    ));
    book.contributors.push(BibliographyContributor {
        role: BibliographyContributorRole::Author,
        value: BibliographyAuthor::Corporate("WHO".into()),
    });
    let mut doc = Document::new();
    doc.add_bibliography_source(&book).unwrap();
    let doc = reopen(&mut doc);
    let sources = doc.bibliography_sources().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].tag, "Smith20");
    assert_eq!(sources[0].guid.as_deref(), Some(GUID));
    assert_eq!(sources[0].source_type.as_deref(), Some("Book"));
    assert_eq!(sources[0].supported.as_ref(), Some(&book));
}

#[test]
fn a_source_identity_is_unique_and_a_refused_add_changes_nothing() {
    let mut doc = Document::new();
    doc.add_bibliography_source(&source("Smith20", GUID, BibliographySourceKind::Book))
        .unwrap();
    let before = source_xml(&mut doc);
    let same_tag = source("Smith20", OTHER_GUID, BibliographySourceKind::Book);
    assert!(refusal(doc.add_bibliography_source(&same_tag)).contains("already exists"));
    // The GUID matches without braces and whatever its case.
    let same_guid = source(
        "Other",
        "1a2b3c4d-2222-3333-4444-55555555eeee",
        BibliographySourceKind::Book,
    );
    assert!(refusal(doc.add_bibliography_source(&same_guid)).contains("already exists"));
    assert_eq!(source_xml(&mut doc), before);
}

#[test]
fn a_replaced_source_keeps_its_guid_and_imported_xml() {
    let mut doc = Document::new();
    doc.add_bibliography_source(&source("Smith20", GUID, BibliographySourceKind::Book))
        .unwrap();
    // A member Word writes that the model does not know, such as a medium.
    let bytes = doc.to_bytes().unwrap();
    let mut parts = package_parts(&bytes);
    let (name, xml) = parts
        .iter_mut()
        .find(|(name, xml)| {
            name.starts_with("customXml/item") && xml.windows(6).any(|w| w == b"b:Year")
        })
        .expect("source collection part");
    let name = name.clone();
    let patched = String::from_utf8(xml.clone()).unwrap().replacen(
        "</b:Source>",
        "<b:Producer x=\"1\">kept</b:Producer></b:Source>",
        1,
    );
    let mut doc = Document::from_bytes(&rezip(&bytes, &name, patched.as_bytes())).unwrap();

    let mut replacement = source("Smith20", GUID, BibliographySourceKind::Book);
    replacement.properties[0].value = "A new title".into();
    doc.replace_bibliography_source(&replacement).unwrap();
    let xml = source_xml(&mut doc);
    assert!(xml.contains("A new title"), "{xml}");
    assert!(
        xml.contains("<b:Producer x=\"1\">kept</b:Producer>"),
        "{xml}"
    );

    replacement.guid = OTHER_GUID.into();
    assert!(refusal(doc.replace_bibliography_source(&replacement)).contains("GUID cannot change"));
    let missing = source("Nobody", GUID, BibliographySourceKind::Book);
    assert!(refusal(doc.replace_bibliography_source(&missing)).contains("does not exist"));
}

fn rezip(bytes: &[u8], name: &str, data: &[u8]) -> Vec<u8> {
    use std::io::{Cursor, Write};
    let parts = package_parts(bytes);
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (part, content) in parts {
        out.start_file(part.as_str(), zip::write::SimpleFileOptions::default())
            .unwrap();
        out.write_all(if part == name { data } else { &content })
            .unwrap();
    }
    out.finish().unwrap().into_inner()
}

#[test]
fn a_cited_source_cannot_be_removed_and_an_uncited_one_can() {
    let book = source("Smith20", GUID, BibliographySourceKind::Book);
    let other = source("Doe19", OTHER_GUID, BibliographySourceKind::Book);
    let mut doc = cited(
        &[book, other],
        vec![member("Smith20")],
        BibliographyStyle::ApaSixthEdition,
    );
    assert!(
        refusal(doc.remove_bibliography_source("Smith20")).contains("referenced by a citation")
    );
    doc.remove_bibliography_source("Doe19").unwrap();
    let tags: Vec<_> = doc
        .bibliography_sources()
        .unwrap()
        .into_iter()
        .map(|source| source.tag)
        .collect();
    assert_eq!(tags, ["Smith20"]);
}

#[test]
fn a_citation_writes_each_member_switch_after_its_tag() {
    let book = source("Smith20", GUID, BibliographySourceKind::Book);
    let other = source("Doe19", OTHER_GUID, BibliographySourceKind::Book);
    let mut first = member("Smith20");
    first.pages = Some("12".into());
    first.suppress_author = true;
    let mut second = member("Doe19");
    second.volume = Some("2".into());
    second.prefix = Some("see ".into());
    second.suffix = Some(" ff.".into());
    second.suppress_year = true;
    second.suppress_title = true;
    let mut doc = cited(
        &[book, other],
        vec![first, second],
        BibliographyStyle::ApaSixthEdition,
    );
    let xml = main_xml(&mut doc);
    assert!(
        xml.contains(r#"CITATION Smith20 \p 12 \n \m Doe19 \v 2 \f &quot;see &quot; \s &quot; ff.&quot; \y \t "#),
        "{xml}"
    );
}

#[test]
fn bibliography_options_set_the_style_and_the_selectors_of_existing_bibliographies() {
    let book = source("Smith20", GUID, BibliographySourceKind::Book);
    let mut doc = cited(
        &[book],
        vec![member("Smith20")],
        BibliographyStyle::ApaSixthEdition,
    );
    let style = doc.bibliography_style().unwrap().expect("style metadata");
    assert_eq!(
        style.supported_style,
        Some(BibliographyStyle::ApaSixthEdition)
    );
    assert_eq!(style.style_key.as_deref(), Some("APA"));

    doc.set_bibliography_options(&BibliographyOptions {
        style: BibliographyStyle::Chicago,
        locale: Some(1033),
        locale_filter: Some(1036),
        tags: vec!["Smith20".into()],
    })
    .unwrap();
    let style = doc.bibliography_style().unwrap().expect("style metadata");
    assert_eq!(style.supported_style, Some(BibliographyStyle::Chicago));
    assert_eq!(style.style_path.as_deref(), Some("\\CHICAGO.XSL"));
    let xml = main_xml(&mut doc);
    assert!(
        xml.contains(r"\l 1033 \f 1036 \m Smith20</w:instrText>"),
        "{xml}"
    );
    assert!(xml.contains("CITATION Smith20"), "{xml}");
}

#[test]
fn apa_formats_every_source_kind() {
    use BibliographySourceKind as K;
    let cases = [
        (
            K::ArticleInAPeriodical,
            "(Smith, 2020)",
            "Smith, J. (2020). A study of things. Weekly.",
        ),
        (
            K::Book,
            "(Smith, 2020)",
            "Smith, J. (2020). A study of things. Bordeaux: Acme.",
        ),
        (
            K::BookSection,
            "(Smith, 2020)",
            "Smith, J. (2020). A study of things. In Big book (pp. 3-9). Bordeaux: Acme.",
        ),
        (
            K::JournalArticle,
            "(Smith, 2020)",
            "Smith, J. (2020). A study of things. Journal, 1-10.",
        ),
        (
            K::ConferenceProceedings,
            "(Smith, 2020)",
            "Smith, J. (2020). A study of things. Conf. Bordeaux: Acme.",
        ),
        (
            K::Report,
            "(Smith, 2020)",
            "Smith, J. (2020). A study of things. Inst. Bordeaux: Acme.",
        ),
        (
            K::SoundRecording,
            "(Smith, 2020)",
            "Smith, J. (2020). A study of things. Bordeaux.",
        ),
        (
            K::Performance,
            "(Smith, 2020)",
            "Smith, J. (2020). A study of things. Bordeaux.",
        ),
        (
            K::Art,
            "(Smith, 2020)",
            "Smith, J. (2020). A study of things. Bordeaux.",
        ),
        (
            K::DocumentFromInternetSite,
            "(Smith, 2020)",
            "Smith, J. (2020). A study of things. Retrieved from Site: https://example.org",
        ),
        (
            K::InternetSite,
            "(Smith, 2020)",
            "Smith, J. (2020). A study of things. Retrieved from Site: https://example.org",
        ),
        (
            K::Film,
            "(Smith, 2020)",
            "Smith, J. (Director). (2020). A study of things [Motion Picture].",
        ),
        (
            K::Interview,
            "(Smith, 2020)",
            "Smith, J. (2020). A study of things. Bordeaux: Acme.",
        ),
        (
            K::Patent,
            "(France Patent No. 123, 2020)",
            "Smith, J. (2020). France Patent No. 123.",
        ),
        (
            K::ElectronicSource,
            "(Smith, 2020)",
            "Smith, J. (2020). A study of things. Bordeaux: Acme.",
        ),
        (K::Case, "(Case, 2020)", "A study of things (Court 2020)."),
        (
            K::Misc,
            "(Smith, 2020)",
            "Smith, J. (2020). A study of things. Bordeaux: Acme.",
        ),
    ];
    for (kind, citation, entry) in cases {
        let mut doc = cited(
            &[source("Smith20", GUID, kind)],
            vec![member("Smith20")],
            BibliographyStyle::ApaSixthEdition,
        );
        let report = doc.update_bibliography_with_default_locale(1033).unwrap();
        assert_eq!(
            (report.updated_citations, report.rebuilt_bibliographies),
            (1, 1),
            "{kind:?}"
        );
        let texts = texts(&reopen(&mut doc));
        assert_eq!(texts[0], format!("Cited here  {citation}"), "{kind:?}");
        assert_eq!(texts[1], entry, "{kind:?}");
    }
}

#[test]
fn every_style_formats_a_book() {
    use BibliographyStyle as S;
    let cases = [
        (
            S::Chicago,
            "(Smith 2020)",
            "Smith, John. 2020. A study of things. Bordeaux: Acme.",
        ),
        (
            S::Gb7714,
            "(Smith, 2020)",
            "Smith, John. 2020. A study of things. Bordeaux\u{a0}: Acme, 2020.",
        ),
        (
            S::GostName,
            "(Smith, 2020)",
            "Smith John A study of things [Book].\u{a0}- Bordeaux\u{a0}: Acme, 2020.",
        ),
        (
            S::GostTitle,
            "(Smith, 2020)",
            "A study of things [Book]\u{a0}/ auth. Smith John.\u{a0}- Bordeaux\u{a0}: Acme, 2020.",
        ),
        (
            S::HarvardAnglia,
            "(Smith, 2020)",
            "Smith, J., 2020. A study of things. Bordeaux: Acme.",
        ),
        (
            S::Iso690AuthorDate,
            "(Smith, 2020)",
            "Smith, John. 2020. A study of things. Bordeaux\u{a0}: Acme, 2020.",
        ),
        (
            S::Iso690Numeric,
            "(1)",
            "1. Smith, John. A study of things. Bordeaux\u{a0}: Acme, 2020.",
        ),
        (
            S::MlaSeventhEdition,
            "(Smith)",
            "Smith, John. A study of things. Bordeaux: Acme, 2020.",
        ),
        (
            S::Sist02,
            "（Smith，2020）",
            "SmithJohn A study of things．Bordeaux，Acme，2020．",
        ),
        (
            S::Turabian,
            "(Smith 2020)",
            "Smith, John. A study of things. Bordeaux: Acme, 2020.",
        ),
    ];
    for (style, citation, entry) in cases {
        let mut doc = cited(
            &[source("Smith20", GUID, BibliographySourceKind::Book)],
            vec![member("Smith20")],
            style,
        );
        doc.update_bibliography_with_default_locale(1033).unwrap();
        let texts = texts(&reopen(&mut doc));
        assert_eq!(texts[0], format!("Cited here  {citation}"), "{style:?}");
        assert_eq!(texts[1], entry, "{style:?}");
    }
}

#[test]
fn ieee_numbers_the_citation_and_lays_the_bibliography_out_as_a_table() {
    let mut doc = cited(
        &[source("Smith20", GUID, BibliographySourceKind::Book)],
        vec![member("Smith20")],
        BibliographyStyle::Ieee,
    );
    doc.update_bibliography_with_default_locale(1033).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc)[0], "Cited here  [1]");
    let tables = doc.tables();
    assert_eq!(tables.len(), 1);
    let table = &tables[0];
    assert_eq!(table.cell(0, 0).expect("number cell").text(), "[1] ");
    let entry = table.cell(0, 1).expect("entry cell").text();
    assert!(entry.contains("A study of things"), "{entry}");
}

#[test]
fn an_apa_citation_shows_its_page() {
    let mut cite = member("Smith20");
    cite.pages = Some("12".into());
    let mut doc = cited(
        &[source("Smith20", GUID, BibliographySourceKind::Book)],
        vec![cite],
        BibliographyStyle::ApaSixthEdition,
    );
    doc.update_bibliography_with_default_locale(1033).unwrap();
    // Whether the short title shows beside the page is left to the Word oracle.
    let citation = &texts(&doc)[0];
    assert!(citation.starts_with("Cited here  (Smith, "), "{citation}");
    assert!(citation.ends_with(", 2020, p. 12)"), "{citation}");
}

#[test]
fn an_update_needs_a_locale_when_neither_the_field_nor_the_source_gives_one() {
    let mut doc = cited(
        &[source("Smith20", GUID, BibliographySourceKind::Book)],
        vec![member("Smith20")],
        BibliographyStyle::ApaSixthEdition,
    );
    let before = main_xml(&mut doc);
    let message = refusal(doc.update_bibliography());
    assert!(message.contains("application default locale"), "{message}");
    assert_eq!(main_xml(&mut doc), before);
}

#[test]
fn an_uncovered_field_refuses_the_whole_update_and_names_what_is_missing() {
    let journal = source("Smith20", GUID, BibliographySourceKind::JournalArticle);
    let mut book = source("Doe19", OTHER_GUID, BibliographySourceKind::Book);
    book.contributors = vec![people(
        BibliographyContributorRole::Author,
        &[("Ann", "Doe")],
    )];
    let mut doc = cited(
        &[journal, book],
        vec![member("Doe19")],
        BibliographyStyle::Chicago,
    );
    let before = main_xml(&mut doc);
    let message = refusal(doc.update_bibliography_with_default_locale(1033));
    assert_eq!(
        message,
        "bibliography: Chicago has no bibliography entry yet for the JournalArticle source \"Smith20\" in locale 1033"
    );
    // The citation of the book is covered, but nothing is published.
    assert_eq!(main_xml(&mut doc), before);
}

#[test]
fn a_refusal_names_the_role_field_or_author_count_it_cannot_render() {
    use BibliographyContributorRole as R;
    let mut edited = source("Edited", GUID, BibliographySourceKind::Book);
    edited
        .contributors
        .push(people(R::Editor, &[("Ann", "Doe")]));
    let mut paged = source("Paged", GUID, BibliographySourceKind::Book);
    paged
        .properties
        .extend(fields(&[(BibliographySourceField::Pages, "320")]));
    let mut pair = source("Pair", GUID, BibliographySourceKind::Book);
    pair.contributors = vec![people(R::Author, &[("John", "Smith"), ("Ann", "Doe")])];
    let cases = [
        (
            edited,
            "Chicago has no bibliography entry with the Editor role yet for the Book source \"Edited\" in locale 1033",
        ),
        (
            paged,
            "Chicago has no bibliography entry with the Pages field yet for the Book source \"Paged\" in locale 1033",
        ),
        (
            pair,
            "Chicago has no bibliography entry with more than one author yet for the Book source \"Pair\" in locale 1033",
        ),
    ];
    // The cited book is covered, so the refusal comes from the uncovered entry.
    let mut covered = source("Covered", OTHER_GUID, BibliographySourceKind::Book);
    covered.contributors = vec![people(R::Author, &[("Lena", "Lund")])];
    for (book, expected) in cases {
        let mut doc = cited(
            &[covered.clone(), book],
            vec![member("Covered")],
            BibliographyStyle::Chicago,
        );
        let message = refusal(doc.update_bibliography_with_default_locale(1033));
        assert_eq!(message, format!("bibliography: {expected}"));
    }
}
