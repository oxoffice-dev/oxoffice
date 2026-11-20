//! OPC digital-signature discovery, verification, and coverage reporting.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use ring::rand::SystemRandom;
use ring::signature::{
    KeyPair, RSA_PKCS1_2048_8192_SHA256, RSA_PKCS1_SHA256, RsaKeyPair, UnparsedPublicKey,
};
use sha2::{Digest, Sha256};
use x509_cert::Certificate;
use x509_cert::der::Decode;

use crate::error::{OpcError, Result};
use crate::package::OpcPackage;
use crate::relationship::{Relationship, Relationships, rel_types};

const DSIG_NS: &str = "http://www.w3.org/2000/09/xmldsig#";
const C14N_EXCLUSIVE: &str = "http://www.w3.org/2001/10/xml-exc-c14n#";
const DIGEST_SHA256: &str = "http://www.w3.org/2001/04/xmlenc#sha256";
const SIGNATURE_RSA_SHA256: &str = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256";
const RELATIONSHIP_TRANSFORM: &str =
    "http://schemas.openxmlformats.org/package/2006/RelationshipTransform";
const OPC_SIGNATURE_NS: &str = "http://schemas.openxmlformats.org/package/2006/digital-signature";
const RELATIONSHIPS_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const SIGNATURE_ORIGIN_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-package.digital-signature-origin";
const SIGNATURE_XML_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-package.digital-signature-xmlsignature+xml";

/// The signer identity embedded in an X.509 certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignerCertificateIdentity {
    pub subject: String,
    pub issuer: String,
    pub serial_number: String,
}

/// One relationship authenticated by an OPC relationship transform.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CoveredRelationship {
    /// `/` for package relationships, otherwise the owning part name.
    pub source_part: String,
    pub relationship_id: String,
}

/// A precise reason why a signature is invalid or incomplete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureIssue {
    MissingPart {
        uri: String,
    },
    ChangedReference {
        uri: String,
        expected_digest: String,
        actual_digest: String,
    },
    DuplicateReference {
        uri: String,
    },
    MissingRelationship {
        source_part: String,
        relationship_id: String,
    },
    DuplicateRelationship {
        source_part: String,
        relationship_id: String,
    },
    ExternalRelationship {
        source_part: String,
        relationship_id: String,
        target: String,
    },
    MissingRelationshipTarget {
        source_part: String,
        relationship_id: String,
        target: String,
    },
    UncoveredPart {
        part_name: String,
    },
    UncoveredRelationship {
        source_part: String,
        relationship_id: String,
    },
    SignatureValueMismatch,
    OrphanSignaturePart {
        part_name: String,
    },
}

/// Verification and declared-coverage result for one signature part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureReport {
    pub signature_part: String,
    pub signer: Option<SignerCertificateIdentity>,
    /// True only when `SignedInfo`, its references, and authenticated manifest references verify.
    pub cryptographically_valid: bool,
    /// True only when every non-signature package part and relationship is covered.
    pub coverage_complete: bool,
    pub covered_parts: Vec<String>,
    pub covered_relationships: Vec<CoveredRelationship>,
    pub issues: Vec<SignatureIssue>,
}

#[derive(Debug, Clone)]
struct XmlName {
    qname: String,
    prefix: String,
    local: String,
    namespace: String,
}

#[derive(Debug, Clone)]
struct XmlAttribute {
    name: XmlName,
    value: String,
}

#[derive(Debug, Clone)]
enum XmlNode {
    Element(XmlElement),
    Text(String),
    ProcessingInstruction { target: String, content: String },
}

#[derive(Debug, Clone)]
struct XmlElement {
    name: XmlName,
    attributes: Vec<XmlAttribute>,
    children: Vec<XmlNode>,
    namespaces: BTreeMap<String, String>,
}

#[derive(Debug)]
enum ReferenceTransform {
    ExclusiveCanonicalization,
    Relationships(Vec<String>),
}

#[derive(Debug)]
struct ReferenceSpec {
    uri: String,
    digest: Vec<u8>,
    transforms: Vec<ReferenceTransform>,
}

// Bound both the original envelope and decoded allocation before parsing ASN.1.
const SIGNING_MATERIAL_LIMIT: usize = 1024 * 1024;

fn signing_material_der(
    input: &[u8],
    expected_label: &str,
) -> std::result::Result<Vec<u8>, String> {
    if input.is_empty() || input.len() > SIGNING_MATERIAL_LIMIT {
        return Err("signing material must contain between 1 byte and 1 MiB".into());
    }
    // DER is binary. Its bytes, including trailing bytes, reach the strict DER parser unchanged.
    let trimmed = input.trim_ascii();
    if !trimmed.starts_with(b"-----") {
        return Ok(input.to_vec());
    }
    if !input.is_ascii() {
        return Err("PEM framing must be ASCII".into());
    }
    let begin = format!("-----BEGIN {expected_label}-----");
    let end = format!("-----END {expected_label}-----");
    let body = trimmed
        .strip_prefix(begin.as_bytes())
        .and_then(|rest| rest.strip_suffix(end.as_bytes()))
        .ok_or_else(|| "expected exactly one matching unencrypted PEM block".to_owned())?;
    let encoded: Vec<u8> = body
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if encoded.is_empty() {
        return Err("empty PEM body".into());
    }
    BASE64
        .decode(encoded)
        .map_err(|_| "invalid padded PEM base64".into())
}

