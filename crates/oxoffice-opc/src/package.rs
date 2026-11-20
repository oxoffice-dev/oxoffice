//! OPC Package reader and writer for ZIP-based OOXML packages.

use std::borrow::Cow;
#[cfg(debug_assertions)]
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use zip::ZipWriter;
use zip::read::ZipArchive;
use zip::write::SimpleFileOptions;

use crate::content_types::ContentTypes;
use crate::error::{OpcError, Result};
use crate::relationship::{Relationships, rel_types};

const EOCD_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
const ZIP64_EOCD_LOCATOR_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x06, 0x07];
const ZIP64_EOCD_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x06, 0x06];
const MAX_ZIP_COMMENT_LEN: u64 = u16::MAX as u64;
const EOCD_LEN: u64 = 22;
const ZIP64_EOCD_LOCATOR_LEN: u64 = 20;
const MIN_ZIP64_EOCD_LEN: u64 = 56;
const ZIP_TAIL_LEN: u64 =
    MAX_ZIP_COMMENT_LEN + EOCD_LEN + ZIP64_EOCD_LOCATOR_LEN + MIN_ZIP64_EOCD_LEN;

/// A single part within the OPC package.
#[derive(Debug, Clone)]
pub struct PackagePart {
    /// Part name (URI), e.g. "/word/document.xml"
    pub name: String,
    /// Raw bytes of the part content
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ContentTypesSource {
    xml: Vec<u8>,
    content_types: ContentTypes,
}

struct SerializedPackage<'a> {
    content_types: Vec<u8>,
    package_relationships: Vec<u8>,
    part_relationships: Vec<(String, Vec<u8>)>,
    parts: Vec<(&'a str, &'a [u8])>,
}

/// Resource limits applied while expanding an OPC ZIP archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageReadLimits {
    pub max_entries: usize,
    pub max_part_uncompressed_bytes: u64,
    pub max_total_uncompressed_bytes: u64,
}

impl PackageReadLimits {
    pub const UNBOUNDED: Self = Self {
        max_entries: usize::MAX,
        max_part_uncompressed_bytes: u64::MAX,
        max_total_uncompressed_bytes: u64::MAX,
    };
}

/// An in-memory representation of an OPC package (ZIP archive).
#[derive(Debug, Clone)]
pub struct OpcPackage {
    /// Content types from `[Content_Types].xml`
    pub content_types: ContentTypes,
    /// Package-level relationships from `_rels/.rels`
    pub package_rels: Relationships,
    /// Part-level relationships keyed by the part they belong to.
    /// Key is the part name (e.g. "/word/document.xml"),
    /// value is the parsed relationships.
    pub part_rels: HashMap<String, Relationships>,
    /// All parts keyed by their URI (e.g. "/word/document.xml"). The bytes
    /// are shared, so cloning a package copies pointers, not part content.
    pub parts: HashMap<String, Arc<Vec<u8>>>,
    content_types_source: Option<ContentTypesSource>,
    /// Set only by a byte read or a reread of such a package. A hand-built
    /// package is not a baseline, and reread then uses the ZIP.
    trusted_baseline: bool,
}

