//! Bounded OpenDocument Text import and export for the native Word document model.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use oxoffice_docx_oxml::document::BodyContent;
use oxoffice_docx_oxml::numbering::{CT_Lvl, ST_NumberFormat};
use oxoffice_docx_oxml::properties::{CT_PPr, CT_RPr};
use oxoffice_docx_oxml::shared::{ST_HighlightColor, ST_Jc, ST_Underline};
use oxoffice_docx_oxml::table::{
    CT_Row, CT_Tbl, CT_TblGrid, CT_TblGridCol, CT_TblPr, CT_TblWidth, CT_Tc, CT_TcPr, CellContent,
    VMerge,
};
use oxoffice_docx_oxml::text::{BreakType, CT_P, CT_R, RunContent, SpecialCharacter};
use oxoffice_docx_oxml::units::Twips;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;
use zip::CompressionMethod;
use zip::ZipWriter;
use zip::read::ZipArchive;
use zip::write::SimpleFileOptions;

use crate::document::{ExportItem, body_export_items};
use crate::paragraph::{Alignment, Paragraph};
use crate::revision::{raw_is_typed_revision, revision_removes_text};
use crate::run::Run;
use crate::{Document, Error, Length, ListLevel, ListNumberFormat, PackageReadLimits, Result};

/// What a list level of an ODT list style draws in front of its paragraphs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListMarker {
    Bullet,
    Number,
    /// A numbered level with an empty number format: it counts and draws nothing.
    Unnumbered,
}

const OFFICE_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:office:1.0";
const TEXT_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:text:1.0";
const STYLE_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:style:1.0";
const FO_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0";
const TABLE_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:table:1.0";
const DRAW_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";
const SVG_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0";
const MANIFEST_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:manifest:1.0";
const ODT_MIMETYPE: &[u8] = b"application/vnd.oasis.opendocument.text";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OdtDiagnostic {
    pub path: String,
    pub message: String,
}

pub struct OdtReadResult {
    pub document: Document,
    pub diagnostics: Vec<OdtDiagnostic>,
}

/// Serialized ODT bytes together with every lossy-conversion diagnostic.
pub struct OdtWriteResult {
    pub bytes: Vec<u8>,
    pub diagnostics: Vec<OdtDiagnostic>,
}

#[derive(Clone, Copy)]
struct OdtLimits {
    archive: PackageReadLimits,
    xml_depth: usize,
    xml_nodes: usize,
    retained_text: usize,
    blocks: usize,
    runs: usize,
    rows: usize,
    columns: usize,
    cells: usize,
    diagnostics: usize,
}

impl OdtLimits {
    const DEFAULT: Self = Self {
        archive: PackageReadLimits {
            max_entries: 4_096,
            max_part_uncompressed_bytes: 64 * 1024 * 1024,
            max_total_uncompressed_bytes: 128 * 1024 * 1024,
        },
        xml_depth: 256,
        xml_nodes: 300_000,
        retained_text: 64 * 1024 * 1024,
        blocks: 100_000,
        runs: 100_000,
        rows: 10_000,
        columns: 256,
        cells: 50_000,
        diagnostics: 10_000,
    };
}

impl Document {
    /// Convert an ODT package into a fresh editable Word document.
    pub fn from_odt_bytes(bytes: &[u8]) -> Result<OdtReadResult> {
        from_odt_with_limits(bytes, OdtLimits::DEFAULT)
    }

    /// Convert an ODT package while applying caller-supplied archive bounds.
    pub fn from_odt_bytes_with_limits(
        bytes: &[u8],
        limits: PackageReadLimits,
    ) -> Result<OdtReadResult> {
        from_odt_with_limits(
            bytes,
            OdtLimits {
                archive: limits,
                ..OdtLimits::DEFAULT
            },
        )
    }

    /// Open and convert an ODT package from a path.
    pub fn open_odt<P: AsRef<Path>>(path: P) -> Result<OdtReadResult> {
        let file = std::fs::File::open(path)?;
        let declared = file.metadata()?.len();
        if declared > OdtLimits::DEFAULT.archive.max_total_uncompressed_bytes {
            return Err(odt_error(None, 0, "ODT input exceeds the size limit"));
        }
        let mut bytes = Vec::with_capacity(usize::try_from(declared).unwrap_or(0));
        file.take(
            OdtLimits::DEFAULT
                .archive
                .max_total_uncompressed_bytes
                .saturating_add(1),
        )
        .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > OdtLimits::DEFAULT.archive.max_total_uncompressed_bytes {
            return Err(odt_error(None, 0, "ODT input exceeds the size limit"));
        }
        Self::from_odt_bytes(&bytes)
    }

    /// Serialize the editable document to the supported ODT subset.
    pub fn to_odt_bytes(&self) -> Result<OdtWriteResult> {
        OdtWriter::new(self).write()
    }

    /// Serialize and save ODT to a path, returning lossy-conversion diagnostics.
    pub fn save_odt<P: AsRef<Path>>(&self, path: P) -> Result<Vec<OdtDiagnostic>> {
        let result = self.to_odt_bytes()?;
        oxoffice_opc::write_atomic_file(
            path.as_ref(),
            &result.bytes,
            "oxoffice",
            "invalid file name",
            "could not allocate ODT-save staging file",
        )?;
        Ok(result.diagnostics)
    }
}

#[derive(Clone)]
struct OdtMedia {
    path: String,
    media_type: &'static str,
    bytes: Vec<u8>,
}

struct OdtWriter<'a> {
    document: &'a Document,
    limits: OdtLimits,
    paragraph_styles: BTreeMap<String, String>,
    text_styles: BTreeMap<String, String>,
    list_styles: BTreeMap<u32, String>,
    used_list_levels: BTreeSet<(u32, u32)>,
    media: Vec<OdtMedia>,
    media_at: BTreeMap<String, usize>,
    diagnostics: Vec<OdtDiagnostic>,
    diagnostic_keys: BTreeSet<(String, String)>,
    retained_output_estimate: usize,
    retained_media_bytes: usize,
    output_blocks: usize,
    output_rows: usize,
    output_cells: usize,
    output_runs: usize,
}

impl<'a> OdtWriter<'a> {
    fn new(document: &'a Document) -> Self {
        Self::new_with_limits(document, OdtLimits::DEFAULT)
    }

    fn new_with_limits(document: &'a Document, limits: OdtLimits) -> Self {
        Self {
            document,
            limits,
            paragraph_styles: BTreeMap::new(),
            text_styles: BTreeMap::new(),
            list_styles: BTreeMap::new(),
            used_list_levels: BTreeSet::new(),
            media: Vec::new(),
            media_at: BTreeMap::new(),
            diagnostics: Vec::new(),
            diagnostic_keys: BTreeSet::new(),
            retained_output_estimate: 2_048,
            retained_media_bytes: 0,
            output_blocks: 0,
            output_rows: 0,
            output_cells: 0,
            output_runs: 0,
        }
    }

    fn write(mut self) -> Result<OdtWriteResult> {
        self.scan_document_losses()?;
        // What block content controls wrap is exported in place.
        for (item, path) in &body_export_items(&self.document.document.body.content) {
            self.scan_body(item, path)?;
        }

        let content = self.content_xml()?;
        let manifest = self.manifest_xml();
        parse_xml("content.xml", content.as_bytes(), self.limits)?;
        parse_xml("META-INF/manifest.xml", manifest.as_bytes(), self.limits)?;
        let part_limit = self.limits.archive.max_part_uncompressed_bytes as usize;
        if content.len() > part_limit || manifest.len() > part_limit {
            return Err(odt_error(None, 0, "ODT XML output exceeds the size limit"));
        }
        let media_total = self.media.iter().try_fold(0_usize, |total, media| {
            if media.bytes.len() > part_limit {
                return None;
            }
            total.checked_add(media.bytes.len())
        });
        let total = media_total
            .and_then(|total| total.checked_add(content.len()))
            .and_then(|total| total.checked_add(manifest.len()))
            .and_then(|total| total.checked_add(ODT_MIMETYPE.len()))
            .ok_or_else(|| odt_error(None, 0, "ODT output exceeds the size limit"))?;
        if total as u64 > self.limits.archive.max_total_uncompressed_bytes {
            return Err(odt_error(None, 0, "ODT output exceeds the size limit"));
        }
        if self.media.len() + 3 > self.limits.archive.max_entries {
            return Err(odt_error(None, 0, "ODT output exceeds the entry limit"));
        }

        let mut output = Cursor::new(Vec::new());
        {
            let mut archive = ZipWriter::new(&mut output);
            write_odt_entry(
                &mut archive,
                "mimetype",
                ODT_MIMETYPE,
                CompressionMethod::Stored,
            )?;
            write_odt_entry(
                &mut archive,
                "content.xml",
                content.as_bytes(),
                CompressionMethod::Deflated,
            )?;
            for media in &self.media {
                write_odt_entry(
                    &mut archive,
                    &media.path,
                    &media.bytes,
                    CompressionMethod::Deflated,
                )?;
            }
            write_odt_entry(
                &mut archive,
                "META-INF/manifest.xml",
                manifest.as_bytes(),
                CompressionMethod::Deflated,
            )?;
            archive
                .finish()
                .map_err(|error| odt_error(None, 0, format!("cannot finish ODT ZIP: {error}")))?;
        }
        let bytes = output.into_inner();
        if bytes.len() as u64 > self.limits.archive.max_total_uncompressed_bytes {
            return Err(odt_error(None, 0, "ODT output exceeds the size limit"));
        }
        Ok(OdtWriteResult {
            bytes,
            diagnostics: self.diagnostics,
        })
    }

    fn scan_body(&mut self, item: &ExportItem<'_>, path: &str) -> Result<()> {
        self.reserve_output(64)?;
        match item {
            ExportItem::Paragraph(paragraph) => self.scan_paragraph(paragraph, path),
            ExportItem::Table(table) => {
                // An owned table is one whose content controls were unwrapped.
                if matches!(**table, Cow::Owned(_)) {
                    self.diagnose(
                        &format!("{path}/content-controls"),
                        "table content controls were flattened during ODT export",
                    )?;
                }
                self.scan_table(table, path)
            }
            ExportItem::ContentControl => {
                self.diagnose(path, "body content control was flattened during ODT export")
            }
            ExportItem::RawXml => {
                self.diagnose(path, "unmodelled body XML was dropped during ODT export")
            }
        }
    }

    fn scan_table(&mut self, table: &CT_Tbl, path: &str) -> Result<()> {
        self.charge_output_block()?;
        self.reserve_output(256)?;
        validate_word_table(table, path)?;
        if table
            .properties
            .as_ref()
            .is_some_and(table_has_lossy_properties)
        {
            self.diagnose(
                &format!("{path}/tblPr"),
                "table properties were dropped during ODT export",
            )?;
        }
        if table
            .grid
            .as_ref()
            .is_some_and(|grid| !grid.columns.is_empty())
        {
            self.diagnose(
                &format!("{path}/tblGrid"),
                "table grid column widths were dropped during ODT export",
            )?;
        }
        for (index, _) in &table.extra_xml {
            self.diagnose(
                &format!("{path}/raw[{index}]"),
                "unmodelled table XML was dropped during ODT export",
            )?;
        }
        for (row_index, row) in table.rows.iter().enumerate() {
            self.output_rows = self
                .output_rows
                .checked_add(1)
                .filter(|rows| *rows <= self.limits.rows)
                .ok_or_else(|| odt_error(None, 0, "ODT output exceeds the row limit"))?;
            self.reserve_output(128)?;
            let row_path = format!("{path}/row[{row_index}]");
            if row.properties.is_some() {
                self.diagnose(
                    &format!("{row_path}/trPr"),
                    "table-row properties were dropped during ODT export",
                )?;
            }
            for (index, raw) in &row.extra_xml {
                if CT_Row::raw_is_root_attributes(*index, raw) {
                    continue;
                }
                self.diagnose(
                    &format!("{row_path}/raw[{index}]"),
                    "unmodelled table-row XML was dropped during ODT export",
                )?;
            }
            for (cell_index, cell) in row.cells.iter().enumerate() {
                let span = cell
                    .properties
                    .as_ref()
                    .and_then(|properties| properties.grid_span)
                    .unwrap_or(1);
                let span = usize::try_from(span)
                    .map_err(|_| odt_error(None, 0, "ODT output cell count overflowed"))?;
                self.output_cells = self
                    .output_cells
                    .checked_add(span)
                    .filter(|cells| *cells <= self.limits.cells)
                    .ok_or_else(|| odt_error(None, 0, "ODT output exceeds the cell limit"))?;
                self.reserve_output(256)?;
                let cell_path = format!("{row_path}/cell[{cell_index}]");
                if cell
                    .properties
                    .as_ref()
                    .is_some_and(cell_has_lossy_properties)
                {
                    self.diagnose(
                        &format!("{cell_path}/tcPr"),
                        "unsupported table-cell properties were dropped during ODT export",
                    )?;
                }
                for (index, _) in &cell.extra_xml {
                    self.diagnose(
                        &format!("{cell_path}/raw[{index}]"),
                        "unmodelled table-cell XML was dropped during ODT export",
                    )?;
                }
                if matches!(
                    cell.properties
                        .as_ref()
                        .and_then(|properties| properties.v_merge.as_ref()),
                    Some(VMerge::Continue)
                ) {
                    if cell_has_substantive_content(cell) {
                        self.diagnose(
                            &format!("{cell_path}/content"),
                            "vertical-merge continuation content was dropped during ODT export",
                        )?;
                    }
                    continue;
                }
                if !cell
                    .content
                    .iter()
                    .any(|content| matches!(content, CellContent::Paragraph(_)))
                {
                    self.charge_output_block()?;
                }
                for (content_index, content) in cell.content.iter().enumerate() {
                    let content_path = format!("{cell_path}/content[{content_index}]");
                    match content {
                        CellContent::Paragraph(paragraph) => {
                            self.scan_paragraph(paragraph, &content_path)?
                        }
                        CellContent::Table(_) => self.diagnose(
                            &content_path,
                            "nested table was dropped during ODT export",
                        )?,
                        // Unwrapped by `unwrap_table_controls`.
                        CellContent::ContentControl(_) => {}
                    }
                }
            }
        }
        Ok(())
    }

    fn scan_paragraph(&mut self, paragraph: &CT_P, path: &str) -> Result<()> {
        self.charge_output_block()?;
        self.reserve_output(256)?;
        let paragraph_properties = self.effective_paragraph_properties(paragraph);
        validate_paragraph_projection(&paragraph_properties, path)?;
        let paragraph_xml = paragraph_style_xml(&paragraph_properties);
        self.ensure_paragraph_style(paragraph_xml)?;
        self.scan_paragraph_losses(paragraph, &paragraph_properties, path)?;
        if let Some((num_id, level)) = paragraph_numbering_properties(&paragraph_properties) {
            if level > 8 {
                return Err(odt_error(
                    Some("content.xml"),
                    0,
                    format!("ODT list level exceeds 8 at {path}"),
                ));
            }
            if self.list_level_has_producer_format(num_id, level) {
                self.diagnose(
                    &format!("numbering[{num_id}]/level[{level}]"),
                    "producer-defined numbering format was flattened without a marker during ODT export",
                )?;
                self.scan_flattened_producer_list_losses(num_id, level)?;
            } else {
                self.scan_shared_definition_continuation_loss(num_id, level)?;
                self.used_list_levels.insert((num_id, level));
                self.ensure_list_style(num_id, path)?;
            }
        }
        let mut trailing_text_style = None;
        let mut projected_runs = 0_usize;
        // The runs of the accepted view are exported, see `write_runs`.
        for (run_index, run) in paragraph.accepted_view().runs.iter().enumerate() {
            self.reserve_output(192)?;
            let run_path = format!("{path}/run[{run_index}]");
            let run_properties = self.effective_run_properties(paragraph, run);
            validate_run_projection(&run_properties, &run_path)?;
            if let Some(font) = selected_run_font(&run_properties) {
                validate_xml_value(font, &format!("{run_path}/rPr/font"))?;
                validate_font_family_projection(font, &format!("{run_path}/rPr/font"))?;
            }
            let run_style = text_style_xml(&run_properties);
            self.ensure_text_style(run_style.clone())?;
            self.scan_run_losses(run, &run_properties, &run_path)?;
            for (content_index, content) in run.content.iter().enumerate() {
                if let RunContent::Drawing(drawing) = content {
                    self.scan_drawing(drawing, &format!("{run_path}/content[{content_index}]"))?;
                }
            }
            for (content_index, content) in run.content.iter().enumerate() {
                let pieces = match content {
                    RunContent::Text(text) | RunContent::DeletedText(text) => {
                        projected_text_run_pieces(&text.text, &run_style, &mut trailing_text_style)
                    }
                    RunContent::Field(field) => projected_text_run_pieces(
                        field
                            .projected_text()
                            .unwrap_or(field.cached_result.as_str()),
                        &run_style,
                        &mut trailing_text_style,
                    ),
                    RunContent::Tab
                    | RunContent::Break(BreakType::Line)
                    | RunContent::SpecialCharacter(_) => {
                        trailing_text_style = None;
                        1
                    }
                    // A symbol is font-encoded rather than Unicode, so it
                    // reaches the ODF body as nothing at all.
                    RunContent::Symbol { .. } => 0,
                    RunContent::Drawing(_) => {
                        if self
                            .media_at
                            .contains_key(&format!("{run_path}/content[{content_index}]"))
                        {
                            trailing_text_style = None;
                            1
                        } else {
                            0
                        }
                    }
                    RunContent::Break(_)
                    | RunContent::FootnoteRef { .. }
                    | RunContent::EndnoteRef { .. }
                    | RunContent::CommentReference { .. } => 0,
                };
                projected_runs = projected_runs
                    .checked_add(pieces)
                    .ok_or_else(|| odt_error(None, 0, "ODT output exceeds the run limit"))?;
            }
        }
        self.output_runs = self
            .output_runs
            .checked_add(projected_runs)
            .filter(|runs| *runs <= self.limits.runs)
            .ok_or_else(|| odt_error(None, 0, "ODT output exceeds the run limit"))?;
        Ok(())
    }