pub(crate) fn create_signature(
    package: &mut OpcPackage,
    private_key_pkcs8_der: &[u8],
    certificate_der: &[u8],
) -> Result<SignatureReport> {
    package.validate_graph()?;
    let certificate_der = signing_material_der(certificate_der, "CERTIFICATE")
        .map_err(OpcError::InvalidSigningCertificate)?;
    let private_key_pkcs8_der = signing_material_der(private_key_pkcs8_der, "PRIVATE KEY")
        .map_err(OpcError::InvalidSigningKey)?;
    let certificate = Certificate::from_der(&certificate_der)
        .map_err(|error| OpcError::InvalidSigningCertificate(error.to_string()))?;
    let subject_public_key_info = certificate.tbs_certificate().subject_public_key_info();
    let key_algorithm = subject_public_key_info.algorithm.oid.to_string();
    if key_algorithm != "1.2.840.113549.1.1.1" {
        return Err(unsupported("key algorithm", &key_algorithm));
    }
    let certificate_public_key = subject_public_key_info
        .subject_public_key
        .as_bytes()
        .ok_or_else(|| {
            OpcError::InvalidSigningCertificate("non-octet-aligned public key".into())
        })?;
    let private_key = RsaKeyPair::from_pkcs8(&private_key_pkcs8_der)
        .map_err(|error| OpcError::InvalidSigningKey(error.to_string()))?;
    if private_key.public_key().as_ref() != certificate_public_key {
        return Err(OpcError::InvalidSigningKey(
            "private key does not match certificate".into(),
        ));
    }
    validate_unsigned_package_relationships(package)?;

    let origin_part = allocate_origin_part(package)?;
    let signature_part = allocate_numbered_part(package, "/_xmlsignatures", "sig", "xml")?;
    package.set_part(&origin_part, Vec::new());
    package
        .content_types
        .add_override(&origin_part, SIGNATURE_ORIGIN_CONTENT_TYPE);
    package
        .content_types
        .add_override(&signature_part, SIGNATURE_XML_CONTENT_TYPE);
    package.package_rels.add(
        rel_types::DIGITAL_SIGNATURE_ORIGIN,
        origin_part.trim_start_matches('/'),
    );
    let signature_target = signature_part
        .strip_prefix("/_xmlsignatures/")
        .ok_or_else(|| {
            OpcError::SignatureCreationFailed("signature part is outside its origin".into())
        })?;
    package
        .get_or_create_part_rels(&origin_part)
        .add(rel_types::DIGITAL_SIGNATURE, signature_target);

    let manifest = create_manifest(package)?;
    let signature_time = current_signature_time()?;
    let object = format!(
        "<ds:Object xmlns:ds=\"{DSIG_NS}\" xmlns:mdssi=\"{OPC_SIGNATURE_NS}\" Id=\"idPackageObject\"><ds:Manifest>{manifest}</ds:Manifest><ds:SignatureProperties><ds:SignatureProperty Id=\"idSignatureTime\" Target=\"#idPackageSignature\"><mdssi:SignatureTime><mdssi:Format>YYYY-MM-DDThh:mm:ssTZD</mdssi:Format><mdssi:Value>{signature_time}</mdssi:Value></mdssi:SignatureTime></ds:SignatureProperty></ds:SignatureProperties></ds:Object>"
    );
    let object_element = parse_xml(object.as_bytes())?;
    let object_digest = BASE64.encode(Sha256::digest(canonicalize(&object_element)));
    let signed_info = format!(
        "<ds:SignedInfo xmlns:ds=\"{DSIG_NS}\"><ds:CanonicalizationMethod Algorithm=\"{C14N_EXCLUSIVE}\"></ds:CanonicalizationMethod><ds:SignatureMethod Algorithm=\"{SIGNATURE_RSA_SHA256}\"></ds:SignatureMethod><ds:Reference URI=\"#idPackageObject\"><ds:Transforms><ds:Transform Algorithm=\"{C14N_EXCLUSIVE}\"></ds:Transform></ds:Transforms><ds:DigestMethod Algorithm=\"{DIGEST_SHA256}\"></ds:DigestMethod><ds:DigestValue>{object_digest}</ds:DigestValue></ds:Reference></ds:SignedInfo>"
    );
    let signed_info_element = parse_xml(signed_info.as_bytes())?;
    let signed_info_canonical = canonicalize(&signed_info_element);
    let mut signature = vec![0_u8; private_key.public().modulus_len()];
    private_key
        .sign(
            &RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            &signed_info_canonical,
            &mut signature,
        )
        .map_err(|_| OpcError::SignatureCreationFailed("RSA-SHA256 signing failed".into()))?;
    let signature_value = BASE64.encode(signature);
    let certificate_value = BASE64.encode(certificate_der);
    let signature_xml = format!(
        "<?xml version=\"1.0\"?><ds:Signature xmlns:ds=\"{DSIG_NS}\" xmlns:mdssi=\"{OPC_SIGNATURE_NS}\" Id=\"idPackageSignature\">{signed_info}<ds:SignatureValue>{signature_value}</ds:SignatureValue><ds:KeyInfo><ds:X509Data><ds:X509Certificate>{certificate_value}</ds:X509Certificate></ds:X509Data></ds:KeyInfo>{object}</ds:Signature>"
    );
    package.set_part(&signature_part, signature_xml.into_bytes());

    let reports = verify_signatures(package)?;
    if let Some(invalid) = reports
        .iter()
        .find(|report| !report.cryptographically_valid || !report.coverage_complete)
    {
        return Err(OpcError::SignatureCreationFailed(format!(
            "staged verification rejected {} with {:?}",
            invalid.signature_part, invalid.issues
        )));
    }
    let report = reports
        .into_iter()
        .find(|report| report.signature_part == signature_part)
        .ok_or_else(|| {
            OpcError::SignatureCreationFailed("staged signature was not discoverable".into())
        })?;
    Ok(report)
}

fn create_manifest(package: &OpcPackage) -> Result<String> {
    let infrastructure = signature_infrastructure_parts(package);
    let mut references = String::new();
    append_reference(
        &mut references,
        "/%5BContent_Types%5D.xml",
        &content_types_bytes(package)?,
        "",
    );

    let mut part_names: Vec<&String> = package
        .parts
        .keys()
        .filter(|part_name| !infrastructure.contains(&crate::package::part_identity(part_name)))
        .collect();
    part_names.sort();
    for part_name in part_names {
        let content_type = package
            .content_types
            .content_type_for(part_name)
            .ok_or_else(|| {
                OpcError::SignatureCreationFailed(format!(
                    "signed part has no content type: {part_name}"
                ))
            })?;
        append_reference(
            &mut references,
            &reference_uri_with_content_type(part_name, content_type),
            &package.parts[part_name],
            "",
        );
    }

    append_relationship_reference(&mut references, package, "/", &package.package_rels)?;
    let mut sources: Vec<&String> = package.part_rels.keys().collect();
    sources.sort();
    for source in sources {
        validate_relationship_source(package, source)?;
        append_relationship_reference(
            &mut references,
            package,
            source,
            &package.part_rels[source],
        )?;
    }
    Ok(references)
}

fn validate_relationship_source(package: &OpcPackage, source: &str) -> Result<()> {
    let normalized = source.starts_with('/')
        && source.len() > 1
        && !source.ends_with('/')
        && !source.contains("//")
        && !source
            .split('/')
            .any(|segment| matches!(segment, "." | ".."));
    if normalized && package.contains_part(source) {
        return Ok(());
    }
    Err(OpcError::SignatureCreationFailed(format!(
        "relationship source is not an existing normalized package part: {source:?}"
    )))
}

fn validate_unsigned_package_relationships(package: &OpcPackage) -> Result<()> {
    validate_unsigned_relationship_set(package, "/", &package.package_rels)?;
    let mut sources: Vec<&String> = package.part_rels.keys().collect();
    sources.sort();
    for source in sources {
        validate_relationship_source(package, source)?;
        validate_unsigned_relationship_set(package, source, &package.part_rels[source])?;
    }
    Ok(())
}

