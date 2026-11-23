//! Deterministic EPUB 3 export for the native Word facade.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use oxoffice_docx_oxml::content_control::{CT_Sdt, SdtContent};
use oxoffice_docx_oxml::document::{BodyContent, CT_Document};
use oxoffice_docx_oxml::drawing::{CT_Drawing, CT_Inline};
use oxoffice_docx_oxml::numbering::{
    CT_AbstractNum, CT_Lvl, CT_Num, CT_Numbering, ST_NumberFormat,
};
use oxoffice_docx_oxml::properties::CT_PPr;
use oxoffice_docx_oxml::revision::CT_Revision;
use oxoffice_docx_oxml::styles::{CT_Style, CT_Styles, StyleType};
use oxoffice_docx_oxml::table::{CT_Row, CT_Tbl, CT_Tc, CellContent};
use oxoffice_docx_oxml::text::{BreakClear, CT_P, CT_R, Field, HyperlinkSpan, RunContent};
use oxoffice_opc::relationship::rel_types;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::document::{visit_accepted_drawings, visit_body_paragraphs};
use crate::revision::{accepted_revision_content, revision_removes_text};
use crate::{Document, Error, Result};

const MAX_EPUB_BYTES: usize = 64 * 1024 * 1024;
const MAX_SOURCE_TEXT_BYTES: usize = 8 * 1024 * 1024;
const MAX_MEDIA_BYTES: usize = 16 * 1024 * 1024;
const MAX_BODY_ITEMS: usize = 100_000;
const MAX_MEDIA_ITEMS: usize = 4_096;
const MAX_DIAGNOSTICS: usize = 10_000;
const MAX_NESTING_DEPTH: usize = 64;
const MAX_PROJECTED_NODES: usize = 100_000;
const MAX_IMAGE_OCCURRENCES: usize = 4_096;
const MAX_RELATIONSHIPS: usize = 16_384;
const MAX_STYLE_ITEMS: usize = 4_096;
const MAX_NUMBERING_ITEMS: usize = 4_096;
const MAX_PROJECTION_KEY_BYTES: usize = 1024 * 1024;

const CONTAINER_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>
"#;

/// What dropping a block kept as raw XML loses: a bookmark marker between
/// blocks, or `unmodelled`.
fn raw_dropped(raw: &[u8], unmodelled: &str) -> String {
    if crate::document::is_block_bookmark_marker(raw) {
        "bookmark marker was dropped during EPUB export".to_owned()
    } else {
        unmodelled.to_owned()
    }
}

const STYLESHEET: &str = r#"body{font-family:serif;line-height:1.4;margin:5%;}img{height:auto;max-width:100%;}table{border-collapse:collapse;}td,th{padding:.25em;}nav ol{list-style:none;padding-left:1.25em;}ul.no-marker{list-style:none;}"#;

/// One stable report of source content that EPUB cannot preserve exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpubDiagnostic {
    pub path: String,
    pub message: String,
}

/// Serialized EPUB bytes together with every lossy-conversion diagnostic.
pub struct EpubWriteResult {
    pub bytes: Vec<u8>,
    pub diagnostics: Vec<EpubDiagnostic>,
}

impl Document {
    /// Serialize this document as a bounded deterministic EPUB 3 publication.
    pub fn to_epub_bytes(&self) -> Result<EpubWriteResult> {
        EpubWriter::new(self).write()
    }

    /// Serialize and atomically save EPUB, returning lossy-conversion diagnostics.
    pub fn save_epub<P: AsRef<Path>>(&self, path: P) -> Result<Vec<EpubDiagnostic>> {
        let result = self.to_epub_bytes()?;
        oxoffice_opc::write_atomic_file(
            path.as_ref(),
            &result.bytes,
            "oxoffice",
            "invalid EPUB file name",
            "could not allocate EPUB-save staging file",
        )?;
        Ok(result.diagnostics)
    }
}

#[derive(Clone)]
struct Heading {
    body_index: usize,
    level: u32,
    text: String,
    anchor: String,
    href: String,
}

struct NavNode {
    heading_index: usize,
    children: Vec<NavNode>,
}

struct SpineItem {
    id: String,
    href: String,
    title: String,
    start: usize,
    end: usize,
    xhtml: String,
}

struct MediaItem {
    relationship_id: String,
    href: String,
    media_type: String,
    data: Vec<u8>,
}

struct EpubWriter<'a> {
    document: &'a Document,
    diagnostics: Vec<EpubDiagnostic>,
}

impl<'a> EpubWriter<'a> {
    fn new(document: &'a Document) -> Self {
        Self {
            document,
            diagnostics: Vec::new(),
        }
    }

    fn write(mut self) -> Result<EpubWriteResult> {
        self.check_source_limits()?;
        self.check_relationship_limits()?;

        let styles = render_styles(&self.document.styles)?;
        let numbering = render_numbering(self.document.numbering.as_ref())?;
        let (media, html_images) = self.media_items()?;
        let mut input = oxoffice_docx_html::HtmlInput {
            document: CT_Document::new(),
            styles,
            numbering,
            images: html_images,
            hyperlink_urls: self.epub_hyperlinks(),
        };
        self.collect_diagnostics()?;
        let body = unwrap_block_controls(&self.document.document.body.content);
        let mut headings = self.headings(&body);
        let mut spine = self.spine_items(&body, &headings);

        let mut spine_index = 0;
        for heading in &mut headings {
            while spine_index + 1 < spine.len() && heading.body_index >= spine[spine_index].end {
                spine_index += 1;
            }
            let item = &spine[spine_index];
            if (item.start..item.end).contains(&heading.body_index) {
                heading.href = format!("{}#{}", item.href, heading.anchor);
            }
        }

        let heading_anchors = headings
            .iter()
            .map(|heading| (heading.body_index, heading.anchor.clone()))
            .collect::<HashMap<_, _>>();
        let list_values = selected_epub_list_values(
            &body,
            &self.document.styles,
            self.document.numbering.as_ref(),
        );

        let equations = crate::math::ExportEquations::default();
        for item in &mut spine {
            let fragment = emit_spine_fragment(
                &body,
                &mut input,
                &self.document.styles,
                &media,
                &equations,
                &heading_anchors,
                &list_values,
                item.start,
                item.end,
            );
            let fragment = fragment?;
            item.xhtml = xhtml_document(&item.title, &fragment);
            if item.xhtml.len() > MAX_EPUB_BYTES {
                return Err(epub_error("generated XHTML exceeds the EPUB output limit"));
            }
        }

        for message in equations.into_diagnostics() {
            self.diagnose("body/equation".to_owned(), message)?;
        }

        let nav_tree = build_nav_tree(&headings);
        let title = self
            .document
            .core_properties
            .as_ref()
            .and_then(|properties| properties.title.as_deref())
            .filter(|title| !title.trim().is_empty())
            .unwrap_or("Untitled document");
        let author = self
            .document
            .core_properties
            .as_ref()
            .and_then(|properties| properties.creator.as_deref())
            .filter(|author| !author.trim().is_empty())
            .unwrap_or("Unknown author");
        let nav = navigation_document(title, &headings, &nav_tree, &spine);
        let identifier = publication_identifier(title, author, &spine, &media);
        let package = package_document(title, author, &identifier, &spine, &media);
        ensure_xml_10("EPUB/package.opf", &package)?;
        ensure_xml_10("EPUB/nav.xhtml", &nav)?;
        for item in &spine {
            ensure_xml_10(&format!("EPUB/{}", item.href), &item.xhtml)?;
        }
        let bytes = write_archive(&package, &nav, &spine, &media)?;

        Ok(EpubWriteResult {
            bytes,
            diagnostics: self.diagnostics,
        })
    }

    fn check_source_limits(&self) -> Result<()> {
        let mut item_count = 0_usize;
        let mut text_bytes = 0_usize;
        let mut projected_nodes = 0_usize;
        let mut image_occurrences = 0_usize;
        if let Some(properties) = &self.document.core_properties {
            for (location, value) in [
                ("document title", properties.title.as_deref()),
                ("document author", properties.creator.as_deref()),
            ] {
                if let Some(value) = value {
                    add_source_bytes(&mut text_bytes, value.len())?;
                    ensure_xml_value(location, value)?;
                }
            }
        }
        if let Some(background) = &self.document.document.background_xml {
            add_source_bytes(&mut text_bytes, background.len())?;
        }
        for content in &self.document.document.body.content {
            measure_body_content(
                content,
                0,
                &mut item_count,
                &mut text_bytes,
                &mut projected_nodes,
                &mut image_occurrences,
            )?;
        }
        if image_occurrences > MAX_IMAGE_OCCURRENCES {
            return Err(epub_error(
                "document has too many image occurrences for EPUB export",
            ));
        }
        let projected_bytes = text_bytes
            .checked_mul(6)
            .and_then(|bytes| {
                projected_nodes
                    .checked_mul(256)
                    .and_then(|markup| bytes.checked_add(markup))
            })
            .ok_or_else(|| epub_error("projected XHTML size overflow during EPUB export"))?;
        if projected_bytes > MAX_EPUB_BYTES / 2 {
            return Err(epub_error(
                "projected XHTML exceeds the EPUB intermediate limit",
            ));
        }
        Ok(())
    }

    fn check_relationship_limits(&self) -> Result<()> {
        let Some(relationships) = self
            .document
            .package
            .get_part_rels(&self.document.doc_part_name)
        else {
            return Ok(());
        };
        if relationships.items.len() > MAX_RELATIONSHIPS {
            return Err(epub_error(
                "document has too many relationships for EPUB export",
            ));
        }
        let mut bytes = 0_usize;
        for relationship in &relationships.items {
            for value in [
                relationship.id.as_str(),
                relationship.rel_type.as_str(),
                relationship.target.as_str(),
                relationship.target_mode.as_deref().unwrap_or(""),
            ] {
                bytes = bytes
                    .checked_add(value.len())
                    .ok_or_else(|| epub_error("relationship size overflow during EPUB export"))?;
                if bytes > MAX_PROJECTION_KEY_BYTES {
                    return Err(epub_error(
                        "document relationships exceed the EPUB projection limit",
                    ));
                }
                ensure_xml_value("relationship value", value)?;
            }
        }
        Ok(())
    }

    fn headings(&self, body: &[BodyContent]) -> Vec<Heading> {
        let mut headings = Vec::new();
        for (body_index, content) in body.iter().enumerate() {
            let BodyContent::Paragraph(paragraph) = content else {
                continue;
            };
            let Some(level) =
                crate::document::paragraph_heading_level(paragraph, &self.document.styles)
            else {
                continue;
            };
            let ordinal = headings.len() + 1;
            headings.push(Heading {
                body_index,
                level,
                text: projected_paragraph_text(paragraph),
                anchor: format!("heading-{ordinal:04}"),
                href: String::new(),
            });
        }
        headings
    }

    fn spine_items(&self, body: &[BodyContent], headings: &[Heading]) -> Vec<SpineItem> {
        let roots = root_heading_indexes(headings);
        if roots.is_empty() {
            return vec![SpineItem {
                id: "document".to_owned(),
                href: "document.xhtml".to_owned(),
                title: "Document".to_owned(),
                start: 0,
                end: body.len(),
                xhtml: String::new(),
            }];
        }

        let mut items = Vec::new();
        let first_body_index = headings[roots[0]].body_index;
        if first_body_index > 0 {
            items.push(SpineItem {
                id: "front".to_owned(),
                href: "front.xhtml".to_owned(),
                title: "Front matter".to_owned(),
                start: 0,
                end: first_body_index,
                xhtml: String::new(),
            });
        }
        for (root_ordinal, heading_index) in roots.iter().copied().enumerate() {
            let start = headings[heading_index].body_index;
            let end = roots
                .get(root_ordinal + 1)
                .map(|next| headings[*next].body_index)
                .unwrap_or(body.len());
            let number = root_ordinal + 1;
            items.push(SpineItem {
                id: format!("chapter-{number:03}"),
                href: format!("chapter-{number:03}.xhtml"),
                title: headings[heading_index].text.clone(),
                start,
                end,
                xhtml: String::new(),
            });
        }
        items
    }

    fn media_items(
        &mut self,
    ) -> Result<(
        Vec<MediaItem>,
        HashMap<String, oxoffice_docx_html::ImageData>,
    )> {
        let mut relationship_ids = referenced_drawing_ids(&self.document.document.body.content);
        relationship_ids.sort_unstable();
        relationship_ids.dedup();
        if relationship_ids.len() > MAX_MEDIA_ITEMS {
            return Err(epub_error("document has too many images for EPUB export"));
        }
        let relationships = self
            .document
            .package
            .get_part_rels(&self.document.doc_part_name);
        let total = relationship_ids.iter().try_fold(0_usize, |total, id| {
            let Some(relationship) = relationships.and_then(|items| items.get_by_id(id)) else {
                return Some(total);
            };
            if relationship.rel_type != rel_types::IMAGE
                || !crate::document::relationship_is_internal(relationship)
            {
                return Some(total);
            }
            let part_name = oxoffice_opc::OpcPackage::resolve_rel_target(
                &self.document.doc_part_name,
                &relationship.target,
            );
            let Some(data) = self.document.package.get_part(&part_name) else {
                return Some(total);
            };
            if validated_epub_image(data).is_none() {
                return Some(total);
            }
            total.checked_add(data.len())
        });
        if total.is_none_or(|total| total > MAX_MEDIA_BYTES) {
            return Err(epub_error("document images exceed the EPUB media limit"));
        }

        let mut media: Vec<MediaItem> = Vec::new();
        let mut html_images = HashMap::new();
        for (ordinal, relationship_id) in relationship_ids.into_iter().enumerate() {
            let Some(relationship) =
                relationships.and_then(|items| items.get_by_id(&relationship_id))
            else {
                continue;
            };
            if relationship.rel_type != rel_types::IMAGE
                || !crate::document::relationship_is_internal(relationship)
            {
                continue;
            }
            let part_name = oxoffice_opc::OpcPackage::resolve_rel_target(
                &self.document.doc_part_name,
                &relationship.target,
            );
            let Some(data) = self.document.package.get_part(&part_name) else {
                continue;
            };
            let Some(format) = validated_epub_image(data) else {
                continue;
            };
            let content_type = format.content_type().to_owned();
            let extension = format.extension();
            html_images.insert(
                relationship.id.clone(),
                oxoffice_docx_html::ImageData {
                    data: (ordinal as u64).to_le_bytes().to_vec(),
                    content_type: content_type.clone(),
                },
            );
            if let Some(existing) = media.iter().find(|existing| {
                existing.media_type == content_type && existing.data.as_slice() == data
            }) {
                media.push(MediaItem {
                    relationship_id: relationship.id.clone(),
                    href: existing.href.clone(),
                    media_type: existing.media_type.clone(),
                    data: existing.data.clone(),
                });
                continue;
            }
            let number = media
                .iter()
                .filter(|item| item.href.starts_with("images/image-"))
                .count()
                + 1;
            media.push(MediaItem {
                relationship_id: relationship.id.clone(),
                href: format!("images/image-{number:03}.{extension}"),
                media_type: content_type,
                data: data.to_vec(),
            });
        }
        Ok((media, html_images))
    }

    fn epub_hyperlinks(&self) -> HashMap<String, String> {
        let mut relationship_ids = referenced_hyperlink_ids(&self.document.document.body.content);
        relationship_ids.sort_unstable();
        relationship_ids.dedup();
        let mut hyperlinks = HashMap::new();
        let relationships = self
            .document
            .package
            .get_part_rels(&self.document.doc_part_name);
        for relationship_id in relationship_ids {
            let Some(relationship) =
                relationships.and_then(|items| items.get_by_id(&relationship_id))
            else {
                continue;
            };
            if relationship.rel_type == rel_types::HYPERLINK
                && relationship.target_mode.as_deref() == Some("External")
                && let Some(target) = safe_absolute_url(&relationship.target)
            {
                hyperlinks.insert(relationship.id.clone(), target.to_owned());
            }
        }
        hyperlinks
    }

    fn collect_diagnostics(&mut self) -> Result<()> {
        if self.document.document.background_xml.is_some() {
            self.diagnose(
                "document/background".to_owned(),
                "document background was dropped during EPUB export".to_owned(),
            )?;
        }
        if let Some(properties) = &self.document.core_properties {
            for (present, name) in [
                (properties.subject.is_some(), "subject"),
                (properties.description.is_some(), "description"),
                (properties.keywords.is_some(), "keywords"),
                (properties.last_modified_by.is_some(), "last-modified-by"),
                (properties.created.is_some(), "created"),
                (properties.modified.is_some(), "modified"),
            ] {
                if present {
                    self.diagnose(
                        format!("metadata/{name}"),
                        format!("document {name} metadata was dropped during EPUB export"),
                    )?;
                }
            }
        }
        if let Some(properties) = &self.document.custom_properties {
            for (index, _) in properties.properties.iter().enumerate() {
                self.diagnose(
                    format!("metadata/custom-property[{index}]"),
                    "custom document property was dropped during EPUB export".to_owned(),
                )?;
            }
        }
        for (index, content) in self.document.document.body.content.iter().enumerate() {
            self.scan_body_content(content, &format!("body[{index}]"), 0)?;
        }
        if self.document.document.body.sect_pr.is_some() {
            self.diagnose(
                "body/properties/section".to_owned(),
                "final section properties were dropped during EPUB export".to_owned(),
            )?;
        }
        Ok(())
    }