    fn scan_document_losses(&mut self) -> Result<()> {
        if self.document.document.background_xml.is_some() {
            self.diagnose(
                "document/background",
                "document background was dropped during ODT export",
            )?;
        }
        if self
            .document
            .document
            .body
            .sect_pr
            .as_ref()
            .is_some_and(final_section_has_unsupported_properties)
        {
            self.diagnose(
                "document/sectPr",
                "final section properties were dropped during ODT export",
            )?;
        }
        let relationships = self
            .document
            .package
            .get_part_rels(&self.document.doc_part_name);
        if let Some(section) = self.document.document.body.sect_pr.as_ref() {
            for (index, reference) in section.header_refs.iter().enumerate() {
                let relationship = relationships.and_then(|relationships| {
                    relationships
                        .items
                        .iter()
                        .find(|relationship| relationship.id == reference.rel_id)
                });
                if relationship.is_none_or(|relationship| {
                    relationship.rel_type != oxoffice_opc::relationship::rel_types::HEADER
                }) {
                    self.diagnose(
                        &format!("document/sectPr/headerReference[{index}]"),
                        "unresolved or wrong-type header reference was dropped during ODT export",
                    )?;
                }
            }
            for (index, reference) in section.footer_refs.iter().enumerate() {
                let relationship = relationships.and_then(|relationships| {
                    relationships
                        .items
                        .iter()
                        .find(|relationship| relationship.id == reference.rel_id)
                });
                if relationship.is_none_or(|relationship| {
                    relationship.rel_type != oxoffice_opc::relationship::rel_types::FOOTER
                }) {
                    self.diagnose(
                        &format!("document/sectPr/footerReference[{index}]"),
                        "unresolved or wrong-type footer reference was dropped during ODT export",
                    )?;
                }
            }
        }
        if relationships.is_some_and(|relationships| {
            relationships.items.iter().any(|relationship| {
                relationship.rel_type == oxoffice_opc::relationship::rel_types::HEADER
            })
        }) {
            self.diagnose(
                "document/headers",
                "header stories were dropped during ODT export",
            )?;
        }
        if relationships.is_some_and(|relationships| {
            relationships.items.iter().any(|relationship| {
                relationship.rel_type == oxoffice_opc::relationship::rel_types::FOOTER
            })
        }) {
            self.diagnose(
                "document/footers",
                "footer stories were dropped during ODT export",
            )?;
        }
        Ok(())
    }

    fn charge_output_block(&mut self) -> Result<()> {
        self.output_blocks = self
            .output_blocks
            .checked_add(1)
            .filter(|blocks| *blocks <= self.limits.blocks)
            .ok_or_else(|| odt_error(None, 0, "ODT output exceeds the block limit"))?;
        Ok(())
    }

    fn scan_paragraph_losses(
        &mut self,
        paragraph: &CT_P,
        effective: &CT_PPr,
        path: &str,
    ) -> Result<()> {
        if paragraph_properties_have_unsupported(effective)
            || paragraph
                .properties
                .as_ref()
                .is_some_and(paragraph_properties_have_unsupported)
        {
            self.diagnose(
                &format!("{path}/pPr"),
                "unsupported paragraph properties were dropped during ODT export",
            )?;
        }
        if [Some(effective), paragraph.properties.as_ref()]
            .into_iter()
            .flatten()
            .any(|properties| properties.style_id.is_some())
        {
            self.diagnose(
                &format!("{path}/pPr/pStyle"),
                "paragraph style identity was materialized and dropped during ODT export",
            )?;
        }
        if [Some(effective), paragraph.properties.as_ref()]
            .into_iter()
            .flatten()
            .any(|properties| {
                properties
                    .line_rule
                    .as_deref()
                    .is_some_and(|rule| rule != "auto" && rule != "exact")
            })
        {
            self.diagnose(
                &format!("{path}/pPr/spacing"),
                "unsupported line-spacing rule was simplified during ODT export",
            )?;
        }
        if [Some(effective), paragraph.properties.as_ref()]
            .into_iter()
            .flatten()
            .any(|properties| properties.line_rule.is_some() && properties.line_spacing.is_none())
        {
            self.diagnose(
                &format!("{path}/pPr/spacing"),
                "line-spacing rule without line spacing was dropped during ODT export",
            )?;
        }
        if [Some(effective), paragraph.properties.as_ref()]
            .into_iter()
            .flatten()
            .any(|properties| properties.num_ilvl.is_some() && properties.num_id.is_none())
        {
            self.diagnose(
                &format!("{path}/pPr/numPr/ilvl"),
                "numbering level without a numbering id was dropped during ODT export",
            )?;
        }
        if [Some(effective), paragraph.properties.as_ref()]
            .into_iter()
            .flatten()
            .any(|properties| {
                properties.ind_first_line.is_some() && properties.ind_hanging.is_some()
            })
        {
            self.diagnose(
                &format!("{path}/pPr/ind/firstLine"),
                "first-line indent was dropped because hanging indent takes precedence during ODT export",
            )?;
        }
        if [Some(effective), paragraph.properties.as_ref()]
            .into_iter()
            .flatten()
            .any(|properties| properties.jc == Some(ST_Jc::Distribute))
        {
            self.diagnose(
                &format!("{path}/pPr/jc"),
                "distributed paragraph alignment was simplified to justify during ODT export",
            )?;
        }
        for (raw_index, (index, raw)) in paragraph.extra_xml.iter().enumerate() {
            if CT_P::raw_is_root_attributes(*index, raw)
                || raw_is_typed_revision(paragraph, raw_index)
            {
                continue;
            }
            self.diagnose(
                &format!("{path}/raw[{index}]"),
                if CT_P::raw_run_wrapper_content(raw).is_some() {
                    "smart tag or custom XML wrapper was flattened during ODT export"
                } else {
                    "unmodelled paragraph XML was dropped during ODT export"
                },
            )?;
        }
        if !paragraph.content_controls.is_empty() {
            self.diagnose(
                &format!("{path}/content-controls"),
                "run content controls were flattened during ODT export",
            )?;
        }
        if paragraph
            .revisions
            .iter()
            .any(|(_, _, revision)| !revision_removes_text(revision))
        {
            self.diagnose(
                &format!("{path}/revisions"),
                "paragraph revisions were flattened during ODT export",
            )?;
        }
        if paragraph
            .revisions
            .iter()
            .any(|(_, _, revision)| revision_removes_text(revision))
        {
            self.diagnose(
                &format!("{path}/revisions"),
                "deleted or moved-away revision content was dropped during ODT export",
            )?;
        }
        if !paragraph.comment_ranges.is_empty() {
            self.diagnose(
                &format!("{path}/comments"),
                "comment markers were dropped during ODT export",
            )?;
        }
        if !paragraph.bookmark_markers.is_empty() {
            self.diagnose(
                &format!("{path}/bookmarks"),
                "bookmark markers were dropped during ODT export",
            )?;
        }
        if !paragraph.hyperlinks.is_empty() {
            self.diagnose(
                &format!("{path}/hyperlinks"),
                "hyperlink wrappers were flattened during ODT export",
            )?;
        }
        Ok(())
    }

    fn scan_run_losses(&mut self, run: &CT_R, effective: &CT_RPr, path: &str) -> Result<()> {
        if run_properties_have_unsupported(effective)
            || run
                .properties
                .as_ref()
                .is_some_and(run_properties_have_unsupported)
        {
            self.diagnose(
                &format!("{path}/rPr"),
                "unsupported run properties were dropped during ODT export",
            )?;
        }
        if [Some(effective), run.properties.as_ref()]
            .into_iter()
            .flatten()
            .any(|properties| properties.style_id.is_some())
        {
            self.diagnose(
                &format!("{path}/rPr/rStyle"),
                "run style identity was materialized and dropped during ODT export",
            )?;
        }
        if run.extra_xml.len() != run.extra_xml_positions.len()
            || run
                .extra_xml_positions
                .iter()
                .any(|position| !CT_R::raw_child_is_root_attributes(*position))
            || !run.alt_drawings.is_empty()
        {
            self.diagnose(
                &format!("{path}/raw"),
                "unmodelled run XML was dropped during ODT export",
            )?;
        }
        for properties in [Some(effective), run.properties.as_ref()]
            .into_iter()
            .flatten()
        {
            if properties
                .vert_align
                .as_deref()
                .is_some_and(|value| !matches!(value, "superscript" | "subscript"))
            {
                self.diagnose(
                    &format!("{path}/rPr/vertAlign"),
                    "unsupported run vertical alignment was dropped during ODT export",
                )?;
            }
            if properties
                .color
                .as_deref()
                .is_some_and(|color| normalized_color(color).is_none())
            {
                self.diagnose(
                    &format!("{path}/rPr/color"),
                    "unsupported run color was dropped during ODT export",
                )?;
            }
            if let Some(shading) = properties.shading.as_ref() {
                if !matches!(shading.val.as_str(), "clear" | "nil") {
                    self.diagnose(
                        &format!("{path}/rPr/shading-pattern"),
                        "run shading pattern was simplified during ODT export",
                    )?;
                }
                if shading
                    .color
                    .as_deref()
                    .is_some_and(|color| color != "auto")
                {
                    self.diagnose(
                        &format!("{path}/rPr/shading-color"),
                        "run shading foreground color was dropped during ODT export",
                    )?;
                }
                let valid_fill = shading
                    .fill
                    .as_deref()
                    .is_some_and(|fill| normalized_color(fill).is_some());
                if shading
                    .fill
                    .as_deref()
                    .is_some_and(|fill| normalized_color(fill).is_none())
                {
                    self.diagnose(
                        &format!("{path}/rPr/shading-fill"),
                        "unsupported run shading fill was dropped during ODT export",
                    )?;
                }
                if valid_fill
                    && properties
                        .highlight
                        .as_ref()
                        .is_some_and(|highlight| highlight != &ST_HighlightColor::None)
                {
                    self.diagnose(
                        &format!("{path}/rPr/highlight"),
                        "run highlight was replaced by shading fill during ODT export",
                    )?;
                }
            }
        }
        for (index, content) in run.content.iter().enumerate() {
            let content_path = format!("{path}/content[{index}]");
            match content {
                RunContent::Text(text) | RunContent::DeletedText(text) => {
                    if !text.text.chars().all(valid_xml_character) {
                        return Err(odt_error(
                            Some("content.xml"),
                            0,
                            format!("invalid XML character at {content_path}"),
                        ));
                    }
                    if text.text.chars().any(|character| {
                        character.is_whitespace() && !matches!(character, ' ' | '\t' | '\r' | '\n')
                    }) {
                        return Err(odt_error(
                            Some("content.xml"),
                            0,
                            format!("ODT cannot preserve Unicode whitespace at {content_path}"),
                        ));
                    }
                    self.reserve_output(text.text.len().saturating_mul(18))?;
                    if matches!(content, RunContent::DeletedText(_)) {
                        self.diagnose(
                            &content_path,
                            "deleted text was flattened during ODT export",
                        )?;
                    }
                }
                RunContent::Break(BreakType::Page | BreakType::Column) => self.diagnose(
                    &content_path,
                    "unsupported break type was dropped during ODT export",
                )?,
                RunContent::Field(field) => {
                    let display = field
                        .projected_text()
                        .unwrap_or(field.cached_result.as_str());
                    validate_xml_value(display, &content_path)?;
                    validate_unicode_whitespace(display, &content_path)?;
                    self.reserve_output(display.len().saturating_mul(18))?;
                    self.diagnose(&content_path, "field was flattened during ODT export")?
                }
                RunContent::FootnoteRef { .. } => self.diagnose(
                    &content_path,
                    "footnote reference was dropped during ODT export",
                )?,
                RunContent::EndnoteRef { .. } => self.diagnose(
                    &content_path,
                    "endnote reference was dropped during ODT export",
                )?,
                RunContent::CommentReference { .. } => self.diagnose(
                    &content_path,
                    "comment reference was dropped during ODT export",
                )?,
                RunContent::Symbol { .. } => self.diagnose(
                    &content_path,
                    "symbol character was dropped during ODT export",
                )?,
                RunContent::Tab
                | RunContent::Break(BreakType::Line)
                | RunContent::Drawing(_)
                | RunContent::SpecialCharacter(_) => {}
            }
        }
        Ok(())
    }

    fn scan_drawing(
        &mut self,
        drawing: &oxoffice_docx_oxml::drawing::CT_Drawing,
        path: &str,
    ) -> Result<()> {
        if drawing.anchor.is_some() {
            self.diagnose(path, "anchored drawing was dropped during ODT export")?;
        }
        let Some(inline) = &drawing.inline else {
            if drawing.anchor.is_none() {
                self.diagnose(path, "empty drawing was dropped during ODT export")?;
            }
            return Ok(());
        };
        if inline.description.is_some() {
            self.diagnose(
                path,
                "inline image description was dropped during ODT export",
            )?;
        }
        if inline.name.is_some() {
            self.diagnose(path, "inline image name was replaced during ODT export")?;
        }
        if inline.raw_xml.is_some() {
            self.diagnose(
                path,
                "retained inline drawing XML was dropped during ODT export",
            )?;
        }
        if inline.chart_rel_id.is_some() || inline.embed_id.is_empty() {
            self.diagnose(
                path,
                "non-image inline drawing was dropped during ODT export",
            )?;
            return Ok(());
        }
        if inline.extent_cx.0 <= 0 || inline.extent_cy.0 <= 0 {
            self.diagnose(
                path,
                "non-positive inline image was dropped during ODT export",
            )?;
            return Ok(());
        }
        const MAX_IMAGE_EMU: i64 = 12_700_000_000;
        if inline.extent_cx.0 > MAX_IMAGE_EMU || inline.extent_cy.0 > MAX_IMAGE_EMU {
            return Err(odt_error(
                Some("content.xml"),
                0,
                format!("ODT cannot preserve inline image dimensions at {path}"),
            ));
        }
        let Some(relationship) = self.image_relationship(&inline.embed_id) else {
            self.diagnose(
                path,
                "unresolved inline image was dropped during ODT export",
            )?;
            return Ok(());
        };
        if relationship.rel_type != oxoffice_opc::relationship::rel_types::IMAGE {
            self.diagnose(
                path,
                "inline drawing with a non-image relationship was dropped during ODT export",
            )?;
            return Ok(());
        }
        match relationship.target_mode.as_deref() {
            None | Some("Internal") => {}
            Some("External") => {
                self.diagnose(path, "external inline image was dropped during ODT export")?;
                return Ok(());
            }
            Some(_) => {
                self.diagnose(
                    path,
                    "invalid inline image target mode was dropped during ODT export",
                )?;
                return Ok(());
            }
        }
        let Some(bytes) = self.relationship_bytes(relationship) else {
            self.diagnose(
                path,
                "unresolved inline image was dropped during ODT export",
            )?;
            return Ok(());
        };
        let Some(info) = oxoffice_media::probe(bytes) else {
            self.diagnose(
                path,
                "unsupported inline image was dropped during ODT export",
            )?;
            return Ok(());
        };
        let format = info.format;
        if !matches!(
            format,
            oxoffice_media::ImageFormat::Png
                | oxoffice_media::ImageFormat::Jpeg
                | oxoffice_media::ImageFormat::Gif
                | oxoffice_media::ImageFormat::Bmp
                | oxoffice_media::ImageFormat::Webp
        ) {
            self.diagnose(
                path,
                "unsupported inline image was dropped during ODT export",
            )?;
            return Ok(());
        }
        let media_bytes = bytes.len();
        let part_limit = self.limits.archive.max_part_uncompressed_bytes as usize;
        if media_bytes > part_limit {
            return Err(odt_error(
                None,
                0,
                "ODT media output exceeds the part size limit",
            ));
        }
        if self.media.len().saturating_add(4) > self.limits.archive.max_entries {
            return Err(odt_error(None, 0, "ODT output exceeds the entry limit"));
        }
        self.reserve_output(512)?;
        let retained_media_bytes = self
            .retained_media_bytes
            .checked_add(media_bytes)
            .filter(|media| {
                media
                    .checked_add(self.retained_output_estimate)
                    .is_some_and(|total| {
                        total as u64 <= self.limits.archive.max_total_uncompressed_bytes
                    })
            })
            .ok_or_else(|| odt_error(None, 0, "ODT output exceeds the size limit"))?;
        let index = self.media.len();
        let bytes = self
            .image_relationship(&inline.embed_id)
            .and_then(|relationship| self.relationship_bytes(relationship))
            .expect("image relationship remained immutable and internal")
            .to_vec();
        self.media.push(OdtMedia {
            path: format!("Pictures/image{}.{}", index + 1, format.extension()),
            media_type: format.content_type(),
            bytes,
        });
        self.retained_media_bytes = retained_media_bytes;
        self.media_at.insert(path.to_owned(), index);
        Ok(())
    }

    fn image_relationship(
        &self,
        relationship_id: &str,
    ) -> Option<&oxoffice_opc::relationship::Relationship> {
        let relationships = self
            .document
            .package
            .get_part_rels(&self.document.doc_part_name)?;
        relationships
            .items
            .iter()
            .find(|relationship| relationship.id == relationship_id)
    }

    fn relationship_bytes(
        &self,
        relationship: &oxoffice_opc::relationship::Relationship,
    ) -> Option<&[u8]> {
        let target = oxoffice_opc::OpcPackage::resolve_rel_target(
            &self.document.doc_part_name,
            &relationship.target,
        );
        self.document.package.get_part(&target)
    }

    fn effective_paragraph_properties(&self, paragraph: &CT_P) -> CT_PPr {
        let direct = paragraph.properties.as_ref();
        let mut effective = self.document.resolve_paragraph_properties(
            direct.and_then(|properties| properties.style_id.as_deref()),
        );
        if let Some(direct) = direct {
            effective.merge_from(direct);
        }
        effective
    }