fn validate_unsigned_relationship_set(
    package: &OpcPackage,
    source: &str,
    relationships: &Relationships,
) -> Result<()> {
    if relationships.items.iter().any(|relationship| {
        relationship.rel_type == rel_types::DIGITAL_SIGNATURE_ORIGIN
            || relationship.rel_type == rel_types::DIGITAL_SIGNATURE
    }) {
        return Err(OpcError::SignatureCreationFailed(format!(
            "unsigned package contains a digital-signature relationship from {source}"
        )));
    }
    let ids: Vec<String> = relationships
        .items
        .iter()
        .map(|relationship| relationship.id.clone())
        .collect();
    let mut covered = BTreeSet::new();
    let mut issues = Vec::new();
    relationship_transform(
        package,
        source,
        relationships,
        &ids,
        &mut covered,
        &mut issues,
    )
    .ok_or_else(|| {
        OpcError::SignatureCreationFailed(format!(
            "invalid relationship graph before signing: {issues:?}"
        ))
    })?;
    Ok(())
}

fn current_signature_time() -> Result<String> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| OpcError::SignatureCreationFailed(error.to_string()))?
        .as_secs();
    let days = i64::try_from(seconds / 86_400)
        .map_err(|error| OpcError::SignatureCreationFailed(error.to_string()))?;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_date_from_unix_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = seconds_of_day % 3_600 / 60;
    let second = seconds_of_day % 60;
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

fn civil_date_from_unix_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

fn append_relationship_reference(
    output: &mut String,
    package: &OpcPackage,
    source: &str,
    relationships: &Relationships,
) -> Result<()> {
    let mut ids: Vec<String> = relationships
        .items
        .iter()
        .filter(|relationship| {
            relationship.rel_type != rel_types::DIGITAL_SIGNATURE_ORIGIN
                && relationship.rel_type != rel_types::DIGITAL_SIGNATURE
        })
        .map(|relationship| relationship.id.clone())
        .collect();
    ids.sort();
    if ids.is_empty() {
        return Ok(());
    }

    let mut covered = BTreeSet::new();
    let mut issues = Vec::new();
    let transformed = relationship_transform(
        package,
        source,
        relationships,
        &ids,
        &mut covered,
        &mut issues,
    )
    .ok_or_else(|| {
        OpcError::SignatureCreationFailed(format!(
            "cannot authenticate relationships for {source}: {issues:?}"
        ))
    })?;
    let uri = if source == "/" {
        "/_rels/.rels".to_string()
    } else {
        relationship_part_name(source)
    };
    let uri = reference_uri_with_content_type(&uri, crate::content_types::RELATIONSHIPS);
    let mut transforms =
        format!("<ds:Transforms><ds:Transform Algorithm=\"{RELATIONSHIP_TRANSFORM}\">");
    for id in ids {
        transforms.push_str("<mdssi:RelationshipReference SourceId=\"");
        escape_attribute(&id, &mut transforms);
        transforms.push_str("\"></mdssi:RelationshipReference>");
    }
    transforms.push_str(&format!(
        "</ds:Transform><ds:Transform Algorithm=\"{C14N_EXCLUSIVE}\"></ds:Transform></ds:Transforms>"
    ));
    append_reference(output, &uri, &transformed, &transforms);
    Ok(())
}

fn append_reference(output: &mut String, uri: &str, bytes: &[u8], transforms: &str) {
    output.push_str("<ds:Reference URI=\"");
    escape_attribute(uri, output);
    output.push_str("\">");
    output.push_str(transforms);
    output.push_str("<ds:DigestMethod Algorithm=\"");
    output.push_str(DIGEST_SHA256);
    output.push_str("\"></ds:DigestMethod><ds:DigestValue>");
    output.push_str(&BASE64.encode(Sha256::digest(bytes)));
    output.push_str("</ds:DigestValue></ds:Reference>");
}

fn relationship_part_name(source: &str) -> String {
    let split = source.rfind('/').unwrap_or(0);
    let directory = &source[..=split];
    let filename = &source[split + 1..];
    format!("{directory}_rels/{filename}.rels")
}

fn percent_encode_uri(uri: &str) -> String {
    let mut encoded = String::with_capacity(uri.len());
    for byte in uri.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(b"0123456789ABCDEF"[(byte >> 4) as usize]));
            encoded.push(char::from(b"0123456789ABCDEF"[(byte & 0x0f) as usize]));
        }
    }
    encoded
}

fn reference_uri_with_content_type(part_name: &str, content_type: &str) -> String {
    format!(
        "{}?ContentType={content_type}",
        percent_encode_uri(part_name)
    )
}

fn allocate_origin_part(package: &OpcPackage) -> Result<String> {
    let conventional = "/_xmlsignatures/origin.sigs";
    if !part_name_occupied(package, conventional) {
        return Ok(conventional.to_string());
    }
    allocate_numbered_part(package, "/_xmlsignatures", "origin", "sigs")
}

fn allocate_numbered_part(
    package: &OpcPackage,
    directory: &str,
    stem: &str,
    extension: &str,
) -> Result<String> {
    let prefix = format!("{directory}/{stem}");
    let suffix = format!(".{extension}");
    let mut maximum = 0_usize;
    for part_name in package
        .parts
        .keys()
        .chain(package.part_rels.keys())
        .chain(package.content_types.overrides.keys())
    {
        let lowercase = part_name.to_ascii_lowercase();
        let Some(number) = lowercase
            .strip_prefix(&prefix)
            .and_then(|value| value.strip_suffix(&suffix))
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|number| *number > 0)
        else {
            continue;
        };
        maximum = maximum.max(number);
    }
    let mut number = maximum.checked_add(1).unwrap_or(1);
    let start = number;
    loop {
        let candidate = format!("{prefix}{number}{suffix}");
        if !part_name_occupied(package, &candidate) {
            return Ok(candidate);
        }
        number = number.checked_add(1).unwrap_or(1);
        if number == start {
            return Err(OpcError::SignatureCreationFailed(format!(
                "no free {stem} part name"
            )));
        }
    }
}

fn part_name_occupied(package: &OpcPackage, name: &str) -> bool {
    package.contains_part(name)
        || package.get_part_rels(name).is_some()
        || package.content_types.contains_override(name)
}

