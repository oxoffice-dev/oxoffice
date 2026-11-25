//! Byte searches over the pieces of a body kept as raw XML.
//!
//! The typed tree does not see inside these pieces: text boxes kept in a
//! run's `mc:AlternateContent` or `w:pict`, drawings kept as source,
//! unknown paragraph, table, row and cell children, revision wrappers and
//! raw content-control content. A caller that answers a question from the
//! typed tree asks here whether such a piece could change the answer.

use crate::content_control::{CT_Sdt, SdtContent};
use crate::document::{BodyContent, CT_Body};
use crate::drawing::CT_Drawing;
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

/// What a walk asks of each piece: whether a piece kept as raw XML holds
/// `needle`, or whether `drawing` says a typed drawing answers.
struct Probe<'a> {
    needle: &'a [u8],
    drawing: &'a dyn Fn(&CT_Drawing) -> bool,
    /// Also look inside field results (their source and result runs) and
    /// the raw children of hyperlinks, where a picture can sit too.
    deep: bool,
}

impl Probe<'_> {
    fn raw(&self, raw: &[u8]) -> bool {
        bytes_contain(raw, self.needle)
    }

    fn extras<T>(&self, extras: &[(T, Vec<u8>)]) -> bool {
        extras.iter().any(|(_, raw)| self.raw(raw))
    }
}

impl CT_Body {
    /// Whether a piece of the body kept as raw XML holds `needle`.
    ///
    /// The typed walks ([`CT_Body::content_controls`]) do not see inside
    /// these pieces; a `false` here says no such piece names `needle`.
    #[doc(hidden)]
    pub fn raw_xml_contains(&self, needle: &[u8]) -> bool {
        self.content
            .iter()
            .any(|block| block.raw_xml_contains(needle))
    }
}

impl BodyContent {
    /// Whether a piece of this block kept as raw XML holds `needle`, as
    /// [`CT_Body::raw_xml_contains`] asks of every block.
    #[doc(hidden)]
    pub fn raw_xml_contains(&self, needle: &[u8]) -> bool {
        block_contains(
            self,
            &Probe {
                needle,
                drawing: &|_| false,
                deep: false,
            },
        )
    }

    /// Whether this block may show the relationship `id`: a typed drawing
    /// of its tree names it (`r:embed` or `r:link`), or a piece kept as raw
    /// XML, a drawing's source included, holds its bytes. Field results and
    /// the raw children of hyperlinks are looked into. `false` says no
    /// drawing of the block shows it; `true` may be a piece that names it
    /// otherwise.
    #[doc(hidden)]
    pub fn may_show_relationship(&self, id: &str) -> bool {
        let names = |embed: &str, link: Option<&str>| embed == id || link == Some(id);
        block_contains(
            self,
            &Probe {
                needle: id.as_bytes(),
                deep: true,
                drawing: &|drawing| {
                    drawing
                        .inline
                        .as_ref()
                        .is_some_and(|inline| names(&inline.embed_id, inline.link_id.as_deref()))
                        || drawing.anchor.as_ref().is_some_and(|anchor| {
                            names(&anchor.embed_id, anchor.link_id.as_deref())
                        })
                },
            },
        )
    }
}

impl BodyContent {
    /// Whether a typed inline drawing of this block still waits for the
    /// drawing id a save gives it ([`crate::drawing::CT_Inline::doc_pr_id_unallocated`]).
    #[doc(hidden)]
    pub fn holds_unallocated_drawing_id(&self) -> bool {
        block_contains(
            self,
            &Probe {
                needle: b"",
                deep: false,
                drawing: &|drawing| {
                    drawing
                        .inline
                        .as_ref()
                        .is_some_and(|inline| inline.doc_pr_id_unallocated)
                },
            },
        )
    }
}

fn block_contains(block: &BodyContent, probe: &Probe<'_>) -> bool {
    match block {
        BodyContent::Paragraph(paragraph) => paragraph_contains(paragraph, probe),
        BodyContent::Table(table) => table_contains(table, probe),
        BodyContent::ContentControl(sdt) => sdt_contains(sdt, probe),
        BodyContent::RawXml(raw) => probe.raw(raw),
    }
}

fn paragraph_contains(paragraph: &CT_P, probe: &Probe<'_>) -> bool {
    probe.extras(&paragraph.extra_xml)
        || (probe.deep
            && paragraph
                .hyperlinks
                .iter()
                .flat_map(|hyperlink| &hyperlink.extra_xml)
                .any(|(_, _, raw)| probe.raw(raw)))
        || paragraph.runs.iter().any(|run| run_contains(run, probe))
        || paragraph
            .content_controls
            .iter()
            .any(|(_, _, _, sdt)| sdt_contains(sdt, probe))
        || paragraph
            .revisions
            .iter()
            .any(|(_, _, revision)| revision.raw_xml_contains(probe.needle))
        || paragraph.rubies.iter().any(|ruby| {
            ruby.raw_xml.iter().any(|raw| probe.raw(raw))
                || ruby.ruby_text.iter().any(|run| run_contains(run, probe))
        })
}

fn drawing_contains(drawing: &CT_Drawing, probe: &Probe<'_>) -> bool {
    (probe.drawing)(drawing)
        || drawing
            .inline
            .as_ref()
            .and_then(|inline| inline.raw_xml.as_deref())
            .is_some_and(|raw| probe.raw(raw))
        || drawing
            .anchor
            .as_ref()
            .and_then(|anchor| anchor.raw_xml.as_deref())
            .is_some_and(|raw| probe.raw(raw))
}

