//! Embedded content (OLE objects, ActiveX controls, VBA projects) and the
//! package signature around it, as the facades check and mutate them.
//!
//! The package side lives here: the signature graph and its invalidation
//! markers, VBA and ActiveX resolution, the target and identity checks,
//! owner range collisions, reachability, and removing relationships and
//! parts. Each facade implements [`EmbeddedHost`] for what depends on its
//! format: which parts can own embedded content, the owner references in
//! one of them, removing one, and staging and committing a candidate.
//!
//! Part names are compared by their canonical identity (see `identity`):
//! resolved, percent-decoded, dot segments collapsed, ASCII case folded.
//! The relationships that define a protected part (the signature origin
//! and signatures, embedded objects, ActiveX parts, the VBA project and its
//! signature) need normalised pack URI targets; any other target is refused
//! only when it names no part, or names a protected part without being a
//! normalised pack URI.
//!
//! Compiled in every build: unlike the `signature` module, nothing here
//! verifies or creates a signature.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::Range;

use oxoffice_core::xml::{XmlLexicalError, validate_strict_xml_1_0};
use quick_xml::XmlVersion;
use quick_xml::events::{BytesDecl, BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;
use sha2::{Digest, Sha256};

use crate::OpcPackage;
use crate::relationship::{Relationship, rel_types};

use identity::{stored_part_identity, target_identities};

mod identity;

const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const STRICT_R_NS: &str = "http://purl.oclc.org/ooxml/officeDocument/relationships";
const ACTIVEX_NS: &str = "http://schemas.microsoft.com/office/2006/activeX";
const DSIG_NS: &str = "http://www.w3.org/2000/09/xmldsig#";
const OPC_SIGNATURE_NS: &str = "http://schemas.openxmlformats.org/package/2006/digital-signature";
const ACTIVEX_PROPERTIES_CONTENT_TYPE: &str = "application/vnd.ms-office.activeX+xml";
const VBA_SIGNATURE_CONTENT_TYPE: &str = "application/vnd.ms-office.vbaProjectSignature";
const VBA_AGILE_SIGNATURE_CONTENT_TYPE: &str = "application/vnd.ms-office.vbaProjectSignatureAgile";
const PACKAGE_SIGNATURE_ORIGIN_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-package.digital-signature-origin";
const PACKAGE_SIGNATURE_XML_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-package.digital-signature-xmlsignature+xml";
const INVALIDATED_PACKAGE_SIGNATURE: &str = "urn:rdocx:relationships/invalidated-package-signature";
const INVALIDATED_VBA_SIGNATURE: &str = "urn:rdocx:relationships/invalidated-vba-project-signature";

/// Executable or host-activated content owned by a package.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbeddedContentKind {
    OleObject,
    ActiveXControl,
    VbaProject,
}

/// Presence and known mutation state of signature evidence for one payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbeddedSignatureState {
    Absent,
    Present,
    Invalidated,
}

/// Explicit handling for signature evidence invalidated by an embedded mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbeddedMutationPolicy {
    PreserveInvalidatedSignatures,
    RemoveInvalidatedSignatures,
}

/// Stable audit facts for one relationship-owned executable payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddedContentInfo {
    pub kind: EmbeddedContentKind,
    pub source_part: String,
    pub relationship_id: String,
    pub target_part: String,
    pub content_type: String,
    pub byte_len: usize,
    pub sha256: [u8; 32],
    pub signature_state: EmbeddedSignatureState,
}

/// Why a check of embedded content or of the package signature refused.
///
/// Each facade maps these to the errors it returns: `operation` and
/// `message` give the text of every kind.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum EmbeddedError {
    /// A relationship the owner markup names is not in the owner's set.
    #[error(
        "resolve embedded content failed: {source_part}: relationship {relationship_id} is missing"
    )]
    MissingRelationship {
        source_part: String,
        relationship_id: String,
    },
    /// A relationship that must name a part of the package is external.
    #[error(
        "resolve embedded content failed: {source_part}: relationship {relationship_id} is external"
    )]
    ExternalRelationship {
        source_part: String,
        relationship_id: String,
    },
    /// A part a relationship names is not in the package.
    #[error("resolve embedded content failed: {part_name}: required part is missing")]
    MissingPart { part_name: String },
    /// Any other refusal, with the operation it stopped.
    #[error("{operation} failed: {message}")]
    Invalid {
        operation: &'static str,
        message: String,
    },
}

impl EmbeddedError {
    /// The operation the check stopped, as the facades name it.
    pub fn operation(&self) -> &'static str {
        match self {
            Self::MissingRelationship { .. }
            | Self::ExternalRelationship { .. }
            | Self::MissingPart { .. } => "resolve embedded content",
            Self::Invalid { operation, .. } => operation,
        }
    }

    /// What the check found, without the operation.
    pub fn message(&self) -> String {
        match self {
            Self::MissingRelationship {
                source_part,
                relationship_id,
            } => format!("{source_part}: relationship {relationship_id} is missing"),
            Self::ExternalRelationship {
                source_part,
                relationship_id,
            } => format!("{source_part}: relationship {relationship_id} is external"),
            Self::MissingPart { part_name } => format!("{part_name}: required part is missing"),
            Self::Invalid { message, .. } => message.clone(),
        }
    }
}

type Result<T> = std::result::Result<T, EmbeddedError>;

/// The two kinds of markup that own embedded content in a part.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerReferenceKind {
    /// An OLE object, owned through its relationship id.
    Ole,
    /// An ActiveX control, owned through the relationship to its properties.
    Control,
}

/// One owner reference in a part's XML: the relationship it names and the
/// byte range of the markup that owns it, which a removal takes out.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnerReference {
    pub relationship_id: String,
    pub range: Range<usize>,
}

/// The owner references of both kinds in one part.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OwnerReferences {
    pub ole: Vec<OwnerReference>,
    pub controls: Vec<OwnerReference>,
}

/// What a facade provides to the shared checks: its package, where its
/// format puts owner markup, and its staging.
pub trait EmbeddedHost: Sized {
    /// The facade's error; every refusal of the shared checks converts to it.
    type Error: From<EmbeddedError>;

    /// The package the checks read.
    fn embedded_package(&self) -> &OpcPackage;

    /// The package a mutation changes.
    fn embedded_package_mut(&mut self) -> &mut OpcPackage;

    /// The part whose relationships hold the VBA project: the main part.
    fn vba_owner_part(&self) -> &str;

    /// Every part whose XML can own OLE objects or ActiveX controls.
    fn owner_parts(&self) -> Vec<String>;

    /// The owner references in one of [`Self::owner_parts`].
    fn owner_references(&self, part: &str) -> std::result::Result<OwnerReferences, Self::Error>;

    /// Takes the owner markup of one relationship out of a part.
    fn remove_owner_reference(
        &mut self,
        part: &str,
        relationship_id: &str,
        kind: OwnerReferenceKind,
    ) -> std::result::Result<(), Self::Error>;

    /// Whether an edit already invalidated the package signature.
    fn package_signature_invalidated(&self) -> bool;

    /// Whether an edit invalidated the signature evidence of one payload.
    fn payload_signature_invalidated(&self, source_part: &str, relationship_id: &str) -> bool;

    /// The marks an embedded mutation updates: the package flag and the
    /// payloads, by source part and relationship id.
    fn invalidated_signatures_mut(&mut self) -> (&mut bool, &mut HashSet<(String, String)>);

    /// A candidate holding every pending edit, which a mutation changes.
    fn stage_candidate(&self) -> std::result::Result<Self, Self::Error>;

    /// Checks a mutated candidate and makes it the facade's state.
    fn commit_candidate(&mut self, staged: Self) -> std::result::Result<(), Self::Error>;
}

#[derive(Clone, Debug)]
struct OwnedEmbeddedContent {
    info: EmbeddedContentInfo,
    control_owners: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug)]
struct SignatureContext {
    package_present: bool,
    package_invalidated: bool,
    attached: bool,
    attached_invalidated: bool,
}

/// Inventories relationship-owned OLE, ActiveX, and VBA payloads of a staged
/// candidate without decoding them.
pub fn embedded_content<H: EmbeddedHost>(
    host: &H,
) -> std::result::Result<Vec<EmbeddedContentInfo>, H::Error> {
    inventory(&host.stage_candidate()?)
}

/// Inventories the payloads of the host as it stands, which is also the
/// check a candidate passes before a commit.
pub fn inventory<H: EmbeddedHost>(
    host: &H,
) -> std::result::Result<Vec<EmbeddedContentInfo>, H::Error> {
    Ok(owned_embedded_content(host)?
        .into_iter()
        .map(|owned| owned.info)
        .collect())
}