pub(crate) fn verify_signatures(package: &OpcPackage) -> Result<Vec<SignatureReport>> {
    let mut signature_parts = discover_signature_parts(package)?;
    let discovered: HashSet<String> = signature_parts
        .iter()
        .map(|name| crate::package::part_identity(name))
        .collect();
    let mut orphaned: Vec<String> = package
        .parts
        .keys()
        .filter(|name| {
            is_signature_xml_part(name)
                && !discovered.contains(&crate::package::part_identity(name))
        })
        .cloned()
        .collect();
    orphaned.sort();
    signature_parts.sort();

    let mut reports = Vec::with_capacity(signature_parts.len() + orphaned.len());
    for part_name in signature_parts {
        let xml = package.get_part(&part_name).ok_or_else(|| {
            OpcError::InvalidSignatureXml(format!("missing signature part {part_name}"))
        })?;
        reports.push(verify_signature_part(package, &part_name, xml)?);
    }
    reports.extend(orphaned.into_iter().map(|part_name| SignatureReport {
        signature_part: part_name.clone(),
        signer: None,
        cryptographically_valid: false,
        coverage_complete: false,
        covered_parts: Vec::new(),
        covered_relationships: Vec::new(),
        issues: vec![SignatureIssue::OrphanSignaturePart { part_name }],
    }));
    Ok(reports)
}

fn discover_signature_parts(package: &OpcPackage) -> Result<Vec<String>> {
    let origins = package
        .package_rels
        .get_all_by_type(rel_types::DIGITAL_SIGNATURE_ORIGIN);
    let mut parts: Vec<String> = Vec::new();
    for origin in origins {
        if is_external(origin) {
            return Err(OpcError::InvalidSignatureXml(format!(
                "external signature origin relationship {}",
                origin.id
            )));
        }
        let origin_part = OpcPackage::resolve_rel_target("/", &origin.target);
        let Some(stored_origin) = package.stored_part_name(&origin_part) else {
            return Err(OpcError::InvalidSignatureXml(format!(
                "signature origin target is missing: {origin_part}"
            )));
        };
        let origin_part = stored_origin.to_owned();
        let origin_rels = package.get_part_rels(&origin_part).ok_or_else(|| {
            OpcError::InvalidSignatureXml(format!(
                "signature origin has no relationships: {origin_part}"
            ))
        })?;
        for signature in origin_rels.get_all_by_type(rel_types::DIGITAL_SIGNATURE) {
            if is_external(signature) {
                return Err(OpcError::InvalidSignatureXml(format!(
                    "external signature relationship {}",
                    signature.id
                )));
            }
            let target = OpcPackage::resolve_rel_target(&origin_part, &signature.target);
            let part_name = package
                .stored_part_name(&target)
                .unwrap_or(&target)
                .to_owned();
            if parts.iter().any(|existing| {
                crate::package::part_identity(existing) == crate::package::part_identity(&part_name)
            }) {
                return Err(OpcError::InvalidSignatureXml(format!(
                    "duplicate signature target {part_name}"
                )));
            }
            parts.push(part_name);
        }
    }
    Ok(parts)
}

fn verify_signature_part(
    package: &OpcPackage,
    signature_part: &str,
    xml: &[u8],
) -> Result<SignatureReport> {
    let root = parse_xml(xml)?;
    require_name(&root, DSIG_NS, "Signature")?;
    let signed_info = direct_child(&root, DSIG_NS, "SignedInfo")?;
    require_algorithm(
        direct_child(signed_info, DSIG_NS, "CanonicalizationMethod")?,
        "canonicalization",
        C14N_EXCLUSIVE,
    )?;
    require_algorithm(
        direct_child(signed_info, DSIG_NS, "SignatureMethod")?,
        "signature method",
        SIGNATURE_RSA_SHA256,
    )?;

    let certificate_element = descendant(&root, DSIG_NS, "X509Certificate")
        .ok_or_else(|| invalid_signature("missing X509Certificate"))?;
    let certificate_der = decode_base64(&element_text(certificate_element), "X509Certificate")?;
    let certificate = Certificate::from_der(&certificate_der)
        .map_err(|error| OpcError::InvalidSigningCertificate(error.to_string()))?;
    let subject_public_key_info = certificate.tbs_certificate().subject_public_key_info();
    let key_algorithm = subject_public_key_info.algorithm.oid.to_string();
    if key_algorithm != "1.2.840.113549.1.1.1" {
        return Err(unsupported("key algorithm", &key_algorithm));
    }
    let public_key = subject_public_key_info
        .subject_public_key
        .as_bytes()
        .ok_or_else(|| {
            OpcError::InvalidSigningCertificate("non-octet-aligned public key".into())
        })?;
    let signer = SignerCertificateIdentity {
        subject: certificate.tbs_certificate().subject().to_string(),
        issuer: certificate.tbs_certificate().issuer().to_string(),
        serial_number: certificate.tbs_certificate().serial_number().to_string(),
    };

    let mut issues = Vec::new();
    let mut covered_parts = BTreeSet::new();
    let mut covered_relationships = BTreeSet::new();
    let mut seen_references = HashSet::new();
    let mut all_reference_digests_valid = true;
    let mut authenticated_roots = Vec::new();

    for reference_element in direct_children(signed_info, DSIG_NS, "Reference") {
        let reference = parse_reference(reference_element)?;
        if !seen_references.insert(reference.uri.clone()) {
            issues.push(SignatureIssue::DuplicateReference {
                uri: reference.uri.clone(),
            });
            all_reference_digests_valid = false;
            continue;
        }
        let actual = apply_reference(
            package,
            &root,
            &reference,
            &mut covered_parts,
            &mut covered_relationships,
            &mut issues,
        )?;
        let digest_valid = compare_digest(&reference, actual, &mut issues);
        all_reference_digests_valid &= digest_valid;
        if digest_valid && let Some(target) = same_document_target(&root, &reference.uri)? {
            authenticated_roots.push(target);
        }
    }

    let mut manifests = Vec::new();
    for target in authenticated_roots {
        collect_matching_elements(target, DSIG_NS, "Manifest", &mut manifests);
    }
    let mut processed_manifests = HashSet::new();
    let mut manifest_index = 0;
    while let Some(manifest) = manifests.get(manifest_index).copied() {
        manifest_index += 1;
        if !processed_manifests.insert(manifest as *const XmlElement) {
            continue;
        }
        for reference_element in direct_children(manifest, DSIG_NS, "Reference") {
            let reference = parse_reference(reference_element)?;
            if !seen_references.insert(reference.uri.clone()) {
                issues.push(SignatureIssue::DuplicateReference {
                    uri: reference.uri.clone(),
                });
                all_reference_digests_valid = false;
                continue;
            }
            let actual = apply_reference(
                package,
                &root,
                &reference,
                &mut covered_parts,
                &mut covered_relationships,
                &mut issues,
            )?;
            let digest_valid = compare_digest(&reference, actual, &mut issues);
            all_reference_digests_valid &= digest_valid;
            if digest_valid && let Some(target) = same_document_target(&root, &reference.uri)? {
                collect_matching_elements(target, DSIG_NS, "Manifest", &mut manifests);
            }
        }
    }

    let signed_info_bytes = canonicalize(signed_info);
    let signature_value = decode_base64(
        &element_text(direct_child(&root, DSIG_NS, "SignatureValue")?),
        "SignatureValue",
    )?;
    let signed_info_valid = UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, public_key)
        .verify(&signed_info_bytes, &signature_value)
        .is_ok();
    if !signed_info_valid {
        issues.push(SignatureIssue::SignatureValueMismatch);
    }

    add_uncovered_issues(package, &covered_parts, &covered_relationships, &mut issues);
    let coverage_complete = !issues.iter().any(is_coverage_issue);
    let cryptographically_valid = signed_info_valid && all_reference_digests_valid;

    Ok(SignatureReport {
        signature_part: signature_part.to_string(),
        signer: Some(signer),
        cryptographically_valid,
        coverage_complete,
        covered_parts: covered_parts.into_iter().collect(),
        covered_relationships: covered_relationships.into_iter().collect(),
        issues,
    })
}

