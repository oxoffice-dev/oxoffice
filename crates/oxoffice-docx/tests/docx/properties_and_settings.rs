//! Document properties (core, application and custom, of every value type)
//! and the settings and web settings parts: each setting is set, saved, read
//! back, removed, and the settings a producer wrote around it are kept.

use std::io::{Cursor, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};

use oxoffice_docx::{
    AppProperties, CharacterSpacingControl, CompatibilityOption, CoreProperties,
    CryptAlgorithmClass, CryptAlgorithmType, CryptProviderType, CustomProperty,
    CustomPropertyValue, Document, DocumentProofState, DocumentProtection, DocumentView,
    DocumentZoom, LimitLocation, MailMerge, MailMergeDestination, MailMergeDocumentType,
    MathJustification, MathProperties, ProofState, ProtectionMode, Result, SettingsDiagnostic,
    SettingsDiagnosticReason, ThemeFontLanguage, Twips, WordCreationProfile, WordPackageClass,
    ZoomKind,
};
use oxoffice_opc::OpcPackage;

use crate::support::{corpus_doc, corpus_root, doc_with, reopen, texts};

/// The format id Word gives every user-defined custom property.
const USER_DEFINED: &str = "{D5CDD505-2E9C-101B-9397-08002B2CF9AE}";

/// The URI of Word's own compatibility settings.
const WORD_URI: &str = "http://schemas.microsoft.com/office/word";

const CORE_RELATIONSHIP: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
const APP_RELATIONSHIP: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties";
const CUSTOM_RELATIONSHIP: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties";
const SETTINGS_RELATIONSHIP: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings";

fn custom(name: &str, pid: i32, value: CustomPropertyValue) -> CustomProperty {
    CustomProperty {
        fmtid: USER_DEFINED.to_owned(),
        pid,
        name: Some(name.to_owned()),
        value,
    }
}

/// The names of the custom properties, in package order.
fn custom_names(doc: &Document) -> Vec<String> {
    doc.custom_properties()
        .iter()
        .map(|property| property.name.clone().unwrap_or_default())
        .collect()
}

/// The saved settings part.
fn settings(doc: &Document) -> String {
    doc.part_xml("word/settings.xml")
        .expect("settings part is present")
}

/// The saved web settings part.
fn web_settings(doc: &Document) -> String {
    doc.part_xml("word/webSettings.xml")
        .expect("web settings part is present")
}

fn has_part(doc: &Document, part: &str) -> bool {
    doc.part_xml(part).is_ok()
}

/// How many elements of this qualified name the XML holds, `w:zoom` matching
/// `<w:zoom/>` and `<w:zoom w:percent="90"/>` but not `<w:zoomed/>`.
fn occurrences(xml: &str, element: &str) -> usize {
    let open = format!("<{element}");
    xml.match_indices(&open)
        .filter(|(at, _)| {
            matches!(
                xml.as_bytes().get(at + open.len()),
                Some(b' ' | b'/' | b'>')
            )
        })
        .count()
}

/// Where this fragment starts in the XML.
fn position(xml: &str, fragment: &str) -> usize {
    xml.find(fragment)
        .unwrap_or_else(|| panic!("{fragment} is missing from {xml}"))
}

/// Each fragment is present, and they come in this order.
fn assert_in_order(xml: &str, fragments: &[&str]) {
    let positions: Vec<usize> = fragments.iter().map(|f| position(xml, f)).collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "{fragments:?} are not in this order in {xml}"
    );
}

/// One part of a corpus file as it is stored, before oxoffice opens it.
fn corpus_part(file: &str, part: &str) -> String {
    let bytes = std::fs::read(corpus_root().join("files").join(file)).expect("corpus file");
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("corpus file is a ZIP");
    let mut entry = zip.by_name(part).expect("corpus part");
    let mut xml = String::new();
    entry
        .read_to_string(&mut xml)
        .expect("corpus part is UTF-8");
    xml
}

/// The document with one existing part's bytes replaced: settings or web
/// settings as a producer writes them, which the facade would not write.
fn with_part(mut doc: Document, part: &str, xml: &str) -> Document {
    let bytes = doc.to_bytes().expect("document saves");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    assert!(package.get_part(part).is_some(), "{part} exists already");
    package.set_part(part, xml.as_bytes().to_vec());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    Document::from_bytes(&out.into_inner()).expect("document with this part opens")
}

/// A new document whose settings part holds these children.
fn doc_with_settings(children: &str) -> Document {
    let xml = format!(
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            r#"<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#,
            r#" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml""#,
            r#" xmlns:x="urn:example:producer">{}</w:settings>"#
        ),
        children
    );
    with_part(doc_with(&["Body"]), "/word/settings.xml", &xml)
}

/// The public fields of math defaults, to compare what was set with what is
/// read back (the model also keeps the source it was read from).
#[derive(Debug, PartialEq)]
struct MathDefaults {
    font: Option<String>,
    justification: Option<MathJustification>,
    margins: (Option<u32>, Option<u32>),
    spacing: [Option<u32>; 4],
    wrap_indent: Option<u32>,
    small_fraction: Option<bool>,
    display_defaults: Option<bool>,
    limits: (Option<LimitLocation>, Option<LimitLocation>),
}

impl From<&MathProperties> for MathDefaults {
    fn from(math: &MathProperties) -> Self {
        Self {
            font: math.math_font.clone(),
            justification: math.justification,
            margins: (math.left_margin, math.right_margin),
            spacing: [
                math.pre_spacing,
                math.post_spacing,
                math.inter_spacing,
                math.intra_spacing,
            ],
            wrap_indent: math.wrap_indent,
            small_fraction: math.small_fraction,
            display_defaults: math.display_defaults,
            limits: (math.integral_limit_location, math.nary_limit_location),
        }
    }
}

// ---- Core properties ----

#[test]
fn title_author_subject_and_keywords_survive_a_save() {
    let mut doc = doc_with(&["Body"]);
    doc.set_title("Rapport annuel — تقرير سنوي");
    doc.set_author("李 Wei & <Partners>");
    doc.set_subject("  Q3 \"figures\"  ");
    doc.set_keywords("finance; 2026; résumé");
    let doc = reopen(&mut doc);
    assert_eq!(doc.title(), Some("Rapport annuel — تقرير سنوي"));
    assert_eq!(doc.author(), Some("李 Wei & <Partners>"));
    assert_eq!(doc.subject(), Some("  Q3 \"figures\"  "));
    assert_eq!(doc.keywords(), Some("finance; 2026; résumé"));
    // The author is the creator of the complete model, and a save stamps
    // nothing of its own.
    let core = doc.core_properties().expect("core properties");
    assert_eq!(core.creator.as_deref(), Some("李 Wei & <Partners>"));
    assert_eq!(
        (&core.created, &core.modified, &core.last_modified_by),
        (&None, &None, &None)
    );
    assert_eq!(texts(&doc), ["Body"]);
}

#[test]
fn every_core_property_round_trips_through_the_complete_model() {
    let mut doc = Document::new();
    let core = CoreProperties {
        title: Some("Annual report".to_owned()),
        creator: Some("Ada".to_owned()),
        subject: Some("Finance".to_owned()),
        description: Some("First line\nsecond line".to_owned()),
        keywords: Some("alpha, beta".to_owned()),
        last_modified_by: Some("Grace".to_owned()),
        created: Some("2026-09-01T08:00:00Z".to_owned()),
        modified: Some("2026-09-02T08:30:00Z".to_owned()),
        category: Some("Reports".to_owned()),
        content_status: Some("Draft".to_owned()),
        identifier: Some("DOC-7".to_owned()),
        language: Some("fr-FR".to_owned()),
        last_printed: Some("2026-09-03T07:00:00Z".to_owned()),
        revision: Some("3".to_owned()),
        version: Some("1.4".to_owned()),
    };
    doc.set_core_properties(core.clone()).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.core_properties(), Some(&core));
    assert_eq!(doc.title(), Some("Annual report"));
    assert_eq!(doc.author(), Some("Ada"));
    // Word reads the two dates only when they are typed as W3C dates.
    let xml = doc.part_xml("docProps/core.xml").unwrap();
    for date in [
        r#"<dcterms:created xsi:type="dcterms:W3CDTF">2026-09-01T08:00:00Z</dcterms:created>"#,
        r#"<dcterms:modified xsi:type="dcterms:W3CDTF">2026-09-02T08:30:00Z</dcterms:modified>"#,
    ] {
        assert!(xml.contains(date), "{date} in {xml}");
    }
}

