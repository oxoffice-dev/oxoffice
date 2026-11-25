//! Byte searches over the pieces of a body kept as raw XML.
//!
//! The typed tree does not see inside these pieces: text boxes kept in a
//! run's `mc:AlternateContent` or `w:pict`, drawings kept as source,
//! unknown paragraph, table, row and cell children, revision wrappers and
//! raw content-control content. A caller that answers a question from the
//! typed tree asks here whether such a piece could change the answer.

use crate::content_control::{CT_Sdt, SdtContent};
use crate::document::{BodyContent, CT_Body};
use crate::table::{CT_Row, CT_Tbl, CT_Tc, CellContent};
use crate::text::{CT_P, CT_R, RunContent};

pub(crate) fn bytes_contain(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return false;
    }
    // Raw XML is UTF-8: the string search of the standard library, built
    // optimized whatever the profile, keeps the walk cheap in an
    // unoptimized build too.
    match (std::str::from_utf8(haystack), std::str::from_utf8(needle)) {
        (Ok(haystack), Ok(needle)) => haystack.contains(needle),
        _ => haystack
            .windows(needle.len())
            .any(|window| window == needle),
    }
}

fn extras_contain<T>(extras: &[(T, Vec<u8>)], needle: &[u8]) -> bool {
    extras.iter().any(|(_, raw)| bytes_contain(raw, needle))
}

impl CT_Body {
    /// Whether a piece of the body kept as raw XML holds `needle`.
    ///
    /// The typed walks ([`CT_Body::content_controls`]) do not see inside
    /// these pieces; a `false` here says no such piece names `needle`.
    #[doc(hidden)]
    pub fn raw_xml_contains(&self, needle: &[u8]) -> bool {
        self.content.iter().any(|block| match block {
            BodyContent::Paragraph(paragraph) => paragraph_contains(paragraph, needle),
            BodyContent::Table(table) => table_contains(table, needle),
            BodyContent::ContentControl(sdt) => sdt_contains(sdt, needle),
            BodyContent::RawXml(raw) => bytes_contain(raw, needle),
        })
    }
}

fn paragraph_contains(paragraph: &CT_P, needle: &[u8]) -> bool {
    extras_contain(&paragraph.extra_xml, needle)
        || paragraph.runs.iter().any(|run| run_contains(run, needle))
        || paragraph
            .content_controls
            .iter()
            .any(|(_, _, _, sdt)| sdt_contains(sdt, needle))
        || paragraph
            .revisions
            .iter()
            .any(|(_, _, revision)| revision.raw_xml_contains(needle))
}

fn run_contains(run: &CT_R, needle: &[u8]) -> bool {
    run.extra_xml.iter().any(|raw| bytes_contain(raw, needle))
        || run.content.iter().any(|content| match content {
            RunContent::Drawing(drawing) => {
                drawing
                    .inline
                    .as_ref()
                    .and_then(|inline| inline.raw_xml.as_deref())
                    .is_some_and(|raw| bytes_contain(raw, needle))
                    || drawing
                        .anchor
                        .as_ref()
                        .and_then(|anchor| anchor.raw_xml.as_deref())
                        .is_some_and(|raw| bytes_contain(raw, needle))
            }
            _ => false,
        })
}

fn table_contains(table: &CT_Tbl, needle: &[u8]) -> bool {
    extras_contain(&table.extra_xml, needle)
        || table
            .content_controls
            .iter()
            .any(|(_, _, sdt)| sdt_contains(sdt, needle))
        || table.rows.iter().any(|row| row_contains(row, needle))
}

fn row_contains(row: &CT_Row, needle: &[u8]) -> bool {
    extras_contain(&row.extra_xml, needle)
        || row
            .content_controls
            .iter()
            .any(|(_, _, sdt)| sdt_contains(sdt, needle))
        || row.cells.iter().any(|cell| cell_contains(cell, needle))
}

fn cell_contains(cell: &CT_Tc, needle: &[u8]) -> bool {
    extras_contain(&cell.extra_xml, needle)
        || cell.content.iter().any(|content| match content {
            CellContent::Paragraph(paragraph) => paragraph_contains(paragraph, needle),
            CellContent::Table(table) => table_contains(table, needle),
            CellContent::ContentControl(sdt) => sdt_contains(sdt, needle),
        })
}

fn sdt_contains(sdt: &CT_Sdt, needle: &[u8]) -> bool {
    sdt.retained_resource_run_xml()
        .any(|(_, raw)| bytes_contain(raw, needle))
        || sdt
            .revisions
            .iter()
            .any(|(_, revision)| revision.raw_xml_contains(needle))
        || sdt.content.iter().any(|content| match content {
            SdtContent::Paragraph(paragraph) => paragraph_contains(paragraph, needle),
            SdtContent::Table(table) => table_contains(table, needle),
            SdtContent::Row(row) => row_contains(row, needle),
            SdtContent::Cell(cell) => cell_contains(cell, needle),
            SdtContent::Run(run) => run_contains(run, needle),
            SdtContent::ContentControl(nested) => sdt_contains(nested, needle),
            SdtContent::RawXml(raw) => bytes_contain(raw, needle),
        })
}