    fn scan_body_content(&mut self, content: &BodyContent, path: &str, depth: usize) -> Result<()> {
        match content {
            BodyContent::Paragraph(paragraph) => self.scan_paragraph(paragraph, path),
            BodyContent::Table(table) => self.scan_table(table, path, depth + 1),
            BodyContent::ContentControl(control) => {
                self.diagnose(
                    path.to_owned(),
                    "body content control was flattened during EPUB export".to_owned(),
                )?;
                self.scan_control(control, path, depth + 1, false)
            }
            BodyContent::RawXml(raw) => self.diagnose(
                path.to_owned(),
                raw_dropped(raw, "unmodelled body XML was dropped during EPUB export"),
            ),
        }
    }

    fn scan_table(&mut self, table: &CT_Tbl, path: &str, depth: usize) -> Result<()> {
        if depth > MAX_NESTING_DEPTH {
            return Err(epub_error("table nesting exceeds the EPUB depth limit"));
        }
        if table.grid.is_some() {
            self.diagnose(
                format!("{path}/grid"),
                "table grid widths were dropped during EPUB export".to_owned(),
            )?;
        }
        if let Some(properties) = &table.properties {
            self.scan_table_properties(properties, path)?;
        }
        for (raw_index, (_, raw)) in table.extra_xml.iter().enumerate() {
            self.diagnose(
                format!("{path}/xml[{raw_index}]"),
                raw_dropped(raw, "unmodelled table XML was dropped during EPUB export"),
            )?;
        }
        // Rows that content controls wrap are scanned in document order.
        for row_index in 0..=table.rows.len() {
            for (control_index, (_, _, control)) in table
                .content_controls
                .iter()
                .enumerate()
                .filter(|(_, (at, _, _))| *at == row_index)
            {
                let control_path = format!("{path}/content-control[{control_index}]");
                self.diagnose(
                    control_path.clone(),
                    "table row content control was flattened during EPUB export".to_owned(),
                )?;
                self.scan_control(control, &control_path, depth + 1, true)?;
            }
            if let Some(row) = table.rows.get(row_index) {
                self.scan_row(row, &format!("{path}/row[{row_index}]"), depth)?;
            }
        }
        Ok(())
    }

    fn scan_row(&mut self, row: &CT_Row, row_path: &str, depth: usize) -> Result<()> {
        if let Some(properties) = &row.properties {
            self.scan_row_properties(properties, row_path)?;
        }
        for (raw_index, (position, raw)) in row.extra_xml.iter().enumerate() {
            if CT_Row::raw_is_root_attributes(*position, raw) {
                continue;
            }
            self.diagnose(
                format!("{row_path}/xml[{raw_index}]"),
                raw_dropped(
                    raw,
                    "unmodelled table-row XML was dropped during EPUB export",
                ),
            )?;
        }
        // Cells that content controls wrap are scanned in document order.
        for cell_index in 0..=row.cells.len() {
            for (control_index, (_, _, control)) in row
                .content_controls
                .iter()
                .enumerate()
                .filter(|(_, (at, _, _))| *at == cell_index)
            {
                let control_path = format!("{row_path}/content-control[{control_index}]");
                self.diagnose(
                    control_path.clone(),
                    "table-cell content control was flattened during EPUB export".to_owned(),
                )?;
                self.scan_control(control, &control_path, depth + 1, true)?;
            }
            if let Some(cell) = row.cells.get(cell_index) {
                self.scan_cell(cell, &format!("{row_path}/cell[{cell_index}]"), depth)?;
            }
        }
        Ok(())
    }

    fn scan_cell(&mut self, cell: &CT_Tc, cell_path: &str, depth: usize) -> Result<()> {
        if let Some(properties) = &cell.properties {
            self.scan_cell_properties(properties, cell_path)?;
        }
        for (raw_index, (_, raw)) in cell.extra_xml.iter().enumerate() {
            self.diagnose(
                format!("{cell_path}/xml[{raw_index}]"),
                raw_dropped(
                    raw,
                    "unmodelled table-cell XML was dropped during EPUB export",
                ),
            )?;
        }
        for (content_index, content) in cell.content.iter().enumerate() {
            let child_path = format!("{cell_path}/content[{content_index}]");
            match content {
                CellContent::Paragraph(paragraph) => {
                    self.scan_cell_paragraph(paragraph, &child_path)?
                }
                CellContent::Table(nested) => self.scan_table(nested, &child_path, depth + 1)?,
                CellContent::ContentControl(control) => {
                    self.diagnose(
                        child_path.clone(),
                        "table-cell content control was flattened during EPUB export".to_owned(),
                    )?;
                    self.scan_control(control, &child_path, depth + 1, true)?;
                }
            }
        }
        Ok(())
    }

    fn scan_cell_paragraph(&mut self, paragraph: &CT_P, path: &str) -> Result<()> {
        if detect_list(
            paragraph,
            &self.document.styles,
            self.document.numbering.as_ref(),
        )
        .is_some()
        {
            self.diagnose(
                format!("{path}/properties/numbering"),
                "table-cell list semantics were flattened during EPUB export".to_owned(),
            )?;
        }
        self.scan_paragraph(paragraph, path)
    }

    /// Scan what a content control wraps, which EPUB export writes in place
    /// as if the control were not there. `in_cell` says whether its
    /// paragraphs are table-cell paragraphs.
    fn scan_control(
        &mut self,
        control: &CT_Sdt,
        path: &str,
        depth: usize,
        in_cell: bool,
    ) -> Result<()> {
        if depth > MAX_NESTING_DEPTH {
            return Err(epub_error(
                "content control nesting exceeds the EPUB depth limit",
            ));
        }
        for (content_index, content) in control.content.iter().enumerate() {
            let child_path = format!("{path}/content[{content_index}]");
            match content {
                SdtContent::Paragraph(paragraph) if in_cell => {
                    self.scan_cell_paragraph(paragraph, &child_path)?
                }
                SdtContent::Paragraph(paragraph) => self.scan_paragraph(paragraph, &child_path)?,
                SdtContent::Table(table) => self.scan_table(table, &child_path, depth + 1)?,
                SdtContent::Row(row) => self.scan_row(row, &child_path, depth)?,
                SdtContent::Cell(cell) => self.scan_cell(cell, &child_path, depth)?,
                SdtContent::Run(run) => self.scan_run(run, &child_path)?,
                SdtContent::ContentControl(nested) => {
                    self.diagnose(
                        child_path.clone(),
                        "nested content control was flattened during EPUB export".to_owned(),
                    )?;
                    self.scan_control(nested, &child_path, depth + 1, in_cell)?;
                }
                SdtContent::RawXml(raw) => self.diagnose(
                    child_path,
                    raw_dropped(
                        raw,
                        "unmodelled content control XML was dropped during EPUB export",
                    ),
                )?,
            }
        }
        for (revision_index, (_, revision)) in control.revisions().iter().enumerate() {
            self.scan_revision(revision, &format!("{path}/revision[{revision_index}]"))?;
        }
        Ok(())
    }

    fn scan_table_properties(
        &mut self,
        properties: &oxoffice_docx_oxml::table::CT_TblPr,
        path: &str,
    ) -> Result<()> {
        for (present, name, message) in [
            (
                properties.style_id.is_some(),
                "style",
                "table style was dropped during EPUB export",
            ),
            (
                properties.width.is_some(),
                "width",
                "table width was dropped during EPUB export",
            ),
            (
                properties.cell_margin.is_some(),
                "cell-margin",
                "table cell margins were dropped during EPUB export",
            ),
            (
                properties.layout.is_some(),
                "layout",
                "table layout mode was dropped during EPUB export",
            ),
            (
                properties.indent.is_some(),
                "indent",
                "table indent was dropped during EPUB export",
            ),
            (
                properties.look.is_some(),
                "look",
                "table conditional look was dropped during EPUB export",
            ),
            (
                properties.change.is_some(),
                "revision",
                "table property revision was dropped during EPUB export",
            ),
        ] {
            if present {
                self.diagnose(format!("{path}/properties/{name}"), message.to_owned())?;
            }
        }
        if properties.borders.is_some() {
            self.diagnose(
                format!("{path}/properties/borders"),
                "table border details were simplified during EPUB export".to_owned(),
            )?;
        }
        if properties
            .shading
            .as_ref()
            .is_some_and(shading_is_simplified)
        {
            self.diagnose(
                format!("{path}/properties/shading"),
                "table shading theme colour or invalid colour was simplified during EPUB export"
                    .to_owned(),
            )?;
        }
        for (index, _) in properties.revision_xml.iter().enumerate() {
            self.diagnose(
                format!("{path}/properties/xml[{index}]"),
                "unmodelled table property XML was dropped during EPUB export".to_owned(),
            )?;
        }
        Ok(())
    }

    fn scan_row_properties(
        &mut self,
        properties: &oxoffice_docx_oxml::table::CT_TrPr,
        path: &str,
    ) -> Result<()> {
        for (present, name) in [
            (properties.height.is_some(), "height"),
            (properties.height_rule.is_some(), "height-rule"),
            (properties.header.is_some(), "repeat-header"),
            (properties.jc.is_some(), "alignment"),
            (properties.cant_split.is_some(), "cant-split"),
            (properties.cnf_style.is_some(), "conditional-style"),
        ] {
            if present {
                self.diagnose(
                    format!("{path}/properties/{name}"),
                    format!("table-row {name} was dropped during EPUB export"),
                )?;
            }
        }
        for (index, _) in properties.revision_markers.iter().enumerate() {
            self.diagnose(
                format!("{path}/properties/revision[{index}]"),
                "table-row revision marker was dropped during EPUB export".to_owned(),
            )?;
        }
        for (index, _) in properties.revision_xml.iter().enumerate() {
            self.diagnose(
                format!("{path}/properties/xml[{index}]"),
                "unmodelled table-row property XML was dropped during EPUB export".to_owned(),
            )?;
        }
        Ok(())
    }

    fn scan_cell_properties(
        &mut self,
        properties: &oxoffice_docx_oxml::table::CT_TcPr,
        path: &str,
    ) -> Result<()> {
        for (present, name) in [
            (properties.width.is_some(), "width"),
            (properties.no_wrap.is_some(), "no-wrap"),
            (properties.text_direction.is_some(), "text-direction"),
            (properties.cnf_style.is_some(), "conditional-style"),
        ] {
            if present {
                self.diagnose(
                    format!("{path}/properties/{name}"),
                    format!("table-cell {name} was dropped during EPUB export"),
                )?;
            }
        }
        if properties.borders.is_some() {
            self.diagnose(
                format!("{path}/properties/borders"),
                "table-cell border details were simplified during EPUB export".to_owned(),
            )?;
        }
        if properties
            .shading
            .as_ref()
            .is_some_and(shading_is_simplified)
        {
            self.diagnose(
                format!("{path}/properties/shading"),
                "table-cell shading theme colour or invalid colour was simplified during EPUB export"
                    .to_owned(),
            )?;
        }
        for (index, _) in properties.extra_xml.iter().enumerate() {
            self.diagnose(
                format!("{path}/properties/xml[{index}]"),
                "unmodelled table-cell property XML was dropped during EPUB export".to_owned(),
            )?;
        }
        Ok(())
    }

