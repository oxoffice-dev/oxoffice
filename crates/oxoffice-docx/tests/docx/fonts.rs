//! Fonts: the font table (names, alternate names, families, pitches),
//! embedded font faces (license checks, obfuscated storage, replacing and
//! removing them), the embedding settings, the document default font and size,
//! theme fonts and the theme font language, on new and corpus documents.

use std::fs;
use std::io::Cursor;

use oxoffice_docx::{
    CT_RPr, Document, EmbeddedFont, EmbeddedFontKind, FontDefinition, FontEmbeddingLicense,
    RunFontSlot, ThemeFontLanguage, Twips, WordCreationProfile, WordPackageClass, derive_font_key,
};
use oxoffice_opc::{OpcPackage, Relationship};

use crate::support::{corpus_doc, corpus_root, doc_with, reopen, texts};

use EmbeddedFontKind::{Bold, BoldItalic, Italic, Regular};

/// A 4 KB subset of Noto Sans Symbols 2, bundled with the layout crate under
/// the SIL Open Font License 1.1.
const NOTO_SYMBOLS_2: &[u8] =
    include_bytes!("../../../oxoffice-layout/fonts/NotoSansSymbols2-bullets-subset.ttf");
const OFL: &str = "SIL Open Font License 1.1";
const KEY: &str = "{00112233-4455-6677-8899-AABBCCDDEEFF}";
const OTHER_KEY: &str = "{8F1E2D3C-4B5A-6978-8796-A5B4C3D2E1F0}";
const NULL_KEY: &str = "{00000000-0000-0000-0000-000000000000}";
const FONT_TABLE: &str = "/word/fontTable.xml";
const FONT_TABLE_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.fontTable+xml";
const FONT_TABLE_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/fontTable";
const THEME_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme";
const THEME_TYPE: &str = "application/vnd.openxmlformats-officedocument.theme+xml";
const FONT_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/font";
const OBFUSCATED_FONT_TYPE: &str = "application/vnd.openxmlformats-officedocument.obfuscatedFont";
const WORD_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// A minimal sfnt face: a `name` table with no records followed by
/// `marker`, and `OS/2` (version 0) and `head` tables of their minimum
/// lengths. Enough for the embedding checks, and each marker gives
/// different bytes.
fn tiny_font(marker: &str) -> Vec<u8> {
    let mut name = vec![0, 0, 0, 0, 0, 6]; // format 0, no records, strings at 6
    name.extend_from_slice(marker.as_bytes());
    sfnt(&[
        (*b"OS/2", vec![0; 78]),
        (*b"head", vec![0; 54]),
        (*b"name", name),
    ])
}

/// A TrueType face holding `tables`, in that order, with its header's
/// search hints.
fn sfnt(tables: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let count = tables.len() as u16;
    let selector = count.ilog2() as u16;
    let range = (1u16 << selector) * 16;
    let mut font = Vec::new();
    font.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // TrueType outlines
    font.extend_from_slice(&count.to_be_bytes());
    font.extend_from_slice(&range.to_be_bytes());
    font.extend_from_slice(&selector.to_be_bytes());
    font.extend_from_slice(&(count * 16 - range).to_be_bytes());
    let mut offset = 12 + 16 * tables.len();
    for (tag, table) in tables {
        font.extend_from_slice(tag);
        font.extend_from_slice(&0u32.to_be_bytes()); // checksum
        font.extend_from_slice(&(offset as u32).to_be_bytes());
        font.extend_from_slice(&(table.len() as u32).to_be_bytes());
        offset += table.len();
    }
    for (_, table) in tables {
        font.extend_from_slice(table);
    }
    font
}

/// The caller's go-ahead to embed a face, under the OFL.
fn license() -> FontEmbeddingLicense {
    FontEmbeddingLicense::new(true, OFL)
}

/// One face to embed under `KEY` and the OFL.
fn face(kind: EmbeddedFontKind, data: Vec<u8>) -> EmbeddedFont {
    EmbeddedFont::new(kind, data, KEY, license())
}

/// The font-table record named `name`, which must be there once.
fn font<'a>(fonts: &'a [FontDefinition], name: &str) -> &'a FontDefinition {
    let mut matches = fonts.iter().filter(|font| font.name == name);
    let found = matches
        .next()
        .unwrap_or_else(|| panic!("the font table lists {name}"));
    assert!(matches.next().is_none(), "the font table lists {name} once");
    found
}

/// The names of the font-table records, in file order.
fn font_names(doc: &Document) -> Vec<String> {
    doc.try_fonts()
        .expect("font table reads")
        .into_iter()
        .map(|font| font.name)
        .collect()
}

/// The package the document saves now.
fn saved_package(doc: &mut Document) -> OpcPackage {
    let bytes = doc.to_bytes().expect("document saves");
    OpcPackage::from_reader(Cursor::new(bytes)).expect("saved package opens")
}

/// The package of a corpus file, as its producer wrote it.
fn corpus_package(name: &str) -> OpcPackage {
    let bytes = fs::read(corpus_root().join("files").join(name)).expect("corpus file reads");
    OpcPackage::from_reader(Cursor::new(bytes)).expect("corpus package opens")
}

/// The parts stored under `/word/fonts/`, sorted.
fn font_parts(package: &OpcPackage) -> Vec<String> {
    let mut parts: Vec<String> = package
        .parts
        .keys()
        .filter(|part| part.starts_with("/word/fonts/"))
        .cloned()
        .collect();
    parts.sort();
    parts
}