/// Extracts one relationship-owned payload byte for byte.
pub fn extract_embedded_content<H: EmbeddedHost>(
    host: &H,
    source_part: &str,
    relationship_id: &str,
) -> std::result::Result<Vec<u8>, H::Error> {
    validate_identity_source(source_part)?;
    let staged = host.stage_candidate()?;
    let owned = find_embedded_content(&staged, source_part, relationship_id)?;
    Ok(required_part(staged.embedded_package(), &owned.info.target_part)?.to_vec())
}

/// Replaces one opaque payload while retaining its relationship identity and part metadata.
pub fn replace_embedded_content<H: EmbeddedHost>(
    host: &mut H,
    source_part: &str,
    relationship_id: &str,
    bytes: &[u8],
    policy: EmbeddedMutationPolicy,
) -> std::result::Result<EmbeddedContentInfo, H::Error> {
    validate_identity_source(source_part)?;
    let mut staged = host.stage_candidate()?;
    reject_part_identity_collisions(staged.embedded_package())?;
    let selected = find_embedded_content(&staged, source_part, relationship_id)?;
    staged
        .embedded_package_mut()
        .set_part(&selected.info.target_part, bytes.to_vec());
    invalidate_embedded_signatures(&mut staged, &selected, policy)?;
    host.commit_candidate(staged)?;
    Ok(find_embedded_content(&host.stage_candidate()?, source_part, relationship_id)?.info)
}

/// Removes one logical embedded object and only its newly unreachable owned candidates.
pub fn remove_embedded_content<H: EmbeddedHost>(
    host: &mut H,
    source_part: &str,
    relationship_id: &str,
    policy: EmbeddedMutationPolicy,
) -> std::result::Result<(), H::Error> {
    validate_identity_source(source_part)?;
    let mut staged = host.stage_candidate()?;
    reject_part_identity_collisions(staged.embedded_package())?;
    let selected = find_embedded_content(&staged, source_part, relationship_id)?;
    remove_embedded_content_in_place(&mut staged, &selected, policy)?;
    host.commit_candidate(staged)
}

fn owned_embedded_content<H: EmbeddedHost>(
    host: &H,
) -> std::result::Result<Vec<OwnedEmbeddedContent>, H::Error> {
    let package = host.embedded_package();
    let main_part = host.vba_owner_part();
    let package_signature_present = has_package_signature(package)?;
    let package_signature_invalidated =
        host.package_signature_invalidated() || package_signature_invalidation_marked(package)?;
    let mut found = BTreeMap::<(String, String), OwnedEmbeddedContent>::new();
    let mut controls = BTreeMap::<String, Vec<(String, String)>>::new();
    let mut source_parts = BTreeSet::<String>::new();

    for (source_part, relationships) in &package.part_rels {
        ensure_unique_relationship_ids(source_part, &relationships.items)?;
        let has_owner_candidate = relationships.items.iter().any(|relationship| {
            matches!(
                relationship.rel_type.as_str(),
                rel_types::OLE_OBJECT
                    | rel_types::STRICT_OLE_OBJECT
                    | rel_types::CONTROL
                    | rel_types::STRICT_CONTROL
            )
        });
        let has_xml_content_type = package
            .content_types
            .content_type_for(source_part)
            .is_some_and(|content_type| content_type.ends_with("xml"));
        if !has_owner_candidate && !has_xml_content_type && source_part != main_part {
            continue;
        }
        validate_identity_source(source_part)?;
        source_parts.insert(source_part.clone());
    }
    let mut owner_parts = BTreeSet::<String>::new();
    for source_part in host.owner_parts() {
        validate_identity_source(&source_part)?;
        source_parts.insert(source_part.clone());
        owner_parts.insert(source_part);
    }

    for source_part in source_parts {
        required_part(package, &source_part)?;
        if !owner_parts.contains(&source_part) {
            continue;
        }
        let references = host.owner_references(&source_part)?;
        reject_owner_range_collisions(&source_part, &references.ole, &references.controls)?;
        for reference in references.ole {
            let relationship =
                required_relationship(package, &source_part, &reference.relationship_id)?;
            require_relationship_kind(
                &source_part,
                relationship,
                &[rel_types::OLE_OBJECT, rel_types::STRICT_OLE_OBJECT],
                "OLE object",
            )?;
            let target = safe_internal_target(&source_part, relationship)?;
            let info = embedded_info(
                host,
                EmbeddedContentKind::OleObject,
                &source_part,
                relationship,
                target,
                SignatureContext {
                    package_present: package_signature_present,
                    package_invalidated: package_signature_invalidated,
                    attached: false,
                    attached_invalidated: false,
                },
            )?;
            insert_unique(&mut found, info, Vec::new())?;
        }
        for reference in references.controls {
            let relationship =
                required_relationship(package, &source_part, &reference.relationship_id)?;
            require_relationship_kind(
                &source_part,
                relationship,
                &[rel_types::CONTROL, rel_types::STRICT_CONTROL],
                "ActiveX control properties",
            )?;
            let control_part = safe_internal_target(&source_part, relationship)?;
            required_part(package, &control_part)?;
            controls
                .entry(control_part)
                .or_default()
                .push((source_part.clone(), reference.relationship_id));
        }
    }

    for (control_part, owners) in controls {
        require_exact_content_type(
            package,
            &control_part,
            ACTIVEX_PROPERTIES_CONTENT_TYPE,
            "ActiveX properties",
        )?;
        let properties = required_part(package, &control_part)?;
        let binary_relationship_id =
            active_x_binary_relationship_id(properties)?.ok_or_else(|| {
                invalid(
                    "inventory embedded content",
                    format!(
                        "{control_part}: ActiveX properties root has no relationship-owned binary"
                    ),
                )
            })?;
        let control_relationships = package.get_part_rels(&control_part).ok_or_else(|| {
            invalid(
                "inventory embedded content",
                format!("{control_part}: ActiveX properties have no relationship set"),
            )
        })?;
        ensure_unique_relationship_ids(&control_part, &control_relationships.items)?;
        let binary_relationships = control_relationships
            .items
            .iter()
            .filter(|relationship| relationship.rel_type == rel_types::ACTIVEX_CONTROL_BINARY)
            .collect::<Vec<_>>();
        if binary_relationships.len() != 1 {
            return Err(invalid(
                "inventory embedded content",
                format!(
                    "{control_part}: found {} ActiveX binary relationships, expected exactly one",
                    binary_relationships.len()
                ),
            )
            .into());
        }
        let relationship = required_relationship(package, &control_part, &binary_relationship_id)?;
        require_relationship_kind(
            &control_part,
            relationship,
            &[rel_types::ACTIVEX_CONTROL_BINARY],
            "ActiveX binary",
        )?;
        let target = safe_internal_target(&control_part, relationship)?;
        let info = embedded_info(
            host,
            EmbeddedContentKind::ActiveXControl,
            &control_part,
            relationship,
            target,
            SignatureContext {
                package_present: package_signature_present,
                package_invalidated: package_signature_invalidated,
                attached: false,
                attached_invalidated: false,
            },
        )?;
        insert_unique(&mut found, info, owners)?;
    }

    if let Some(relationships) = package.get_part_rels(main_part) {
        ensure_unique_relationship_ids(main_part, &relationships.items)?;
        let vba_projects = relationships
            .items
            .iter()
            .filter(|relationship| relationship.rel_type == rel_types::VBA_PROJECT)
            .collect::<Vec<_>>();
        if vba_projects.len() > 1 {
            return Err(invalid(
                "inventory embedded content",
                format!(
                    "{main_part}: found {} VBA project relationships, expected at most one",
                    vba_projects.len()
                ),
            )
            .into());
        }
        for relationship in vba_projects {
            let target = safe_internal_target(main_part, relationship)?;
            let attached_signature = attached_vba_signature_state(package, &target)?;
            let info = embedded_info(
                host,
                EmbeddedContentKind::VbaProject,
                main_part,
                relationship,
                target,
                SignatureContext {
                    package_present: package_signature_present,
                    package_invalidated: package_signature_invalidated,
                    attached: attached_signature.is_some(),
                    attached_invalidated: attached_signature == Some(true),
                },
            )?;
            insert_unique(&mut found, info, Vec::new())?;
        }
    }

    Ok(found.into_values().collect())
}

fn find_embedded_content<H: EmbeddedHost>(
    host: &H,
    source_part: &str,
    relationship_id: &str,
) -> std::result::Result<OwnedEmbeddedContent, H::Error> {
    owned_embedded_content(host)?
        .into_iter()
        .find(|owned| {
            owned.info.source_part == source_part && owned.info.relationship_id == relationship_id
        })
        .ok_or_else(|| {
            invalid(
                "resolve embedded content",
                format!(
                    "{source_part}: relationship {relationship_id} is not an owned embedded payload"
                ),
            )
            .into()
        })
}