#[test]
fn setting_one_core_property_keeps_the_others_word_wrote() {
    let mut doc = corpus_doc("word/report.docx");
    let before = doc
        .core_properties()
        .expect("Word wrote core properties")
        .clone();
    assert_eq!(before.title.as_deref(), Some("Quarterly Operations Review"));
    assert_eq!(before.creator.as_deref(), Some("Operations team"));
    assert_eq!(before.last_modified_by.as_deref(), Some("Hadrien Mary"));
    assert_eq!(before.revision.as_deref(), Some("2"));
    assert_eq!(before.created.as_deref(), Some("2026-11-20T22:06:00Z"));
    // Word writes an empty subject: there is none to read.
    assert_eq!(before.subject, None);

    doc.set_subject("Operations");
    let doc = reopen(&mut doc);
    let expected = CoreProperties {
        subject: Some("Operations".to_owned()),
        ..before
    };
    assert_eq!(doc.core_properties(), Some(&expected));
}

#[test]
fn removing_the_core_properties_drops_the_part_and_a_title_brings_it_back() {
    let mut doc = doc_with(&["Body"]);
    doc.set_title("Draft");
    let removed = doc.remove_core_properties().unwrap();
    assert_eq!(
        removed.and_then(|core| core.title).as_deref(),
        Some("Draft")
    );
    assert_eq!(doc.remove_core_properties().unwrap(), None);

    let mut doc = reopen(&mut doc);
    assert_eq!(doc.core_properties(), None);
    assert_eq!(doc.title(), None);
    assert!(!has_part(&doc, "docProps/core.xml"));
    assert!(
        !doc.part_xml("_rels/.rels")
            .unwrap()
            .contains(CORE_RELATIONSHIP)
    );

    doc.set_title("Final");
    let doc = reopen(&mut doc);
    assert_eq!(doc.title(), Some("Final"));
    assert!(
        doc.part_xml("_rels/.rels")
            .unwrap()
            .contains(CORE_RELATIONSHIP)
    );
    assert!(
        doc.part_xml("[Content_Types].xml")
            .unwrap()
            .contains(r#"<Override PartName="/docProps/core.xml""#)
    );
    assert_eq!(texts(&doc), ["Body"]);
}

// ---- Application properties ----

#[test]
fn application_properties_survive_a_save_and_are_removed() {
    let mut doc = doc_with(&["Body"]);
    let mut app = doc
        .application_properties()
        .expect("a new document has application properties")
        .clone();
    assert_eq!(app.application.as_deref(), Some("oxoffice"));
    let version = app.application_version.clone().expect("a version");
    assert!(
        version.len() == 7 && version.as_bytes()[2] == b'.',
        "the version has Word's XX.YYYY shape: {version}"
    );

    app.company = Some("Contoso & Fils".to_owned());
    app.manager = Some("Ana Lima".to_owned());
    app.template = Some("Report.dotx".to_owned());
    app.pages = Some(3);
    app.words = Some(228);
    app.characters = Some(1260);
    app.characters_with_spaces = Some(1486);
    app.lines = Some(10);
    app.paragraphs = Some(2);
    app.total_time = Some(14);
    app.scale_crop = Some(false);
    app.links_up_to_date = Some(true);
    app.shared_document = Some(false);
    app.hyperlinks_changed = Some(true);
    doc.set_application_properties(app.clone()).unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(doc.application_properties(), Some(&app));

    let removed = doc.remove_application_properties().unwrap();
    assert_eq!(removed.as_ref(), Some(&app));
    let doc = reopen(&mut doc);
    assert_eq!(doc.application_properties(), None);
    assert!(!has_part(&doc, "docProps/app.xml"));
    assert!(
        !doc.part_xml("_rels/.rels")
            .unwrap()
            .contains(APP_RELATIONSHIP)
    );
}

#[test]
fn an_application_version_word_cannot_open_is_refused_and_changes_nothing() {
    let mut doc = doc_with(&["Body"]);
    let before = doc.to_bytes().unwrap();
    let mut app = AppProperties::default();
    app.company = Some("Contoso".to_owned());
    for version in ["1.2.3", "16.0", "16.00000", "ab.cdef", ""] {
        app.application_version = Some(version.to_owned());
        let error = doc
            .set_application_properties(app.clone())
            .expect_err(version)
            .to_string();
        assert!(error.contains("XX.YYYY"), "{version}: {error}");
    }
    assert_eq!(doc.to_bytes().unwrap(), before);

    app.application_version = Some("16.0000".to_owned());
    doc.set_application_properties(app.clone()).unwrap();
    assert_eq!(reopen(&mut doc).application_properties(), Some(&app));
}

#[test]
fn editing_word_application_properties_keeps_its_other_entries_in_place() {
    let mut doc = corpus_doc("word/report.docx");
    let mut app = doc
        .application_properties()
        .expect("Word wrote them")
        .clone();
    assert_eq!(app.application.as_deref(), Some("Microsoft Office Word"));
    assert_eq!(app.application_version.as_deref(), Some("16.0000"));
    assert_eq!(app.template.as_deref(), Some("Normal.dotm"));
    assert_eq!(
        (app.pages, app.words, app.lines),
        (Some(3), Some(228), Some(10))
    );
    assert_eq!(app.company.as_deref(), Some(""));

    app.company = Some("Contoso".to_owned());
    app.manager = Some("Ana".to_owned());
    doc.set_application_properties(app).unwrap();
    let doc = reopen(&mut doc);
    let read = doc.application_properties().unwrap();
    assert_eq!(read.company.as_deref(), Some("Contoso"));
    assert_eq!(read.manager.as_deref(), Some("Ana"));
    assert_eq!(read.pages, Some(3));

    // The entries the model does not know stay, and the edited ones stay
    // where Word put them.
    let xml = doc.part_xml("docProps/app.xml").unwrap();
    assert_in_order(
        &xml,
        &[
            "<Application>Microsoft Office Word</Application><DocSecurity>0</DocSecurity>",
            r#"<HeadingPairs><vt:vector size="2" baseType="variant">"#,
            "</TitlesOfParts><Manager>Ana</Manager><Company>Contoso</Company><LinksUpToDate>",
            "<HyperlinkBase></HyperlinkBase>",
        ],
    );
}

// ---- Custom properties ----

#[test]
fn custom_properties_of_every_value_type_survive_a_save() {
    let properties = [
        custom(
            "Client",
            2,
            CustomPropertyValue::Lpwstr("Société Générale — 東京".to_owned()),
        ),
        custom(
            "Code",
            3,
            CustomPropertyValue::Lpstr("R&D <budget>".to_owned()),
        ),
        custom("Pages", 4, CustomPropertyValue::I4(-42)),
        custom("Ratio", 5, CustomPropertyValue::R8(0.125)),
        custom("Approved", 6, CustomPropertyValue::Bool(true)),
        custom(
            "Due",
            7,
            CustomPropertyValue::FileTime("2026-12-01T09:30:00Z".to_owned()),
        ),
        custom("Placeholder", 8, CustomPropertyValue::Empty),
        // A value type the model does not know is kept as its XML.
        custom(
            "Large",
            9,
            CustomPropertyValue::Raw(b"<vt:i8>9007199254740993</vt:i8>".to_vec()),
        ),
        custom(
            "Référence client",
            10,
            CustomPropertyValue::Lpwstr(String::new()),
        ),
    ];
    let mut doc = doc_with(&["Body"]);
    assert!(doc.custom_properties().is_empty());
    for property in &properties {
        doc.set_custom_property(property.clone()).unwrap();
    }
    let doc = reopen(&mut doc);
    assert_eq!(doc.custom_properties(), properties);
    assert_eq!(
        doc.custom_property("Due").map(|p| &p.value),
        Some(&CustomPropertyValue::FileTime(
            "2026-12-01T09:30:00Z".to_owned()
        ))
    );
    assert!(
        doc.part_xml("_rels/.rels")
            .unwrap()
            .contains(CUSTOM_RELATIONSHIP)
    );
    assert_eq!(texts(&doc), ["Body"]);
}

#[test]
fn setting_a_custom_property_again_replaces_it_where_it_stands() {
    let mut doc = Document::new();
    for (name, pid) in [("Alpha", 2), ("Beta", 3), ("Gamma", 4)] {
        doc.set_custom_property(custom(name, pid, CustomPropertyValue::I4(pid)))
            .unwrap();
    }
    doc.set_custom_property(custom(
        "Beta",
        3,
        CustomPropertyValue::Lpwstr("two".to_owned()),
    ))
    .unwrap();
    // A property set again may take another free id.
    doc.set_custom_property(custom("Alpha", 9, CustomPropertyValue::Bool(false)))
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(custom_names(&doc), ["Alpha", "Beta", "Gamma"]);
    let beta = doc.custom_property("Beta").expect("Beta");
    assert_eq!(beta.value, CustomPropertyValue::Lpwstr("two".to_owned()));
    assert_eq!(beta.pid, 3);
    assert_eq!(doc.custom_property("Alpha").map(|p| p.pid), Some(9));
    // Names are case-sensitive.
    assert_eq!(doc.custom_property("beta"), None);
}

#[test]
fn custom_properties_are_removed_one_by_one_or_all_at_once() {
    let mut doc = doc_with(&["Body"]);
    assert_eq!(doc.remove_custom_properties().unwrap(), None);
    for (name, pid) in [("Alpha", 2), ("Beta", 3), ("Gamma", 4)] {
        doc.set_custom_property(custom(name, pid, CustomPropertyValue::I4(pid)))
            .unwrap();
    }
    let mut doc = reopen(&mut doc);
    let removed = doc.remove_custom_property("Beta").unwrap();
    assert_eq!(removed, Some(custom("Beta", 3, CustomPropertyValue::I4(3))));
    assert_eq!(doc.remove_custom_property("Beta").unwrap(), None);
    let mut doc = reopen(&mut doc);
    assert_eq!(custom_names(&doc), ["Alpha", "Gamma"]);

    let all = doc.remove_custom_properties().unwrap().expect("two left");
    assert_eq!(
        all.iter().map(|p| p.name.as_deref()).collect::<Vec<_>>(),
        [Some("Alpha"), Some("Gamma")]
    );
    let doc = reopen(&mut doc);
    assert!(doc.custom_properties().is_empty());
    assert!(!has_part(&doc, "docProps/custom.xml"));
    assert!(
        !doc.part_xml("_rels/.rels")
            .unwrap()
            .contains(CUSTOM_RELATIONSHIP)
    );
}

#[test]
fn a_custom_property_without_a_name_or_with_a_taken_id_is_refused() {
    let mut doc = doc_with(&["Body"]);
    doc.set_custom_property(custom("Approved", 2, CustomPropertyValue::Bool(true)))
        .unwrap();
    let before = doc.to_bytes().unwrap();
    let nameless = CustomProperty {
        name: None,
        ..custom("x", 5, CustomPropertyValue::Bool(true))
    };
    for (case, property, message) in [
        ("no name", nameless, "non-empty name"),
        (
            "empty name",
            custom("", 5, CustomPropertyValue::Bool(true)),
            "non-empty name",
        ),
        (
            "id 1",
            custom("Other", 1, CustomPropertyValue::Bool(true)),
            "at least 2",
        ),
        (
            "id 0",
            custom("Other", 0, CustomPropertyValue::Bool(true)),
            "at least 2",
        ),
        (
            "taken id",
            custom("Other", 2, CustomPropertyValue::Bool(true)),
            "already in use",
        ),
    ] {
        let error = doc
            .set_custom_property(property)
            .expect_err(case)
            .to_string();
        assert!(error.contains(message), "{case}: {error}");
    }
    assert_eq!(doc.to_bytes().unwrap(), before);
    assert_eq!(custom_names(&doc), ["Approved"]);
}

#[test]
fn a_libreoffice_file_keeps_its_own_custom_properties_part() {
    // LibreOffice writes an empty custom properties part.
    let mut doc = corpus_doc("libreoffice/report.docx");
    assert!(doc.custom_properties().is_empty());
    doc.set_custom_property(custom("Reviewed", 2, CustomPropertyValue::Bool(true)))
        .unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(custom_names(&doc), ["Reviewed"]);
    assert_eq!(
        doc.remove_custom_property("Reviewed")
            .unwrap()
            .map(|p| p.value),
        Some(CustomPropertyValue::Bool(true))
    );
    let doc = reopen(&mut doc);
    assert!(doc.custom_properties().is_empty());
    assert!(
        has_part(&doc, "docProps/custom.xml"),
        "the part the file came with stays"
    );
}

// Fails today: a debug build panics, its edit check compares the properties
// with == and NaN is not equal to itself. Issue: #179
#[test]
#[cfg_attr(
    debug_assertions,
    should_panic(expected = "setting a NaN number property panicked")
)]
fn a_custom_number_property_may_be_nan() {
    let mut doc = doc_with(&["Body"]);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        doc.set_custom_property(custom("Ratio", 2, CustomPropertyValue::R8(f64::NAN)))
    }));
    assert!(outcome.is_ok(), "setting a NaN number property panicked");
    outcome.unwrap().unwrap();
    let doc = reopen(&mut doc);
    match doc.custom_property("Ratio").map(|p| &p.value) {
        Some(CustomPropertyValue::R8(value)) => assert!(value.is_nan(), "{value}"),
        other => panic!("Ratio reads {other:?}"),
    }
}