/// The font relationships of the font table, in file order.
fn font_relationships(package: &OpcPackage) -> Vec<Relationship> {
    package
        .get_part_rels(FONT_TABLE)
        .map(|relationships| {
            relationships
                .items
                .iter()
                .filter(|relationship| relationship.rel_type == FONT_REL)
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// The part a font-table relationship points at.
fn font_part_of(relationship: &Relationship) -> String {
    OpcPackage::resolve_rel_target(FONT_TABLE, &relationship.target)
}

/// The parts that relationships of `rel_type` from the main document point at.
fn document_targets(package: &OpcPackage, rel_type: &str) -> Vec<String> {
    package
        .get_part_rels("/word/document.xml")
        .expect("document relationships")
        .items
        .iter()
        .filter(|relationship| relationship.rel_type == rel_type)
        .map(|relationship| {
            OpcPackage::resolve_rel_target("/word/document.xml", &relationship.target)
        })
        .collect()
}

/// The XML of the `w:font` record named `name` in a font-table part.
fn font_record_xml<'a>(xml: &'a str, name: &str) -> &'a str {
    let start = xml
        .find(&format!(r#"<w:font w:name="{name}">"#))
        .unwrap_or_else(|| panic!("the font table writes a record for {name}: {xml}"));
    let end = start + xml[start..].find("</w:font>").expect("record end");
    &xml[start..end]
}

/// A new document whose font table is `records` (raw `w:font` XML) and whose
/// font table relates these faces: (relationship id, relative target, content
/// type, stored bytes). The faces go under `/word/`.
fn doc_with_font_table(records: &str, faces: &[(&str, &str, &str, Vec<u8>)]) -> Document {
    let bytes = Document::new().to_bytes().expect("new document saves");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:fonts xmlns:w="{WORD_NS}" xmlns:r="{REL_NS}">{records}</w:fonts>"#
    );
    package.set_part(FONT_TABLE, xml.into_bytes());
    for (id, target, content_type, data) in faces {
        let part = OpcPackage::resolve_rel_target(FONT_TABLE, target);
        package.set_part(&part, data.clone());
        package.content_types.add_override(&part, content_type);
        package
            .get_or_create_part_rels(FONT_TABLE)
            .add_with_id(id, FONT_REL, target);
    }
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    Document::from_bytes(&out.into_inner()).expect("document with this font table opens")
}

/// A new document whose font table holds `record` (raw `w:font` XML), and
/// relates `rId9` to a font part as an embedded face would, without a
/// modelled face naming it.
fn doc_with_font_record(record: &str) -> Document {
    doc_with_font_table(
        record,
        &[(
            "rId9",
            "fonts/font9.odttf",
            OBFUSCATED_FONT_TYPE,
            b"stored face bytes".to_vec(),
        )],
    )
}

// ---- Reading the font table ----

#[test]
fn google_embedded_noto_faces_read_as_their_stored_truetype_bytes() {
    // Google Docs stores plain TrueType faces under a null key.
    let doc = corpus_doc("google/report.docx");
    let package = corpus_package("google/report.docx");
    assert_eq!(font_names(&doc), ["Calibri", "Arial", "Noto Sans Symbols"]);
    let fonts = doc.fonts();
    let noto = font(&fonts, "Noto Sans Symbols");
    let faces: Vec<_> = noto
        .embedded_fonts
        .iter()
        .map(|face| (face.kind, face.font_key.as_str(), face.subsetted))
        .collect();
    assert_eq!(faces, [(Regular, NULL_KEY, false), (Bold, NULL_KEY, false)]);
    for (index, part) in [
        "/word/fonts/NotoSansSymbols-regular.ttf",
        "/word/fonts/NotoSansSymbols-bold.ttf",
    ]
    .into_iter()
    .enumerate()
    {
        let stored = package.get_part(part).expect("Google stores the face");
        assert!(
            stored.starts_with(&[0, 1, 0, 0]),
            "{part} is plain TrueType"
        );
        assert_eq!(noto.embedded_fonts[index].data, stored, "{part}");
        assert_eq!(
            doc.font_part_metadata(2, index).unwrap(),
            (part.to_owned(), stored.len())
        );
    }
    // Google records no license: the faces read as not authorized.
    assert!(
        noto.embedded_fonts
            .iter()
            .all(|face| face.license == FontEmbeddingLicense::new(false, ""))
    );
    assert!(doc.font_part_metadata(2, 2).is_err());
    assert_eq!(doc.embed_true_type_fonts().unwrap(), Some(true));
}

// ---- Describing fonts ----

#[test]
fn describing_a_font_again_updates_its_record_in_place() {
    let mut doc = Document::new();
    doc.set_font(
        FontDefinition::new("Calibri")
            .with_alternate_name("Carlito")
            .with_family("swiss")
            .with_pitch("variable"),
    )
    .unwrap();
    // The same description again changes nothing.
    let before = doc.to_bytes().unwrap();
    doc.set_font(
        FontDefinition::new("Calibri")
            .with_alternate_name("Carlito")
            .with_family("swiss")
            .with_pitch("variable"),
    )
    .unwrap();
    assert_eq!(doc.to_bytes().unwrap(), before);
    // A new description replaces every field, and an absent one is dropped.
    doc.set_font(FontDefinition::new("Calibri").with_family("roman"))
        .unwrap();

    let doc = reopen(&mut doc);
    assert_eq!(font_names(&doc), ["Calibri", "Times New Roman"]);
    assert_eq!(
        doc.fonts()[0],
        FontDefinition::new("Calibri").with_family("roman")
    );
}

#[test]
fn a_font_name_written_twice_is_changed_and_removed_by_its_index() {
    // LibreOffice writes Symbol twice: records 1 and 8.
    let mut doc = corpus_doc("libreoffice/report.docx");
    let names = font_names(&doc);
    assert_eq!(names[1], "Symbol");
    assert_eq!(names[8], "Symbol");
    assert_eq!(names.iter().filter(|name| *name == "Symbol").count(), 2);

    // By name, every call refuses to pick one and points at the index.
    let before = doc.to_bytes().unwrap();
    let refusals = [
        doc.set_font(FontDefinition::new("Symbol")).unwrap_err(),
        doc.remove_font("Symbol").unwrap_err(),
        doc.embed_font("Symbol", face(Regular, tiny_font("Symbol face")))
            .unwrap_err(),
        doc.remove_embedded_font("Symbol", Regular).unwrap_err(),
    ];
    for error in refusals {
        let error = error.to_string();
        assert!(
            error.contains("2 records are named Symbol") && error.contains("index"),
            "{error}"
        );
    }
    assert_eq!(doc.to_bytes().unwrap(), before);

    // An index outside the table, or a name that is not the record's.
    let error = doc
        .set_font_at(9, FontDefinition::new("Symbol"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("out of range"), "{error}");
    let error = doc
        .set_font_at(8, FontDefinition::new("Wingdings"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("does not rename"), "{error}");
    assert_eq!(doc.to_bytes().unwrap(), before);

    // The second record changes; the first stays as LibreOffice wrote it.
    doc.set_font_at(
        8,
        FontDefinition::new("Symbol")
            .with_family("decorative")
            .with_pitch("variable"),
    )
    .unwrap();
    doc.embed_font_at(8, face(Regular, tiny_font("Symbol face")))
        .unwrap();
    let mut doc = reopen(&mut doc);
    let fonts = doc.try_fonts().unwrap();
    assert_eq!(fonts[1].family.as_deref(), Some("roman"));
    assert!(fonts[1].embedded_fonts.is_empty());
    assert_eq!(fonts[8].family.as_deref(), Some("decorative"));
    assert_eq!(fonts[8].pitch.as_deref(), Some("variable"));
    assert_eq!(fonts[8].embedded_fonts.len(), 1);
    assert_eq!(fonts[8].embedded_fonts[0].data, tiny_font("Symbol face"));

    let removed = doc.remove_embedded_font_at(8, Regular).unwrap();
    assert_eq!(
        removed.map(|face| face.data),
        Some(tiny_font("Symbol face"))
    );
    assert_eq!(doc.remove_embedded_font_at(8, Regular).unwrap(), None);
    assert!(font_parts(&saved_package(&mut doc)).is_empty());

    // Once the second record is gone, the name is unique again.
    let removed = doc.remove_font_at(8).unwrap();
    assert_eq!(removed.family.as_deref(), Some("decorative"));
    assert!(doc.remove_font_at(8).is_err());
    let doc = reopen(&mut doc);
    assert_eq!(font_names(&doc), names[..8]);
    let mut doc = doc;
    assert_eq!(
        doc.remove_font("Symbol").unwrap().map(|font| font.name),
        Some("Symbol".to_owned())
    );
}

#[test]
fn font_descriptions_outside_the_schema_are_refused_and_change_nothing() {
    let mut doc = Document::new();
    let before = doc.to_bytes().unwrap();
    let mut with_face = FontDefinition::new("Faces");
    with_face
        .embedded_fonts
        .push(face(Regular, tiny_font("face given to set_font")));
    for refused in [
        FontDefinition::new("Gelasio").with_family("serif"),
        FontDefinition::new("Gelasio").with_pitch("proportional"),
        FontDefinition::new(""),
        FontDefinition::new("   "),
        FontDefinition::new("Tab\tName"),
        FontDefinition::new("Gelasio").with_alternate_name("bell\u{7}"),
        with_face,
    ] {
        let name = refused.name.clone();
        assert!(doc.set_font(refused).is_err(), "{name:?} is refused");
    }
    assert_eq!(doc.to_bytes().unwrap(), before);
    assert_eq!(font_names(&doc), ["Calibri", "Times New Roman"]);
}

#[test]
fn a_minimal_document_gets_a_font_table_and_a_theme_when_first_given_them() {
    let mut doc =
        Document::new_with_profile(WordCreationProfile::Minimal(WordPackageClass::Document));
    assert!(doc.try_fonts().unwrap().is_empty());
    assert!(doc.theme().is_none());
    doc.set_font(FontDefinition::new("Gelasio").with_family("roman"))
        .unwrap();
    let mut theme = oxoffice_drawing::theme::CT_OfficeStyleSheet::office_default();
    theme.theme_elements.font_scheme.minor_font.latin.typeface = "Carlito".to_owned();
    doc.set_theme(theme.clone()).unwrap();

    let package = saved_package(&mut doc);
    assert_eq!(document_targets(&package, FONT_TABLE_REL), [FONT_TABLE]);
    assert_eq!(
        package.content_types.content_type_for(FONT_TABLE),
        Some(FONT_TABLE_TYPE)
    );
    let themes = document_targets(&package, THEME_REL);
    assert_eq!(themes.len(), 1);
    assert_eq!(
        package.content_types.content_type_for(&themes[0]),
        Some(THEME_TYPE)
    );
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.fonts(),
        [FontDefinition::new("Gelasio").with_family("roman")]
    );
    assert_eq!(doc.theme(), Some(&theme));
}

// ---- Removing the records Word and LibreOffice write ----

#[test]
fn removing_from_a_font_table_that_cannot_be_read_is_refused() {
    // The family before the charset is out of schema order: the table is
    // kept as written and not modelled.
    let mut doc = doc_with_font_record(
        r#"<w:font w:name="Gelasio"><w:family w:val="roman"/><w:charset w:val="00"/></w:font>"#,
    );
    let error = doc.try_fonts().unwrap_err().to_string();
    assert!(error.contains("unmodeled"), "{error}");
    let error = doc
        .set_font(FontDefinition::new("Gelasio"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("unmodeled font table"), "{error}");
    let before = doc.to_bytes().expect("document saves");

    let removals = [
        doc.remove_font("Gelasio").map(|removed| removed.is_some()),
        doc.remove_embedded_font("Gelasio", Regular)
            .map(|removed| removed.is_some()),
    ];
    assert_eq!(doc.to_bytes().expect("document saves"), before);
    for removal in removals {
        let error = removal.expect_err("a removal from a font table it cannot read is refused");
        assert!(error.to_string().contains("unmodeled"), "{error}");
    }
    let error = doc.font_part_metadata(0, 0).unwrap_err().to_string();
    assert!(error.contains("unmodeled"), "{error}");
}

#[test]
fn a_font_record_naming_a_relationship_outside_its_namespace_is_still_refused() {
    // rId9 relates the font table to a font part; the kept XML names it
    // through an attribute in another namespace, so removing the record
    // would leave the relationship and the part with nothing to use them.
    let named = r#"<x:face xmlns:x="urn:example:fonts" x:rel="rId9"/>"#;
    let mut doc = doc_with_font_record(&format!(
        r#"<w:font w:name="Gelasio"><w:charset w:val="00"/>{named}</w:font>"#
    ));
    let before = doc.to_bytes().expect("document saves");
    let error = doc
        .remove_font("Gelasio")
        .expect_err("the record names rId9")
        .to_string();
    assert!(
        error.contains("retained XML names a relationship: font Gelasio names rId9"),
        "{error}"
    );
    assert_eq!(doc.to_bytes().expect("document saves"), before);

    // Another record naming rId9 keeps it in use: the first one goes, and
    // the last one is refused.
    let mut doc = doc_with_font_record(&format!(
        r#"<w:font w:name="Gelasio">{named}</w:font><w:font w:name="Lora">{named}</w:font>"#
    ));
    doc.remove_font("Gelasio").expect("Lora still names rId9");
    assert!(doc.remove_font("Lora").is_err());
    assert_eq!(font_names(&reopen(&mut doc)), ["Lora"]);

    // A record naming its own modelled face's relationship goes with it.
    let mut doc = doc_with_font_record(&format!(
        r#"<w:font w:name="Gelasio"><w:embedRegular r:id="rId9" w:fontKey="{KEY}"/>{named}</w:font>"#
    ));
    doc.remove_font("Gelasio")
        .expect("rId9 is the record's own face");
    let mut doc = reopen(&mut doc);
    assert!(font_names(&doc).is_empty());
    assert!(font_parts(&saved_package(&mut doc)).is_empty());

    // A value that names no relationship of the font table does not block.
    let mut doc = doc_with_font_record(
        r#"<w:font w:name="Gelasio"><w:charset w:val="00"/><x:face xmlns:x="urn:example:fonts" x:rel="rId77"/></w:font>"#,
    );
    doc.remove_font("Gelasio")
        .expect("rId77 is not a relationship of the font table");
    assert!(font_names(&reopen(&mut doc)).is_empty());
}

#[test]
fn an_unknown_child_that_names_no_relationship_goes_with_its_record() {
    let mut doc = doc_with_font_record(
        r#"<w:font w:name="Gelasio"><w:panose1 w:val="02020503060000020004"/><x:note xmlns:x="urn:example:fonts" x:by="a producer"/><w:charset w:val="00"/><w:family w:val="roman"/></w:font><w:font w:name="Courier"><w:family w:val="modern"/></w:font>"#,
    );
    assert_eq!(
        doc.remove_font("Gelasio")
            .expect("the record can be removed"),
        Some(FontDefinition::new("Gelasio").with_family("roman"))
    );

    let doc = reopen(&mut doc);
    assert_eq!(font_names(&doc), ["Courier"]);
    let xml = doc.part_xml(FONT_TABLE).expect("font table");
    assert!(!xml.contains("urn:example:fonts"), "{xml}");
}

#[test]
fn clearing_a_field_that_holds_unmodelled_xml_is_refused() {
    // (record, the field the bare definition clears and that blocks)
    for (record, blocking) in [
        (
            r#"<w:font w:name="Gelasio"><w:family w:val="roman" xmlns:x="urn:example:fonts" x:hint="serif"/><w:pitch w:val="variable"/></w:font>"#,
            "family",
        ),
        (
            r#"<w:font w:name="Gelasio"><w:family w:val="roman"><!-- a producer's note --></w:family></w:font>"#,
            "family",
        ),
        (
            r#"<w:font w:name="Gelasio"><w:pitch w:val="variable" xmlns:x="urn:example:fonts" x:hint="fixed"/></w:font>"#,
            "pitch",
        ),
        (
            r#"<w:font w:name="Gelasio"><w:altName w:val="Georgia" xmlns:x="urn:example:fonts" x:hint="serif"/></w:font>"#,
            "alternate name",
        ),
    ] {
        let mut doc = doc_with_font_record(record);
        let before = doc.to_bytes().expect("document saves");
        let error = doc
            .set_font(FontDefinition::new("Gelasio"))
            .expect_err(record)
            .to_string();
        assert!(
            error.contains(&format!("cannot clear the {blocking} of font Gelasio")),
            "{record}: {error}"
        );
        assert_eq!(doc.to_bytes().expect("document saves"), before, "{record}");
    }

    // Clearing another field keeps the family and its attribute.
    let mut doc = doc_with_font_record(
        r#"<w:font w:name="Gelasio"><w:family w:val="roman" xmlns:x="urn:example:fonts" x:hint="serif"/><w:pitch w:val="variable"/></w:font>"#,
    );
    doc.set_font(FontDefinition::new("Gelasio").with_family("roman"))
        .expect("the pitch can be cleared");
    let xml = reopen(&mut doc).part_xml(FONT_TABLE).expect("font table");
    assert!(xml.contains(r#"x:hint="serif""#), "{xml}");
    assert!(!xml.contains("w:pitch"), "{xml}");
}

#[test]
fn setting_a_field_the_record_keeps_as_written_is_refused() {
    // A field element without `w:val`, or with an element inside, reads as
    // unset and stays as written; writing a second one would leave a font
    // table the next open cannot read. (record, field set, that field.)
    for (record, definition, field) in [
        (
            r#"<w:font w:name="Gelasio"><w:family w:val="roman"><x:ext xmlns:x="urn:example:fonts"/></w:family></w:font>"#,
            FontDefinition::new("Gelasio").with_family("swiss"),
            "family",
        ),
        (
            r#"<w:font w:name="Gelasio"><w:family/></w:font>"#,
            FontDefinition::new("Gelasio").with_family("swiss"),
            "family",
        ),
        (
            r#"<w:font w:name="Gelasio"><w:pitch><x:ext xmlns:x="urn:example:fonts"/></w:pitch></w:font>"#,
            FontDefinition::new("Gelasio").with_pitch("fixed"),
            "pitch",
        ),
        (
            r#"<w:font w:name="Gelasio"><w:altName xmlns:x="urn:example:fonts" x:val="Georgia"/></w:font>"#,
            FontDefinition::new("Gelasio").with_alternate_name("Georgia"),
            "alternate name",
        ),
        (
            r#"<w:font w:name="Gelasio"><w:pitch xmlns:x="urn:example:fonts" x:val="fixed"> </w:pitch></w:font>"#,
            FontDefinition::new("Gelasio").with_pitch("fixed"),
            "pitch",
        ),
    ] {
        let mut doc = doc_with_font_record(record);
        let before = doc.to_bytes().expect("document saves");
        let error = doc.set_font(definition).expect_err(record).to_string();
        assert!(
            error.contains(&format!("cannot set the {field} of font Gelasio")),
            "{record}: {error}"
        );
        assert_eq!(doc.to_bytes().expect("document saves"), before, "{record}");
    }

    // Another field is set beside the kept element, and the table still reads.
    let mut doc = doc_with_font_record(
        r#"<w:font w:name="Gelasio"><w:family w:val="roman"><x:ext xmlns:x="urn:example:fonts"/></w:family></w:font>"#,
    );
    doc.set_font(FontDefinition::new("Gelasio").with_pitch("variable"))
        .expect("the pitch can be set");
    let mut reopened = reopen(&mut doc);
    assert_eq!(
        font(&reopened.fonts(), "Gelasio").pitch.as_deref(),
        Some("variable")
    );
    let xml = reopened.part_xml(FONT_TABLE).expect("font table");
    assert_eq!(xml.matches("<w:family").count(), 1, "{xml}");
    reopened
        .set_font(FontDefinition::new("Gelasio").with_pitch("fixed"))
        .expect("the reopened table is still modelled");

    // An unknown child where the family would go is not a kept family.
    let mut doc = doc_with_font_record(
        r#"<w:font w:name="Gelasio"><w:charset w:val="00"/><x:note xmlns:x="urn:example:fonts"/></w:font>"#,
    );
    doc.set_font(FontDefinition::new("Gelasio").with_family("swiss"))
        .expect("the family can be set beside an unknown child");
    let doc = reopen(&mut doc);
    assert_eq!(
        font(&doc.fonts(), "Gelasio").family.as_deref(),
        Some("swiss")
    );
}

#[test]
fn embedding_a_face_beside_a_face_the_record_keeps_as_written_is_refused() {
    let record = format!(
        r#"<w:font w:name="Gelasio"><w:embedRegular r:id="rId9" w:fontKey="{KEY}"><x:note xmlns:x="urn:example:fonts"/></w:embedRegular></w:font>"#
    );
    let mut doc = doc_with_font_record(&record);
    let before = doc.to_bytes().expect("document saves");
    let error = doc
        .embed_font("Gelasio", face(Regular, tiny_font("Gelasio regular")))
        .expect_err("a second regular face")
        .to_string();
    assert!(
        error.contains("cannot embed the embedRegular face of font Gelasio"),
        "{error}"
    );
    assert_eq!(doc.to_bytes().expect("document saves"), before);

    // A face of another kind goes in, and the table still reads.
    doc.embed_font("Gelasio", face(Bold, tiny_font("Gelasio bold")))
        .expect("a bold face");
    let doc = reopen(&mut doc);
    assert_eq!(font(&doc.fonts(), "Gelasio").embedded_fonts.len(), 1);
}

#[test]
fn a_run_font_name_with_markup_characters_survives_a_save() {
    let mut doc = Document::new();
    doc.add_paragraph("").add_run("x").font("A&B \"Q\" <C>");
    let first_font = |doc: &Document| {
        let paragraph = doc.paragraph(0).expect("the paragraph");
        let run = paragraph.runs().next().expect("the run");
        run.font_name().map(str::to_owned)
    };
    let doc = reopen(&mut doc);
    assert_eq!(first_font(&doc).as_deref(), Some("A&B \"Q\" <C>"));

    // A tab is kept, save after save: the reader does not normalize it away.
    let mut doc = Document::new();
    doc.add_paragraph("").add_run("x").font("A\tB");
    let mut doc = reopen(&mut doc);
    doc.paragraph_mut(0)
        .expect("the paragraph")
        .run_mut(0)
        .expect("the run")
        .set_bold(true);
    let doc = reopen(&mut doc);
    assert_eq!(first_font(&doc).as_deref(), Some("A\tB"));

    // A file names the font with entities; a foreign attribute stays as written.
    let mut doc = doc_with(&[]);
    doc = {
        let bytes = doc.to_bytes().expect("document saves");
        let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
        let xml =
            String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
        let xml = xml.replacen(
            "<w:body>",
            r#"<w:body><w:p><w:r><w:rPr><w:rFonts w:ascii="A&amp;B" xmlns:x="urn:example:fonts" x:note="1&amp;2"/></w:rPr><w:t>x</w:t></w:r></w:p>"#,
            1,
        );
        package.set_part("/word/document.xml", xml.into_bytes());
        let mut out = Cursor::new(Vec::new());
        package.write_to(&mut out).expect("package writes");
        Document::from_bytes(&out.into_inner()).expect("document opens")
    };
    assert_eq!(first_font(&doc).as_deref(), Some("A&B"));
    // An edit of the run writes its properties again.
    doc.paragraph_mut(0)
        .expect("the paragraph")
        .run_mut(0)
        .expect("the run")
        .set_bold(true);
    let mut doc = reopen(&mut doc);
    assert_eq!(first_font(&doc).as_deref(), Some("A&B"));
    let xml = saved_package(&mut doc)
        .get_part("/word/document.xml")
        .map(|bytes| String::from_utf8(bytes.to_vec()).unwrap())
        .unwrap();
    assert!(xml.contains(r#"w:ascii="A&amp;B""#), "{xml}");
    assert!(xml.contains("<w:b/>"), "{xml}");
    assert!(xml.contains(r#"x:note="1&amp;2""#), "{xml}");
}

// ---- Embedding faces ----

#[test]
fn an_embedded_font_survives_a_save_with_its_bytes_key_and_license() {
    let mut doc = doc_with(&["• bullets drawn from the embedded face"]);
    assert_eq!(doc.embed_true_type_fonts().unwrap(), None);
    let embedded = face(Regular, NOTO_SYMBOLS_2.to_vec()).with_subsetted(true);
    // The font table has no record for this name yet: embedding adds one.
    doc.embed_font("Noto Sans Symbols 2", embedded.clone())
        .unwrap();

    let doc = reopen(&mut doc);
    assert_eq!(
        font_names(&doc),
        ["Calibri", "Times New Roman", "Noto Sans Symbols 2"]
    );
    let fonts = doc.fonts();
    let noto = font(&fonts, "Noto Sans Symbols 2");
    assert_eq!(noto.embedded_fonts, [embedded]);
    assert_eq!(noto.embedded_fonts[0].data, NOTO_SYMBOLS_2);
    assert_eq!(noto.family, None);
    // Word only loads embedded faces with this setting on.
    assert_eq!(doc.embed_true_type_fonts().unwrap(), Some(true));
    assert_eq!(texts(&doc), ["• bullets drawn from the embedded face"]);
}

#[test]
fn the_four_faces_of_a_family_survive_a_save_in_schema_order() {
    let mut doc = Document::new();
    let faces = [
        (BoldItalic, tiny_font("Gelasio bold italic")),
        (Italic, tiny_font("Gelasio italic")),
        (Bold, tiny_font("Gelasio bold")),
        (Regular, tiny_font("Gelasio regular")),
    ];
    for (kind, data) in &faces {
        doc.embed_font("Gelasio", face(*kind, data.clone()))
            .unwrap();
    }

    let mut doc = reopen(&mut doc);
    let fonts = doc.fonts();
    let read: Vec<_> = font(&fonts, "Gelasio")
        .embedded_fonts
        .iter()
        .map(|face| (face.kind, face.data.clone()))
        .collect();
    let mut expected = faces.to_vec();
    expected.reverse();
    assert_eq!(read, expected);
    let xml = doc.part_xml(FONT_TABLE).unwrap();
    let record = font_record_xml(&xml, "Gelasio");
    let positions: Vec<usize> = [
        "<w:embedRegular ",
        "<w:embedBold ",
        "<w:embedItalic ",
        "<w:embedBoldItalic ",
    ]
    .iter()
    .map(|element| record.find(element).expect(element))
    .collect();
    assert!(positions.is_sorted(), "{record}");
    let package = saved_package(&mut doc);
    assert_eq!(font_parts(&package).len(), 4);
    assert_eq!(font_relationships(&package).len(), 4);
}

#[test]
fn embedding_a_face_again_replaces_its_bytes_in_one_part() {
    let mut doc = Document::new();
    doc.embed_font("Gelasio", face(Regular, tiny_font("first regular face")))
        .unwrap();
    let replacement = EmbeddedFont::new(
        Regular,
        tiny_font("second regular face"),
        OTHER_KEY,
        FontEmbeddingLicense::new(true, "Apache License 2.0"),
    );
    doc.embed_font("Gelasio", replacement.clone()).unwrap();
    // The same face once more changes nothing.
    let before = doc.to_bytes().unwrap();
    doc.embed_font("Gelasio", replacement.clone()).unwrap();
    assert_eq!(doc.to_bytes().unwrap(), before);

    let mut doc = reopen(&mut doc);
    assert_eq!(font(&doc.fonts(), "Gelasio").embedded_fonts, [replacement]);
    let package = saved_package(&mut doc);
    assert_eq!(font_parts(&package).len(), 1);
    assert_eq!(font_relationships(&package).len(), 1);
}

#[test]
fn a_face_without_its_os2_head_or_name_table_or_with_an_unprintable_tag_is_refused() {
    let mut doc = Document::new();
    let before = doc.to_bytes().unwrap();
    let complete = tiny_font("complete");
    let mut refused = Vec::new();
    // The same face with one of its three tables renamed away.
    for (tag, renamed) in [(b"OS/2", b"OS/3"), (b"head", b"heae"), (b"name", b"namf")] {
        let mut face = complete.clone();
        let record = face[12..60]
            .chunks(16)
            .position(|record| &record[..4] == tag)
            .expect("the table is listed");
        face[12 + record * 16..12 + record * 16 + 4].copy_from_slice(renamed);
        refused.push((
            format!("a face without {}", String::from_utf8_lossy(tag)),
            face,
        ));
    }
    // A complete face led by a table whose tag is not printable: Word could
    // not tell its de-obfuscation key back on open.
    let mut name = vec![0, 0, 0, 0, 0, 6];
    name.extend_from_slice(b"unprintable");
    refused.push((
        "a face with an unprintable tag".to_owned(),
        sfnt(&[
            ([1, b'a', b'b', b'c'], vec![0; 4]),
            (*b"OS/2", vec![0; 78]),
            (*b"head", vec![0; 54]),
            (*b"name", name),
        ]),
    ));
    for (what, face) in refused {
        let embedded = EmbeddedFont::new(Regular, face, KEY, license());
        assert!(
            doc.embed_font("Tiny", embedded).is_err(),
            "{what} is refused"
        );
    }
    assert_eq!(doc.to_bytes().unwrap(), before);
    doc.embed_font("Tiny", face(Regular, complete))
        .expect("the complete face is embedded");
}

#[test]
fn embedding_needs_an_authorized_license_with_a_named_identity() {
    let mut doc = Document::new();
    let before = doc.to_bytes().unwrap();
    for refused in [
        FontEmbeddingLicense::new(false, OFL),
        FontEmbeddingLicense::new(true, ""),
        FontEmbeddingLicense::new(true, "   "),
        FontEmbeddingLicense::new(true, "OFL\n1.1"),
    ] {
        let identity = refused.identity.clone();
        let embedded = EmbeddedFont::new(Regular, NOTO_SYMBOLS_2.to_vec(), KEY, refused);
        assert!(
            doc.embed_font("Noto Sans Symbols 2", embedded).is_err(),
            "{identity:?} is refused"
        );
    }
    assert_eq!(doc.to_bytes().unwrap(), before);

    // The identity is kept exactly, markup characters included.
    let identity = r#"OFL 1.1 & "Reserved Font Name" <Noto>"#;
    let embedded = EmbeddedFont::new(
        Regular,
        NOTO_SYMBOLS_2.to_vec(),
        KEY,
        FontEmbeddingLicense::new(true, identity),
    );
    doc.embed_font("Noto Sans Symbols 2", embedded).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        font(&doc.fonts(), "Noto Sans Symbols 2").embedded_fonts[0].license,
        FontEmbeddingLicense::new(true, identity)
    );
}

#[test]
fn embedding_refuses_bytes_that_are_not_one_font_face_and_malformed_keys() {
    let mut doc = Document::new();
    let before = doc.to_bytes().unwrap();
    let mut collection = tiny_font("a face inside a collection");
    collection[..4].copy_from_slice(b"ttcf");
    let mut nameless = tiny_font("a face without a name table");
    nameless[44..48].copy_from_slice(b"glyf"); // the `name` record, the third
    let mut truncated = tiny_font("a face cut short");
    truncated.truncate(truncated.len() - 3);
    for (what, data) in [
        ("text", b"plain text, not a font face at all".to_vec()),
        ("collection", collection),
        ("nameless", nameless),
        ("truncated", truncated),
        ("empty", Vec::new()),
    ] {
        assert!(
            doc.embed_font("Broken", face(Regular, data)).is_err(),
            "{what} is refused"
        );
    }
    for key in [
        "{00112233-4455-6677-8899-aabbccddeeff}",
        "00112233-4455-6677-8899-AABBCCDDEEFF",
        "{00112233445566778899AABBCCDDEEFF}",
        "{00112233-4455-6677-8899-AABBCCDDEE}",
        "",
    ] {
        let embedded = EmbeddedFont::new(Regular, NOTO_SYMBOLS_2.to_vec(), key, license());
        assert!(
            doc.embed_font("Noto Sans Symbols 2", embedded).is_err(),
            "{key:?} is refused"
        );
    }
    assert!(
        doc.embed_font("", face(Regular, tiny_font("a face without a name")))
            .is_err()
    );
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn a_derived_font_key_is_stable_per_face_and_embeds() {
    let key = derive_font_key(NOTO_SYMBOLS_2).unwrap();
    assert_eq!(derive_font_key(NOTO_SYMBOLS_2).unwrap(), key);
    // The braced GUID form Word writes, in upper case.
    assert_eq!(key.len(), 38);
    for (index, character) in key.chars().enumerate() {
        match index {
            0 => assert_eq!(character, '{'),
            37 => assert_eq!(character, '}'),
            9 | 14 | 19 | 24 => assert_eq!(character, '-'),
            _ => assert!(
                character.is_ascii_digit() || ('A'..='F').contains(&character),
                "{key}"
            ),
        }
    }
    // The first 16 bytes of the face's SHA-256, so the key an embed without a
    // caller key writes stays the same from one release to the next.
    assert_eq!(key, "{A95E7D74-21E3-0E8A-D304-A0EB48AA18AE}");
    assert_ne!(derive_font_key(&tiny_font("another face")).unwrap(), key);
    assert!(derive_font_key(b"plain text, not a font face at all").is_err());

    let mut doc = Document::new();
    let embedded = EmbeddedFont::new(Regular, NOTO_SYMBOLS_2.to_vec(), key.clone(), license());
    doc.embed_font("Noto Sans Symbols 2", embedded).unwrap();
    let doc = reopen(&mut doc);
    let fonts = doc.fonts();
    let face = &font(&fonts, "Noto Sans Symbols 2").embedded_fonts[0];
    assert_eq!(
        (face.font_key.as_str(), face.data.as_slice()),
        (key.as_str(), NOTO_SYMBOLS_2)
    );
}

#[test]
fn removing_an_embedded_face_drops_its_part_and_keeps_the_record() {
    let mut doc = Document::new();
    doc.set_font(FontDefinition::new("Gelasio").with_family("roman"))
        .unwrap();
    let bold = face(Bold, tiny_font("Gelasio bold"));
    doc.embed_font("Gelasio", face(Regular, tiny_font("Gelasio regular")))
        .unwrap();
    doc.embed_font("Gelasio", bold.clone()).unwrap();
    let mut doc = reopen(&mut doc);

    assert_eq!(
        doc.remove_embedded_font("Gelasio", Bold).unwrap(),
        Some(bold)
    );
    assert_eq!(doc.remove_embedded_font("Gelasio", Bold).unwrap(), None);
    assert_eq!(doc.remove_embedded_font("Gelasio", Italic).unwrap(), None);
    assert_eq!(doc.remove_embedded_font("Nowhere", Regular).unwrap(), None);

    let mut doc = reopen(&mut doc);
    let fonts = doc.fonts();
    let gelasio = font(&fonts, "Gelasio");
    assert_eq!(gelasio.family.as_deref(), Some("roman"));
    let kinds: Vec<_> = gelasio
        .embedded_fonts
        .iter()
        .map(|face| face.kind)
        .collect();
    assert_eq!(kinds, [Regular]);
    assert_eq!(gelasio.embedded_fonts[0].data, tiny_font("Gelasio regular"));
    let package = saved_package(&mut doc);
    let relationships = font_relationships(&package);
    assert_eq!(relationships.len(), 1);
    assert_eq!(font_parts(&package), [font_part_of(&relationships[0])]);
}

#[test]
fn removing_a_font_removes_its_embedded_faces_from_the_package() {
    let mut doc = Document::new();
    doc.embed_font("Gelasio", face(Regular, tiny_font("Gelasio regular")))
        .unwrap();
    doc.embed_font("Gelasio", face(Italic, tiny_font("Gelasio italic")))
        .unwrap();
    let mut doc = reopen(&mut doc);
    let stored = font_parts(&saved_package(&mut doc));
    assert_eq!(stored.len(), 2);

    let removed = doc
        .remove_font("Gelasio")
        .unwrap()
        .expect("Gelasio is listed");
    let kinds: Vec<_> = removed
        .embedded_fonts
        .iter()
        .map(|face| face.kind)
        .collect();
    assert_eq!(kinds, [Regular, Italic]);
    assert_eq!(removed.embedded_fonts[1].data, tiny_font("Gelasio italic"));

    let mut doc = reopen(&mut doc);
    assert_eq!(font_names(&doc), ["Calibri", "Times New Roman"]);
    let package = saved_package(&mut doc);
    assert!(font_parts(&package).is_empty());
    assert!(font_relationships(&package).is_empty());
    for part in &stored {
        assert_eq!(package.content_types.content_type_for(part), None, "{part}");
    }
}

#[test]
fn the_font_embedding_settings_survive_a_save_and_can_be_cleared() {
    let mut doc = Document::new();
    doc.set_embed_true_type_fonts(Some(false)).unwrap();
    doc.set_embed_system_fonts(Some(true)).unwrap();
    doc.set_save_subset_fonts(Some(true)).unwrap();

    let mut doc = reopen(&mut doc);
    assert_eq!(doc.embed_true_type_fonts().unwrap(), Some(false));
    assert_eq!(doc.embed_system_fonts().unwrap(), Some(true));
    assert_eq!(doc.save_subset_fonts().unwrap(), Some(true));
    // Embedding a face turns the TrueType setting back on.
    doc.embed_font("Gelasio", face(Regular, tiny_font("Gelasio regular")))
        .unwrap();
    assert_eq!(doc.embed_true_type_fonts().unwrap(), Some(true));

    doc.set_embed_true_type_fonts(None).unwrap();
    doc.set_embed_system_fonts(None).unwrap();
    doc.set_save_subset_fonts(None).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.embed_true_type_fonts().unwrap(), None);
    assert_eq!(doc.embed_system_fonts().unwrap(), None);
    assert_eq!(doc.save_subset_fonts().unwrap(), None);
    let settings = doc.part_xml("word/settings.xml").unwrap();
    for element in ["embedTrueTypeFonts", "embedSystemFonts", "saveSubsetFonts"] {
        assert!(!settings.contains(element), "{element} in {settings}");
    }
    // Clearing the setting does not take the face out.
    assert_eq!(font(&doc.fonts(), "Gelasio").embedded_fonts.len(), 1);
}

// ---- Embedded faces of the Google-saved report ----

// ---- Document default font and size ----

#[test]
fn clearing_the_default_font_and_size_keeps_the_paragraph_defaults() {
    let mut doc = Document::new();
    doc.set_default_font_name_value(None).unwrap();
    doc.set_default_font_size_value(None).unwrap();
    doc.add_paragraph("plain");

    let doc = reopen(&mut doc);
    assert_eq!(doc.default_font_name(), None);
    assert_eq!(doc.default_font_size(), None);
    let paragraph = doc.paragraph(0).unwrap();
    let run = doc.effective_run_properties(&paragraph, &paragraph.run(0).unwrap());
    assert_eq!(
        (&run.font_ascii, &run.font_east_asia, &run.font_cs, run.sz),
        (&None, &None, &None, None)
    );
    let spacing = doc
        .default_paragraph_properties()
        .expect("paragraph defaults");
    assert_eq!(spacing.space_after, Some(Twips(160)));
}

#[test]
fn the_default_font_replaces_the_theme_fonts_of_the_word_report_defaults() {
    // Word's defaults name theme fonts for every script; Normal overrides
    // only the Latin ones with Calibri.
    let mut doc = corpus_doc("word/report.docx");
    let effective = |doc: &Document, index: usize| -> CT_RPr {
        let paragraph = doc.paragraph(index).unwrap();
        doc.effective_run_properties(&paragraph, &paragraph.run(0).unwrap())
    };
    let heading = doc
        .paragraphs()
        .iter()
        .position(|paragraph| paragraph.style_id() == Some("Titre1"))
        .expect("a heading 1 paragraph");
    let summary = effective(&doc, 1);
    assert_eq!(
        (
            summary.font_ascii_theme.as_deref(),
            summary.font_hansi_theme.as_deref(),
            summary.font_east_asia_theme.as_deref(),
            summary.font_cs_theme.as_deref(),
        ),
        (
            Some("minorHAnsi"),
            Some("minorHAnsi"),
            Some("minorEastAsia"),
            Some("minorBidi")
        )
    );
    doc.set_default_font_name_value(Some("Georgia")).unwrap();

    let doc = reopen(&mut doc);
    assert!(doc.paragraph(1).unwrap().text().starts_with("Summary."));
    let summary = effective(&doc, 1);
    // A theme font left in the defaults would win over the named one.
    assert_eq!(
        (
            summary.font_ascii_theme.as_deref(),
            summary.font_hansi_theme.as_deref(),
            summary.font_east_asia.as_deref(),
            summary.font_east_asia_theme.as_deref(),
            summary.font_cs.as_deref(),
            summary.font_cs_theme.as_deref(),
        ),
        (None, None, Some("Georgia"), None, Some("Georgia"), None)
    );
    // A style naming its own theme font keeps it.
    assert_eq!(
        effective(&doc, heading).font_ascii_theme.as_deref(),
        Some("majorHAnsi")
    );
}

// ---- Theme fonts ----

#[test]
fn run_theme_fonts_and_the_font_hint_survive_a_save() {
    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("");
    let mut run = paragraph.add_run("見出し Heading");
    run.set_slot_theme_font(RunFontSlot::Ascii, Some("majorHAnsi"));
    run.set_slot_theme_font(RunFontSlot::HighAnsi, Some("majorHAnsi"));
    run.set_slot_theme_font(RunFontSlot::EastAsia, Some("majorEastAsia"));
    run.set_font_hint(Some("eastAsia"));
    let mut run = paragraph.add_run("Explicit Latin");
    run.set_slot_theme_font(RunFontSlot::Ascii, Some("minorHAnsi"));
    run.set_slot_theme_font(RunFontSlot::EastAsia, Some("minorEastAsia"));
    // An explicit font replaces the theme font of its own slot only.
    run.set_slot_font(RunFontSlot::Ascii, Some("Gelasio"));
    let mut run = paragraph.add_run("All explicit");
    run.set_slot_theme_font(RunFontSlot::ComplexScript, Some("minorBidi"));
    // A whole-run font replaces every theme font.
    run.set_font("Caladea");

    let doc = reopen(&mut doc);
    let paragraph = doc.paragraph(0).unwrap();
    let slots = |index: usize| {
        let run = paragraph.run(index).unwrap();
        [
            RunFontSlot::Ascii,
            RunFontSlot::HighAnsi,
            RunFontSlot::EastAsia,
            RunFontSlot::ComplexScript,
        ]
        .map(|slot| {
            (
                run.slot_font(slot).map(str::to_owned),
                run.slot_theme_font(slot).map(str::to_owned),
            )
        })
    };
    let theme = |name: &str| (None, Some(name.to_owned()));
    let explicit = |name: &str| (Some(name.to_owned()), None);
    assert_eq!(
        slots(0),
        [
            theme("majorHAnsi"),
            theme("majorHAnsi"),
            theme("majorEastAsia"),
            (None, None)
        ]
    );
    assert_eq!(paragraph.run(0).unwrap().font_hint(), Some("eastAsia"));
    assert_eq!(
        slots(1),
        [
            explicit("Gelasio"),
            (None, None),
            theme("minorEastAsia"),
            (None, None)
        ]
    );
    assert_eq!(slots(2), std::array::from_fn(|_| explicit("Caladea")));
}

// ---- Theme font language ----

#[test]
fn the_theme_font_language_survives_a_save_and_can_be_removed() {
    let mut doc = Document::new();
    assert_eq!(doc.theme_font_language(), None);
    let languages = ThemeFontLanguage {
        latin: Some("fr-FR".to_owned()),
        east_asia: Some("zh-CN".to_owned()),
        bidi: Some("ar-SA".to_owned()),
    };
    doc.set_theme_font_language(languages.clone()).unwrap();

    let mut doc = reopen(&mut doc);
    assert_eq!(doc.theme_font_language(), Some(&languages));
    // The language defaults are the same setting under another name.
    let hebrew = ThemeFontLanguage {
        latin: None,
        east_asia: None,
        bidi: Some("he-IL".to_owned()),
    };
    doc.set_language_defaults(hebrew.clone()).unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(doc.theme_font_language(), Some(&hebrew));

    assert_eq!(doc.remove_theme_font_language().unwrap(), Some(hebrew));
    assert_eq!(doc.remove_theme_font_language().unwrap(), None);
    let doc = reopen(&mut doc);
    assert_eq!(doc.theme_font_language(), None);
    assert!(
        !doc.part_xml("word/settings.xml")
            .unwrap()
            .contains("themeFontLang")
    );
}