    fn scan_shared_definition_continuation_loss(&mut self, num_id: u32, level: u32) -> Result<()> {
        let Some(numbering) = &self.document.numbering else {
            return Ok(());
        };
        let Some(instance) = numbering.nums.iter().find(|value| value.num_id == num_id) else {
            return Ok(());
        };
        let Some(definition) = numbering.get_abstract_num_for(num_id) else {
            return Ok(());
        };
        let Some(definition_level) = definition.levels.iter().find(|value| value.ilvl == level)
        else {
            return Ok(());
        };
        let level_override = instance
            .level_overrides
            .iter()
            .find(|value| value.ilvl == level);
        let effective_level = level_override
            .and_then(|value| value.level.as_ref())
            .unwrap_or(definition_level);
        if matches!(
            effective_level.num_fmt.as_ref(),
            Some(ST_NumberFormat::Bullet | ST_NumberFormat::None | ST_NumberFormat::Other(_))
        ) || level_override.is_some_and(|value| value.start_override.is_some())
        {
            return Ok(());
        }
        let continued = self
            .used_list_levels
            .iter()
            .any(|(previous, previous_level)| {
                *previous != num_id
                    && *previous_level == level
                    && numbering
                        .get_abstract_num_for(*previous)
                        .is_some_and(|previous| {
                            previous.abstract_num_id == definition.abstract_num_id
                        })
            });
        if continued {
            self.diagnose(&format!("numbering[{num_id}]/level[{level}]/shared-definition-continuation"),
                "shared-definition numbering continuation cannot be represented by the concrete ODT list writer")?;
        }
        Ok(())
    }

    fn paragraph_numbering(&self, paragraph: &CT_P) -> Option<(u32, u32)> {
        let numbering =
            paragraph_numbering_properties(&self.effective_paragraph_properties(paragraph))?;
        (!self.list_level_has_producer_format(numbering.0, numbering.1)).then_some(numbering)
    }

    fn list_level_has_producer_format(&self, num_id: u32, level: u32) -> bool {
        self.effective_list_level(num_id, level)
            .and_then(|definition| definition.num_fmt.as_ref())
            .is_some_and(|format| matches!(format, ST_NumberFormat::Other(_)))
    }

    /// The number a list level starts at, as layout counts it: the
    /// instance's start override, else the definition's start, which a
    /// swapped-in level does not change.
    fn list_start(&self, num_id: u32, level: u32) -> u32 {
        let Some(numbering) = self.document.numbering.as_ref() else {
            return 1;
        };
        numbering
            .nums
            .iter()
            .find(|item| item.num_id == num_id)
            .and_then(|instance| {
                instance
                    .level_overrides
                    .iter()
                    .find(|item| item.ilvl == level)
            })
            .and_then(|item| item.start_override)
            .or_else(|| {
                numbering
                    .get_abstract_num_for(num_id)?
                    .levels
                    .iter()
                    .find(|item| item.ilvl == level)?
                    .start
            })
            .unwrap_or(1)
    }

    /// The definition's level, or the level the instance swaps in for it, as
    /// layout draws it.
    fn effective_list_level(&self, num_id: u32, level: u32) -> Option<&CT_Lvl> {
        let numbering = self.document.numbering.as_ref()?;
        let definition = numbering
            .get_abstract_num_for(num_id)?
            .levels
            .iter()
            .find(|item| item.ilvl == level)?;
        Some(
            numbering
                .nums
                .iter()
                .find(|item| item.num_id == num_id)
                .and_then(|instance| {
                    instance
                        .level_overrides
                        .iter()
                        .find(|item| item.ilvl == level)
                })
                .and_then(|item| item.level.as_ref())
                .unwrap_or(definition),
        )
    }

    fn effective_run_properties(&self, paragraph: &CT_P, run: &CT_R) -> CT_RPr {
        let paragraph_style = paragraph
            .properties
            .as_ref()
            .and_then(|properties| properties.style_id.as_deref());
        let direct = run.properties.as_ref();
        let mut effective = self.document.resolve_run_properties(
            paragraph_style,
            direct.and_then(|properties| properties.style_id.as_deref()),
        );
        if let Some(direct) = direct {
            effective.merge_from(direct);
        }
        effective
    }

    fn ensure_paragraph_style(&mut self, xml: String) -> Result<Option<String>> {
        if xml.is_empty() {
            return Ok(None);
        }
        if let Some(name) = self.paragraph_styles.get(&xml) {
            return Ok(Some(name.clone()));
        }
        self.reserve_output(xml.len().saturating_add(160))?;
        let name = format!("P{}", self.paragraph_styles.len() + 1);
        self.paragraph_styles.insert(xml, name.clone());
        Ok(Some(name))
    }

    fn ensure_text_style(&mut self, xml: String) -> Result<Option<String>> {
        if xml.is_empty() {
            return Ok(None);
        }
        if let Some(name) = self.text_styles.get(&xml) {
            return Ok(Some(name.clone()));
        }
        self.reserve_output(xml.len().saturating_add(160))?;
        let name = format!("T{}", self.text_styles.len() + 1);
        self.text_styles.insert(xml, name.clone());
        Ok(Some(name))
    }

    fn ensure_list_style(&mut self, num_id: u32, path: &str) -> Result<String> {
        if let Some(name) = self.list_styles.get(&num_id) {
            return Ok(name.clone());
        }
        if self
            .document
            .numbering
            .as_ref()
            .and_then(|numbering| numbering.get_abstract_num_for(num_id))
            .is_none()
        {
            self.diagnose(
                path,
                "unknown list definition was exported as decimal ODT list",
            )?;
        } else {
            self.scan_numbering_container_losses(num_id)?;
        }
        self.reserve_output(1_024)?;
        let name = format!("L{}", self.list_styles.len() + 1);
        self.list_styles.insert(num_id, name.clone());
        Ok(name)
    }

    fn scan_numbering_container_losses(&mut self, num_id: u32) -> Result<()> {
        let Some(numbering) = self.document.numbering.as_ref() else {
            return Ok(());
        };
        let instance = numbering
            .nums
            .iter()
            .find(|instance| instance.num_id == num_id);
        let abstract_num = numbering.get_abstract_num_for(num_id);
        let losses = (
            !numbering.root_attributes.is_empty(),
            !numbering.extra_xml.is_empty(),
            instance.is_some_and(|value| !value.extra_attributes.is_empty()),
            instance.is_some_and(|value| !value.extra_xml.is_empty()),
            abstract_num.is_some_and(|value| value.multi_level_type.is_some()),
            abstract_num.is_some_and(|value| !value.extra_attributes.is_empty()),
            abstract_num.is_some_and(|value| !value.extra_xml.is_empty()),
        );
        let path = format!("numbering[{num_id}]");
        for (present, suffix, message) in [
            (
                losses.0,
                "root-attributes",
                "retained numbering root attributes were dropped during ODT export",
            ),
            (
                losses.1,
                "root-xml",
                "retained numbering root XML was dropped during ODT export",
            ),
            (
                losses.2,
                "instance-attributes",
                "retained numbering instance attributes were dropped during ODT export",
            ),
            (
                losses.3,
                "instance-overrides",
                "numbering instance overrides or retained XML were dropped during ODT export",
            ),
            (
                losses.4,
                "abstract-type",
                "abstract numbering type metadata was dropped during ODT export",
            ),
            (
                losses.5,
                "abstract-attributes",
                "retained abstract numbering attributes were dropped during ODT export",
            ),
            (
                losses.6,
                "abstract-xml",
                "retained abstract numbering XML was dropped during ODT export",
            ),
        ] {
            if present {
                self.diagnose(&format!("{path}/{suffix}"), message)?;
            }
        }
        Ok(())
    }

    fn scan_flattened_producer_list_losses(&mut self, num_id: u32, level: u32) -> Result<()> {
        self.scan_numbering_container_losses(num_id)?;
        let definition = self.effective_list_level(num_id, level);
        let losses = definition.map_or(
            (false, false, false, false, false, false, false),
            |definition| {
                (
                    self.list_start(num_id, level) != 1,
                    definition.suffix.is_some(),
                    definition.lvl_text.is_some(),
                    definition.lvl_jc.is_some(),
                    definition.ppr.is_some() || definition.ppr_raw.is_some(),
                    definition.rpr.is_some() || definition.rpr_raw.is_some(),
                    !definition.extra_xml.is_empty() || !definition.extra_attributes.is_empty(),
                )
            },
        );
        let path = format!("numbering[{num_id}]/level[{level}]");
        for (present, message) in [
            (
                losses.0,
                "custom list start value was dropped during ODT export",
            ),
            (losses.1, "list marker suffix was dropped during ODT export"),
            (
                losses.2,
                "list marker text or bullet glyph was dropped during ODT export",
            ),
            (
                losses.3,
                "list marker justification was dropped during ODT export",
            ),
            (
                losses.4,
                "list level paragraph formatting was dropped during ODT export",
            ),
            (
                losses.5,
                "list marker run formatting was dropped during ODT export",
            ),
            (
                losses.6,
                "retained list level XML or attributes were dropped during ODT export",
            ),
        ] {
            if present {
                self.diagnose(&path, message)?;
            }
        }
        Ok(())
    }

    fn content_xml(&mut self) -> Result<String> {
        let mut output = String::new();
        output.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
        write!(
            output,
            "<office:document-content xmlns:office=\"{OFFICE_NS}\" xmlns:text=\"{TEXT_NS}\" xmlns:style=\"{STYLE_NS}\" xmlns:fo=\"{FO_NS}\" xmlns:table=\"{TABLE_NS}\" xmlns:draw=\"{DRAW_NS}\" xmlns:xlink=\"{XLINK_NS}\" xmlns:svg=\"{SVG_NS}\" office:version=\"1.3\"><office:automatic-styles>"
        )
        .unwrap();
        for (xml, name) in &self.paragraph_styles {
            write!(
                output,
                "<style:style style:name=\"{name}\" style:family=\"paragraph\"><style:paragraph-properties{xml}/></style:style>"
            )
            .unwrap();
        }
        for (xml, name) in &self.text_styles {
            write!(
                output,
                "<style:style style:name=\"{name}\" style:family=\"text\"><style:text-properties{xml}/></style:style>"
            )
            .unwrap();
        }
        let list_styles = self.list_styles.clone();
        for (num_id, name) in list_styles {
            write!(output, "<text:list-style style:name=\"{name}\">").unwrap();
            for level in 0..9_u32 {
                // A `none` level is a numbered level that draws nothing: the
                // count of the levels around it goes on.
                let element = match self.list_level_marker(
                    num_id,
                    level,
                    &format!("numbering[{num_id}]/level[{level}]"),
                )? {
                    ListMarker::Bullet => "list-level-style-bullet text:bullet-char=\"•\"",
                    ListMarker::Number => "list-level-style-number style:num-format=\"1\"",
                    ListMarker::Unnumbered => "list-level-style-number style:num-format=\"\"",
                };
                let (name, attributes) = element.split_once(' ').unwrap_or((element, ""));
                write!(
                    output,
                    "<text:{name} text:level=\"{}\" {attributes}/>",
                    level + 1
                )
                .unwrap();
            }
            output.push_str("</text:list-style>");
        }
        output.push_str("</office:automatic-styles><office:body><office:text>");
        // A list continues through the content controls it runs into.
        let items = body_export_items(&self.document.document.body.content)
            .into_iter()
            .filter(|(item, _)| !matches!(item, ExportItem::ContentControl))
            .collect::<Vec<_>>();
        let mut index = 0_usize;
        let mut written_lists = BTreeSet::new();
        while index < items.len() {
            let (item, path) = &items[index];
            if let ExportItem::Paragraph(paragraph) = item
                && let Some((num_id, _)) = self.paragraph_numbering(paragraph)
            {
                let start = index;
                while index < items.len()
                    && matches!(
                        &items[index].0,
                        ExportItem::Paragraph(candidate)
                            if self.paragraph_numbering(candidate).is_some_and(|(candidate_id, _)| candidate_id == num_id)
                    )
                {
                    index += 1;
                }
                let continue_numbering = !written_lists.insert(num_id);
                self.write_list(
                    &mut output,
                    &items[start..index],
                    num_id,
                    continue_numbering,
                )?;
                continue;
            }
            self.write_body(&mut output, item, path)?;
            index += 1;
        }
        output.push_str("</office:text></office:body></office:document-content>");
        Ok(output)
    }

    fn write_body(&self, output: &mut String, item: &ExportItem<'_>, path: &str) -> Result<()> {
        match item {
            ExportItem::Paragraph(paragraph) => self.write_paragraph(output, paragraph, path),
            ExportItem::Table(table) => self.write_table(output, table, path),
            ExportItem::ContentControl | ExportItem::RawXml => Ok(()),
        }
    }