impl OpcPackage {
    /// Open an OPC package from a file path.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        Self::from_reader(file)
    }

    /// Open an OPC package from any reader that implements Read + Seek.
    pub fn from_reader<R: Read + Seek>(reader: R) -> Result<Self> {
        Self::from_reader_with_limits(reader, PackageReadLimits::UNBOUNDED)
    }

    /// Open an agile-encrypted OPC package with no archive-expansion limit.
    #[cfg(feature = "agile-encryption")]
    pub fn from_encrypted_reader<R: Read + Seek>(reader: R, password: &str) -> Result<Self> {
        Self::from_encrypted_reader_with_limits(reader, password, PackageReadLimits::UNBOUNDED)
    }

    /// Authenticate and open an agile-encrypted OPC package with expansion limits.
    #[cfg(feature = "agile-encryption")]
    pub fn from_encrypted_reader_with_limits<R: Read + Seek>(
        reader: R,
        password: &str,
        limits: PackageReadLimits,
    ) -> Result<Self> {
        let plaintext = crate::encryption::decrypt_package(reader, password, limits)?;
        Self::from_reader_with_limits(std::io::Cursor::new(plaintext), limits)
    }

    /// Open an OPC package while bounding archive expansion.
    pub fn from_reader_with_limits<R: Read + Seek>(
        mut reader: R,
        limits: PackageReadLimits,
    ) -> Result<Self> {
        let entry_count = pre_index_entry_count(&mut reader)?;
        if entry_count > limits.max_entries as u64 {
            return Err(OpcError::PackageLimitExceeded {
                kind: "entry count",
                limit: limits.max_entries as u64,
            });
        }

        let mut archive = ZipArchive::new(&mut reader)?;
        if entry_count > archive.len() as u64 {
            let directory_start = archive.central_directory_start();
            let metadata = archive.metadata();
            drop(archive);
            let name =
                physical_duplicate_part_name(&mut reader, directory_start, entry_count, &metadata)?
                    .ok_or_else(|| {
                        invalid_zip("central directory entry count differs from ZIP index")
                    })?;
            return Err(OpcError::DuplicatePartName(name));
        }
        if archive.len() > limits.max_entries {
            return Err(OpcError::PackageLimitExceeded {
                kind: "entry count",
                limit: limits.max_entries as u64,
            });
        }
        let mut raw_parts: HashMap<String, Vec<u8>> = HashMap::new();
        let mut raw_part_identities = HashSet::new();
        let mut total_uncompressed_bytes = 0_u64;

        // Read all entries from the ZIP
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            if entry.is_dir() {
                continue;
            }
            if entry.size() > limits.max_part_uncompressed_bytes {
                return Err(OpcError::PackageLimitExceeded {
                    kind: "part size",
                    limit: limits.max_part_uncompressed_bytes,
                });
            }
            if total_uncompressed_bytes
                .checked_add(entry.size())
                .is_none_or(|total| total > limits.max_total_uncompressed_bytes)
            {
                return Err(OpcError::PackageLimitExceeded {
                    kind: "total uncompressed size",
                    limit: limits.max_total_uncompressed_bytes,
                });
            }
            let name = normalize_part_name(entry.name()?.as_ref());
            let name = name.strip_prefix('/').unwrap_or(&name).to_string();
            if !raw_part_identities.insert(name.to_ascii_lowercase()) {
                return Err(OpcError::DuplicatePartName(name));
            }
            let mut data = Vec::new();
            let read_limit = limits
                .max_part_uncompressed_bytes
                .min(limits.max_total_uncompressed_bytes - total_uncompressed_bytes);
            entry
                .by_ref()
                .take(read_limit.saturating_add(1))
                .read_to_end(&mut data)?;
            let data_len = data.len() as u64;
            if data_len > limits.max_part_uncompressed_bytes {
                return Err(OpcError::PackageLimitExceeded {
                    kind: "part size",
                    limit: limits.max_part_uncompressed_bytes,
                });
            }
            total_uncompressed_bytes = total_uncompressed_bytes
                .checked_add(data_len)
                .filter(|total| *total <= limits.max_total_uncompressed_bytes)
                .ok_or(OpcError::PackageLimitExceeded {
                    kind: "total uncompressed size",
                    limit: limits.max_total_uncompressed_bytes,
                })?;
            raw_parts.insert(name, data);
        }

        // Parse [Content_Types].xml
        let ct_xml = deterministic_part_key(&raw_parts, "/[Content_Types].xml")
            .and_then(|name| raw_parts.get(name))
            .ok_or_else(|| OpcError::PartNotFound("[Content_Types].xml".into()))?;
        let content_types = ContentTypes::from_xml(ct_xml)?;

        let mut entry_names = raw_parts.keys().collect::<Vec<_>>();
        entry_names.sort();
        for name in entry_names {
            if is_xml_entry(&content_types, name)
                && let Some((code_point, line, column)) =
                    first_invalid_xml_character(&raw_parts[name])
            {
                return Err(OpcError::InvalidXmlCharacter {
                    part: format!("/{}", name.trim_start_matches('/')),
                    code_point,
                    line,
                    column,
                });
            }
        }

        // Parse package-level relationships: _rels/.rels
        let package_rels = if let Some(rels_xml) =
            deterministic_part_key(&raw_parts, "/_rels/.rels").and_then(|name| raw_parts.get(name))
        {
            Relationships::from_xml(rels_xml)?
        } else {
            Relationships::new()
        };

        // Parse all part-level .rels files
        let mut part_rels = HashMap::new();
        let rels_entries: Vec<String> = raw_parts
            .keys()
            .filter(|name| {
                name.to_ascii_lowercase().ends_with(".rels")
                    && part_identity(name) != part_identity("/_rels/.rels")
            })
            .cloned()
            .collect();

        for rels_path in rels_entries {
            if let Some(xml_data) = raw_parts.get(&rels_path) {
                let rels = Relationships::from_xml(xml_data)?;
                // Convert rels path to the part name it belongs to.
                // e.g. "word/_rels/document.xml.rels" → "/word/document.xml"
                let part_name = rels_path_to_part_name(&rels_path);
                part_rels.insert(part_name, rels);
            }
        }

        // Build parts map with leading "/" normalized
        let mut parts = HashMap::new();
        for (name, data) in &raw_parts {
            if part_identity(name) == part_identity("/[Content_Types].xml")
                || part_identity(name) == part_identity("/_rels/.rels")
                || name.to_ascii_lowercase().ends_with(".rels")
            {
                continue;
            }
            let normalized = if name.starts_with('/') {
                name.clone()
            } else {
                format!("/{name}")
            };
            parts.insert(normalized, Arc::new(data.clone()));
        }

        Ok(OpcPackage {
            content_types,
            package_rels,
            part_rels,
            parts,
            content_types_source: Some(ContentTypesSource {
                xml: ct_xml.clone(),
                content_types: ContentTypes::from_xml(ct_xml)?,
            }),
            trusted_baseline: true,
        })
    }

    /// Save the OPC package to a file path.
    ///
    /// The complete ZIP is built in memory and published through
    /// [`write_atomic_file`], so a failed save leaves an existing file as it
    /// was.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let mut output = std::io::Cursor::new(Vec::new());
        self.write_to(&mut output)?;
        write_atomic_file(
            path.as_ref(),
            output.get_ref(),
            "oxoffice-opc",
            "invalid package file name",
            "could not allocate package save staging file",
        )?;
        Ok(())
    }

    /// Write the OPC package to any writer.
    pub fn write_to<W: Write + Seek>(&self, writer: W) -> Result<()> {
        // Serialize before the ZIP writer exists: a failure then writes nothing.
        let entries = self.zip_entries()?;
        let mut zip = ZipWriter::new(writer);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in entries {
            zip.start_file(name, options)?;
            zip.write_all(&data)?;
        }
        zip.finish()?;
        Ok(())
    }

    /// Gives the package that writing this one with [`OpcPackage::write_to`]
    /// and reading the bytes back gives, or the error the write raises,
    /// without building the ZIP when the fast path is proved.
    ///
    /// Precondition: `read` is still the unmodified package a read or an
    /// earlier reread produced, and this package is the candidate edited
    /// from that baseline. A later edit of `read` is not a baseline. The
    /// trusted flag records that read or reread provenance. It does not
    /// detect an edit made after the read, and it does not detect an
    /// arbitrary public part-map edit. A hand-built package starts
    /// untrusted, so the known hand-built fallback is the ZIP path because
    /// the flag is false, not because the flag inspected the map. The parts this package
    /// still shares with `read` are taken to pass the checks of a write
    /// because a read or an earlier reread made them. Read-proved XML has
    /// already passed the forbidden-character check. Relationships and content types that still hold
    /// what they were read from are kept as they are, and only what changed
    /// is serialized and parsed again. A package with a name a write would
    /// store under another entry name, a part too large for a plain ZIP
    /// entry goes through the ZIP.
    #[doc(hidden)]
    pub fn reread(mut self, read: &OpcPackage) -> Result<Self> {
        let renamed_part = |name: &String| {
            !written_as_is(name)
                || name
                    .len()
                    .checked_sub(".rels".len())
                    .is_some_and(|start| name.as_bytes()[start..].eq_ignore_ascii_case(b".rels"))
                || name.eq_ignore_ascii_case("/[Content_Types].xml")
        };
        if !read.trusted_baseline
            || self.parts.keys().any(renamed_part)
            || self.part_rels.keys().any(|owner| !written_as_is(owner))
            || self
                .parts
                .values()
                .any(|data| data.len() >= u32::MAX as usize)
        {
            let mut bytes = std::io::Cursor::new(Vec::new());
            // Count the fallback write even when `write_to` returns an error.
            // Other `write_to` callers count themselves. The fast path below
            // does not call this note.
            note_reread_zip_write();
            self.write_to(&mut bytes)?;
            bytes.set_position(0);
            return Self::from_reader(bytes);
        }

        // The checks of `serialize`, in its order, on the entries that may
        // differ from what was read: relationships equal to those read were
        // checked when read. With no renamed entry, the ZIP entry names
        // cannot collide once the part and relationship maps hold no two
        // equivalent names.
        self.validate_graph()?;
        let content_types = self.content_types_bytes()?;
        let package_relationships = self.package_rels.to_xml()?;
        let mut owners = self
            .part_rels
            .iter()
            .filter(|(owner, relationships)| {
                !read
                    .part_rels
                    .get(*owner)
                    .is_some_and(|read| read.same_as(relationships))
            })
            .map(|(owner, _)| owner.clone())
            .collect::<Vec<_>>();
        owners.sort();
        let mut part_relationships = Vec::with_capacity(owners.len());
        for owner in owners {
            let xml = self.part_rels[&owner].to_xml()?;
            part_relationships.push((owner, xml));
        }
        // A part read as an XML entry held no character XML 1.0 forbids, so
        // a shared part is checked again only when its content type may have
        // made it one.
        let retyped_overrides = self
            .content_types
            .overrides
            .iter()
            .filter(|(name, content_type)| {
                read.content_types.overrides.get(*name) != Some(content_type)
            })
            .chain(
                read.content_types
                    .overrides
                    .iter()
                    .filter(|(name, _)| !self.content_types.overrides.contains_key(*name)),
            )
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        let retyped = |name: &str| {
            self.content_types.defaults != read.content_types.defaults
                || retyped_overrides
                    .iter()
                    .any(|retyped| retyped.eq_ignore_ascii_case(name))
        };
        let mut parts = self
            .parts
            .iter()
            .filter(|(name, data)| {
                let shared = read
                    .parts
                    .get(*name)
                    .is_some_and(|read| Arc::ptr_eq(read, data));
                (!shared || retyped(name))
                    && is_xml_entry(&self.content_types, name)
                    && !(shared && is_xml_entry(&read.content_types, name))
            })
            .map(|(name, data)| (name.as_str(), data.as_slice()))
            .collect::<Vec<_>>();
        parts.sort_by_key(|(name, _)| *name);
        self.reject_written_xml_characters("[Content_Types].xml", &content_types)?;
        self.reject_written_xml_characters("_rels/.rels", &package_relationships)?;
        for (owner, xml) in &part_relationships {
            self.reject_written_xml_characters(&part_name_to_rels_path(owner), xml)?;
        }
        for (name, data) in parts {
            self.reject_written_xml_characters(name, data)?;
        }

        // What reading the written entries back parses.
        if !self
            .content_types_source
            .as_ref()
            .is_some_and(|source| source.content_types == self.content_types)
        {
            self.content_types = ContentTypes::from_xml(&content_types)?;
            self.content_types_source = Some(ContentTypesSource {
                content_types: ContentTypes::from_xml(&content_types)?,
                xml: content_types,
            });
        }
        if !self.package_rels.is_as_read() {
            self.package_rels = Relationships::from_xml(&package_relationships)?;
        }
        for (owner, xml) in part_relationships {
            if !self.part_rels[&owner].is_as_read() {
                self.part_rels.insert(owner, Relationships::from_xml(&xml)?);
            }
        }
        self.trusted_baseline = true;
        Ok(self)
    }

    /// Whether two packages hold the same parts, relationships and content
    /// types, and keep the same bytes they were read from.
    ///
    /// Precondition: each package is a normalized read or a reread. A read
    /// normalizes part names. A reread has the same shape when the fast path
    /// holds or after the ZIP fallback. Under that precondition the packages
    /// are the same exactly when the bytes written for them are. This does
    /// not compare an arbitrary hand-built package, and it does not detect
    /// a public map edit made after the read.
    #[doc(hidden)]
    pub fn same_package_as(&self, other: &OpcPackage) -> bool {
        self.content_types == other.content_types
            && self.content_types_source == other.content_types_source
            && self.package_rels.same_as(&other.package_rels)
            && self.part_rels.len() == other.part_rels.len()
            && self.part_rels.iter().all(|(owner, relationships)| {
                other
                    .part_rels
                    .get(owner)
                    .is_some_and(|other| relationships.same_as(other))
            })
            && self.parts.len() == other.parts.len()
            && self.parts.iter().all(|(name, data)| {
                other
                    .parts
                    .get(name)
                    .is_some_and(|other| Arc::ptr_eq(data, other) || data == other)
            })
    }

    /// The ZIP entries [`Self::write_to`] writes, in its order, each as the
    /// exact entry-name string the writer emits and its bytes.
    ///
    /// `[Content_Types].xml` and `_rels/.rels` come first, then one
    /// relationship part per relationship owner, sorted by owner part name,
    /// then the parts, sorted by part name. An ordinary part drops at most
    /// one leading slash from the first authored stored key. Metadata and
    /// relationship names use the existing serializer mappings. Metadata
    /// bytes are owned. Ordinary part bytes are borrowed from the package.
    /// The same package always yields the same entries, and a package that
    /// cannot be saved returns that save error.
    pub fn zip_entries(&self) -> Result<Vec<(String, Cow<'_, [u8]>)>> {
        let serialized = self.serialize()?;
        let mut entries =
            Vec::with_capacity(2 + serialized.part_relationships.len() + serialized.parts.len());
        entries.push((
            "[Content_Types].xml".to_owned(),
            Cow::Owned(serialized.content_types),
        ));
        entries.push((
            "_rels/.rels".to_owned(),
            Cow::Owned(serialized.package_relationships),
        ));
        entries.extend(
            serialized
                .part_relationships
                .into_iter()
                .map(|(rels_path, rels_xml)| (rels_path, Cow::Owned(rels_xml))),
        );
        entries.extend(serialized.parts.into_iter().map(|(name, data)| {
            (
                name.strip_prefix('/').unwrap_or(name).to_owned(),
                Cow::Borrowed(data),
            )
        }));
        for (name, _) in &entries {
            reject_zip_local_entry_name(name)?;
        }
        Ok(entries)
    }

    /// Append a password-protected CFB envelope around the deterministic OPC ZIP.
    ///
    /// The output buffer is unchanged if package serialization, encryption, or
    /// allocation fails.
    #[cfg(feature = "agile-encryption")]
    pub fn write_encrypted_to(&self, output: &mut Vec<u8>, password: &str) -> Result<()> {
        let mut plaintext = std::io::Cursor::new(Vec::new());
        self.write_to(&mut plaintext)?;
        crate::encryption::write_encrypted_package(output, &plaintext.into_inner(), password)
    }

    /// Get raw bytes of a part by its URI.
    pub fn get_part(&self, part_name: &str) -> Option<&[u8]> {
        let key = deterministic_part_key(&self.parts, part_name)?;
        self.parts.get(key).map(|data| data.as_slice())
    }

    /// Get shared raw part bytes by the same case-equivalent URI lookup as [`Self::get_part`].
    pub fn get_part_shared(&self, part_name: &str) -> Option<&Arc<Vec<u8>>> {
        let key = deterministic_part_key(&self.parts, part_name)?;
        self.parts.get(key)
    }

    /// Return whether a case-equivalent normalized part name exists.
    pub fn contains_part(&self, part_name: &str) -> bool {
        deterministic_part_key(&self.parts, part_name).is_some()
    }

    #[cfg(feature = "digital-signatures")]
    pub(crate) fn stored_part_name(&self, part_name: &str) -> Option<&str> {
        deterministic_part_key(&self.parts, part_name).map(String::as_str)
    }

    /// Set (or replace) a part's raw bytes, owned or already shared.
    pub fn set_part(&mut self, part_name: &str, data: impl Into<Arc<Vec<u8>>>) {
        let key = deterministic_part_key(&self.parts, part_name)
            .cloned()
            .unwrap_or_else(|| part_name.to_owned());
        self.parts.insert(key, data.into());
    }

    /// Remove and return a case-equivalent normalized part, with the bytes
    /// it may still share with a clone of this package.
    pub fn remove_part(&mut self, part_name: &str) -> Option<Arc<Vec<u8>>> {
        let key = deterministic_part_key(&self.parts, part_name)?.clone();
        self.parts.remove(&key)
    }

    pub(crate) fn content_types_bytes(&self) -> Result<Vec<u8>> {
        self.content_types.validate_identities()?;
        if let Some(source) = &self.content_types_source
            && source.content_types == self.content_types
        {
            return Ok(source.xml.clone());
        }
        self.content_types.to_xml()
    }

    /// Verify every digital signature discovered through the OPC relationship graph.
    ///
    /// Cryptographic validity authenticates the embedded certificate's key. It does
    /// not establish that the certificate chains to a trusted root. Certificate
    /// trust remains caller policy.
    #[cfg(feature = "digital-signatures")]
    pub fn verify_signatures(&self) -> Result<Vec<crate::SignatureReport>> {
        crate::signature::verify_signatures(self)
    }

    /// Sign a cloned candidate package with the strict RSA-SHA256 profile.
    ///
    /// The private key must be PKCS#8 DER or one unencrypted PRIVATE KEY PEM
    /// block. The certificate must be X.509 DER or one CERTIFICATE PEM block. The live package changes only after the candidate verifies with
    /// complete declared coverage. Certificate-chain trust remains caller
    /// policy.
    #[cfg(feature = "digital-signatures")]
    pub fn sign(
        &mut self,
        private_key_pkcs8_der: &[u8],
        certificate_der: &[u8],
    ) -> Result<crate::SignatureReport> {
        let mut candidate = self.clone();
        let report = crate::signature::create_signature(
            &mut candidate,
            private_key_pkcs8_der,
            certificate_der,
        )?;
        *self = candidate;
        Ok(report)
    }

    /// Get the relationships for a specific part.
    pub fn get_part_rels(&self, part_name: &str) -> Option<&Relationships> {
        let key = deterministic_part_key(&self.part_rels, part_name)?;
        self.part_rels.get(key)
    }

    /// Get mutable relationships for a case-equivalent normalized owner.
    pub fn get_part_rels_mut(&mut self, part_name: &str) -> Option<&mut Relationships> {
        let key = deterministic_part_key(&self.part_rels, part_name)?.clone();
        self.part_rels.get_mut(&key)
    }

    #[cfg(feature = "digital-signatures")]
    pub(crate) fn stored_part_rels_owner(&self, part_name: &str) -> Option<&str> {
        deterministic_part_key(&self.part_rels, part_name).map(String::as_str)
    }

    /// Get or create the relationships for a specific part.
    pub fn get_or_create_part_rels(&mut self, part_name: &str) -> &mut Relationships {
        let key = deterministic_part_key(&self.part_rels, part_name)
            .cloned()
            .unwrap_or_else(|| part_name.to_owned());
        self.part_rels.entry(key).or_default()
    }

    /// Replaces the relationships for a part while preserving the package's
    /// existing spelling for a case-equivalent relationship owner.
    pub fn set_part_rels(&mut self, part_name: &str, relationships: Relationships) {
        let key = deterministic_part_key(&self.part_rels, part_name)
            .cloned()
            .unwrap_or_else(|| part_name.to_owned());
        self.part_rels.insert(key, relationships);
    }

    /// Remove and return relationships for a case-equivalent normalized owner.
    pub fn remove_part_rels(&mut self, part_name: &str) -> Option<Relationships> {
        let key = deterministic_part_key(&self.part_rels, part_name)?.clone();
        self.part_rels.remove(&key)
    }

    /// Resolve the target URI of a relationship relative to its source part.
    ///
    /// `.` and `..` segments are collapsed, so a target such as
    /// `../media/image1.png` on `/word/charts/chart1.xml` resolves to
    /// `/media/image1.png` and matches the key it is stored under. Without
    /// this, parts referenced through a parent directory could never be found.
    pub fn resolve_rel_target(source_part: &str, rel_target: &str) -> String {
        let joined = if rel_target.starts_with('/') {
            rel_target.to_string()
        } else {
            // Directory of the source part, including the trailing slash.
            let dir = match source_part.rfind('/') {
                Some(pos) => &source_part[..=pos],
                None => "/",
            };
            format!("{dir}{rel_target}")
        };
        normalize_part_name(&joined)
    }

    /// Find the main document part URI by looking at package relationships.
    pub fn main_document_part(&self) -> Option<String> {
        self.package_rels
            .get_by_type(rel_types::DOCUMENT)
            .map(|rel| {
                if rel.target.starts_with('/') {
                    rel.target.clone()
                } else {
                    format!("/{}", rel.target)
                }
            })
    }

    /// Create an empty package with the universal OPC content types.
    pub fn new() -> Self {
        OpcPackage {
            content_types: ContentTypes::minimal(),
            package_rels: Relationships::new(),
            part_rels: HashMap::new(),
            parts: HashMap::new(),
            content_types_source: None,
            trusted_baseline: false,
        }
    }

    /// Create a package whose office document relationship targets a main part.
    pub fn with_main_part(part_name: &str, content_type: &str) -> Self {
        let mut package = Self::new();
        let part_name = part_name.strip_prefix('/').unwrap_or(part_name);
        package.package_rels.add(rel_types::DOCUMENT, part_name);
        package
            .content_types
            .add_override(&format!("/{part_name}"), content_type);
        package
    }

    fn serialize(&self) -> Result<SerializedPackage<'_>> {
        self.validate_graph()?;

        let content_types = self.content_types_bytes()?;
        let package_relationships = self.package_rels.to_xml()?;
        let mut part_relationships = Vec::with_capacity(self.part_rels.len());
        let mut relationship_owners = self.part_rels.iter().collect::<Vec<_>>();
        relationship_owners.sort_by_key(|(owner, _)| owner.as_str());
        for (owner, relationships) in relationship_owners {
            part_relationships.push((part_name_to_rels_path(owner), relationships.to_xml()?));
        }

        let mut parts = self
            .parts
            .iter()
            .map(|(name, data)| (name.as_str(), data.as_slice()))
            .collect::<Vec<_>>();
        parts.sort_by_key(|(name, _)| *name);
        validate_zip_entry_identities(&part_relationships, &parts)?;

        // No writer may publish a character XML 1.0 forbids, so every XML
        // entry is checked here, where every save of every format passes.
        self.reject_written_xml_characters("[Content_Types].xml", &content_types)?;
        self.reject_written_xml_characters("_rels/.rels", &package_relationships)?;
        for (rels_path, rels_xml) in &part_relationships {
            self.reject_written_xml_characters(rels_path, rels_xml)?;
        }
        for (name, data) in &parts {
            if is_xml_entry(&self.content_types, name) {
                self.reject_written_xml_characters(name, data)?;
            }
        }
        Ok(SerializedPackage {
            content_types,
            package_relationships,
            part_relationships,
            parts,
        })
    }

    pub(crate) fn validate_graph(&self) -> Result<()> {
        validate_part_map_identities(&self.parts)?;
        validate_part_map_identities(&self.part_rels)?;
        self.content_types.validate_identities()?;
        self.package_rels.validate_ids()?;
        for relationships in self.part_rels.values() {
            relationships.validate_ids()?;
        }
        Ok(())
    }
}