fn invalidate_embedded_signatures<H: EmbeddedHost>(
    host: &mut H,
    selected: &OwnedEmbeddedContent,
    policy: EmbeddedMutationPolicy,
) -> Result<()> {
    let identity = (
        selected.info.source_part.clone(),
        selected.info.relationship_id.clone(),
    );
    if selected.info.signature_state != EmbeddedSignatureState::Absent {
        host.invalidated_signatures_mut().1.insert(identity.clone());
    }
    if has_package_signature(host.embedded_package())? {
        *host.invalidated_signatures_mut().0 = true;
    }
    if policy == EmbeddedMutationPolicy::PreserveInvalidatedSignatures
        && selected.info.kind == EmbeddedContentKind::VbaProject
    {
        mark_vba_signature_invalidated(host.embedded_package_mut(), &selected.info.target_part)?;
    }
    if policy == EmbeddedMutationPolicy::RemoveInvalidatedSignatures {
        let package = host.embedded_package_mut();
        remove_package_signatures(package)?;
        if selected.info.kind == EmbeddedContentKind::VbaProject {
            remove_vba_signatures(package, &selected.info.target_part)?;
        }
        let (package_invalidated, payloads) = host.invalidated_signatures_mut();
        payloads.remove(&identity);
        *package_invalidated = false;
    }
    Ok(())
}

fn remove_embedded_content_in_place<H: EmbeddedHost>(
    host: &mut H,
    selected: &OwnedEmbeddedContent,
    policy: EmbeddedMutationPolicy,
) -> std::result::Result<(), H::Error> {
    let info = &selected.info;
    if info.kind == EmbeddedContentKind::VbaProject
        && relationship_target_is_reachable_except(
            host.embedded_package(),
            &info.target_part,
            Some((&info.source_part, &info.relationship_id)),
        )?
    {
        return Err(invalid(
            "remove embedded content",
            format!(
                "{}: VBA project target {} is shared by another relationship",
                info.source_part, info.target_part
            ),
        )
        .into());
    }
    invalidate_embedded_signatures(host, selected, policy)?;
    match info.kind {
        EmbeddedContentKind::OleObject => {
            host.remove_owner_reference(
                &info.source_part,
                &info.relationship_id,
                OwnerReferenceKind::Ole,
            )?;
            let package = host.embedded_package_mut();
            remove_relationship(package, &info.source_part, &info.relationship_id)?;
            delete_if_unreachable(package, &info.target_part)?;
        }
        EmbeddedContentKind::ActiveXControl => {
            for (owner_part, owner_relationship_id) in &selected.control_owners {
                host.remove_owner_reference(
                    owner_part,
                    owner_relationship_id,
                    OwnerReferenceKind::Control,
                )?;
                remove_relationship(
                    host.embedded_package_mut(),
                    owner_part,
                    owner_relationship_id,
                )?;
            }
            let package = host.embedded_package_mut();
            if relationship_target_is_reachable(package, &info.source_part)? {
                return Ok(());
            }
            remove_relationship(package, &info.source_part, &info.relationship_id)?;
            delete_if_unreachable(package, &info.target_part)?;
            delete_if_unreachable(package, &info.source_part)?;
        }
        EmbeddedContentKind::VbaProject => {
            let package = host.embedded_package_mut();
            if policy == EmbeddedMutationPolicy::PreserveInvalidatedSignatures {
                retain_vba_signature_parts_as_evidence(package, &info.target_part);
            }
            remove_relationship(package, &info.source_part, &info.relationship_id)?;
            delete_if_unreachable(package, &info.target_part)?;
        }
    }
    Ok(())
}

/// Whether the package carries an invalidation marker, or a signature whose
/// manifest names a part or relationship the package lacks, found without
/// verifying any signature.
pub fn package_signature_known_invalid_without_verifying(package: &OpcPackage) -> bool {
    let marked = package_signature_invalidation_marked(package).unwrap_or(false);
    let missing = signature_manifest_has_missing_reference(package);
    marked || missing
}

/// Whether a package just opened carries a package signature known to be
/// invalid: marked so by an earlier save, failing verification or incomplete
/// in its coverage, or with an unsupported part transform chain.
#[cfg(feature = "digital-signatures")]
pub fn known_invalid_package_signature_on_open(package: &OpcPackage) -> bool {
    if package_signature_invalidation_marked(package).unwrap_or(false) {
        return true;
    }
    match package.verify_signatures() {
        Ok(reports) => {
            !reports.is_empty()
                && reports
                    .iter()
                    .any(|report| !report.cryptographically_valid || !report.coverage_complete)
        }
        Err(crate::OpcError::UnsupportedSignatureAlgorithm {
            kind: "part transform chain",
            ..
        }) => true,
        Err(_) => signature_manifest_has_missing_reference(package),
    }
}

fn relationship_is_internal(relationship: &Relationship) -> bool {
    matches!(relationship.target_mode.as_deref(), None | Some("Internal"))
}

fn invalid(operation: &'static str, message: String) -> EmbeddedError {
    EmbeddedError::Invalid { operation, message }
}

/// Adds the invalidation marker to a signed package an edit invalidated,
/// once.
pub fn persist_invalidated_package_signature(
    package: &mut OpcPackage,
    invalidated: bool,
) -> Result<()> {
    if !invalidated || package_signature_invalidation_marked(package)? {
        return Ok(());
    }
    let Some(graph) = package_signature_graph(package)? else {
        return Ok(());
    };
    let origin_part = &graph.origins[0].1;
    package.package_rels.add(
        INVALIDATED_PACKAGE_SIGNATURE,
        origin_part.strip_prefix('/').unwrap_or(origin_part),
    );
    Ok(())
}

fn embedded_info<H: EmbeddedHost>(
    host: &H,
    kind: EmbeddedContentKind,
    source_part: &str,
    relationship: &Relationship,
    target_part: String,
    signature: SignatureContext,
) -> Result<EmbeddedContentInfo> {
    let package = host.embedded_package();
    let bytes = required_part(package, &target_part)?;
    let content_type = package
        .content_types
        .content_type_for(&target_part)
        .ok_or_else(|| {
            invalid(
                "inventory embedded content",
                format!("{target_part}: embedded payload has no content type"),
            )
        })?
        .to_owned();
    let signature_state = if !signature.package_present && !signature.attached {
        EmbeddedSignatureState::Absent
    } else if signature.package_invalidated
        || signature.attached_invalidated
        || host.payload_signature_invalidated(source_part, &relationship.id)
    {
        EmbeddedSignatureState::Invalidated
    } else {
        EmbeddedSignatureState::Present
    };
    Ok(EmbeddedContentInfo {
        kind,
        source_part: source_part.to_owned(),
        relationship_id: relationship.id.clone(),
        target_part,
        content_type,
        byte_len: bytes.len(),
        sha256: Sha256::digest(bytes).into(),
        signature_state,
    })
}

fn insert_unique(
    found: &mut BTreeMap<(String, String), OwnedEmbeddedContent>,
    info: EmbeddedContentInfo,
    control_owners: Vec<(String, String)>,
) -> Result<()> {
    let key = (info.source_part.clone(), info.relationship_id.clone());
    if let Some(existing) = found.get_mut(&key) {
        if existing.info.kind != info.kind || existing.info.target_part != info.target_part {
            return Err(invalid(
                "inventory embedded content",
                format!(
                    "{}: relationship {} has ambiguous executable ownership",
                    key.0, key.1
                ),
            ));
        }
        existing.control_owners.extend(control_owners);
        existing.control_owners.sort();
        existing.control_owners.dedup();
    } else {
        found.insert(
            key,
            OwnedEmbeddedContent {
                info,
                control_owners,
            },
        );
    }
    Ok(())
}

fn reject_owner_range_collisions(
    source_part: &str,
    ole_references: &[OwnerReference],
    control_references: &[OwnerReference],
) -> Result<()> {
    for (kind, references) in [("OLE", ole_references), ("ActiveX", control_references)] {
        if let Some((left, right)) = references.iter().enumerate().find_map(|(index, left)| {
            references[index + 1..]
                .iter()
                .find(|right| owner_ranges_overlap(&left.range, &right.range))
                .map(|right| (left, right))
        }) {
            return Err(invalid(
                "inventory embedded content",
                format!(
                    "{source_part}: {kind} relationships {} and {} claim overlapping owner XML",
                    left.relationship_id, right.relationship_id
                ),
            ));
        }
    }
    if let Some((ole, control)) = ole_references.iter().find_map(|ole| {
        control_references
            .iter()
            .find(|control| owner_ranges_overlap(&ole.range, &control.range))
            .map(|control| (ole, control))
    }) {
        return Err(invalid(
            "inventory embedded content",
            format!(
                "{source_part}: OLE relationship {} and ActiveX relationship {} claim overlapping owner XML",
                ole.relationship_id, control.relationship_id
            ),
        ));
    }
    Ok(())
}

fn owner_ranges_overlap(left: &Range<usize>, right: &Range<usize>) -> bool {
    left.start < right.end && right.start < left.end
}