fn parse_reference(element: &XmlElement) -> Result<ReferenceSpec> {
    let uri = attribute(element, "", "URI")
        .ok_or_else(|| invalid_signature("Reference has no URI"))?
        .to_string();
    let digest_method = direct_child(element, DSIG_NS, "DigestMethod")?;
    require_algorithm(digest_method, "digest method", DIGEST_SHA256)?;
    let digest = decode_base64(
        &element_text(direct_child(element, DSIG_NS, "DigestValue")?),
        "DigestValue",
    )?;
    let mut transforms = Vec::new();
    if let Some(container) = optional_direct_child(element, DSIG_NS, "Transforms")? {
        for transform in direct_children(container, DSIG_NS, "Transform") {
            let algorithm = attribute(transform, "", "Algorithm")
                .ok_or_else(|| invalid_signature("Transform has no Algorithm"))?;
            match algorithm {
                C14N_EXCLUSIVE => transforms.push(ReferenceTransform::ExclusiveCanonicalization),
                RELATIONSHIP_TRANSFORM => {
                    let mut ids = Vec::new();
                    for reference in element_children(transform) {
                        if !has_name(reference, OPC_SIGNATURE_NS, "RelationshipReference") {
                            return Err(unsupported(
                                "relationship transform child",
                                &reference.name.qname,
                            ));
                        }
                        let id = attribute(reference, "", "SourceId").ok_or_else(|| {
                            invalid_signature("RelationshipReference has no SourceId")
                        })?;
                        if ids.iter().any(|existing| existing == id) {
                            return Err(invalid_signature(&format!(
                                "duplicate relationship SourceId {id}"
                            )));
                        }
                        ids.push(id.to_string());
                    }
                    transforms.push(ReferenceTransform::Relationships(ids));
                }
                other => return Err(unsupported("transform", other)),
            }
        }
    }
    Ok(ReferenceSpec {
        uri,
        digest,
        transforms,
    })
}

fn apply_reference(
    package: &OpcPackage,
    root: &XmlElement,
    reference: &ReferenceSpec,
    covered_parts: &mut BTreeSet<String>,
    covered_relationships: &mut BTreeSet<CoveredRelationship>,
    issues: &mut Vec<SignatureIssue>,
) -> Result<Option<Vec<u8>>> {
    if let Some(element) = same_document_target(root, &reference.uri)? {
        if !matches!(
            reference.transforms.as_slice(),
            [ReferenceTransform::ExclusiveCanonicalization]
        ) {
            return Err(unsupported("same-document transform chain", &reference.uri));
        }
        return Ok(Some(canonicalize(element)));
    }

    let part_name = reference_part_name(&reference.uri)?;
    if let Some((source_part, relationships)) = relationships_for_uri(package, &part_name) {
        let ids = match reference.transforms.as_slice() {
            [
                ReferenceTransform::Relationships(ids),
                ReferenceTransform::ExclusiveCanonicalization,
            ]
            | [ReferenceTransform::Relationships(ids)] => ids,
            _ => return Err(unsupported("relationship transform chain", &reference.uri)),
        };
        let transformed = relationship_transform(
            package,
            &source_part,
            relationships,
            ids,
            covered_relationships,
            issues,
        );
        return Ok(transformed);
    }

    if !reference.transforms.is_empty() {
        return Err(unsupported("part transform chain", &reference.uri));
    }
    let is_content_types = part_name.eq_ignore_ascii_case("/[Content_Types].xml");
    let bytes = if is_content_types {
        Some(content_types_bytes(package)?)
    } else {
        package.get_part(&part_name).map(<[u8]>::to_vec)
    };
    match bytes {
        Some(bytes) => {
            let covered_name = if is_content_types {
                "/[Content_Types].xml".to_owned()
            } else {
                package
                    .stored_part_name(&part_name)
                    .unwrap_or(&part_name)
                    .to_owned()
            };
            covered_parts.insert(covered_name);
            Ok(Some(bytes))
        }
        None => {
            issues.push(SignatureIssue::MissingPart {
                uri: reference.uri.clone(),
            });
            Ok(None)
        }
    }
}

fn same_document_target<'a>(root: &'a XmlElement, uri: &str) -> Result<Option<&'a XmlElement>> {
    let Some(id) = uri.strip_prefix('#') else {
        return Ok(None);
    };
    let elements = find_all_by_id(root, id);
    match elements.as_slice() {
        [] => Err(invalid_signature(&format!(
            "missing same-document reference #{id}"
        ))),
        [element] => Ok(Some(*element)),
        _ => Err(invalid_signature(&format!(
            "duplicate same-document Id {id}"
        ))),
    }
}

