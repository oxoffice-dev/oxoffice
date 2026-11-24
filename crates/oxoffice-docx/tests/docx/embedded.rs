//! Embedded content (OLE objects, ActiveX controls, VBA projects) and the
//! package signature around it, as the facade handles them today.
//!
//! The fixtures are built in code: a new document whose body holds an OLE
//! object and an ActiveX control, with a VBA project carrying its own
//! signature. The PowerPoint module of the pptx binary asks the same
//! questions of a deck; the specification of embedded content in opc
//! (`specs/2026-11-21-embedded-content-and-signatures-in-opc.md`) lists the
//! answers that change.

use std::io::Cursor;

use oxoffice_docx::{
    Document, EmbeddedContentKind, EmbeddedMutationPolicy, EmbeddedSignatureState, Error,
};
use oxoffice_opc::OpcPackage;
use oxoffice_opc::relationship::rel_types;

use crate::support::docx_with_body;

const O_NS: &str = "urn:schemas-microsoft-com:office:office";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const ACTIVEX_NS: &str = "http://schemas.microsoft.com/office/2006/activeX";

const MAIN: &str = "/word/document.xml";
const OLE_REL: &str = "rIdOle1";
const CONTROL_REL: &str = "rIdCtl1";
const VBA_REL: &str = "rIdVba1";
const OLE_PART: &str = "/word/embeddings/oleObject1.bin";
const CONTROL_PART: &str = "/word/activeX/activeX1.xml";
const BINARY_PART: &str = "/word/activeX/activeX1.bin";
const VBA_PART: &str = "/word/vbaProject.bin";
const VBA_SIGNATURE_PART: &str = "/word/vbaProjectSignature.bin";
const IMAGE_PART: &str = "/word/media/image 1.png";

const OLE_CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.oleObject";
const ACTIVEX_CONTENT_TYPE: &str = "application/vnd.ms-office.activeX+xml";
const ACTIVEX_BINARY_CONTENT_TYPE: &str = "application/vnd.ms-office.activeX";
const VBA_CONTENT_TYPE: &str = "application/vnd.ms-office.vbaProject";
const VBA_SIGNATURE_CONTENT_TYPE: &str = "application/vnd.ms-office.vbaProjectSignature";
const MACRO_MAIN_CONTENT_TYPE: &str = "application/vnd.ms-word.document.macroEnabled.main+xml";

const OLE_BYTES: &[u8] = b"OLE payload";
const BINARY_BYTES: &[u8] = b"ActiveX binary payload";
const VBA_BYTES: &[u8] = b"VBA project payload";
const VBA_SIGNATURE_BYTES: &[u8] = b"VBA signature payload";

const INVALIDATED_VBA_SIGNATURE: &str = "urn:rdocx:relationships/invalidated-vba-project-signature";

fn package_of(bytes: &[u8]) -> OpcPackage {
    OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens")
}

fn bytes_of(package: &OpcPackage) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    out.into_inner()
}

fn add_part(package: &mut OpcPackage, part: &str, content_type: &str, bytes: &[u8]) {
    package.set_part(part, bytes.to_vec());
    package.content_types.add_override(part, content_type);
}

fn activex_properties(declaration: &str) -> String {
    format!(
        r#"{declaration}<ax:ocx xmlns:ax="{ACTIVEX_NS}" xmlns:r="{R_NS}" ax:classid="{{8BD21D40-EC42-11CE-9E0D-00AA006002F3}}" ax:persistence="persistStorage" r:id="rId1"/>"#
    )
}