    fn scan_paragraph(&mut self, paragraph: &CT_P, path: &str) -> Result<()> {
        if paragraph
            .properties
            .as_ref()
            .and_then(|properties| properties.style_id.as_deref())
            .is_none()
            && default_paragraph_style_has_visible_effects(&self.document.styles, paragraph)
        {
            self.diagnose(
                format!("{path}/properties/default-style"),
                "default paragraph or run formatting was dropped during EPUB export".to_owned(),
            )?;
        }
        if let Some(properties) = &paragraph.properties {
            if let Some(style_id) = properties.style_id.as_deref() {
                let style = self.document.styles.get_by_id(style_id);
                if style.is_none_or(style_has_unprojected_visuals) {
                    self.diagnose(
                        format!("{path}/properties/style"),
                        "paragraph style formatting was dropped during EPUB export".to_owned(),
                    )?;
                }
            }
            if projected_heading_level(paragraph, &self.document.styles)
                .is_some_and(|level| level > 6)
            {
                let lowered = properties.outline_lvl.is_some_and(|level| level >= 6)
                    || crate::document::paragraph_heading_level(paragraph, &self.document.styles)
                        .is_some_and(|level| level > 6);
                self.diagnose(
                    format!("{path}/properties/heading-level"),
                    if lowered {
                        "heading level above 6 was reduced to level 6 during EPUB export"
                    } else {
                        "style-derived heading level above 6 was flattened to a paragraph during EPUB export"
                    }
                    .to_owned(),
                )?;
            }
            if properties
                .shading
                .as_ref()
                .is_some_and(|shading| shading_is_simplified(shading))
            {
                self.diagnose(
                    format!("{path}/properties/shading"),
                    "paragraph shading theme colour or invalid colour was simplified during EPUB export"
                        .to_owned(),
                )?;
            }
            if properties.num_id.is_some_and(|id| id != 0)
                && list_definition(paragraph, self.document.numbering.as_ref()).is_none()
            {
                self.diagnose(
                    format!("{path}/properties/numbering"),
                    "unresolved list definition was flattened during EPUB export".to_owned(),
                )?;
            }
            if let Some(level) = list_definition(paragraph, self.document.numbering.as_ref()) {
                let producer_defined =
                    matches!(level.num_fmt.as_ref(), Some(ST_NumberFormat::Other(_)));
                let unrendered_standard =
                    !producer_defined && epub_list_semantics(level.num_fmt.as_ref()).is_none();
                if producer_defined {
                    self.diagnose(
                        format!("{path}/properties/numbering/format"),
                        "producer-defined numbering format was emitted without a marker during EPUB export"
                            .to_owned(),
                    )?;
                    if level.start.is_some_and(|start| start != 1) {
                        self.diagnose(
                            format!("{path}/properties/numbering/start"),
                            "producer-defined list start value was dropped during EPUB export"
                                .to_owned(),
                        )?;
                    }
                } else if unrendered_standard {
                    self.diagnose(
                        format!("{path}/properties/numbering/format"),
                        "standard numbering format is not rendered by EPUB export and was emitted without a marker"
                            .to_owned(),
                    )?;
                    if level.start.is_some_and(|start| start != 1) {
                        self.diagnose(
                            format!("{path}/properties/numbering/start"),
                            "unrendered list start value was dropped during EPUB export".to_owned(),
                        )?;
                    }
                } else if level.num_fmt == Some(ST_NumberFormat::Ordinal) {
                    self.diagnose(
                        format!("{path}/properties/numbering/format"),
                        "ordinal list markers were reduced to decimal during EPUB export"
                            .to_owned(),
                    )?;
                }
                if let Some(marker) = &level.lvl_text {
                    let standard = format!("%{}.", level.ilvl + 1);
                    // A Chicago item writes its level text, unless it holds
                    // another level's number.
                    let own_marker_written = level.num_fmt == Some(ST_NumberFormat::Chicago)
                        && level.legal != Some(true)
                        && (1..=9)
                            .filter(|other| *other != level.ilvl + 1)
                            .all(|other| !marker.contains(&format!("%{other}")));
                    if producer_defined
                        || unrendered_standard
                        || level.num_fmt == Some(ST_NumberFormat::Bullet)
                        || (marker != &standard && !own_marker_written)
                    {
                        self.diagnose(
                            format!("{path}/properties/numbering/marker"),
                            if producer_defined || unrendered_standard {
                                "list marker text was dropped during EPUB export"
                            } else {
                                "custom list marker text was replaced by EPUB list semantics"
                            }
                            .to_owned(),
                        )?;
                    }
                }
                if level
                    .ppr
                    .as_ref()
                    .is_some_and(|properties| !properties.carries_nothing())
                    || level.ppr_raw.is_some()
                {
                    self.diagnose(
                        format!("{path}/properties/numbering/paragraph-style"),
                        "list-level paragraph styling was dropped during EPUB export".to_owned(),
                    )?;
                }
                if level
                    .rpr
                    .as_ref()
                    .is_some_and(|properties| !properties.carries_nothing())
                    || level.rpr_raw.is_some()
                {
                    self.diagnose(
                        format!("{path}/properties/numbering/marker-style"),
                        "list marker run styling was dropped during EPUB export".to_owned(),
                    )?;
                }
                if level.lvl_jc.is_some() {
                    self.diagnose(
                        format!("{path}/properties/numbering/alignment"),
                        "list marker alignment was dropped during EPUB export".to_owned(),
                    )?;
                }
                if level.suffix.is_some() {
                    self.diagnose(
                        format!("{path}/properties/numbering/suffix"),
                        if producer_defined || unrendered_standard {
                            "list marker suffix was dropped during EPUB export"
                        } else {
                            "list marker suffix spacing was normalized during EPUB export"
                        }
                        .to_owned(),
                    )?;
                }
                if !level.extra_xml.is_empty() || !level.extra_attributes.is_empty() {
                    self.diagnose(
                        format!("{path}/properties/numbering/xml"),
                        "unmodelled list-level XML was dropped during EPUB export".to_owned(),
                    )?;
                }
            }
            for (present, name, message) in [
                (
                    properties.before_autospacing.is_some(),
                    "before-auto-spacing",
                    "paragraph automatic spacing was dropped during EPUB export",
                ),
                (
                    properties.after_autospacing.is_some(),
                    "after-auto-spacing",
                    "paragraph automatic spacing was dropped during EPUB export",
                ),
                (
                    properties.ind_hanging.is_some(),
                    "hanging-indent",
                    "paragraph hanging indent was dropped during EPUB export",
                ),
                (
                    properties.keep_next.is_some(),
                    "keep-next",
                    "paragraph keep-next was dropped during EPUB export",
                ),
                (
                    properties.keep_lines.is_some(),
                    "keep-lines",
                    "paragraph keep-lines was dropped during EPUB export",
                ),
                (
                    properties.page_break_before.is_some(),
                    "page-break-before",
                    "paragraph page-break-before was dropped during EPUB export",
                ),
                (
                    properties.widow_control.is_some(),
                    "widow-control",
                    "paragraph widow control was dropped during EPUB export",
                ),
                (
                    properties.suppress_auto_hyphens.is_some(),
                    "suppress-hyphens",
                    "paragraph hyphenation control was dropped during EPUB export",
                ),
                (
                    properties.borders.is_some(),
                    "borders",
                    "paragraph borders were dropped during EPUB export",
                ),
                (
                    properties.tabs.is_some(),
                    "tabs",
                    "paragraph tab stops were dropped during EPUB export",
                ),
                (
                    properties
                        .rpr
                        .as_ref()
                        .is_some_and(|properties| !properties.carries_nothing()),
                    "mark-properties",
                    "paragraph mark properties were dropped during EPUB export",
                ),
                (
                    properties.sect_pr.is_some(),
                    "section",
                    "paragraph section properties were dropped during EPUB export",
                ),
                (
                    properties.numbering_revision.is_some(),
                    "numbering-revision",
                    "paragraph numbering revision was dropped during EPUB export",
                ),
                (
                    properties.change.is_some(),
                    "revision",
                    "paragraph property revision was dropped during EPUB export",
                ),
            ] {
                if present {
                    self.diagnose(format!("{path}/properties/{name}"), message.to_owned())?;
                }
            }
            for (index, _) in properties.numbering_revision_xml.iter().enumerate() {
                self.diagnose(
                    format!("{path}/properties/numbering-xml[{index}]"),
                    "unmodelled paragraph numbering XML was dropped during EPUB export".to_owned(),
                )?;
            }
            for (index, _) in properties.revision_xml.iter().enumerate() {
                self.diagnose(
                    format!("{path}/properties/xml[{index}]"),
                    "unmodelled paragraph property XML was dropped during EPUB export".to_owned(),
                )?;
            }
        }
        // The typed source coordinate retains the parser's in-scope namespace
        // decision, including paragraph-local shadows of document bindings.
        let mut typed_raw_markers = HashMap::<(usize, usize, u8), usize>::new();
        for marker in &paragraph.bookmark_markers {
            let kind = if marker.is_start() { 2 } else { 3 };
            *typed_raw_markers
                .entry((marker.run_index(), marker.raw_before(), kind))
                .or_default() += 1;
        }
        for (run_index, raw_before, revision) in &paragraph.revisions {
            let kind = match revision.kind() {
                oxoffice_docx_oxml::revision::RevisionKind::Insertion => 10,
                oxoffice_docx_oxml::revision::RevisionKind::Deletion => 11,
                oxoffice_docx_oxml::revision::RevisionKind::MoveFrom => 12,
                oxoffice_docx_oxml::revision::RevisionKind::MoveTo => 13,
                _ => continue,
            };
            *typed_raw_markers
                .entry((*run_index, *raw_before, kind))
                .or_default() += 1;
        }
        let mut consumed_raw = vec![false; paragraph.extra_xml.len()];
        let mut raw_ordinals = HashMap::<usize, usize>::new();
        for (index, (run_index, raw)) in paragraph.extra_xml.iter().enumerate() {
            if CT_P::raw_is_root_attributes(*run_index, raw) {
                consumed_raw[index] = true;
                continue;
            }
            let raw_ordinal = raw_ordinals.entry(*run_index).or_default();
            let ordinal = *raw_ordinal;
            *raw_ordinal += 1;
            let Some(marker) = raw_marker_kind(raw) else {
                continue;
            };
            if let Some(remaining) = typed_raw_markers.get_mut(&(*run_index, ordinal, marker.kind))
                && *remaining > 0
            {
                *remaining -= 1;
                consumed_raw[index] = true;
            }
        }
        for (extra_index, consumed) in consumed_raw.into_iter().enumerate() {
            // An equation is written as MathML.
            if consumed || crate::math::raw_is_equation(paragraph, extra_index) {
                continue;
            }
            let raw_path = format!("{path}/xml[{extra_index}]");
            if let Some(content) =
                CT_P::raw_run_wrapper_content(&paragraph.extra_xml[extra_index].1)
            {
                self.diagnose(
                    raw_path.clone(),
                    "smart tag or custom XML wrapper was flattened during EPUB export".to_owned(),
                )?;
                self.scan_inline_content(&content, &raw_path)?;
            } else {
                self.diagnose(
                    raw_path,
                    "unmodelled paragraph XML was dropped during EPUB export".to_owned(),
                )?;
            }
        }
        for (control_index, (_, _, _, control)) in paragraph.content_controls.iter().enumerate() {
            let control_path = format!("{path}/content-control[{control_index}]");
            self.diagnose(
                control_path.clone(),
                "run content control was flattened during EPUB export".to_owned(),
            )?;
            self.scan_control(control, &control_path, 1, false)?;
        }
        for (revision_index, (_, _, revision)) in paragraph.revisions.iter().enumerate() {
            self.scan_revision(revision, &format!("{path}/revision[{revision_index}]"))?;
        }
        for (marker_index, _) in paragraph.comment_ranges.iter().enumerate() {
            self.diagnose(
                format!("{path}/comment-range[{marker_index}]"),
                "comment range marker was dropped during EPUB export".to_owned(),
            )?;
        }
        for (marker_index, _) in paragraph.bookmark_markers.iter().enumerate() {
            self.diagnose(
                format!("{path}/bookmark[{marker_index}]"),
                "bookmark marker was dropped during EPUB export".to_owned(),
            )?;
        }
        for (hyperlink_index, hyperlink) in paragraph.hyperlinks.iter().enumerate() {
            let hyperlink_path = format!("{path}/hyperlink[{hyperlink_index}]");
            if hyperlink.anchor.is_some() {
                self.diagnose(
                    hyperlink_path.clone(),
                    "internal hyperlink anchor was dropped during EPUB export".to_owned(),
                )?;
            } else if let Some(reason) = self.hyperlink_loss_reason(hyperlink.rel_id.as_deref()) {
                self.diagnose(hyperlink_path.clone(), reason.to_owned())?;
            }
            if !hyperlink.extra_attributes.is_empty() || hyperlink.preserved_raw_before.is_some() {
                self.diagnose(
                    format!("{hyperlink_path}/owner-xml"),
                    "unmodelled hyperlink owner XML was dropped during EPUB export".to_owned(),
                )?;
            }
            for (raw_index, _) in hyperlink.extra_xml.iter().enumerate() {
                self.diagnose(
                    format!("{hyperlink_path}/xml[{raw_index}]"),
                    "unmodelled hyperlink child XML was dropped during EPUB export".to_owned(),
                )?;
            }
        }
        for (run_index, run) in paragraph.runs.iter().enumerate() {
            self.scan_run(run, &format!("{path}/run[{run_index}]"))?;
        }
        Ok(())
    }

    /// Scan a tracked revision. An insertion or a move in is written in place,
    /// and a deletion or a move away is left out, as the accepted view reads.
    fn scan_revision(&mut self, revision: &CT_Revision, path: &str) -> Result<()> {
        if revision_removes_text(revision) {
            return self.diagnose(
                path.to_owned(),
                "deleted or moved-away revision content was dropped during EPUB export".to_owned(),
            );
        }
        self.diagnose(
            path.to_owned(),
            "paragraph revision wrapper was flattened during EPUB export".to_owned(),
        )?;
        match accepted_revision_content(revision) {
            Some(content) => self.scan_inline_content(content, path),
            None => Ok(()),
        }
    }

    /// Scan the runs of the inline content of a wrapper or a revision, as its
    /// accepted view writes them.
    fn scan_inline_content(&mut self, content: &CT_P, path: &str) -> Result<()> {
        for (run_index, run) in content.accepted_view().runs.iter().enumerate() {
            self.scan_run(run, &format!("{path}/run[{run_index}]"))?;
        }
        Ok(())
    }