    fn write_list(
        &self,
        output: &mut String,
        list: &[(ExportItem<'_>, String)],
        num_id: u32,
        continue_numbering: bool,
    ) -> Result<()> {
        let style = self.list_styles.get(&num_id).expect("scanned list style");
        let mut items = Vec::with_capacity(list.len());
        for (item, path) in list {
            let ExportItem::Paragraph(paragraph) = item else {
                unreachable!();
            };
            let (_, level) = self
                .paragraph_numbering(paragraph)
                .expect("scanned list paragraph");
            items.push((*paragraph, path.clone(), level as usize));
        }
        write_list_level(self, output, &items, 0, style, continue_numbering)?;
        Ok(())
    }

    fn write_table(&self, output: &mut String, table: &CT_Tbl, path: &str) -> Result<()> {
        let columns = table_column_count(table)?;
        write!(output, "<table:table table:name=\"Table\">").unwrap();
        write!(
            output,
            "<table:table-column table:number-columns-repeated=\"{columns}\"/>"
        )
        .unwrap();
        for (row_index, row) in table.rows.iter().enumerate() {
            output.push_str("<table:table-row>");
            for (cell_index, cell) in row.cells.iter().enumerate() {
                let cell_path = format!("{path}/row[{row_index}]/cell[{cell_index}]");
                let properties = cell.properties.as_ref();
                let span = properties.and_then(|value| value.grid_span).unwrap_or(1);
                if matches!(
                    properties.and_then(|value| value.v_merge.as_ref()),
                    Some(VMerge::Continue)
                ) {
                    output.push_str("<table:covered-table-cell/>");
                    for _ in 1..span {
                        output.push_str("<table:covered-table-cell/>");
                    }
                    continue;
                }
                output.push_str("<table:table-cell");
                if span > 1 {
                    write!(output, " table:number-columns-spanned=\"{span}\"").unwrap();
                }
                if matches!(
                    properties.and_then(|value| value.v_merge.as_ref()),
                    Some(VMerge::Restart)
                ) {
                    let rowspan = table_rowspan(table, row_index, cell_index)?;
                    write!(output, " table:number-rows-spanned=\"{rowspan}\"").unwrap();
                }
                output.push('>');
                let mut wrote_paragraph = false;
                let mut content_index = 0_usize;
                let mut written_lists = BTreeSet::new();
                while content_index < cell.content.len() {
                    match &cell.content[content_index] {
                        CellContent::Paragraph(paragraph) => {
                            if let Some((num_id, _)) = self.paragraph_numbering(paragraph) {
                                let start = content_index;
                                while content_index < cell.content.len()
                                    && matches!(
                                        &cell.content[content_index],
                                        CellContent::Paragraph(candidate)
                                            if self.paragraph_numbering(candidate).is_some_and(|(candidate_id, _)| candidate_id == num_id)
                                    )
                                {
                                    content_index += 1;
                                }
                                let style =
                                    self.list_styles.get(&num_id).expect("scanned list style");
                                let mut items = Vec::with_capacity(content_index - start);
                                for item_index in start..content_index {
                                    let CellContent::Paragraph(item) = &cell.content[item_index]
                                    else {
                                        unreachable!();
                                    };
                                    let (_, level) = self
                                        .paragraph_numbering(item)
                                        .expect("scanned list paragraph");
                                    items.push((
                                        item,
                                        format!("{cell_path}/content[{item_index}]"),
                                        level as usize,
                                    ));
                                }
                                let continue_numbering = !written_lists.insert(num_id);
                                write_list_level(
                                    self,
                                    output,
                                    &items,
                                    0,
                                    style,
                                    continue_numbering,
                                )?;
                            } else {
                                self.write_paragraph(
                                    output,
                                    paragraph,
                                    &format!("{cell_path}/content[{content_index}]"),
                                )?;
                                content_index += 1;
                            }
                            wrote_paragraph = true;
                        }
                        // Content controls are unwrapped by `unwrap_table_controls`.
                        CellContent::Table(_) | CellContent::ContentControl(_) => {
                            content_index += 1;
                        }
                    }
                }
                if !wrote_paragraph {
                    output.push_str("<text:p/>");
                }
                output.push_str("</table:table-cell>");
                for _ in 1..span {
                    output.push_str("<table:covered-table-cell/>");
                }
            }
            output.push_str("</table:table-row>");
        }
        output.push_str("</table:table>");
        Ok(())
    }

    fn write_paragraph(&self, output: &mut String, paragraph: &CT_P, path: &str) -> Result<()> {
        let properties = self.effective_paragraph_properties(paragraph);
        let style_xml = paragraph_style_xml(&properties);
        let style = self.paragraph_styles.get(&style_xml);
        let heading = properties
            .outline_lvl
            .or_else(|| heading_level_from_style(properties.style_id.as_deref()));
        let tag = if heading.is_some() {
            "text:h"
        } else {
            "text:p"
        };
        write!(output, "<{tag}").unwrap();
        if let Some(style) = style {
            write!(output, " text:style-name=\"{style}\"").unwrap();
        }
        if let Some(level) = heading {
            write!(output, " text:outline-level=\"{}\"", level.clamp(0, 8) + 1).unwrap();
        }
        output.push('>');
        self.write_runs(output, paragraph, path)?;
        write!(output, "</{tag}>").unwrap();
        Ok(())
    }

    fn write_runs(&self, output: &mut String, paragraph: &CT_P, path: &str) -> Result<()> {
        // The runs the text readers read, those of content controls, tracked
        // insertions, smart tags and custom XML included, deleted ones left out.
        for (run_index, run) in paragraph.accepted_view().runs.iter().enumerate() {
            let run_path = format!("{path}/run[{run_index}]");
            let properties = self.effective_run_properties(paragraph, run);
            let style_xml = text_style_xml(&properties);
            let style = self.text_styles.get(&style_xml);
            if let Some(style) = style {
                write!(output, "<text:span text:style-name=\"{style}\">").unwrap();
            }
            for (content_index, content) in run.content.iter().enumerate() {
                let content_path = format!("{run_path}/content[{content_index}]");
                match content {
                    RunContent::Text(text) | RunContent::DeletedText(text) => {
                        write_odf_text(output, &text.text)
                    }
                    RunContent::Tab => output.push_str("<text:tab/>"),
                    RunContent::Break(BreakType::Line) => output.push_str("<text:line-break/>"),
                    RunContent::Break(_) => {}
                    RunContent::SpecialCharacter(character) => match character {
                        SpecialCharacter::CarriageReturn => output.push_str("<text:line-break/>"),
                        SpecialCharacter::NoBreakHyphen => write_odf_text(output, "\u{2011}"),
                        SpecialCharacter::SoftHyphen => write_odf_text(output, "\u{00ad}"),
                        SpecialCharacter::PositionalTab { .. } => output.push_str("<text:tab/>"),
                    },
                    // A symbol is font-encoded rather than Unicode, so there
                    // is no portable ODF spelling for it.
                    RunContent::Symbol { .. } => {}
                    RunContent::Drawing(drawing) => {
                        if let Some(inline) = &drawing.inline
                            && let Some(media_index) = self.media_at.get(&content_path)
                        {
                            let media = &self.media[*media_index];
                            write!(
                                output,
                                "<draw:frame draw:name=\"Image{}\" text:anchor-type=\"as-char\" svg:width=\"{}pt\" svg:height=\"{}pt\"><draw:image xlink:href=\"{}\" xlink:type=\"simple\" xlink:show=\"embed\" xlink:actuate=\"onLoad\"/></draw:frame>",
                                media_index + 1,
                                emu_points(inline.extent_cx.0),
                                emu_points(inline.extent_cy.0),
                                media.path
                            )
                            .unwrap();
                        }
                    }
                    RunContent::Field(field) => {
                        write_odf_text(
                            output,
                            field
                                .projected_text()
                                .unwrap_or(field.cached_result.as_str()),
                        );
                    }
                    RunContent::FootnoteRef { .. }
                    | RunContent::EndnoteRef { .. }
                    | RunContent::CommentReference { .. } => {}
                }
            }
            if style.is_some() {
                output.push_str("</text:span>");
            }
        }
        Ok(())
    }

    fn list_level_marker(&mut self, num_id: u32, level: u32, path: &str) -> Result<ListMarker> {
        let abstract_num = self
            .document
            .numbering
            .as_ref()
            .and_then(|numbering| numbering.get_abstract_num_for(num_id));
        let level_definition = self.effective_list_level(num_id, level);
        let (
            is_bullet,
            is_decimal,
            is_unnumbered,
            has_other_format,
            start,
            suffix,
            level_text,
            justification,
            paragraph_formatting,
            marker_formatting,
            retained_xml,
        ) = level_definition.map_or(
            (
                false, false, false, false, 1, false, false, false, false, false, false,
            ),
            |definition| {
                let format = definition.num_fmt.as_ref();
                let unnumbered = matches!(format, Some(ST_NumberFormat::None));
                (
                    matches!(format, Some(ST_NumberFormat::Bullet)),
                    matches!(format, Some(ST_NumberFormat::Decimal)),
                    unnumbered,
                    format.is_some()
                        && !matches!(
                            format,
                            Some(
                                ST_NumberFormat::Bullet
                                    | ST_NumberFormat::Decimal
                                    | ST_NumberFormat::None
                            )
                        ),
                    self.list_start(num_id, level),
                    definition.suffix.is_some(),
                    // A level that draws no marker loses no marker text.
                    definition.lvl_text.is_some() && !unnumbered,
                    definition.lvl_jc.is_some(),
                    definition.ppr.is_some() || definition.ppr_raw.is_some(),
                    definition.rpr.is_some() || definition.rpr_raw.is_some(),
                    !definition.extra_xml.is_empty() || !definition.extra_attributes.is_empty(),
                )
            },
        );
        let level_is_used = self.used_list_levels.contains(&(num_id, level));
        if level_is_used && abstract_num.is_some() && level_definition.is_none() {
            self.diagnose(
                path,
                "undefined numbering level was exported as decimal ODT list",
            )?;
        }
        if level_is_used && start != 1 {
            self.diagnose(path, "custom list start value was reset during ODT export")?;
        }
        for (present, message) in [
            (suffix, "list marker suffix was dropped during ODT export"),
            (
                level_text,
                "list marker text or bullet glyph was simplified during ODT export",
            ),
            (
                justification,
                "list marker justification was dropped during ODT export",
            ),
            (
                paragraph_formatting,
                "list level paragraph formatting was dropped during ODT export",
            ),
            (
                marker_formatting,
                "list marker run formatting was dropped during ODT export",
            ),
            (
                retained_xml,
                "retained list level XML or attributes were dropped during ODT export",
            ),
        ] {
            if level_is_used && present {
                self.diagnose(path, message)?;
            }
        }
        if is_bullet {
            return Ok(ListMarker::Bullet);
        }
        if is_unnumbered {
            return Ok(ListMarker::Unnumbered);
        }
        if is_decimal {
            return Ok(ListMarker::Number);
        }
        if level_is_used && has_other_format {
            self.diagnose(
                path,
                "non-decimal numbering format was exported as decimal ODT list",
            )?;
        }
        Ok(ListMarker::Number)
    }

    fn manifest_xml(&self) -> String {
        let mut output = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><manifest:manifest xmlns:manifest=\"{MANIFEST_NS}\" manifest:version=\"1.3\"><manifest:file-entry manifest:full-path=\"/\" manifest:media-type=\"{}\"/><manifest:file-entry manifest:full-path=\"content.xml\" manifest:media-type=\"text/xml\"/>",
            std::str::from_utf8(ODT_MIMETYPE).expect("ASCII MIME type")
        );
        for media in &self.media {
            write!(
                output,
                "<manifest:file-entry manifest:full-path=\"{}\" manifest:media-type=\"{}\"/>",
                media.path, media.media_type
            )
            .unwrap();
        }
        output.push_str("</manifest:manifest>");
        output
    }

    fn diagnose(&mut self, path: &str, message: &str) -> Result<()> {
        let key = (path.to_owned(), message.to_owned());
        if !self.diagnostic_keys.insert(key) {
            return Ok(());
        }
        if self.diagnostics.len() >= self.limits.diagnostics {
            return Err(odt_error(
                Some("content.xml"),
                0,
                "ODT exceeds the diagnostic limit",
            ));
        }
        self.diagnostics.push(OdtDiagnostic {
            path: path.to_owned(),
            message: message.to_owned(),
        });
        Ok(())
    }

    fn reserve_output(&mut self, amount: usize) -> Result<()> {
        self.retained_output_estimate = self
            .retained_output_estimate
            .checked_add(amount)
            .filter(|value| *value as u64 <= self.limits.archive.max_part_uncompressed_bytes)
            .ok_or_else(|| odt_error(None, 0, "ODT XML output exceeds the size limit"))?;
        Ok(())
    }
}

fn valid_xml_character(value: char) -> bool {
    matches!(value, '\u{9}' | '\u{A}' | '\u{D}')
        || ('\u{20}'..='\u{D7FF}').contains(&value)
        || ('\u{E000}'..='\u{FFFD}').contains(&value)
        || ('\u{10000}'..='\u{10FFFF}').contains(&value)
}

fn validate_xml_value(value: &str, path: &str) -> Result<()> {
    if value.chars().all(valid_xml_character) {
        Ok(())
    } else {
        Err(odt_error(
            Some("content.xml"),
            0,
            format!("invalid XML character at {path}"),
        ))
    }
}

fn validate_unicode_whitespace(value: &str, path: &str) -> Result<()> {
    if value.chars().any(|character| {
        character.is_whitespace() && !matches!(character, ' ' | '\t' | '\r' | '\n')
    }) {
        return Err(odt_error(
            Some("content.xml"),
            0,
            format!("ODT cannot preserve Unicode whitespace at {path}"),
        ));
    }
    Ok(())
}

fn write_odt_entry(
    archive: &mut ZipWriter<&mut Cursor<Vec<u8>>>,
    name: &str,
    bytes: &[u8],
    method: CompressionMethod,
) -> Result<()> {
    archive
        .start_file(
            name,
            SimpleFileOptions::DEFAULT
                .compression_method(method)
                .unix_permissions(0o644),
        )
        .map_err(|error| odt_error(Some(name), 0, format!("cannot create ODT entry: {error}")))?;
    archive
        .write_all(bytes)
        .map_err(|error| odt_error(Some(name), 0, format!("cannot write ODT entry: {error}")))
}

fn write_list_level(
    writer: &OdtWriter<'_>,
    output: &mut String,
    items: &[(&CT_P, String, usize)],
    level: usize,
    style: &str,
    continue_numbering: bool,
) -> Result<()> {
    write!(output, "<text:list text:style-name=\"{style}\"").unwrap();
    if continue_numbering {
        output.push_str(" text:continue-numbering=\"true\"");
    }
    output.push('>');
    let mut index = 0_usize;
    while index < items.len() {
        let (paragraph, path, item_level) = &items[index];
        if *item_level < level {
            break;
        }
        if *item_level > level {
            let nested_start = index;
            while index < items.len() && items[index].2 > level {
                index += 1;
            }
            output.push_str("<text:list-item>");
            write_list_level(
                writer,
                output,
                &items[nested_start..index],
                level + 1,
                style,
                false,
            )?;
            output.push_str("</text:list-item>");
            continue;
        }
        output.push_str("<text:list-item>");
        writer.write_paragraph(output, paragraph, path)?;
        index += 1;
        if index < items.len() && items[index].2 > level {
            let nested_start = index;
            while index < items.len() && items[index].2 > level {
                index += 1;
            }
            write_list_level(
                writer,
                output,
                &items[nested_start..index],
                level + 1,
                style,
                false,
            )?;
        }
        output.push_str("</text:list-item>");
    }
    output.push_str("</text:list>");
    Ok(())
}

fn paragraph_numbering_properties(properties: &CT_PPr) -> Option<(u32, u32)> {
    let num_id = properties.num_id?;
    (num_id != 0).then_some((num_id, properties.num_ilvl.unwrap_or(0)))
}

fn validate_paragraph_projection(properties: &CT_PPr, path: &str) -> Result<()> {
    const MAX_LENGTH_TWIPS: i32 = 20_000_000;
    for (name, value) in [
        ("space-before", properties.space_before),
        ("space-after", properties.space_after),
        ("left-indent", properties.ind_left),
        ("right-indent", properties.ind_right),
    ] {
        if value.is_some_and(|value| !(0..=MAX_LENGTH_TWIPS).contains(&value.0)) {
            return Err(odt_error(
                Some("content.xml"),
                0,
                format!("ODT cannot preserve {name} at {path}"),
            ));
        }
    }
    if properties
        .ind_first_line
        .is_some_and(|value| value.0.unsigned_abs() > MAX_LENGTH_TWIPS as u32)
    {
        return Err(odt_error(
            Some("content.xml"),
            0,
            format!("ODT cannot preserve first-line indent at {path}"),
        ));
    }
    if properties
        .ind_hanging
        .is_some_and(|value| !(0..=MAX_LENGTH_TWIPS).contains(&value.0))
    {
        return Err(odt_error(
            Some("content.xml"),
            0,
            format!("ODT cannot preserve hanging indent at {path}"),
        ));
    }
    if let Some(line) = properties.line_spacing {
        let valid = match properties.line_rule.as_deref() {
            Some("exact" | "atLeast") => (1..=MAX_LENGTH_TWIPS).contains(&line.0),
            _ => (1..=24_000).contains(&line.0),
        };
        if !valid {
            return Err(odt_error(
                Some("content.xml"),
                0,
                format!("ODT cannot preserve line spacing at {path}"),
            ));
        }
    }
    if properties.outline_lvl.is_some_and(|level| level > 8) {
        return Err(odt_error(
            Some("content.xml"),
            0,
            format!("ODT cannot preserve outline level at {path}"),
        ));
    }
    if properties.outline_lvl.is_none()
        && let Some(suffix) = properties
            .style_id
            .as_deref()
            .and_then(|style| style.strip_prefix("Heading"))
        && !suffix.is_empty()
        && suffix.bytes().all(|byte| byte.is_ascii_digit())
        && !suffix
            .parse::<u32>()
            .is_ok_and(|level| (1..=9).contains(&level))
    {
        return Err(odt_error(
            Some("content.xml"),
            0,
            format!("ODT cannot preserve outline level at {path}"),
        ));
    }
    Ok(())
}

fn validate_run_projection(properties: &CT_RPr, path: &str) -> Result<()> {
    const MAX_FONT_HALF_POINTS: u32 = 2_000_000;
    if properties
        .sz
        .is_some_and(|size| !(1..=MAX_FONT_HALF_POINTS).contains(&size.0))
    {
        return Err(odt_error(
            Some("content.xml"),
            0,
            format!("ODT cannot preserve font size at {path}"),
        ));
    }
    Ok(())
}

fn validate_font_family_projection(font: &str, path: &str) -> Result<()> {
    let normalized = font
        .chars()
        .map(|character| match character {
            '\t' | '\r' | '\n' => ' ',
            other => other,
        })
        .collect::<String>();
    if normalized.trim().trim_matches(['\'', '"']) != font {
        return Err(odt_error(
            Some("content.xml"),
            0,
            format!("ODT cannot preserve font family at {path}"),
        ));
    }
    Ok(())
}

fn final_section_has_unsupported_properties(
    section: &oxoffice_docx_oxml::document::CT_SectPr,
) -> bool {
    let mut projected = section.clone();
    projected.header_refs.clear();
    projected.footer_refs.clear();
    // The section writer omits explicit portrait. Reopening that same source
    // leaves orientation absent, which has the same effective portrait value.
    if projected.orientation.is_none() {
        projected.orientation = Some(oxoffice_docx_oxml::shared::ST_PageOrientation::Portrait);
    }
    projected != oxoffice_docx_oxml::document::CT_SectPr::default_letter()
}

fn paragraph_style_xml(properties: &CT_PPr) -> String {
    let mut output = String::new();
    if let Some(alignment) = properties.jc {
        let value = match alignment {
            ST_Jc::Start | ST_Jc::Left => "left",
            ST_Jc::End | ST_Jc::Right => "right",
            ST_Jc::Center => "center",
            ST_Jc::Both | ST_Jc::Distribute => "justify",
        };
        write!(output, " fo:text-align=\"{value}\"").unwrap();
    }
    for (name, value) in [
        ("margin-top", properties.space_before),
        ("margin-bottom", properties.space_after),
        ("margin-left", properties.ind_left),
        ("margin-right", properties.ind_right),
    ] {
        if let Some(value) = value {
            write!(output, " fo:{name}=\"{}pt\"", twips_points(value.0)).unwrap();
        }
    }
    // Word ignores `w:firstLine` when `w:hanging` is there, and so does layout.
    if let Some(value) = properties.ind_hanging {
        write!(
            output,
            " fo:text-indent=\"{}pt\"",
            twips_points(value.0.saturating_neg())
        )
        .unwrap();
    } else if let Some(value) = properties.ind_first_line {
        write!(output, " fo:text-indent=\"{}pt\"", twips_points(value.0)).unwrap();
    }
    if let Some(value) = properties.line_spacing {
        match properties.line_rule.as_deref() {
            Some("exact" | "atLeast") => {
                write!(output, " fo:line-height=\"{}pt\"", twips_points(value.0)).unwrap();
            }
            _ => {
                write!(
                    output,
                    " fo:line-height=\"{}%\"",
                    automatic_line_height_percent(value.0)
                )
                .unwrap();
            }
        }
    }
    output
}

fn text_style_xml(properties: &CT_RPr) -> String {
    let mut output = String::new();
    if let Some(font) = selected_run_font(properties) {
        write!(output, " fo:font-family=\"{}\"", escape_xml(font)).unwrap();
    }
    if let Some(size) = properties.sz {
        write!(output, " fo:font-size=\"{}pt\"", decimal(size.to_pt())).unwrap();
    }
    if let Some(value) = properties.bold {
        write!(
            output,
            " fo:font-weight=\"{}\"",
            if value { "bold" } else { "normal" }
        )
        .unwrap();
    }
    if let Some(value) = properties.italic {
        write!(
            output,
            " fo:font-style=\"{}\"",
            if value { "italic" } else { "normal" }
        )
        .unwrap();
    }
    if let Some(value) = properties.underline {
        write!(
            output,
            " style:text-underline-style=\"{}\"",
            if value == ST_Underline::None {
                "none"
            } else {
                "solid"
            }
        )
        .unwrap();
    }
    if let Some(value) = properties.strike {
        write!(
            output,
            " style:text-line-through-style=\"{}\"",
            if value { "solid" } else { "none" }
        )
        .unwrap();
    }
    if let Some(color) = properties.color.as_deref().and_then(normalized_color) {
        write!(output, " fo:color=\"#{color}\"").unwrap();
    }
    let background = properties
        .shading
        .as_ref()
        .and_then(|shading| shading.fill.as_deref())
        .and_then(normalized_color)
        .or_else(|| properties.highlight.and_then(highlight_color));
    if let Some(background) = background {
        write!(output, " fo:background-color=\"#{background}\"").unwrap();
    }
    match properties.vert_align.as_deref() {
        Some("superscript") => output.push_str(" style:text-position=\"super\""),
        Some("subscript") => output.push_str(" style:text-position=\"sub\""),
        _ => {}
    }
    output
}

fn selected_run_font(properties: &CT_RPr) -> Option<&str> {
    properties
        .font_ascii
        .as_deref()
        .or(properties.font_hansi.as_deref())
        .or(properties.font_east_asia.as_deref())
        .or(properties.font_cs.as_deref())
}

fn normalized_color(value: &str) -> Option<String> {
    let value = value.trim_start_matches('#');
    (value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| value.to_ascii_uppercase())
}

fn highlight_color(value: ST_HighlightColor) -> Option<String> {
    let value = match value {
        ST_HighlightColor::Black => "000000",
        ST_HighlightColor::Blue => "0000FF",
        ST_HighlightColor::Cyan => "00FFFF",
        ST_HighlightColor::DarkBlue => "000080",
        ST_HighlightColor::DarkCyan => "008080",
        ST_HighlightColor::DarkGray => "808080",
        ST_HighlightColor::DarkGreen => "008000",
        ST_HighlightColor::DarkMagenta => "800080",
        ST_HighlightColor::DarkRed => "800000",
        ST_HighlightColor::DarkYellow => "808000",
        ST_HighlightColor::Green => "00FF00",
        ST_HighlightColor::LightGray => "C0C0C0",
        ST_HighlightColor::Magenta => "FF00FF",
        ST_HighlightColor::None => return None,
        ST_HighlightColor::Red => "FF0000",
        ST_HighlightColor::White => "FFFFFF",
        ST_HighlightColor::Yellow => "FFFF00",
    };
    Some(value.to_owned())
}

fn heading_level_from_style(style: Option<&str>) -> Option<u32> {
    let level = style?.strip_prefix("Heading")?.parse::<u32>().ok()?;
    (1..=9).contains(&level).then_some(level - 1)
}

fn projected_text_run_pieces(
    text: &str,
    style: &str,
    trailing_text_style: &mut Option<String>,
) -> usize {
    let mut pieces = 0;
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\t' | '\n' => {
                pieces += 1;
                *trailing_text_style = None;
            }
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
                pieces += 1;
                *trailing_text_style = None;
            }
            _ if trailing_text_style.as_deref() != Some(style) => {
                pieces += 1;
                *trailing_text_style = Some(style.to_owned());
            }
            _ => {}
        }
    }
    pieces
}