impl OpcPackage {
    /// Refuse a forbidden literal XML character, including in unchanged parts.
    fn reject_written_xml_characters(&self, entry: &str, data: &[u8]) -> Result<()> {
        let Some((code_point, line, column)) = first_invalid_xml_character(data) else {
            return Ok(());
        };
        let name = entry.strip_prefix('/').unwrap_or(entry);
        Err(OpcError::InvalidXmlCharacter {
            part: format!("/{name}"),
            code_point,
            line,
            column,
        })
    }
}

/// Whether an entry is XML: a relationship or content types entry, or a part
/// whose content type ends in `xml`.
fn is_xml_entry(content_types: &ContentTypes, name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".rels")
        || part_identity(name) == part_identity("/[Content_Types].xml")
        || content_types
            .content_type_for(&format!("/{}", name.strip_prefix('/').unwrap_or(name)))
            .is_some_and(|content_type| content_type.to_ascii_lowercase().ends_with("xml"))
}

/// The code point, line and column of the first character outside the XML
/// 1.0 `Char` production in a UTF-8 or UTF-16 entry. The byte scan keeps
/// producer XML available even when its decoder would reject that character.
fn first_invalid_xml_character(data: &[u8]) -> Option<(u32, usize, usize)> {
    if let Some(body) = data.strip_prefix(&[0xFF, 0xFE]) {
        return first_invalid_utf16_character(body, true);
    }
    if let Some(body) = data.strip_prefix(&[0xFE, 0xFF]) {
        return first_invalid_utf16_character(body, false);
    }
    if data.len() >= 4 && data[1] == 0 && data[3] == 0 {
        return first_invalid_utf16_character(data, true);
    }
    if data.len() >= 4 && data[0] == 0 && data[2] == 0 {
        return first_invalid_utf16_character(data, false);
    }
    let (offset, code_point) = data
        .iter()
        .enumerate()
        .find_map(|(index, byte)| match byte {
            0x00..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F => Some((index, u32::from(*byte))),
            0xEF if data.get(index + 1) == Some(&0xBF) => match data.get(index + 2) {
                Some(0xBE) => Some((index, 0xFFFE)),
                Some(0xBF) => Some((index, 0xFFFF)),
                _ => None,
            },
            _ => None,
        })?;
    let body_start = if data.starts_with(&[0xEF, 0xBB, 0xBF]) {
        3
    } else {
        0
    };
    let mut line = 1;
    let mut line_start = body_start;
    let mut index = body_start;
    while index < offset {
        match data[index] {
            b'\r' => {
                line += 1;
                if data.get(index + 1) == Some(&b'\n') {
                    index += 1;
                }
                line_start = index + 1;
            }
            b'\n' => {
                line += 1;
                line_start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    let column = data[line_start.min(offset)..offset]
        .iter()
        .filter(|byte| **byte & 0xC0 != 0x80)
        .count()
        + 1;
    Some((code_point, line, column))
}

fn first_invalid_utf16_character(data: &[u8], little_endian: bool) -> Option<(u32, usize, usize)> {
    let units = data.chunks_exact(2).map(|pair| {
        if little_endian {
            u16::from_le_bytes([pair[0], pair[1]])
        } else {
            u16::from_be_bytes([pair[0], pair[1]])
        }
    });
    let mut line = 1;
    let mut column = 1;
    let mut after_cr = false;
    for decoded in char::decode_utf16(units) {
        let code_point = match decoded {
            Ok(character) => u32::from(character),
            Err(error) => u32::from(error.unpaired_surrogate()),
        };
        if matches!(code_point, 0x00..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F | 0xD800..=0xDFFF | 0xFFFE | 0xFFFF)
        {
            return Some((code_point, line, column));
        }
        match code_point {
            0x0D => {
                line += 1;
                column = 1;
                after_cr = true;
            }
            0x0A if after_cr => after_cr = false,
            0x0A => {
                line += 1;
                column = 1;
            }
            _ => {
                column += 1;
                after_cr = false;
            }
        }
    }
    None
}

pub(crate) fn part_identity(part_name: &str) -> String {
    normalize_part_name(part_name).to_ascii_lowercase()
}

fn deterministic_part_key<'a, T>(
    values: &'a HashMap<String, T>,
    part_name: &str,
) -> Option<&'a String> {
    let identity = part_identity(part_name);
    values
        .keys()
        .filter(|candidate| has_part_identity(candidate, &identity))
        .min()
}

/// Debug-only note invoked once immediately before `reread`'s fallback
/// `write_to`. The note runs when that write returns an error. It does not
/// run on the fast path and it does not run for any other `write_to` caller.
/// `None` clears the note. The note must not call `reread`.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn set_reread_zip_note(note: Option<fn()>) {
    REREAD_ZIP_NOTE.set(note);
}