    fn scan_run(&mut self, run: &CT_R, path: &str) -> Result<()> {
        if let Some(properties) = &run.properties {
            self.scan_run_properties(properties, path)?;
        }
        for raw_index in 0..run.extra_xml.len() {
            if run
                .extra_xml_positions
                .get(raw_index)
                .is_some_and(|position| CT_R::raw_child_is_root_attributes(*position))
            {
                continue;
            }
            self.diagnose(
                format!("{path}/xml[{raw_index}]"),
                "unmodelled run XML was dropped during EPUB export".to_owned(),
            )?;
        }
        for (drawing_index, _) in run.alt_drawings.iter().enumerate() {
            self.diagnose(
                format!("{path}/alternate-drawing[{drawing_index}]"),
                "alternate drawing payload was dropped during EPUB export".to_owned(),
            )?;
        }
        for (content_index, content) in run.content.iter().enumerate() {
            let content_path = format!("{path}/content[{content_index}]");
            match content {
                RunContent::Text(text) if text.preserve_space => self.diagnose(
                    format!("{content_path}/space"),
                    "preserved Word text spacing was normalized during EPUB export".to_owned(),
                )?,
                RunContent::DeletedText(text) if text.preserve_space => {
                    self.diagnose(
                        format!("{content_path}/space"),
                        "preserved Word text spacing was normalized during EPUB export".to_owned(),
                    )?;
                    self.diagnose(
                        content_path,
                        "deleted-text revision semantics were flattened during EPUB export"
                            .to_owned(),
                    )?;
                }
                RunContent::Break(oxoffice_docx_oxml::text::BreakType::Column, _) => self
                    .diagnose(
                        content_path,
                        "column break was simplified to a line break during EPUB export".to_owned(),
                    )?,
                RunContent::DeletedText(_) => self.diagnose(
                    content_path,
                    "deleted-text revision semantics were flattened during EPUB export".to_owned(),
                )?,
                RunContent::Field(_) => self.diagnose(
                    content_path,
                    "field semantics were flattened to cached display during EPUB export"
                        .to_owned(),
                )?,
                RunContent::FootnoteRef { .. } => self.diagnose(
                    content_path,
                    "footnote reference was dropped during EPUB export".to_owned(),
                )?,
                RunContent::EndnoteRef { .. } => self.diagnose(
                    content_path,
                    "endnote reference was dropped during EPUB export".to_owned(),
                )?,
                RunContent::CommentReference { .. } => self.diagnose(
                    content_path,
                    "comment reference was dropped during EPUB export".to_owned(),
                )?,
                RunContent::Drawing(drawing) => {
                    let relationship_id = drawing
                        .inline
                        .as_ref()
                        .map(|image| image.embed_id.as_str())
                        .or_else(|| drawing.anchor.as_ref().map(|image| image.embed_id.as_str()));
                    if let Some(reason) = self.image_loss_reason(relationship_id) {
                        self.diagnose(content_path.clone(), reason.to_owned())?;
                    } else if drawing.anchor.is_some() {
                        self.diagnose(
                            content_path.clone(),
                            "floating image placement was converted to inline flow".to_owned(),
                        )?;
                    }
                    let source = drawing
                        .inline
                        .as_ref()
                        .map(|image| {
                            (
                                image.name.as_deref(),
                                image.raw_xml.is_some(),
                                image.extent_cx.0,
                                image.extent_cy.0,
                            )
                        })
                        .or_else(|| {
                            drawing.anchor.as_ref().map(|image| {
                                (
                                    image.name.as_deref(),
                                    image.raw_xml.is_some(),
                                    image.extent_cx.0,
                                    image.extent_cy.0,
                                )
                            })
                        });
                    if let Some((name, has_raw_xml, width, height)) = source {
                        if name.is_some() {
                            self.diagnose(
                                format!("{content_path}/name"),
                                "drawing name was dropped during EPUB export".to_owned(),
                            )?;
                        }
                        if width != 0 || height != 0 {
                            self.diagnose(
                                format!("{content_path}/extent"),
                                "drawing extent was simplified to responsive EPUB sizing"
                                    .to_owned(),
                            )?;
                        }
                        if has_raw_xml {
                            self.diagnose(
                                format!("{content_path}/xml"),
                                "preserved drawing XML was dropped during EPUB export".to_owned(),
                            )?;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn scan_run_properties(
        &mut self,
        properties: &oxoffice_docx_oxml::properties::CT_RPr,
        path: &str,
    ) -> Result<()> {
        for (present, name, message) in [
            (
                properties.style_id.is_some(),
                "style",
                "run style was dropped during EPUB export",
            ),
            (
                properties.font_hansi.is_some() && properties.font_hansi != properties.font_ascii,
                "font-hansi",
                "alternate run font was dropped during EPUB export",
            ),
            (
                properties.font_east_asia.is_some(),
                "font-east-asia",
                "East Asian run font was dropped during EPUB export",
            ),
            (
                properties.font_cs.is_some(),
                "font-complex-script",
                "complex-script run font was dropped during EPUB export",
            ),
            (
                properties.font_ascii_theme.is_some() || properties.font_hansi_theme.is_some(),
                "theme-font",
                "theme run font was dropped during EPUB export",
            ),
            (
                properties.bold_cs.is_some() && properties.bold_cs != properties.bold,
                "bold-complex-script",
                "complex-script bold was dropped during EPUB export",
            ),
            (
                properties.italic_cs.is_some() && properties.italic_cs != properties.italic,
                "italic-complex-script",
                "complex-script italic was dropped during EPUB export",
            ),
            (
                properties.sz_cs.is_some() && properties.sz_cs != properties.sz,
                "size-complex-script",
                "complex-script font size was dropped during EPUB export",
            ),
            (
                properties.color_theme.is_some(),
                "theme-colour",
                "theme run colour was dropped during EPUB export",
            ),
            (
                properties.width_scale.is_some(),
                "width-scale",
                "run width scale was dropped during EPUB export",
            ),
            (
                properties.position.is_some(),
                "position",
                "run text position was dropped during EPUB export",
            ),
            (
                properties.vanish.is_some(),
                "hidden",
                "hidden-text semantics were dropped during EPUB export",
            ),
            (
                properties.change.is_some(),
                "revision",
                "run property revision was dropped during EPUB export",
            ),
        ] {
            if present {
                self.diagnose(format!("{path}/properties/{name}"), message.to_owned())?;
            }
        }
        if properties.dstrike.is_some() {
            self.diagnose(
                format!("{path}/properties/double-strike"),
                "double strikethrough was simplified during EPUB export".to_owned(),
            )?;
        }
        if properties.underline.is_some_and(|underline| {
            !matches!(
                underline,
                oxoffice_docx_oxml::shared::ST_Underline::None
                    | oxoffice_docx_oxml::shared::ST_Underline::Single
            )
        }) {
            self.diagnose(
                format!("{path}/properties/underline"),
                "non-basic underline style was simplified to a single underline during EPUB export"
                    .to_owned(),
            )?;
        }
        if properties
            .shading
            .as_ref()
            .is_some_and(|shading| shading_is_simplified(shading))
        {
            self.diagnose(
                format!("{path}/properties/shading"),
                "run shading theme colour or invalid colour was simplified during EPUB export"
                    .to_owned(),
            )?;
        }
        for (index, _) in properties.revision_markers.iter().enumerate() {
            self.diagnose(
                format!("{path}/properties/revision-marker[{index}]"),
                "run revision marker was dropped during EPUB export".to_owned(),
            )?;
        }
        for (index, _) in properties.revision_xml.iter().enumerate() {
            self.diagnose(
                format!("{path}/properties/xml[{index}]"),
                "unmodelled run property XML was dropped during EPUB export".to_owned(),
            )?;
        }
        Ok(())
    }

    fn image_loss_reason(&self, relationship_id: Option<&str>) -> Option<&'static str> {
        let Some(relationship_id) = relationship_id else {
            return Some("drawing without an image relationship was dropped during EPUB export");
        };
        let Some(relationship) = self
            .document
            .package
            .get_part_rels(&self.document.doc_part_name)
            .and_then(|relationships| relationships.get_by_id(relationship_id))
        else {
            return Some("unresolved image relationship was dropped during EPUB export");
        };
        if relationship.rel_type != rel_types::IMAGE
            || !crate::document::relationship_is_internal(relationship)
        {
            return Some("non-package image relationship was dropped during EPUB export");
        }
        let part_name = oxoffice_opc::OpcPackage::resolve_rel_target(
            &self.document.doc_part_name,
            &relationship.target,
        );
        let Some(data) = self.document.package.get_part(&part_name) else {
            return Some("unresolved image part was dropped during EPUB export");
        };
        if validated_epub_image(data).is_none() {
            return Some("non-core EPUB image type was dropped during EPUB export");
        }
        None
    }

    fn hyperlink_loss_reason(&self, relationship_id: Option<&str>) -> Option<&'static str> {
        let Some(relationship_id) = relationship_id else {
            return Some(
                "hyperlink without an external relationship was dropped during EPUB export",
            );
        };
        let Some(relationship) = self
            .document
            .package
            .get_part_rels(&self.document.doc_part_name)
            .and_then(|relationships| relationships.get_by_id(relationship_id))
        else {
            return Some("unresolved hyperlink relationship was dropped during EPUB export");
        };
        if relationship.rel_type != rel_types::HYPERLINK
            || relationship.target_mode.as_deref() != Some("External")
        {
            return Some("non-external hyperlink relationship was dropped during EPUB export");
        }
        if safe_absolute_url(&relationship.target).is_none() {
            return Some(
                "empty, relative, or unsafe hyperlink target was dropped during EPUB export",
            );
        }
        None
    }

    fn diagnose(&mut self, path: String, message: String) -> Result<()> {
        if self.diagnostics.len() >= MAX_DIAGNOSTICS {
            return Err(epub_error("EPUB diagnostics exceed the configured limit"));
        }
        self.diagnostics.push(EpubDiagnostic { path, message });
        Ok(())
    }
}

fn render_styles(styles: &CT_Styles) -> Result<CT_Styles> {
    if styles.styles.len() > MAX_STYLE_ITEMS {
        return Err(epub_error("document has too many styles for EPUB export"));
    }
    let mut key_bytes = 0_usize;
    for style in &styles.styles {
        key_bytes = key_bytes
            .checked_add(style.style_id.len())
            .ok_or_else(|| epub_error("style key size overflow during EPUB export"))?;
        if key_bytes > MAX_PROJECTION_KEY_BYTES {
            return Err(epub_error(
                "document style keys exceed the EPUB projection limit",
            ));
        }
        ensure_xml_value("style identifier", &style.style_id)?;
    }

    let mut projected = CT_Styles::new();
    projected.styles.reserve(styles.styles.len());
    for style in &styles.styles {
        let ppr = style.ppr.as_ref().and_then(|source| {
            source
                .outline_lvl
                .map(|outline_lvl| oxoffice_docx_oxml::properties::CT_PPr {
                    outline_lvl: Some(outline_lvl),
                    ..Default::default()
                })
        });
        projected.styles.push(CT_Style {
            style_id: style.style_id.clone(),
            style_type: style.style_type,
            name: None,
            based_on: None,
            next_style: None,
            linked_style: None,
            auto_redefine: None,
            hidden: None,
            ui_priority: None,
            semi_hidden: None,
            unhide_when_used: None,
            quick_format: None,
            locked: None,
            is_default: style.is_default,
            ppr,
            rpr: None,
            table_properties: None,
            table_properties_original: None,
            table_properties_xml: None,
            table_row_properties: None,
            table_cell_properties: None,
            conditional_table_styles: Vec::new(),
            extra_attributes: Vec::new(),
            modeled_xml: Vec::new(),
            extra_xml: Vec::new(),
        });
    }
    Ok(projected)
}

fn render_numbering(numbering: Option<&CT_Numbering>) -> Result<Option<CT_Numbering>> {
    let Some(numbering) = numbering else {
        return Ok(None);
    };
    let item_count = numbering
        .nums
        .len()
        .checked_add(numbering.abstract_nums.len())
        .and_then(|count| {
            numbering
                .abstract_nums
                .iter()
                .try_fold(count, |count, item| count.checked_add(item.levels.len()))
        })
        .ok_or_else(|| epub_error("numbering item count overflow during EPUB export"))?;
    if item_count > MAX_NUMBERING_ITEMS {
        return Err(epub_error(
            "document has too many numbering items for EPUB export",
        ));
    }

    let nums = numbering
        .nums
        .iter()
        .map(|item| CT_Num {
            num_id: item.num_id,
            abstract_num_id: item.abstract_num_id,
            abstract_num_id_raw: None,
            level_overrides: item.level_overrides.clone(),
            extra_xml: Vec::new(),
            extra_attributes: Vec::new(),
        })
        .collect();
    let abstract_nums = numbering
        .abstract_nums
        .iter()
        .map(|item| CT_AbstractNum {
            abstract_num_id: item.abstract_num_id,
            levels: item
                .levels
                .iter()
                .map(|level| CT_Lvl {
                    ilvl: level.ilvl,
                    template_code: level.template_code.clone(),
                    tentative: level.tentative,
                    start: level.start,
                    start_raw: None,
                    num_fmt: level.num_fmt.clone(),
                    num_fmt_raw: None,
                    restart: level.restart,
                    restart_raw: None,
                    p_style: None,
                    p_style_raw: None,
                    legal: level.legal,
                    legal_raw: None,
                    suffix: level.suffix,
                    suffix_raw: None,
                    lvl_text: None,
                    lvl_text_raw: None,
                    lvl_jc: level.lvl_jc,
                    lvl_jc_raw: None,
                    ppr: None,
                    rpr: None,
                    extra_xml: Vec::new(),
                    extra_attributes: Vec::new(),
                    ppr_raw: None,
                    rpr_raw: None,
                })
                .collect(),
            nsid: None,
            nsid_raw: None,
            multi_level_type: None,
            multi_level_type_raw: None,
            tmpl: None,
            tmpl_raw: None,
            extra_xml: Vec::new(),
            extra_attributes: Vec::new(),
        })
        .collect();
    Ok(Some(CT_Numbering {
        abstract_nums,
        nums,
        root_attributes: Vec::new(),
        extra_xml: Vec::new(),
    }))
}

fn style_has_unprojected_visuals(style: &CT_Style) -> bool {
    let projected_ppr = style.ppr.as_ref().and_then(|properties| {
        properties
            .outline_lvl
            .map(|outline_lvl| oxoffice_docx_oxml::properties::CT_PPr {
                outline_lvl: Some(outline_lvl),
                ..Default::default()
            })
    });
    style.ppr != projected_ppr
        || style
            .rpr
            .as_ref()
            .is_some_and(|properties| !properties.carries_nothing())
        || style.based_on.is_some()
        || style.table_properties.is_some()
        || style.table_properties_original.is_some()
        || style.table_properties_xml.is_some()
        || !style.conditional_table_styles.is_empty()
        || !style.extra_xml.is_empty()
}

fn default_paragraph_style_has_visible_effects(styles: &CT_Styles, paragraph: &CT_P) -> bool {
    let has_text = paragraph.runs.iter().any(|run| {
        run.content.iter().any(|content| match content {
            RunContent::Text(text) | RunContent::DeletedText(text) => !text.text.is_empty(),
            RunContent::Field(field) => !field.cached_result.is_empty(),
            _ => false,
        })
    });
    if styles.doc_defaults.as_ref().is_some_and(|defaults| {
        defaults
            .ppr
            .as_ref()
            .is_some_and(paragraph_properties_have_visible_effects)
            || has_text
                && defaults
                    .rpr
                    .as_ref()
                    .is_some_and(run_properties_have_visible_effects)
    }) {
        return true;
    }
    let Some(mut style) = styles.get_default(StyleType::Paragraph) else {
        return false;
    };
    for _ in 0..=MAX_NESTING_DEPTH {
        if style
            .ppr
            .as_ref()
            .is_some_and(paragraph_properties_have_visible_effects)
            || has_text
                && style
                    .rpr
                    .as_ref()
                    .is_some_and(run_properties_have_visible_effects)
        {
            return true;
        }
        let Some(base) = style
            .based_on
            .as_deref()
            .and_then(|style_id| styles.get_by_id(style_id))
        else {
            return false;
        };
        style = base;
    }
    true
}

fn paragraph_properties_have_visible_effects(
    properties: &oxoffice_docx_oxml::properties::CT_PPr,
) -> bool {
    properties.style_id.is_some()
        || properties.jc.is_some()
        || properties.space_before.is_some()
        || properties.space_after.is_some()
        || properties.line_spacing.is_some()
        || properties.line_rule.is_some()
        || properties.before_autospacing.is_some()
        || properties.after_autospacing.is_some()
        || properties.ind_left.is_some()
        || properties.ind_right.is_some()
        || properties.ind_first_line.is_some()
        || properties.ind_hanging.is_some()
        || properties.keep_next.is_some()
        || properties.keep_lines.is_some()
        || properties.page_break_before.is_some()
        || properties.widow_control.is_some()
        || properties.suppress_auto_hyphens.is_some()
        || properties.outline_lvl.is_some()
        || properties.borders.is_some()
        || properties.tabs.is_some()
        || properties.shading.is_some()
        || properties
            .rpr
            .as_ref()
            .is_some_and(run_properties_have_visible_effects)
        || properties.num_ilvl.is_some()
        || properties.num_id.is_some()
        || properties.sect_pr.is_some()
}

fn run_properties_have_visible_effects(
    properties: &oxoffice_docx_oxml::properties::CT_RPr,
) -> bool {
    properties.style_id.is_some()
        || properties.font_ascii.is_some()
        || properties.font_hansi.is_some()
        || properties.font_east_asia.is_some()
        || properties.font_cs.is_some()
        || properties.font_ascii_theme.is_some()
        || properties.font_hansi_theme.is_some()
        || properties.bold.is_some()
        || properties.bold_cs.is_some()
        || properties.italic.is_some()
        || properties.italic_cs.is_some()
        || properties.underline.is_some()
        || properties.strike.is_some()
        || properties.dstrike.is_some()
        || properties.sz.is_some()
        || properties.sz_cs.is_some()
        || properties.color.is_some()
        || properties.color_theme.is_some()
        || properties.highlight.is_some()
        || properties.caps.is_some()
        || properties.small_caps.is_some()
        || properties.vert_align.is_some()
        || properties.spacing.is_some()
        || properties.width_scale.is_some()
        || properties.position.is_some()
        || properties.shading.is_some()
        || properties.vanish.is_some()
}

fn list_definition<'a>(
    paragraph: &CT_P,
    numbering: Option<&'a CT_Numbering>,
) -> Option<&'a CT_Lvl> {
    let properties = paragraph.properties.as_ref()?;
    let num_id = properties.num_id?;
    if num_id == 0 {
        return None;
    }
    let level = properties.num_ilvl.unwrap_or(0);
    let numbering = numbering?;
    let abstract_id = numbering
        .nums
        .iter()
        .find(|item| item.num_id == num_id)?
        .abstract_num_id;
    numbering
        .abstract_nums
        .iter()
        .find(|item| item.abstract_num_id == abstract_id)?
        .levels
        .iter()
        .find(|item| item.ilvl == level)
}

fn projected_paragraph_text(paragraph: &CT_P) -> String {
    // The text a reader sees, as `Paragraph::text` reads it: simple field
    // results and tracked insertions included.
    paragraph.accepted_text()
}

fn referenced_drawing_ids(content: &[BodyContent]) -> Vec<String> {
    let mut ids = Vec::new();
    visit_accepted_drawings(content, &mut |drawing| {
        if let Some(id) = drawing
            .inline
            .as_ref()
            .map(|image| image.embed_id.as_str())
            .or_else(|| drawing.anchor.as_ref().map(|image| image.embed_id.as_str()))
        {
            ids.push(id.to_owned());
        }
    });
    ids
}

fn referenced_hyperlink_ids(content: &[BodyContent]) -> Vec<String> {
    let mut ids = Vec::new();
    visit_body_paragraphs(content, false, &mut |paragraph| {
        ids.extend(
            paragraph
                .hyperlinks
                .iter()
                .filter_map(|hyperlink| hyperlink.rel_id.clone()),
        );
    });
    ids
}

/// The heading level a paragraph is exported at, before the level-6 clamp:
/// its own outline level, else the shared heading rule (built-in name
/// `heading N`, else id), else its style's outline level.
fn projected_heading_level(paragraph: &CT_P, styles: &CT_Styles) -> Option<u32> {
    let properties = paragraph.properties.as_ref()?;
    if let Some(level) = properties.outline_lvl {
        return level.checked_add(1);
    }
    if let Some(level) = crate::document::paragraph_heading_level(paragraph, styles) {
        return Some(level);
    }
    let style_id = properties.style_id.as_deref()?;
    styles
        .get_by_id(style_id)
        .and_then(|style| style.ppr.as_ref())
        .and_then(|properties| properties.outline_lvl)
        .and_then(|level| level.checked_add(1))
}

/// Whether a list paragraph is exported as a heading rather than a list
/// item: by its own outline level, the shared heading rule (built-in name
/// `heading N`, else id) or its style's outline level. `styles` are the
/// source styles, which keep the names.
fn projects_as_heading(paragraph: &CT_P, styles: &CT_Styles) -> bool {
    let Some(properties) = &paragraph.properties else {
        return false;
    };
    if properties.outline_lvl.is_some_and(|level| level <= 8) {
        return true;
    }
    if crate::document::paragraph_heading_level(paragraph, styles).is_some() {
        return true;
    }
    let Some(style_id) = properties.style_id.as_deref() else {
        return false;
    };
    styles
        .get_by_id(style_id)
        .and_then(|style| style.ppr.as_ref())
        .and_then(|properties| properties.outline_lvl)
        .is_some_and(|level| level <= 8)
}

fn safe_absolute_url(url: &str) -> Option<&str> {
    const SAFE_SCHEMES: &[&str] = &["http", "https", "mailto", "tel", "ftp", "ftps"];
    if url.is_empty()
        || url != url.trim()
        || !url.is_ascii()
        || url.bytes().filter(|byte| *byte == b'#').count() > 1
    {
        return None;
    }
    let separator = url.find(':')?;
    let scheme = &url[..separator];
    if scheme.is_empty()
        || !scheme.as_bytes()[0].is_ascii_alphabetic()
        || !scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
        || !SAFE_SCHEMES
            .iter()
            .any(|allowed| scheme.eq_ignore_ascii_case(allowed))
    {
        return None;
    }
    let remainder = &url[separator + 1..];
    if remainder.is_empty() {
        return None;
    }
    let bytes = url.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' {
            if !bytes
                .get(index + 1..index + 3)
                .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit))
            {
                return None;
            }
            index += 3;
            continue;
        }
        if !(byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b':'
                    | b'/'
                    | b'?'
                    | b'#'
                    | b'['
                    | b']'
                    | b'@'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
            ))
        {
            return None;
        }
        index += 1;
    }
    if matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "ftp" | "ftps"
    ) {
        let authority_and_path = remainder.strip_prefix("//")?;
        let authority = authority_and_path
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default();
        if authority_and_path[authority.len()..].contains(['[', ']']) {
            return None;
        }
        if authority.is_empty()
            || authority.starts_with('.')
            || authority.ends_with('.')
            || authority.contains("..")
            || authority.bytes().filter(|byte| *byte == b'@').count() > 1
        {
            return None;
        }
        let (userinfo, host_port) = authority
            .rsplit_once('@')
            .map_or((None, authority), |(userinfo, host)| (Some(userinfo), host));
        if userinfo.is_some_and(|userinfo| {
            userinfo.is_empty()
                || !userinfo.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric()
                        || matches!(
                            byte,
                            b'-' | b'.'
                                | b'_'
                                | b'~'
                                | b'!'
                                | b'$'
                                | b'&'
                                | b'\''
                                | b'('
                                | b')'
                                | b'*'
                                | b'+'
                                | b','
                                | b';'
                                | b'='
                                | b':'
                                | b'%'
                        )
                })
        }) {
            return None;
        }
        if let Some(host) = host_port.strip_prefix('[') {
            let close = host.find(']')?;
            let literal = &host[..close];
            if !valid_ip_literal(literal) || host[close + 1..].contains(']') {
                return None;
            }
            let suffix = &host[close + 1..];
            if !suffix.is_empty()
                && !suffix.strip_prefix(':').is_some_and(|port| {
                    !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())
                })
            {
                return None;
            }
        } else {
            if host_port.contains(['[', ']']) || host_port.matches(':').count() > 1 {
                return None;
            }
            let (host, port) = host_port
                .rsplit_once(':')
                .map_or((host_port, None), |(host, port)| (host, Some(port)));
            if host.is_empty()
                || port.is_some_and(|port| {
                    port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit())
                })
            {
                return None;
            }
        }
    } else if remainder.contains(['[', ']']) {
        return None;
    }
    Some(url)
}

