//! Embedded content (OLE objects, ActiveX controls, VBA projects) and the
//! package signature around it.
//!
//! The fixtures are built in code: a new deck whose slide holds an OLE
//! object and an ActiveX control, with a VBA project carrying its own
//! signature. The docx binary's `embedded` module asks the same questions of
//! a Word document: both facades run the same checks, from
//! `oxoffice_opc::embedded`
//! (`specs/2026-11-21-embedded-content-and-signatures-in-opc.md`).

use std::io::Cursor;

use oxoffice_opc::OpcPackage;
use oxoffice_opc::relationship::rel_types;
use oxoffice_pptx::{
    EmbeddedContentKind, EmbeddedMutationPolicy, EmbeddedSignatureState, Error, Presentation,
};

const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const ACTIVEX_NS: &str = "http://schemas.microsoft.com/office/2006/activeX";
const OLE_URI: &str = "http://schemas.openxmlformats.org/presentationml/2006/ole";

const PRESENTATION: &str = "/ppt/presentation.xml";
const SLIDE: &str = "/ppt/slides/slide1.xml";
const OLE_REL: &str = "rIdOle1";
const CONTROL_REL: &str = "rIdCtl1";
const VBA_REL: &str = "rIdVba1";
const OLE_PART: &str = "/ppt/embeddings/oleObject1.bin";
const CONTROL_PART: &str = "/ppt/activeX/activeX1.xml";
const BINARY_PART: &str = "/ppt/activeX/activeX1.bin";
const VBA_PART: &str = "/ppt/vbaProject.bin";
const VBA_SIGNATURE_PART: &str = "/ppt/vbaProjectSignature.bin";
const IMAGE_PART: &str = "/ppt/media/image 1.png";

const OLE_CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.oleObject";
const ACTIVEX_CONTENT_TYPE: &str = "application/vnd.ms-office.activeX+xml";
const ACTIVEX_BINARY_CONTENT_TYPE: &str = "application/vnd.ms-office.activeX";
const VBA_CONTENT_TYPE: &str = "application/vnd.ms-office.vbaProject";
const VBA_SIGNATURE_CONTENT_TYPE: &str = "application/vnd.ms-office.vbaProjectSignature";
const MACRO_MAIN_CONTENT_TYPE: &str =
    "application/vnd.ms-powerpoint.presentation.macroEnabled.main+xml";

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

fn blank_layout(prs: &Presentation) -> usize {
    (0..prs.layout_count())
        .find(|&index| prs.layout_name(index) == Some("Blank"))
        .expect("the default template has a Blank layout")
}

