//! Fonts: the font table (names, alternate names, families, pitches),
//! embedded font faces (license checks, obfuscated storage, replacing and
//! removing them), the embedding settings, the document default font and size,
//! theme fonts and the theme font language, on new and corpus documents.

use std::fs;
use std::io::Cursor;

use oxoffice_docx::{
    CT_RPr, Document, EmbeddedFont, EmbeddedFontKind, FontDefinition, FontEmbeddingLicense, Length,
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
const MC_NS: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";

/// A one-table sfnt face whose `name` table holds `marker`: enough for the
/// embedding checks, and each marker gives different bytes. `marker` needs
/// four bytes or more.
fn tiny_font(marker: &str) -> Vec<u8> {
    let table = marker.as_bytes();
    let mut font = Vec::new();
    font.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // TrueType outlines
    font.extend_from_slice(&1u16.to_be_bytes()); // numTables
    font.extend_from_slice(&16u16.to_be_bytes()); // searchRange
    font.extend_from_slice(&0u16.to_be_bytes()); // entrySelector
    font.extend_from_slice(&0u16.to_be_bytes()); // rangeShift
    font.extend_from_slice(b"name");
    font.extend_from_slice(&0u32.to_be_bytes()); // checksum
    font.extend_from_slice(&28u32.to_be_bytes()); // offset
    font.extend_from_slice(&(table.len() as u32).to_be_bytes());
    font.extend_from_slice(table);
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

/// The 16 bytes of a `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}` key, in the
/// order its hex digits are written.
fn key_bytes(key: &str) -> [u8; 16] {
    let hex: String = key.chars().filter(char::is_ascii_hexdigit).collect();
    assert_eq!(hex.len(), 32, "{key} holds 16 bytes");
    let mut bytes = [0u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).expect("hex digit");
    }
    bytes
}

/// The obfuscation XOR key of ECMA-376 Part 1, 17.8.1: the key's bytes read
/// from its last hex pair to its first.
fn spec_xor_key(key: &str) -> [u8; 16] {
    let mut bytes = key_bytes(key);
    bytes.reverse();
    bytes
}

/// The same key when the GUID goes through .NET's `Guid.ToByteArray`, which
/// swaps the bytes of its first three groups, before the reversal.
fn swapped_xor_key(key: &str) -> [u8; 16] {
    let g = key_bytes(key);
    let mut bytes = [
        g[3], g[2], g[1], g[0], g[5], g[4], g[7], g[6], g[8], g[9], g[10], g[11], g[12], g[13],
        g[14], g[15],
    ];
    bytes.reverse();
    bytes
}

/// `data` with its first 32 bytes XORed with `key`, as font obfuscation does
/// both ways.
fn xor_head(data: &[u8], key: [u8; 16]) -> Vec<u8> {
    let mut out = data.to_vec();
    for (index, byte) in out.iter_mut().take(32).enumerate() {
        *byte ^= key[index % 16];
    }
    out
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
fn the_word_report_reads_its_alternate_names_families_and_pitches() {
    // French Word names East Asian fonts in English with their Japanese
    // names as alternates.
    let doc = corpus_doc("word/report.docx");
    assert_eq!(
        font_names(&doc),
        [
            "Symbol",
            "Times New Roman",
            "Cambria",
            "MS Mincho",
            "Calibri",
            "MS Gothic",
            "Courier"
        ]
    );
    let fonts = doc.fonts();
    let described = |name: &str| {
        let font = font(&fonts, name);
        (
            font.alternate_name.as_deref(),
            font.family.as_deref(),
            font.pitch.as_deref(),
        )
    };
    assert_eq!(
        described("MS Mincho"),
        (Some("ＭＳ 明朝"), Some("roman"), Some("fixed"))
    );
    assert_eq!(
        described("MS Gothic"),
        (Some("ＭＳ ゴシック"), Some("modern"), Some("fixed"))
    );
    assert_eq!(
        described("Symbol"),
        (None, Some("decorative"), Some("variable"))
    );
    assert_eq!(
        described("Calibri"),
        (None, Some("swiss"), Some("variable"))
    );
    assert_eq!(described("Cambria"), (None, Some("auto"), Some("variable")));
    assert!(fonts.iter().all(|font| font.embedded_fonts.is_empty()));
}

#[test]
fn the_libreoffice_report_lists_both_of_its_symbol_records_and_refuses_to_pick_one() {
    let mut doc = corpus_doc("libreoffice/report.docx");
    let fonts = doc.fonts();
    let symbols: Vec<(Option<&str>, Option<&str>)> = fonts
        .iter()
        .filter(|font| font.name == "Symbol")
        .map(|font| (font.family.as_deref(), font.pitch.as_deref()))
        .collect();
    assert_eq!(
        symbols,
        [
            (Some("roman"), Some("variable")),
            (Some("auto"), Some("default"))
        ]
    );
    // LibreOffice names the font it drew with and the one it stands for.
    assert_eq!(
        font(&fonts, "Liberation Serif").alternate_name.as_deref(),
        Some("Times New Roman")
    );
    assert_eq!(
        font(&fonts, "Courier").alternate_name.as_deref(),
        Some("Courier New")
    );

    // Which of the two records an edit means is not known: both are refused.
    let before = doc.to_bytes().unwrap();
    assert!(
        doc.set_font(FontDefinition::new("Symbol").with_family("decorative"))
            .is_err()
    );
    let error = doc.remove_font("Symbol").unwrap_err().to_string();
    assert!(error.contains("ambiguous font record"), "{error}");
    assert_eq!(doc.to_bytes().unwrap(), before);
}

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
fn a_described_font_survives_a_save_after_the_default_records() {
    let mut doc = Document::new();
    assert_eq!(font_names(&doc), ["Calibri", "Times New Roman"]);
    let described = FontDefinition::new("Gelasio")
        .with_alternate_name("Georgia")
        .with_family("roman")
        .with_pitch("variable");
    doc.set_font(described.clone()).unwrap();

    let doc = reopen(&mut doc);
    assert_eq!(font_names(&doc), ["Calibri", "Times New Roman", "Gelasio"]);
    assert_eq!(font(&doc.fonts(), "Gelasio"), &described);
}

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
fn font_names_with_markup_characters_and_cjk_text_survive_a_save() {
    let mut doc = Document::new();
    let markup =
        FontDefinition::new(r#"Fira & <Code> "Retina""#).with_alternate_name("Fira 'Code'");
    let cjk = FontDefinition::new("微软雅黑")
        .with_alternate_name("Microsoft YaHei")
        .with_family("swiss");
    let japanese = FontDefinition::new("ＭＳ ゴシック").with_alternate_name("MS Gothic");
    for described in [&markup, &cjk, &japanese] {
        doc.set_font(described.clone()).unwrap();
    }

    let doc = reopen(&mut doc);
    let fonts = doc.fonts();
    assert_eq!(font(&fonts, &markup.name), &markup);
    assert_eq!(font(&fonts, "微软雅黑"), &cjk);
    assert_eq!(font(&fonts, "ＭＳ ゴシック"), &japanese);
}

#[test]
fn describing_a_word_font_keeps_its_panose_charset_and_signature() {
    // The facade reads neither panose, charset nor signature, so they are
    // checked in the saved font table.
    let mut doc = corpus_doc("word/report.docx");
    doc.set_font(
        FontDefinition::new("Calibri")
            .with_alternate_name("Carlito")
            .with_family("swiss")
            .with_pitch("variable"),
    )
    .unwrap();
    doc.embed_font("Calibri", face(Bold, tiny_font("Calibri bold stand-in")))
        .unwrap();

    let doc = reopen(&mut doc);
    let fonts = doc.fonts();
    let calibri = font(&fonts, "Calibri");
    assert_eq!(calibri.alternate_name.as_deref(), Some("Carlito"));
    assert_eq!(calibri.embedded_fonts[0].kind, Bold);
    let xml = doc.part_xml(FONT_TABLE).unwrap();
    let record = font_record_xml(&xml, "Calibri");
    // The children stay in the schema order Word reads them in.
    let positions: Vec<usize> = [
        r#"<w:altName w:val="Carlito"/>"#,
        r#"<w:panose1 w:val="020F0502020204030204"/>"#,
        r#"<w:charset w:val="00"/>"#,
        r#"<w:family w:val="swiss"/>"#,
        r#"<w:pitch w:val="variable"/>"#,
        r#"<w:sig w:usb0="E0002AFF" w:usb1="C000247B" w:usb2="00000009""#,
        "<w:embedBold ",
    ]
    .iter()
    .map(|child| {
        record
            .find(child)
            .unwrap_or_else(|| panic!("{child} in {record}"))
    })
    .collect();
    assert!(positions.is_sorted(), "{record}");
    // The records left alone keep what Word wrote.
    let mincho = font_record_xml(&xml, "MS Mincho");
    assert!(
        mincho.contains(r#"<w:altName w:val="ＭＳ 明朝"/>"#),
        "{mincho}"
    );
    assert!(mincho.contains("<w:notTrueType/>"), "{mincho}");
}

#[test]
fn removing_a_described_font_takes_it_out_of_the_table() {
    let mut doc = Document::new();
    let described = FontDefinition::new("Gelasio").with_family("roman");
    doc.set_font(described.clone()).unwrap();
    assert_eq!(doc.remove_font("Gelasio").unwrap(), Some(described));
    assert_eq!(doc.remove_font("Gelasio").unwrap(), None);
    assert_eq!(doc.remove_font("Not In The Table").unwrap(), None);

    let mut doc = reopen(&mut doc);
    assert_eq!(font_names(&doc), ["Calibri", "Times New Roman"]);
    assert_eq!(
        doc.remove_font("Times New Roman").unwrap(),
        Some(FontDefinition::new("Times New Roman"))
    );
    let doc = reopen(&mut doc);
    assert_eq!(font_names(&doc), ["Calibri"]);
}

#[test]
fn a_font_record_word_wrote_can_be_removed() {
    let mut doc = corpus_doc("word/report.docx");
    let removed = match doc.remove_font("Courier") {
        Ok(removed) => removed,
        Err(error) => panic!("remove_font refused a record Word wrote: {error}"),
    };
    assert_eq!(
        removed.map(|font| font.family),
        Some(Some("auto".to_owned()))
    );
    let doc = reopen(&mut doc);
    assert!(!font_names(&doc).contains(&"Courier".to_owned()));
}

#[test]
fn a_field_of_a_font_record_word_wrote_can_be_cleared() {
    let mut doc = corpus_doc("word/report.docx");
    assert_eq!(
        font(&doc.fonts(), "MS Mincho").alternate_name.as_deref(),
        Some("ＭＳ 明朝")
    );
    let without_alternate_name = FontDefinition::new("MS Mincho")
        .with_family("roman")
        .with_pitch("fixed");
    if let Err(error) = doc.set_font(without_alternate_name) {
        panic!("set_font refused to clear a field of a record Word wrote: {error}");
    }
    let doc = reopen(&mut doc);
    let fonts = doc.fonts();
    let record = font(&fonts, "MS Mincho");
    assert_eq!(
        (
            record.alternate_name.as_deref(),
            record.family.as_deref(),
            record.pitch.as_deref()
        ),
        (None, Some("roman"), Some("fixed"))
    );
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
fn a_font_record_word_wrote_is_removed_with_its_panose_charset_and_signature() {
    let mut doc = corpus_doc("word/report.docx");
    let removed = doc
        .remove_font("Symbol")
        .expect("Word's Symbol record can be removed");
    assert_eq!(
        removed,
        Some(
            FontDefinition::new("Symbol")
                .with_family("decorative")
                .with_pitch("variable")
        )
    );

    let doc = reopen(&mut doc);
    assert_eq!(
        font_names(&doc),
        [
            "Times New Roman",
            "Cambria",
            "MS Mincho",
            "Calibri",
            "MS Gothic",
            "Courier"
        ]
    );
    // Symbol's panose and signature went with it, the next record keeps its.
    let xml = doc.part_xml(FONT_TABLE).expect("font table");
    assert!(!xml.contains("05050102010706020507"), "{xml}");
    assert!(!xml.contains(r#"w:usb1="10000000""#), "{xml}");
    assert!(
        xml.contains(
            r#"<w:font w:name="Times New Roman"><w:panose1 w:val="02020603050405020304"/><w:charset w:val="00"/>"#
        ),
        "{xml}"
    );
}

#[test]
fn a_font_record_libreoffice_wrote_is_removed_with_its_charset() {
    let mut doc = corpus_doc("libreoffice/report.docx");
    let removed = doc
        .remove_font("Times New Roman")
        .expect("LibreOffice's Times New Roman record can be removed");
    assert_eq!(
        removed,
        Some(
            FontDefinition::new("Times New Roman")
                .with_family("roman")
                .with_pitch("variable")
        )
    );

    let doc = reopen(&mut doc);
    assert_eq!(
        font_names(&doc),
        [
            "Symbol",
            "Arial",
            "Liberation Serif",
            "Cambria",
            "Calibri",
            "Courier",
            "Liberation Sans",
            "Symbol"
        ]
    );
    // The font LibreOffice drew with still names the one it stands for.
    let fonts = doc.fonts();
    let liberation = fonts
        .iter()
        .find(|font| font.name == "Liberation Serif")
        .expect("Liberation Serif record");
    assert_eq!(
        liberation.alternate_name.as_deref(),
        Some("Times New Roman")
    );
}

#[test]
fn a_font_record_whose_kept_xml_names_a_relationship_is_still_refused() {
    // Only modelled embedded faces have their relationship and part removed
    // after the record, so each of these would leave rId9 and its part behind.
    for record in [
        // An unknown child with a relationship id, between Word's children.
        r#"<w:font w:name="Gelasio"><w:panose1 w:val="02020503060000020004"/><x:face xmlns:x="urn:example:fonts" r:id="rId9"/><w:charset w:val="00"/><w:family w:val="roman"/><w:pitch w:val="variable"/></w:font>"#.to_owned(),
        // The same under another prefix for the relationships namespace.
        format!(
            r#"<w:font w:name="Gelasio"><w:charset w:val="00"/><x:face xmlns:x="urn:example:fonts" xmlns:rel="{REL_NS}" rel:embed="rId9"/></w:font>"#
        ),
        // An embedded face behind markup compatibility.
        format!(
            r#"<w:font w:name="Gelasio"><w:charset w:val="00"/><mc:AlternateContent xmlns:mc="{MC_NS}"><mc:Choice Requires="w"><w:embedRegular r:id="rId9" w:fontKey="{KEY}"/></mc:Choice></mc:AlternateContent></w:font>"#
        ),
        // An embedded face whose content the model does not read, so it is
        // kept as written, relationship id included.
        format!(
            r#"<w:font w:name="Gelasio"><w:charset w:val="00"/><w:embedRegular r:id="rId9" w:fontKey="{KEY}"><x:note xmlns:x="urn:example:fonts"/></w:embedRegular></w:font>"#
        ),
    ] {
        let mut doc = doc_with_font_record(&record);
        let before = doc.to_bytes().expect("document saves");
        let error = doc
            .remove_font("Gelasio")
            .expect_err(&record)
            .to_string();
        assert!(error.contains("names a relationship"), "{record}: {error}");
        assert_eq!(font_names(&doc), ["Gelasio"], "{record}");
        assert_eq!(doc.to_bytes().expect("document saves"), before, "{record}");
    }
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
fn a_field_of_a_record_the_removal_guard_refuses_can_be_cleared() {
    // Clearing a field keeps the record, so what the record keeps besides
    // its fields stays: (record, text that must survive).
    for (record, kept) in [
        // Kept XML naming a relationship.
        (
            r#"<w:font w:name="Gelasio"><x:face xmlns:x="urn:example:fonts" r:id="rId9"/><w:family w:val="roman"/><w:pitch w:val="variable"/></w:font>"#.to_owned(),
            r#"r:id="rId9""#,
        ),
        // An embedded face the model does not read, kept as written.
        (
            format!(
                r#"<w:font w:name="Gelasio"><w:family w:val="roman"/><w:pitch w:val="variable"/><w:embedRegular r:id="rId9" w:fontKey="{KEY}"><x:note xmlns:x="urn:example:fonts"/></w:embedRegular></w:font>"#
            ),
            "x:note",
        ),
        // An attribute of the record itself.
        (
            r#"<w:font w:name="Gelasio" xmlns:x="urn:example:fonts" x:by="a producer"><w:family w:val="roman"/><w:pitch w:val="variable"/></w:font>"#.to_owned(),
            r#"x:by="a producer""#,
        ),
        // A modelled embedded face with an attribute it does not model.
        (
            format!(
                r#"<w:font w:name="Gelasio"><w:family w:val="roman"/><w:pitch w:val="variable"/><w:embedRegular r:id="rId9" w:fontKey="{KEY}" xmlns:x="urn:example:fonts" x:by="a producer"/></w:font>"#
            ),
            r#"x:by="a producer""#,
        ),
    ] {
        let mut doc = doc_with_font_record(&record);
        doc.set_font(FontDefinition::new("Gelasio").with_pitch("variable"))
            .unwrap_or_else(|error| panic!("{record}: {error}"));

        let mut doc = reopen(&mut doc);
        let fonts = doc.fonts();
        let gelasio = font(&fonts, "Gelasio");
        assert_eq!(
            (gelasio.family.as_deref(), gelasio.pitch.as_deref()),
            (None, Some("variable")),
            "{record}"
        );
        let xml = doc.part_xml(FONT_TABLE).expect("font table");
        assert!(xml.contains(kept), "{record}: {xml}");
        assert_eq!(
            font_parts(&saved_package(&mut doc)),
            ["/word/fonts/font9.odttf"],
            "{record}"
        );
    }
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
fn an_embedded_face_is_stored_obfuscated_with_its_key() {
    let mut doc = Document::new();
    doc.embed_font(
        "Noto Sans Symbols 2",
        face(Regular, NOTO_SYMBOLS_2.to_vec()),
    )
    .unwrap();

    let package = saved_package(&mut doc);
    let relationships = font_relationships(&package);
    assert_eq!(relationships.len(), 1);
    let part = font_part_of(&relationships[0]);
    assert_eq!(font_parts(&package), std::slice::from_ref(&part));
    assert_eq!(
        package.content_types.content_type_for(&part),
        Some(OBFUSCATED_FONT_TYPE)
    );
    let table = std::str::from_utf8(package.get_part(FONT_TABLE).unwrap()).unwrap();
    let record = font_record_xml(table, "Noto Sans Symbols 2");
    assert!(
        record.contains(&format!(r#"r:id="{}""#, relationships[0].id))
            && record.contains(&format!(r#"w:fontKey="{KEY}""#))
            && record.contains("<w:embedRegular "),
        "{record}"
    );
    // Only the first 32 bytes are XORed with the key, as the standard says.
    let stored = package.get_part(&part).unwrap();
    assert_eq!(stored.len(), NOTO_SYMBOLS_2.len());
    assert_ne!(stored[..32], NOTO_SYMBOLS_2[..32]);
    assert_eq!(stored[32..], NOTO_SYMBOLS_2[32..]);
    assert_eq!(xor_head(stored, spec_xor_key(KEY)), NOTO_SYMBOLS_2);
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
    nameless[12..16].copy_from_slice(b"glyf");
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
fn a_document_with_embedded_fonts_saves_to_the_same_bytes_every_time() {
    let build = || {
        let mut doc = doc_with(&["Same input, same file."]);
        doc.set_font(FontDefinition::new("Gelasio").with_family("roman"))
            .unwrap();
        doc.embed_font("Gelasio", face(Regular, tiny_font("Gelasio regular")))
            .unwrap();
        doc.embed_font("Noto Sans Symbols 2", face(Bold, NOTO_SYMBOLS_2.to_vec()))
            .unwrap();
        doc
    };
    let mut first = build();
    let bytes = first.to_bytes().unwrap();
    assert_eq!(first.to_bytes().unwrap(), bytes);
    assert_eq!(build().to_bytes().unwrap(), bytes);
}

#[test]
fn a_face_obfuscated_with_either_reading_of_its_key_reads_back() {
    // Producers disagree on the byte order of the key: the face reads back
    // whichever one wrote it.
    assert_ne!(spec_xor_key(OTHER_KEY), swapped_xor_key(OTHER_KEY));
    for (reading, xor_key) in [
        ("standard", spec_xor_key(OTHER_KEY)),
        ("byte-swapped", swapped_xor_key(OTHER_KEY)),
    ] {
        let doc = doc_with_font_table(
            &format!(
                r#"<w:font w:name="Noto Sans Symbols 2"><w:family w:val="swiss"/><w:embedRegular r:id="rIdProducer" w:fontKey="{OTHER_KEY}"/></w:font>"#
            ),
            &[(
                "rIdProducer",
                "fonts/producer.odttf",
                OBFUSCATED_FONT_TYPE,
                xor_head(NOTO_SYMBOLS_2, xor_key),
            )],
        );
        let fonts = doc.try_fonts().unwrap();
        let face = &font(&fonts, "Noto Sans Symbols 2").embedded_fonts[0];
        assert_eq!(face.font_key, OTHER_KEY, "{reading}");
        assert!(
            face.data == NOTO_SYMBOLS_2,
            "the {reading} reading reads back"
        );
    }
}

#[test]
fn a_font_table_pointing_at_a_missing_face_is_reported() {
    let record = format!(
        r#"<w:font w:name="Gelasio"><w:embedRegular r:id="rIdGelasio" w:fontKey="{KEY}"/></w:font>"#
    );
    // No relationship for the face at all.
    let mut doc = doc_with_font_table(&record, &[]);
    let error = doc.try_fonts().unwrap_err().to_string();
    assert!(error.contains("font relationships are missing"), "{error}");
    // The rest of the document still reads and saves.
    doc.add_paragraph("Still editable.");
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Still editable."]);

    // A relationship whose face part was stripped from the package.
    let bytes = doc_with_font_table(
        &record,
        &[(
            "rIdGelasio",
            "fonts/gelasio.odttf",
            OBFUSCATED_FONT_TYPE,
            tiny_font("Gelasio regular"),
        )],
    )
    .to_bytes()
    .unwrap();
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    assert!(package.remove_part("/word/fonts/gelasio.odttf").is_some());
    let mut stripped = Cursor::new(Vec::new());
    package.write_to(&mut stripped).unwrap();
    let mut doc = Document::from_bytes(&stripped.into_inner()).unwrap();
    let error = doc.try_fonts().unwrap_err().to_string();
    assert!(error.contains("embedded font part is missing"), "{error}");
    doc.add_paragraph("Still editable.");
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Still editable."]);
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

#[test]
fn google_embedded_faces_survive_an_edit_and_a_save() {
    let original = corpus_package("google/report.docx");
    let mut doc = corpus_doc("google/report.docx");
    doc.add_paragraph("Added after the export.");

    let mut doc = reopen(&mut doc);
    let fonts = doc.fonts();
    let noto = font(&fonts, "Noto Sans Symbols");
    assert_eq!(noto.embedded_fonts.len(), 2);
    let package = saved_package(&mut doc);
    assert_eq!(font_parts(&package), font_parts(&original));
    for part in font_parts(&original) {
        assert_eq!(package.get_part(&part), original.get_part(&part), "{part}");
    }
    assert_eq!(
        noto.embedded_fonts[1].data,
        original
            .get_part("/word/fonts/NotoSansSymbols-bold.ttf")
            .unwrap()
    );
    assert_eq!(doc.embed_true_type_fonts().unwrap(), Some(true));
}

#[test]
fn removing_a_google_embedded_face_drops_its_truetype_part() {
    let original = corpus_package("google/report.docx");
    let bold = original
        .get_part("/word/fonts/NotoSansSymbols-bold.ttf")
        .unwrap();
    let mut doc = corpus_doc("google/report.docx");
    let removed = doc
        .remove_embedded_font("Noto Sans Symbols", Bold)
        .unwrap()
        .expect("Google embeds a bold face");
    assert_eq!(removed.data, bold);
    assert_eq!(removed.font_key, NULL_KEY);

    let mut doc = reopen(&mut doc);
    let kinds: Vec<_> = font(&doc.fonts(), "Noto Sans Symbols")
        .embedded_fonts
        .iter()
        .map(|face| face.kind)
        .collect();
    assert_eq!(kinds, [Regular]);
    let package = saved_package(&mut doc);
    assert_eq!(
        font_parts(&package),
        ["/word/fonts/NotoSansSymbols-regular.ttf"]
    );
    let targets: Vec<String> = font_relationships(&package)
        .iter()
        .map(font_part_of)
        .collect();
    assert_eq!(targets, ["/word/fonts/NotoSansSymbols-regular.ttf"]);
}

#[test]
fn a_font_embedded_into_the_google_report_is_obfuscated_beside_its_plain_faces() {
    let original = corpus_package("google/report.docx");
    let mut doc = corpus_doc("google/report.docx");
    doc.embed_font("Gelasio", face(Regular, tiny_font("Gelasio regular")))
        .unwrap();

    let mut doc = reopen(&mut doc);
    let fonts = doc.fonts();
    assert_eq!(font(&fonts, "Noto Sans Symbols").embedded_fonts.len(), 2);
    assert_eq!(
        font(&fonts, "Gelasio").embedded_fonts[0].data,
        tiny_font("Gelasio regular")
    );
    let package = saved_package(&mut doc);
    let added: Vec<String> = font_parts(&package)
        .into_iter()
        .filter(|part| original.get_part(part).is_none())
        .collect();
    assert_eq!(added.len(), 1);
    assert_eq!(
        package.content_types.content_type_for(&added[0]),
        Some(OBFUSCATED_FONT_TYPE)
    );
    assert_eq!(
        xor_head(package.get_part(&added[0]).unwrap(), spec_xor_key(KEY)),
        tiny_font("Gelasio regular")
    );
    for part in font_parts(&original) {
        assert_eq!(package.get_part(&part), original.get_part(&part), "{part}");
    }
}

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

#[test]
fn a_cjk_default_font_survives_and_a_control_character_is_refused() {
    let mut doc = Document::new();
    doc.set_default_font_name_value(Some("ＭＳ 明朝")).unwrap();
    doc.set_default_font_size_value(Some(Length::pt(10.5)))
        .unwrap();
    let before = doc.to_bytes().unwrap();
    assert!(
        doc.set_default_font_name_value(Some("Bad\u{1}Font"))
            .is_err()
    );
    assert_eq!(doc.to_bytes().unwrap(), before);
    doc.add_paragraph("東京");

    let doc = reopen(&mut doc);
    assert_eq!(doc.default_font_name(), Some("ＭＳ 明朝"));
    let paragraph = doc.paragraph(0).unwrap();
    let run = doc.effective_run_properties(&paragraph, &paragraph.run(0).unwrap());
    assert_eq!(run.font_east_asia.as_deref(), Some("ＭＳ 明朝"));
    assert_eq!(run.sz_cs.map(|size| size.to_pt()), Some(10.5));
}

// ---- Theme fonts ----

#[test]
fn the_themes_name_their_heading_and_body_fonts() {
    let scheme = |doc: &Document| {
        let fonts = &doc.theme().expect("a theme").theme_elements.font_scheme;
        let japanese = |collection: &oxoffice_drawing::theme::CT_FontCollection| {
            collection
                .supplemental_fonts
                .iter()
                .find(|font| font.script == "Jpan")
                .map(|font| font.typeface.clone())
        };
        (
            fonts.major_font.latin.typeface.clone(),
            fonts.minor_font.latin.typeface.clone(),
            japanese(&fonts.major_font),
            japanese(&fonts.minor_font),
        )
    };
    let text = |value: &str| value.to_owned();
    // The Word report's theme names Calibri for headings and Cambria for body
    // text.
    assert_eq!(
        scheme(&corpus_doc("word/report.docx")),
        (
            text("Calibri"),
            text("Cambria"),
            Some(text("ＭＳ ゴシック")),
            Some(text("ＭＳ 明朝"))
        )
    );
    // A new document takes today's Office theme.
    assert_eq!(
        scheme(&Document::new()),
        (
            text("Aptos Display"),
            text("Aptos"),
            Some(text("游ゴシック Light")),
            Some(text("游ゴシック"))
        )
    );
}

#[test]
fn a_changed_theme_font_scheme_survives_a_save() {
    let mut doc = Document::new();
    let mut theme = doc.theme().expect("a new document has a theme").clone();
    let fonts = &mut theme.theme_elements.font_scheme;
    fonts.major_font.latin.typeface = "Gelasio".to_owned();
    fonts.minor_font.latin.typeface = "Caladea".to_owned();
    fonts.minor_font.east_asian.typeface = "Yu Mincho".to_owned();
    fonts.minor_font.complex_script.typeface = "Noto Sans Arabic".to_owned();
    fonts
        .minor_font
        .supplemental_fonts
        .retain(|font| font.script != "Hang");
    let japanese = fonts
        .minor_font
        .supplemental_fonts
        .iter_mut()
        .find(|font| font.script == "Jpan")
        .expect("a Japanese body font");
    japanese.typeface = "BIZ UDMincho".to_owned();
    doc.set_theme(theme.clone()).unwrap();

    let doc = reopen(&mut doc);
    assert_eq!(doc.theme(), Some(&theme));
    let xml = doc.part_xml("word/theme/theme1.xml").unwrap();
    for written in [
        r#"<a:latin typeface="Gelasio""#,
        r#"<a:latin typeface="Caladea""#,
        r#"<a:ea typeface="Yu Mincho""#,
        r#"<a:cs typeface="Noto Sans Arabic""#,
        r#"<a:font script="Jpan" typeface="BIZ UDMincho""#,
    ] {
        assert!(xml.contains(written), "{written} in the theme part");
    }
    // The major collection keeps its Korean font: only the minor one lost it.
    assert_eq!(xml.matches(r#"script="Hang""#).count(), 1);
}

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

#[test]
fn the_corpus_theme_font_languages_read_as_their_producers_wrote_them() {
    let language = |latin: &str, east_asia: &str, bidi: Option<&str>| ThemeFontLanguage {
        latin: Some(latin.to_owned()),
        east_asia: Some(east_asia.to_owned()),
        bidi: bidi.map(str::to_owned),
    };
    assert_eq!(
        corpus_doc("word/report.docx").theme_font_language(),
        Some(&language("en-US", "ja-JP", None))
    );
    // LibreOffice writes an empty bidirectional language, kept as written.
    let mut libreoffice = corpus_doc("libreoffice/report.docx");
    assert_eq!(
        libreoffice.theme_font_language(),
        Some(&language("en-US", "ja-JP", Some("")))
    );
    assert_eq!(
        reopen(&mut libreoffice).theme_font_language(),
        Some(&language("en-US", "ja-JP", Some("")))
    );
    assert_eq!(corpus_doc("google/report.docx").theme_font_language(), None);
}