fn required_relationship<'a>(
    package: &'a OpcPackage,
    source_part: &str,
    relationship_id: &str,
) -> Result<&'a Relationship> {
    let matches = package
        .get_part_rels(source_part)
        .map(|relationships| {
            relationships
                .items
                .iter()
                .filter(|relationship| relationship.id == relationship_id)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    match matches.as_slice() {
        [relationship] => Ok(*relationship),
        [] => Err(EmbeddedError::MissingRelationship {
            source_part: source_part.to_owned(),
            relationship_id: relationship_id.to_owned(),
        }),
        _ => Err(invalid(
            "resolve embedded content",
            format!(
                "{source_part}: relationship id {relationship_id} is ambiguous across {} entries",
                matches.len()
            ),
        )),
    }
}

fn require_relationship_kind(
    source_part: &str,
    relationship: &Relationship,
    expected: &[&str],
    label: &str,
) -> Result<()> {
    if expected.contains(&relationship.rel_type.as_str()) {
        Ok(())
    } else {
        Err(invalid(
            "inventory embedded content",
            format!(
                "{source_part}: relationship {} has type {}, expected {label}",
                relationship.id, relationship.rel_type
            ),
        ))
    }
}

fn safe_internal_target(source_part: &str, relationship: &Relationship) -> Result<String> {
    if !relationship_is_internal(relationship) {
        return Err(EmbeddedError::ExternalRelationship {
            source_part: source_part.to_owned(),
            relationship_id: relationship.id.clone(),
        });
    }
    if !relationship_target_is_normalized_pack_uri(&relationship.target)
        || target_escapes_package_root(source_part, &relationship.target)
    {
        return Err(invalid(
            "resolve embedded content",
            format!(
                "{source_part}: relationship {} has an unsafe internal target {}",
                relationship.id, relationship.target
            ),
        ));
    }
    Ok(OpcPackage::resolve_rel_target(
        source_part,
        &relationship.target,
    ))
}

/// Whether a relationship target is a normalised pack URI: ASCII, no empty,
/// `.` or trailing-dot segment, no `\`, `?` or `#`, and only the escapes a
/// normaliser keeps, in uppercase.
pub fn relationship_target_is_normalized_pack_uri(target: &str) -> bool {
    if target.is_empty()
        || target.ends_with('/')
        || target.contains("//")
        || target.contains(['\\', '?', '#'])
        || !target.is_ascii()
    {
        return false;
    }
    let relative_first_segment =
        (!target.starts_with('/')).then(|| target.split('/').next().unwrap_or_default());
    if relative_first_segment.is_some_and(|segment| segment.contains(':')) {
        return false;
    }
    let bytes = target.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' {
            let (Some(&high), Some(&low)) = (bytes.get(index + 1), bytes.get(index + 2)) else {
                return false;
            };
            if !high.is_ascii_hexdigit()
                || !low.is_ascii_hexdigit()
                || high.is_ascii_lowercase()
                || low.is_ascii_lowercase()
            {
                return false;
            }
            let decoded = (hex_value(high) << 4) | hex_value(low);
            if decoded.is_ascii_alphanumeric()
                || matches!(decoded, b'-' | b'.' | b'_' | b'~' | b'/' | b'\\')
                || decoded.is_ascii_control()
            {
                return false;
            }
            index += 3;
            continue;
        }
        if !(byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'@'
                    | b'/'
            ))
        {
            return false;
        }
        index += 1;
    }
    target
        .split('/')
        .filter(|segment| !segment.is_empty())
        .all(|segment| segment == ".." || (segment != "." && !segment.ends_with('.')))
}

fn hex_value(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'A'..=b'F' => byte - b'A' + 10,
        _ => 0,
    }
}

fn target_escapes_package_root(source_part: &str, target: &str) -> bool {
    let mut depth = if target.starts_with('/') {
        0
    } else {
        source_part
            .trim_start_matches('/')
            .split('/')
            .count()
            .saturating_sub(1)
    };
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." if depth == 0 => return true,
            ".." => depth -= 1,
            _ => depth += 1,
        }
    }
    false
}

/// The bytes of a part a relationship names, or [`EmbeddedError::MissingPart`].
pub fn required_part<'a>(package: &'a OpcPackage, part_name: &str) -> Result<&'a [u8]> {
    package
        .get_part(part_name)
        .ok_or_else(|| EmbeddedError::MissingPart {
            part_name: part_name.to_owned(),
        })
}

fn validate_identity_source(source_part: &str) -> Result<()> {
    if !relationship_target_is_normalized_pack_uri(source_part)
        || !source_part.starts_with('/')
        || source_part == "/"
        || source_part.ends_with('/')
        || source_part.contains("//")
        || source_part.contains('\\')
        || source_part
            .split('/')
            .any(|segment| matches!(segment, "." | ".."))
        || source_part.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(invalid(
            "resolve embedded content",
            format!("unsafe source part identity {source_part}"),
        ));
    }
    Ok(())
}

/// Refuses an XML declaration that names an encoding other than UTF-8.
pub fn validate_utf8_xml_declaration(
    declaration: &BytesDecl<'_>,
    operation: &'static str,
) -> Result<()> {
    match declaration.encoding() {
        Some(Ok(encoding)) if !encoding.eq_ignore_ascii_case(b"UTF-8") => Err(invalid(
            operation,
            "XML declaration attributes are invalid, duplicated, or out of order".to_owned(),
        )),
        Some(Err(error)) => Err(invalid(
            operation,
            format!("invalid XML declaration: {error}"),
        )),
        None | Some(Ok(_)) => Ok(()),
    }
}

/// The strict XML 1.0 check of a part that owns or holds embedded content,
/// with its messages.
pub fn validate_embedded_lexical(xml: &[u8], operation: &'static str) -> Result<()> {
    validate_strict_xml_1_0(xml).map_err(|error| embedded_lexical_error(xml, operation, error))
}

fn embedded_lexical_error(
    xml: &[u8],
    operation: &'static str,
    error: XmlLexicalError,
) -> EmbeddedError {
    let message = match error {
        XmlLexicalError::InvalidUtf8 => {
            let detail = std::str::from_utf8(xml)
                .err()
                .map_or_else(|| "invalid UTF-8".to_owned(), |error| error.to_string());
            format!("XML is not UTF-8: {detail}")
        }
        XmlLexicalError::InvalidDeclaration(reason) if reason == "version must be 1.0" => {
            "XML declaration version must be 1.0".to_owned()
        }
        XmlLexicalError::InvalidDeclaration(reason)
            if reason == "attributes are invalid, duplicated, or out of order" =>
        {
            "XML declaration attributes are invalid, duplicated, or out of order".to_owned()
        }
        XmlLexicalError::InvalidDeclaration(reason) if reason == "must begin with version" => {
            "XML declaration is missing its version".to_owned()
        }
        XmlLexicalError::InvalidDeclaration(reason) => {
            format!("invalid XML declaration: {reason}")
        }
        XmlLexicalError::ForbiddenLiteralCharacter => {
            "XML contains a forbidden literal XML 1.0 character".to_owned()
        }
        XmlLexicalError::InvalidName(reason) if reason.starts_with("qualified name ") => {
            format!("invalid XML {reason}")
        }
        XmlLexicalError::InvalidName(reason) if reason.starts_with("name ") => {
            format!("invalid XML {reason}")
        }
        XmlLexicalError::InvalidName(reason) => reason,
        XmlLexicalError::InvalidNamespace(reason)
            if reason == "element uses the reserved xmlns prefix" =>
        {
            "XML element uses the reserved xmlns prefix".to_owned()
        }
        XmlLexicalError::InvalidNamespace(reason) if reason == "invalid namespace declaration" => {
            "XML contains an invalid namespace declaration".to_owned()
        }
        XmlLexicalError::InvalidNamespace(reason) => format!("XML uses {reason}"),
        XmlLexicalError::DuplicateExpandedAttribute => {
            "XML element has duplicate expanded-name attributes".to_owned()
        }
        XmlLexicalError::InvalidReference(reason)
            if reason == "attribute contains a literal less-than sign" =>
        {
            "XML attribute contains a literal less-than sign".to_owned()
        }
        XmlLexicalError::InvalidReference(reason)
            if reason.starts_with("undeclared entity reference ") =>
        {
            reason.replacen(
                "undeclared entity reference ",
                "undeclared general entity reference ",
                1,
            )
        }
        XmlLexicalError::InvalidReference(reason) => reason,
        XmlLexicalError::InvalidProcessingInstruction(reason)
            if reason == "reserved XML target" =>
        {
            if operation == "inventory embedded content" {
                "ActiveX properties contain a reserved XML processing instruction".to_owned()
            } else {
                "reserved XML processing instruction target".to_owned()
            }
        }
        XmlLexicalError::InvalidProcessingInstruction(reason) if reason.starts_with("name ") => {
            format!("invalid XML {reason}")
        }
        XmlLexicalError::InvalidProcessingInstruction(reason) => reason,
        XmlLexicalError::InvalidComment(reason) => reason,
    };
    invalid(operation, message)
}

