//! Fonts a document declares and embeds: definitions, embedding licences and embedded fonts.

use super::*;

/// One font-table record authored through the native facade.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FontDefinition {
    pub name: String,
    pub alternate_name: Option<String>,
    pub family: Option<String>,
    pub pitch: Option<String>,
    pub embedded_fonts: Vec<EmbeddedFont>,
}

impl FontDefinition {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            alternate_name: None,
            family: None,
            pitch: None,
            embedded_fonts: Vec::new(),
        }
    }

    pub fn with_alternate_name(mut self, name: impl Into<String>) -> Self {
        self.alternate_name = Some(name.into());
        self
    }

    pub fn with_family(mut self, family: impl Into<String>) -> Self {
        self.family = Some(family.into());
        self
    }

    pub fn with_pitch(mut self, pitch: impl Into<String>) -> Self {
        self.pitch = Some(pitch.into());
        self
    }
}

/// The font-table slot occupied by an embedded font face.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbeddedFontKind {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

/// The caller's explicit authorization and exact license identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FontEmbeddingLicense {
    pub authorized: bool,
    pub identity: String,
}

impl FontEmbeddingLicense {
    pub fn new(authorized: bool, identity: impl Into<String>) -> Self {
        Self {
            authorized,
            identity: identity.into(),
        }
    }
}

/// Caller-owned bytes and metadata for one embedded font face.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddedFont {
    pub kind: EmbeddedFontKind,
    pub data: Vec<u8>,
    pub font_key: String,
    pub subsetted: bool,
    pub license: FontEmbeddingLicense,
}

/// Derive a deterministic uppercase OOXML key from checked single-face font bytes.
pub fn derive_font_key(data: &[u8]) -> Result<String> {
    use sha2::{Digest, Sha256};
    if !valid_authored_sfnt(data) {
        return Err(Error::Other(
            "font-key derivation requires a bounded supported sfnt font".to_owned(),
        ));
    }
    let digest = Sha256::digest(data);
    let hex = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>();
    Ok(format!(
        "{{{}-{}-{}-{}-{}}}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

impl EmbeddedFont {
    pub fn new(
        kind: EmbeddedFontKind,
        data: Vec<u8>,
        font_key: impl Into<String>,
        license: FontEmbeddingLicense,
    ) -> Self {
        Self {
            kind,
            data,
            font_key: font_key.into(),
            subsetted: false,
            license,
        }
    }

    pub fn with_subsetted(mut self, subsetted: bool) -> Self {
        self.subsetted = subsetted;
        self
    }
}

impl From<EmbeddedFontKind> for FontFaceKind {
    fn from(value: EmbeddedFontKind) -> Self {
        match value {
            EmbeddedFontKind::Regular => Self::Regular,
            EmbeddedFontKind::Bold => Self::Bold,
            EmbeddedFontKind::Italic => Self::Italic,
            EmbeddedFontKind::BoldItalic => Self::BoldItalic,
        }
    }
}

impl From<FontFaceKind> for EmbeddedFontKind {
    fn from(value: FontFaceKind) -> Self {
        match value {
            FontFaceKind::Regular => Self::Regular,
            FontFaceKind::Bold => Self::Bold,
            FontFaceKind::Italic => Self::Italic,
            FontFaceKind::BoldItalic => Self::BoldItalic,
        }
    }
}
