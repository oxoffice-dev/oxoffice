//! Word package classes and the profiles a new document is created with.

use super::*;

/// The package class declared by a Word main document part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordPackageClass {
    /// A macro-free `.docx` document.
    Document,
    /// A macro-enabled `.docm` document.
    MacroEnabledDocument,
    /// A macro-free `.dotx` template.
    Template,
    /// A macro-enabled `.dotm` template.
    MacroEnabledTemplate,
}

/// Completeness and package class for a newly authored Word document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordCreationProfile {
    /// The compact package graph historically emitted by rdocx, the predecessor
    /// of oxoffice.
    ///
    /// It has no settings part, so it names no compatibility mode.
    Minimal(WordPackageClass),
    /// A Word-compatible blank package with the standard owned support parts.
    ///
    /// Its settings name compatibility mode 15, as Word's own new documents do.
    WordCompatible(WordPackageClass),
}

impl WordPackageClass {
    pub(crate) fn content_type(self) -> &'static str {
        match self {
            Self::Document => content_types::WORD_DOCUMENT,
            Self::MacroEnabledDocument => content_types::WORD_DOCUMENT_MACRO_ENABLED,
            Self::Template => content_types::WORD_TEMPLATE,
            Self::MacroEnabledTemplate => content_types::WORD_TEMPLATE_MACRO_ENABLED,
        }
    }

    pub(super) fn from_content_type(content_type: &str) -> Option<Self> {
        match content_type {
            content_types::WORD_DOCUMENT => Some(Self::Document),
            content_types::WORD_DOCUMENT_MACRO_ENABLED => Some(Self::MacroEnabledDocument),
            content_types::WORD_TEMPLATE => Some(Self::Template),
            content_types::WORD_TEMPLATE_MACRO_ENABLED => Some(Self::MacroEnabledTemplate),
            _ => None,
        }
    }

    /// The class that a `.docx`, `.docm`, `.dotx`, or `.dotm` path names.
    pub(super) fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "docx" => Some(Self::Document),
            "docm" => Some(Self::MacroEnabledDocument),
            "dotx" => Some(Self::Template),
            "dotm" => Some(Self::MacroEnabledTemplate),
            _ => None,
        }
    }

    pub(super) fn is_macro_enabled(self) -> bool {
        matches!(
            self,
            Self::MacroEnabledDocument | Self::MacroEnabledTemplate
        )
    }
}