/// An OLE object paragraph and an ActiveX control paragraph in the body, a
/// VBA project with its VBA signature, in a macro-enabled document.
fn fixture_package() -> OpcPackage {
    let body = format!(
        concat!(
            r#"<w:p><w:r><w:object xmlns:o="{o}" xmlns:r="{r}">"#,
            r#"<o:OLEObject Type="Embed" ProgID="Package" ShapeID="_x0000_i1025" DrawAspect="Content" ObjectID="_1000000001" r:id="{ole}"/>"#,
            r#"</w:object></w:r></w:p>"#,
            r#"<w:p><w:r><w:object xmlns:r="{r}">"#,
            r#"<w:control r:id="{control}" w:name="CheckBox1" w:shapeid="_x0000_i1026"/>"#,
            r#"</w:object></w:r></w:p>"#,
        ),
        o = O_NS,
        r = R_NS,
        ole = OLE_REL,
        control = CONTROL_REL,
    );
    let mut package = package_of(&docx_with_body(&body));
    assert_eq!(package.main_document_part().as_deref(), Some(MAIN));
    package
        .content_types
        .add_override(MAIN, MACRO_MAIN_CONTENT_TYPE);

    add_part(&mut package, OLE_PART, OLE_CONTENT_TYPE, OLE_BYTES);
    add_part(
        &mut package,
        CONTROL_PART,
        ACTIVEX_CONTENT_TYPE,
        activex_properties(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#).as_bytes(),
    );
    add_part(
        &mut package,
        BINARY_PART,
        ACTIVEX_BINARY_CONTENT_TYPE,
        BINARY_BYTES,
    );
    add_part(&mut package, VBA_PART, VBA_CONTENT_TYPE, VBA_BYTES);
    add_part(
        &mut package,
        VBA_SIGNATURE_PART,
        VBA_SIGNATURE_CONTENT_TYPE,
        VBA_SIGNATURE_BYTES,
    );

    let main = package.get_or_create_part_rels(MAIN);
    main.add_with_id(OLE_REL, rel_types::OLE_OBJECT, "embeddings/oleObject1.bin");
    main.add_with_id(CONTROL_REL, rel_types::CONTROL, "activeX/activeX1.xml");
    main.add_with_id(VBA_REL, rel_types::VBA_PROJECT, "vbaProject.bin");
    package.get_or_create_part_rels(CONTROL_PART).add_with_id(
        "rId1",
        rel_types::ACTIVEX_CONTROL_BINARY,
        "activeX1.bin",
    );
    package.get_or_create_part_rels(VBA_PART).add_with_id(
        "rId1",
        rel_types::VBA_PROJECT_SIGNATURE,
        "vbaProjectSignature.bin",
    );
    package
}

fn open(package: &OpcPackage) -> Document {
    Document::from_bytes(&bytes_of(package)).expect("fixture opens")
}

fn fixture() -> Document {
    open(&fixture_package())
}

fn saved_package(doc: &mut Document) -> OpcPackage {
    package_of(&doc.to_bytes().expect("document saves"))
}

fn relationship_types(package: &OpcPackage, source: &str) -> Vec<String> {
    let relationships = if source == "/" {
        Some(&package.package_rels)
    } else {
        package.get_part_rels(source)
    };
    relationships
        .map(|relationships| {
            relationships
                .items
                .iter()
                .map(|relationship| relationship.rel_type.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn states(doc: &Document) -> Vec<(EmbeddedContentKind, EmbeddedSignatureState)> {
    doc.embedded_content()
        .expect("inventory")
        .into_iter()
        .map(|info| (info.kind, info.signature_state))
        .collect()
}

fn state_of(doc: &Document, kind: EmbeddedContentKind) -> EmbeddedSignatureState {
    states(doc)
        .into_iter()
        .find(|(found, _)| *found == kind)
        .map(|(_, state)| state)
        .unwrap_or_else(|| panic!("{kind:?} is listed"))
}

fn refusal<T: std::fmt::Debug>(result: Result<T, Error>) -> String {
    match result {
        Err(error @ Error::InvalidEmbeddedMutation { .. }) => error.to_string(),
        other => panic!("expected an embedded-content refusal, got {other:?}"),
    }
}

#[test]
fn the_inventory_lists_each_payload_with_its_owner_target_and_size() {
    let doc = fixture();
    let found = doc
        .embedded_content()
        .expect("inventory")
        .into_iter()
        .map(|info| {
            (
                info.kind,
                info.source_part,
                info.relationship_id,
                info.target_part,
                info.content_type,
                info.byte_len,
                info.signature_state,
            )
        })
        .collect::<Vec<_>>();
    use EmbeddedContentKind::*;
    use EmbeddedSignatureState::*;
    assert_eq!(
        found,
        [
            (
                ActiveXControl,
                CONTROL_PART.to_owned(),
                "rId1".to_owned(),
                BINARY_PART.to_owned(),
                ACTIVEX_BINARY_CONTENT_TYPE.to_owned(),
                BINARY_BYTES.len(),
                Absent,
            ),
            (
                OleObject,
                MAIN.to_owned(),
                OLE_REL.to_owned(),
                OLE_PART.to_owned(),
                OLE_CONTENT_TYPE.to_owned(),
                OLE_BYTES.len(),
                Absent,
            ),
            (
                VbaProject,
                MAIN.to_owned(),
                VBA_REL.to_owned(),
                VBA_PART.to_owned(),
                VBA_CONTENT_TYPE.to_owned(),
                VBA_BYTES.len(),
                // The VBA project carries its own signature.
                Present,
            ),
        ]
    );
}

#[test]
fn extract_returns_each_payload_byte_for_byte() {
    let doc = fixture();
    for (source, relationship, bytes) in [
        (MAIN, OLE_REL, OLE_BYTES),
        (CONTROL_PART, "rId1", BINARY_BYTES),
        (MAIN, VBA_REL, VBA_BYTES),
    ] {
        assert_eq!(
            doc.extract_embedded_content(source, relationship)
                .expect("payload extracts"),
            bytes,
            "{source} {relationship}"
        );
    }
    // A relationship that owns no payload is refused, not read.
    assert!(
        refusal(doc.extract_embedded_content(MAIN, "rIdMissing"))
            .contains("is not an owned embedded payload")
    );
}

#[test]
fn replace_writes_new_bytes_under_the_same_relationship() {
    for policy in [
        EmbeddedMutationPolicy::PreserveInvalidatedSignatures,
        EmbeddedMutationPolicy::RemoveInvalidatedSignatures,
    ] {
        let mut doc = fixture();
        let before = doc
            .embedded_content()
            .unwrap()
            .into_iter()
            .find(|info| info.kind == EmbeddedContentKind::OleObject)
            .unwrap();
        let after = doc
            .replace_embedded_content(MAIN, OLE_REL, b"new OLE payload", policy)
            .expect("OLE payload replaces");
        assert_eq!(after.relationship_id, before.relationship_id);
        assert_eq!(after.target_part, before.target_part);
        assert_eq!(after.content_type, before.content_type);
        assert_eq!(after.byte_len, b"new OLE payload".len());
        assert_ne!(after.sha256, before.sha256);
        assert_eq!(after.signature_state, EmbeddedSignatureState::Absent);
        let reopened = Document::from_bytes(&doc.to_bytes().unwrap()).unwrap();
        assert_eq!(
            reopened.extract_embedded_content(MAIN, OLE_REL).unwrap(),
            b"new OLE payload",
            "{policy:?}"
        );
    }
}

#[test]
fn replacing_a_signed_vba_project_marks_or_drops_its_vba_signature_by_policy() {
    // Preserve keeps the signature and marks it invalidated; Remove drops
    // the relationship and the signature part.
    for (policy, state, marked, signature_kept) in [
        (
            EmbeddedMutationPolicy::PreserveInvalidatedSignatures,
            EmbeddedSignatureState::Invalidated,
            true,
            true,
        ),
        (
            EmbeddedMutationPolicy::RemoveInvalidatedSignatures,
            EmbeddedSignatureState::Absent,
            false,
            false,
        ),
    ] {
        let mut doc = fixture();
        let info = doc
            .replace_embedded_content(MAIN, VBA_REL, b"new VBA payload", policy)
            .expect("VBA project replaces");
        assert_eq!(info.signature_state, state, "{policy:?}");
        let package = saved_package(&mut doc);
        let types = relationship_types(&package, VBA_PART);
        assert_eq!(
            types.iter().any(|t| t == INVALIDATED_VBA_SIGNATURE),
            marked,
            "{policy:?}: {types:?}"
        );
        assert_eq!(
            types.iter().any(|t| t == rel_types::VBA_PROJECT_SIGNATURE),
            signature_kept,
            "{policy:?}: {types:?}"
        );
        assert_eq!(
            package.contains_part(VBA_SIGNATURE_PART),
            signature_kept,
            "{policy:?}"
        );
        let reopened = Document::from_bytes(&bytes_of(&package)).unwrap();
        assert_eq!(
            state_of(&reopened, EmbeddedContentKind::VbaProject),
            state,
            "{policy:?} after a reopen"
        );
    }
}

#[test]
fn remove_takes_the_owner_markup_the_relationship_and_the_parts() {
    type Check = fn(&OpcPackage);
    let cases: [(&str, &str, EmbeddedContentKind, Check); 3] = [
        ("OLE", OLE_REL, EmbeddedContentKind::OleObject, |package| {
            assert!(!package.contains_part(OLE_PART));
            let xml = String::from_utf8_lossy(package.get_part(MAIN).unwrap()).into_owned();
            assert!(!xml.contains("OLEObject"), "{xml}");
            assert!(
                package
                    .get_part_rels(MAIN)
                    .unwrap()
                    .get_by_id(OLE_REL)
                    .is_none()
            );
        }),
        (
            "ActiveX",
            "rId1",
            EmbeddedContentKind::ActiveXControl,
            |package| {
                assert!(!package.contains_part(CONTROL_PART));
                assert!(!package.contains_part(BINARY_PART));
                let xml = String::from_utf8_lossy(package.get_part(MAIN).unwrap()).into_owned();
                assert!(!xml.contains("w:control"), "{xml}");
                assert!(
                    package
                        .get_part_rels(MAIN)
                        .unwrap()
                        .get_by_id(CONTROL_REL)
                        .is_none()
                );
            },
        ),
        ("VBA", VBA_REL, EmbeddedContentKind::VbaProject, |package| {
            assert!(!package.contains_part(VBA_PART));
            assert!(
                package
                    .get_part_rels(MAIN)
                    .unwrap()
                    .get_by_id(VBA_REL)
                    .is_none()
            );
        }),
    ];
    for (name, relationship, kind, check) in cases {
        let mut doc = fixture();
        let source = if kind == EmbeddedContentKind::ActiveXControl {
            CONTROL_PART
        } else {
            MAIN
        };
        doc.remove_embedded_content(
            source,
            relationship,
            EmbeddedMutationPolicy::RemoveInvalidatedSignatures,
        )
        .unwrap_or_else(|error| panic!("{name} removes: {error}"));
        assert!(
            states(&doc).iter().all(|(found, _)| *found != kind),
            "{name} is no longer listed"
        );
        check(&saved_package(&mut doc));
    }
}

#[test]
fn removing_a_signed_vba_project_keeps_or_drops_its_signature_part_by_policy() {
    // Preserve leaves the VBA signature part in the package as evidence.
    for (policy, signature_kept) in [
        (EmbeddedMutationPolicy::PreserveInvalidatedSignatures, true),
        (EmbeddedMutationPolicy::RemoveInvalidatedSignatures, false),
    ] {
        let mut doc = fixture();
        doc.remove_embedded_content(MAIN, VBA_REL, policy)
            .expect("VBA project removes");
        let package = saved_package(&mut doc);
        assert!(!package.contains_part(VBA_PART), "{policy:?}");
        assert_eq!(
            package.contains_part(VBA_SIGNATURE_PART),
            signature_kept,
            "{policy:?}"
        );
    }
}

/// Reachability compares canonical part identities: case does not count.
#[test]
fn removing_an_activex_control_removes_the_other_parts_its_properties_held() {
    // A picture property: the properties part's second relationship.
    let mut package = fixture_package();
    add_part(
        &mut package,
        "/word/media/image5.emf",
        "image/x-emf",
        b"not really an EMF",
    );
    package.get_or_create_part_rels(CONTROL_PART).add_with_id(
        "rId2",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image",
        "../media/image5.emf",
    );
    let mut doc = open(&package);
    doc.remove_embedded_content(
        CONTROL_PART,
        "rId1",
        EmbeddedMutationPolicy::RemoveInvalidatedSignatures,
    )
    .expect("the ActiveX control removes");
    let saved = saved_package(&mut doc);
    assert!(!saved.contains_part(CONTROL_PART));
    assert!(
        !saved.contains_part("/word/media/image5.emf"),
        "the picture goes with it"
    );
}

#[test]
fn removing_ole_content_keeps_a_part_another_relationship_names_under_another_case() {
    let mut package = fixture_package();
    package.get_or_create_part_rels(MAIN).add_with_id(
        "rIdOther",
        "http://example.com/relationships/other",
        "embeddings/OLEOBJECT1.BIN",
    );
    let mut doc = open(&package);
    doc.remove_embedded_content(
        MAIN,
        OLE_REL,
        EmbeddedMutationPolicy::RemoveInvalidatedSignatures,
    )
    .expect("OLE object removes");
    let saved = saved_package(&mut doc);
    assert!(saved.contains_part(OLE_PART));
}

#[test]
fn removing_a_vba_project_another_relationship_names_under_another_case_is_refused_as_shared() {
    let mut package = fixture_package();
    package.get_or_create_part_rels(MAIN).add_with_id(
        "rIdOther",
        "http://example.com/relationships/other",
        "VBAPROJECT.BIN",
    );
    let error = refusal(act(&package, Act::RemoveVba));
    assert!(
        error.contains("is shared by another relationship"),
        "{error}"
    );
}

/// Two controls naming one ActiveX part under another case own one control:
/// it is listed once, and removing it takes both owners and its parts.
#[test]
fn two_controls_naming_one_activex_part_under_another_case_are_one_control() {
    let mut package = fixture_package();
    let xml = String::from_utf8(package.get_part(MAIN).unwrap().to_vec()).unwrap();
    let second = format!(
        r#"<w:p><w:r><w:object xmlns:r="{R_NS}"><w:control r:id="rIdCtl2" w:name="CheckBox2" w:shapeid="_x0000_i1027"/></w:object></w:r></w:p><w:sectPr"#
    );
    package.set_part(MAIN, xml.replacen("<w:sectPr", &second, 1).into_bytes());
    package.get_or_create_part_rels(MAIN).add_with_id(
        "rIdCtl2",
        rel_types::CONTROL,
        "activeX/ACTIVEX1.XML",
    );
    let mut doc = open(&package);
    let controls = doc
        .embedded_content()
        .expect("inventory")
        .into_iter()
        .filter(|info| info.kind == EmbeddedContentKind::ActiveXControl)
        .map(|info| info.source_part)
        .collect::<Vec<_>>();
    assert_eq!(controls, [CONTROL_PART]);
    doc.remove_embedded_content(
        CONTROL_PART,
        "rId1",
        EmbeddedMutationPolicy::RemoveInvalidatedSignatures,
    )
    .expect("the control removes");
    assert!(
        states(&doc)
            .iter()
            .all(|(kind, _)| *kind != EmbeddedContentKind::ActiveXControl),
        "{:?}",
        states(&doc)
    );
    let saved = saved_package(&mut doc);
    assert!(!saved.contains_part(CONTROL_PART));
    assert!(!saved.contains_part(BINARY_PART));
    let main = String::from_utf8(saved.get_part(MAIN).unwrap().to_vec()).unwrap();
    assert!(!main.contains("w:control"), "{main}");
}

/// The VBA project's own relationship is recognised whatever the case of
/// the relationship part that holds it.
#[test]
fn removing_a_vba_project_held_by_a_main_part_relationship_set_under_another_case() {
    let mut package = fixture_package();
    let relationships = package.remove_part_rels(MAIN).unwrap();
    package.set_part_rels("/word/DOCUMENT.xml", relationships);
    let reopened = package_of(&bytes_of(&package));
    assert!(reopened.part_rels.contains_key("/word/DOCUMENT.xml"));
    act(&reopened, Act::RemoveVba).expect("the VBA project removes");
}

/// Each refusal both facades make, as Word answers it.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(feature = "digital-signatures"), allow(dead_code))]
enum Act {
    Inventory,
    SaveAfterEdit,
    RemoveOle,
    RemoveVba,
}

fn act(package: &OpcPackage, act: Act) -> Result<(), Error> {
    let mut doc = Document::from_bytes(&bytes_of(package))?;
    let policy = EmbeddedMutationPolicy::PreserveInvalidatedSignatures;
    match act {
        Act::Inventory => doc.embedded_content().map(drop),
        Act::SaveAfterEdit => {
            doc.add_paragraph("edited");
            doc.to_bytes().map(drop)
        }
        Act::RemoveOle => doc.remove_embedded_content(MAIN, OLE_REL, policy),
        Act::RemoveVba => doc.remove_embedded_content(MAIN, VBA_REL, policy),
    }
}

fn relationship_mut<'a>(
    package: &'a mut OpcPackage,
    source: &str,
    id: &str,
) -> &'a mut oxoffice_opc::Relationship {
    package
        .get_part_rels_mut(source)
        .unwrap()
        .items
        .iter_mut()
        .find(|relationship| relationship.id == id)
        .unwrap()
}

/// An image whose relationship target holds a raw space: not a normalised
/// pack URI, and unrelated to embedded content.
fn add_image_with_a_space(package: &mut OpcPackage) {
    add_part(package, IMAGE_PART, "image/png", b"not really a PNG");
    package.get_or_create_part_rels(MAIN).add_with_id(
        "rIdImage",
        rel_types::IMAGE,
        "media/image 1.png",
    );
}

type Edit = fn(&mut OpcPackage);

/// Cases that need no signature: (name, edit, act, Word's refusal).
const UNSIGNED_REFUSALS: [(&str, Edit, Act, &str); 9] = [
    (
        "OLE relationship with a dot segment",
        |package| {
            relationship_mut(package, MAIN, OLE_REL).target = "./embeddings/oleObject1.bin".into()
        },
        Act::Inventory,
        "has an unsafe internal target ./embeddings/oleObject1.bin",
    ),
    (
        "VBA relationship with a dot segment",
        |package| relationship_mut(package, MAIN, VBA_REL).target = "./vbaProject.bin".into(),
        Act::Inventory,
        "has an unsafe internal target ./vbaProject.bin",
    ),
    (
        "OLE relationship with TargetMode internal in lowercase",
        |package| relationship_mut(package, MAIN, OLE_REL).target_mode = Some("internal".into()),
        Act::Inventory,
        "relationship rIdOle1 is external",
    ),
    (
        "VBA signature part with another content type",
        |package| {
            package
                .content_types
                .add_override(VBA_SIGNATURE_PART, "application/octet-stream")
        },
        Act::Inventory,
        "VBA signature has content type application/octet-stream",
    ),
    (
        "ActiveX properties with another content type",
        |package| {
            package
                .content_types
                .add_override(CONTROL_PART, "application/xml")
        },
        Act::Inventory,
        "ActiveX properties has content type application/xml",
    ),
    (
        "ActiveX properties declared XML 1.1",
        |package| {
            package.set_part(
                CONTROL_PART,
                activex_properties(r#"<?xml version="1.1" encoding="UTF-8"?>"#).into_bytes(),
            )
        },
        Act::Inventory,
        "XML declaration version must be 1.0",
    ),
    (
        "ActiveX properties declared in another encoding",
        |package| {
            package.set_part(
                CONTROL_PART,
                activex_properties(r#"<?xml version="1.0" encoding="ISO-8859-1"?>"#).into_bytes(),
            )
        },
        Act::Inventory,
        "XML declaration attributes are invalid",
    ),
    (
        "ActiveX properties with two binaries",
        |package| {
            add_part(
                package,
                "/word/activeX/activeX2.bin",
                ACTIVEX_BINARY_CONTENT_TYPE,
                b"two",
            );
            package.get_or_create_part_rels(CONTROL_PART).add_with_id(
                "rId2",
                rel_types::ACTIVEX_CONTROL_BINARY,
                "activeX2.bin",
            );
        },
        Act::Inventory,
        "found 2 ActiveX binary relationships, expected exactly one",
    ),
    (
        "two VBA projects",
        |package| {
            add_part(package, "/word/vbaProject2.bin", VBA_CONTENT_TYPE, b"two");
            package.get_or_create_part_rels(MAIN).add_with_id(
                "rIdVba2",
                rel_types::VBA_PROJECT,
                "vbaProject2.bin",
            );
        },
        Act::Inventory,
        "found 2 VBA project relationships, expected at most one",
    ),
];

#[test]
fn word_refuses_what_powerpoint_refuses_without_a_signature() {
    for (name, edit, operation, message) in UNSIGNED_REFUSALS {
        let mut package = fixture_package();
        edit(&mut package);
        let error = refusal(act(&package, operation));
        assert!(error.contains(message), "{name}: {error}");
    }
}

/// Only a target that reaches a protected part, or names no part, is
/// refused: an odd target to an ordinary part is not.
#[test]
fn removing_content_with_an_unrelated_image_target_holding_a_space_is_accepted() {
    for operation in [Act::RemoveOle, Act::RemoveVba] {
        let mut package = fixture_package();
        add_image_with_a_space(&mut package);
        act(&package, operation).unwrap_or_else(|error| panic!("{operation:?}: {error}"));
    }
}

/// Cases where a relationship unrelated to embedded content is refused:
/// (name, source part, target). The last two were accepted before the
/// amendment of 2026-11-24 as normalised pack URIs; once decoded they name
/// no part.
const UNRELATED_TARGET_REFUSALS: [(&str, &str, &str); 6] = [
    (
        "an escaped alias of the VBA project",
        MAIN,
        "vbaProject%2Ebin",
    ),
    (
        "a backslash reading of the VBA project",
        MAIN,
        "..\\word\\vbaProject.bin",
    ),
    ("an escaped separator", MAIN, "media%2Fimage.png"),
    (
        "a climb above the package root",
        MAIN,
        "../../media/image.png",
    ),
    (
        "an escape decoding to invalid UTF-8",
        MAIN,
        "media/caf%E9.png",
    ),
    (
        "an escape decoding to a control character",
        MAIN,
        "media/image%C2%85.png",
    ),
];

#[test]
fn removing_content_refuses_a_target_that_names_no_part_or_a_protected_part_oddly() {
    for (name, source, target) in UNRELATED_TARGET_REFUSALS {
        let mut package = fixture_package();
        package.get_or_create_part_rels(source).add_with_id(
            "rIdOdd",
            "http://example.com/relationships/other",
            target,
        );
        let error = refusal(act(&package, Act::RemoveOle));
        assert!(
            error.contains("relationship rIdOdd has an unsafe internal target"),
            "{name}: {error}"
        );
    }
}

/// Two stored parts that name one part once percent-decoded open as they
/// did, but an embedded mutation refuses them as ambiguous.
#[test]
fn removing_content_refuses_two_parts_that_name_one_part_once_decoded() {
    let mut package = fixture_package();
    add_part(&mut package, "/word/media/a b.png", "image/png", b"one");
    add_part(&mut package, "/word/media/a%20b.png", "image/png", b"two");
    let mut doc = open(&package);
    doc.embedded_content().expect("an unsigned inventory");
    let error = refusal(doc.remove_embedded_content(
        MAIN,
        OLE_REL,
        EmbeddedMutationPolicy::RemoveInvalidatedSignatures,
    ));
    assert!(error.contains("ambiguous part names"), "{error}");
}

#[test]
fn removing_a_vba_project_whose_target_another_relationship_shares_is_refused() {
    let mut package = fixture_package();
    package.get_or_create_part_rels(MAIN).add_with_id(
        "rIdOther",
        "http://example.com/relationships/other",
        "vbaProject.bin",
    );
    let error = refusal(act(&package, Act::RemoveVba));
    assert!(
        error.contains("is shared by another relationship"),
        "{error}"
    );
}

#[test]
fn two_owners_claiming_overlapping_markup_are_refused() {
    // A picture owning an ActiveX control holds a text box whose paragraph
    // owns the OLE object: both owners claim the OLE object's markup.
    let body = format!(
        concat!(
            r#"<w:p><w:r><w:pict xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="{o}" xmlns:r="{r}">"#,
            r#"<v:shape id="tb1"><v:textbox><w:txbxContent><w:p><w:r><w:object>"#,
            r#"<o:OLEObject Type="Embed" ProgID="Package" ShapeID="_x0000_i1025" DrawAspect="Content" ObjectID="_1000000001" r:id="{ole}"/>"#,
            r#"</w:object></w:r></w:p></w:txbxContent></v:textbox></v:shape>"#,
            r#"<w:control r:id="{control}" w:name="CheckBox1" w:shapeid="_x0000_i1026"/>"#,
            r#"</w:pict></w:r></w:p>"#,
        ),
        o = O_NS,
        r = R_NS,
        ole = OLE_REL,
        control = CONTROL_REL,
    );
    let mut package = fixture_package();
    let xml = String::from_utf8(package.get_part(MAIN).unwrap().to_vec()).unwrap();
    let start = xml.find("<w:body>").unwrap() + "<w:body>".len();
    let end = xml.find("<w:sectPr").unwrap();
    package.set_part(
        MAIN,
        format!("{}{body}{}", &xml[..start], &xml[end..]).into_bytes(),
    );
    let error = refusal(act(&package, Act::Inventory));
    assert!(error.contains("claim overlapping owner XML"), "{error}");
}

#[cfg(feature = "digital-signatures")]
mod signed {
    use super::*;

    const INVALIDATED_PACKAGE_SIGNATURE: &str =
        "urn:rdocx:relationships/invalidated-package-signature";

    fn marks_package_signature_invalidated(package: &OpcPackage) -> bool {
        relationship_types(package, "/")
            .iter()
            .any(|rel_type| rel_type == INVALIDATED_PACKAGE_SIGNATURE)
    }

    const KEY: &[u8] = include_bytes!("../../../../tests/signing/test-signing-key.pem");
    const CERTIFICATE: &[u8] = include_bytes!("../../../../tests/signing/test-signing-cert.pem");
    const ORIGIN_PART: &str = "/_xmlsignatures/origin.sigs";
    const SIGNATURE_PART: &str = "/_xmlsignatures/sig1.xml";

    fn sign(package: &mut OpcPackage) {
        package.sign(KEY, CERTIFICATE).expect("package signs");
        assert!(package.contains_part(ORIGIN_PART));
        assert!(package.contains_part(SIGNATURE_PART));
    }

    fn signed_fixture_package() -> OpcPackage {
        let mut package = fixture_package();
        sign(&mut package);
        package
    }

    #[test]
    fn a_signed_document_lists_every_payload_as_signed() {
        let doc = open(&signed_fixture_package());
        assert!(
            doc.verify_signatures()
                .unwrap()
                .iter()
                .all(|report| report.cryptographically_valid && report.coverage_complete)
        );
        assert!(
            states(&doc)
                .iter()
                .all(|(_, state)| *state == EmbeddedSignatureState::Present),
            "{:?}",
            states(&doc)
        );
    }

    #[test]
    fn a_signed_document_saved_without_an_edit_keeps_a_valid_signature() {
        let mut doc = open(&signed_fixture_package());
        let saved = saved_package(&mut doc);
        assert!(!marks_package_signature_invalidated(&saved));
        let reports = saved.verify_signatures().unwrap();
        assert!(
            !reports.is_empty()
                && reports
                    .iter()
                    .all(|report| report.cryptographically_valid && report.coverage_complete)
        );
    }

    #[test]
    fn a_signed_document_saved_after_a_text_edit_marks_its_signature_invalidated() {
        let mut doc = open(&signed_fixture_package());
        doc.add_paragraph("edited");
        let saved = saved_package(&mut doc);
        assert!(marks_package_signature_invalidated(&saved));
        assert!(saved.contains_part(ORIGIN_PART));
        assert!(saved.contains_part(SIGNATURE_PART));
        let reopened = Document::from_bytes(&bytes_of(&saved)).unwrap();
        assert_eq!(
            state_of(&reopened, EmbeddedContentKind::OleObject),
            EmbeddedSignatureState::Invalidated
        );
    }

    #[test]
    fn replacing_content_in_a_signed_document_marks_or_drops_the_signature_by_policy() {
        for (policy, state, signature_kept) in [
            (
                EmbeddedMutationPolicy::PreserveInvalidatedSignatures,
                EmbeddedSignatureState::Invalidated,
                true,
            ),
            (
                EmbeddedMutationPolicy::RemoveInvalidatedSignatures,
                EmbeddedSignatureState::Absent,
                false,
            ),
        ] {
            let mut doc = open(&signed_fixture_package());
            let info = doc
                .replace_embedded_content(MAIN, OLE_REL, b"new OLE payload", policy)
                .expect("OLE payload replaces");
            assert_eq!(info.signature_state, state, "{policy:?}");
            let saved = saved_package(&mut doc);
            assert_eq!(
                marks_package_signature_invalidated(&saved),
                signature_kept,
                "{policy:?}"
            );
            assert_eq!(
                saved.contains_part(ORIGIN_PART),
                signature_kept,
                "{policy:?}"
            );
            assert_eq!(
                saved.contains_part(SIGNATURE_PART),
                signature_kept,
                "{policy:?}"
            );
            let reopened = Document::from_bytes(&bytes_of(&saved)).unwrap();
            assert_eq!(
                state_of(&reopened, EmbeddedContentKind::OleObject),
                state,
                "{policy:?} after a reopen"
            );
        }
    }

    #[test]
    fn removing_content_from_a_signed_document_marks_or_drops_the_signature_by_policy() {
        for (policy, signature_kept) in [
            (EmbeddedMutationPolicy::PreserveInvalidatedSignatures, true),
            (EmbeddedMutationPolicy::RemoveInvalidatedSignatures, false),
        ] {
            let mut doc = open(&signed_fixture_package());
            doc.remove_embedded_content(MAIN, OLE_REL, policy)
                .expect("OLE object removes");
            let saved = saved_package(&mut doc);
            assert!(!saved.contains_part(OLE_PART), "{policy:?}");
            assert_eq!(
                marks_package_signature_invalidated(&saved),
                signature_kept,
                "{policy:?}"
            );
            assert_eq!(
                saved.contains_part(ORIGIN_PART),
                signature_kept,
                "{policy:?}"
            );
        }
    }

    /// The image target reaches an ordinary part: the edited document saves,
    /// its signature marked invalidated, whether the image came before or
    /// after the signature.
    #[test]
    fn a_signed_document_saves_with_an_image_target_holding_a_space() {
        for image_before_signing in [true, false] {
            let mut package = fixture_package();
            if image_before_signing {
                add_image_with_a_space(&mut package);
                sign(&mut package);
            } else {
                sign(&mut package);
                add_image_with_a_space(&mut package);
            }
            let mut doc = open(&package);
            doc.add_paragraph("edited");
            let saved = saved_package(&mut doc);
            assert!(
                marks_package_signature_invalidated(&saved),
                "image before signing: {image_before_signing}"
            );
            assert!(saved.contains_part(IMAGE_PART));
        }
    }

    #[test]
    fn removing_content_from_a_signed_document_with_an_image_target_holding_a_space_is_accepted() {
        let mut package = fixture_package();
        add_image_with_a_space(&mut package);
        sign(&mut package);
        act(&package, Act::RemoveOle).expect("the OLE object removes");
    }

    /// Aliases of the origin through an escape or another reading, besides
    /// case: (name, target).
    const ORIGIN_ALIASES: [(&str, &str); 3] = [
        ("an escaped underscore", "../%5Fxmlsignatures/origin.sigs"),
        ("a backslash", "..\\_xmlsignatures\\origin.sigs"),
        ("a query", "../_xmlsignatures/origin.sigs?x=1"),
    ];

    #[test]
    fn a_signed_document_does_not_save_with_an_unrelated_relationship_naming_the_origin_by_an_alias()
     {
        for (name, target) in ORIGIN_ALIASES {
            let mut package = signed_fixture_package();
            package.get_or_create_part_rels(MAIN).add_with_id(
                "rIdOrigin",
                rel_types::IMAGE,
                target,
            );
            let error = refusal(act(&package, Act::SaveAfterEdit));
            assert!(
                error.contains("relationship rIdOrigin targets package signature part")
                    || error.contains("relationship rIdOrigin has an unsafe internal target"),
                "{name}: {error}"
            );
        }
    }

    #[test]
    fn a_signed_document_does_not_save_with_an_unrelated_relationship_naming_the_origin_under_another_case()
     {
        let mut package = signed_fixture_package();
        package.get_or_create_part_rels(MAIN).add_with_id(
            "rIdOrigin",
            rel_types::IMAGE,
            "../_XMLSIGNATURES/ORIGIN.SIGS",
        );
        let error = refusal(act(&package, Act::SaveAfterEdit));
        assert!(
            error.contains(
                "unrelated relationship rIdOrigin targets package signature part /_XMLSIGNATURES/ORIGIN.SIGS"
            ),
            "{error}"
        );
    }

    /// Two stored parts that name one part once percent-decoded: a target
    /// naming one names the other, so a signed save refuses them; an
    /// unsigned one saves.
    #[test]
    fn a_signed_document_does_not_save_with_two_parts_that_name_one_part_once_decoded() {
        for signed in [false, true] {
            let mut package = fixture_package();
            if signed {
                sign(&mut package);
            }
            add_part(&mut package, "/word/media/a b.png", "image/png", b"one");
            add_part(&mut package, "/word/media/a%20b.png", "image/png", b"two");
            let result = act(&package, Act::SaveAfterEdit);
            if signed {
                let error = refusal(result);
                assert!(error.contains("ambiguous part names"), "{error}");
            } else {
                result.expect("an unsigned document saves");
            }
        }
    }

    /// An unrelated relationship whose target names no part, among them the
    /// escapes the amendment of 2026-11-24 refuses: (name, target, reason).
    const SIGNED_TARGETS_NAMING_NO_PART: [(&str, &str, &str); 3] = [
        (
            "an escaped separator",
            "media%2Fimage.png",
            "it encodes a path separator",
        ),
        (
            "an escape decoding to invalid UTF-8",
            "media/caf%E9.png",
            "it decodes to invalid UTF-8",
        ),
        (
            "an escape decoding to a control character",
            "media/image%C2%85.png",
            "it holds a control character",
        ),
    ];

    #[test]
    fn a_signed_document_does_not_save_with_an_unrelated_target_that_names_no_part() {
        for (name, target, reason) in SIGNED_TARGETS_NAMING_NO_PART {
            let mut package = signed_fixture_package();
            package
                .get_or_create_part_rels(MAIN)
                .add_with_id("rIdOdd", rel_types::IMAGE, target);
            let error = refusal(act(&package, Act::SaveAfterEdit));
            assert!(
                error.contains(&format!(
                    "relationship rIdOdd has an unsafe internal target {target}: {reason}"
                )),
                "{name}: {error}"
            );
        }
    }

    /// Cases on a signed package, edited after signing: (name, edit, act,
    /// Word's refusal).
    const SIGNED_REFUSALS: [(&str, Edit, Act, &str); 6] = [
        (
            "origin part with another content type",
            |package| {
                package
                    .content_types
                    .add_override(ORIGIN_PART, "application/octet-stream")
            },
            Act::SaveAfterEdit,
            "package signature origin has content type application/octet-stream",
        ),
        (
            "signature part with another content type",
            |package| {
                package
                    .content_types
                    .add_override(SIGNATURE_PART, "application/xml")
            },
            Act::SaveAfterEdit,
            "package XML signature has content type application/xml",
        ),
        (
            "another relationship to the origin",
            |package| {
                package.get_or_create_part_rels(MAIN).add_with_id(
                    "rIdOrigin",
                    "http://example.com/relationships/other",
                    "../_xmlsignatures/origin.sigs",
                );
            },
            Act::SaveAfterEdit,
            "unrelated relationship rIdOrigin targets package signature part",
        ),
        (
            "a relationship to the signature part from outside the origin",
            |package| {
                package.get_or_create_part_rels(MAIN).add_with_id(
                    "rIdSignature",
                    "http://example.com/relationships/other",
                    "../_xmlsignatures/sig1.xml",
                );
            },
            Act::SaveAfterEdit,
            "unrelated relationship rIdSignature targets package signature part",
        ),
        (
            "origin relationship with a dot segment",
            |package| {
                let origin = package
                    .package_rels
                    .items
                    .iter_mut()
                    .find(|relationship| {
                        relationship.rel_type == rel_types::DIGITAL_SIGNATURE_ORIGIN
                    })
                    .unwrap();
                origin.target = "./_xmlsignatures/origin.sigs".into();
            },
            Act::SaveAfterEdit,
            "has an unsafe internal target ./_xmlsignatures/origin.sigs",
        ),
        (
            "signature relationship with a dot segment",
            |package| {
                package.get_part_rels_mut(ORIGIN_PART).unwrap().items[0].target =
                    "./sig1.xml".into();
            },
            Act::SaveAfterEdit,
            "has an unsafe internal target ./sig1.xml",
        ),
    ];

    #[test]
    fn word_refuses_what_powerpoint_refuses_on_a_signed_package() {
        for (name, edit, operation, message) in SIGNED_REFUSALS {
            let mut package = signed_fixture_package();
            edit(&mut package);
            let error = refusal(act(&package, operation));
            assert!(error.contains(message), "{name}: {error}");
        }
    }

    /// The signed-corpus run of the specification's Risks section: every
    /// document of the corpus, signed with the test key, gets one paragraph
    /// and is saved, and the count of saves and refusals by producer is
    /// printed. Slow: run it by hand, with `--ignored --nocapture`.
    #[test]
    #[ignore = "a run over the whole corpus, by hand"]
    fn every_corpus_document_signed_saves_after_one_edit() {
        use std::collections::BTreeMap;

        let mut counts = BTreeMap::<String, (usize, usize, usize)>::new();
        let mut refused = Vec::new();
        let mut unsigned = Vec::new();
        for file in crate::support::corpus_files("docx") {
            let producer = file.path.split('/').next().unwrap_or_default().to_owned();
            let mut package =
                OpcPackage::from_reader(Cursor::new(file.bytes())).expect("the document opens");
            let entry = counts.entry(producer).or_default();
            if let Err(error) = package.sign(KEY, CERTIFICATE) {
                entry.2 += 1;
                unsigned.push(format!("{}: {error}", file.path));
                continue;
            }
            let saved = Document::from_bytes(&bytes_of(&package)).and_then(|mut doc| {
                doc.add_paragraph("edited after signing");
                doc.to_bytes()
            });
            match saved {
                Ok(_) => entry.0 += 1,
                Err(error) => {
                    entry.1 += 1;
                    refused.push(format!("{}: {error}", file.path));
                }
            }
        }
        for (producer, (saves, refusals, not_signed)) in &counts {
            println!("{producer}: {saves} saved, {refusals} refused, {not_signed} not signed");
        }
        println!("not signed: {unsigned:#?}");
        assert!(refused.is_empty(), "refused: {refused:#?}");
    }
}