// ---- Settings: on-off settings ----

/// An on-off setting: its element in the settings or web settings part and
/// its three calls.
struct Toggle {
    element: &'static str,
    get: fn(&Document) -> Option<bool>,
    set: fn(&mut Document, bool) -> Result<()>,
    remove: fn(&mut Document) -> Result<Option<bool>>,
}

const TOGGLES: [Toggle; 8] = [
    Toggle {
        element: "w:removePersonalInformation",
        get: Document::remove_personal_information,
        set: Document::set_remove_personal_information,
        remove: Document::remove_remove_personal_information,
    },
    Toggle {
        element: "w:removeDateAndTime",
        get: Document::remove_date_and_time,
        set: Document::set_remove_date_and_time,
        remove: Document::remove_remove_date_and_time,
    },
    Toggle {
        element: "w:mirrorMargins",
        get: Document::mirror_margins,
        set: Document::set_mirror_margins,
        remove: Document::remove_mirror_margins,
    },
    Toggle {
        element: "w:gutterAtTop",
        get: Document::gutter_at_top,
        set: Document::set_gutter_at_top,
        remove: Document::remove_gutter_at_top,
    },
    Toggle {
        element: "w:linkStyles",
        get: Document::link_styles,
        set: Document::set_link_styles,
        remove: Document::remove_link_styles,
    },
    Toggle {
        element: "w:doNotHyphenateCaps",
        get: Document::do_not_hyphenate_caps,
        set: Document::set_do_not_hyphenate_caps,
        remove: Document::remove_do_not_hyphenate_caps,
    },
    Toggle {
        element: "w:bookFoldPrinting",
        get: Document::book_fold_printing,
        set: Document::set_book_fold_printing,
        remove: Document::remove_book_fold_printing,
    },
    Toggle {
        element: "w:bookFoldRevPrinting",
        get: Document::book_fold_rev_printing,
        set: Document::set_book_fold_rev_printing,
        remove: Document::remove_book_fold_rev_printing,
    },
];