fn remove_relationship(
    package: &mut OpcPackage,
    source_part: &str,
    relationship_id: &str,
) -> Result<()> {
    required_relationship(package, source_part, relationship_id)?;
    let relationships = package.get_part_rels_mut(source_part).ok_or_else(|| {
        invalid(
            "remove embedded content",
            format!("{source_part}: relationship set disappeared"),
        )
    })?;
    relationships
        .items
        .retain(|relationship| relationship.id != relationship_id);
    Ok(())
}

fn delete_if_unreachable(package: &mut OpcPackage, candidate: &str) -> Result<()> {
    if relationship_target_is_reachable(package, candidate)? {
        return Ok(());
    }
    package.remove_part(candidate);
    package.remove_part_rels(candidate);
    package.content_types.remove_override(candidate);
    Ok(())
}

fn relationship_target_is_reachable(package: &OpcPackage, candidate: &str) -> Result<bool> {
    relationship_target_is_reachable_except(package, candidate, None)
}

fn relationship_target_is_reachable_except(
    package: &OpcPackage,
    candidate: &str,
    excluded: Option<(&str, &str)>,
) -> Result<bool> {
    let candidate = stored_part_identity(candidate);
    Ok(internal_targets(package)?.iter().any(|target| {
        excluded != Some((target.source_part, target.relationship.id.as_str()))
            && target.identities.contains(&candidate)
    }))
}

/// One internal relationship of the package and every part identity its
/// target can name.
struct InternalTarget<'a> {
    /// `/` for a package relationship.
    source_part: &'a str,
    from_package: bool,
    relationship: &'a Relationship,
    identities: Vec<String>,
}

/// Whether a relationship of this type defines a part the checks protect:
/// the signature origin and signatures, embedded objects, ActiveX controls
/// and binaries, the VBA project and its signature, and the invalidation
/// markers that name them. Its target must be a normalised pack URI.
fn defines_protected_part(relationship: &Relationship) -> bool {
    matches!(
        relationship.rel_type.as_str(),
        rel_types::DIGITAL_SIGNATURE_ORIGIN
            | rel_types::DIGITAL_SIGNATURE
            | INVALIDATED_PACKAGE_SIGNATURE
            | rel_types::OLE_OBJECT
            | rel_types::STRICT_OLE_OBJECT
            | rel_types::CONTROL
            | rel_types::STRICT_CONTROL
            | rel_types::ACTIVEX_CONTROL_BINARY
            | rel_types::VBA_PROJECT
            | rel_types::VBA_PROJECT_SIGNATURE
            | rel_types::VBA_PROJECT_SIGNATURE_AGILE
            | INVALIDATED_VBA_SIGNATURE
    )
}

/// Every internal relationship of the package with the identities its
/// target names.
///
/// A relationship that defines a protected part needs a normalised pack URI
/// target. Any other target is refused only when it cannot be resolved, or
/// when it is not a normalised pack URI and one of its readings names a
/// protected part. An ordinary part reached through an odd target, such as
/// an image whose target holds a raw space, is accepted.
fn internal_targets(package: &OpcPackage) -> Result<Vec<InternalTarget<'_>>> {
    let package_relationships = package
        .package_rels
        .items
        .iter()
        .map(|relationship| ("/", true, relationship));
    let part_relationships = package
        .part_rels
        .iter()
        .flat_map(|(source_part, relationships)| {
            relationships
                .items
                .iter()
                .map(move |relationship| (source_part.as_str(), false, relationship))
        });
    let mut targets = Vec::new();
    let mut protected = HashSet::new();
    for (source_part, from_package, relationship) in package_relationships.chain(part_relationships)
    {
        if !relationship_is_internal(relationship) {
            continue;
        }
        if defines_protected_part(relationship) {
            safe_internal_target(source_part, relationship)?;
        }
        let identities = target_identities(source_part, &relationship.target)
            .map_err(|reason| unsafe_internal_target(source_part, relationship, reason))?;
        if defines_protected_part(relationship) {
            protected.extend(identities.iter().cloned());
        }
        targets.push(InternalTarget {
            source_part,
            from_package,
            relationship,
            identities,
        });
    }
    if let Some(target) = targets.iter().find(|target| {
        !defines_protected_part(target.relationship)
            && !relationship_target_is_normalized_pack_uri(&target.relationship.target)
            && target
                .identities
                .iter()
                .any(|identity| protected.contains(identity))
    }) {
        return Err(unsafe_internal_target(
            target.source_part,
            target.relationship,
            "it is not a normalised pack URI and names a protected part",
        ));
    }
    Ok(targets)
}

fn unsafe_internal_target(
    source_part: &str,
    relationship: &Relationship,
    reason: impl std::fmt::Display,
) -> EmbeddedError {
    invalid(
        "resolve embedded content",
        format!(
            "{source_part}: relationship {} has an unsafe internal target {}: {reason}",
            relationship.id, relationship.target
        ),
    )
}

/// Refuses two stored parts that name the same part once percent-decoded:
/// a target naming one names the other too.
fn reject_part_identity_collisions(package: &OpcPackage) -> Result<()> {
    let mut names = package.parts.keys().collect::<Vec<_>>();
    names.sort_unstable();
    let mut seen = HashMap::with_capacity(names.len());
    for name in names {
        if let Some(other) = seen.insert(stored_part_identity(name), name) {
            return Err(invalid(
                "inventory embedded content",
                format!("parts {other} and {name} name the same part: ambiguous part names"),
            ));
        }
    }
    Ok(())
}

fn active_x_binary_relationship_id(xml: &[u8]) -> Result<Option<String>> {
    validate_embedded_lexical(xml, "inventory embedded content")?;
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut root_seen = false;
    let mut declaration_seen = false;
    let mut prolog_content_seen = false;
    let mut relationship_id = None;
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| invalid("inventory embedded content", error.to_string()))?;
        let root_namespace = namespace_is(&namespace, &[ACTIVEX_NS]);
        let event = event.into_owned();
        drop(namespace);
        match event {
            Event::Start(element) if depth == 0 => {
                if root_seen || !root_namespace || local_name(element.name().as_ref()) != b"ocx" {
                    return Err(invalid(
                        "inventory embedded content",
                        "ActiveX properties must contain one ax:ocx root".to_owned(),
                    ));
                }
                relationship_id = relationship_id_attribute(&reader, &element)?;
                root_seen = true;
                depth = 1;
            }
            Event::Start(_) => {
                depth = depth.checked_add(1).ok_or_else(|| {
                    invalid(
                        "inventory embedded content",
                        "ActiveX properties nesting is too deep".to_owned(),
                    )
                })?
            }
            Event::Empty(element) if depth == 0 => {
                if root_seen || !root_namespace || local_name(element.name().as_ref()) != b"ocx" {
                    return Err(invalid(
                        "inventory embedded content",
                        "ActiveX properties must contain one ax:ocx root".to_owned(),
                    ));
                }
                relationship_id = relationship_id_attribute(&reader, &element)?;
                root_seen = true;
            }
            Event::Empty(_) => {}
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    invalid(
                        "inventory embedded content",
                        "ActiveX properties have an unmatched closing element".to_owned(),
                    )
                })?;
            }
            Event::Decl(declaration) => {
                if depth != 0 || root_seen || declaration_seen || prolog_content_seen {
                    return Err(invalid(
                        "inventory embedded content",
                        "ActiveX properties contain a misplaced XML declaration".to_owned(),
                    ));
                }
                validate_utf8_xml_declaration(&declaration, "inventory embedded content")?;
                declaration_seen = true;
            }
            Event::DocType(_) => {
                return Err(invalid(
                    "inventory embedded content",
                    "ActiveX properties cannot contain a document type".to_owned(),
                ));
            }
            Event::Comment(_) | Event::PI(_) if depth == 0 => {
                if !root_seen {
                    prolog_content_seen = true;
                }
            }
            Event::Text(text)
                if depth == 0 && {
                    let bytes: &[u8] = text.as_ref();
                    bytes.iter().all(u8::is_ascii_whitespace)
                } =>
            {
                if !root_seen {
                    prolog_content_seen = true;
                }
            }
            Event::Eof if root_seen && depth == 0 => return Ok(relationship_id),
            Event::Eof => {
                return Err(invalid(
                    "inventory embedded content",
                    "ActiveX properties XML ended before its root closed".to_owned(),
                ));
            }
            _ if depth > 0 => {}
            _ => {
                return Err(invalid(
                    "inventory embedded content",
                    "ActiveX properties contain content outside ax:ocx".to_owned(),
                ));
            }
        }
        buffer.clear();
    }
}

