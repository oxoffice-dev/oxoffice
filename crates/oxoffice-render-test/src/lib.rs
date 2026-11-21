//! The pieces of the render tests (`specs/2026-11-21-render-fidelity.md`)
//! that depend on no facade: a PDF read back, the checks a coherent render
//! passes, the hashes that say whether a render changed and the files they
//! are kept in under `tests/render/`. A dev-dependency of the facades' test
//! binaries only, never published; it depends on neither facade.

pub mod checks;
pub mod fingerprint;
pub mod pdf;
pub mod xml;