fn relationship_transform(
    package: &OpcPackage,
    source_part: &str,
    relationships: &Relationships,
    ids: &[String],
    covered: &mut BTreeSet<CoveredRelationship>,
    issues: &mut Vec<SignatureIssue>,
) -> Option<Vec<u8>> {
    let mut selected = Vec::new();
    let mut valid = true;
    for id in ids {
        let matches: Vec<&Relationship> = relationships
            .items
            .iter()
            .filter(|relationship| relationship.id == *id)
            .collect();
        match matches.as_slice() {
            [] => {
                issues.push(SignatureIssue::MissingRelationship {
                    source_part: source_part.to_string(),
                    relationship_id: id.clone(),
                });
                valid = false;
            }
            [relationship] if is_external(relationship) => {
                issues.push(SignatureIssue::ExternalRelationship {
                    source_part: source_part.to_string(),
                    relationship_id: id.clone(),
                    target: relationship.target.clone(),
                });
                valid = false;
            }
            [relationship] => {
                let target = OpcPackage::resolve_rel_target(source_part, &relationship.target);
                if !package.contains_part(&target) {
                    issues.push(SignatureIssue::MissingRelationshipTarget {
                        source_part: source_part.to_string(),
                        relationship_id: id.clone(),
                        target,
                    });
                    valid = false;
                    continue;
                }
                selected.push(*relationship);
                covered.insert(CoveredRelationship {
                    source_part: source_part.to_string(),
                    relationship_id: id.clone(),
                });
            }
            _ => {
                issues.push(SignatureIssue::DuplicateRelationship {
                    source_part: source_part.to_string(),
                    relationship_id: id.clone(),
                });
                valid = false;
            }
        }
    }
    if !valid {
        return None;
    }
    selected.sort_by(|left, right| left.id.cmp(&right.id));
    let mut output = format!("<Relationships xmlns=\"{RELATIONSHIPS_NS}\">");
    for relationship in selected {
        output.push_str("<Relationship Id=\"");
        escape_attribute(&relationship.id, &mut output);
        output.push_str("\" Target=\"");
        escape_attribute(&relationship.target, &mut output);
        output.push_str("\" TargetMode=\"Internal\" Type=\"");
        escape_attribute(&relationship.rel_type, &mut output);
        output.push_str("\"></Relationship>");
    }
    output.push_str("</Relationships>");
    Some(output.into_bytes())
}

fn compare_digest(
    reference: &ReferenceSpec,
    actual_bytes: Option<Vec<u8>>,
    issues: &mut Vec<SignatureIssue>,
) -> bool {
    let Some(actual_bytes) = actual_bytes else {
        return false;
    };
    let actual = Sha256::digest(&actual_bytes).to_vec();
    if actual == reference.digest {
        return true;
    }
    issues.push(SignatureIssue::ChangedReference {
        uri: reference.uri.clone(),
        expected_digest: BASE64.encode(&reference.digest),
        actual_digest: BASE64.encode(actual),
    });
    false
}

fn add_uncovered_issues(
    package: &OpcPackage,
    covered_parts: &BTreeSet<String>,
    covered_relationships: &BTreeSet<CoveredRelationship>,
    issues: &mut Vec<SignatureIssue>,
) {
    let infrastructure = signature_infrastructure_parts(package);
    if !covered_parts.contains("/[Content_Types].xml") {
        issues.push(SignatureIssue::UncoveredPart {
            part_name: "/[Content_Types].xml".to_string(),
        });
    }
    let mut parts: Vec<&String> = package
        .parts
        .keys()
        .filter(|name| !infrastructure.contains(&crate::package::part_identity(name)))
        .collect();
    parts.sort();
    for part_name in parts {
        if !covered_parts.contains(part_name) {
            issues.push(SignatureIssue::UncoveredPart {
                part_name: part_name.clone(),
            });
        }
    }

    add_uncovered_relationships("/", &package.package_rels, covered_relationships, issues);
    let mut sources: Vec<&String> = package.part_rels.keys().collect();
    sources.sort();
    for source in sources {
        add_uncovered_relationships(
            source,
            &package.part_rels[source],
            covered_relationships,
            issues,
        );
    }
}

fn signature_infrastructure_parts(package: &OpcPackage) -> BTreeSet<String> {
    let mut parts = BTreeSet::new();
    for origin in package
        .package_rels
        .get_all_by_type(rel_types::DIGITAL_SIGNATURE_ORIGIN)
    {
        if is_external(origin) {
            continue;
        }
        let origin_part = OpcPackage::resolve_rel_target("/", &origin.target);
        if let Some(stored_origin) = package.stored_part_name(&origin_part) {
            parts.insert(crate::package::part_identity(stored_origin));
        }
        if let Some(relationships) = package.get_part_rels(&origin_part) {
            for signature in relationships.get_all_by_type(rel_types::DIGITAL_SIGNATURE) {
                if !is_external(signature) {
                    parts.insert(crate::package::part_identity(
                        &OpcPackage::resolve_rel_target(&origin_part, &signature.target),
                    ));
                }
            }
        }
    }
    parts
}

fn add_uncovered_relationships(
    source: &str,
    relationships: &Relationships,
    covered: &BTreeSet<CoveredRelationship>,
    issues: &mut Vec<SignatureIssue>,
) {
    let mut items: Vec<&Relationship> = relationships
        .items
        .iter()
        .filter(|relationship| {
            relationship.rel_type != rel_types::DIGITAL_SIGNATURE_ORIGIN
                && relationship.rel_type != rel_types::DIGITAL_SIGNATURE
        })
        .collect();
    items.sort_by(|left, right| left.id.cmp(&right.id));
    for relationship in items {
        let item = CoveredRelationship {
            source_part: source.to_string(),
            relationship_id: relationship.id.clone(),
        };
        if !covered.contains(&item) {
            issues.push(SignatureIssue::UncoveredRelationship {
                source_part: source.to_string(),
                relationship_id: relationship.id.clone(),
            });
        }
    }
}

pub(crate) fn content_types_bytes(package: &OpcPackage) -> Result<Vec<u8>> {
    package.content_types_bytes()
}

fn relationships_for_uri<'a>(
    package: &'a OpcPackage,
    part_name: &str,
) -> Option<(String, &'a Relationships)> {
    if part_name.eq_ignore_ascii_case("/_rels/.rels") {
        return Some(("/".to_string(), &package.package_rels));
    }
    let marker = "/_rels/";
    let lowercase = part_name.to_ascii_lowercase();
    let marker_index = lowercase.rfind(marker)?;
    let tail = part_name.get(marker_index + marker.len()..)?;
    if !tail.to_ascii_lowercase().ends_with(".rels") {
        return None;
    }
    let filename = tail.get(..tail.len().checked_sub(".rels".len())?)?;
    let directory = &part_name[..marker_index + 1];
    let source = format!("{directory}{filename}");
    let stored_source = package
        .stored_part_rels_owner(&source)
        .unwrap_or(&source)
        .to_owned();
    package
        .get_part_rels(&stored_source)
        .map(|relationships| (stored_source, relationships))
}

fn reference_part_name(uri: &str) -> Result<String> {
    let path = uri.split('?').next().unwrap_or(uri);
    if path.is_empty() || !path.starts_with('/') {
        return Err(invalid_signature(&format!(
            "non-package reference URI {uri}"
        )));
    }
    percent_decode(path)
}