#[cfg(debug_assertions)]
thread_local! {
    static REREAD_ZIP_NOTE: Cell<Option<fn()>> = const { Cell::new(None) };
}

fn note_reread_zip_write() {
    #[cfg(debug_assertions)]
    if let Some(note) = REREAD_ZIP_NOTE.get() {
        note();
    }
}

/// Whether a write stores the part or relationship owner `name` under a ZIP
/// entry that reading it back names `name` again: an absolute name with no
/// empty or dot segment and no trailing backslash, which the reader takes
/// for a directory. The ordinary entry name and the relationship entry name
/// must both pass `reject_zip_local_entry_name`. A failure there is not a
/// fast-path error. `reread` takes the ZIP fallback, and `write_to` keeps
/// its existing validation order.
fn written_as_is(name: &str) -> bool {
    let canonical = name.starts_with('/')
        && !name.ends_with('\\')
        && name
            .split('/')
            .skip(1)
            .all(|segment| !matches!(segment, "" | "." | ".."));
    canonical
        && reject_zip_local_entry_name(name.strip_prefix('/').unwrap_or(name)).is_ok()
        && reject_zip_local_entry_name(&part_name_to_rels_path(name)).is_ok()
}

/// Whether `candidate` names the part `identity` names. A name that is its
/// own normalized form, as nearly every stored name is, is compared without
/// normalizing it, so a lookup allocates nothing per stored name.
fn has_part_identity(candidate: &str, identity: &str) -> bool {
    let normalized = candidate.starts_with('/')
        && !candidate.ends_with('/')
        && !candidate.contains("//")
        && !candidate.contains("/.");
    candidate.eq_ignore_ascii_case(identity) || !normalized && part_identity(candidate) == identity
}

