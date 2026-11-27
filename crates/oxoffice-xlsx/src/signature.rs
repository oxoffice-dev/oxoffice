//! The policy of docx and pptx for a package signature (`_xmlsignatures`),
//! which `oxoffice-opc::embedded` holds: a signature covers the bytes of the
//! parts it names, so a save that changes any part or relationship makes it
//! invalid. The signature parts are kept as evidence and the package is
//! marked, once, by a relationship from the package to the signature origin
//! (`invalidated-package-signature`); a package opened with that mark, or with
//! a signature that does not verify, has its signature known invalid and
//! carries the mark through every later save. A workbook that is not signed
//! is not touched by any of it.

use std::sync::Arc;

use oxoffice_opc::OpcPackage;
use oxoffice_opc::embedded;

use crate::{Error, Result};

/// Whether a package just opened carries a package signature known to be
/// invalid: marked so by an earlier save, or failing verification.
#[cfg(feature = "digital-signatures")]
pub(crate) fn known_invalid_on_open(package: &OpcPackage) -> bool {
    embedded::known_invalid_package_signature_on_open(package)
}

/// Whether a package just opened carries a package signature known to be
/// invalid, which, without the `digital-signatures` feature, is told by the
/// mark and by the parts the signature names that the package lacks.
#[cfg(not(feature = "digital-signatures"))]
pub(crate) fn known_invalid_on_open(package: &OpcPackage) -> bool {
    embedded::package_signature_known_invalid_without_verifying(package)
}

/// The package a later save is compared with, when it carries a package
/// signature.
pub(crate) fn baseline(package: &OpcPackage) -> Option<Arc<OpcPackage>> {
    embedded::has_package_signature(package)
        .unwrap_or(false)
        .then(|| Arc::new(package.clone()))
}

/// Whether the package a save writes holds the parts, content types and
/// relationships of the one it was opened with: only then a signature stays
/// valid.
pub(crate) fn unchanged(saved: &OpcPackage, baseline: &OpcPackage) -> bool {
    embedded::packages_are_semantically_equal(saved, baseline)
}

/// Marks the package signature of `package` invalidated, once.
pub(crate) fn mark_invalidated(package: &mut OpcPackage) -> Result<()> {
    embedded::persist_invalidated_package_signature(package, true)
        .map_err(|error| Error::Refused(error.to_string()))
}