/// A slide holding an OLE object frame and an ActiveX control, a VBA project
/// with its VBA signature, in a macro-enabled deck.
fn fixture_package() -> OpcPackage {
    let mut prs = Presentation::new().expect("the default template opens");
    prs.add_slide(blank_layout(&prs)).expect("slide adds");
    let mut package = package_of(&prs.to_bytes().expect("deck saves"));
    assert!(package.contains_part(SLIDE));

    let frame = format!(
        concat!(
            r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="10" name="Object 1"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>"#,
            r#"<p:xfrm><a:off xmlns:a="{a}" x="0" y="0"/><a:ext xmlns:a="{a}" cx="914400" cy="914400"/></p:xfrm>"#,
            r#"<a:graphic xmlns:a="{a}"><a:graphicData uri="{uri}">"#,
            r#"<p:oleObj xmlns:r="{r}" r:id="{ole}" name="Packager Shell Object" progId="Package"><p:embed/></p:oleObj>"#,
            r#"</a:graphicData></a:graphic></p:graphicFrame>"#,
        ),
        a = A_NS,
        uri = OLE_URI,
        r = R_NS,
        ole = OLE_REL,
    );
    let controls = format!(
        r#"<p:controls><p:control xmlns:r="{R_NS}" r:id="{CONTROL_REL}" name="CheckBox1" spid="_x0000_s1025"/></p:controls>"#
    );
    let xml = String::from_utf8(package.get_part(SLIDE).unwrap().to_vec()).unwrap();
    let tree_end = xml.find("</p:spTree>").expect("slide shape tree");
    let after_tree = tree_end + "</p:spTree>".len();
    let xml = format!(
        "{}{frame}</p:spTree>{controls}{}",
        &xml[..tree_end],
        &xml[after_tree..]
    );
    package.set_part(SLIDE, xml.into_bytes());
    package
        .content_types
        .add_override(PRESENTATION, MACRO_MAIN_CONTENT_TYPE);

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

    let slide = package.get_or_create_part_rels(SLIDE);
    slide.add_with_id(
        OLE_REL,
        rel_types::OLE_OBJECT,
        "../embeddings/oleObject1.bin",
    );
    slide.add_with_id(CONTROL_REL, rel_types::CONTROL, "../activeX/activeX1.xml");
    package.get_or_create_part_rels(PRESENTATION).add_with_id(
        VBA_REL,
        rel_types::VBA_PROJECT,
        "vbaProject.bin",
    );
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

fn open(package: &OpcPackage) -> Presentation {
    Presentation::from_bytes(&bytes_of(package)).expect("fixture opens")
}

fn fixture() -> Presentation {
    open(&fixture_package())
}

fn saved_package(prs: &Presentation) -> OpcPackage {
    package_of(&prs.to_bytes().expect("deck saves"))
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

fn states(prs: &Presentation) -> Vec<(EmbeddedContentKind, EmbeddedSignatureState)> {
    prs.embedded_content()
        .expect("inventory")
        .into_iter()
        .map(|info| (info.kind, info.signature_state))
        .collect()
}

fn state_of(prs: &Presentation, kind: EmbeddedContentKind) -> EmbeddedSignatureState {
    states(prs)
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
    let prs = fixture();
    let found = prs
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
                VbaProject,
                PRESENTATION.to_owned(),
                VBA_REL.to_owned(),
                VBA_PART.to_owned(),
                VBA_CONTENT_TYPE.to_owned(),
                VBA_BYTES.len(),
                // The VBA project carries its own signature.
                Present,
            ),
            (
                OleObject,
                SLIDE.to_owned(),
                OLE_REL.to_owned(),
                OLE_PART.to_owned(),
                OLE_CONTENT_TYPE.to_owned(),
                OLE_BYTES.len(),
                Absent,
            ),
        ]
    );
}

#[test]
fn extract_returns_each_payload_byte_for_byte() {
    let prs = fixture();
    for (source, relationship, bytes) in [
        (SLIDE, OLE_REL, OLE_BYTES),
        (CONTROL_PART, "rId1", BINARY_BYTES),
        (PRESENTATION, VBA_REL, VBA_BYTES),
    ] {
        assert_eq!(
            prs.extract_embedded_content(source, relationship)
                .expect("payload extracts"),
            bytes,
            "{source} {relationship}"
        );
    }
    // A relationship that owns no payload is refused, not read.
    assert!(
        refusal(prs.extract_embedded_content(SLIDE, "rIdMissing"))
            .contains("is not an owned embedded payload")
    );
}