fn write_odf_text(output: &mut String, text: &str) {
    let mut ordinary = String::new();
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            ' ' => {
                if !ordinary.is_empty() {
                    output.push_str(&escape_xml(&ordinary));
                    ordinary.clear();
                }
                output.push_str("<text:s/>");
            }
            '\t' => {
                if !ordinary.is_empty() {
                    output.push_str(&escape_xml(&ordinary));
                    ordinary.clear();
                }
                output.push_str("<text:tab/>");
            }
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
                if !ordinary.is_empty() {
                    output.push_str(&escape_xml(&ordinary));
                    ordinary.clear();
                }
                output.push_str("<text:line-break/>");
            }
            '\n' => {
                if !ordinary.is_empty() {
                    output.push_str(&escape_xml(&ordinary));
                    ordinary.clear();
                }
                output.push_str("<text:line-break/>");
            }
            value => ordinary.push(value),
        }
    }
    if !ordinary.is_empty() {
        output.push_str(&escape_xml(&ordinary));
    }
}

fn escape_xml(value: &str) -> String {
    quick_xml::escape::escape(value).into_owned()
}

fn decimal(value: f64) -> String {
    let mut output = format!("{value:.10}");
    while output.contains('.') && output.ends_with('0') {
        output.pop();
    }
    if output.ends_with('.') {
        output.pop();
    }
    output
}

fn automatic_line_height_percent(value: i32) -> String {
    const SCALE: i64 = 10_000_000_000;
    let numerator = i64::from(value) * 5 * SCALE;
    let scaled = if value == 24_000 {
        numerator / 12
    } else {
        numerator / 12 + 1
    };
    let whole = scaled / SCALE;
    let fraction = scaled % SCALE;
    if fraction == 0 {
        return whole.to_string();
    }
    let mut fraction = format!("{fraction:010}");
    while fraction.ends_with('0') {
        fraction.pop();
    }
    format!("{whole}.{fraction}")
}

fn twips_points(value: i32) -> String {
    if value.unsigned_abs() == 20_000_000 {
        return (value / 20).to_string();
    }
    let points = f64::from(value) / 20.0;
    decimal(points + points.signum() * 0.000_000_001)
}

fn emu_points(value: i64) -> String {
    if value == 12_700_000_000 {
        return "1000000".to_string();
    }
    decimal((value as f64 + 0.25) / 12_700.0)
}

fn paragraph_properties_have_unsupported(properties: &CT_PPr) -> bool {
    properties.style_id.is_some()
        || properties.jc == Some(ST_Jc::Distribute)
        || (properties.line_rule.is_some() && properties.line_spacing.is_none())
        || (properties.num_ilvl.is_some() && properties.num_id.is_none())
        || (properties.ind_first_line.is_some() && properties.ind_hanging.is_some())
        || properties.before_autospacing.is_some()
        || properties.after_autospacing.is_some()
        || properties.keep_next.is_some()
        || properties.keep_lines.is_some()
        || properties.page_break_before.is_some()
        || properties.widow_control.is_some()
        || properties.suppress_auto_hyphens.is_some()
        || properties.borders.is_some()
        || properties.tabs.is_some()
        || properties.shading.is_some()
        || properties.rpr.is_some()
        || properties.sect_pr.is_some()
        || properties.numbering_revision.is_some()
        || !properties.numbering_revision_xml.is_empty()
        || properties.change.is_some()
        || !properties.revision_xml.is_empty()
}

fn run_properties_have_unsupported(properties: &CT_RPr) -> bool {
    let selected = selected_run_font(properties);
    let alternate_font = [
        properties.font_hansi.as_deref(),
        properties.font_east_asia.as_deref(),
        properties.font_cs.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|font| Some(font) != selected);
    properties.style_id.is_some()
        || alternate_font
        || properties.font_ascii_theme.is_some()
        || properties.font_hansi_theme.is_some()
        || properties
            .bold_cs
            .is_some_and(|value| Some(value) != properties.bold)
        || properties
            .italic_cs
            .is_some_and(|value| Some(value) != properties.italic)
        || properties
            .underline
            .is_some_and(|value| !matches!(value, ST_Underline::None | ST_Underline::Single))
        || properties.dstrike.is_some()
        || properties
            .sz_cs
            .is_some_and(|value| Some(value) != properties.sz)
        || properties.color_theme.is_some()
        || properties.spacing.is_some()
        || properties.width_scale.is_some()
        || properties.position.is_some()
        || properties
            .vert_align
            .as_deref()
            .is_some_and(|value| !matches!(value, "superscript" | "subscript"))
        || properties.caps.is_some()
        || properties.small_caps.is_some()
        || properties.vanish.is_some()
        || !properties.revision_markers.is_empty()
        || properties.change.is_some()
        || !properties.revision_xml.is_empty()
}

fn cell_has_lossy_properties(properties: &CT_TcPr) -> bool {
    properties.width.is_some()
        || properties.borders.is_some()
        || properties.shading.is_some()
        || properties.v_align.is_some()
        || properties.no_wrap.is_some()
        || properties.text_direction.is_some()
        || properties.cnf_style.is_some()
        || !properties.extra_xml.is_empty()
}

fn cell_has_substantive_content(cell: &CT_Tc) -> bool {
    cell.content.iter().any(|content| match content {
        CellContent::Paragraph(paragraph) => paragraph != &CT_P::new(),
        CellContent::Table(_) | CellContent::ContentControl(_) => true,
    })
}

fn table_has_lossy_properties(properties: &CT_TblPr) -> bool {
    properties.width.is_some()
        || properties.style_id.is_some()
        || properties.jc.is_some()
        || properties.borders.is_some()
        || properties.cell_margin.is_some()
        || properties.layout.is_some()
        || properties.indent.is_some()
        || properties.shading.is_some()
        || properties.look.is_some()
        || properties.change.is_some()
        || !properties.revision_xml.is_empty()
}

fn table_column_count(table: &CT_Tbl) -> Result<usize> {
    let grid = table.grid.as_ref().map_or(0, |grid| grid.columns.len());
    let rows = table.rows.iter().try_fold(0_usize, |maximum, row| {
        row_width(row).map(|width| maximum.max(width))
    })?;
    let columns = grid.max(rows);
    if columns == 0 || columns > OdtLimits::DEFAULT.columns {
        return Err(odt_error(
            Some("content.xml"),
            0,
            "ODT table column count is invalid",
        ));
    }
    Ok(columns)
}

fn validate_word_table(table: &CT_Tbl, path: &str) -> Result<()> {
    let columns = table_column_count(table)?;
    for (row_index, row) in table.rows.iter().enumerate() {
        if row_width(row)? != columns {
            return Err(odt_error(
                Some("content.xml"),
                0,
                format!("malformed Word table grid at {path}/row[{row_index}]"),
            ));
        }
    }
    for (row_index, row) in table.rows.iter().enumerate() {
        for (cell_index, cell) in row.cells.iter().enumerate() {
            if matches!(
                cell.properties
                    .as_ref()
                    .and_then(|properties| properties.v_merge.as_ref()),
                Some(VMerge::Continue)
            ) {
                let column = row_cell_column(row, cell_index)?;
                let span = cell
                    .properties
                    .as_ref()
                    .and_then(|properties| properties.grid_span)
                    .unwrap_or(1);
                let valid = if row_index == 0 {
                    false
                } else {
                    match row_cell_covering_column(&table.rows[row_index - 1], column)? {
                        Some((candidate_column, candidate)) => {
                            let properties = candidate.properties.as_ref();
                            let candidate_span =
                                properties.and_then(|value| value.grid_span).unwrap_or(1);
                            let merge = properties.and_then(|value| value.v_merge.as_ref());
                            candidate_column == column
                                && candidate_span == span
                                && matches!(merge, Some(VMerge::Restart | VMerge::Continue))
                        }
                        None => false,
                    }
                };
                if !valid {
                    return Err(odt_error(
                        Some("content.xml"),
                        0,
                        format!(
                            "malformed vertical table span at {path}/row[{row_index}]/cell[{cell_index}]"
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn row_cell_covering_column(row: &CT_Row, column: usize) -> Result<Option<(usize, &CT_Tc)>> {
    let mut start = 0_usize;
    for cell in &row.cells {
        let end = start
            .checked_add(cell_grid_span(cell)?)
            .ok_or_else(|| odt_error(Some("content.xml"), 0, "ODT table grid span overflows"))?;
        if (start..end).contains(&column) {
            return Ok(Some((start, cell)));
        }
        start = end;
    }
    Ok(None)
}

fn cell_grid_span(cell: &CT_Tc) -> Result<usize> {
    let span = cell
        .properties
        .as_ref()
        .and_then(|properties| properties.grid_span)
        .unwrap_or(1);
    usize::try_from(span)
        .ok()
        .filter(|span| *span > 0)
        .ok_or_else(|| odt_error(Some("content.xml"), 0, "ODT table grid span is invalid"))
}

fn row_width(row: &CT_Row) -> Result<usize> {
    row.cells.iter().try_fold(0_usize, |column, cell| {
        column
            .checked_add(cell_grid_span(cell)?)
            .ok_or_else(|| odt_error(Some("content.xml"), 0, "ODT table grid span overflows"))
    })
}

fn row_cell_column(row: &CT_Row, cell_index: usize) -> Result<usize> {
    row.cells[..cell_index]
        .iter()
        .try_fold(0_usize, |column, cell| {
            column
                .checked_add(cell_grid_span(cell)?)
                .ok_or_else(|| odt_error(Some("content.xml"), 0, "ODT table grid span overflows"))
        })
}

fn table_rowspan(table: &CT_Tbl, row_index: usize, cell_index: usize) -> Result<usize> {
    let row = &table.rows[row_index];
    let column = row_cell_column(row, cell_index)?;
    let span = row.cells[cell_index]
        .properties
        .as_ref()
        .and_then(|properties| properties.grid_span)
        .unwrap_or(1);
    let mut rowspan = 1_usize;
    for candidate_row in table.rows.iter().skip(row_index + 1) {
        let Some((_, candidate)) = candidate_row.cells.iter().enumerate().find(|(index, _)| {
            row_cell_column(candidate_row, *index)
                .is_ok_and(|candidate_column| candidate_column == column)
        }) else {
            break;
        };
        let properties = candidate.properties.as_ref();
        if !matches!(
            properties.and_then(|value| value.v_merge.as_ref()),
            Some(VMerge::Continue)
        ) || properties.and_then(|value| value.grid_span).unwrap_or(1) != span
        {
            break;
        }
        rowspan += 1;
    }
    if rowspan == 1 {
        return Err(odt_error(
            Some("content.xml"),
            0,
            "vertical table merge restart has no continuation",
        ));
    }
    Ok(rowspan)
}

fn from_odt_with_limits(bytes: &[u8], limits: OdtLimits) -> Result<OdtReadResult> {
    let mut archive = OdtArchive::open(bytes, limits.archive)?;
    let mimetype = archive.read_required("mimetype")?;
    if mimetype.as_slice() != ODT_MIMETYPE {
        return Err(odt_error(
            Some("mimetype"),
            0,
            "ODT mimetype is missing or invalid",
        ));
    }
    let content_bytes = archive.read_required("content.xml")?;
    let styles_bytes = archive.read_optional("styles.xml")?;
    let manifest_bytes = archive.read_optional("META-INF/manifest.xml")?;

    let manifest = manifest_bytes
        .as_deref()
        .map(|xml| parse_xml("META-INF/manifest.xml", xml, limits))
        .transpose()?;
    let encrypted = encrypted_manifest_paths(manifest.as_ref())?;
    for required_part in ["/", "content.xml"] {
        if encrypted.contains(required_part) {
            return Err(odt_error(
                Some(required_part),
                0,
                "encrypted required ODT content is unsupported",
            ));
        }
    }
    if styles_bytes.is_some() && encrypted.contains("styles.xml") {
        return Err(odt_error(
            Some("styles.xml"),
            0,
            "encrypted ODT styles are unsupported",
        ));
    }

    let content = parse_xml("content.xml", &content_bytes, limits)?;
    let styles = styles_bytes
        .as_deref()
        .map(|xml| parse_xml("styles.xml", xml, limits))
        .transpose()?;

    Importer::new(archive, content, styles, encrypted, limits).project()
}

struct OdtArchive<'a> {
    archive: ZipArchive<Cursor<&'a [u8]>>,
    names: HashSet<String>,
    limit: u64,
}

impl<'a> OdtArchive<'a> {
    fn open(bytes: &'a [u8], limits: PackageReadLimits) -> Result<Self> {
        let raw_names = central_directory_names(bytes)?;
        if raw_names.len() > limits.max_entries {
            return Err(odt_error(None, 0, "ODT archive exceeds the entry limit"));
        }
        let mut names = HashSet::with_capacity(raw_names.len());
        for name in raw_names {
            validate_entry_name(&name)?;
            if !names.insert(name.clone()) {
                return Err(odt_error(Some(&name), 0, "duplicate ODT archive entry"));
            }
        }
        let mut archive = ZipArchive::new(Cursor::new(bytes))
            .map_err(|error| odt_error(None, 0, format!("invalid ODT ZIP: {error}")))?;
        if archive.len() > limits.max_entries {
            return Err(odt_error(None, 0, "ODT archive exceeds the entry limit"));
        }
        let mut total = 0_u64;
        for index in 0..archive.len() {
            let entry = archive.by_index(index).map_err(|error| {
                odt_error(None, 0, format!("cannot index ODT archive: {error}"))
            })?;
            let decoded_name = entry.name().map_err(|error| {
                odt_error(None, 0, format!("cannot decode ODT entry name: {error}"))
            })?;
            let name = decoded_name.as_ref();
            validate_entry_name(name)?;
            if !entry.is_file() {
                return Err(odt_error(Some(name), 0, "ODT entry is not a regular file"));
            }
            if entry.encrypted() {
                return Err(odt_error(
                    Some(name),
                    0,
                    "encrypted ZIP entries are unsupported",
                ));
            }
            if !matches!(
                entry.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            ) {
                return Err(odt_error(
                    Some(name),
                    0,
                    "ODT entry uses unsupported compression",
                ));
            }
            if entry.size() > limits.max_part_uncompressed_bytes {
                return Err(odt_error(Some(name), 0, "ODT part exceeds the size limit"));
            }
            total = total
                .checked_add(entry.size())
                .filter(|total| *total <= limits.max_total_uncompressed_bytes)
                .ok_or_else(|| odt_error(None, 0, "ODT archive exceeds the expansion limit"))?;
        }
        if !names.contains("mimetype") || !names.contains("content.xml") {
            return Err(odt_error(None, 0, "ODT requires mimetype and content.xml"));
        }
        Ok(Self {
            archive,
            names,
            limit: limits.max_part_uncompressed_bytes,
        })
    }

    fn read_required(&mut self, name: &str) -> Result<Vec<u8>> {
        self.read_optional(name)?.ok_or_else(|| {
            odt_error(
                Some(name),
                0,
                format!("required ODT part {name} is missing"),
            )
        })
    }

    fn read_optional(&mut self, name: &str) -> Result<Option<Vec<u8>>> {
        if !self.names.contains(name) {
            return Ok(None);
        }
        let mut entry = self
            .archive
            .by_name(name)
            .map_err(|error| odt_error(Some(name), 0, format!("cannot read ODT part: {error}")))?;
        let mut bytes = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
        entry
            .by_ref()
            .take(self.limit.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| {
                odt_error(Some(name), 0, format!("cannot expand ODT part: {error}"))
            })?;
        if bytes.len() as u64 > self.limit {
            return Err(odt_error(Some(name), 0, "ODT part exceeds the size limit"));
        }
        Ok(Some(bytes))
    }
}

fn central_directory_names(bytes: &[u8]) -> Result<Vec<String>> {
    const EOCD: &[u8; 4] = b"PK\x05\x06";
    const CENTRAL: &[u8; 4] = b"PK\x01\x02";
    let search_start = bytes.len().saturating_sub(65_557);
    let eocd = (search_start..bytes.len().saturating_sub(3))
        .rev()
        .find(|offset| &bytes[*offset..*offset + 4] == EOCD)
        .ok_or_else(|| odt_error(None, 0, "ODT ZIP end record is missing"))?;
    if eocd + 22 > bytes.len() {
        return Err(odt_error(None, eocd as u64, "truncated ODT ZIP end record"));
    }
    let disk = read_u16(bytes, eocd + 4)?;
    let central_disk = read_u16(bytes, eocd + 6)?;
    let disk_entries = read_u16(bytes, eocd + 8)?;
    let entries = read_u16(bytes, eocd + 10)?;
    let central_size = read_u32(bytes, eocd + 12)?;
    let central_offset = read_u32(bytes, eocd + 16)?;
    let comment = read_u16(bytes, eocd + 20)? as usize;
    if disk != 0 || central_disk != 0 || disk_entries != entries {
        return Err(odt_error(
            None,
            eocd as u64,
            "multi-disk ODT ZIP is unsupported",
        ));
    }
    if entries == u16::MAX || central_size == u32::MAX || central_offset == u32::MAX {
        return Err(odt_error(
            None,
            eocd as u64,
            "ZIP64 ODT archives are unsupported",
        ));
    }
    if eocd + 22 + comment != bytes.len() {
        return Err(odt_error(
            None,
            eocd as u64,
            "invalid ODT ZIP comment length",
        ));
    }
    let mut position = central_offset as usize;
    let central_end = position
        .checked_add(central_size as usize)
        .filter(|end| *end <= eocd)
        .ok_or_else(|| {
            odt_error(
                None,
                position as u64,
                "invalid ODT central directory bounds",
            )
        })?;
    let mut names = Vec::with_capacity(entries as usize);
    for _ in 0..entries {
        if position + 46 > central_end || &bytes[position..position + 4] != CENTRAL {
            return Err(odt_error(
                None,
                position as u64,
                "invalid ODT central directory entry",
            ));
        }
        let name_len = read_u16(bytes, position + 28)? as usize;
        let extra_len = read_u16(bytes, position + 30)? as usize;
        let comment_len = read_u16(bytes, position + 32)? as usize;
        let name_start = position + 46;
        let next = name_start
            .checked_add(name_len)
            .and_then(|value| value.checked_add(extra_len))
            .and_then(|value| value.checked_add(comment_len))
            .filter(|next| *next <= central_end)
            .ok_or_else(|| odt_error(None, position as u64, "truncated ODT central entry"))?;
        let name = std::str::from_utf8(&bytes[name_start..name_start + name_len])
            .map_err(|_| odt_error(None, position as u64, "ODT entry name is not UTF-8"))?;
        names.push(name.to_string());
        position = next;
    }
    if position != central_end {
        return Err(odt_error(
            None,
            position as u64,
            "unexpected ODT central directory bytes",
        ));
    }
    Ok(names)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| odt_error(None, offset as u64, "truncated ODT ZIP field"))?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| odt_error(None, offset as u64, "truncated ODT ZIP field"))?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn validate_entry_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.starts_with('/')
        || name.starts_with('\\')
        || name.contains('\\')
        || name.contains('\0')
        || name
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(odt_error(Some(name), 0, "unsafe ODT archive entry name"));
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct XmlName {
    namespace: Option<String>,
    local: String,
}

#[derive(Clone, Debug)]
struct XmlAttribute {
    name: XmlName,
    value: String,
}

#[derive(Clone, Debug)]
enum XmlChild {
    Element(XmlNode),
    Text(String),
}

#[derive(Clone, Debug)]
struct XmlNode {
    name: XmlName,
    attributes: Vec<XmlAttribute>,
    children: Vec<XmlChild>,
}

impl XmlNode {
    fn is(&self, namespace: &str, local: &str) -> bool {
        self.name.namespace.as_deref() == Some(namespace) && self.name.local == local
    }

    fn attr(&self, namespace: Option<&str>, local: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|attribute| {
                attribute.name.namespace.as_deref() == namespace && attribute.name.local == local
            })
            .map(|attribute| attribute.value.as_str())
    }

    fn elements(&self) -> impl Iterator<Item = &XmlNode> {
        self.children.iter().filter_map(|child| match child {
            XmlChild::Element(element) => Some(element),
            XmlChild::Text(_) => None,
        })
    }
}

fn parse_xml(part: &str, xml: &[u8], limits: OdtLimits) -> Result<XmlNode> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut stack: Vec<XmlNode> = Vec::new();
    let mut root = None;
    let mut nodes = 0_usize;
    let mut text_bytes = 0_usize;
    loop {
        let offset = reader.buffer_position();
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| odt_error(Some(part), offset, format!("malformed XML: {error}")))?;
        let namespace = namespace_value(namespace, part, offset)?;
        match event {
            Event::Start(element) => {
                nodes = nodes.checked_add(1).ok_or_else(|| {
                    odt_error(Some(part), offset, "ODT XML node count overflowed")
                })?;
                if nodes > limits.xml_nodes {
                    return Err(odt_error(
                        Some(part),
                        offset,
                        "ODT XML exceeds the node limit",
                    ));
                }
                if stack.len() >= limits.xml_depth {
                    return Err(odt_error(
                        Some(part),
                        offset,
                        "ODT XML exceeds the depth limit",
                    ));
                }
                stack.push(xml_node(&reader, namespace, &element, part, offset)?);
            }
            Event::Empty(element) => {
                nodes = nodes.checked_add(1).ok_or_else(|| {
                    odt_error(Some(part), offset, "ODT XML node count overflowed")
                })?;
                if nodes > limits.xml_nodes {
                    return Err(odt_error(
                        Some(part),
                        offset,
                        "ODT XML exceeds the node limit",
                    ));
                }
                if stack.len() >= limits.xml_depth {
                    return Err(odt_error(
                        Some(part),
                        offset,
                        "ODT XML exceeds the depth limit",
                    ));
                }
                attach_node(
                    &mut stack,
                    &mut root,
                    xml_node(&reader, namespace, &element, part, offset)?,
                    part,
                    offset,
                )?;
            }
            Event::End(_) => {
                let node = stack.pop().ok_or_else(|| {
                    odt_error(Some(part), offset, "ODT XML end tag has no open element")
                })?;
                attach_node(&mut stack, &mut root, node, part, offset)?;
            }
            Event::Text(text) => {
                let decoded = text.xml10_content().map_err(|error| {
                    odt_error(Some(part), offset, format!("invalid XML text: {error}"))
                })?;
                let value = quick_xml::escape::unescape(&decoded)
                    .map_err(|error| {
                        odt_error(
                            Some(part),
                            offset,
                            format!("unresolved XML entity: {error}"),
                        )
                    })?
                    .into_owned();
                append_text(&mut stack, value, &mut text_bytes, limits, part, offset)?;
            }
            Event::CData(text) => {
                let value = text
                    .xml10_content()
                    .map_err(|error| {
                        odt_error(Some(part), offset, format!("invalid XML CDATA: {error}"))
                    })?
                    .into_owned();
                append_text(&mut stack, value, &mut text_bytes, limits, part, offset)?;
            }
            Event::GeneralRef(reference) => {
                let value = if let Some(character) =
                    reference.resolve_char_ref().map_err(|error| {
                        odt_error(
                            Some(part),
                            offset,
                            format!("invalid XML reference: {error}"),
                        )
                    })? {
                    character.to_string()
                } else {
                    let reference_bytes: &[u8] = &reference;
                    match reference_bytes {
                        b"amp" => "&".to_string(),
                        b"lt" => "<".to_string(),
                        b"gt" => ">".to_string(),
                        b"apos" => "'".to_string(),
                        b"quot" => "\"".to_string(),
                        _ => {
                            return Err(odt_error(
                                Some(part),
                                offset,
                                "unresolved XML entity is unsupported",
                            ));
                        }
                    }
                };
                append_text(&mut stack, value, &mut text_bytes, limits, part, offset)?;
            }
            Event::DocType(_) => {
                return Err(odt_error(
                    Some(part),
                    offset,
                    "DTD declarations are forbidden",
                ));
            }
            Event::Eof => break,
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) => {}
        }
        buffer.clear();
    }
    if !stack.is_empty() {
        return Err(odt_error(
            Some(part),
            xml.len() as u64,
            "unclosed XML element",
        ));
    }
    root.ok_or_else(|| odt_error(Some(part), 0, "ODT XML has no root element"))
}

fn namespace_value(
    namespace: ResolveResult<'_>,
    part: &str,
    offset: u64,
) -> Result<Option<String>> {
    match namespace {
        ResolveResult::Bound(namespace) => Ok(Some(
            std::str::from_utf8(namespace.as_ref())
                .map_err(|_| odt_error(Some(part), offset, "namespace URI is not UTF-8"))?
                .to_string(),
        )),
        ResolveResult::Unbound => Ok(None),
        ResolveResult::Unknown(_) => Err(odt_error(
            Some(part),
            offset,
            "ODT XML uses an unresolved namespace prefix",
        )),
    }
}

fn xml_node(
    reader: &NsReader<&[u8]>,
    namespace: Option<String>,
    element: &BytesStart<'_>,
    part: &str,
    offset: u64,
) -> Result<XmlNode> {
    let local = std::str::from_utf8(element.local_name().as_ref())
        .map_err(|_| odt_error(Some(part), offset, "element local name is not UTF-8"))?
        .to_string();
    let mut attributes = Vec::new();
    let mut seen = HashSet::new();
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            odt_error(
                Some(part),
                offset,
                format!("malformed XML attribute: {error}"),
            )
        })?;
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        let namespace = namespace_value(namespace, part, offset)?;
        let local = std::str::from_utf8(local.as_ref())
            .map_err(|_| odt_error(Some(part), offset, "attribute local name is not UTF-8"))?
            .to_string();
        if !seen.insert((namespace.clone(), local.clone())) {
            return Err(odt_error(
                Some(part),
                offset,
                "duplicate expanded XML attribute",
            ));
        }
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
            .map_err(|error| {
                odt_error(
                    Some(part),
                    offset,
                    format!("invalid XML attribute value: {error}"),
                )
            })?
            .into_owned();
        attributes.push(XmlAttribute {
            name: XmlName { namespace, local },
            value,
        });
    }
    Ok(XmlNode {
        name: XmlName { namespace, local },
        attributes,
        children: Vec::new(),
    })
}