fn percent_decode(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes.get(index + 1).and_then(|byte| hex(*byte));
            let low = bytes.get(index + 2).and_then(|byte| hex(*byte));
            let (Some(high), Some(low)) = (high, low) else {
                return Err(invalid_signature(&format!(
                    "invalid percent escape in {value}"
                )));
            };
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| invalid_signature("reference URI is not UTF-8"))
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse_xml(xml: &[u8]) -> Result<XmlElement> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<XmlElement> = Vec::new();
    let mut root = None;
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(start)) => {
                let inherited = stack.last().map(|element| &element.namespaces);
                stack.push(parse_element_start(&reader, &start, inherited)?);
            }
            Ok(Event::Empty(start)) => {
                let inherited = stack.last().map(|element| &element.namespaces);
                let element = parse_element_start(&reader, &start, inherited)?;
                attach_element(element, &mut stack, &mut root)?;
            }
            Ok(Event::End(_)) => {
                let element = stack
                    .pop()
                    .ok_or_else(|| invalid_signature("unexpected closing element"))?;
                attach_element(element, &mut stack, &mut root)?;
            }
            Ok(Event::Text(text)) => {
                if let Some(parent) = stack.last_mut() {
                    let decoded = text
                        .xml_content(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|error| invalid_signature(&error.to_string()))?;
                    let unescaped = quick_xml::escape::unescape(&decoded)
                        .map_err(|error| invalid_signature(&error.to_string()))?;
                    parent.children.push(XmlNode::Text(unescaped.into_owned()));
                }
            }
            Ok(Event::CData(text)) => {
                if let Some(parent) = stack.last_mut() {
                    let decoded = text
                        .xml_content(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|error| invalid_signature(&error.to_string()))?;
                    parent.children.push(XmlNode::Text(decoded.into_owned()));
                }
            }
            Ok(Event::PI(instruction)) => {
                if let Some(parent) = stack.last_mut() {
                    let target = reader
                        .decoder()
                        .decode(instruction.target())
                        .map_err(|error| invalid_signature(&error.to_string()))?;
                    let content = reader
                        .decoder()
                        .decode(instruction.content())
                        .map_err(|error| invalid_signature(&error.to_string()))?;
                    parent.children.push(XmlNode::ProcessingInstruction {
                        target: normalize_xml_line_endings(&target),
                        content: normalize_xml_line_endings(&content),
                    });
                }
            }
            Ok(Event::Eof) => break,
            Ok(Event::Decl(_) | Event::Comment(_) | Event::DocType(_)) => {}
            Ok(Event::GeneralRef(reference)) => {
                return Err(invalid_signature(&format!(
                    "unresolved general entity {}",
                    String::from_utf8_lossy(reference.as_ref())
                )));
            }
            Err(error) => return Err(error.into()),
        }
        buffer.clear();
    }
    if !stack.is_empty() {
        return Err(invalid_signature("unclosed XML element"));
    }
    root.ok_or_else(|| invalid_signature("signature XML has no root element"))
}

fn parse_element_start(
    reader: &Reader<&[u8]>,
    start: &BytesStart<'_>,
    inherited: Option<&BTreeMap<String, String>>,
) -> Result<XmlElement> {
    let mut namespaces = inherited.cloned().unwrap_or_default();
    namespaces.insert("xml".to_string(), XML_NS.to_string());
    let mut decoded_attributes = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute?;
        let key = std::str::from_utf8(attribute.key.as_ref())?.to_string();
        let value = attribute
            .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, reader.decoder())?
            .into_owned();
        if key == "xmlns" {
            namespaces.insert(String::new(), value);
        } else if let Some(prefix) = key.strip_prefix("xmlns:") {
            namespaces.insert(prefix.to_string(), value);
        } else {
            decoded_attributes.push((key, value));
        }
    }
    let qname = std::str::from_utf8(start.name().as_ref())?.to_string();
    let name = resolve_name(&qname, &namespaces, true)?;
    let attributes = decoded_attributes
        .into_iter()
        .map(|(qname, value)| {
            Ok(XmlAttribute {
                name: resolve_name(&qname, &namespaces, false)?,
                value,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(XmlElement {
        name,
        attributes,
        children: Vec::new(),
        namespaces,
    })
}

fn resolve_name(
    qname: &str,
    namespaces: &BTreeMap<String, String>,
    use_default: bool,
) -> Result<XmlName> {
    let (prefix, local) = qname
        .split_once(':')
        .map_or(("", qname), |(prefix, local)| (prefix, local));
    let namespace = if prefix.is_empty() && !use_default {
        String::new()
    } else {
        namespaces.get(prefix).cloned().unwrap_or_default()
    };
    if !prefix.is_empty() && namespace.is_empty() {
        return Err(invalid_signature(&format!(
            "unbound namespace prefix {prefix}"
        )));
    }
    Ok(XmlName {
        qname: qname.to_string(),
        prefix: prefix.to_string(),
        local: local.to_string(),
        namespace,
    })
}

fn attach_element(
    element: XmlElement,
    stack: &mut [XmlElement],
    root: &mut Option<XmlElement>,
) -> Result<()> {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(XmlNode::Element(element));
    } else if root.replace(element).is_some() {
        return Err(invalid_signature("signature XML has multiple roots"));
    }
    Ok(())
}

fn canonicalize(element: &XmlElement) -> Vec<u8> {
    let mut output = String::new();
    write_canonical(element, &BTreeMap::new(), &mut output);
    output.into_bytes()
}

fn write_canonical(
    element: &XmlElement,
    rendered_namespaces: &BTreeMap<String, String>,
    output: &mut String,
) {
    output.push('<');
    output.push_str(&element.name.qname);

    let mut visible = BTreeSet::new();
    visible.insert(element.name.prefix.clone());
    for attribute in &element.attributes {
        if !attribute.name.prefix.is_empty() && attribute.name.prefix != "xml" {
            visible.insert(attribute.name.prefix.clone());
        }
    }
    let mut child_namespaces = rendered_namespaces.clone();
    for prefix in visible {
        let uri = if prefix.is_empty() {
            element.name.namespace.as_str()
        } else {
            element
                .namespaces
                .get(&prefix)
                .map(String::as_str)
                .unwrap_or("")
        };
        if rendered_namespaces.get(&prefix).map(String::as_str) != Some(uri) {
            if prefix.is_empty() {
                output.push_str(" xmlns=\"");
            } else {
                output.push_str(" xmlns:");
                output.push_str(&prefix);
                output.push_str("=\"");
            }
            escape_attribute(uri, output);
            output.push('"');
            child_namespaces.insert(prefix, uri.to_string());
        }
    }

    let mut attributes: Vec<&XmlAttribute> = element.attributes.iter().collect();
    attributes.sort_by(|left, right| {
        (left.name.namespace.as_str(), left.name.local.as_str())
            .cmp(&(right.name.namespace.as_str(), right.name.local.as_str()))
    });
    for attribute in attributes {
        output.push(' ');
        output.push_str(&attribute.name.qname);
        output.push_str("=\"");
        escape_attribute(&attribute.value, output);
        output.push('"');
    }
    output.push('>');
    for child in &element.children {
        match child {
            XmlNode::Element(child) => write_canonical(child, &child_namespaces, output),
            XmlNode::Text(text) => escape_text(text, output),
            XmlNode::ProcessingInstruction { target, content } => {
                output.push_str("<?");
                output.push_str(target);
                escape_processing_instruction(content, output);
                output.push_str("?>");
            }
        }
    }
    output.push_str("</");
    output.push_str(&element.name.qname);
    output.push('>');
}

fn escape_processing_instruction(value: &str, output: &mut String) {
    for character in value.chars() {
        if character == '\r' {
            output.push_str("&#xD;");
        } else {
            output.push(character);
        }
    }
}

fn normalize_xml_line_endings(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\r' {
            if characters.peek() == Some(&'\n') {
                characters.next();
            }
            normalized.push('\n');
        } else {
            normalized.push(character);
        }
    }
    normalized
}