fn validate_part_map_identities<T>(values: &HashMap<String, T>) -> Result<()> {
    let mut identities = HashSet::with_capacity(values.len());
    let mut names = values.keys().collect::<Vec<_>>();
    names.sort_unstable();
    for name in names {
        if !identities.insert(part_identity(name)) {
            return Err(OpcError::DuplicatePartName(name.clone()));
        }
    }
    Ok(())
}

fn validate_zip_entry_identities(
    part_relationships: &[(String, Vec<u8>)],
    parts: &[(&str, &[u8])],
) -> Result<()> {
    let mut identities = HashSet::from([
        part_identity("/[Content_Types].xml"),
        part_identity("/_rels/.rels"),
    ]);
    for (name, _) in part_relationships {
        if !identities.insert(part_identity(name)) {
            return Err(OpcError::DuplicatePartName(name.clone()));
        }
    }
    for (name, _) in parts {
        if !identities.insert(part_identity(name)) {
            return Err(OpcError::DuplicatePartName((*name).to_owned()));
        }
    }
    Ok(())
}

fn pre_index_entry_count<R: Read + Seek>(reader: &mut R) -> Result<u64> {
    let file_len = reader.seek(SeekFrom::End(0))?;
    if file_len < EOCD_LEN {
        return Err(invalid_zip("end of central directory not found"));
    }

    let tail_len = file_len.min(ZIP_TAIL_LEN);
    let tail_start = file_len - tail_len;
    reader.seek(SeekFrom::Start(tail_start))?;
    let mut tail = vec![0; tail_len as usize];
    reader.read_exact(&mut tail)?;

    let last_candidate = tail.len() - EOCD_LEN as usize;
    let entry_count = (0..=last_candidate)
        .rev()
        .filter(|&offset| tail[offset..].starts_with(&EOCD_SIGNATURE))
        .filter_map(|offset| eocd_entry_count(&tail, tail_start, file_len, offset))
        .max();

    entry_count.ok_or_else(|| invalid_zip("valid end of central directory not found"))
}