fn attach_node(
    stack: &mut [XmlNode],
    root: &mut Option<XmlNode>,
    node: XmlNode,
    part: &str,
    offset: u64,
) -> Result<()> {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(XmlChild::Element(node));
    } else if root.replace(node).is_some() {
        return Err(odt_error(
            Some(part),
            offset,
            "ODT XML has multiple root elements",
        ));
    }
    Ok(())
}

fn append_text(
    stack: &mut [XmlNode],
    value: String,
    retained: &mut usize,
    limits: OdtLimits,
    part: &str,
    offset: u64,
) -> Result<()> {
    if value.is_empty() {
        return Ok(());
    }
    if stack.is_empty() && value.chars().all(char::is_whitespace) {
        return Ok(());
    }
    *retained = retained
        .checked_add(value.len())
        .filter(|retained| *retained <= limits.retained_text)
        .ok_or_else(|| {
            odt_error(
                Some(part),
                offset,
                "ODT XML exceeds the retained text limit",
            )
        })?;
    let parent = stack
        .last_mut()
        .ok_or_else(|| odt_error(Some(part), offset, "text appears outside the XML root"))?;
    parent.children.push(XmlChild::Text(value));
    Ok(())
}

fn encrypted_manifest_paths(manifest: Option<&XmlNode>) -> Result<HashSet<String>> {
    let mut encrypted = HashSet::new();
    let Some(manifest) = manifest else {
        return Ok(encrypted);
    };
    if !manifest.is(MANIFEST_NS, "manifest") {
        return Err(odt_error(
            Some("META-INF/manifest.xml"),
            0,
            "invalid ODT manifest root",
        ));
    }
    for entry in manifest
        .elements()
        .filter(|entry| entry.is(MANIFEST_NS, "file-entry"))
    {
        let path = entry
            .attr(Some(MANIFEST_NS), "full-path")
            .or_else(|| entry.attr(None, "full-path"));
        if entry
            .elements()
            .any(|child| child.is(MANIFEST_NS, "encryption-data"))
        {
            let path = path.ok_or_else(|| {
                odt_error(
                    Some("META-INF/manifest.xml"),
                    0,
                    "encrypted manifest entry has no full path",
                )
            })?;
            encrypted.insert(path.to_string());
        }
    }
    Ok(encrypted)
}

#[derive(Clone, Debug, Default, PartialEq)]
struct EffectiveStyle {
    font: Option<String>,
    size: Option<f64>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    color: Option<String>,
    background: Option<String>,
    vertical: Option<VerticalText>,
    alignment: Option<Alignment>,
    before: Option<Length>,
    after: Option<Length>,
    left: Option<Length>,
    right: Option<Length>,
    first: Option<Length>,
    line: Option<LineHeight>,
}