fn run_contains(run: &CT_R, probe: &Probe<'_>) -> bool {
    run.extra_xml.iter().any(|raw| probe.raw(raw))
        || run
            .alt_drawings
            .iter()
            .any(|drawing| drawing_contains(drawing, probe))
        || run.content.iter().any(|content| match content {
            RunContent::Drawing(drawing) => drawing_contains(drawing, probe),
            RunContent::Field(field) if probe.deep => {
                field.any_held_piece(&|raw| probe.raw(raw), &|run| run_contains(run, probe))
            }
            _ => false,
        })
}

fn table_contains(table: &CT_Tbl, probe: &Probe<'_>) -> bool {
    probe.extras(&table.extra_xml)
        || table
            .content_controls
            .iter()
            .any(|(_, _, sdt)| sdt_contains(sdt, probe))
        || table.rows.iter().any(|row| row_contains(row, probe))
}

fn row_contains(row: &CT_Row, probe: &Probe<'_>) -> bool {
    probe.extras(&row.extra_xml)
        || row
            .content_controls
            .iter()
            .any(|(_, _, sdt)| sdt_contains(sdt, probe))
        || row.cells.iter().any(|cell| cell_contains(cell, probe))
}

fn cell_contains(cell: &CT_Tc, probe: &Probe<'_>) -> bool {
    probe.extras(&cell.extra_xml)
        || cell.content.iter().any(|content| match content {
            CellContent::Paragraph(paragraph) => paragraph_contains(paragraph, probe),
            CellContent::Table(table) => table_contains(table, probe),
            CellContent::ContentControl(sdt) => sdt_contains(sdt, probe),
        })
}

fn sdt_contains(sdt: &CT_Sdt, probe: &Probe<'_>) -> bool {
    sdt.retained_resource_run_xml()
        .any(|(_, raw)| probe.raw(raw))
        || sdt
            .revisions
            .iter()
            .any(|(_, revision)| revision.raw_xml_contains(probe.needle))
        || sdt.content.iter().any(|content| match content {
            SdtContent::Paragraph(paragraph) => paragraph_contains(paragraph, probe),
            SdtContent::Table(table) => table_contains(table, probe),
            SdtContent::Row(row) => row_contains(row, probe),
            SdtContent::Cell(cell) => cell_contains(cell, probe),
            SdtContent::Run(run) => run_contains(run, probe),
            SdtContent::ContentControl(nested) => sdt_contains(nested, probe),
            SdtContent::RawXml(raw) => probe.raw(raw),
        })
}

impl BodyContent {
    /// The shape of the owners a handle can name in this block: for each
    /// paragraph of its tree (table cells and content controls included),
    /// in document order, its runs, hyperlinks and inline controls, and the
    /// content count of each run. A block read back from its bytes with
    /// another shape would give a kept handle another owner.
    #[doc(hidden)]
    pub fn owner_shape(&self) -> Vec<usize> {
        let mut shape = Vec::new();
        block_shape(self, &mut shape);
        shape
    }
}

fn block_shape(block: &BodyContent, shape: &mut Vec<usize>) {
    match block {
        BodyContent::Paragraph(paragraph) => paragraph_shape(paragraph, shape),
        BodyContent::Table(table) => table_shape(table, shape),
        BodyContent::ContentControl(sdt) => sdt_shape(sdt, shape),
        BodyContent::RawXml(_) => shape.push(usize::MAX),
    }
}

fn paragraph_shape(paragraph: &CT_P, shape: &mut Vec<usize>) {
    shape.extend([
        paragraph.runs.len(),
        paragraph.hyperlinks.len(),
        paragraph.content_controls.len(),
    ]);
    shape.extend(paragraph.runs.iter().map(|run| run.content.len()));
    for (_, _, _, sdt) in &paragraph.content_controls {
        sdt_shape(sdt, shape);
    }
}

fn table_shape(table: &CT_Tbl, shape: &mut Vec<usize>) {
    shape.push(table.rows.len());
    for row in &table.rows {
        row_shape(row, shape);
    }
}

fn row_shape(row: &CT_Row, shape: &mut Vec<usize>) {
    shape.push(row.cells.len());
    for cell in &row.cells {
        cell_shape(cell, shape);
    }
}

fn cell_shape(cell: &CT_Tc, shape: &mut Vec<usize>) {
    shape.push(cell.content.len());
    for content in &cell.content {
        match content {
            CellContent::Paragraph(paragraph) => paragraph_shape(paragraph, shape),
            CellContent::Table(table) => table_shape(table, shape),
            CellContent::ContentControl(sdt) => sdt_shape(sdt, shape),
        }
    }
}

fn sdt_shape(sdt: &CT_Sdt, shape: &mut Vec<usize>) {
    shape.push(sdt.content.len());
    for content in &sdt.content {
        match content {
            SdtContent::Paragraph(paragraph) => paragraph_shape(paragraph, shape),
            SdtContent::Table(table) => table_shape(table, shape),
            SdtContent::Row(row) => row_shape(row, shape),
            SdtContent::Cell(cell) => cell_shape(cell, shape),
            SdtContent::Run(run) => shape.push(run.content.len()),
            SdtContent::ContentControl(nested) => sdt_shape(nested, shape),
            SdtContent::RawXml(_) => shape.push(usize::MAX),
        }
    }
}
