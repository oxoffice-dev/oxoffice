//! Saving a workbook: to bytes or to a file, staged in a sibling file and
//! renamed over the path, never over the file it was opened from unless
//! asked; the extension selects the class.

use std::borrow::Cow;
use std::io::Cursor;
use std::path::Path;

use oxoffice_opc::{OpcError, OpcPackage, write_atomic_file};
use oxoffice_xlsx_oxml::namespaces::{content_types, rel_types, transitional_relationship_type};

use crate::{Error, Result, Workbook};

/// What a save does beyond writing the workbook.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SaveOptions {
    /// Replace the file the workbook was opened from. Off by default: a save
    /// refuses that file, so an edit never silently replaces its input.
    /// Saves to bytes ignore it.
    pub overwrite_source: bool,
}

/// The class of a workbook package, from its main content type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkbookClass {
    /// `.xlsx`
    Workbook,
    /// `.xlsm`
    MacroEnabledWorkbook,
    /// `.xltx`
    Template,
    /// `.xltm`
    MacroEnabledTemplate,
}

impl WorkbookClass {
    pub(crate) fn from_content_type(content_type: &str) -> Option<Self> {
        [
            WorkbookClass::Workbook,
            WorkbookClass::MacroEnabledWorkbook,
            WorkbookClass::Template,
            WorkbookClass::MacroEnabledTemplate,
        ]
        .into_iter()
        .find(|class| class.content_type().eq_ignore_ascii_case(content_type))
    }

    /// The class a file extension names.
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match extension.as_str() {
            "xlsx" => WorkbookClass::Workbook,
            "xlsm" => WorkbookClass::MacroEnabledWorkbook,
            "xltx" => WorkbookClass::Template,
            "xltm" => WorkbookClass::MacroEnabledTemplate,
            _ => return None,
        })
    }

    /// The main content type of the class.
    pub fn content_type(self) -> &'static str {
        match self {
            WorkbookClass::Workbook => content_types::WORKBOOK,
            WorkbookClass::MacroEnabledWorkbook => content_types::WORKBOOK_MACRO_ENABLED,
            WorkbookClass::Template => content_types::TEMPLATE,
            WorkbookClass::MacroEnabledTemplate => content_types::TEMPLATE_MACRO_ENABLED,
        }
    }

    pub fn is_macro_enabled(self) -> bool {
        matches!(
            self,
            WorkbookClass::MacroEnabledWorkbook | WorkbookClass::MacroEnabledTemplate
        )
    }
}

