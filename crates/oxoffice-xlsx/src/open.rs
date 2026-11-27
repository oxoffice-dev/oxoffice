//! Opening a workbook: from a path or bytes, password-protected or not, the
//! package told by its content.

use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use oxoffice_opc::{CompoundFile, OpcError, OpcPackage, compound_file};
use oxoffice_xlsx_oxml::namespaces::{content_types, rel_types, transitional_relationship_type};
use oxoffice_xlsx_oxml::workbook::WorkbookXml;

use crate::save::WorkbookClass;
use crate::sheet::SheetEntry;
use crate::signature;
use crate::{Error, Result, Workbook};

impl Workbook {
    /// Opens a workbook from a path.
    ///
    /// The workbook remembers the file, and [`Workbook::save`] refuses to
    /// replace it unless asked, see [`Workbook::save_with_options`].
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let bytes = std::fs::read(path.as_ref())?;
        let mut workbook = Self::from_bytes(&bytes)?;
        workbook.remember_source(path.as_ref());
        Ok(workbook)
    }

    /// Opens a workbook from the bytes of a file.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        refuse_compound_file(bytes)?;
        Self::from_package(read_package(bytes)?)
    }

    /// Opens a password-protected workbook (Agile encryption) from a path.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn open_encrypted<P: AsRef<Path>>(path: P, password: &str) -> Result<Self> {
        let bytes = std::fs::read(path.as_ref())?;
        let mut workbook = Self::from_encrypted_bytes(&bytes, password)?;
        workbook.remember_source(path.as_ref());
        Ok(workbook)
    }

    /// Opens a password-protected workbook (Agile encryption) from the bytes
    /// of a file. A workbook that is not password-protected is refused.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn from_encrypted_bytes(bytes: &[u8], password: &str) -> Result<Self> {
        // A compound file whose directory does not read as one of the known
        // kinds may still decrypt: the decryption says what it lacks.
        match refuse_compound_file(bytes) {
            Err(Error::PasswordProtected | Error::CompoundFile) => {}
            Err(error) => return Err(error),
            Ok(()) => return Err(Error::NotPasswordProtected),
        }
        let package = OpcPackage::from_encrypted_reader(Cursor::new(bytes), password)?;
        Self::from_package(package)
    }

    /// Records `path` as the file this workbook was opened from. A path that
    /// cannot be resolved records nothing.
    fn remember_source(&mut self, path: &Path) {
        self.source_path = std::fs::canonicalize(path).ok();
    }

    /// The workbook a package holds, or the refusal its main content type
    /// names.
    pub(crate) fn from_package(package: OpcPackage) -> Result<Self> {
        let workbook_part = package
            .package_rels
            .items
            .iter()
            .find(|relationship| {
                transitional_relationship_type(&relationship.rel_type) == rel_types::OFFICE_DOCUMENT
                    && !is_external(relationship)
            })
            .map(|relationship| OpcPackage::resolve_rel_target("/", &relationship.target))
            .ok_or(Error::NotAWorkbook { content_type: None })?;
        let content_type = package
            .content_types
            .content_type_for(&workbook_part)
            .map(str::to_owned);
        let class = classify(content_type.as_deref())?;
        let source = package
            .get_part_shared(&workbook_part)
            .cloned()
            .ok_or_else(|| Error::MissingPart {
                part: workbook_part.clone(),
            })?;
        let workbook =
            WorkbookXml::parse(Arc::clone(&source)).map_err(|error| Error::MalformedPart {
                part: workbook_part.clone(),
                message: error.to_string(),
            })?;
        let sheets = SheetEntry::resolve(&package, &workbook_part, &workbook)?;
        let signature_invalid = signature::known_invalid_on_open(&package);
        let signature_baseline = signature::baseline(&package);
        Ok(Workbook {
            package,
            source_path: None,
            workbook_part,
            class,
            workbook,
            sheets,
            edited: false,
            calculates: false,
            formulas: OnceLock::new(),
            signature_invalid,
            signature_baseline,
        })
    }
}

/// Whether a relationship names a target outside the package.
pub(crate) fn is_external(relationship: &oxoffice_opc::Relationship) -> bool {
    relationship
        .target_mode
        .as_deref()
        .is_some_and(|mode| mode.eq_ignore_ascii_case("external"))
}

/// The class a main content type names, or the refusal of what it is.
fn classify(content_type: Option<&str>) -> Result<WorkbookClass> {
    let lower = content_type
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if let Some(class) = WorkbookClass::from_content_type(&lower) {
        return Ok(class);
    }
    Err(
        if lower == content_types::BINARY_WORKBOOK.to_ascii_lowercase() {
            Error::BinaryWorkbook
        } else if lower.contains("wordprocessingml") || lower.contains("ms-word") {
            Error::WordDocument
        } else if lower.contains("presentationml") || lower.contains("ms-powerpoint") {
            Error::PowerPointPresentation
        } else {
            Error::NotAWorkbook {
                content_type: content_type.map(str::to_owned),
            }
        },
    )
}

/// Refuses a compound file by what it holds.
fn refuse_compound_file(bytes: &[u8]) -> Result<()> {
    match compound_file(bytes) {
        None => Ok(()),
        Some(CompoundFile::Encrypted) => Err(Error::PasswordProtected),
        Some(CompoundFile::Workbook) => Err(Error::LegacyWorkbook),
        Some(CompoundFile::Document) => Err(Error::LegacyDocument),
        Some(CompoundFile::Presentation) => Err(Error::LegacyPresentation),
        Some(CompoundFile::Other) => Err(Error::CompoundFile),
    }
}

/// The package `bytes` hold; an OpenDocument file, which has no content
/// types, is told by its `mimetype`.
fn read_package(bytes: &[u8]) -> Result<OpcPackage> {
    match OpcPackage::from_reader(Cursor::new(bytes)) {
        Ok(package) => Ok(package),
        Err(OpcError::PartNotFound(part)) => Err(match opendocument_type(bytes) {
            Some(mimetype)
                if mimetype.starts_with("application/vnd.oasis.opendocument.spreadsheet") =>
            {
                Error::OpenDocumentSpreadsheet
            }
            Some(mimetype) => Error::NotAWorkbook {
                content_type: Some(mimetype),
            },
            None => Error::Package(OpcError::PartNotFound(part)),
        }),
        Err(error) => Err(error.into()),
    }
}

/// The `mimetype` entry of an OpenDocument package.
fn opendocument_type(bytes: &[u8]) -> Option<String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut entry = archive.by_name("mimetype").ok()?;
    let mut mimetype = String::new();
    entry
        .by_ref()
        .take(256)
        .read_to_string(&mut mimetype)
        .ok()?;
    Some(mimetype.trim().to_owned())
}