/// The `r:id` attribute of an owner element, in the transitional or strict
/// relationships namespace; `None` when absent or empty, and refused when
/// given twice.
pub fn relationship_id_attribute(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<Option<String>> {
    let mut relationship_id = None;
    let mut semantic_attribute_seen = false;
    for attribute in element.attributes() {
        let attribute =
            attribute.map_err(|error| invalid("scan embedded XML", error.to_string()))?;
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if local.as_ref() == b"id" && namespace_is(&namespace, &[R_NS, STRICT_R_NS]) {
            if semantic_attribute_seen {
                return Err(invalid(
                    "scan embedded XML",
                    format!(
                        "{} has duplicate relationship id attributes",
                        String::from_utf8_lossy(element.name().as_ref())
                    ),
                ));
            }
            semantic_attribute_seen = true;
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .map_err(|error| invalid("scan embedded XML", error.to_string()))?
                .into_owned();
            relationship_id = (!value.is_empty()).then_some(value);
        }
    }
    Ok(relationship_id)
}

fn namespace_is(namespace: &ResolveResult<'_>, expected: &[&str]) -> bool {
    matches!(namespace, ResolveResult::Bound(Namespace(uri)) if expected.iter().any(|value| *uri == value.as_bytes()))
}

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

/// The XML without the owner ranges a removal takes out, which must not overlap.
pub fn remove_owner_ranges(xml: &[u8], mut ranges: Vec<Range<usize>>) -> Result<Vec<u8>> {
    ranges.sort_by_key(|range| range.start);
    ranges.dedup();
    let mut output = Vec::with_capacity(xml.len());
    let mut copied = 0usize;
    for range in ranges {
        if range.start < copied || range.end > xml.len() || range.start > range.end {
            return Err(invalid(
                "remove embedded content",
                "overlapping or invalid owning XML ranges".to_owned(),
            ));
        }
        output.extend_from_slice(&xml[copied..range.start]);
        copied = range.end;
    }
    output.extend_from_slice(&xml[copied..]);
    Ok(output)
}

fn is_vba_signature(relationship: &Relationship) -> bool {
    matches!(
        relationship.rel_type.as_str(),
        rel_types::VBA_PROJECT_SIGNATURE | rel_types::VBA_PROJECT_SIGNATURE_AGILE
    )
}

fn attached_vba_signature_state(package: &OpcPackage, project_part: &str) -> Result<Option<bool>> {
    let Some(relationships) = package.get_part_rels(project_part) else {
        return Ok(None);
    };
    ensure_unique_relationship_ids(project_part, &relationships.items)?;
    let signatures = relationships
        .items
        .iter()
        .filter(|relationship| is_vba_signature(relationship))
        .collect::<Vec<_>>();
    if signatures.len() > 1 {
        return Err(invalid(
            "inventory embedded content",
            format!(
                "{project_part}: found {} VBA project signature relationships, expected at most one",
                signatures.len()
            ),
        ));
    }
    let markers = relationships
        .items
        .iter()
        .filter(|relationship| relationship.rel_type == INVALIDATED_VBA_SIGNATURE)
        .collect::<Vec<_>>();
    if markers.len() > 1 {
        return Err(invalid(
            "inventory embedded content",
            format!(
                "{project_part}: found {} VBA signature invalidation markers, expected at most one",
                markers.len()
            ),
        ));
    }
    let Some(signature) = signatures.first() else {
        if markers.is_empty() {
            return Ok(None);
        }
        return Err(invalid(
            "inventory embedded content",
            format!("{project_part}: VBA invalidation marker has no signature relationship"),
        ));
    };
    let target = safe_internal_target(project_part, signature)?;
    required_part(package, &target)?;
    let expected_content_type = if signature.rel_type == rel_types::VBA_PROJECT_SIGNATURE {
        VBA_SIGNATURE_CONTENT_TYPE
    } else {
        VBA_AGILE_SIGNATURE_CONTENT_TYPE
    };
    require_exact_content_type(package, &target, expected_content_type, "VBA signature")?;
    if let Some(marker) = markers.first() {
        let marker_target = safe_internal_target(project_part, marker)?;
        if stored_part_identity(&marker_target) != stored_part_identity(&target) {
            return Err(invalid(
                "inventory embedded content",
                format!(
                    "{project_part}: VBA invalidation marker targets {marker_target}, expected {target}"
                ),
            ));
        }
        Ok(Some(true))
    } else {
        Ok(Some(false))
    }
}

fn mark_vba_signature_invalidated(package: &mut OpcPackage, project_part: &str) -> Result<()> {
    let state = attached_vba_signature_state(package, project_part)?;
    if state.is_none() || state == Some(true) {
        return Ok(());
    }
    let target = package
        .get_part_rels(project_part)
        .and_then(|relationships| {
            relationships
                .items
                .iter()
                .find(|relationship| is_vba_signature(relationship))
        })
        .map(|relationship| relationship.target.clone())
        .ok_or_else(|| {
            invalid(
                "invalidate VBA signature",
                format!("{project_part}: signature relationship disappeared"),
            )
        })?;
    package
        .get_or_create_part_rels(project_part)
        .add(INVALIDATED_VBA_SIGNATURE, &target);
    Ok(())
}

#[derive(Debug)]
struct PackageSignatureGraph {
    origins: Vec<(String, String)>,
    signatures: Vec<(String, String)>,
}

fn package_signature_graph(package: &OpcPackage) -> Result<Option<PackageSignatureGraph>> {
    ensure_unique_relationship_ids("/", &package.package_rels.items)?;
    let origins = package
        .package_rels
        .items
        .iter()
        .filter(|relationship| relationship.rel_type == rel_types::DIGITAL_SIGNATURE_ORIGIN)
        .collect::<Vec<_>>();
    if package
        .package_rels
        .items
        .iter()
        .any(|relationship| relationship.rel_type == rel_types::DIGITAL_SIGNATURE)
    {
        return Err(invalid(
            "inventory embedded content",
            "/: digital-signature relationship is outside an origin part".to_owned(),
        ));
    }
    if origins.len() > 1 {
        return Err(invalid(
            "inventory embedded content",
            format!(
                "/: found {} digital-signature origins, expected at most one",
                origins.len()
            ),
        ));
    }
    let markers = package
        .package_rels
        .items
        .iter()
        .filter(|relationship| relationship.rel_type == INVALIDATED_PACKAGE_SIGNATURE)
        .collect::<Vec<_>>();
    if markers.len() > 1 {
        return Err(invalid(
            "inventory embedded content",
            format!(
                "/: found {} package-signature invalidation markers, expected at most one",
                markers.len()
            ),
        ));
    }
    let Some(origin) = origins.first() else {
        if !markers.is_empty() {
            return Err(invalid(
                "inventory embedded content",
                "/: package-signature invalidation marker has no origin".to_owned(),
            ));
        }
        for (source_part, relationships) in &package.part_rels {
            if relationships.items.iter().any(|relationship| {
                matches!(
                    relationship.rel_type.as_str(),
                    rel_types::DIGITAL_SIGNATURE_ORIGIN | rel_types::DIGITAL_SIGNATURE
                )
            }) {
                return Err(invalid(
                    "inventory embedded content",
                    format!("{source_part}: misplaced digital-signature relationship"),
                ));
            }
        }
        return Ok(None);
    };
    reject_part_identity_collisions(package)?;
    let origin_part = safe_internal_target("/", origin)?;
    let origin_identity = stored_part_identity(&origin_part);
    if let Some(marker) = markers.first()
        && stored_part_identity(&safe_internal_target("/", marker)?) != origin_identity
    {
        return Err(invalid(
            "inventory embedded content",
            format!("/: package-signature invalidation marker does not target {origin_part}"),
        ));
    }
    for (source_part, relationships) in &package.part_rels {
        if relationships.items.iter().any(|relationship| {
            matches!(
                relationship.rel_type.as_str(),
                rel_types::DIGITAL_SIGNATURE_ORIGIN | rel_types::DIGITAL_SIGNATURE
            )
        }) {
            ensure_unique_relationship_ids(source_part, &relationships.items)?;
        }
        if relationships
            .items
            .iter()
            .any(|relationship| relationship.rel_type == rel_types::DIGITAL_SIGNATURE_ORIGIN)
        {
            return Err(invalid(
                "inventory embedded content",
                format!("{source_part}: misplaced digital-signature origin relationship"),
            ));
        }
        if stored_part_identity(source_part) != origin_identity
            && relationships
                .items
                .iter()
                .any(|relationship| relationship.rel_type == rel_types::DIGITAL_SIGNATURE)
        {
            return Err(invalid(
                "inventory embedded content",
                format!("{source_part}: misplaced digital-signature relationship"),
            ));
        }
    }
    required_part(package, &origin_part)?;
    require_exact_content_type(
        package,
        &origin_part,
        PACKAGE_SIGNATURE_ORIGIN_CONTENT_TYPE,
        "package signature origin",
    )?;
    let origin_relationships = package.get_part_rels(&origin_part).ok_or_else(|| {
        invalid(
            "inventory embedded content",
            format!("{origin_part}: digital-signature origin has no relationship set"),
        )
    })?;
    ensure_unique_relationship_ids(&origin_part, &origin_relationships.items)?;
    if let Some(unrelated) = origin_relationships
        .items
        .iter()
        .find(|relationship| relationship.rel_type != rel_types::DIGITAL_SIGNATURE)
    {
        return Err(invalid(
            "inventory embedded content",
            format!(
                "{origin_part}: digital-signature origin has unrelated relationship {}",
                unrelated.id
            ),
        ));
    }
    let signatures = origin_relationships
        .items
        .iter()
        .filter(|relationship| relationship.rel_type == rel_types::DIGITAL_SIGNATURE)
        .collect::<Vec<_>>();
    if signatures.is_empty() {
        return Err(invalid(
            "inventory embedded content",
            format!("{origin_part}: digital-signature origin has no signature relationship"),
        ));
    }
    let mut signature_parts = HashSet::new();
    let mut graph = PackageSignatureGraph {
        origins: vec![(origin.id.clone(), origin_part.clone())],
        signatures: Vec::new(),
    };
    for signature in signatures {
        let signature_part = safe_internal_target(&origin_part, signature)?;
        if !signature_parts.insert(stored_part_identity(&signature_part)) {
            return Err(invalid(
                "inventory embedded content",
                format!("duplicate digital-signature target {signature_part}"),
            ));
        }
        required_part(package, &signature_part)?;
        require_exact_content_type(
            package,
            &signature_part,
            PACKAGE_SIGNATURE_XML_CONTENT_TYPE,
            "package XML signature",
        )?;
        graph.signatures.push((origin_part.clone(), signature_part));
    }
    reject_unrelated_signature_incoming(package, &graph)?;
    Ok(Some(graph))
}

fn require_exact_content_type(
    package: &OpcPackage,
    part: &str,
    expected: &str,
    role: &str,
) -> Result<()> {
    match package.content_types.content_type_for(part) {
        Some(actual) if actual == expected => Ok(()),
        Some(actual) => Err(invalid(
            "inventory embedded content",
            format!("{part}: {role} has content type {actual}, expected {expected}"),
        )),
        None => Err(invalid(
            "inventory embedded content",
            format!("{part}: {role} has no content type, expected {expected}"),
        )),
    }
}

fn reject_unrelated_signature_incoming(
    package: &OpcPackage,
    graph: &PackageSignatureGraph,
) -> Result<()> {
    let origin_part = stored_part_identity(&graph.origins[0].1);
    let signature_parts = graph
        .signatures
        .iter()
        .map(|(_, part)| stored_part_identity(part))
        .collect::<HashSet<_>>();
    for target in internal_targets(package)? {
        let relationship = target.relationship;
        let reaches_origin = target.identities.contains(&origin_part);
        let reaches_signature = target
            .identities
            .iter()
            .any(|identity| signature_parts.contains(identity));
        let refused = if target.from_package {
            let allowed_origin = reaches_origin
                && matches!(
                    relationship.rel_type.as_str(),
                    rel_types::DIGITAL_SIGNATURE_ORIGIN | INVALIDATED_PACKAGE_SIGNATURE
                );
            (reaches_origin && !allowed_origin) || reaches_signature
        } else {
            let allowed_signature = stored_part_identity(target.source_part) == origin_part
                && relationship.rel_type == rel_types::DIGITAL_SIGNATURE
                && reaches_signature;
            reaches_origin || (reaches_signature && !allowed_signature)
        };
        if refused {
            return Err(invalid(
                "inventory embedded content",
                format!(
                    "{}: unrelated relationship {} targets package signature part {}",
                    target.source_part,
                    relationship.id,
                    OpcPackage::resolve_rel_target(target.source_part, &relationship.target)
                ),
            ));
        }
    }
    Ok(())
}

/// Whether the package carries a package signature, after the checks of its
/// signature graph.
pub fn has_package_signature(package: &OpcPackage) -> Result<bool> {
    package_signature_graph(package).map(|graph| graph.is_some())
}

fn package_signature_invalidation_marked(package: &OpcPackage) -> Result<bool> {
    package_signature_graph(package)?;
    Ok(package
        .package_rels
        .items
        .iter()
        .any(|relationship| relationship.rel_type == INVALIDATED_PACKAGE_SIGNATURE))
}

fn ensure_unique_relationship_ids(source_part: &str, relationships: &[Relationship]) -> Result<()> {
    let mut seen = HashSet::new();
    if let Some(duplicate) = relationships
        .iter()
        .map(|relationship| relationship.id.as_str())
        .find(|id| !seen.insert(*id))
    {
        return Err(invalid(
            "resolve embedded content",
            format!("{source_part}: duplicate relationship id {duplicate}"),
        ));
    }
    Ok(())
}

fn remove_package_signatures(package: &mut OpcPackage) -> Result<()> {
    let Some(graph) = package_signature_graph(package)? else {
        return Ok(());
    };
    package.package_rels.items.retain(|relationship| {
        relationship.rel_type != rel_types::DIGITAL_SIGNATURE_ORIGIN
            && relationship.rel_type != INVALIDATED_PACKAGE_SIGNATURE
    });
    for (_, signature_part) in graph.signatures {
        package.remove_part(&signature_part);
        package.remove_part_rels(&signature_part);
        package.content_types.remove_override(&signature_part);
    }
    for (_, origin_part) in graph.origins {
        package.remove_part(&origin_part);
        package.remove_part_rels(&origin_part);
        package.content_types.remove_override(&origin_part);
    }
    Ok(())
}

fn remove_vba_signatures(package: &mut OpcPackage, project_part: &str) -> Result<()> {
    attached_vba_signature_state(package, project_part)?;
    let signatures = package
        .get_part_rels(project_part)
        .map(|relationships| {
            relationships
                .items
                .iter()
                .filter(|relationship| is_vba_signature(relationship))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(relationships) = package.get_part_rels_mut(project_part) {
        relationships.items.retain(|relationship| {
            !is_vba_signature(relationship) && relationship.rel_type != INVALIDATED_VBA_SIGNATURE
        });
    }
    for signature in signatures {
        if relationship_is_internal(&signature) {
            let signature_part = safe_internal_target(project_part, &signature)?;
            delete_if_unreachable(package, &signature_part)?;
        }
    }
    Ok(())
}

fn retain_vba_signature_parts_as_evidence(package: &mut OpcPackage, project_part: &str) {
    if let Some(relationships) = package.get_part_rels_mut(project_part) {
        relationships.items.retain(|relationship| {
            !is_vba_signature(relationship) && relationship.rel_type != INVALIDATED_VBA_SIGNATURE
        });
    }
}

#[derive(Debug)]
struct SignatureReference {
    path: String,
    relationship_ids: Vec<String>,
}

fn signature_manifest_has_missing_reference(package: &OpcPackage) -> bool {
    let Some(graph) = package_signature_graph(package).ok().flatten() else {
        return false;
    };
    graph.signatures.iter().any(|(_, signature_part)| {
        required_part(package, signature_part)
            .ok()
            .and_then(|xml| signature_references(xml).ok())
            .is_some_and(|references| {
                references.into_iter().any(|reference| {
                    if signature_reference_is_package_manifest(&reference.path) {
                        return false;
                    }
                    if let Some(source) = relationship_source_from_path(&reference.path) {
                        let Some(relationships) = package.get_part_rels(&source) else {
                            return true;
                        };
                        return reference.relationship_ids.iter().any(|id| {
                            relationships
                                .items
                                .iter()
                                .filter(|relationship| relationship.id == *id)
                                .count()
                                != 1
                        });
                    }
                    !package.contains_part(&reference.path)
                })
            })
    })
}

fn signature_reference_is_package_manifest(path: &str) -> bool {
    path.eq_ignore_ascii_case("/[Content_Types].xml") || path.eq_ignore_ascii_case("/_rels/.rels")
}

fn signature_references(xml: &[u8]) -> Result<Vec<SignatureReference>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut references = Vec::new();
    let mut active_reference = None::<(usize, SignatureReference)>;
    let mut depth = 0usize;
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| invalid("inspect package signature", error.to_string()))?;
        let is_dsig = namespace_is(&namespace, &[DSIG_NS]);
        let is_opc_signature = namespace_is(&namespace, &[OPC_SIGNATURE_NS]);
        let event = event.into_owned();
        match event {
            Event::Start(element) => {
                if is_dsig
                    && local_name(element.name().as_ref()) == b"Reference"
                    && let Some(uri) = unqualified_attribute(&element, b"URI")?
                    && uri.starts_with('/')
                {
                    let path = uri.split('?').next().unwrap_or(&uri);
                    active_reference = Some((
                        depth,
                        SignatureReference {
                            path: percent_decode_path(path).ok_or_else(|| {
                                invalid(
                                    "inspect package signature",
                                    format!("invalid package reference URI {uri}"),
                                )
                            })?,
                            relationship_ids: Vec::new(),
                        },
                    ));
                }
                depth = depth.checked_add(1).ok_or_else(|| {
                    invalid(
                        "inspect package signature",
                        "signature XML nesting is too deep".to_owned(),
                    )
                })?;
            }
            Event::Empty(element) => {
                if is_dsig
                    && local_name(element.name().as_ref()) == b"Reference"
                    && let Some(uri) = unqualified_attribute(&element, b"URI")?
                    && uri.starts_with('/')
                {
                    let path = uri.split('?').next().unwrap_or(&uri);
                    references.push(SignatureReference {
                        path: percent_decode_path(path).ok_or_else(|| {
                            invalid(
                                "inspect package signature",
                                format!("invalid package reference URI {uri}"),
                            )
                        })?,
                        relationship_ids: Vec::new(),
                    });
                } else if is_opc_signature
                    && local_name(element.name().as_ref()) == b"RelationshipReference"
                    && let Some((_, reference)) = active_reference.as_mut()
                    && let Some(id) = unqualified_attribute(&element, b"SourceId")?
                {
                    reference.relationship_ids.push(id);
                }
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    invalid(
                        "inspect package signature",
                        "signature XML has an unmatched closing element".to_owned(),
                    )
                })?;
                if active_reference
                    .as_ref()
                    .is_some_and(|(reference_depth, _)| *reference_depth == depth)
                    && let Some((_, reference)) = active_reference.take()
                {
                    references.push(reference);
                }
            }
            Event::Eof if depth == 0 => return Ok(references),
            Event::Eof => {
                return Err(invalid(
                    "inspect package signature",
                    "signature XML ended before its root closed".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn unqualified_attribute(element: &BytesStart<'_>, name: &[u8]) -> Result<Option<String>> {
    for attribute in element.attributes() {
        let attribute =
            attribute.map_err(|error| invalid("scan embedded XML", error.to_string()))?;
        if attribute.key.as_ref() == name {
            return attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .map(|value| Some(value.into_owned()))
                .map_err(|error| invalid("scan embedded XML", error.to_string()));
        }
    }
    Ok(None)
}

fn relationship_source_from_path(path: &str) -> Option<String> {
    let marker = "/_rels/";
    let identity = path.to_ascii_lowercase();
    let marker_index = identity.rfind(marker)?;
    let filename = path.get(marker_index + marker.len()..)?;
    if !filename
        .get(filename.len().saturating_sub(".rels".len())..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".rels"))
    {
        return None;
    }
    let filename = &filename[..filename.len() - ".rels".len()];
    Some(format!("{}{filename}", &path[..marker_index + 1]))
}

fn percent_decode_path(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex_digit(*bytes.get(index + 1)?)?;
            let low = hex_digit(*bytes.get(index + 2)?)?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Whether two packages hold the same parts, content types and
/// relationships: a save between them keeps a signature valid.
pub fn packages_are_semantically_equal(left: &OpcPackage, right: &OpcPackage) -> bool {
    left.parts == right.parts
        && left.content_types.defaults == right.content_types.defaults
        && left.content_types.overrides == right.content_types.overrides
        && left.package_rels.items == right.package_rels.items
        && left.part_rels.len() == right.part_rels.len()
        && left.part_rels.iter().all(|(source, relationships)| {
            right
                .part_rels
                .get(source)
                .is_some_and(|other| relationships.items == other.items)
        })
}

/// Whether a mutation from `original` to `candidate`, both signed,
/// invalidates the package signature.
pub fn synchronized_package_mutation_invalidates_signature(
    original: &OpcPackage,
    candidate: &OpcPackage,
) -> bool {
    let original_signed = original
        .package_rels
        .items
        .iter()
        .any(|relationship| relationship.rel_type == rel_types::DIGITAL_SIGNATURE_ORIGIN);
    let candidate_signed = candidate
        .package_rels
        .items
        .iter()
        .any(|relationship| relationship.rel_type == rel_types::DIGITAL_SIGNATURE_ORIGIN);
    original_signed && candidate_signed && !packages_are_semantically_equal(original, candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "/_xmlsignatures/origin.sigs";
    const SIGNATURE: &str = "/_xmlsignatures/sig1.xml";
    const MAIN: &str = "/word/document.xml";

    /// A package whose signature graph passes its checks: an origin, one
    /// signature, and a main part with a VBA project.
    fn signed_package() -> OpcPackage {
        let mut package = OpcPackage::new();
        for (part, content_type) in [
            (ORIGIN, PACKAGE_SIGNATURE_ORIGIN_CONTENT_TYPE),
            (SIGNATURE, PACKAGE_SIGNATURE_XML_CONTENT_TYPE),
            (MAIN, "application/xml"),
            (
                "/word/vbaProject.bin",
                "application/vnd.ms-office.vbaProject",
            ),
            ("/word/media/image 1.png", "image/png"),
        ] {
            package.set_part(part, b"part".to_vec());
            package.content_types.add_override(part, content_type);
        }
        package.package_rels.add_with_id(
            "rIdOrigin",
            rel_types::DIGITAL_SIGNATURE_ORIGIN,
            "_xmlsignatures/origin.sigs",
        );
        package.get_or_create_part_rels(ORIGIN).add_with_id(
            "rId1",
            rel_types::DIGITAL_SIGNATURE,
            "sig1.xml",
        );
        package.get_or_create_part_rels(MAIN).add_with_id(
            "rIdVba",
            rel_types::VBA_PROJECT,
            "vbaProject.bin",
        );
        package
    }

    fn add_relationship(package: &mut OpcPackage, target: &str) {
        package.get_or_create_part_rels(MAIN).add_with_id(
            "rIdOther",
            "http://example.com/relationships/other",
            target,
        );
    }

    fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
        result.expect_err("refused").to_string()
    }

    #[test]
    fn a_signed_package_passes_its_graph_checks() {
        assert_eq!(has_package_signature(&signed_package()), Ok(true));
    }

    #[test]
    fn an_odd_target_to_an_ordinary_part_is_accepted() {
        let mut package = signed_package();
        add_relationship(&mut package, "media/image 1.png");
        assert_eq!(has_package_signature(&package), Ok(true));
        assert_eq!(
            relationship_target_is_reachable(&package, "/word/media/image%201.png"),
            Ok(true)
        );
    }

    #[test]
    fn a_target_that_names_no_part_is_refused() {
        for target in ["media%2Fimage.png", "../../image.png", "media/a%FF.png"] {
            let mut package = signed_package();
            add_relationship(&mut package, target);
            let error = refusal(has_package_signature(&package));
            assert!(
                error.contains("rIdOther has an unsafe internal target"),
                "{target}: {error}"
            );
        }
    }

    #[test]
    fn an_odd_target_to_a_protected_part_is_refused() {
        for target in [
            "vbaProject%2Ebin",
            "..\\word\\vbaProject.bin",
            "vbaProject.bin#x",
        ] {
            let mut package = signed_package();
            add_relationship(&mut package, target);
            let error = refusal(relationship_target_is_reachable(
                &package,
                "/word/other.xml",
            ));
            assert!(
                error.contains("rIdOther has an unsafe internal target"),
                "{target}: {error}"
            );
        }
    }

    #[test]
    fn an_unrelated_relationship_naming_the_origin_by_any_alias_is_refused() {
        for target in [
            "../_xmlsignatures/origin.sigs",
            "../_XMLSIGNATURES/ORIGIN.SIGS",
            "/_XmlSignatures/Origin.Sigs",
        ] {
            let mut package = signed_package();
            add_relationship(&mut package, target);
            let error = refusal(has_package_signature(&package));
            assert!(
                error.contains("unrelated relationship rIdOther targets package signature part"),
                "{target}: {error}"
            );
        }
    }

    #[test]
    fn reachability_ignores_case_and_escapes() {
        let mut package = signed_package();
        add_relationship(&mut package, "MEDIA/IMAGE%201.PNG");
        assert_eq!(
            relationship_target_is_reachable(&package, "/word/media/image 1.png"),
            Ok(true)
        );
        assert_eq!(
            relationship_target_is_reachable(&package, "/word/media/image2.png"),
            Ok(false)
        );
    }

    #[test]
    fn two_parts_that_name_one_part_once_decoded_are_refused_on_a_signed_package_only() {
        let mut package = signed_package();
        package.set_part("/word/media/image%201.png", b"other".to_vec());
        let error = refusal(has_package_signature(&package));
        assert!(error.contains("ambiguous part names"), "{error}");
        assert!(error.contains("/word/media/image 1.png"), "{error}");
        assert!(error.contains("/word/media/image%201.png"), "{error}");

        let mut unsigned = OpcPackage::new();
        unsigned.set_part("/word/media/image 1.png", b"one".to_vec());
        unsigned.set_part("/word/media/image%201.png", b"two".to_vec());
        assert_eq!(has_package_signature(&unsigned), Ok(false));
        assert!(refusal(reject_part_identity_collisions(&unsigned)).contains("ambiguous"));
    }
}