fn escape_attribute(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '"' => output.push_str("&quot;"),
            '\t' => output.push_str("&#x9;"),
            '\n' => output.push_str("&#xA;"),
            '\r' => output.push_str("&#xD;"),
            _ => output.push(character),
        }
    }
}

fn escape_text(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '\r' => output.push_str("&#xD;"),
            _ => output.push(character),
        }
    }
}

fn require_algorithm(element: &XmlElement, kind: &'static str, expected: &str) -> Result<()> {
    let actual = attribute(element, "", "Algorithm")
        .ok_or_else(|| invalid_signature(&format!("{} has no Algorithm", element.name.qname)))?;
    if actual != expected {
        return Err(unsupported(kind, actual));
    }
    Ok(())
}

fn require_name(element: &XmlElement, namespace: &str, local: &str) -> Result<()> {
    if has_name(element, namespace, local) {
        Ok(())
    } else {
        Err(invalid_signature(&format!(
            "expected {{{namespace}}}{local}, found {}",
            element.name.qname
        )))
    }
}

fn direct_child<'a>(
    element: &'a XmlElement,
    namespace: &str,
    local: &str,
) -> Result<&'a XmlElement> {
    optional_direct_child(element, namespace, local)?
        .ok_or_else(|| invalid_signature(&format!("missing {{{namespace}}}{local}")))
}

fn optional_direct_child<'a>(
    element: &'a XmlElement,
    namespace: &str,
    local: &str,
) -> Result<Option<&'a XmlElement>> {
    let matches: Vec<&XmlElement> = element_children(element)
        .filter(|child| has_name(child, namespace, local))
        .collect();
    match matches.as_slice() {
        [] => Ok(None),
        [child] => Ok(Some(*child)),
        _ => Err(invalid_signature(&format!(
            "duplicate {{{namespace}}}{local}"
        ))),
    }
}

fn direct_children<'a>(
    element: &'a XmlElement,
    namespace: &'a str,
    local: &'a str,
) -> impl Iterator<Item = &'a XmlElement> {
    element_children(element).filter(move |child| has_name(child, namespace, local))
}

fn element_children(element: &XmlElement) -> impl Iterator<Item = &XmlElement> {
    element.children.iter().filter_map(|child| match child {
        XmlNode::Element(element) => Some(element),
        XmlNode::Text(_) | XmlNode::ProcessingInstruction { .. } => None,
    })
}

fn descendant<'a>(element: &'a XmlElement, namespace: &str, local: &str) -> Option<&'a XmlElement> {
    element_children(element).find_map(|child| {
        has_name(child, namespace, local)
            .then_some(child)
            .or_else(|| descendant(child, namespace, local))
    })
}

fn collect_matching_elements<'a>(
    element: &'a XmlElement,
    namespace: &str,
    local: &str,
    found: &mut Vec<&'a XmlElement>,
) {
    if has_name(element, namespace, local) {
        found.push(element);
    }
    collect_descendants(element, namespace, local, found);
}

fn collect_descendants<'a>(
    element: &'a XmlElement,
    namespace: &str,
    local: &str,
    found: &mut Vec<&'a XmlElement>,
) {
    for child in element_children(element) {
        if has_name(child, namespace, local) {
            found.push(child);
        }
        collect_descendants(child, namespace, local, found);
    }
}

fn find_all_by_id<'a>(element: &'a XmlElement, id: &str) -> Vec<&'a XmlElement> {
    let mut found = Vec::new();
    collect_by_id(element, id, &mut found);
    found
}

fn collect_by_id<'a>(element: &'a XmlElement, id: &str, found: &mut Vec<&'a XmlElement>) {
    if attribute(element, "", "Id") == Some(id) {
        found.push(element);
    }
    for child in element_children(element) {
        collect_by_id(child, id, found);
    }
}

fn has_name(element: &XmlElement, namespace: &str, local: &str) -> bool {
    element.name.namespace == namespace && element.name.local == local
}

fn attribute<'a>(element: &'a XmlElement, namespace: &str, local: &str) -> Option<&'a str> {
    element
        .attributes
        .iter()
        .find(|attribute| attribute.name.namespace == namespace && attribute.name.local == local)
        .map(|attribute| attribute.value.as_str())
}

fn element_text(element: &XmlElement) -> String {
    let mut text = String::new();
    collect_text(element, &mut text);
    text.trim().to_string()
}

fn collect_text(element: &XmlElement, output: &mut String) {
    for child in &element.children {
        match child {
            XmlNode::Element(child) => collect_text(child, output),
            XmlNode::Text(text) => output.push_str(text),
            XmlNode::ProcessingInstruction { .. } => {}
        }
    }
}

fn decode_base64(value: &str, field: &str) -> Result<Vec<u8>> {
    let compact: String = value
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    BASE64
        .decode(compact)
        .map_err(|error| invalid_signature(&format!("invalid {field}: {error}")))
}

fn is_external(relationship: &Relationship) -> bool {
    relationship
        .target_mode
        .as_deref()
        .is_some_and(|mode| mode.eq_ignore_ascii_case("external"))
}

fn is_signature_part(name: &str) -> bool {
    name.get(.."/_xmlsignatures/".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("/_xmlsignatures/"))
}

fn is_signature_xml_part(name: &str) -> bool {
    is_signature_part(name)
        && name
            .get(name.len().saturating_sub(4)..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".xml"))
}

fn is_coverage_issue(issue: &SignatureIssue) -> bool {
    !matches!(
        issue,
        SignatureIssue::SignatureValueMismatch | SignatureIssue::ChangedReference { .. }
    )
}

fn invalid_signature(message: &str) -> OpcError {
    OpcError::InvalidSignatureXml(message.to_string())
}

fn unsupported(kind: &'static str, algorithm: &str) -> OpcError {
    OpcError::UnsupportedSignatureAlgorithm {
        kind,
        algorithm: algorithm.to_string(),
    }
}
