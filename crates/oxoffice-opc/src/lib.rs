//! Open Packaging Convention (OPC) reader/writer for OOXML packages.
//!
//! This crate handles the ZIP archive layer of OOXML files, including:
//! - Reading and writing ZIP-based OPC packages
//! - Parsing `[Content_Types].xml`
//! - Parsing `.rels` relationship files
//! - Navigating parts by URI and resolving relationships
//! - Checking embedded content (OLE, ActiveX, VBA) and the package
//!   signature graph around it
//! - Telling what a compound file holds, so that a facade refuses a legacy
//!   or password-protected file by name

pub mod compound;
pub mod content_types;
pub mod embedded;
#[cfg(feature = "agile-encryption")]
mod encryption;
mod error;
mod package;
pub mod relationship;
#[cfg(feature = "digital-signatures")]
mod signature;

pub use compound::{CompoundFile, compound_file};
pub use content_types::{ContentType, ContentTypes};
pub use error::OpcError;
pub use package::{
    OpcPackage, PackagePart, PackageReadLimits, PartJournal, SaveOptions, is_same_file,
    write_atomic_file,
};
pub use relationship::{Relationship, Relationships};
#[cfg(feature = "digital-signatures")]
pub use signature::{
    CoveredRelationship, SignatureIssue, SignatureReport, SignerCertificateIdentity,
};
