//! A package's parts read and written in their order, and a package whose
//! XML parts are written another way, each proved to hold what it held.

use std::io::{Cursor, Read, Write};

use crate::canonical::{canonical, first_difference};
use crate::writing::{Writing, rewrite};

/// Every part of a package by its ZIP entry name, with its bytes, in the
/// order of the entries.
pub fn package_parts(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let mut parts = Vec::with_capacity(zip.len());
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|error| error.to_string())?;
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|error| error.to_string())?;
        let name = entry
            .name()
            .map_err(|error| error.to_string())?
            .into_owned();
        parts.push((name, data));
    }
    Ok(parts)
}

/// A package holding these parts, in this order.
pub fn write_package(parts: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in parts {
        out.start_file(name.as_str(), zip::write::SimpleFileOptions::DEFAULT)
            .expect("a ZIP entry starts");
        out.write_all(data).expect("a ZIP entry is written");
    }
    out.finish().expect("the ZIP is written").into_inner()
}

/// Whether [`rewrite_package`] rewrites this part: every XML part of the
/// WordprocessingML, PresentationML and DrawingML vocabularies (the main
/// part, styles, numbering, settings, headers and footers, notes, comments,
/// themes, charts, slides, layouts, masters, notes slides...), not the
/// package's own parts (content types, relationships), the document
/// properties or custom XML data, which other readers parse.
pub fn rewritten_part(name: &str) -> bool {
    name.ends_with(".xml")
        && name != "[Content_Types].xml"
        && !name.starts_with("docProps/")
        && !name.starts_with("customXml/")
        && !name.contains("_rels/")
}

/// A package with its XML parts written another way.
pub struct RewrittenPackage {
    pub bytes: Vec<u8>,
    /// The parts the rewrite changed, by ZIP entry name.
    pub parts: Vec<String>,
    /// How many places it changed in them.
    pub changes: usize,
}

/// `bytes` with every part [`rewritten_part`] names written as `writing`
/// says. Each part it changes must read the same as the original in the
/// canonical form, or the rewrite is refused: the rewrite would prove
/// nothing.
pub fn rewrite_package(bytes: &[u8], writing: Writing) -> Result<RewrittenPackage, String> {
    let mut parts = package_parts(bytes)?;
    let (mut changed, mut changes) = (Vec::new(), 0);
    for (name, data) in &mut parts {
        if !rewritten_part(name) {
            continue;
        }
        let rewritten = rewrite(data, writing)
            .map_err(|error| format!("{name}: {} refused: {error}", writing.name()))?;
        if rewritten.xml == *data {
            continue;
        }
        let before = canonical(data).map_err(|error| format!("{name} reads: {error}"))?;
        let after = canonical(&rewritten.xml)
            .map_err(|error| format!("{name} {} reads: {error}", writing.name()))?;
        if let Some(difference) = first_difference(&before, &after) {
            return Err(format!(
                "{name}: {} changes what the part says, {difference}",
                writing.name()
            ));
        }
        *data = rewritten.xml;
        changed.push(name.clone());
        changes += rewritten.changes;
    }
    Ok(RewrittenPackage {
        bytes: write_package(&parts),
        parts: changed,
        changes,
    })
}