impl EffectiveStyle {
    fn overlay(&mut self, other: &Self) {
        macro_rules! replace {
            ($field:ident) => {
                if other.$field.is_some() {
                    self.$field = other.$field.clone();
                }
            };
        }
        replace!(font);
        replace!(size);
        replace!(bold);
        replace!(italic);
        replace!(underline);
        replace!(strike);
        replace!(color);
        replace!(background);
        replace!(vertical);
        replace!(alignment);
        replace!(before);
        replace!(after);
        replace!(left);
        replace!(right);
        replace!(first);
        replace!(line);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum VerticalText {
    Superscript,
    Subscript,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum LineHeight {
    Exact(f64),
    Multiple(f64),
}

#[derive(Clone, Debug)]
struct StyleDef {
    family: String,
    name: String,
    parent: Option<String>,
    values: EffectiveStyle,
}

struct Importer<'a> {
    archive: OdtArchive<'a>,
    content: XmlNode,
    styles_root: Option<XmlNode>,
    encrypted: HashSet<String>,
    styles: HashMap<(String, String), StyleDef>,
    defaults: HashMap<String, EffectiveStyle>,
    resolved: HashMap<(String, String), EffectiveStyle>,
    list_kinds: HashMap<String, Vec<ListMarker>>,
    fonts: HashMap<String, String>,
    document: Document,
    diagnostics: Vec<OdtDiagnostic>,
    diagnostic_keys: HashSet<(String, String)>,
    limits: OdtLimits,
    blocks: usize,
    runs: usize,
    rows: usize,
    cells: usize,
    projected_text: usize,
}

impl<'a> Importer<'a> {
    fn new(
        archive: OdtArchive<'a>,
        content: XmlNode,
        styles_root: Option<XmlNode>,
        encrypted: HashSet<String>,
        limits: OdtLimits,
    ) -> Self {
        Self {
            archive,
            content,
            styles_root,
            encrypted,
            styles: HashMap::new(),
            defaults: HashMap::new(),
            resolved: HashMap::new(),
            list_kinds: HashMap::new(),
            fonts: HashMap::new(),
            document: Document::new(),
            diagnostics: Vec::new(),
            diagnostic_keys: HashSet::new(),
            limits,
            blocks: 0,
            runs: 0,
            rows: 0,
            cells: 0,
            projected_text: 0,
        }
    }

    fn project(mut self) -> Result<OdtReadResult> {
        if !self.content.is(OFFICE_NS, "document-content") {
            return Err(odt_error(
                Some("content.xml"),
                0,
                "invalid ODT content root",
            ));
        }
        if let Some(styles) = self.styles_root.clone() {
            if !styles.is(OFFICE_NS, "document-styles") {
                return Err(odt_error(Some("styles.xml"), 0, "invalid ODT styles root"));
            }
            self.collect_style_document(&styles, "styles.xml")?;
        }
        let content = self.content.clone();
        self.collect_style_document(&content, "content.xml")?;
        self.resolve_all_styles()?;

        let body = content
            .elements()
            .find(|node| node.is(OFFICE_NS, "body"))
            .and_then(|node| node.elements().find(|node| node.is(OFFICE_NS, "text")))
            .ok_or_else(|| odt_error(Some("content.xml"), 0, "ODT text body is missing"))?;
        self.project_container(body, "office:body/office:text", None)?;

        let bytes = self.document.to_bytes()?;
        let document = Document::from_bytes(&bytes)?;
        Ok(OdtReadResult {
            document,
            diagnostics: self.diagnostics,
        })
    }

    fn collect_style_document(&mut self, root: &XmlNode, part: &str) -> Result<()> {
        self.collect_fonts(root);
        self.walk_style_containers(root, part)
    }

    fn collect_fonts(&mut self, node: &XmlNode) {
        if node.is(STYLE_NS, "font-face")
            && let Some(name) = node.attr(Some(STYLE_NS), "name")
            && let Some(family) = node.attr(Some(SVG_NS), "font-family")
        {
            self.fonts.insert(
                name.to_string(),
                family.trim_matches(['\'', '"']).to_string(),
            );
        }
        for child in node.elements() {
            self.collect_fonts(child);
        }
    }

    fn walk_style_containers(&mut self, node: &XmlNode, part: &str) -> Result<()> {
        if node.is(STYLE_NS, "default-style") {
            let family = required_attr(node, STYLE_NS, "family", part)?;
            let values =
                self.parse_style_values(node, part, &format!("style:default-style[{family}]"))?;
            self.defaults
                .entry(family.to_string())
                .or_default()
                .overlay(&values);
        } else if node.is(STYLE_NS, "style") {
            let family = required_attr(node, STYLE_NS, "family", part)?;
            let name = required_attr(node, STYLE_NS, "name", part)?;
            let values = self.parse_style_values(node, part, &format!("style:style[{name}]"))?;
            let def = StyleDef {
                family: family.to_string(),
                name: name.to_string(),
                parent: node
                    .attr(Some(STYLE_NS), "parent-style-name")
                    .map(str::to_string),
                values,
            };
            let key = (def.family.clone(), def.name.clone());
            if self.styles.insert(key, def).is_some() {
                return Err(odt_error(Some(part), 0, "duplicate ODT style definition"));
            }
        } else if node.is(TEXT_NS, "list-style") {
            let name = required_attr(node, STYLE_NS, "name", part)?.to_string();
            let mut levels = vec![ListMarker::Number; 9];
            for level in node.elements() {
                let index = level
                    .attr(Some(TEXT_NS), "level")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(1)
                    .saturating_sub(1)
                    .min(8);
                if level.is(TEXT_NS, "list-level-style-bullet") {
                    levels[index] = ListMarker::Bullet;
                } else if level.is(TEXT_NS, "list-level-style-number") {
                    // An empty number format draws nothing (ODF 19.500).
                    levels[index] = if level.attr(Some(STYLE_NS), "num-format") == Some("") {
                        ListMarker::Unnumbered
                    } else {
                        ListMarker::Number
                    };
                }
            }
            self.list_kinds.insert(name, levels);
        }
        for child in node.elements() {
            self.walk_style_containers(child, part)?;
        }
        Ok(())
    }

    fn parse_style_values(
        &mut self,
        style: &XmlNode,
        part: &str,
        path: &str,
    ) -> Result<EffectiveStyle> {
        let mut values = EffectiveStyle::default();
        for properties in style.elements() {
            if properties.is(STYLE_NS, "text-properties") {
                for attribute in &properties.attributes {
                    let result = parse_text_property(attribute, &self.fonts);
                    match result {
                        Some(Ok(change)) => change.apply(&mut values),
                        Some(Err(message)) => self.diagnostic(path, message)?,
                        None if !is_ignorable_style_attribute(attribute) => self.diagnostic(
                            path,
                            format!("unsupported text property {}", attribute.name.local),
                        )?,
                        None => {}
                    }
                }
            } else if properties.is(STYLE_NS, "paragraph-properties") {
                for attribute in &properties.attributes {
                    let result = parse_paragraph_property(attribute);
                    match result {
                        Some(Ok(change)) => change.apply(&mut values),
                        Some(Err(message)) => self.diagnostic(path, message)?,
                        None if !is_ignorable_style_attribute(attribute) => self.diagnostic(
                            path,
                            format!("unsupported paragraph property {}", attribute.name.local),
                        )?,
                        None => {}
                    }
                }
            }
        }
        let _ = part;
        Ok(values)
    }

    fn resolve_all_styles(&mut self) -> Result<()> {
        let keys: Vec<(String, String)> = self.styles.keys().cloned().collect();
        for key in keys {
            self.resolve_style(&key.0, &key.1, &mut Vec::new())?;
        }
        Ok(())
    }

    fn resolve_style(
        &mut self,
        family: &str,
        name: &str,
        visiting: &mut Vec<(String, String)>,
    ) -> Result<EffectiveStyle> {
        let key = (family.to_string(), name.to_string());
        if let Some(value) = self.resolved.get(&key) {
            return Ok(value.clone());
        }
        if visiting.contains(&key) {
            return Err(odt_error(
                Some("styles.xml"),
                0,
                format!("ODT style inheritance cycle at {family}/{name}"),
            ));
        }
        let Some(definition) = self.styles.get(&key).cloned() else {
            self.diagnostic(
                &format!("style:{family}/{name}"),
                format!("missing ODT style {name}"),
            )?;
            return Ok(self.defaults.get(family).cloned().unwrap_or_default());
        };
        visiting.push(key.clone());
        let mut value = if let Some(parent) = definition.parent.as_deref() {
            self.resolve_style(family, parent, visiting)?
        } else {
            self.defaults.get(family).cloned().unwrap_or_default()
        };
        visiting.pop();
        value.overlay(&definition.values);
        self.resolved.insert(key, value.clone());
        Ok(value)
    }

    fn effective_style(&mut self, family: &str, name: Option<&str>) -> Result<EffectiveStyle> {
        match name {
            Some(name) => self.resolve_style(family, name, &mut Vec::new()),
            None => Ok(self.defaults.get(family).cloned().unwrap_or_default()),
        }
    }

    fn project_container(
        &mut self,
        container: &XmlNode,
        path: &str,
        list: Option<(u32, usize)>,
    ) -> Result<()> {
        let mut positions: HashMap<(Option<String>, String), usize> = HashMap::new();
        for child in container.elements() {
            let key = (child.name.namespace.clone(), child.name.local.clone());
            let index = positions.entry(key).or_default();
            *index += 1;
            let child_path = format!("{path}/{}[{}]", display_name(child), *index);
            if child.is(TEXT_NS, "p") || child.is(TEXT_NS, "h") {
                self.project_paragraph(child, &child_path, list)?;
            } else if child.is(TEXT_NS, "list") {
                self.project_list(child, &child_path, list.map_or(0, |(_, level)| level + 1))?;
            } else if child.is(TABLE_NS, "table") {
                self.project_table(child, &child_path)?;
            } else if child.is(TEXT_NS, "section") || child.is(OFFICE_NS, "text") {
                self.project_container(child, &child_path, list)?;
            } else if child.is(DRAW_NS, "frame") {
                let paragraph = XmlNode {
                    name: XmlName {
                        namespace: Some(TEXT_NS.to_string()),
                        local: "p".to_string(),
                    },
                    attributes: Vec::new(),
                    children: vec![XmlChild::Element(child.clone())],
                };
                self.project_paragraph(&paragraph, &child_path, list)?;
            } else {
                self.diagnostic(
                    &child_path,
                    format!("unsupported ODT subtree {}", display_name(child)),
                )?;
            }
        }
        Ok(())
    }

    fn project_list(&mut self, list: &XmlNode, path: &str, level: usize) -> Result<()> {
        let mut paragraphs = Vec::new();
        self.collect_list_paragraphs(list, path, level, &mut paragraphs)?;
        self.document.document.body.content.extend(
            paragraphs
                .into_iter()
                .map(|paragraph| BodyContent::Paragraph(Box::new(paragraph))),
        );
        Ok(())
    }

    fn collect_list_paragraphs(
        &mut self,
        list: &XmlNode,
        path: &str,
        level: usize,
        output: &mut Vec<CT_P>,
    ) -> Result<()> {
        if level >= 9 {
            return Err(odt_error(
                Some("content.xml"),
                0,
                "ODT list exceeds nine levels",
            ));
        }
        if list.attr(Some(TEXT_NS), "continue-numbering").is_some()
            || list.attr(Some(TEXT_NS), "continue-list").is_some()
        {
            self.diagnostic(path, "unsupported ODT list continuation semantics")?;
        }
        let style_name = list.attr(Some(TEXT_NS), "style-name");
        let levels = style_name
            .and_then(|name| self.list_kinds.get(name))
            .cloned()
            .unwrap_or_else(|| vec![ListMarker::Bullet; 9]);
        if style_name.is_some_and(|name| !self.list_kinds.contains_key(name)) {
            self.diagnostic(path, "missing ODT list style, using bullets")?;
        }
        let definitions: Vec<ListLevel> = levels
            .iter()
            .map(|marker| match marker {
                ListMarker::Bullet => ListLevel::bullet(),
                ListMarker::Number => ListLevel::decimal(),
                ListMarker::Unnumbered => ListLevel::new(ListNumberFormat::None),
            })
            .collect();
        let num_id = self.document.add_list_definition(&definitions);
        for (item_index, item) in list
            .elements()
            .filter(|item| item.is(TEXT_NS, "list-item") || item.is(TEXT_NS, "list-header"))
            .enumerate()
        {
            let item_name = if item.is(TEXT_NS, "list-header") {
                "list-header"
            } else {
                "list-item"
            };
            let item_path = format!("{path}/text:{item_name}[{}]", item_index + 1);
            if item.attr(Some(TEXT_NS), "start-value").is_some() {
                self.diagnostic(&item_path, "unsupported ODT list start value")?;
            }
            let numbering = item.is(TEXT_NS, "list-item").then_some((num_id, level));
            self.collect_list_item_paragraphs(item, &item_path, numbering, level, output)?;
        }
        Ok(())
    }

    fn collect_list_item_paragraphs(
        &mut self,
        container: &XmlNode,
        path: &str,
        numbering: Option<(u32, usize)>,
        level: usize,
        output: &mut Vec<CT_P>,
    ) -> Result<()> {
        let mut positions: HashMap<(Option<String>, String), usize> = HashMap::new();
        for child in container.elements() {
            let key = (child.name.namespace.clone(), child.name.local.clone());
            let child_index = positions.entry(key).or_default();
            *child_index += 1;
            let child_path = format!("{path}/{}[{}]", display_name(child), *child_index);
            if child.is(TEXT_NS, "p") || child.is(TEXT_NS, "h") {
                output.push(self.build_paragraph(child, &child_path, numbering)?);
            } else if child.is(TEXT_NS, "list") {
                self.collect_list_paragraphs(child, &child_path, level + 1, output)?;
            } else if child.is(TEXT_NS, "section") || child.is(OFFICE_NS, "text") {
                self.collect_list_item_paragraphs(child, &child_path, numbering, level, output)?;
            } else if child.is(DRAW_NS, "frame") {
                let paragraph = XmlNode {
                    name: XmlName {
                        namespace: Some(TEXT_NS.to_string()),
                        local: "p".to_string(),
                    },
                    attributes: Vec::new(),
                    children: vec![XmlChild::Element(child.clone())],
                };
                output.push(self.build_paragraph(&paragraph, &child_path, numbering)?);
            } else {
                self.diagnostic(
                    &child_path,
                    format!("unsupported ODT list subtree {}", display_name(child)),
                )?;
            }
        }
        Ok(())
    }

    fn project_paragraph(
        &mut self,
        node: &XmlNode,
        path: &str,
        list: Option<(u32, usize)>,
    ) -> Result<()> {
        let paragraph = self.build_paragraph(node, path, list)?;
        self.document
            .document
            .body
            .content
            .push(BodyContent::Paragraph(Box::new(paragraph)));
        Ok(())
    }

    fn build_paragraph(
        &mut self,
        node: &XmlNode,
        path: &str,
        list: Option<(u32, usize)>,
    ) -> Result<CT_P> {
        self.bump_blocks()?;
        let style_name = node.attr(Some(TEXT_NS), "style-name");
        let paragraph_style = self.effective_style("paragraph", style_name)?;
        let mut pieces = Vec::new();
        self.collect_inline(node, path, &paragraph_style, &mut pieces)?;
        let mut paragraph = CT_P::new();
        {
            let mut target = Paragraph {
                inner: &mut paragraph,
            };
            apply_paragraph_style(&mut target, &paragraph_style);
            if node.is(TEXT_NS, "h") {
                let level = node
                    .attr(Some(TEXT_NS), "outline-level")
                    .and_then(|value| value.parse::<u32>().ok())
                    .unwrap_or(1)
                    .clamp(1, 9);
                target.set_style(&format!("Heading{level}"));
                target.set_outline_level(level - 1);
            }
            if let Some((num_id, level)) = list {
                target.set_numbering(num_id, level as u32);
            }
            for piece in pieces {
                match piece {
                    InlinePiece::Text(text, style) => {
                        self.bump_runs()?;
                        let mut run = target.add_run(&text);
                        apply_run_style(&mut run, &style);
                    }
                    InlinePiece::Break => {
                        self.bump_runs()?;
                        target.add_line_break();
                    }
                    InlinePiece::Tab => {
                        self.bump_runs()?;
                        target.add_tab();
                    }
                    InlinePiece::Image {
                        relationship,
                        width,
                        height,
                    } => {
                        self.bump_runs()?;
                        target.add_picture(&relationship, width, height);
                    }
                }
            }
        }
        Ok(paragraph)
    }

    fn collect_inline(
        &mut self,
        node: &XmlNode,
        path: &str,
        inherited: &EffectiveStyle,
        output: &mut Vec<InlinePiece>,
    ) -> Result<()> {
        let mut whitespace = InlineWhitespace::default();
        self.collect_inline_inner(node, path, inherited, output, &mut whitespace)
    }

    fn collect_inline_inner(
        &mut self,
        node: &XmlNode,
        path: &str,
        inherited: &EffectiveStyle,
        output: &mut Vec<InlinePiece>,
        whitespace: &mut InlineWhitespace,
    ) -> Result<()> {
        let mut element_positions: HashMap<(Option<String>, String), usize> = HashMap::new();
        for child in &node.children {
            match child {
                XmlChild::Text(text) => {
                    self.bump_projected_text(text.len())?;
                    collapse_odt_text(text, inherited, whitespace, output);
                }
                XmlChild::Element(element) if element.is(TEXT_NS, "span") => {
                    let key = (element.name.namespace.clone(), element.name.local.clone());
                    let index = element_positions.entry(key).or_default();
                    *index += 1;
                    let child_path = format!("{path}/text:span[{}]", *index);
                    let mut style = inherited.clone();
                    let own =
                        self.effective_style("text", element.attr(Some(TEXT_NS), "style-name"))?;
                    style.overlay(&own);
                    self.collect_inline_inner(element, &child_path, &style, output, whitespace)?;
                }
                XmlChild::Element(element) if element.is(TEXT_NS, "s") => {
                    let count = repeated_count(element, TEXT_NS, "c", self.limits.retained_text)?;
                    self.bump_projected_text(count)?;
                    push_inline_text(output, " ".repeat(count), inherited.clone());
                    whitespace.pending = false;
                    whitespace.style = None;
                    whitespace.emitted = true;
                }
                XmlChild::Element(element) if element.is(TEXT_NS, "tab") => {
                    output.push(InlinePiece::Tab);
                    whitespace.pending = false;
                    whitespace.style = None;
                    whitespace.emitted = true;
                }
                XmlChild::Element(element) if element.is(TEXT_NS, "line-break") => {
                    output.push(InlinePiece::Break);
                    whitespace.pending = false;
                    whitespace.style = None;
                    whitespace.emitted = true;
                }
                XmlChild::Element(element) if element.is(DRAW_NS, "frame") => {
                    let key = (element.name.namespace.clone(), element.name.local.clone());
                    let index = element_positions.entry(key).or_default();
                    *index += 1;
                    let child_path = format!("{path}/draw:frame[{}]", *index);
                    if let Some(image) = self.project_image(element, &child_path)? {
                        output.push(image);
                        whitespace.pending = false;
                        whitespace.style = None;
                        whitespace.emitted = true;
                    }
                }
                XmlChild::Element(element) => {
                    let key = (element.name.namespace.clone(), element.name.local.clone());
                    let index = element_positions.entry(key).or_default();
                    *index += 1;
                    let child_path = format!("{path}/{}[{}]", display_name(element), *index);
                    self.diagnostic(
                        &child_path,
                        format!("unsupported ODT inline subtree {}", display_name(element)),
                    )?;
                }
            }
        }
        Ok(())
    }

    fn project_image(&mut self, frame: &XmlNode, path: &str) -> Result<Option<InlinePiece>> {
        let Some(image) = frame.elements().find(|node| node.is(DRAW_NS, "image")) else {
            self.diagnostic(path, "drawing frame has no supported image")?;
            return Ok(None);
        };
        let Some(href) = image.attr(Some(XLINK_NS), "href") else {
            self.diagnostic(path, "ODT image has no package target")?;
            return Ok(None);
        };
        let target = safe_image_target(href).ok_or_else(|| {
            odt_error(
                Some("content.xml"),
                0,
                format!("unsafe ODT image target {href}"),
            )
        })?;
        if self.encrypted.contains(&target) {
            return Err(odt_error(
                Some(&target),
                0,
                "referenced ODT image is encrypted",
            ));
        }
        let Some(bytes) = self.archive.read_optional(&target)? else {
            self.diagnostic(path, format!("missing ODT image target {target}"))?;
            return Ok(None);
        };
        let Some(info) = oxoffice_media::probe(&bytes) else {
            self.diagnostic(path, format!("unsupported ODT image content {target}"))?;
            return Ok(None);
        };
        let explicit = match (
            frame.attr(Some(SVG_NS), "width"),
            frame.attr(Some(SVG_NS), "height"),
        ) {
            (Some(width), Some(height)) => Some((
                parse_positive_length(width)
                    .map_err(|message| odt_error(Some("content.xml"), 0, message))?,
                parse_positive_length(height)
                    .map_err(|message| odt_error(Some("content.xml"), 0, message))?,
            )),
            (None, None) => None,
            _ => {
                self.diagnostic(
                    path,
                    "ODT image frame has only one dimension, using intrinsic size",
                )?;
                None
            }
        };
        let (width, height) = explicit
            .or_else(|| {
                info.native_size(72.0)
                    .map(|size| (Length::emu(size.width_emu), Length::emu(size.height_emu)))
            })
            .ok_or_else(|| odt_error(Some(&target), 0, "ODT image dimensions are unavailable"))?;
        let relationship = self.document.embed_image(&bytes, &target);
        Ok(Some(InlinePiece::Image {
            relationship,
            width,
            height,
        }))
    }

    fn project_table(&mut self, table: &XmlNode, path: &str) -> Result<()> {
        self.bump_blocks()?;
        let mut column_hint = 0_usize;
        for column in table
            .elements()
            .filter(|node| node.is(TABLE_NS, "table-column"))
        {
            column_hint = column_hint
                .checked_add(repeated_count(
                    column,
                    TABLE_NS,
                    "number-columns-repeated",
                    self.limits.columns,
                )?)
                .ok_or_else(|| odt_error(Some("content.xml"), 0, "ODT column count overflowed"))?;
        }
        let mut source_rows = Vec::new();
        collect_table_rows(table, &mut source_rows);
        let mut rows = Vec::new();
        for (row_index, row) in source_rows.into_iter().enumerate() {
            let repeated = repeated_count(row, TABLE_NS, "number-rows-repeated", self.limits.rows)?;
            for repeat in 0..repeated {
                self.rows = self
                    .rows
                    .checked_add(1)
                    .filter(|value| *value <= self.limits.rows)
                    .ok_or_else(|| {
                        odt_error(Some("content.xml"), 0, "ODT table exceeds the row limit")
                    })?;
                rows.push(self.parse_table_row(
                    row,
                    &format!("{path}/table:table-row[{}]", row_index + repeat + 1),
                )?);
            }
        }
        let columns = rows
            .iter()
            .map(|row| row.iter().map(|cell| cell.colspan).sum::<usize>())
            .max()
            .unwrap_or(column_hint)
            .max(column_hint);
        if columns == 0 || columns > self.limits.columns {
            return Err(odt_error(
                Some("content.xml"),
                0,
                "ODT table column count is invalid",
            ));
        }
        let mut tbl = CT_Tbl::new();
        let width = Twips(9000 / columns as i32);
        tbl.properties = Some(CT_TblPr {
            width: Some(CT_TblWidth::dxa(width.0 * columns as i32)),
            ..Default::default()
        });
        tbl.grid = Some(CT_TblGrid {
            columns: (0..columns).map(|_| CT_TblGridCol { width }).collect(),
            ..Default::default()
        });
        let mut active: Vec<Option<(usize, usize)>> = vec![None; columns];
        for row in rows {
            let mut target = CT_Row::new();
            let mut column = 0_usize;
            let mut source = row.into_iter();
            while column < columns {
                if let Some((remaining, span)) = active[column] {
                    let mut cell = CT_Tc::new();
                    cell.properties = Some(CT_TcPr {
                        grid_span: (span > 1).then_some(span as u32),
                        v_merge: Some(VMerge::Continue),
                        ..Default::default()
                    });
                    target.cells.push(cell);
                    for slot in active.iter_mut().skip(column).take(span) {
                        *slot = if remaining > 1 {
                            Some((remaining - 1, span))
                        } else {
                            None
                        };
                    }
                    column += span;
                    continue;
                }
                let Some(model) = source.next() else {
                    target.cells.push(CT_Tc::new());
                    column += 1;
                    continue;
                };
                if column + model.colspan > columns {
                    return Err(odt_error(
                        Some("content.xml"),
                        0,
                        "ODT table span exceeds the grid",
                    ));
                }
                let mut cell = CT_Tc::new();
                cell.properties = Some(CT_TcPr {
                    grid_span: (model.colspan > 1).then_some(model.colspan as u32),
                    v_merge: (model.rowspan > 1).then_some(VMerge::Restart),
                    ..Default::default()
                });
                cell.content.clear();
                for paragraph in model.paragraphs {
                    cell.content.push(CellContent::Paragraph(paragraph));
                }
                if cell.paragraphs().is_empty() {
                    cell.content.push(CellContent::Paragraph(CT_P::new()));
                }
                if model.rowspan > 1 {
                    for slot in active.iter_mut().skip(column).take(model.colspan) {
                        if slot.is_some() {
                            return Err(odt_error(
                                Some("content.xml"),
                                0,
                                "overlapping ODT table spans",
                            ));
                        }
                        *slot = Some((model.rowspan - 1, model.colspan));
                    }
                }
                target.cells.push(cell);
                column += model.colspan;
            }
            if source.next().is_some() {
                return Err(odt_error(
                    Some("content.xml"),
                    0,
                    "ODT table row exceeds the grid",
                ));
            }
            tbl.rows.push(target);
        }
        self.document
            .document
            .body
            .content
            .push(BodyContent::Table(Box::new(tbl)));
        Ok(())
    }

    fn parse_table_row(&mut self, row: &XmlNode, path: &str) -> Result<Vec<TableCellModel>> {
        let mut output = Vec::new();
        for (index, cell) in row
            .elements()
            .filter(|node| {
                node.is(TABLE_NS, "table-cell") || node.is(TABLE_NS, "covered-table-cell")
            })
            .enumerate()
        {
            let repeat = repeated_count(
                cell,
                TABLE_NS,
                "number-columns-repeated",
                self.limits.columns,
            )?;
            for repeat_index in 0..repeat {
                self.cells = self
                    .cells
                    .checked_add(1)
                    .filter(|value| *value <= self.limits.cells)
                    .ok_or_else(|| {
                        odt_error(Some("content.xml"), 0, "ODT table exceeds the cell limit")
                    })?;
                if cell.is(TABLE_NS, "covered-table-cell") {
                    continue;
                }
                let colspan = repeated_count(
                    cell,
                    TABLE_NS,
                    "number-columns-spanned",
                    self.limits.columns,
                )?;
                let rowspan =
                    repeated_count(cell, TABLE_NS, "number-rows-spanned", self.limits.rows)?;
                let mut paragraphs = Vec::new();
                let cell_path = format!("{path}/table:table-cell[{}]", index + repeat_index + 1);
                let mut positions: HashMap<(Option<String>, String), usize> = HashMap::new();
                for child in cell.elements() {
                    let key = (child.name.namespace.clone(), child.name.local.clone());
                    let child_index = positions.entry(key).or_default();
                    *child_index += 1;
                    let child_path =
                        format!("{cell_path}/{}[{}]", display_name(child), *child_index);
                    if child.is(TEXT_NS, "p") || child.is(TEXT_NS, "h") {
                        paragraphs.push(self.build_paragraph(child, &child_path, None)?);
                    } else if child.is(TEXT_NS, "list") {
                        self.collect_list_paragraphs(child, &child_path, 0, &mut paragraphs)?;
                    } else {
                        self.diagnostic(
                            &child_path,
                            format!("unsupported ODT table-cell subtree {}", display_name(child)),
                        )?;
                    }
                }
                output.push(TableCellModel {
                    colspan,
                    rowspan,
                    paragraphs,
                });
            }
        }
        Ok(output)
    }

    fn diagnostic(&mut self, path: &str, message: impl Into<String>) -> Result<()> {
        let message = message.into();
        if !self
            .diagnostic_keys
            .insert((path.to_string(), message.clone()))
        {
            return Ok(());
        }
        if self.diagnostics.len() >= self.limits.diagnostics {
            return Err(odt_error(
                Some("content.xml"),
                0,
                "ODT exceeds the diagnostic limit",
            ));
        }
        self.diagnostics.push(OdtDiagnostic {
            path: path.to_string(),
            message,
        });
        Ok(())
    }

    fn bump_blocks(&mut self) -> Result<()> {
        self.blocks = self
            .blocks
            .checked_add(1)
            .filter(|value| *value <= self.limits.blocks)
            .ok_or_else(|| {
                odt_error(
                    Some("content.xml"),
                    0,
                    "ODT exceeds the projected block limit",
                )
            })?;
        Ok(())
    }

    fn bump_runs(&mut self) -> Result<()> {
        self.runs = self
            .runs
            .checked_add(1)
            .filter(|value| *value <= self.limits.runs)
            .ok_or_else(|| {
                odt_error(
                    Some("content.xml"),
                    0,
                    "ODT exceeds the projected run limit",
                )
            })?;
        Ok(())
    }

    fn bump_projected_text(&mut self, amount: usize) -> Result<()> {
        self.projected_text = self
            .projected_text
            .checked_add(amount)
            .filter(|value| *value <= self.limits.retained_text)
            .ok_or_else(|| {
                odt_error(
                    Some("content.xml"),
                    0,
                    "ODT exceeds the projected text limit",
                )
            })?;
        Ok(())
    }
}

#[derive(Clone)]
enum InlinePiece {
    Text(String, EffectiveStyle),
    Break,
    Tab,
    Image {
        relationship: String,
        width: Length,
        height: Length,
    },
}

#[derive(Default)]
struct InlineWhitespace {
    pending: bool,
    emitted: bool,
    style: Option<EffectiveStyle>,
}

fn collapse_odt_text(
    text: &str,
    style: &EffectiveStyle,
    whitespace: &mut InlineWhitespace,
    output: &mut Vec<InlinePiece>,
) {
    let mut visible = String::new();
    for character in text.chars() {
        if character.is_whitespace() {
            if !visible.is_empty() {
                push_inline_text(output, std::mem::take(&mut visible), style.clone());
            }
            whitespace.pending = true;
            whitespace.style.get_or_insert_with(|| style.clone());
        } else {
            if whitespace.pending && whitespace.emitted {
                push_inline_text(
                    output,
                    " ".to_string(),
                    whitespace.style.take().unwrap_or_else(|| style.clone()),
                );
            }
            visible.push(character);
            whitespace.pending = false;
            whitespace.style = None;
            whitespace.emitted = true;
        }
    }
    if !visible.is_empty() {
        push_inline_text(output, visible, style.clone());
    }
}

fn push_inline_text(output: &mut Vec<InlinePiece>, text: String, style: EffectiveStyle) {
    if let Some(InlinePiece::Text(previous, previous_style)) = output.last_mut()
        && *previous_style == style
    {
        previous.push_str(&text);
    } else {
        output.push(InlinePiece::Text(text, style));
    }
}

struct TableCellModel {
    colspan: usize,
    rowspan: usize,
    paragraphs: Vec<CT_P>,
}

#[derive(Clone)]
enum StyleChange {
    Font(String),
    Size(f64),
    Bold(bool),
    Italic(bool),
    Underline(bool),
    Strike(bool),
    Color(String),
    Background(String),
    Vertical(VerticalText),
    Alignment(Alignment),
    Before(Length),
    After(Length),
    Left(Length),
    Right(Length),
    First(Length),
    Line(LineHeight),
}

impl StyleChange {
    fn apply(self, style: &mut EffectiveStyle) {
        match self {
            Self::Font(value) => style.font = Some(value),
            Self::Size(value) => style.size = Some(value),
            Self::Bold(value) => style.bold = Some(value),
            Self::Italic(value) => style.italic = Some(value),
            Self::Underline(value) => style.underline = Some(value),
            Self::Strike(value) => style.strike = Some(value),
            Self::Color(value) => style.color = Some(value),
            Self::Background(value) => style.background = Some(value),
            Self::Vertical(value) => style.vertical = Some(value),
            Self::Alignment(value) => style.alignment = Some(value),
            Self::Before(value) => style.before = Some(value),
            Self::After(value) => style.after = Some(value),
            Self::Left(value) => style.left = Some(value),
            Self::Right(value) => style.right = Some(value),
            Self::First(value) => style.first = Some(value),
            Self::Line(value) => style.line = Some(value),
        }
    }
}

fn parse_text_property(
    attribute: &XmlAttribute,
    fonts: &HashMap<String, String>,
) -> Option<std::result::Result<StyleChange, String>> {
    let namespace = attribute.name.namespace.as_deref();
    let name = attribute.name.local.as_str();
    let value = attribute.value.trim();
    let lower = value.to_ascii_lowercase();
    let result = match (namespace, name) {
        (Some(STYLE_NS), "font-name") => Ok(StyleChange::Font(
            fonts
                .get(value)
                .cloned()
                .unwrap_or_else(|| value.to_string()),
        )),
        (Some(FO_NS), "font-family") => Ok(StyleChange::Font(
            value.trim_matches(['\'', '"']).to_string(),
        )),
        (Some(FO_NS), "font-size") => {
            parse_positive_length(value).map(|length| StyleChange::Size(length.to_pt()))
        }
        (Some(FO_NS), "font-weight") => match lower.as_str() {
            "bold" | "600" | "700" | "800" | "900" => Ok(StyleChange::Bold(true)),
            "normal" | "400" | "500" => Ok(StyleChange::Bold(false)),
            _ => Err(format!("unsupported font weight {value}")),
        },
        (Some(FO_NS), "font-style") => match lower.as_str() {
            "italic" | "oblique" => Ok(StyleChange::Italic(true)),
            "normal" => Ok(StyleChange::Italic(false)),
            _ => Err(format!("unsupported font style {value}")),
        },
        (Some(STYLE_NS), "text-underline-style") => Ok(StyleChange::Underline(lower != "none")),
        (Some(STYLE_NS), "text-line-through-style") => Ok(StyleChange::Strike(lower != "none")),
        (Some(FO_NS), "color") => parse_color(value).map(StyleChange::Color),
        (Some(FO_NS), "background-color") => parse_color(value).map(StyleChange::Background),
        (Some(STYLE_NS), "text-position") => {
            if lower.starts_with("super") {
                Ok(StyleChange::Vertical(VerticalText::Superscript))
            } else if lower.starts_with("sub") {
                Ok(StyleChange::Vertical(VerticalText::Subscript))
            } else {
                Err(format!("unsupported text position {value}"))
            }
        }
        _ => return None,
    };
    Some(result)
}

fn parse_paragraph_property(
    attribute: &XmlAttribute,
) -> Option<std::result::Result<StyleChange, String>> {
    let namespace = attribute.name.namespace.as_deref();
    let name = attribute.name.local.as_str();
    let value = attribute.value.trim();
    let lower = value.to_ascii_lowercase();
    let result = match (namespace, name) {
        (Some(FO_NS), "text-align") => match lower.as_str() {
            "start" | "left" => Ok(StyleChange::Alignment(Alignment::Left)),
            "center" => Ok(StyleChange::Alignment(Alignment::Center)),
            "end" | "right" => Ok(StyleChange::Alignment(Alignment::Right)),
            "justify" => Ok(StyleChange::Alignment(Alignment::Justify)),
            _ => Err(format!("unsupported paragraph alignment {value}")),
        },
        (Some(FO_NS), "margin-top") => parse_length(value, false).map(StyleChange::Before),
        (Some(FO_NS), "margin-bottom") => parse_length(value, false).map(StyleChange::After),
        (Some(FO_NS), "margin-left") => parse_length(value, false).map(StyleChange::Left),
        (Some(FO_NS), "margin-right") => parse_length(value, false).map(StyleChange::Right),
        (Some(FO_NS), "text-indent") => parse_length(value, true).map(StyleChange::First),
        (Some(FO_NS), "line-height") if lower.ends_with('%') => {
            match lower.trim_end_matches('%').parse::<f64>() {
                Ok(percentage)
                    if percentage.is_finite() && percentage > 0.0 && percentage <= 10_000.0 =>
                {
                    Ok(StyleChange::Line(LineHeight::Multiple(percentage / 100.0)))
                }
                Ok(_) => Err(format!(
                    "percentage line height is outside the supported range {value}"
                )),
                Err(_) => Err(format!("invalid percentage line height {value}")),
            }
        }
        (Some(FO_NS), "line-height") => parse_positive_length(value)
            .map(|length| StyleChange::Line(LineHeight::Exact(length.to_pt()))),
        _ => return None,
    };
    Some(result)
}

fn is_ignorable_style_attribute(attribute: &XmlAttribute) -> bool {
    matches!(
        (
            attribute.name.namespace.as_deref(),
            attribute.name.local.as_str()
        ),
        (Some(STYLE_NS), "font-name-asian" | "font-name-complex")
            | (Some(STYLE_NS), "font-size-asian" | "font-size-complex")
            | (Some(STYLE_NS), "font-weight-asian" | "font-weight-complex")
            | (Some(STYLE_NS), "font-style-asian" | "font-style-complex")
            | (Some(STYLE_NS), "writing-mode")
    )
}

fn apply_paragraph_style(paragraph: &mut Paragraph<'_>, style: &EffectiveStyle) {
    if let Some(value) = style.alignment {
        paragraph.set_alignment(value);
    }
    if let Some(value) = style.before {
        paragraph.set_space_before(value);
    }
    if let Some(value) = style.after {
        paragraph.set_space_after(value);
    }
    if let Some(value) = style.left {
        paragraph.set_indent_left(value);
    }
    if let Some(value) = style.right {
        paragraph.set_indent_right(value);
    }
    if let Some(value) = style.first {
        paragraph.set_signed_first_line_indent_value(Some(value));
    }
    if let Some(value) = style.line {
        match value {
            LineHeight::Exact(points) => paragraph.set_line_spacing(points),
            LineHeight::Multiple(multiple) => paragraph.set_line_spacing_multiple(multiple),
        }
    }
}

fn apply_run_style(run: &mut Run<'_>, style: &EffectiveStyle) {
    if let Some(value) = &style.font {
        run.set_font(value);
    }
    if let Some(value) = style.size {
        run.set_size(value);
    }
    if let Some(value) = style.bold {
        run.set_bold(value);
    }
    if let Some(value) = style.italic {
        run.set_italic(value);
    }
    if let Some(value) = style.underline {
        run.set_underline(value);
    }
    if let Some(value) = style.strike {
        run.set_strike(value);
    }
    if let Some(value) = &style.color {
        run.set_color(value);
    }
    if let Some(value) = &style.background {
        run.set_highlight(value);
    }
    match style.vertical {
        Some(VerticalText::Superscript) => run.set_superscript(),
        Some(VerticalText::Subscript) => run.set_subscript(),
        None => {}
    }
}

fn parse_length(value: &str, signed: bool) -> std::result::Result<Length, String> {
    let value = value.trim().to_ascii_lowercase();
    let units = [
        ("cm", 72.0 / 2.54),
        ("mm", 72.0 / 25.4),
        ("in", 72.0),
        ("pt", 1.0),
        ("pc", 12.0),
        ("px", 0.75),
    ];
    let (number, factor) = units
        .iter()
        .find_map(|(unit, factor)| value.strip_suffix(unit).map(|number| (number, *factor)))
        .ok_or_else(|| format!("unsupported ODT length {value}"))?;
    let points = number
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("invalid ODT length {value}"))?
        * factor;
    if !points.is_finite() || points.abs() > 1_000_000.0 || (!signed && points < 0.0) {
        return Err(format!("ODT length is outside the supported range {value}"));
    }
    Ok(Length::pt(points))
}

fn parse_positive_length(value: &str) -> std::result::Result<Length, String> {
    let length = parse_length(value, false)?;
    if length.to_emu() <= 0 {
        return Err(format!("ODT length must be positive {value}"));
    }
    Ok(length)
}

fn parse_color(value: &str) -> std::result::Result<String, String> {
    let value = value.trim();
    if value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        Ok(value[1..].to_ascii_uppercase())
    } else {
        Err(format!("unsupported ODT color {value}"))
    }
}