#[test]
fn on_off_settings_are_turned_on_turned_off_and_removed() {
    for toggle in &TOGGLES {
        let name = toggle.element;
        let mut doc = doc_with(&["Body"]);
        assert_eq!((toggle.get)(&doc), None, "{name}");

        (toggle.set)(&mut doc, true).unwrap();
        let mut doc = reopen(&mut doc);
        assert_eq!((toggle.get)(&doc), Some(true), "{name}");
        assert!(settings(&doc).contains(&format!("<{name}/>")), "{name}");

        (toggle.set)(&mut doc, false).unwrap();
        let mut doc = reopen(&mut doc);
        assert_eq!((toggle.get)(&doc), Some(false), "{name}");
        assert!(
            settings(&doc).contains(&format!(r#"<{name} w:val="false"/>"#)),
            "{name}"
        );

        assert_eq!((toggle.remove)(&mut doc).unwrap(), Some(false), "{name}");
        assert_eq!((toggle.remove)(&mut doc).unwrap(), None, "{name}");
        let doc = reopen(&mut doc);
        assert_eq!((toggle.get)(&doc), None, "{name}");
        assert_eq!(occurrences(&settings(&doc), name), 0, "{name}");
        // Each toggle writes its own element only.
        for other in TOGGLES.iter().filter(|other| other.element != name) {
            assert_eq!((other.get)(&doc), None, "{name} touched {}", other.element);
        }
    }
}

#[test]
fn automatic_hyphenation_and_even_and_odd_headers_are_removed_with_their_value() {
    let mut doc = doc_with(&["Body"]);
    doc.set_auto_hyphenation(true).unwrap();
    doc.set_even_and_odd_headers(true).unwrap();
    let mut doc = reopen(&mut doc);
    // Automatic hyphenation has no reader: the part and the remover say it.
    assert!(settings(&doc).contains("<w:autoHyphenation/>"));
    assert!(doc.even_and_odd_headers());

    assert_eq!(doc.remove_auto_hyphenation().unwrap(), Some(true));
    assert_eq!(doc.remove_auto_hyphenation().unwrap(), None);
    assert_eq!(doc.remove_even_and_odd_headers().unwrap(), Some(true));
    let mut doc = reopen(&mut doc);
    assert_eq!(occurrences(&settings(&doc), "w:autoHyphenation"), 0);
    assert_eq!(occurrences(&settings(&doc), "w:evenAndOddHeaders"), 0);
    assert!(!doc.even_and_odd_headers());

    doc.set_auto_hyphenation(false).unwrap();
    let mut doc = reopen(&mut doc);
    assert!(settings(&doc).contains(r#"<w:autoHyphenation w:val="false"/>"#));
    assert_eq!(doc.remove_auto_hyphenation().unwrap(), Some(false));

    // LibreOffice spells the value out.
    let mut libreoffice = corpus_doc("libreoffice/report.docx");
    assert_eq!(libreoffice.remove_auto_hyphenation().unwrap(), Some(true));
    let libreoffice = reopen(&mut libreoffice);
    assert_eq!(occurrences(&settings(&libreoffice), "w:autoHyphenation"), 0);
}

// ---- Settings: values ----

#[test]
fn valued_settings_round_trip_and_are_removed_with_their_value() {
    let mut doc = doc_with(&["Body"]);
    doc.set_default_tab_stop(Twips(708)).unwrap();
    doc.set_hyphenation_zone(Twips(357)).unwrap();
    doc.set_consecutive_hyphen_limit(3).unwrap();
    doc.set_book_fold_printing_sheets(4).unwrap();
    doc.set_default_table_style("TableGrid").unwrap();
    doc.set_decimal_symbol(",").unwrap();
    doc.set_list_separator(";").unwrap();
    doc.set_character_spacing_control(CharacterSpacingControl::CompressPunctuation)
        .unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(doc.default_tab_stop(), Some(Twips(708)));
    assert_eq!(doc.hyphenation_zone(), Some(Twips(357)));
    assert_eq!(doc.consecutive_hyphen_limit(), Some(3));
    assert_eq!(doc.book_fold_printing_sheets(), Some(4));
    assert_eq!(doc.default_table_style(), Some("TableGrid"));
    assert_eq!(doc.decimal_symbol(), Some(","));
    assert_eq!(doc.list_separator(), Some(";"));
    assert_eq!(
        doc.character_spacing_control(),
        Some(CharacterSpacingControl::CompressPunctuation)
    );
    let xml = settings(&doc);
    for written in [
        r#"<w:defaultTabStop w:val="708"/>"#,
        r#"<w:hyphenationZone w:val="357"/>"#,
        r#"<w:consecutiveHyphenLimit w:val="3"/>"#,
        r#"<w:bookFoldPrintingSheets w:val="4"/>"#,
        r#"<w:defaultTableStyle w:val="TableGrid"/>"#,
        r#"<w:characterSpacingControl w:val="compressPunctuation"/>"#,
        r#"<w:decimalSymbol w:val=","/>"#,
        r#"<w:listSeparator w:val=";"/>"#,
    ] {
        assert!(xml.contains(written), "{written} in {xml}");
    }

    assert_eq!(doc.remove_default_tab_stop().unwrap(), Some(Twips(708)));
    assert_eq!(doc.remove_hyphenation_zone().unwrap(), Some(Twips(357)));
    assert_eq!(doc.remove_consecutive_hyphen_limit().unwrap(), Some(3));
    assert_eq!(doc.remove_book_fold_printing_sheets().unwrap(), Some(4));
    assert_eq!(
        doc.remove_default_table_style().unwrap().as_deref(),
        Some("TableGrid")
    );
    assert_eq!(doc.remove_decimal_symbol().unwrap().as_deref(), Some(","));
    assert_eq!(doc.remove_list_separator().unwrap().as_deref(), Some(";"));
    assert_eq!(
        doc.remove_character_spacing_control().unwrap(),
        Some(CharacterSpacingControl::CompressPunctuation)
    );
    let doc = reopen(&mut doc);
    assert_eq!(
        (
            doc.default_tab_stop(),
            doc.hyphenation_zone(),
            doc.consecutive_hyphen_limit(),
            doc.book_fold_printing_sheets(),
        ),
        (None, None, None, None)
    );
    assert_eq!(
        (
            doc.default_table_style(),
            doc.decimal_symbol(),
            doc.list_separator(),
            doc.character_spacing_control(),
        ),
        (None, None, None, None)
    );
}

#[test]
fn a_negative_tab_stop_or_hyphenation_zone_is_refused_and_changes_nothing() {
    let mut doc = doc_with(&["Body"]);
    doc.set_default_tab_stop(Twips(720)).unwrap();
    doc.set_hyphenation_zone(Twips(360)).unwrap();
    let before = doc.to_bytes().unwrap();
    let error = doc.set_default_tab_stop(Twips(-1)).unwrap_err().to_string();
    assert!(error.contains("non-negative"), "{error}");
    let error = doc
        .set_hyphenation_zone(Twips(-360))
        .unwrap_err()
        .to_string();
    assert!(error.contains("non-negative"), "{error}");
    assert_eq!(doc.to_bytes().unwrap(), before);
    assert_eq!(doc.default_tab_stop(), Some(Twips(720)));
    assert_eq!(doc.hyphenation_zone(), Some(Twips(360)));
}

#[test]
fn view_zoom_and_proof_state_round_trip_and_are_removed() {
    let mut doc = doc_with(&["Body"]);
    for view in [
        DocumentView::None,
        DocumentView::Print,
        DocumentView::Outline,
        DocumentView::MasterPages,
        DocumentView::Normal,
        DocumentView::Web,
    ] {
        doc.set_view(view).unwrap();
        doc = reopen(&mut doc);
        assert_eq!(doc.view(), Some(view));
    }
    for zoom in [
        DocumentZoom {
            kind: None,
            percent: Some(146),
        },
        DocumentZoom {
            kind: Some(ZoomKind::None),
            percent: Some(100),
        },
        DocumentZoom {
            kind: Some(ZoomKind::FullPage),
            percent: Some(55),
        },
        DocumentZoom {
            kind: Some(ZoomKind::BestFit),
            percent: None,
        },
        DocumentZoom {
            kind: Some(ZoomKind::TextFit),
            percent: Some(120),
        },
    ] {
        doc.set_zoom(zoom).unwrap();
        doc = reopen(&mut doc);
        assert_eq!(doc.zoom(), Some(zoom));
        assert_eq!(occurrences(&settings(&doc), "w:zoom"), 1);
    }
    let proof = DocumentProofState {
        spelling: Some(ProofState::Clean),
        grammar: Some(ProofState::Dirty),
    };
    doc.set_proof_state(proof).unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(doc.proof_state(), Some(proof));
    assert!(settings(&doc).contains(r#"<w:proofState w:spelling="clean" w:grammar="dirty"/>"#));

    assert_eq!(doc.remove_view().unwrap(), Some(DocumentView::Web));
    assert_eq!(
        doc.remove_zoom().unwrap(),
        Some(DocumentZoom {
            kind: Some(ZoomKind::TextFit),
            percent: Some(120)
        })
    );
    assert_eq!(doc.remove_proof_state().unwrap(), Some(proof));
    let doc = reopen(&mut doc);
    assert_eq!(
        (doc.view(), doc.zoom(), doc.proof_state()),
        (None, None, None)
    );
    let xml = settings(&doc);
    for element in ["w:view", "w:zoom", "w:proofState"] {
        assert_eq!(occurrences(&xml, element), 0, "{element} in {xml}");
    }
}

// ---- Settings: protection and mail merge ----

#[test]
fn document_protection_is_recorded_as_given_for_every_mode() {
    for (mode, token) in [
        (ProtectionMode::ReadOnly, "readOnly"),
        (ProtectionMode::Comments, "comments"),
        (ProtectionMode::TrackedChanges, "trackedChanges"),
        (ProtectionMode::Forms, "forms"),
    ] {
        let mut doc = doc_with(&["Body"]);
        let protection = DocumentProtection {
            mode,
            enforcement: Some(true),
            formatting: None,
            provider_type: None,
            algorithm_class: None,
            algorithm_type: None,
            algorithm_sid: None,
            spin_count: None,
            hash: None,
            salt: None,
        };
        doc.set_document_protection(protection.clone()).unwrap();
        let mut doc = reopen(&mut doc);
        assert_eq!(doc.document_protection(), Some(&protection), "{mode:?}");
        // No password is invented.
        let xml = settings(&doc);
        assert!(xml.contains(&format!(r#"w:edit="{token}""#)), "{xml}");
        assert!(!xml.contains("w:hash="), "{xml}");
        assert!(!xml.contains("w:salt="), "{xml}");

        assert_eq!(doc.remove_document_protection().unwrap(), Some(protection));
        let doc = reopen(&mut doc);
        assert_eq!(doc.document_protection(), None, "{mode:?}");
        assert_eq!(occurrences(&settings(&doc), "w:documentProtection"), 0);
    }

    // A password hash computed elsewhere is written verbatim.
    let mut doc = doc_with(&["Body"]);
    let hashed = DocumentProtection {
        mode: ProtectionMode::ReadOnly,
        enforcement: Some(true),
        formatting: Some(false),
        provider_type: Some(CryptProviderType::RsaAes),
        algorithm_class: Some(CryptAlgorithmClass::Hash),
        algorithm_type: Some(CryptAlgorithmType::Any),
        algorithm_sid: Some(14),
        spin_count: Some(100_000),
        hash: Some("aGFzaA==".to_owned()),
        salt: Some("c2FsdA==".to_owned()),
    };
    doc.set_document_protection(hashed.clone()).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.document_protection(), Some(&hashed));
    let xml = settings(&doc);
    assert!(xml.contains(r#"w:hash="aGFzaA==""#), "{xml}");
    assert!(xml.contains(r#"w:cryptSpinCount="100000""#), "{xml}");
}

#[test]
fn the_form_protection_of_corpus_forms_is_read_and_can_be_lifted() {
    for name in ["authored/form.docx", "libreoffice/form.docx"] {
        let mut doc = corpus_doc(name);
        let protection = doc
            .document_protection()
            .expect("the form is protected")
            .clone();
        assert_eq!(protection.mode, ProtectionMode::Forms, "{name}");
        assert_eq!(protection.enforcement, Some(true), "{name}");
        let controls = doc.content_controls().len();

        assert_eq!(
            doc.remove_document_protection().unwrap().as_ref(),
            Some(&protection),
            "{name}"
        );
        let mut doc = reopen(&mut doc);
        assert_eq!(doc.document_protection(), None, "{name}");
        assert_eq!(doc.content_controls().len(), controls, "{name}");

        let read_only = DocumentProtection {
            mode: ProtectionMode::ReadOnly,
            ..protection
        };
        doc.set_document_protection(read_only.clone()).unwrap();
        let doc = reopen(&mut doc);
        assert_eq!(doc.document_protection(), Some(&read_only), "{name}");
    }
    // LibreOffice writes the hash attributes empty.
    let libreoffice = corpus_doc("libreoffice/form.docx");
    let protection = libreoffice.document_protection().unwrap();
    assert_eq!(protection.provider_type, Some(CryptProviderType::RsaAes));
    assert_eq!(protection.spin_count, Some(0));
    assert_eq!(protection.hash.as_deref(), Some(""));
}

#[test]
fn mail_merge_settings_round_trip_and_are_removed() {
    let mut doc = doc_with(&["Dear «Name»"]);
    let merge = MailMerge {
        main_document_type: Some(MailMergeDocumentType::FormLetters),
        link_to_query: Some(true),
        data_type: Some("textFile".to_owned()),
        connect_string: Some(r"C:\data\clients & partners.csv".to_owned()),
        query: Some("SELECT * FROM clients.csv WHERE city = 'Zürich'".to_owned()),
        do_not_suppress_blank_lines: Some(true),
        destination: Some(MailMergeDestination::Email),
        address_field_name: Some("Email".to_owned()),
        mail_subject: Some("Votre facture".to_owned()),
        mail_as_attachment: Some(false),
        view_merged_data: Some(true),
        active_record: Some(3),
        check_errors: Some(2),
    };
    doc.set_mail_merge_settings(merge.clone()).unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(doc.mail_merge_settings(), Some(&merge));
    assert_eq!(occurrences(&settings(&doc), "w:mailMerge"), 1);
    // Word reads the members in schema order, with its own tokens.
    assert_in_order(
        &settings(&doc),
        &[
            r#"<w:mainDocumentType w:val="formLetters"/>"#,
            "<w:linkToQuery",
            r#"<w:dataType w:val="textFile"/>"#,
            "<w:connectString",
            "<w:query",
            "<w:doNotSuppressBlankLines",
            r#"<w:destination w:val="email"/>"#,
            "<w:addressFieldName",
            "<w:mailSubject",
            "<w:mailAsAttachment",
            "<w:viewMergedData",
            r#"<w:activeRecord w:val="3"/>"#,
            r#"<w:checkErrors w:val="2"/>"#,
        ],
    );

    let shorter = MailMerge {
        main_document_type: Some(MailMergeDocumentType::MailingLabels),
        destination: Some(MailMergeDestination::Printer),
        ..MailMerge::default()
    };
    doc.set_mail_merge_settings(shorter.clone()).unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(doc.mail_merge_settings(), Some(&shorter));
    assert!(!settings(&doc).contains("w:connectString"));

    assert_eq!(doc.remove_mail_merge_settings().unwrap(), Some(shorter));
    let doc = reopen(&mut doc);
    assert_eq!(doc.mail_merge_settings(), None);
    assert_eq!(occurrences(&settings(&doc), "w:mailMerge"), 0);
    assert_eq!(texts(&doc), ["Dear «Name»"]);
}

// ---- Settings: compatibility, variables, languages, math ----

#[test]
fn compatibility_options_and_settings_are_set_replaced_and_removed() {
    let mut doc = doc_with(&["Body"]);
    // Word writes compatibility mode 15 into every new document.
    assert_eq!(
        doc.compatibility_settings()
            .iter()
            .map(|s| (s.name.as_str(), s.uri.as_str(), s.value.as_str()))
            .collect::<Vec<_>>(),
        [("compatibilityMode", WORD_URI, "15")]
    );
    assert!(doc.compatibility_options().is_empty());

    // Set out of order, kept in the order of the schema.
    doc.set_compatibility_option(CompatibilityOption::CachedColBalance, true)
        .unwrap();
    doc.set_compatibility_option(
        CompatibilityOption::UseSingleBorderforContiguousCells,
        false,
    )
    .unwrap();
    doc.set_compatibility_option(CompatibilityOption::DoNotExpandShiftReturn, true)
        .unwrap();
    doc.set_compatibility_setting("compatibilityMode", WORD_URI, "14")
        .unwrap();
    doc.set_compatibility_setting("enableOpenTypeFeatures", WORD_URI, "1")
        .unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(
        doc.compatibility_options(),
        [
            (
                CompatibilityOption::UseSingleBorderforContiguousCells,
                false
            ),
            (CompatibilityOption::DoNotExpandShiftReturn, true),
            (CompatibilityOption::CachedColBalance, true),
        ]
    );
    assert_eq!(
        doc.compatibility_option(CompatibilityOption::DoNotExpandShiftReturn),
        Some(true)
    );
    assert_eq!(
        doc.compatibility_option(CompatibilityOption::NoLeading),
        None
    );
    assert_eq!(
        doc.compatibility_settings()
            .iter()
            .map(|s| (s.name.as_str(), s.value.as_str()))
            .collect::<Vec<_>>(),
        [("compatibilityMode", "14"), ("enableOpenTypeFeatures", "1")]
    );
    // The options come before the named settings, as Word requires.
    assert_in_order(
        &settings(&doc),
        &[
            r#"<w:useSingleBorderforContiguousCells w:val="false"/>"#,
            "<w:doNotExpandShiftReturn/>",
            "<w:cachedColBalance/>",
            r#"<w:compatSetting w:name="compatibilityMode""#,
        ],
    );

    assert_eq!(
        doc.remove_compatibility_option(CompatibilityOption::DoNotExpandShiftReturn)
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        doc.remove_compatibility_option(CompatibilityOption::NoLeading)
            .unwrap(),
        None
    );
    // A setting is named by its name and its URI together.
    assert_eq!(
        doc.remove_compatibility_setting("enableOpenTypeFeatures", "urn:other")
            .unwrap(),
        None
    );
    let removed = doc
        .remove_compatibility_setting("enableOpenTypeFeatures", WORD_URI)
        .unwrap()
        .expect("removed");
    assert_eq!(removed.value, "1");
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.compatibility_option(CompatibilityOption::DoNotExpandShiftReturn),
        None
    );
    assert_eq!(doc.compatibility_options().len(), 2);
    assert_eq!(
        doc.compatibility_settings()
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["compatibilityMode"]
    );
}

#[test]
fn document_variables_keep_unicode_and_markup_characters() {
    let mut doc = doc_with(&["Body"]);
    assert_eq!(doc.document_variable("Client"), None);
    let variables = [
        ("Client", "Société Générale & Fils <\"SA\">"),
        ("客户", "東京 — مرحبا"),
        ("Empty", ""),
        ("Padded", "  spaced  "),
    ];
    for (name, value) in variables {
        doc.set_document_variable(name, value).unwrap();
    }
    let mut doc = reopen(&mut doc);
    for (name, value) in variables {
        assert_eq!(doc.document_variable(name), Some(value), "{name}");
    }
    // Names are case-sensitive.
    assert_eq!(doc.document_variable("client"), None);

    doc.set_document_variable("Client", "Replaced").unwrap();
    assert_eq!(
        doc.remove_document_variable("客户").unwrap().as_deref(),
        Some("東京 — مرحبا")
    );
    assert_eq!(doc.remove_document_variable("Missing").unwrap(), None);
    let doc = reopen(&mut doc);
    assert_eq!(doc.document_variable("Client"), Some("Replaced"));
    assert_eq!(doc.document_variable("客户"), None);
    assert_eq!(doc.document_variable("Empty"), Some(""));
    assert_eq!(occurrences(&settings(&doc), "w:docVar"), 3);
}

// Fails today: settings values are written with a raw line feed or tab, which
// every XML reader turns into a space (same class as #100). Issue: #180
#[test]
#[should_panic(expected = "came back changed after a save")]
fn a_line_feed_or_a_tab_in_a_document_variable_or_a_merge_query_survives_a_save() {
    let mut doc = doc_with(&["Body"]);
    doc.set_document_variable("Address", "1 Main Street\nSpringfield\tUSA")
        .unwrap();
    doc.set_mail_merge_settings(MailMerge {
        query: Some("SELECT *\nFROM clients".to_owned()),
        ..MailMerge::default()
    })
    .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.document_variable("Address"),
        Some("1 Main Street\nSpringfield\tUSA"),
        "the document variable came back changed after a save"
    );
    assert_eq!(
        doc.mail_merge_settings().and_then(|m| m.query.as_deref()),
        Some("SELECT *\nFROM clients"),
        "the merge query came back changed after a save"
    );
}

#[test]
fn theme_languages_and_language_defaults_round_trip_and_are_removed() {
    let mut doc = doc_with(&["Body"]);
    let languages = ThemeFontLanguage {
        latin: Some("fr-FR".to_owned()),
        east_asia: Some("zh-CN".to_owned()),
        bidi: Some("ar-SA".to_owned()),
    };
    doc.set_theme_font_language(languages.clone()).unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(doc.theme_font_language(), Some(&languages));
    assert!(
        settings(&doc)
            .contains(r#"<w:themeFontLang w:val="fr-FR" w:eastAsia="zh-CN" w:bidi="ar-SA"/>"#)
    );
    assert_eq!(doc.remove_theme_font_language().unwrap(), Some(languages));
    assert_eq!(doc.remove_theme_font_language().unwrap(), None);
    let mut doc = reopen(&mut doc);
    assert_eq!(doc.theme_font_language(), None);
    assert_eq!(occurrences(&settings(&doc), "w:themeFontLang"), 0);

    let german = ThemeFontLanguage {
        latin: Some("de-DE".to_owned()),
        ..ThemeFontLanguage::default()
    };
    doc.set_language_defaults(german.clone()).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.theme_font_language(), Some(&german));

    // LibreOffice writes an empty right-to-left language.
    let libreoffice = corpus_doc("libreoffice/report.docx");
    assert_eq!(
        libreoffice.theme_font_language(),
        Some(&ThemeFontLanguage {
            latin: Some("en-US".to_owned()),
            east_asia: Some("ja-JP".to_owned()),
            bidi: Some(String::new()),
        })
    );
}

#[test]
fn math_defaults_round_trip_and_are_removed() {
    let mut doc = doc_with(&["Body"]);
    assert!(doc.math_properties().is_none());
    let mut math = MathProperties::new();
    math.math_font = Some("Cambria Math".to_owned());
    math.justification = Some(MathJustification::Left);
    math.left_margin = Some(10);
    math.right_margin = Some(20);
    math.pre_spacing = Some(1);
    math.post_spacing = Some(2);
    math.inter_spacing = Some(3);
    math.intra_spacing = Some(4);
    math.wrap_indent = Some(1440);
    math.small_fraction = Some(true);
    math.display_defaults = Some(false);
    math.integral_limit_location = Some(LimitLocation::UnderOver);
    math.nary_limit_location = Some(LimitLocation::SubSuperscript);
    doc.set_math_properties(math.clone()).unwrap();
    let mut doc = reopen(&mut doc);
    let read = doc.math_properties().expect("math defaults");
    assert_eq!(MathDefaults::from(read), MathDefaults::from(&math));
    assert!(!read.has_unsupported_content());
    assert!(settings(&doc).contains(r#"<m:defJc m:val="left"/>"#));

    let removed = doc.remove_math_properties().unwrap().expect("removed");
    assert_eq!(MathDefaults::from(&removed), MathDefaults::from(&math));
    let doc = reopen(&mut doc);
    assert!(doc.math_properties().is_none());
    assert_eq!(occurrences(&settings(&doc), "m:mathPr"), 0);
}

// ---- Web settings ----

const WEB_TOGGLES: [Toggle; 8] = [
    Toggle {
        element: "w:optimizeForBrowser",
        get: Document::web_optimize_for_browser,
        set: Document::set_web_optimize_for_browser,
        remove: Document::remove_web_optimize_for_browser,
    },
    Toggle {
        element: "w:relyOnVML",
        get: Document::web_rely_on_vml,
        set: Document::set_web_rely_on_vml,
        remove: Document::remove_web_rely_on_vml,
    },
    Toggle {
        element: "w:allowPNG",
        get: Document::web_allow_png,
        set: Document::set_web_allow_png,
        remove: Document::remove_web_allow_png,
    },
    Toggle {
        element: "w:doNotRelyOnCSS",
        get: Document::web_do_not_rely_on_css,
        set: Document::set_web_do_not_rely_on_css,
        remove: Document::remove_web_do_not_rely_on_css,
    },
    Toggle {
        element: "w:doNotSaveAsSingleFile",
        get: Document::web_do_not_save_as_single_file,
        set: Document::set_web_do_not_save_as_single_file,
        remove: Document::remove_web_do_not_save_as_single_file,
    },
    Toggle {
        element: "w:doNotOrganizeInFolder",
        get: Document::web_do_not_organize_in_folder,
        set: Document::set_web_do_not_organize_in_folder,
        remove: Document::remove_web_do_not_organize_in_folder,
    },
    Toggle {
        element: "w:doNotUseLongFileNames",
        get: Document::web_do_not_use_long_file_names,
        set: Document::set_web_do_not_use_long_file_names,
        remove: Document::remove_web_do_not_use_long_file_names,
    },
    Toggle {
        element: "w:saveSmartTagsAsXml",
        get: Document::web_save_smart_tags_as_xml,
        set: Document::set_web_save_smart_tags_as_xml,
        remove: Document::remove_web_save_smart_tags_as_xml,
    },
];

#[test]
fn web_settings_round_trip_in_a_part_made_for_them_and_dropped_with_them() {
    let mut doc = doc_with(&["Body"]);
    assert!(!has_part(&doc, "word/webSettings.xml"));
    for (index, toggle) in WEB_TOGGLES.iter().enumerate() {
        (toggle.set)(&mut doc, index % 2 == 0).unwrap();
    }
    doc.set_web_encoding("utf-8").unwrap();
    doc.set_web_pixels_per_inch(120).unwrap();
    doc.set_web_target_screen_size("1024x768").unwrap();
    let read = reopen(&mut doc);
    let xml = web_settings(&read);
    for (index, toggle) in WEB_TOGGLES.iter().enumerate() {
        let on = index % 2 == 0;
        assert_eq!((toggle.get)(&read), Some(on), "{}", toggle.element);
        let written = if on {
            format!("<{}/>", toggle.element)
        } else {
            format!(r#"<{} w:val="false"/>"#, toggle.element)
        };
        assert!(xml.contains(&written), "{written} in {xml}");
    }
    assert_eq!(read.web_encoding(), Some("utf-8"));
    assert_eq!(read.web_pixels_per_inch(), Some(120));
    assert_eq!(read.web_target_screen_size(), Some("1024x768"));
    assert!(read.web_settings_diagnostics().is_empty());

    // Removed in the session that made the part: the part goes with the
    // last one.
    for (index, toggle) in WEB_TOGGLES.iter().enumerate() {
        let on = index % 2 == 0;
        assert_eq!(
            (toggle.remove)(&mut doc).unwrap(),
            Some(on),
            "{}",
            toggle.element
        );
    }
    assert_eq!(doc.remove_web_encoding().unwrap().as_deref(), Some("utf-8"));
    assert_eq!(doc.remove_web_pixels_per_inch().unwrap(), Some(120));
    assert_eq!(
        doc.remove_web_target_screen_size().unwrap().as_deref(),
        Some("1024x768")
    );
    let doc = reopen(&mut doc);
    assert!(!has_part(&doc, "word/webSettings.xml"));
    assert_eq!(doc.web_allow_png(), None);
    assert_eq!(doc.web_encoding(), None);
}

#[test]
fn a_word_file_web_settings_are_read_and_its_divisions_kept_through_an_edit() {
    let doc = corpus_doc("word/report.docx");
    assert_eq!(doc.web_allow_png(), Some(true));
    assert!(doc.web_division_ids().is_empty());

    // Word records the HTML divisions of a pasted web page.
    let divisions = concat!(
        r#"<w:divs><w:div w:id="1585722434"><w:bodyDiv w:val="1"/><w:marLeft w:val="0"/>"#,
        r#"<w:marRight w:val="0"/><w:marTop w:val="0"/><w:marBottom w:val="0"/>"#,
        r#"<w:divBdr><w:top w:val="none" w:sz="0" w:space="0" w:color="auto"/></w:divBdr>"#,
        r#"<w:divsChild><w:div w:id="402335124"><w:marLeft w:val="0"/><w:marRight w:val="0"/>"#,
        r#"<w:marTop w:val="0"/><w:marBottom w:val="0"/></w:div></w:divsChild></w:div></w:divs>"#,
    );
    let original = corpus_part("word/report.docx", "word/webSettings.xml");
    let planted = original.replace(
        "<w:allowPNG/>",
        &format!("{divisions}<w:optimizeForBrowser/><w:allowPNG/>"),
    );
    let mut doc = with_part(doc, "/word/webSettings.xml", &planted);
    assert_eq!(doc.web_division_ids(), [1_585_722_434, 402_335_124]);

    doc.set_web_pixels_per_inch(96).unwrap();
    assert_eq!(doc.remove_web_optimize_for_browser().unwrap(), Some(true));
    let doc = reopen(&mut doc);
    assert_eq!(doc.web_division_ids(), [1_585_722_434, 402_335_124]);
    assert_eq!(doc.web_pixels_per_inch(), Some(96));
    assert_eq!(doc.web_optimize_for_browser(), None);
    assert_eq!(doc.web_allow_png(), Some(true));
    let xml = web_settings(&doc);
    assert_in_order(
        &xml,
        &[
            divisions,
            "<w:allowPNG/>",
            r#"<w:pixelsPerInch w:val="96"/>"#,
        ],
    );
    assert!(
        xml.contains(r#"mc:Ignorable="w14 w15 w16se w16cid w16 w16cex w16sdtdh w16sdtfl w16du""#)
    );
}

// ---- Settings parts as producers write them ----

#[test]
fn settings_oxoffice_does_not_model_survive_its_edits_in_place() {
    let producer = [
        r#"<w:writeProtection w:recommended="1"/>"#,
        "<w:doNotDisplayPageBoundaries/>",
        r#"<x:producer x:id="7"><x:inner>kept text</x:inner></x:producer>"#,
        "<!-- a producer comment -->",
        r#"<w:rsids><w:rsidRoot w:val="00A1B2C3"/><w:rsid w:val="00A1B2C3"/></w:rsids>"#,
        r#"<w:clrSchemeMapping w:bg1="light1" w:t1="dark1"/>"#,
        "<w:doNotAutoCompressPictures/>",
        r#"<w14:docId w14:val="1A2B3C4D"/>"#,
    ];
    let mut doc = doc_with_settings(&producer.concat());
    assert!(doc.settings_diagnostics().is_empty());

    doc.set_view(DocumentView::Print).unwrap();
    doc.set_zoom(DocumentZoom {
        kind: None,
        percent: Some(90),
    })
    .unwrap();
    doc.set_mirror_margins(true).unwrap();
    doc.set_link_styles(true).unwrap();
    doc.set_default_tab_stop(Twips(708)).unwrap();
    doc.set_auto_hyphenation(true).unwrap();
    doc.set_character_spacing_control(CharacterSpacingControl::DoNotCompress)
        .unwrap();
    doc.set_compatibility_option(CompatibilityOption::UseFELayout, true)
        .unwrap();
    doc.set_compatibility_setting("compatibilityMode", WORD_URI, "15")
        .unwrap();
    doc.set_document_variable("Client", "Contoso").unwrap();
    doc.set_theme_font_language(ThemeFontLanguage {
        latin: Some("en-GB".to_owned()),
        ..ThemeFontLanguage::default()
    })
    .unwrap();
    doc.set_decimal_symbol(".").unwrap();
    assert_eq!(doc.remove_link_styles().unwrap(), Some(true));
    let doc = reopen(&mut doc);

    assert_eq!(doc.view(), Some(DocumentView::Print));
    assert_eq!(doc.zoom().and_then(|zoom| zoom.percent), Some(90));
    assert_eq!(doc.mirror_margins(), Some(true));
    assert_eq!(doc.link_styles(), None);
    assert_eq!(doc.default_tab_stop(), Some(Twips(708)));
    assert_eq!(doc.document_variable("Client"), Some("Contoso"));
    assert_eq!(doc.decimal_symbol(), Some("."));
    assert!(doc.settings_diagnostics().is_empty());

    // Every producer child is kept verbatim and in its order; each typed one
    // takes its place in Word's schema order between them.
    let xml = settings(&doc);
    assert_in_order(&xml, &producer);
    assert_in_order(
        &xml,
        &[
            "<w:writeProtection",
            r#"<w:view w:val="print"/>"#,
            r#"<w:zoom w:percent="90"/>"#,
            "<w:doNotDisplayPageBoundaries/>",
            "<w:mirrorMargins/>",
            r#"<w:defaultTabStop w:val="708"/>"#,
            "<w:autoHyphenation/>",
            r#"<w:characterSpacingControl w:val="doNotCompress"/>"#,
            "<w:compat><w:useFELayout/>",
            r#"<w:docVars><w:docVar w:name="Client" w:val="Contoso"/></w:docVars>"#,
            "<w:rsids>",
            r#"<w:themeFontLang w:val="en-GB"/>"#,
            "<w:clrSchemeMapping",
            r#"<w:decimalSymbol w:val="."/>"#,
        ],
    );
}

#[test]
fn a_duplicated_or_malformed_setting_is_reported_and_a_set_replaces_it() {
    let mut doc = doc_with_settings(concat!(
        r#"<w:zoom w:percent="100"/><w:zoom w:percent="120"/>"#,
        r#"<w:defaultTabStop w:val="wide"/>"#,
        r#"<w:hyphenationZone w:val="425"/>"#,
    ));
    // No single occurrence is the value: none is read.
    assert_eq!(doc.zoom(), None);
    assert_eq!(doc.default_tab_stop(), None);
    assert_eq!(doc.hyphenation_zone(), Some(Twips(425)));
    assert_eq!(
        doc.settings_diagnostics(),
        [
            SettingsDiagnostic {
                path: vec!["zoom"],
                occurrences: 2,
                reason: SettingsDiagnosticReason::Duplicated,
            },
            SettingsDiagnostic {
                path: vec!["defaultTabStop"],
                occurrences: 1,
                reason: SettingsDiagnosticReason::Malformed,
            },
        ]
    );

    doc.set_zoom(DocumentZoom {
        kind: None,
        percent: Some(90),
    })
    .unwrap();
    doc.set_default_tab_stop(Twips(708)).unwrap();
    assert!(doc.settings_diagnostics().is_empty());
    let doc = reopen(&mut doc);
    assert_eq!(doc.zoom().and_then(|zoom| zoom.percent), Some(90));
    assert_eq!(doc.default_tab_stop(), Some(Twips(708)));
    assert!(doc.settings_diagnostics().is_empty());
    let xml = settings(&doc);
    assert_eq!(occurrences(&xml, "w:zoom"), 1, "{xml}");
    assert_eq!(occurrences(&xml, "w:defaultTabStop"), 1, "{xml}");
    assert!(!xml.contains("wide"), "{xml}");
}

#[test]
fn a_settings_part_made_for_one_setting_goes_with_it() {
    // The minimal profile writes no settings part.
    let mut doc =
        Document::new_with_profile(WordCreationProfile::Minimal(WordPackageClass::Document));
    doc.add_paragraph("Body");
    assert!(!has_part(&doc, "word/settings.xml"));
    assert!(doc.compatibility_settings().is_empty());

    doc.set_document_variable("Stage", "draft").unwrap();
    let read = reopen(&mut doc);
    assert_eq!(read.document_variable("Stage"), Some("draft"));
    assert!(
        read.part_xml("word/_rels/document.xml.rels")
            .unwrap()
            .contains(SETTINGS_RELATIONSHIP)
    );

    // Removed in the session that made the part: the part goes with it.
    assert_eq!(
        doc.remove_document_variable("Stage").unwrap().as_deref(),
        Some("draft")
    );
    let doc = reopen(&mut doc);
    assert!(!has_part(&doc, "word/settings.xml"));
    assert!(
        !doc.part_xml("word/_rels/document.xml.rels")
            .unwrap()
            .contains(SETTINGS_RELATIONSHIP)
    );
    assert_eq!(texts(&doc), ["Body"]);
}

// ---- Word- and Google-saved files ----

#[test]
fn the_word_report_settings_read_as_a_french_word_wrote_them() {
    let doc = corpus_doc("word/report.docx");
    assert_eq!(
        doc.zoom(),
        Some(DocumentZoom {
            kind: None,
            percent: Some(146)
        })
    );
    assert_eq!(doc.view(), None);
    assert_eq!(doc.default_tab_stop(), Some(Twips(720)));
    assert_eq!(doc.hyphenation_zone(), Some(Twips(425)));
    assert_eq!(
        doc.character_spacing_control(),
        Some(CharacterSpacingControl::DoNotCompress)
    );
    // A French Word: decimal comma, semicolon list separator.
    assert_eq!(doc.decimal_symbol(), Some(","));
    assert_eq!(doc.list_separator(), Some(";"));
    assert_eq!(
        doc.compatibility_options(),
        [(CompatibilityOption::UseFELayout, true)]
    );
    let compat: Vec<(&str, &str)> = doc
        .compatibility_settings()
        .iter()
        .map(|s| (s.name.as_str(), s.value.as_str()))
        .collect();
    assert_eq!(
        compat,
        [
            ("compatibilityMode", "14"),
            ("overrideTableStyleFontSizeAndJustification", "1"),
            ("enableOpenTypeFeatures", "1"),
            ("doNotFlipMirrorIndents", "1"),
            ("useWord2013TrackBottomHyphenation", "1"),
        ]
    );
    assert_eq!(
        doc.theme_font_language(),
        Some(&ThemeFontLanguage {
            latin: Some("en-US".to_owned()),
            east_asia: Some("ja-JP".to_owned()),
            bidi: None,
        })
    );
    let math = doc.math_properties().expect("Word writes math defaults");
    assert_eq!(math.math_font.as_deref(), Some("Cambria Math"));
    assert_eq!(math.justification, Some(MathJustification::CenterGroup));
    assert_eq!(math.wrap_indent, Some(1440));
    assert_eq!(
        (math.integral_limit_location, math.nary_limit_location),
        (
            Some(LimitLocation::SubSuperscript),
            Some(LimitLocation::UnderOver)
        )
    );
    assert_eq!(
        (
            doc.document_protection(),
            doc.proof_state(),
            doc.mirror_margins()
        ),
        (None, None, None)
    );
    assert!(doc.settings_diagnostics().is_empty());
}

#[test]
fn edits_to_the_word_report_settings_keep_its_rsids_and_extensions() {
    let mut doc = corpus_doc("word/report.docx");
    doc.set_view(DocumentView::Web).unwrap();
    doc.set_zoom(DocumentZoom {
        kind: None,
        percent: Some(80),
    })
    .unwrap();
    doc.set_mirror_margins(true).unwrap();
    doc.set_proof_state(DocumentProofState {
        spelling: Some(ProofState::Dirty),
        grammar: None,
    })
    .unwrap();
    doc.set_document_variable("Client", "Société").unwrap();
    doc.set_decimal_symbol(".").unwrap();
    assert_eq!(doc.remove_hyphenation_zone().unwrap(), Some(Twips(425)));
    let doc = reopen(&mut doc);
    assert_eq!(doc.view(), Some(DocumentView::Web));
    assert_eq!(doc.zoom().and_then(|zoom| zoom.percent), Some(80));
    assert_eq!(doc.mirror_margins(), Some(true));
    assert_eq!(doc.document_variable("Client"), Some("Société"));
    assert_eq!(doc.decimal_symbol(), Some("."));
    assert_eq!(doc.hyphenation_zone(), None);
    assert_eq!(doc.list_separator(), Some(";"));
    assert_eq!(doc.compatibility_settings().len(), 5);

    let xml = settings(&doc);
    assert_eq!(occurrences(&xml, "w:hyphenationZone"), 0);
    assert_in_order(
        &xml,
        &[
            r#"<w:view w:val="web"/>"#,
            r#"<w:zoom w:percent="80"/>"#,
            "<w:mirrorMargins/>",
            r#"<w:proofState w:spelling="dirty"/>"#,
            r#"<w:defaultTabStop w:val="720"/>"#,
            "</w:compat>",
            r#"<w:docVars><w:docVar w:name="Client" w:val="Société"/></w:docVars>"#,
            r#"<w:rsids><w:rsidRoot w:val="00B47730"/>"#,
            r#"<m:mathPr><m:mathFont m:val="Cambria Math"/><m:brkBin m:val="before"/>"#,
            r#"<w:themeFontLang w:val="en-US" w:eastAsia="ja-JP"/>"#,
            "<w:shapeDefaults>",
            r#"<w:decimalSymbol w:val="."/><w:listSeparator w:val=";"/>"#,
            r#"<w14:docId w14:val="49BD98B8"/>"#,
            r#"<w15:docId w15:val="{24E648D8-7188-A54E-8BBA-293CB30C2B9A}"/>"#,
        ],
    );
    assert!(
        xml.contains(r#"mc:Ignorable="w14 w15 w16se w16cid w16 w16cex w16sdtdh w16sdtfl w16du""#)
    );
}

#[test]
fn removing_what_a_word_file_does_not_have_changes_nothing() {
    let mut doc = corpus_doc("word/report.docx");
    assert_eq!(doc.remove_view().unwrap(), None);
    assert_eq!(doc.remove_proof_state().unwrap(), None);
    assert_eq!(doc.remove_document_protection().unwrap(), None);
    assert_eq!(doc.remove_mail_merge_settings().unwrap(), None);
    assert_eq!(doc.remove_document_variable("Client").unwrap(), None);
    assert_eq!(doc.remove_link_styles().unwrap(), None);
    assert_eq!(doc.remove_auto_hyphenation().unwrap(), None);
    assert_eq!(
        doc.remove_compatibility_option(CompatibilityOption::NoLeading)
            .unwrap(),
        None
    );
    assert_eq!(
        doc.remove_compatibility_setting("compatibilityMode", "urn:other")
            .unwrap(),
        None
    );
    assert_eq!(doc.remove_web_encoding().unwrap(), None);
    assert_eq!(doc.remove_custom_property("Client").unwrap(), None);
    assert_eq!(doc.remove_custom_properties().unwrap(), None);
    for part in [
        "word/settings.xml",
        "word/webSettings.xml",
        "docProps/core.xml",
        "docProps/app.xml",
    ] {
        assert_eq!(
            doc.part_xml(part).unwrap(),
            corpus_part("word/report.docx", part),
            "{part} is as Word wrote it"
        );
    }
}

#[test]
fn a_google_file_gains_properties_and_settings_and_keeps_its_own() {
    let mut doc = corpus_doc("google/report.docx");
    // Google Docs writes no document properties.
    assert_eq!(doc.core_properties(), None);
    assert_eq!(doc.application_properties(), None);
    assert_eq!(doc.default_tab_stop(), Some(Twips(720)));
    assert_eq!(
        doc.compatibility_settings()
            .iter()
            .map(|s| (s.name.as_str(), s.value.as_str()))
            .collect::<Vec<_>>(),
        [("compatibilityMode", "15")]
    );
    let body = texts(&doc);

    doc.set_title("Quarterly review");
    doc.set_author("Operations");
    doc.set_zoom(DocumentZoom {
        kind: None,
        percent: Some(125),
    })
    .unwrap();
    doc.set_document_variable("Source", "Google Docs").unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.title(), Some("Quarterly review"));
    assert_eq!(doc.author(), Some("Operations"));
    assert_eq!(doc.zoom().and_then(|zoom| zoom.percent), Some(125));
    assert_eq!(doc.document_variable("Source"), Some("Google Docs"));
    assert_eq!(texts(&doc), body);

    // Google's settings root binds a default namespace and declares the
    // compatibility value before its name: both stay.
    let xml = settings(&doc);
    assert!(
        xml.contains(r#"xmlns="http://schemas.microsoft.com/office/tasks/2019/documenttasks""#)
    );
    assert_in_order(
        &xml,
        &[
            r#"<w:zoom w:percent="125"/>"#,
            r#"<w:embedTrueTypeFonts w:val="1"/>"#,
            r#"<w:compatSetting w:val="15" w:name="compatibilityMode""#,
            r#"<w:docVars>"#,
            "<w:clrSchemeMapping",
        ],
    );
}

// Fails today: these setters take a character XML cannot carry, then every save
// of the document fails with InvalidXmlCharacter. Issue: #181
#[test]
#[should_panic(expected = "accepted a character XML cannot carry")]
fn a_value_xml_cannot_carry_is_refused_and_the_document_still_saves() {
    type Attempt = fn(&mut Document) -> Result<()>;
    let attempts: [(&str, Attempt); 4] = [
        ("set_document_variable", |doc| {
            doc.set_document_variable("Bell", "ring\u{7}")
        }),
        ("set_custom_property", |doc| {
            doc.set_custom_property(custom(
                "Bell",
                2,
                CustomPropertyValue::Lpwstr("ring\u{7}".to_owned()),
            ))
        }),
        ("set_application_properties", |doc| {
            let mut app = doc.application_properties().cloned().unwrap_or_default();
            app.company = Some("ring\u{7}".to_owned());
            doc.set_application_properties(app)
        }),
        ("set_core_properties", |doc| {
            let mut core = doc.core_properties().cloned().unwrap_or_default();
            core.title = Some("ring\u{7}".to_owned());
            doc.set_core_properties(core)
        }),
    ];
    // Every setter is tried, so a fix of some of them keeps this failing.
    let mut accepted = Vec::new();
    for (setter, attempt) in attempts {
        let mut doc = doc_with(&["Body"]);
        let before = doc.to_bytes().unwrap();
        if attempt(&mut doc).is_ok() {
            accepted.push(setter);
            continue;
        }
        assert_eq!(doc.to_bytes().unwrap(), before, "{setter}");
    }
    assert!(
        accepted.is_empty(),
        "{accepted:?} accepted a character XML cannot carry"
    );
}