fn valid_ip_literal(literal: &str) -> bool {
    if literal.parse::<std::net::Ipv6Addr>().is_ok() {
        return true;
    }
    let Some(versioned) = literal
        .strip_prefix('v')
        .or_else(|| literal.strip_prefix('V'))
    else {
        return false;
    };
    let Some((version, address)) = versioned.split_once('.') else {
        return false;
    };
    !version.is_empty()
        && version.bytes().all(|byte| byte.is_ascii_hexdigit())
        && !address.is_empty()
        && address.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'-' | b'.'
                        | b'_'
                        | b'~'
                        | b'!'
                        | b'$'
                        | b'&'
                        | b'\''
                        | b'('
                        | b')'
                        | b'*'
                        | b'+'
                        | b','
                        | b';'
                        | b'='
                        | b':'
                )
        })
}

fn ensure_xml_value(location: &str, value: &str) -> Result<()> {
    if value.chars().all(is_xml_10_character) {
        return Ok(());
    }
    Err(epub_error(format!(
        "{location} contains a character forbidden by XML 1.0"
    )))
}

fn ensure_xml_10(location: &str, xml: &str) -> Result<()> {
    ensure_xml_value(location, xml)
}

fn is_xml_10_character(character: char) -> bool {
    matches!(character, '\u{0009}' | '\u{000A}' | '\u{000D}')
        || ('\u{0020}'..='\u{D7FF}').contains(&character)
        || ('\u{E000}'..='\u{FFFD}').contains(&character)
        || ('\u{10000}'..='\u{10FFFF}').contains(&character)
}

#[derive(Clone, Copy)]
struct RawMarkerKind {
    kind: u8,
}

fn raw_marker_kind(raw: &[u8]) -> Option<RawMarkerKind> {
    let mut reader = quick_xml::Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let name = loop {
        match reader.read_event_into(&mut buffer).ok()? {
            quick_xml::events::Event::Start(element) | quick_xml::events::Event::Empty(element) => {
                break element.name().as_ref().to_vec();
            }
            quick_xml::events::Event::Eof => return None,
            _ => buffer.clear(),
        }
    };
    let local = name
        .iter()
        .rposition(|byte| *byte == b':')
        .map_or(name.as_slice(), |separator| &name[separator + 1..]);
    let kind = match local {
        b"commentRangeStart" => Some(0),
        b"commentRangeEnd" => Some(1),
        b"bookmarkStart" => Some(2),
        b"bookmarkEnd" => Some(3),
        b"ins" => Some(10),
        b"del" => Some(11),
        b"moveFrom" => Some(12),
        b"moveTo" => Some(13),
        _ => None,
    }?;
    Some(RawMarkerKind { kind })
}

fn root_heading_indexes(headings: &[Heading]) -> Vec<usize> {
    let mut roots = Vec::new();
    let mut stack = Vec::<u32>::new();
    for (index, heading) in headings.iter().enumerate() {
        while stack.last().is_some_and(|level| *level >= heading.level) {
            stack.pop();
        }
        if stack.is_empty() {
            roots.push(index);
        }
        stack.push(heading.level);
    }
    roots
}

fn build_nav_tree(headings: &[Heading]) -> Vec<NavNode> {
    fn parse(headings: &[Heading], next: &mut usize, parent_level: u32) -> Vec<NavNode> {
        let mut nodes = Vec::new();
        while *next < headings.len() && headings[*next].level > parent_level {
            let index = *next;
            let level = headings[index].level;
            *next += 1;
            let children = parse(headings, next, level);
            nodes.push(NavNode {
                heading_index: index,
                children,
            });
        }
        nodes
    }
    let mut next = 0;
    parse(headings, &mut next, 0)
}