// File and Cursor readers share the same physical directory scan. Indexed
// metadata supplies zip's actual UTF-8/CP437 decoder for each raw name, so
// this guard adds no second name encoding policy.
fn physical_duplicate_part_name<R: Read + Seek>(
    reader: &mut R,
    directory_start: u64,
    count: u64,
    metadata: &zip::read::ZipArchiveMetadata,
) -> Result<Option<String>> {
    let mut decoded = HashMap::new();
    for index in 0..metadata.len() {
        let entry = metadata.entry(index)?;
        let name = if entry.is_dir() {
            None
        } else {
            Some(
                normalize_part_name(entry.name()?.as_ref())
                    .trim_start_matches('/')
                    .to_owned(),
            )
        };
        decoded.insert(entry.name_raw().to_vec(), name);
    }
    reader.seek(SeekFrom::Start(directory_start))?;
    let mut identities = HashSet::new();
    for _ in 0..count {
        let mut header = [0; 46];
        reader.read_exact(&mut header)?;
        if !header.starts_with(b"PK\x01\x02") {
            return Err(invalid_zip("invalid central directory entry"));
        }
        let name_length = usize::from(read_u16(&header, 28).expect("fixed central header"));
        let mut raw_name = vec![0; name_length];
        reader.read_exact(&mut raw_name)?;
        let name = decoded
            .get(&raw_name)
            .ok_or_else(|| invalid_zip("central directory name is not indexed"))?;
        if let Some(name) = name
            && !identities.insert(name.to_ascii_lowercase())
        {
            return Ok(Some(name.clone()));
        }
        let extra = u32::from(read_u16(&header, 30).expect("fixed central header"));
        let comment = u32::from(read_u16(&header, 32).expect("fixed central header"));
        reader.seek(SeekFrom::Current(i64::from(extra + comment)))?;
    }
    Ok(None)
}

