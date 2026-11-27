//! Equivalent writings of the XML parts of a package
//! (`specs/2026-11-25-equivalent-xml-writings.md`): the same XML written the
//! way another producer writes it, the canonical form that proves a rewrite
//! says what the original says, and the check both facades' writings tests
//! run. A dev-dependency of their test binaries only, never published.

mod canonical;
pub mod harness;
mod writing;

pub use canonical::{Folded, canonical, canonical_folded, first_difference};
pub use writing::{Writing, package_parts, rewrite, rewrite_package, write_package};