#[allow(clippy::too_many_arguments)]
fn emit_spine_fragment(
    content: &[BodyContent],
    input: &mut oxoffice_docx_html::HtmlInput,
    source_styles: &oxoffice_docx_oxml::styles::CT_Styles,
    media: &[MediaItem],
    equations: &crate::math::ExportEquations,
    heading_anchors: &HashMap<usize, String>,
    list_values: &HashMap<usize, u32>,
    start: usize,
    end: usize,
) -> Result<String> {
    let mut output = String::new();
    let mut index = start;
    while index < end {
        if matches!(
            content[index],
            BodyContent::ContentControl(_) | BodyContent::RawXml(_)
        ) {
            index += 1;
            continue;
        }
        if let BodyContent::Paragraph(paragraph) = &content[index]
            && let Some(list) = detect_list(paragraph, source_styles, input.numbering.as_ref())
        {
            emit_list_level(
                &mut output,
                content,
                input,
                source_styles,
                media,
                equations,
                heading_anchors,
                list_values,
                &mut index,
                end,
                list,
                0,
            )?;
            continue;
        }
        let anchor = heading_anchors.get(&index).map(String::as_str);
        let fragment = render_source_block(
            input,
            source_styles,
            &content[index],
            media,
            equations,
            anchor,
            false,
        )?;
        push_bounded_xhtml(&mut output, &fragment)?;
        index += 1;
    }
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
fn emit_list_level(
    output: &mut String,
    content: &[BodyContent],
    input: &mut oxoffice_docx_html::HtmlInput,
    source_styles: &oxoffice_docx_oxml::styles::CT_Styles,
    media: &[MediaItem],
    equations: &crate::math::ExportEquations,
    heading_anchors: &HashMap<usize, String>,
    list_values: &HashMap<usize, u32>,
    index: &mut usize,
    end: usize,
    list: ListInfo,
    depth: usize,
) -> Result<()> {
    if depth >= MAX_NESTING_DEPTH {
        return Err(epub_error("list nesting exceeds the EPUB depth limit"));
    }
    let tag = list.kind.tag();
    let effective_start = list_values.get(index).copied().unwrap_or(list.start);
    let mut expected_value = effective_start;
    let mut opening = format!("<{tag}");
    if list.kind == ListKind::None {
        opening.push_str(" class=\"no-marker\"");
    }
    if list.kind == ListKind::Ordered && effective_start != 1 {
        write!(opening, " start=\"{effective_start}\"")
            .map_err(|_| epub_error("failed to format EPUB list start"))?;
    }
    if let Some(marker_style) = list.marker_style {
        write!(opening, " style=\"list-style-type:{marker_style}\"")
            .map_err(|_| epub_error("failed to format EPUB list marker style"))?;
    }
    opening.push_str(">\n");
    push_bounded_xhtml(output, &opening)?;
    let mut item_open = false;

    while *index < end {
        let BodyContent::Paragraph(paragraph) = &content[*index] else {
            break;
        };
        let Some(item) = detect_list(paragraph, source_styles, input.numbering.as_ref()) else {
            break;
        };
        if item.level < list.level
            || (item.level == list.level && (item.num_id != list.num_id || item.kind != list.kind))
        {
            break;
        }
        if item.level > list.level {
            if !item_open || item.num_id != list.num_id {
                break;
            }
            emit_list_level(
                output,
                content,
                input,
                source_styles,
                media,
                equations,
                heading_anchors,
                list_values,
                index,
                end,
                item,
                depth + 1,
            )?;
            continue;
        }

        if item_open {
            push_bounded_xhtml(output, "</li>\n")?;
        }
        let anchor = heading_anchors.get(index).map(String::as_str);
        let preserve_heading = projects_as_heading(paragraph, source_styles);
        let value = list_values.get(index).copied().unwrap_or(expected_value);
        let marker = list
            .item_markers
            .then(|| effective_list_level(paragraph, source_styles, input.numbering.as_ref()))
            .flatten()
            .map(|(_, _, level)| item_marker(level, value));
        // The HTML delegate reads only direct numPr. Materialize the already
        // selected source list identity on this detached paragraph projection,
        // because projected styles intentionally omit numbering inheritance.
        // The original paragraph and the selected counter state stay untouched.
        let mut projected_paragraph = paragraph.clone();
        let properties = projected_paragraph.properties.get_or_insert_default();
        properties.num_id = Some(item.num_id);
        properties.num_ilvl = Some(item.level);
        let fragment = render_source_block(
            input,
            source_styles,
            &BodyContent::Paragraph(projected_paragraph),
            media,
            equations,
            if preserve_heading { anchor } else { None },
            preserve_heading,
        )?;
        // The item keeps the direction and the shading the HTML writer gives
        // it, its marker and number are the EPUB list's.
        let (attributes, inner) = if preserve_heading {
            ("", fragment.as_str())
        } else {
            list_item_parts(&fragment)?
        };
        let mut item = String::from("<li");
        if attributes.contains(" dir=\"rtl\"") {
            item.push_str(" dir=\"rtl\"");
        }
        let declarations = [
            marker.map(|marker| format!("list-style-type:{}", css_string(&format!("{marker} ")))),
            item_background(attributes).map(|color| format!("background-color:{color}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        if list.kind == ListKind::Ordered && !list.item_markers && value != expected_value {
            write!(item, " value=\"{value}\"")
                .map_err(|_| epub_error("failed to format an EPUB list item"))?;
        }
        if !declarations.is_empty() {
            write!(item, " style=\"{}\"", escape_xml(&declarations.join(";")))
                .map_err(|_| epub_error("failed to format an EPUB list item"))?;
        }
        item.push('>');
        push_bounded_xhtml(output, &item)?;
        push_bounded_xhtml(output, inner)?;
        item_open = true;
        if list.kind == ListKind::Ordered {
            expected_value = value
                .checked_add(1)
                .ok_or_else(|| epub_error("list counter overflow during EPUB export"))?;
        }
        *index += 1;
    }

    if item_open {
        push_bounded_xhtml(output, "</li>\n")?;
    }
    push_bounded_xhtml(output, &format!("</{tag}>\n"))?;
    Ok(())
}

fn render_source_block(
    input: &mut oxoffice_docx_html::HtmlInput,
    source_styles: &oxoffice_docx_oxml::styles::CT_Styles,
    content: &BodyContent,
    media: &[MediaItem],
    equations: &crate::math::ExportEquations,
    anchor: Option<&str>,
    suppress_numbering: bool,
) -> Result<String> {
    let image_occurrences = supported_image_occurrences(content, input);
    let mut content = render_body_projection(content)
        .ok_or_else(|| epub_error("unsupported source block reached EPUB projection"))?;
    // The HTML writer reads the heading level from a `HeadingN` id: give it
    // the level the shared rule reads in the source style, whatever its
    // localised id, at most 6.
    if let BodyContent::Paragraph(paragraph) = &mut content
        && let Some(level) = crate::document::paragraph_heading_level(paragraph, source_styles)
    {
        paragraph.properties.get_or_insert_default().style_id =
            Some(format!("Heading{}", level.min(6)));
    }
    if suppress_numbering && let BodyContent::Paragraph(paragraph) = &mut content {
        let properties = paragraph.properties.get_or_insert_default();
        properties.num_id = None;
        properties.num_ilvl = None;
    }
    input.document.body.content = vec![content];
    input.document.body.sect_pr = None;
    let fragment = oxoffice_docx_html::to_html_fragment_with_equations(
        input,
        &oxoffice_docx_html::HtmlOptions {
            inline_images: true,
        },
        &HashMap::new(),
        equations,
    );
    let fragment = replace_media_sources(&fragment, input, media, &image_occurrences)?;
    let fragment = lift_page_breaks(&fragment)?;
    let mut fragment = normalize_xhtml(fragment);
    if let Some(anchor) = anchor {
        inject_first_source_anchor(&mut fragment, anchor)?;
    }
    if fragment.len() > MAX_EPUB_BYTES / 2 {
        return Err(epub_error(
            "generated source block exceeds the EPUB intermediate limit",
        ));
    }
    Ok(fragment)
}

/// The body with each block content control replaced by the paragraphs and
/// tables it wraps, nested controls included, so that a wrapped heading, list
/// item or table is exported as if the control were not there. It clones what
/// the controls wrap, so it runs after the source limits are checked.
fn unwrap_block_controls(content: &[BodyContent]) -> Cow<'_, [BodyContent]> {
    if !content
        .iter()
        .any(|item| matches!(item, BodyContent::ContentControl(_)))
    {
        return Cow::Borrowed(content);
    }
    let mut unwrapped = Vec::with_capacity(content.len());
    for item in content {
        match item {
            BodyContent::ContentControl(control) => push_control_blocks(control, &mut unwrapped),
            item => unwrapped.push(item.clone()),
        }
    }
    Cow::Owned(unwrapped)
}

fn push_control_blocks(control: &CT_Sdt, blocks: &mut Vec<BodyContent>) {
    for item in &control.content {
        match item {
            SdtContent::Paragraph(paragraph) => {
                blocks.push(BodyContent::Paragraph(Box::new(paragraph.clone())))
            }
            SdtContent::Table(table) => blocks.push(BodyContent::Table(Box::new(table.clone()))),
            SdtContent::ContentControl(nested) => push_control_blocks(nested, blocks),
            SdtContent::Row(_)
            | SdtContent::Cell(_)
            | SdtContent::Run(_)
            | SdtContent::RawXml(_) => {}
        }
    }
}

fn render_body_projection(content: &BodyContent) -> Option<BodyContent> {
    match content {
        BodyContent::Paragraph(paragraph) => Some(BodyContent::Paragraph(Box::new(
            render_paragraph_projection(paragraph),
        ))),
        BodyContent::Table(table) => {
            Some(BodyContent::Table(Box::new(render_table_projection(table))))
        }
        BodyContent::ContentControl(_) | BodyContent::RawXml(_) => None,
    }
}

fn render_paragraph_projection(paragraph: &CT_P) -> CT_P {
    let mut properties = paragraph
        .properties
        .as_ref()
        .map(render_paragraph_properties);
    if properties
        .as_ref()
        .and_then(|properties| properties.outline_lvl)
        .is_some_and(|level| level >= 6)
    {
        let properties = properties.get_or_insert_default();
        properties.outline_lvl = Some(5);
        properties.style_id = Some("Heading6".to_owned());
    }
    // The runs the text readers read, those of content controls, tracked
    // insertions, smart tags and custom XML included, deleted ones left out.
    let view = paragraph.accepted_view();
    CT_P {
        properties,
        runs: view.runs.iter().map(render_run_projection).collect(),
        hyperlinks: view
            .hyperlinks
            .iter()
            .map(|hyperlink| HyperlinkSpan {
                rel_id: hyperlink.rel_id.clone(),
                // The bookmarks are not projected and a chapter split would
                // part a link from its target: the anchor is dropped, as the
                // scan reports, and the run stays plain text.
                anchor: None,
                tooltip: hyperlink.tooltip.clone(),
                doc_location: hyperlink.doc_location.clone(),
                run_start: hyperlink.run_start,
                run_end: hyperlink.run_end,
                extra_attributes: Vec::new(),
                extra_xml: Vec::new(),
                preserved_raw_before: None,
            })
            .collect(),
        comment_ranges: Vec::new(),
        bookmark_markers: Vec::new(),
        extra_xml: Vec::new(),
        content_controls: Vec::new(),
        revisions: Vec::new(),
        // Equations stay at their run boundary in the accepted view; the
        // HTML delegate writes them as MathML.
        equations: view.equations.clone(),
        // The base runs are already projected above. The phonetic line is an
        // annotation with no EPUB spelling, so it is dropped rather than
        // doubling the base text.
        rubies: Vec::new(),
    }
}

fn render_paragraph_properties(
    source: &oxoffice_docx_oxml::properties::CT_PPr,
) -> oxoffice_docx_oxml::properties::CT_PPr {
    oxoffice_docx_oxml::properties::CT_PPr {
        style_id: source.style_id.clone(),
        jc: source.jc,
        space_before: source.space_before,
        space_after: source.space_after,
        line_spacing: source.line_spacing,
        line_rule: source.line_rule.clone(),
        ind_left: source.ind_left,
        ind_right: source.ind_right,
        ind_first_line: source.ind_first_line,
        ind_hanging: source.ind_hanging,
        outline_lvl: source.outline_lvl,
        shading: source
            .shading
            .as_ref()
            .map(|shading| Box::new(render_shading_projection(shading))),
        num_ilvl: source.num_ilvl,
        num_id: source.num_id,
        ..Default::default()
    }
}

fn render_run_projection(run: &CT_R) -> CT_R {
    CT_R {
        properties: run.properties.as_ref().map(render_run_properties),
        content: run.content.iter().map(render_run_content).collect(),
        extra_xml: Vec::new(),
        extra_xml_positions: Vec::new(),
        alt_drawings: Vec::new(),
    }
}

fn render_run_properties(
    source: &oxoffice_docx_oxml::properties::CT_RPr,
) -> oxoffice_docx_oxml::properties::CT_RPr {
    oxoffice_docx_oxml::properties::CT_RPr {
        font_ascii: source.font_ascii.clone(),
        bold: source.bold,
        italic: source.italic,
        underline: source.underline.and_then(|underline| match underline {
            oxoffice_docx_oxml::shared::ST_Underline::None => None,
            _ => Some(oxoffice_docx_oxml::shared::ST_Underline::Single),
        }),
        strike: source.strike,
        dstrike: source.dstrike,
        sz: source.sz,
        color: source.color.clone(),
        highlight: source.highlight,
        caps: source.caps,
        small_caps: source.small_caps,
        vert_align: source.vert_align.clone(),
        spacing: source.spacing,
        shading: source
            .shading
            .as_ref()
            .map(|shading| Box::new(render_shading_projection(shading))),
        ..Default::default()
    }
}

fn render_shading_projection(
    source: &oxoffice_docx_oxml::properties::CT_Shd,
) -> oxoffice_docx_oxml::properties::CT_Shd {
    // The pattern and both colours: the HTML writer paints what layout
    // paints from them.
    oxoffice_docx_oxml::properties::CT_Shd {
        val: source.val.clone(),
        color: source.color.clone(),
        fill: source.fill.clone(),
        ..Default::default()
    }
}

fn render_run_content(content: &RunContent) -> RunContent {
    match content {
        RunContent::Text(text) => RunContent::Text(text.clone()),
        RunContent::DeletedText(text) => RunContent::DeletedText(text.clone()),
        RunContent::Tab => RunContent::Tab,
        // XHTML gets a plain break: EPUB readers have no floats to clear.
        RunContent::Break(kind, _) => RunContent::Break(*kind, BreakClear::None),
        RunContent::Drawing(drawing) => RunContent::Drawing(render_drawing_projection(drawing)),
        RunContent::Field(field) => {
            let mut projected = Field::new("", &field.cached_result);
            projected.dirty = field.dirty;
            RunContent::Field(projected)
        }
        RunContent::FootnoteRef { id, custom_mark } => RunContent::FootnoteRef {
            id: *id,
            custom_mark: custom_mark.clone(),
        },
        RunContent::EndnoteRef { id, custom_mark } => RunContent::EndnoteRef {
            id: *id,
            custom_mark: custom_mark.clone(),
        },
        RunContent::CommentReference { id, .. } => RunContent::CommentReference {
            id: *id,
            raw_before: 0,
        },
        RunContent::Symbol { font, char_code } => RunContent::Symbol {
            font: font.clone(),
            char_code: *char_code,
        },
        RunContent::SpecialCharacter(character) => RunContent::SpecialCharacter(*character),
    }
}

fn render_drawing_projection(drawing: &CT_Drawing) -> CT_Drawing {
    let source = drawing.inline.as_ref().map(|inline| {
        (
            inline.extent_cx,
            inline.extent_cy,
            inline.embed_id.as_str(),
            inline.chart_rel_id.as_ref(),
            inline.description.as_ref(),
        )
    });
    let source = source.or_else(|| {
        drawing.anchor.as_ref().map(|anchor| {
            (
                anchor.extent_cx,
                anchor.extent_cy,
                anchor.embed_id.as_str(),
                anchor.chart_rel_id.as_ref(),
                anchor.description.as_ref(),
            )
        })
    });
    let Some((extent_cx, extent_cy, embed_id, chart_rel_id, description)) = source else {
        return CT_Drawing {
            inline: None,
            anchor: None,
        };
    };
    CT_Drawing::inline(CT_Inline {
        doc_pr_id: drawing
            .inline
            .as_ref()
            .map(|inline| inline.doc_pr_id)
            .or_else(|| drawing.anchor.as_ref().map(|anchor| anchor.doc_pr_id))
            .unwrap_or(1),
        extent_cx,
        extent_cy,
        embed_id: embed_id.to_owned(),
        link_id: None,
        chart_rel_id: chart_rel_id.cloned(),
        description: description.cloned(),
        title: drawing
            .inline
            .as_ref()
            .and_then(|inline| inline.title.clone())
            .or_else(|| {
                drawing
                    .anchor
                    .as_ref()
                    .and_then(|anchor| anchor.title.clone())
            }),
        decorative: drawing
            .inline
            .as_ref()
            .map(|inline| inline.decorative)
            .or_else(|| drawing.anchor.as_ref().map(|anchor| anchor.decorative))
            .unwrap_or(false),
        name: None,
        source_rect: None,
        raw_xml: None,
        doc_pr_id_unallocated: false,
    })
}

fn render_table_projection(table: &CT_Tbl) -> CT_Tbl {
    CT_Tbl {
        properties: table
            .properties
            .as_ref()
            .map(|source| oxoffice_docx_oxml::table::CT_TblPr {
                jc: source.jc,
                borders: source.borders.clone(),
                shading: source.shading.as_ref().map(render_shading_projection),
                ..Default::default()
            }),
        grid: None,
        // Rows and cells that content controls wrap are exported in place.
        rows: table
            .rows()
            .into_iter()
            .map(|row| CT_Row {
                table_property_exception: None,
                properties: None,
                cells: row
                    .cells()
                    .into_iter()
                    .map(|cell| CT_Tc {
                        properties: cell.properties.as_ref().map(|source| {
                            oxoffice_docx_oxml::table::CT_TcPr {
                                grid_span: source.grid_span,
                                v_merge: source.v_merge,
                                borders: source.borders.clone(),
                                shading: source.shading.as_ref().map(render_shading_projection),
                                v_align: source.v_align,
                                ..Default::default()
                            }
                        }),
                        content: render_cell_content_projection(&cell.content),
                        extra_xml: Vec::new(),
                    })
                    .collect(),
                extra_xml: Vec::new(),
                content_controls: Vec::new(),
            })
            .collect(),
        extra_xml: Vec::new(),
        content_controls: Vec::new(),
    }
}

/// Project the paragraphs and nested tables of a table cell, with what
/// content controls wrap in place.
fn render_cell_content_projection(content: &[CellContent]) -> Vec<CellContent> {
    let mut projected = Vec::with_capacity(content.len());
    for item in content {
        match item {
            CellContent::Paragraph(paragraph) => projected.push(CellContent::Paragraph(
                render_paragraph_projection(paragraph),
            )),
            CellContent::Table(table) => {
                projected.push(CellContent::Table(render_table_projection(table)))
            }
            CellContent::ContentControl(control) => {
                push_control_cell_projection(control, &mut projected)
            }
        }
    }
    projected
}

fn push_control_cell_projection(control: &CT_Sdt, projected: &mut Vec<CellContent>) {
    for item in &control.content {
        match item {
            SdtContent::Paragraph(paragraph) => projected.push(CellContent::Paragraph(
                render_paragraph_projection(paragraph),
            )),
            SdtContent::Table(table) => {
                projected.push(CellContent::Table(render_table_projection(table)))
            }
            SdtContent::ContentControl(nested) => push_control_cell_projection(nested, projected),
            SdtContent::Row(_)
            | SdtContent::Cell(_)
            | SdtContent::Run(_)
            | SdtContent::RawXml(_) => {}
        }
    }
}

fn inject_first_source_anchor(fragment: &mut String, anchor: &str) -> Result<()> {
    let Some(start) = fragment.find('<') else {
        return Err(epub_error("heading projection produced no source element"));
    };
    let Some(relative_end) = fragment[start..].find([' ', '>']) else {
        return Err(epub_error(
            "heading projection produced a malformed source element",
        ));
    };
    let insertion = start + relative_end;
    fragment.insert_str(insertion, &format!(" id=\"{anchor}\""));
    Ok(())
}

/// The attributes and the content of the list item the HTML writer made of
/// a list paragraph.
fn list_item_parts(fragment: &str) -> Result<(&str, &str)> {
    let no_item = || epub_error("list paragraph projection produced no list item");
    let open = fragment
        .match_indices("<li")
        .map(|(at, _)| at + "<li".len())
        .find(|at| matches!(fragment.as_bytes().get(*at), Some(b' ' | b'>')))
        .ok_or_else(no_item)?;
    let start = fragment[open..]
        .find('>')
        .map(|end| open + end)
        .ok_or_else(no_item)?;
    let end = fragment[start..]
        .find("</li>")
        .map(|end| start + end)
        .ok_or_else(|| epub_error("list paragraph projection left its list item open"))?;
    Ok((&fragment[open..start], &fragment[start + 1..end]))
}

/// The `background-color` a list item's HTML attributes declare.
fn item_background(attributes: &str) -> Option<&str> {
    let value = attributes.split("background-color:").nth(1)?;
    let end = value.find([';', '"']).unwrap_or(value.len());
    Some(&value[..end])
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ListKind {
    Ordered,
    Unordered,
    None,
}

impl ListKind {
    fn tag(self) -> &'static str {
        if self == Self::Ordered { "ol" } else { "ul" }
    }
}

#[derive(Clone, Copy)]
struct ListInfo {
    num_id: u32,
    level: u32,
    kind: ListKind,
    start: u32,
    marker_style: Option<&'static str>,
    /// Whether each item writes its own marker text: a format CSS does not
    /// name (Chicago's).
    item_markers: bool,
}

// Use the same selected native counter state once in source order, rather
// than restarting a second numbering algorithm at each XHTML list/spine.
fn effective_epub_list_properties(paragraph: &CT_P, styles: &CT_Styles) -> CT_PPr {
    let direct = paragraph.properties.as_ref();
    let mut properties = oxoffice_docx_layout::style_resolver::resolve_paragraph_properties(
        direct.and_then(|value| value.style_id.as_deref()),
        styles,
    );
    if let Some(direct) = direct {
        properties.merge_from(direct);
    }
    properties
}

fn selected_epub_list_values(
    content: &[BodyContent],
    styles: &CT_Styles,
    numbering: Option<&CT_Numbering>,
) -> HashMap<usize, u32> {
    let mut values = HashMap::new();
    let Some(numbering) = numbering else {
        return values;
    };
    let mut state = oxoffice_docx_layout::style_resolver::NumberingState::new();
    for (index, block) in content.iter().enumerate() {
        let mut advance = |paragraph: &CT_P, properties: CT_PPr| {
            if let Some(num_id) = properties.num_id.filter(|value| *value != 0) {
                let level = properties.num_ilvl.unwrap_or(0);
                oxoffice_docx_layout::style_resolver::generate_marker(
                    num_id, level, numbering, &mut state,
                );
                if matches!(block, BodyContent::Paragraph(root) if std::ptr::eq(root.as_ref(), paragraph))
                {
                    values.insert(index, state.current(num_id, level));
                }
            }
            if properties.sect_pr.is_some() {
                state.restart_after_section_break(numbering);
            }
        };
        if let BodyContent::Table(table) = block {
            // Reuse actual cell styles and accepted native source selection,
            // even where EPUB intentionally flattens the visual table lists.
            for (paragraph, properties) in
                oxoffice_docx_layout::table::effective_table_paragraph_properties(table, styles)
            {
                advance(paragraph, properties);
            }
        } else {
            visit_body_paragraphs(std::slice::from_ref(block), true, &mut |paragraph| {
                advance(paragraph, effective_epub_list_properties(paragraph, styles));
            });
        }
    }
    values
}

fn detect_list(
    paragraph: &CT_P,
    styles: &CT_Styles,
    numbering: Option<&CT_Numbering>,
) -> Option<ListInfo> {
    let (num_id, level, definition) = effective_list_level(paragraph, styles, numbering)?;
    let (kind, marker_style) = epub_list_semantics(definition.num_fmt.as_ref())?;
    Some(ListInfo {
        num_id,
        level,
        kind,
        start: definition.start.unwrap_or(1),
        marker_style,
        // Legal numbering shows Chicago levels as decimal numbers.
        item_markers: definition.num_fmt == Some(ST_NumberFormat::Chicago)
            && definition.legal != Some(true),
    })
}

/// The list a paragraph is on, its style's included, and the level it
/// uses, a level override of its instance applied.
fn effective_list_level<'a>(
    paragraph: &CT_P,
    styles: &CT_Styles,
    numbering: Option<&'a CT_Numbering>,
) -> Option<(u32, u32, &'a CT_Lvl)> {
    let properties = effective_epub_list_properties(paragraph, styles);
    let num_id = properties.num_id?;
    let level = properties.num_ilvl.unwrap_or(0);
    if num_id == 0 {
        return None;
    }
    let numbering = numbering?;
    let abstract_id = numbering
        .nums
        .iter()
        .find(|numbering| numbering.num_id == num_id)?
        .abstract_num_id;
    let abstract_numbering = numbering
        .abstract_nums
        .iter()
        .find(|numbering| numbering.abstract_num_id == abstract_id)?;
    let base = abstract_numbering
        .levels
        .iter()
        .find(|definition| definition.ilvl == level)?;
    let definition = numbering
        .nums
        .iter()
        .find(|instance| instance.num_id == num_id)?
        .level_overrides
        .iter()
        .find(|value| value.ilvl == level)
        .and_then(|value| value.level.as_ref())
        .unwrap_or(base);
    Some((num_id, level, definition))
}

/// The marker layout draws for item `value` of a level whose format CSS
/// does not name: its level text with the level's own number in that
/// format, and with no other level's number (a Chicago level is read
/// alone).
fn item_marker(level: &CT_Lvl, value: u32) -> String {
    let own = format!("%{}", level.ilvl + 1);
    let number = oxoffice_docx_layout::style_resolver::format_number(
        value,
        level.num_fmt.clone().unwrap_or(ST_NumberFormat::Decimal),
    );
    let text = level
        .lvl_text
        .as_deref()
        .unwrap_or("%1.")
        .replace(&own, &number);
    (1..=9).fold(text, |text, other| text.replace(&format!("%{other}"), ""))
}

/// `text` as a CSS string: quoted, its quote and backslash escaped, a
/// control character left out.
fn css_string(text: &str) -> String {
    let mut out = String::from("'");
    for character in text.chars().filter(|character| !character.is_control()) {
        if matches!(character, '\'' | '\\') {
            out.push('\\');
        }
        out.push(character);
    }
    out.push('\'');
    out
}

fn epub_list_semantics(
    format: Option<&ST_NumberFormat>,
) -> Option<(ListKind, Option<&'static str>)> {
    match format {
        Some(ST_NumberFormat::Bullet) => Some((ListKind::Unordered, None)),
        Some(ST_NumberFormat::None) => Some((ListKind::None, None)),
        None | Some(ST_NumberFormat::Decimal | ST_NumberFormat::Ordinal) => {
            Some((ListKind::Ordered, None))
        }
        Some(ST_NumberFormat::UpperRoman) => Some((ListKind::Ordered, Some("upper-roman"))),
        Some(ST_NumberFormat::LowerRoman) => Some((ListKind::Ordered, Some("lower-roman"))),
        Some(ST_NumberFormat::UpperLetter) => Some((ListKind::Ordered, Some("upper-alpha"))),
        Some(ST_NumberFormat::LowerLetter) => Some((ListKind::Ordered, Some("lower-alpha"))),
        // Each item writes its marker (`item_marker`).
        Some(ST_NumberFormat::Chicago) => Some((ListKind::Ordered, None)),
        Some(_) => None,
    }
}

fn push_bounded_xhtml(output: &mut String, value: &str) -> Result<()> {
    let next = output
        .len()
        .checked_add(value.len())
        .ok_or_else(|| epub_error("XHTML size overflow during EPUB export"))?;
    if next > MAX_EPUB_BYTES / 2 {
        return Err(epub_error(
            "generated XHTML exceeds the EPUB intermediate limit",
        ));
    }
    output.push_str(value);
    Ok(())
}

struct ImageOccurrence {
    relationship_id: String,
    description: String,
}

fn supported_image_occurrences(
    content: &BodyContent,
    input: &oxoffice_docx_html::HtmlInput,
) -> Vec<ImageOccurrence> {
    // The emitter writes the pictures of the projected block, that is of the
    // accepted view of its paragraphs, in the order this visitor reaches them.
    let mut occurrences = Vec::new();
    visit_accepted_drawings(std::slice::from_ref(content), &mut |drawing| {
        let source = drawing
            .inline
            .as_ref()
            .map(|image| (image.embed_id.as_str(), image.description.as_deref()))
            .or_else(|| {
                drawing
                    .anchor
                    .as_ref()
                    .map(|image| (image.embed_id.as_str(), image.description.as_deref()))
            });
        let Some((relationship_id, description)) = source else {
            return;
        };
        if input.images.contains_key(relationship_id) {
            occurrences.push(ImageOccurrence {
                relationship_id: relationship_id.to_owned(),
                description: description.unwrap_or_default().to_owned(),
            });
        }
    });
    occurrences
}

fn replace_media_sources(
    fragment: &str,
    input: &oxoffice_docx_html::HtmlInput,
    media: &[MediaItem],
    occurrences: &[ImageOccurrence],
) -> Result<String> {
    let mut output = String::with_capacity(fragment.len());
    let mut remaining = fragment;
    let mut occurrence_index = 0;
    while let Some(start) = remaining.find("<img ") {
        output.push_str(&remaining[..start]);
        let tag = &remaining[start..];
        let end = tag
            .find('>')
            .ok_or_else(|| epub_error("image projection produced an unterminated tag"))?;
        let tag = &tag[..=end];
        let value_start = tag
            .find(" src=\"")
            .map(|offset| offset + " src=\"".len())
            .ok_or_else(|| epub_error("image projection produced no source attribute"))?;
        let value_end = tag[value_start..]
            .find('"')
            .map(|offset| value_start + offset)
            .ok_or_else(|| epub_error("image projection produced an open source attribute"))?;
        let source = &tag[value_start..value_end];
        let relationship_id = input.images.iter().find_map(|(relationship_id, image)| {
            let marker = format!(
                "data:{};base64,{}",
                image.content_type,
                base64_encode(&image.data)
            );
            (marker == source).then_some(relationship_id)
        });
        let relationship_id = relationship_id
            .ok_or_else(|| epub_error("image projection produced an unknown source marker"))?;
        let occurrence = occurrences
            .get(occurrence_index)
            .filter(|occurrence| occurrence.relationship_id == *relationship_id)
            .ok_or_else(|| epub_error("image projection lost source occurrence correlation"))?;
        occurrence_index += 1;
        let item = media
            .iter()
            .find(|item| item.relationship_id == *relationship_id)
            .ok_or_else(|| epub_error("image projection lost its packaged media item"))?;
        let attribute_start = value_start - "src=\"".len();
        output.push_str(&tag[..attribute_start]);
        write!(
            output,
            "alt=\"{}\" src=\"",
            escape_xml(&occurrence.description)
        )
        .map_err(|_| epub_error("failed to format EPUB image alternative text"))?;
        output.push_str(&item.href);
        // The HTML emitter writes a described picture's alt text right after
        // `src`; EPUB has written its own before it.
        let rest = &tag[value_end..];
        match rest
            .strip_prefix("\" alt=\"")
            .and_then(|after| after.find('"').map(|end| &after[end + 1..]))
        {
            Some(after_alt) => {
                output.push('"');
                output.push_str(after_alt);
            }
            None => output.push_str(rest),
        }
        remaining = &remaining[start + tag.len()..];
    }
    output.push_str(remaining);
    if occurrence_index != occurrences.len() {
        return Err(epub_error(
            "image projection omitted a supported source occurrence",
        ));
    }
    Ok(output)
}

#[derive(Clone)]
struct OpenXhtmlTag {
    name: String,
    opening: String,
}

fn lift_page_breaks(fragment: &str) -> Result<String> {
    let mut output = String::with_capacity(fragment.len());
    let mut stack = Vec::<OpenXhtmlTag>::new();
    let mut remaining = fragment;
    while let Some(start) = remaining.find('<') {
        output.push_str(&remaining[..start]);
        let tag_and_rest = &remaining[start..];
        let end = tag_and_rest
            .find('>')
            .ok_or_else(|| epub_error("HTML projection produced an unterminated tag"))?;
        let tag = &tag_and_rest[..=end];
        let name = xhtml_tag_name(tag)
            .ok_or_else(|| epub_error("HTML projection produced a malformed tag"))?;
        let closing = tag.as_bytes().get(1) == Some(&b'/');
        let void = tag.ends_with("/>") || matches!(name, "br" | "hr" | "img" | "input");

        if name == "hr" && !closing {
            let keep = stack
                .iter()
                .rposition(|entry| flow_container(&entry.name))
                .map_or(0, |index| index + 1);
            for entry in stack[keep..].iter().rev() {
                write!(output, "</{}>", entry.name)
                    .map_err(|_| epub_error("failed to close XHTML around a page break"))?;
            }
            output.push_str(tag);
            for entry in &stack[keep..] {
                output.push_str(&opening_without_id(&entry.opening));
            }
        } else {
            output.push_str(tag);
            if closing {
                if stack.last().is_none_or(|entry| entry.name != name) {
                    return Err(epub_error("HTML projection produced mismatched XHTML tags"));
                }
                stack.pop();
            } else if !void {
                stack.push(OpenXhtmlTag {
                    name: name.to_owned(),
                    opening: tag.to_owned(),
                });
            }
        }
        remaining = &tag_and_rest[end + 1..];
    }
    output.push_str(remaining);
    if !stack.is_empty() {
        return Err(epub_error("HTML projection left XHTML tags open"));
    }
    Ok(output)
}

fn xhtml_tag_name(tag: &str) -> Option<&str> {
    let content = tag.strip_prefix('<')?;
    let content = content.strip_prefix('/').unwrap_or(content);
    let end = content
        .find(|character: char| character.is_ascii_whitespace() || matches!(character, '/' | '>'))
        .unwrap_or(content.len());
    (end > 0).then_some(&content[..end])
}

fn flow_container(name: &str) -> bool {
    matches!(
        name,
        "article"
            | "aside"
            | "blockquote"
            | "caption"
            | "dd"
            | "div"
            | "dt"
            | "figcaption"
            | "figure"
            | "footer"
            | "header"
            | "li"
            | "main"
            | "nav"
            | "section"
            | "td"
            | "th"
    )
}

fn opening_without_id(opening: &str) -> String {
    let Some(start) = opening.find(" id=\"") else {
        return opening.to_owned();
    };
    let value_start = start + " id=\"".len();
    let Some(end) = opening[value_start..].find('"') else {
        return opening.to_owned();
    };
    let mut reopened = String::with_capacity(opening.len());
    reopened.push_str(&opening[..start]);
    reopened.push_str(&opening[value_start + end + 1..]);
    reopened
}

fn normalize_xhtml(fragment: String) -> String {
    fragment
        .replace("&emsp;", "&#8195;")
        .replace("<br>", "<br/>")
        .replace("<hr>", "<hr/>")
        .replace(" style=\"max-width:100%\">", " style=\"max-width:100%\"/>")
}

fn xhtml_document(title: &str, fragment: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"en\" lang=\"en\">\n<head><title>{}</title><link rel=\"stylesheet\" type=\"text/css\" href=\"styles.css\"/></head>\n<body>\n{fragment}</body>\n</html>\n",
        escape_xml(title)
    )
}