fn repeated_count(node: &XmlNode, namespace: &str, local: &str, maximum: usize) -> Result<usize> {
    let Some(value) = node.attr(Some(namespace), local) else {
        return Ok(1);
    };
    let value = value.parse::<usize>().map_err(|_| {
        odt_error(
            Some("content.xml"),
            0,
            format!("invalid ODT repeat or span value {value}"),
        )
    })?;
    if value == 0 || value > maximum {
        return Err(odt_error(
            Some("content.xml"),
            0,
            "ODT repeat or span exceeds its bound",
        ));
    }
    Ok(value)
}

fn collect_table_rows<'a>(node: &'a XmlNode, rows: &mut Vec<&'a XmlNode>) {
    for child in node.elements() {
        if child.is(TABLE_NS, "table-row") {
            rows.push(child);
        } else if child.is(TABLE_NS, "table-rows") || child.is(TABLE_NS, "table-header-rows") {
            collect_table_rows(child, rows);
        }
    }
}

fn safe_image_target(href: &str) -> Option<String> {
    if href.is_empty()
        || href.starts_with('/')
        || href.starts_with('#')
        || href.contains(':')
        || href.contains('\\')
        || href.contains('\0')
        || href
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        None
    } else {
        Some(href.to_string())
    }
}

fn required_attr<'a>(
    node: &'a XmlNode,
    namespace: &str,
    local: &str,
    part: &str,
) -> Result<&'a str> {
    node.attr(Some(namespace), local).ok_or_else(|| {
        odt_error(
            Some(part),
            0,
            format!("{} requires {local}", display_name(node)),
        )
    })
}

fn display_name(node: &XmlNode) -> String {
    let prefix = match node.name.namespace.as_deref() {
        Some(OFFICE_NS) => "office",
        Some(TEXT_NS) => "text",
        Some(STYLE_NS) => "style",
        Some(TABLE_NS) => "table",
        Some(DRAW_NS) => "draw",
        Some(MANIFEST_NS) => "manifest",
        Some(_) => "foreign",
        None => "unbound",
    };
    format!("{prefix}:{}", node.name.local)
}

fn odt_error(part: Option<&str>, offset: u64, message: impl Into<String>) -> Error {
    Error::Odt {
        part: part.map(str::to_string),
        offset,
        message: message.into(),
    }
}