#[test]
fn replace_writes_new_bytes_under_the_same_relationship() {
    for policy in [
        EmbeddedMutationPolicy::PreserveInvalidatedSignatures,
        EmbeddedMutationPolicy::RemoveInvalidatedSignatures,
    ] {
        let mut prs = fixture();
        let before = prs
            .embedded_content()
            .unwrap()
            .into_iter()
            .find(|info| info.kind == EmbeddedContentKind::OleObject)
            .unwrap();
        let after = prs
            .replace_embedded_content(SLIDE, OLE_REL, b"new OLE payload", policy)
            .expect("OLE payload replaces");
        assert_eq!(after.relationship_id, before.relationship_id);
        assert_eq!(after.target_part, before.target_part);
        assert_eq!(after.content_type, before.content_type);
        assert_eq!(after.byte_len, b"new OLE payload".len());
        assert_ne!(after.sha256, before.sha256);
        assert_eq!(after.signature_state, EmbeddedSignatureState::Absent);
        let reopened = Presentation::from_bytes(&prs.to_bytes().unwrap()).unwrap();
        assert_eq!(
            reopened.extract_embedded_content(SLIDE, OLE_REL).unwrap(),
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
        let mut prs = fixture();
        let info = prs
            .replace_embedded_content(PRESENTATION, VBA_REL, b"new VBA payload", policy)
            .expect("VBA project replaces");
        assert_eq!(info.signature_state, state, "{policy:?}");
        let package = saved_package(&prs);
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
        let reopened = Presentation::from_bytes(&bytes_of(&package)).unwrap();
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
    let cases: [(&str, &str, &str, EmbeddedContentKind, Check); 3] = [
        (
            "OLE",
            SLIDE,
            OLE_REL,
            EmbeddedContentKind::OleObject,
            |package| {
                assert!(!package.contains_part(OLE_PART));
                let xml = String::from_utf8_lossy(package.get_part(SLIDE).unwrap()).into_owned();
                assert!(!xml.contains("oleObj"), "{xml}");
                assert!(
                    package
                        .get_part_rels(SLIDE)
                        .unwrap()
                        .get_by_id(OLE_REL)
                        .is_none()
                );
            },
        ),
        (
            "ActiveX",
            CONTROL_PART,
            "rId1",
            EmbeddedContentKind::ActiveXControl,
            |package| {
                assert!(!package.contains_part(CONTROL_PART));
                assert!(!package.contains_part(BINARY_PART));
                let xml = String::from_utf8_lossy(package.get_part(SLIDE).unwrap()).into_owned();
                assert!(!xml.contains("p:control "), "{xml}");
                assert!(
                    package
                        .get_part_rels(SLIDE)
                        .unwrap()
                        .get_by_id(CONTROL_REL)
                        .is_none()
                );
            },
        ),
        (
            "VBA",
            PRESENTATION,
            VBA_REL,
            EmbeddedContentKind::VbaProject,
            |package| {
                assert!(!package.contains_part(VBA_PART));
                assert!(
                    package
                        .get_part_rels(PRESENTATION)
                        .unwrap()
                        .get_by_id(VBA_REL)
                        .is_none()
                );
            },
        ),
    ];
    for (name, source, relationship, kind, check) in cases {
        let mut prs = fixture();
        prs.remove_embedded_content(
            source,
            relationship,
            EmbeddedMutationPolicy::RemoveInvalidatedSignatures,
        )
        .unwrap_or_else(|error| panic!("{name} removes: {error}"));
        assert!(
            states(&prs).iter().all(|(found, _)| *found != kind),
            "{name} is no longer listed"
        );
        check(&saved_package(&prs));
    }
}

#[test]
fn removing_a_signed_vba_project_keeps_or_drops_its_signature_part_by_policy() {
    // Preserve leaves the VBA signature part in the package as evidence.
    for (policy, signature_kept) in [
        (EmbeddedMutationPolicy::PreserveInvalidatedSignatures, true),
        (EmbeddedMutationPolicy::RemoveInvalidatedSignatures, false),
    ] {
        let mut prs = fixture();
        prs.remove_embedded_content(PRESENTATION, VBA_REL, policy)
            .expect("VBA project removes");
        let package = saved_package(&prs);
        assert!(!package.contains_part(VBA_PART), "{policy:?}");
        assert_eq!(
            package.contains_part(VBA_SIGNATURE_PART),
            signature_kept,
            "{policy:?}"
        );
    }
}

/// Reachability compares part names without case, as OPC requires.
#[test]
fn removing_an_activex_control_removes_the_other_parts_its_properties_held() {
    // A picture property: the properties part's second relationship.
    let mut package = fixture_package();
    add_part(
        &mut package,
        "/ppt/media/image5.emf",
        "image/x-emf",
        b"not really an EMF",
    );
    package.get_or_create_part_rels(CONTROL_PART).add_with_id(
        "rId2",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image",
        "../media/image5.emf",
    );
    let mut prs = open(&package);
    prs.remove_embedded_content(
        CONTROL_PART,
        "rId1",
        EmbeddedMutationPolicy::RemoveInvalidatedSignatures,
    )
    .expect("the ActiveX control removes");
    let saved = saved_package(&prs);
    assert!(!saved.contains_part(CONTROL_PART));
    assert!(
        !saved.contains_part("/ppt/media/image5.emf"),
        "the picture goes with it"
    );
}

#[test]
fn removing_ole_content_keeps_a_part_another_relationship_names_under_another_case() {
    let mut package = fixture_package();
    package.get_or_create_part_rels(SLIDE).add_with_id(
        "rIdOther",
        "http://example.com/relationships/other",
        "../embeddings/OLEOBJECT1.BIN",
    );
    let mut prs = open(&package);
    prs.remove_embedded_content(
        SLIDE,
        OLE_REL,
        EmbeddedMutationPolicy::RemoveInvalidatedSignatures,
    )
    .expect("OLE object removes");
    assert!(saved_package(&prs).contains_part(OLE_PART));
}

/// The operations the refusal cases run.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(feature = "digital-signatures"), allow(dead_code))]
enum Act {
    Inventory,
    SaveAfterEdit,
    RemoveOle,
    RemoveVba,
}

fn act(package: &OpcPackage, act: Act) -> Result<(), Error> {
    let mut prs = Presentation::from_bytes(&bytes_of(package))?;
    let policy = EmbeddedMutationPolicy::PreserveInvalidatedSignatures;
    match act {
        Act::Inventory => prs.embedded_content().map(drop),
        Act::SaveAfterEdit => {
            prs.add_slide(blank_layout(&prs))?;
            prs.to_bytes().map(drop)
        }
        Act::RemoveOle => prs.remove_embedded_content(SLIDE, OLE_REL, policy),
        Act::RemoveVba => prs.remove_embedded_content(PRESENTATION, VBA_REL, policy),
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

/// A picture on the slide whose image relationship, `id`, targets `target`.
fn add_picture(package: &mut OpcPackage, id: &str, target: &str) {
    let picture = format!(
        concat!(
            r#"<p:pic><p:nvPicPr><p:cNvPr id="11" name="Picture 1"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr>"#,
            r#"<p:blipFill><a:blip xmlns:a="{a}" xmlns:r="{r}" r:embed="{id}"/></p:blipFill>"#,
            r#"<p:spPr/></p:pic>"#,
        ),
        a = A_NS,
        r = R_NS,
        id = id,
    );
    let xml = String::from_utf8(package.get_part(SLIDE).unwrap().to_vec()).unwrap();
    let tree_end = xml.find("</p:spTree>").expect("slide shape tree");
    package.set_part(
        SLIDE,
        format!("{}{picture}{}", &xml[..tree_end], &xml[tree_end..]).into_bytes(),
    );
    package
        .get_or_create_part_rels(SLIDE)
        .add_with_id(id, rel_types::IMAGE, target);
}

/// A picture whose relationship target holds a raw space: not a normalised
/// pack URI, and unrelated to embedded content.
fn add_image_with_a_space(package: &mut OpcPackage) {
    add_part(package, IMAGE_PART, "image/png", b"not really a PNG");
    add_picture(package, "rIdImage", "../media/image 1.png");
}

type Edit = fn(&mut OpcPackage);

/// Cases that need no signature, each of which Word refuses too: (name,
/// edit, act).
const UNSIGNED_CASES: [(&str, Edit, Act); 11] = [
    (
        "OLE relationship with a dot segment",
        |package| {
            relationship_mut(package, SLIDE, OLE_REL).target =
                "./../embeddings/oleObject1.bin".into()
        },
        Act::Inventory,
    ),
    (
        "VBA relationship with a dot segment",
        |package| {
            relationship_mut(package, PRESENTATION, VBA_REL).target = "./vbaProject.bin".into()
        },
        Act::Inventory,
    ),
    (
        "OLE relationship with TargetMode internal in lowercase",
        |package| relationship_mut(package, SLIDE, OLE_REL).target_mode = Some("internal".into()),
        Act::Inventory,
    ),
    (
        "VBA signature part with another content type",
        |package| {
            package
                .content_types
                .add_override(VBA_SIGNATURE_PART, "application/octet-stream")
        },
        Act::Inventory,
    ),
    (
        "ActiveX properties with another content type",
        |package| {
            package
                .content_types
                .add_override(CONTROL_PART, "application/xml")
        },
        Act::Inventory,
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
    ),
    (
        "ActiveX properties with two binaries",
        |package| {
            add_part(
                package,
                "/ppt/activeX/activeX2.bin",
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
    ),
    (
        "two VBA projects",
        |package| {
            add_part(package, "/ppt/vbaProject2.bin", VBA_CONTENT_TYPE, b"two");
            package.get_or_create_part_rels(PRESENTATION).add_with_id(
                "rIdVba2",
                rel_types::VBA_PROJECT,
                "vbaProject2.bin",
            );
        },
        Act::Inventory,
    ),
    (
        "remove a VBA project whose target another relationship shares",
        |package| {
            package.get_or_create_part_rels(PRESENTATION).add_with_id(
                "rIdOther",
                "http://example.com/relationships/other",
                "vbaProject.bin",
            );
        },
        Act::RemoveVba,
    ),
    (
        // PowerPoint's reachability already ignores case, as Word's does:
        // once it refuses a shared VBA target, it refuses this one too.
        "remove a VBA project another relationship names under another case",
        |package| {
            package.get_or_create_part_rels(PRESENTATION).add_with_id(
                "rIdOther",
                "http://example.com/relationships/other",
                "VBAPROJECT.BIN",
            );
        },
        Act::RemoveVba,
    ),
];

/// What each of [`UNSIGNED_CASES`] is refused for, in its order.
const UNSIGNED_REASONS: [&str; 11] = [
    "unsafe internal target ./../embeddings/oleObject1.bin",
    "unsafe internal target ./vbaProject.bin",
    "has an external target",
    "VBA signature has content type",
    "ActiveX properties has content type",
    "XML declaration version must be 1.0",
    "XML declaration attributes are invalid",
    "found 2 ActiveX binary relationships",
    "found 2 VBA project relationships",
    "is shared by another relationship",
    "is shared by another relationship",
];

#[test]
fn powerpoint_refuses_what_word_refuses_without_a_signature() {
    for ((name, edit, operation), reason) in UNSIGNED_CASES.into_iter().zip(UNSIGNED_REASONS) {
        let mut package = fixture_package();
        edit(&mut package);
        let error = act(&package, operation).expect_err(name).to_string();
        assert!(error.contains(reason), "{name}: {error}");
    }
}

#[test]
fn an_unrelated_image_target_holding_a_space_does_not_stop_a_removal() {
    // Not a normalised pack URI, but it names an ordinary part: only a
    // target reaching a protected part is refused.
    let mut package = fixture_package();
    add_image_with_a_space(&mut package);
    act(&package, Act::RemoveOle).expect("the OLE object removes");
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
    fn a_signed_deck_lists_every_payload_as_signed() {
        let prs = open(&signed_fixture_package());
        assert!(
            prs.verify_signatures()
                .unwrap()
                .iter()
                .all(|report| report.cryptographically_valid && report.coverage_complete)
        );
        assert!(
            states(&prs)
                .iter()
                .all(|(_, state)| *state == EmbeddedSignatureState::Present),
            "{:?}",
            states(&prs)
        );
    }

    #[test]
    fn a_signed_deck_saved_without_an_edit_keeps_a_valid_signature() {
        let prs = open(&signed_fixture_package());
        let saved = saved_package(&prs);
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
    fn a_signed_deck_saved_after_an_edit_marks_its_signature_invalidated() {
        let mut prs = open(&signed_fixture_package());
        prs.add_slide(blank_layout(&prs)).unwrap();
        let saved = saved_package(&prs);
        assert!(marks_package_signature_invalidated(&saved));
        assert!(saved.contains_part(ORIGIN_PART));
        assert!(saved.contains_part(SIGNATURE_PART));
        let reopened = Presentation::from_bytes(&bytes_of(&saved)).unwrap();
        assert_eq!(
            state_of(&reopened, EmbeddedContentKind::OleObject),
            EmbeddedSignatureState::Invalidated
        );
    }

    #[test]
    fn replacing_content_in_a_signed_deck_marks_or_drops_the_signature_by_policy() {
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
            let mut prs = open(&signed_fixture_package());
            let info = prs
                .replace_embedded_content(SLIDE, OLE_REL, b"new OLE payload", policy)
                .expect("OLE payload replaces");
            assert_eq!(info.signature_state, state, "{policy:?}");
            let saved = saved_package(&prs);
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
            let reopened = Presentation::from_bytes(&bytes_of(&saved)).unwrap();
            assert_eq!(
                state_of(&reopened, EmbeddedContentKind::OleObject),
                state,
                "{policy:?} after a reopen"
            );
        }
    }

    #[test]
    fn removing_content_from_a_signed_deck_marks_or_drops_the_signature_by_policy() {
        for (policy, signature_kept) in [
            (EmbeddedMutationPolicy::PreserveInvalidatedSignatures, true),
            (EmbeddedMutationPolicy::RemoveInvalidatedSignatures, false),
        ] {
            let mut prs = open(&signed_fixture_package());
            prs.remove_embedded_content(SLIDE, OLE_REL, policy)
                .expect("OLE object removes");
            let saved = saved_package(&prs);
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

    #[test]
    fn a_signed_deck_saves_with_an_image_target_holding_a_space() {
        let mut package = fixture_package();
        add_image_with_a_space(&mut package);
        sign(&mut package);
        // Not a normalised pack URI, but it names an ordinary part: only a
        // target reaching a protected part is refused.
        act(&package, Act::SaveAfterEdit).expect("the edited deck saves");
    }

    /// Cases on a signed package, edited after signing, each of which Word
    /// refuses on save too: (name, edit).
    const SIGNED_CASES: [(&str, Edit); 7] = [
        ("origin part with another content type", |package| {
            package
                .content_types
                .add_override(ORIGIN_PART, "application/octet-stream")
        }),
        ("signature part with another content type", |package| {
            package
                .content_types
                .add_override(SIGNATURE_PART, "application/xml")
        }),
        ("another relationship to the origin", |package| {
            package.get_or_create_part_rels(SLIDE).add_with_id(
                "rIdOrigin",
                "http://example.com/relationships/other",
                "../../_xmlsignatures/origin.sigs",
            );
        }),
        (
            "a relationship to the signature part from outside the origin",
            |package| {
                package.get_or_create_part_rels(SLIDE).add_with_id(
                    "rIdSignature",
                    "http://example.com/relationships/other",
                    "../../_xmlsignatures/sig1.xml",
                );
            },
        ),
        ("origin relationship with a dot segment", |package| {
            let origin = package
                .package_rels
                .items
                .iter_mut()
                .find(|relationship| relationship.rel_type == rel_types::DIGITAL_SIGNATURE_ORIGIN)
                .unwrap();
            origin.target = "./_xmlsignatures/origin.sigs".into();
        }),
        ("signature relationship with a dot segment", |package| {
            package.get_part_rels_mut(ORIGIN_PART).unwrap().items[0].target = "./sig1.xml".into();
        }),
        (
            // The canonical identity catches the case alias.
            "an unrelated picture naming the origin under another case",
            |package| add_picture(package, "rIdOrigin", "../../_XMLSIGNATURES/ORIGIN.SIGS"),
        ),
    ];

    /// What each of [`SIGNED_CASES`] is refused for, in its order.
    const SIGNED_REASONS: [&str; 7] = [
        "package signature origin has content type",
        "package XML signature has content type",
        "targets package signature part /_xmlsignatures/origin.sigs",
        "targets package signature part /_xmlsignatures/sig1.xml",
        "unsafe internal target ./_xmlsignatures/origin.sigs",
        "unsafe internal target ./sig1.xml",
        "targets package signature part /_XMLSIGNATURES/ORIGIN.SIGS",
    ];

    #[test]
    fn powerpoint_refuses_what_word_refuses_on_a_signed_package() {
        for ((name, edit), reason) in SIGNED_CASES.into_iter().zip(SIGNED_REASONS) {
            let mut package = signed_fixture_package();
            edit(&mut package);
            let error = act(&package, Act::SaveAfterEdit)
                .expect_err(name)
                .to_string();
            assert!(error.contains(reason), "{name}: {error}");
        }
    }

    #[test]
    fn a_signed_deck_saves_with_an_unrelated_image_target_holding_a_space_added_after_signing() {
        let mut package = signed_fixture_package();
        add_image_with_a_space(&mut package);
        act(&package, Act::SaveAfterEdit).expect("the edited deck saves");
    }

    /// The signed-corpus run of the specification's Risks section: every
    /// deck of the corpus, signed with the test key, gets one text box and
    /// is saved, and the count of saves and refusals by producer is
    /// printed. Slow: run it by hand, with `--ignored --nocapture`.
    #[test]
    #[ignore = "a run over the whole corpus, by hand"]
    fn every_corpus_deck_signed_saves_after_one_edit() {
        use std::collections::BTreeMap;

        use oxoffice_pptx::Emu;

        let mut counts = BTreeMap::<String, (usize, usize, usize)>::new();
        let mut refused = Vec::new();
        let mut unsigned = Vec::new();
        for file in crate::support::corpus_files("pptx") {
            let producer = file.path.split('/').next().unwrap_or_default().to_owned();
            let mut package =
                OpcPackage::from_reader(Cursor::new(file.bytes())).expect("the deck opens");
            let entry = counts.entry(producer).or_default();
            if let Err(error) = package.sign(KEY, CERTIFICATE) {
                entry.2 += 1;
                unsigned.push(format!("{}: {error}", file.path));
                continue;
            }
            let saved = Presentation::from_bytes(&bytes_of(&package)).and_then(|mut prs| {
                if prs.slides().len() == 0 {
                    prs.add_slide(0)?;
                }
                prs.slide_mut(0).expect("a slide").add_textbox(
                    Emu(0),
                    Emu(0),
                    Emu(914_400),
                    Emu(914_400),
                )?;
                prs.to_bytes()
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