fn navigation_document(
    title: &str,
    headings: &[Heading],
    tree: &[NavNode],
    spine: &[SpineItem],
) -> String {
    let mut list = String::new();
    if tree.is_empty() {
        let href = spine
            .first()
            .map(|item| item.href.as_str())
            .unwrap_or("document.xhtml");
        let _ = write!(
            list,
            "<ol><li><a href=\"{}\">{}</a></li></ol>",
            escape_xml(href),
            escape_xml(title)
        );
    } else {
        write_nav_nodes(&mut list, headings, tree);
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"en\" lang=\"en\">\n<head><title>{}</title><link rel=\"stylesheet\" type=\"text/css\" href=\"styles.css\"/></head>\n<body><nav epub:type=\"toc\" id=\"toc\"><h1>Contents</h1>{list}</nav></body>\n</html>\n",
        escape_xml(title)
    )
}

fn write_nav_nodes(output: &mut String, headings: &[Heading], nodes: &[NavNode]) {
    output.push_str("<ol>");
    for node in nodes {
        let heading = &headings[node.heading_index];
        let _ = write!(
            output,
            "<li><a href=\"{}\">{}</a>",
            escape_xml(&heading.href),
            escape_xml(&heading.text)
        );
        if !node.children.is_empty() {
            write_nav_nodes(output, headings, &node.children);
        }
        output.push_str("</li>");
    }
    output.push_str("</ol>");
}

fn package_document(
    title: &str,
    author: &str,
    identifier: &str,
    spine: &[SpineItem],
    media: &[MediaItem],
) -> String {
    let mut manifest = String::from(
        "<item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>\n<item id=\"styles\" href=\"styles.css\" media-type=\"text/css\"/>\n",
    );
    let mut spine_xml = String::new();
    for item in spine {
        // EPUB 3 names each content document that holds MathML.
        let properties = if item.xhtml.contains("<math") {
            " properties=\"mathml\""
        } else {
            ""
        };
        let _ = writeln!(
            manifest,
            "<item id=\"{}\" href=\"{}\" media-type=\"application/xhtml+xml\"{properties}/>",
            escape_xml(&item.id),
            escape_xml(&item.href)
        );
        let _ = writeln!(spine_xml, "<itemref idref=\"{}\"/>", escape_xml(&item.id));
    }
    let mut emitted_hrefs = Vec::<&str>::new();
    for item in media {
        if emitted_hrefs.contains(&item.href.as_str()) {
            continue;
        }
        emitted_hrefs.push(&item.href);
        let id = format!("image-{:03}", emitted_hrefs.len());
        let _ = writeln!(
            manifest,
            "<item id=\"{id}\" href=\"{}\" media-type=\"{}\"/>",
            escape_xml(&item.href),
            escape_xml(&item.media_type)
        );
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"publication-id\" xml:lang=\"en\">\n<metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\">\n<dc:identifier id=\"publication-id\">{}</dc:identifier>\n<dc:title>{}</dc:title>\n<dc:language>en</dc:language>\n<dc:creator>{}</dc:creator>\n<meta property=\"dcterms:modified\">1980-01-01T00:00:00Z</meta>\n</metadata>\n<manifest>\n{manifest}</manifest>\n<spine>\n{spine_xml}</spine>\n</package>\n",
        escape_xml(identifier),
        escape_xml(title),
        escape_xml(author)
    )
}

fn publication_identifier(
    title: &str,
    author: &str,
    spine: &[SpineItem],
    media: &[MediaItem],
) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for bytes in [title.as_bytes(), author.as_bytes()] {
        fnv1a(&mut hash, bytes);
    }
    for item in spine {
        fnv1a(&mut hash, item.xhtml.as_bytes());
    }
    let mut media = media.iter().collect::<Vec<_>>();
    media.sort_unstable_by(|left, right| left.relationship_id.cmp(&right.relationship_id));
    for item in media {
        fnv1a(&mut hash, item.media_type.as_bytes());
        fnv1a(&mut hash, &item.data);
    }
    format!("urn:rdocx:{hash:016x}")
}

fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

fn write_archive(
    package: &str,
    nav: &str,
    spine: &[SpineItem],
    media: &[MediaItem],
) -> Result<Vec<u8>> {
    let cursor = BoundedCursor::new(MAX_EPUB_BYTES);
    let mut archive = ZipWriter::new(cursor);
    let stored = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default());
    let deflated = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default());

    add_entry(&mut archive, "mimetype", b"application/epub+zip", stored)?;
    add_entry(
        &mut archive,
        "META-INF/container.xml",
        CONTAINER_XML.as_bytes(),
        deflated,
    )?;
    add_entry(
        &mut archive,
        "EPUB/package.opf",
        package.as_bytes(),
        deflated,
    )?;
    add_entry(&mut archive, "EPUB/nav.xhtml", nav.as_bytes(), deflated)?;
    add_entry(
        &mut archive,
        "EPUB/styles.css",
        STYLESHEET.as_bytes(),
        deflated,
    )?;
    for item in spine {
        add_entry(
            &mut archive,
            &format!("EPUB/{}", item.href),
            item.xhtml.as_bytes(),
            deflated,
        )?;
    }
    let mut emitted_hrefs = Vec::<&str>::new();
    for item in media {
        if emitted_hrefs.contains(&item.href.as_str()) {
            continue;
        }
        emitted_hrefs.push(&item.href);
        add_entry(
            &mut archive,
            &format!("EPUB/{}", item.href),
            &item.data,
            deflated,
        )?;
    }
    let cursor = archive.finish().map_err(zip_error)?;
    Ok(cursor.into_inner())
}

fn add_entry(
    archive: &mut ZipWriter<BoundedCursor>,
    name: &str,
    bytes: &[u8],
    options: SimpleFileOptions,
) -> Result<()> {
    archive.start_file(name, options).map_err(zip_error)?;
    archive.write_all(bytes)?;
    Ok(())
}

struct BoundedCursor {
    bytes: Vec<u8>,
    position: u64,
    limit: usize,
}