fn eocd_entry_count(tail: &[u8], tail_start: u64, file_len: u64, offset: usize) -> Option<u64> {
    let eocd = tail.get(offset..offset.checked_add(EOCD_LEN as usize)?)?;
    let comment_len = u64::from(read_u16(eocd, 20)?);
    let absolute_offset = tail_start.checked_add(offset as u64)?;
    if absolute_offset
        .checked_add(EOCD_LEN)?
        .checked_add(comment_len)?
        > file_len
    {
        return None;
    }

    let disk_number = read_u16(eocd, 4)?;
    let central_directory_disk = read_u16(eocd, 6)?;
    if disk_number != 0 || central_directory_disk != 0 {
        return None;
    }

    let entries_on_disk = read_u16(eocd, 8)?;
    let total_entries = read_u16(eocd, 10)?;
    let central_directory_size = read_u32(eocd, 12)?;
    let central_directory_offset = read_u32(eocd, 16)?;
    let uses_zip64 = entries_on_disk == u16::MAX
        || total_entries == u16::MAX
        || central_directory_size == u32::MAX
        || central_directory_offset == u32::MAX;

    if uses_zip64 {
        return zip64_entry_count(tail, tail_start, absolute_offset);
    }
    if entries_on_disk != total_entries
        || !central_directory_bounds_are_plausible(
            absolute_offset,
            u64::from(central_directory_offset),
            u64::from(central_directory_size),
        )
    {
        return None;
    }

    Some(u64::from(total_entries))
}

fn zip64_entry_count(tail: &[u8], tail_start: u64, eocd_offset: u64) -> Option<u64> {
    let locator_offset = eocd_offset.checked_sub(ZIP64_EOCD_LOCATOR_LEN)?;
    let locator = tail_slice(tail, tail_start, locator_offset, ZIP64_EOCD_LOCATOR_LEN)?;
    if !locator.starts_with(&ZIP64_EOCD_LOCATOR_SIGNATURE)
        || read_u32(locator, 4)? != 0
        || read_u32(locator, 16)? != 1
    {
        return None;
    }

    let zip64_offset = read_u64(locator, 8)?;
    let zip64 = tail_slice(tail, tail_start, zip64_offset, MIN_ZIP64_EOCD_LEN)?;
    if !zip64.starts_with(&ZIP64_EOCD_SIGNATURE) {
        return None;
    }
    let record_size = read_u64(zip64, 4)?;
    if record_size < 44
        || zip64_offset.checked_add(12)?.checked_add(record_size)? != locator_offset
        || read_u32(zip64, 16)? != 0
        || read_u32(zip64, 20)? != 0
    {
        return None;
    }

    let entries_on_disk = read_u64(zip64, 24)?;
    let total_entries = read_u64(zip64, 32)?;
    if entries_on_disk != total_entries
        || !central_directory_bounds_are_plausible(
            zip64_offset,
            read_u64(zip64, 48)?,
            read_u64(zip64, 40)?,
        )
    {
        return None;
    }

    Some(total_entries)
}

fn central_directory_bounds_are_plausible(
    directory_end: u64,
    relative_directory_offset: u64,
    directory_size: u64,
) -> bool {
    directory_end
        .checked_sub(directory_size)
        .is_some_and(|actual_directory_offset| relative_directory_offset <= actual_directory_offset)
}

fn tail_slice(tail: &[u8], tail_start: u64, absolute_offset: u64, len: u64) -> Option<&[u8]> {
    let offset = usize::try_from(absolute_offset.checked_sub(tail_start)?).ok()?;
    let len = usize::try_from(len).ok()?;
    tail.get(offset..offset.checked_add(len)?)
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset.checked_add(8)?)?.try_into().ok()?,
    ))
}

fn invalid_zip(detail: &'static str) -> OpcError {
    OpcError::Zip(zip::result::ZipError::InvalidArchive(detail.into()))
}

impl Default for OpcPackage {
    fn default() -> Self {
        Self::new()
    }
}