impl Workbook {
    /// The bytes of the workbook as it stands, in its class.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.to_bytes_with_options(SaveOptions::default())
    }

    /// The bytes of the workbook as it stands, saved with `options`.
    pub fn to_bytes_with_options(&self, options: SaveOptions) -> Result<Vec<u8>> {
        let mut bytes = Cursor::new(Vec::new());
        self.saved_package(options, None)?.write_to(&mut bytes)?;
        Ok(bytes.into_inner())
    }

    /// Saves the workbook to `path`.
    ///
    /// The bytes are staged in a synced sibling file and renamed over `path`
    /// through [`oxoffice_opc::write_atomic_file`], so a failed save leaves
    /// an existing file as it was. A symbolic link at `path` is kept and the
    /// file it names replaced; on Unix that file keeps its permission bits.
    ///
    /// A `.xlsx`, `.xlsm`, `.xltx` or `.xltm` extension selects the main
    /// content type; any other extension keeps the class the workbook was
    /// opened with. A workbook that carries a VBA project cannot be saved
    /// under a macro-free extension ([`Error::MacroFreeExtension`]).
    ///
    /// A workbook opened from a file refuses to be saved over it, whatever
    /// the spelling of `path` (a symbolic link, a relative path, a hard
    /// link), with [`Error::OverwritesSource`];
    /// [`Workbook::save_with_options`] with
    /// [`SaveOptions::overwrite_source`] replaces it.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        self.save_with_options(path, SaveOptions::default())
    }

    /// Saves to `path`, as [`Workbook::save`] does, with `options`.
    pub fn save_with_options<P: AsRef<Path>>(&self, path: P, options: SaveOptions) -> Result<()> {
        let path = path.as_ref();
        self.refuse_source(path, options)?;
        let mut bytes = Cursor::new(Vec::new());
        self.saved_package(options, self.class_for_path(path)?)?
            .write_to(&mut bytes)?;
        publish(path, bytes.get_ref())
    }

    /// The bytes of the workbook protected by `password` (Agile encryption).
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn to_encrypted_bytes(&self, password: &str) -> Result<Vec<u8>> {
        self.to_encrypted_bytes_with_options(password, SaveOptions::default())
    }

    /// The bytes of the workbook protected by `password`, saved with
    /// `options`.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn to_encrypted_bytes_with_options(
        &self,
        password: &str,
        options: SaveOptions,
    ) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.saved_package(options, None)?
            .write_encrypted_to(&mut bytes, password)?;
        Ok(bytes)
    }

    /// Saves the workbook to `path` protected by `password`, as
    /// [`Workbook::save`] saves it.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn save_encrypted<P: AsRef<Path>>(&self, path: P, password: &str) -> Result<()> {
        self.save_encrypted_with_options(path, password, SaveOptions::default())
    }

    /// Saves to `path` protected by `password`, with `options`.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn save_encrypted_with_options<P: AsRef<Path>>(
        &self,
        path: P,
        password: &str,
        options: SaveOptions,
    ) -> Result<()> {
        let path = path.as_ref();
        self.refuse_source(path, options)?;
        let mut bytes = Vec::new();
        self.saved_package(options, self.class_for_path(path)?)?
            .write_encrypted_to(&mut bytes, password)?;
        publish(path, &bytes)
    }

    fn refuse_source(&self, path: &Path, options: SaveOptions) -> Result<()> {
        if !options.overwrite_source
            && let Some(source) = &self.source_path
            && oxoffice_opc::is_same_file(source, path)
        {
            return Err(Error::OverwritesSource {
                path: path.to_path_buf(),
            });
        }
        Ok(())
    }

    /// The class a save to `path` writes, when it is not the workbook's.
    fn class_for_path(&self, path: &Path) -> Result<Option<WorkbookClass>> {
        let Some(target) = WorkbookClass::from_path(path).filter(|target| *target != self.class)
        else {
            return Ok(None);
        };
        let carries_vba =
            self.package
                .get_part_rels(&self.workbook_part)
                .is_some_and(|relationships| {
                    relationships.items.iter().any(|relationship| {
                        transitional_relationship_type(&relationship.rel_type)
                            == rel_types::VBA_PROJECT
                    })
                });
        if carries_vba && !target.is_macro_enabled() {
            return Err(Error::MacroFreeExtension {
                path: path.to_path_buf(),
            });
        }
        Ok(Some(target))
    }

    /// The package a save writes: the workbook's own while nothing changes
    /// it, else a copy with the changes made ([`Workbook::write_staged_parts`]).
    fn saved_package(
        &self,
        _options: SaveOptions,
        class: Option<WorkbookClass>,
    ) -> Result<Cow<'_, OpcPackage>> {
        if class.is_none() && !self.edited {
            return Ok(Cow::Borrowed(&self.package));
        }
        let mut package = self.package.clone();
        if self.edited {
            self.write_staged_parts(&mut package)?;
        }
        if let Some(class) = class {
            package
                .content_types
                .add_override(&self.workbook_part, class.content_type());
        }
        Ok(Cow::Owned(package))
    }
}

fn publish(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic_file(
        path,
        bytes,
        "oxoffice",
        "invalid workbook file name",
        "could not allocate workbook save staging file",
    )
    .map_err(OpcError::from)?;
    Ok(())
}