impl BoundedCursor {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            position: 0,
            limit,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for BoundedCursor {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let start = usize::try_from(self.position).map_err(|_| output_limit_error())?;
        let end = start
            .checked_add(buffer.len())
            .ok_or_else(output_limit_error)?;
        if end > self.limit {
            return Err(output_limit_error());
        }
        if end > self.bytes.len() {
            self.bytes.resize(end, 0);
        }
        self.bytes[start..end].copy_from_slice(buffer);
        self.position = end as u64;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Seek for BoundedCursor {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        let base = match position {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::End(offset) => self.bytes.len() as i128 + i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
        };
        if base < 0 || base > self.limit as i128 {
            return Err(output_limit_error());
        }
        self.position = base as u64;
        Ok(self.position)
    }
}

fn measure_body_content(
    content: &BodyContent,
    depth: usize,
    item_count: &mut usize,
    text_bytes: &mut usize,
    projected_nodes: &mut usize,
    image_occurrences: &mut usize,
) -> Result<()> {
    if depth > MAX_NESTING_DEPTH {
        return Err(epub_error("table nesting exceeds the EPUB depth limit"));
    }
    add_item(item_count, projected_nodes)?;
    match content {
        BodyContent::Paragraph(paragraph) => {
            measure_paragraph(paragraph, text_bytes, projected_nodes, image_occurrences)?
        }
        BodyContent::Table(table) => measure_table(
            table,
            depth + 1,
            item_count,
            text_bytes,
            projected_nodes,
            image_occurrences,
        )?,
        BodyContent::ContentControl(control) => measure_control(
            control,
            depth + 1,
            item_count,
            text_bytes,
            projected_nodes,
            image_occurrences,
        )?,
        BodyContent::RawXml(raw) => add_source_bytes(text_bytes, raw.len())?,
    }
    Ok(())
}

/// Measure what a block content control wraps, which is exported in place.
/// The rows, cells and runs that controls wrap are measured through
/// `CT_Tbl::rows`, `CT_Row::cells` and `CT_P::accepted_bookmark_runs`.
fn measure_control(
    control: &CT_Sdt,
    depth: usize,
    item_count: &mut usize,
    text_bytes: &mut usize,
    projected_nodes: &mut usize,
    image_occurrences: &mut usize,
) -> Result<()> {
    if depth > MAX_NESTING_DEPTH {
        return Err(epub_error(
            "content control nesting exceeds the EPUB depth limit",
        ));
    }
    for content in &control.content {
        match content {
            SdtContent::Paragraph(paragraph) => {
                add_item(item_count, projected_nodes)?;
                measure_paragraph(paragraph, text_bytes, projected_nodes, image_occurrences)?
            }
            SdtContent::Table(table) => {
                add_item(item_count, projected_nodes)?;
                measure_table(
                    table,
                    depth + 1,
                    item_count,
                    text_bytes,
                    projected_nodes,
                    image_occurrences,
                )?
            }
            SdtContent::ContentControl(nested) => measure_control(
                nested,
                depth + 1,
                item_count,
                text_bytes,
                projected_nodes,
                image_occurrences,
            )?,
            SdtContent::RawXml(raw) => add_source_bytes(text_bytes, raw.len())?,
            SdtContent::Row(_) | SdtContent::Cell(_) | SdtContent::Run(_) => {}
        }
    }
    Ok(())
}

fn measure_table(
    table: &CT_Tbl,
    depth: usize,
    item_count: &mut usize,
    text_bytes: &mut usize,
    projected_nodes: &mut usize,
    image_occurrences: &mut usize,
) -> Result<()> {
    if depth > MAX_NESTING_DEPTH {
        return Err(epub_error("table nesting exceeds the EPUB depth limit"));
    }
    for (_, raw) in &table.extra_xml {
        add_source_bytes(text_bytes, raw.len())?;
    }
    if let Some(properties) = &table.properties {
        if let Some(borders) = &properties.borders {
            measure_table_borders(borders, text_bytes)?;
        }
        for raw in &properties.revision_xml {
            add_source_bytes(text_bytes, raw.len())?;
        }
    }
    // Rows and cells that content controls wrap are exported in place.
    for row in table.rows() {
        add_projected_nodes(projected_nodes, 1)?;
        for (_, raw) in &row.extra_xml {
            add_source_bytes(text_bytes, raw.len())?;
        }
        if let Some(properties) = &row.properties {
            for raw in &properties.revision_xml {
                add_source_bytes(text_bytes, raw.len())?;
            }
        }
        for cell in row.cells() {
            add_projected_nodes(projected_nodes, 1)?;
            for (_, raw) in &cell.extra_xml {
                add_source_bytes(text_bytes, raw.len())?;
            }
            if let Some(properties) = &cell.properties {
                if let Some(shading) = &properties.shading {
                    measure_shading(shading, text_bytes)?;
                }
                if let Some(borders) = &properties.borders {
                    measure_table_borders(borders, text_bytes)?;
                }
                for (_, raw) in &properties.extra_xml {
                    add_source_bytes(text_bytes, raw.len())?;
                }
            }
            for content in &cell.content {
                add_item(item_count, projected_nodes)?;
                match content {
                    CellContent::Paragraph(paragraph) => measure_paragraph(
                        paragraph,
                        text_bytes,
                        projected_nodes,
                        image_occurrences,
                    )?,
                    CellContent::Table(nested) => measure_table(
                        nested,
                        depth + 1,
                        item_count,
                        text_bytes,
                        projected_nodes,
                        image_occurrences,
                    )?,
                    CellContent::ContentControl(control) => measure_control(
                        control,
                        depth + 1,
                        item_count,
                        text_bytes,
                        projected_nodes,
                        image_occurrences,
                    )?,
                }
            }
        }
    }
    Ok(())
}

fn measure_paragraph(
    paragraph: &CT_P,
    text_bytes: &mut usize,
    projected_nodes: &mut usize,
    image_occurrences: &mut usize,
) -> Result<()> {
    if let Some(properties) = &paragraph.properties {
        for value in [
            properties.style_id.as_deref(),
            properties.line_rule.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            add_source_bytes(text_bytes, value.len())?;
        }
        if let Some(shading) = &properties.shading {
            measure_shading(shading, text_bytes)?;
        }
        for raw in &properties.numbering_revision_xml {
            add_source_bytes(text_bytes, raw.len())?;
        }
        for raw in &properties.revision_xml {
            add_source_bytes(text_bytes, raw.len())?;
        }
    }
    for hyperlink in &paragraph.hyperlinks {
        if hyperlink.run_start > hyperlink.run_end || hyperlink.run_end > paragraph.runs.len() {
            return Err(epub_error(
                "hyperlink span is outside its source paragraph during EPUB export",
            ));
        }
        let expansion = (hyperlink.run_end - hyperlink.run_start)
            .checked_add(1)
            .ok_or_else(|| epub_error("hyperlink span size overflow during EPUB export"))?;
        add_projected_nodes(projected_nodes, expansion)?;
        if let Some(value) = &hyperlink.rel_id {
            add_source_bytes(text_bytes, value.len())?;
        }
        if let Some(value) = &hyperlink.anchor {
            add_source_bytes(text_bytes, value.len())?;
        }
        for (name, value) in &hyperlink.extra_attributes {
            add_source_bytes(text_bytes, name.len())?;
            add_source_bytes(text_bytes, value.len())?;
        }
        for (_, _, raw) in &hyperlink.extra_xml {
            add_source_bytes(text_bytes, raw.len())?;
        }
    }
    // The runs of content controls and tracked insertions are exported in
    // place. Those of smart tags and custom XML are measured as raw XML below.
    for run in paragraph.accepted_bookmark_runs() {
        add_projected_nodes(projected_nodes, 1 + run.content.len())?;
        if let Some(properties) = &run.properties {
            for value in [
                properties.font_ascii.as_deref(),
                properties.color.as_deref(),
                properties.vert_align.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                add_source_bytes(text_bytes, value.len())?;
            }
            if let Some(shading) = &properties.shading {
                measure_shading(shading, text_bytes)?;
            }
            for raw in &properties.revision_xml {
                add_source_bytes(text_bytes, raw.len())?;
            }
        }
        for content in &run.content {
            match content {
                RunContent::Text(text) | RunContent::DeletedText(text) => {
                    add_source_bytes(text_bytes, text.text.len())?;
                }
                RunContent::Tab | RunContent::Break(..) | RunContent::SpecialCharacter(_) => {
                    add_source_bytes(text_bytes, 1)?
                }
                RunContent::Symbol { font, .. } => {
                    add_source_bytes(text_bytes, font.len().saturating_add(1))?
                }
                RunContent::Field(field) => measure_field(field, text_bytes, 0)?,
                RunContent::Drawing(drawing) => {
                    *image_occurrences = image_occurrences.checked_add(1).ok_or_else(|| {
                        epub_error("image occurrence count overflow during EPUB export")
                    })?;
                    if *image_occurrences > MAX_IMAGE_OCCURRENCES {
                        return Err(epub_error(
                            "document has too many image occurrences for EPUB export",
                        ));
                    }
                    let image = drawing.inline.as_ref().map(|image| {
                        (
                            image.embed_id.as_str(),
                            image.chart_rel_id.as_deref(),
                            image.description.as_deref(),
                            image.name.as_deref(),
                        )
                    });
                    let image = image.or_else(|| {
                        drawing.anchor.as_ref().map(|image| {
                            (
                                image.embed_id.as_str(),
                                image.chart_rel_id.as_deref(),
                                image.description.as_deref(),
                                image.name.as_deref(),
                            )
                        })
                    });
                    if let Some((embed_id, chart_rel_id, description, name)) = image {
                        add_source_bytes(text_bytes, embed_id.len())?;
                        for value in [chart_rel_id, description, name].into_iter().flatten() {
                            add_source_bytes(text_bytes, value.len())?;
                        }
                    }
                }
                RunContent::FootnoteRef { .. }
                | RunContent::EndnoteRef { .. }
                | RunContent::CommentReference { .. } => {}
            }
        }
        for raw in &run.extra_xml {
            add_source_bytes(text_bytes, raw.len())?;
        }
    }
    for (_, raw) in &paragraph.extra_xml {
        add_source_bytes(text_bytes, raw.len())?;
    }
    Ok(())
}

fn measure_shading(
    shading: &oxoffice_docx_oxml::properties::CT_Shd,
    text_bytes: &mut usize,
) -> Result<()> {
    add_source_bytes(text_bytes, shading.val.len())?;
    for value in [shading.color.as_deref(), shading.fill.as_deref()]
        .into_iter()
        .flatten()
    {
        add_source_bytes(text_bytes, value.len())?;
    }
    Ok(())
}

fn shading_is_simplified(shading: &oxoffice_docx_oxml::properties::CT_Shd) -> bool {
    // Layout reads `auto` or six hex digits (after a `#`), anything else as
    // automatic.
    let invalid = |value: Option<&str>| {
        value.is_some_and(|value| {
            let digits = value.strip_prefix('#').unwrap_or(value);
            !value.eq_ignore_ascii_case("auto")
                && !(digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
        })
    };
    invalid(shading.color.as_deref())
        || invalid(shading.fill.as_deref())
        // The projection keeps only the literal colours, so any theme
        // reference on the source shading is information the EPUB does not
        // carry.
        || shading.theme_color.is_some()
        || shading.theme_tint.is_some()
        || shading.theme_shade.is_some()
        || shading.theme_fill.is_some()
        || shading.theme_fill_tint.is_some()
        || shading.theme_fill_shade.is_some()
        || !shading.extra_attributes.is_empty()
}

fn measure_table_borders(
    borders: &oxoffice_docx_oxml::table::CT_TblBorders,
    text_bytes: &mut usize,
) -> Result<()> {
    for edge in [
        borders.top.as_ref(),
        borders.bottom.as_ref(),
        borders.left.as_ref(),
        borders.right.as_ref(),
        borders.inside_h.as_ref(),
        borders.inside_v.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(color) = &edge.color {
            add_source_bytes(text_bytes, color.len())?;
        }
    }
    Ok(())
}

fn measure_field(
    field: &oxoffice_docx_oxml::text::Field,
    text_bytes: &mut usize,
    depth: usize,
) -> Result<()> {
    if depth >= MAX_NESTING_DEPTH {
        return Err(epub_error("field nesting exceeds the EPUB depth limit"));
    }
    add_source_bytes(text_bytes, field.cached_result.len())?;
    add_source_bytes(text_bytes, field.instruction.raw.len())?;
    add_source_bytes(text_bytes, field.instruction.name.len())?;
    for argument in &field.instruction.arguments {
        measure_field_argument(argument, text_bytes, depth + 1)?;
    }
    for switch in &field.instruction.switches {
        add_source_bytes(text_bytes, switch.name.len())?;
        if let Some(argument) = &switch.argument {
            measure_field_argument(argument, text_bytes, depth + 1)?;
        }
    }
    Ok(())
}

fn measure_field_argument(
    argument: &oxoffice_docx_oxml::text::FieldArgument,
    text_bytes: &mut usize,
    depth: usize,
) -> Result<()> {
    match argument {
        oxoffice_docx_oxml::text::FieldArgument::Text(text) => {
            add_source_bytes(text_bytes, text.len())
        }
        oxoffice_docx_oxml::text::FieldArgument::Nested(field) => {
            measure_field(field, text_bytes, depth)
        }
        oxoffice_docx_oxml::text::FieldArgument::Joined(pieces) => {
            for piece in pieces {
                measure_field_argument(piece, text_bytes, depth)?;
            }
            Ok(())
        }
    }
}

/// Count one paragraph or table and the XHTML node it projects to.
fn add_item(item_count: &mut usize, projected_nodes: &mut usize) -> Result<()> {
    *item_count = item_count
        .checked_add(1)
        .ok_or_else(|| epub_error("document item count overflow during EPUB export"))?;
    if *item_count > MAX_BODY_ITEMS {
        return Err(epub_error(
            "document has too many body items for EPUB export",
        ));
    }
    add_projected_nodes(projected_nodes, 1)
}

fn add_projected_nodes(total: &mut usize, additional: usize) -> Result<()> {
    *total = total
        .checked_add(additional)
        .ok_or_else(|| epub_error("projected XHTML node overflow during EPUB export"))?;
    if *total > MAX_PROJECTED_NODES {
        return Err(epub_error(
            "document has too many projected XHTML nodes for EPUB export",
        ));
    }
    Ok(())
}

fn add_source_bytes(total: &mut usize, additional: usize) -> Result<()> {
    *total = total
        .checked_add(additional)
        .ok_or_else(|| epub_error("document text size overflow during EPUB export"))?;
    if *total > MAX_SOURCE_TEXT_BYTES {
        return Err(epub_error("document text exceeds the EPUB source limit"));
    }
    Ok(())
}

fn validated_epub_image(data: &[u8]) -> Option<oxoffice_media::ImageFormat> {
    let format = oxoffice_media::ImageFormat::sniff(data)?;
    let probed = oxoffice_media::probe(data)?;
    if probed.format != format {
        return None;
    }
    match format {
        oxoffice_media::ImageFormat::Png if valid_png_structure(data) => Some(format),
        oxoffice_media::ImageFormat::Jpeg if valid_jpeg_structure(data) => Some(format),
        oxoffice_media::ImageFormat::Gif if valid_gif_structure(data) => Some(format),
        _ => None,
    }
}

fn valid_png_structure(data: &[u8]) -> bool {
    if !data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return false;
    }
    let mut offset = 8_usize;
    let mut chunk_index = 0_usize;
    let mut bit_depth = None;
    let mut colour_type = None;
    let mut saw_plte = false;
    let mut saw_idat = false;
    let mut ended_idat = false;
    while let Some(header_end) = offset.checked_add(8) {
        let Some(header) = data.get(offset..header_end) else {
            return false;
        };
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let kind = &header[4..8];
        let Some(payload_end) = header_end.checked_add(length) else {
            return false;
        };
        let Some(chunk_end) = payload_end.checked_add(4) else {
            return false;
        };
        let Some(payload) = data.get(header_end..payload_end) else {
            return false;
        };
        let Some(crc_bytes) = data.get(payload_end..chunk_end) else {
            return false;
        };
        let expected_crc =
            u32::from_be_bytes([crc_bytes[0], crc_bytes[1], crc_bytes[2], crc_bytes[3]]);
        if !kind.iter().all(u8::is_ascii_alphabetic)
            || !kind[2].is_ascii_uppercase()
            || png_crc32(kind, payload) != expected_crc
        {
            return false;
        }
        match kind {
            b"IHDR" => {
                if chunk_index != 0
                    || length != 13
                    || payload[0..4] == [0, 0, 0, 0]
                    || payload[4..8] == [0, 0, 0, 0]
                    || !png_bit_depth_is_valid(payload[8], payload[9])
                    || payload[10] != 0
                    || payload[11] != 0
                    || payload[12] > 1
                {
                    return false;
                }
                bit_depth = Some(payload[8]);
                colour_type = Some(payload[9]);
            }
            b"PLTE" => {
                if chunk_index == 0
                    || saw_plte
                    || saw_idat
                    || length == 0
                    || length > 768
                    || !length.is_multiple_of(3)
                    || matches!(colour_type, Some(0 | 4))
                    || colour_type == Some(3)
                        && bit_depth.is_none_or(|depth| length / 3 > 1_usize << depth)
                {
                    return false;
                }
                saw_plte = true;
            }
            b"IDAT" => {
                if chunk_index == 0 || ended_idat || colour_type == Some(3) && !saw_plte {
                    return false;
                }
                saw_idat = true;
            }
            b"IEND" => {
                return length == 0 && saw_idat && chunk_end == data.len();
            }
            _ => {
                if kind[0].is_ascii_uppercase() {
                    return false;
                }
                if saw_idat {
                    ended_idat = true;
                }
            }
        }
        offset = chunk_end;
        chunk_index += 1;
    }
    false
}

fn png_bit_depth_is_valid(bit_depth: u8, colour_type: u8) -> bool {
    match colour_type {
        0 => matches!(bit_depth, 1 | 2 | 4 | 8 | 16),
        2 | 4 | 6 => matches!(bit_depth, 8 | 16),
        3 => matches!(bit_depth, 1 | 2 | 4 | 8),
        _ => false,
    }
}

fn png_crc32(kind: &[u8], payload: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in kind.iter().chain(payload) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 0 {
                crc >> 1
            } else {
                (crc >> 1) ^ 0xedb8_8320
            };
        }
    }
    !crc
}

fn valid_jpeg_structure(data: &[u8]) -> bool {
    if !data.starts_with(b"\xff\xd8") {
        return false;
    }
    let mut offset = 2_usize;
    let mut in_scan = false;
    let mut saw_sof = false;
    let mut saw_sos = false;
    while offset < data.len() {
        if in_scan && data[offset] != 0xff {
            offset += 1;
            continue;
        }
        if data[offset] != 0xff {
            return false;
        }
        while data.get(offset) == Some(&0xff) {
            offset += 1;
        }
        let Some(&marker) = data.get(offset) else {
            return false;
        };
        offset += 1;
        if in_scan && (marker == 0 || (0xd0..=0xd7).contains(&marker)) {
            continue;
        }
        in_scan = false;
        if marker == 0xd9 {
            return saw_sof && saw_sos && offset == data.len();
        }
        if marker == 0xd8 {
            return false;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let Some(length_bytes) = data.get(offset..offset.saturating_add(2)) else {
            return false;
        };
        let length = usize::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));
        if length < 2 {
            return false;
        }
        let Some(segment_end) = offset.checked_add(length) else {
            return false;
        };
        if segment_end > data.len() {
            return false;
        }
        if matches!(
            marker,
            0xc0 | 0xc1
                | 0xc2
                | 0xc3
                | 0xc5
                | 0xc6
                | 0xc7
                | 0xc9
                | 0xca
                | 0xcb
                | 0xcd
                | 0xce
                | 0xcf
        ) {
            saw_sof = true;
        }
        if marker == 0xda {
            if !saw_sof {
                return false;
            }
            saw_sos = true;
            in_scan = true;
        }
        offset = segment_end;
    }
    false
}

fn valid_gif_structure(data: &[u8]) -> bool {
    if !(data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a")) || data.len() < 13 {
        return false;
    }
    let packed = data[10];
    let global_table = if packed & 0x80 != 0 {
        3_usize << (usize::from(packed & 0x07) + 1)
    } else {
        0
    };
    let Some(mut offset) = 13_usize.checked_add(global_table) else {
        return false;
    };
    if offset > data.len() {
        return false;
    }
    let mut saw_image = false;
    loop {
        let Some(&kind) = data.get(offset) else {
            return false;
        };
        offset += 1;
        match kind {
            0x2c => {
                let Some(descriptor) = data.get(offset..offset.saturating_add(9)) else {
                    return false;
                };
                let width = u16::from_le_bytes([descriptor[4], descriptor[5]]);
                let height = u16::from_le_bytes([descriptor[6], descriptor[7]]);
                if width == 0 || height == 0 {
                    return false;
                }
                offset += 9;
                let local_table = if descriptor[8] & 0x80 != 0 {
                    3_usize << (usize::from(descriptor[8] & 0x07) + 1)
                } else {
                    0
                };
                let Some(after_table) = offset.checked_add(local_table) else {
                    return false;
                };
                if after_table >= data.len() {
                    return false;
                }
                let minimum_code_size = data[after_table];
                if !(2..=8).contains(&minimum_code_size) {
                    return false;
                }
                offset = after_table + 1;
                let Some((after_blocks, has_image_data)) = gif_sub_blocks_end(data, offset) else {
                    return false;
                };
                if !has_image_data {
                    return false;
                }
                offset = after_blocks;
                saw_image = true;
            }
            0x21 => {
                if data.get(offset).is_none() {
                    return false;
                }
                offset += 1;
                let Some((after_blocks, _)) = gif_sub_blocks_end(data, offset) else {
                    return false;
                };
                offset = after_blocks;
            }
            0x3b => return saw_image && offset == data.len(),
            _ => return false,
        }
    }
}

fn gif_sub_blocks_end(data: &[u8], mut offset: usize) -> Option<(usize, bool)> {
    let mut has_data = false;
    loop {
        let size = usize::from(*data.get(offset)?);
        offset = offset.checked_add(1)?;
        if size == 0 {
            return Some((offset, has_data));
        }
        has_data = true;
        offset = offset.checked_add(size)?;
        data.get(..offset)?;
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let first = u32::from(chunk[0]);
        let second = chunk.get(1).copied().map(u32::from).unwrap_or(0);
        let third = chunk.get(2).copied().map(u32::from).unwrap_or(0);
        let bits = (first << 16) | (second << 8) | third;
        output.push(ALPHABET[((bits >> 18) & 63) as usize] as char);
        output.push(ALPHABET[((bits >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[((bits >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(bits & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

fn epub_error(message: impl Into<String>) -> Error {
    Error::Other(format!("EPUB export error: {}", message.into()))
}

fn zip_error(error: zip::result::ZipError) -> Error {
    epub_error(error.to_string())
}

fn output_limit_error() -> std::io::Error {
    std::io::Error::other("EPUB output exceeds the size limit")
}