/// Replace the file at `path` with `bytes` through a synced sibling file.
///
/// The bytes are written to `.{file name}.{staging_tag}-{process id}-{attempt}.tmp`
/// beside the file being replaced, synced, and renamed over it, so readers see
/// the old file or the new one and never a partial write. A file name too long
/// for that staging name is shortened in it. Any failure removes the staged
/// file and leaves the destination as it was. A symbolic link at `path` is
/// followed and kept, and the file it names is replaced. On Unix the
/// replacement keeps the permission bits of the file it replaces, from the
/// moment the staged file is created. The owner, extended attributes, and
/// other hard links of that file are not carried over. A file this process
/// cannot open for writing is refused and left as it was.
///
/// A path that names a device or a FIFO, such as `/dev/null`, has no file to
/// replace. It takes the bytes in place, as a plain write would.
///
/// `staging_tag` is a fragment of a file name and must not contain a path
/// separator.
///
/// # Errors
///
/// Returns `invalid_name_message` when `path` names no file, the error of
/// opening the file to replace for writing, such as
/// [`PermissionDenied`](std::io::ErrorKind::PermissionDenied) when it is
/// read-only, an
/// [`InvalidInput`](std::io::ErrorKind::InvalidInput) error when `path` goes
/// through more than 40 symbolic links, `exhausted_message` when every staging
/// name is taken, and the I/O error of a failed write, staging, sync, or
/// rename.
pub fn write_atomic_file(
    path: &Path,
    bytes: &[u8],
    staging_tag: &str,
    invalid_name_message: &'static str,
    exhausted_message: &'static str,
) -> std::io::Result<()> {
    // Renaming over a device or a FIFO would put a regular file in its place.
    // The kernel follows every link here, including /dev/stdout.
    if std::fs::metadata(path).is_ok_and(|metadata| !metadata.is_file() && !metadata.is_dir()) {
        return std::fs::write(path, bytes);
    }
    let path = resolve_symbolic_links(path)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, invalid_name_message)
    })?;
    let existing = std::fs::metadata(&path).ok();
    // A rename needs write access to the directory, not to the file it
    // replaces. Opening the file for writing, without truncating it, refuses
    // the files the in-place write refused, such as a read-only file or
    // another user's file.
    if existing.as_ref().is_some_and(std::fs::Metadata::is_file) {
        std::fs::OpenOptions::new().write(true).open(&path)?;
    }
    // Only Unix mode bits carry over. On Windows the staged file keeps its
    // default attributes.
    let permissions = existing
        .filter(|_| cfg!(unix))
        .map(|metadata| metadata.permissions());
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    // Creating the staged file with the kept mode means it is never more open
    // than the file it replaces. The umask can narrow that mode, so it is set
    // again once the file exists.
    #[cfg(unix)]
    if let Some(permissions) = &permissions {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        options.mode(permissions.mode());
    }
    // wasm targets have no process id, and std panics when asked for one. The
    // retry below covers two processes that share a staging name.
    let process_id = if cfg!(target_family = "wasm") {
        0
    } else {
        std::process::id()
    };
    for attempt in 0..128_u8 {
        let temporary = parent.join(staging_file_name(
            file_name,
            &format!(".{staging_tag}-{process_id}-{attempt}.tmp"),
        ));
        let mut file = match options.open(&temporary) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let result = permissions
            .clone()
            .map_or(Ok(()), |permissions| file.set_permissions(permissions))
            .and_then(|()| file.write_all(bytes))
            .and_then(|()| file.sync_all());
        drop(file);
        let result = result.and_then(|()| replace_file(&temporary, &path));
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        return result;
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        exhausted_message,
    ))
}

/// Name a staged file `.{file name}{suffix}`, keeping it within 255 bytes.
///
/// Common file systems refuse a longer name, so a long file name keeps only
/// the prefix that fits, cut on a character boundary.
fn staging_file_name(file_name: &std::ffi::OsStr, suffix: &str) -> std::ffi::OsString {
    let available = 255_usize.saturating_sub(1 + suffix.len());
    let mut name = std::ffi::OsString::from(".");
    if file_name.len() <= available {
        name.push(file_name);
    } else {
        let file_name = file_name.to_string_lossy();
        let mut end = available.min(file_name.len());
        while !file_name.is_char_boundary(end) {
            end -= 1;
        }
        name.push(&file_name[..end]);
    }
    name.push(suffix);
    name
}

/// Follow symbolic links from `path` to the file a save replaces.
///
/// A relative link target resolves against the directory holding the link,
/// and a dangling link resolves to the missing file it names.
fn resolve_symbolic_links(path: &Path) -> std::io::Result<PathBuf> {
    let mut resolved = path.to_path_buf();
    // Up to 40 links are followed, the Linux limit, so 41 paths are checked.
    // A longer chain is treated as a loop.
    for _ in 0..=40 {
        match std::fs::symlink_metadata(&resolved) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let target = std::fs::read_link(&resolved)?;
                resolved = match resolved.parent() {
                    Some(parent) => parent.join(target),
                    None => target,
                };
            }
            _ => return Ok(resolved),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "too many levels of symbolic links",
    ))
}

#[cfg(not(target_os = "windows"))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(target_os = "windows")]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: both path buffers are NUL-terminated and remain alive for the call.
    let replaced = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Collapse `.` and `..` segments in an absolute part name.
///
/// A `..` that would escape the package root is dropped, matching how OPC
/// consumers treat over-long parent traversals.
fn normalize_part_name(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }

    let mut out = String::with_capacity(path.len());
    for segment in segments {
        out.push('/');
        out.push_str(segment);
    }
    if out.is_empty() { "/".to_string() } else { out }
}

/// Convert a .rels file path to the part name it belongs to.
/// e.g. "word/_rels/document.xml.rels" → "/word/document.xml"
fn rels_path_to_part_name(rels_path: &str) -> String {
    // Strip the ".rels" suffix once (`trim_end_matches` would strip repeats,
    // turning "a.rels.rels" into "a") and drop only the "_rels" path segment
    // that directly precedes the file name.
    let lowercase = rels_path.to_ascii_lowercase();
    let without_suffix = if lowercase.ends_with(".rels") {
        &rels_path[..rels_path.len() - ".rels".len()]
    } else {
        rels_path
    };
    let lowercase = without_suffix.to_ascii_lowercase();
    let path = match lowercase.rfind("_rels/") {
        Some(pos) => format!(
            "{}{}",
            &without_suffix[..pos],
            &without_suffix[pos + "_rels/".len()..]
        ),
        None => without_suffix.to_string(),
    };
    if path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    }
}

/// Refuse a ZIP entry name that pinned `zip` 9.0.3 `ZipWriter::start_file`
/// rejects before it writes the payload.
///
/// The local-header types are private. `SimpleFileOptions::default` adds no
/// extra field, and the fixed local header is the 30-byte ZIP local file
/// header. Their sum with the file name must stay below `u16::MAX`. The
/// boundary is pinned by `zip_entries_and_write_to_share_pinned_zip_name_limit`
/// against that writer, not against this constant alone.
fn reject_zip_local_entry_name(name: &str) -> Result<()> {
    const LOCAL_HEADER_WITHOUT_NAME_OR_EXTRA: usize = 30;
    if LOCAL_HEADER_WITHOUT_NAME_OR_EXTRA.saturating_add(name.len()) >= usize::from(u16::MAX) {
        return Err(zip::result::ZipError::InvalidArchive(
            "ZipLocalBlock + filename + extra fields must be less than 64KiB when combined".into(),
        )
        .into());
    }
    Ok(())
}

/// Convert a part name to its .rels file path.
/// e.g. "/word/document.xml" → "word/_rels/document.xml.rels"
fn part_name_to_rels_path(part_name: &str) -> String {
    let name = part_name.strip_prefix('/').unwrap_or(part_name);
    if let Some(pos) = name.rfind('/') {
        let dir = &name[..pos];
        let file = &name[pos + 1..];
        format!("{dir}/_rels/{file}.rels")
    } else {
        format!("_rels/{name}.rels")
    }
}
