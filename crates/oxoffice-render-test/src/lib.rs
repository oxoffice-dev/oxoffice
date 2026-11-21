//! The pieces the render tests of the docx and pptx test binaries share
//! (`specs/2026-11-21-render-fidelity.md`): a PDF read back, the checks a
//! coherent render passes, the hashes that say whether a render changed and
//! the files they are kept in under `tests/render/`. A dev-dependency only,
//! never published; it depends on neither facade.

pub mod checks;
pub mod fingerprint;
pub mod pdf;
pub mod xml;
