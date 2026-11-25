//! Equivalent writings of the XML parts of a package
//! (`specs/2026-11-25-equivalent-xml-writings.md`): the same XML written the
//! way another producer may write it (an empty element with an end tag, other
//! prefixes, the default namespace, indentation, another attribute order or
//! quote, character references), and the canonical form that proves a
//! rewrite holds what the original holds. A dev-dependency of the facades'
//! test binaries only, never published; it depends on neither facade.

mod canonical;
pub mod harness;
mod package;
mod tokens;
mod writing;

pub use canonical::{canonical, first_difference};
pub use package::{
    RewrittenPackage, package_parts, rewrite_package, rewritten_part, write_package,
};
pub use writing::{MAIN_NAMESPACES, Rewritten, Writing, rewrite};
