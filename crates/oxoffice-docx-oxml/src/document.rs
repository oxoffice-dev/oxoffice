//! Document-level elements: `CT_Document` and `CT_Body`.

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, Event};
use quick_xml::{Reader, Writer, XmlVersion};

use crate::borders::CT_BorderEdge;
use crate::content_control::{CT_Sdt, SdtOwner};
use crate::error::{OxmlError, Result};
use crate::header_footer::{HdrFtrRef, HdrFtrType};
use crate::namespace::{W_NS, matches_local_name};
use crate::numbering::{
    local_namespace_overrides, merged_owner_bindings, namespace_bindings, split_namespace_binding,
    word_prefixes_at,
};
use crate::properties::{
    get_word_val_attr, is_word_attribute, is_word_element, parse_twips_measure, parse_word_toggle,
};
use crate::raw_xml::{capture_element, capture_empty_element};
use crate::revision::{CT_Revision, RevisionKind};
use crate::shared::{ST_OnOff, ST_PageOrientation, ST_SectionType};
use crate::table::{CT_Tbl, ST_VerticalJc};
use crate::text::{
    CT_P, ROOT_MC_BINDING, ROOT_R_BINDING, ROOT_WP_BINDING, capture_root_attribute_record,
    declare_w14_on_part_root, is_root_attribute_record,
};
use crate::units::Twips;

mod section;
mod source;

use section::SectionScalar;
pub use section::{
    CT_Column, CT_Columns, CT_DocGrid, CT_LineNumber, CT_NoteProperties, CT_PageBorders,
    CT_PageNumberType, CT_PaperSource, CT_SectPr, CT_SectPrRawPosition, CT_SectPrReferenceAnchor,
    ST_DocGrid, ST_LineNumberRestart, ST_PageBorderDisplay, ST_PageBorderOffset,
    ST_PageBorderZOrder,
};
pub(crate) use section::{checked_section_child_slot, validate_checked_section_child};
use source::ReturnedWordRead;
pub(crate) use source::{
    AdmittedWordSource, ConstructorFailures, ControlSourceCursor, PartReaderKind, TemplateCopyWork,
    TemplateWordRead, WordCaptureWindow, WordFieldInput, WordOrdinaryFact, WordOwner,
    WordPublicationCoverage, WordPublicationPiece, WordPublicationTape, WordSourceMap,
    WordSourceReader, WordXmlInput, admit_paragraph_result_field_input, admit_raw_word_source,
    admit_reader_source, admit_revision_field_input, admit_word_field_input,
    admit_word_input_with_constructors, admit_word_model, admit_word_reader, admit_word_source,
    admit_word_xml, admitted_word_child, control_has_property_measurement_failure_in,
    reader_input_origin, returned_block_inputs, template_paragraph_clone_pairs,
    template_scalar_bytes, word_model_children_maxima, word_model_maxima, word_owner_cost,
};
pub use source::{ScopedWordOperation, ScopedWordSource, close_content_fragment_namespaces};

#[doc(hidden)]
pub use crate::text::WrittenVisitor;

/// Content that can appear in a document body (paragraphs and tables).
///
/// Each child is boxed. A direct-body removal then shifts pointers instead of
/// copying the child value, which is what keeps that removal from growing with
/// the rest of the body.
#[derive(Debug, Clone, PartialEq)]
pub enum BodyContent {
    Paragraph(Box<CT_P>),
    Table(Box<CT_Tbl>),
    ContentControl(Box<CT_Sdt>),
    /// Raw XML for unknown elements and controls that cannot be typed safely.
    RawXml(Box<Vec<u8>>),
}

impl BodyContent {
    /// Whether this block's typed owners may be reused by value.
    ///
    /// The walk matches [`admit_word_model`]. A field, a paragraph bookmark,
    /// or a tab stop disqualifies the block. Raw XML stays exact bytes and
    /// is not rejected by itself. This does not serialize the block.
    #[doc(hidden)]
    pub fn owner_facts_reusable_by_value(&self) -> bool {
        use crate::content_control::SdtContent;
        use crate::revision::RevisionContent;
        use crate::table::CellContent;
        use crate::text::RunContent;
        let root = match self {
            BodyContent::Paragraph(paragraph) => WordOwner::Paragraph(paragraph),
            BodyContent::Table(table) => WordOwner::Table(table),
            BodyContent::ContentControl(control) => WordOwner::Control(control),
            BodyContent::RawXml(_) => return true,
        };
        let mut pending = vec![root];
        while let Some(owner) = pending.pop() {
            match owner {
                WordOwner::Body(body) => {
                    for child in &body.content {
                        match child {
                            BodyContent::Paragraph(paragraph) => {
                                pending.push(WordOwner::Paragraph(paragraph))
                            }
                            BodyContent::Table(table) => pending.push(WordOwner::Table(table)),
                            BodyContent::ContentControl(control) => {
                                pending.push(WordOwner::Control(control))
                            }
                            BodyContent::RawXml(_) => {}
                        }
                    }
                }
                WordOwner::Paragraph(paragraph) => {
                    if !paragraph.bookmark_markers.is_empty() {
                        return false;
                    }
                    if paragraph
                        .properties
                        .as_ref()
                        .and_then(|properties| properties.tabs.as_ref())
                        .is_some_and(|tabs| !tabs.tabs.is_empty())
                    {
                        return false;
                    }
                    for run in &paragraph.runs {
                        pending.push(WordOwner::Run(run));
                    }
                    for (_, _, _, control) in &paragraph.content_controls {
                        pending.push(WordOwner::Control(control));
                    }
                    for (_, _, revision) in &paragraph.revisions {
                        pending.push(WordOwner::Revision(revision));
                    }
                    for ruby in &paragraph.rubies {
                        for run in &ruby.ruby_text {
                            pending.push(WordOwner::Run(run));
                        }
                    }
                }
                WordOwner::Run(run) => {
                    for child in &run.content {
                        match child {
                            RunContent::Drawing(drawing) => {
                                pending.push(WordOwner::Drawing(drawing))
                            }
                            RunContent::Field(_) => return false,
                            _ => {}
                        }
                    }
                }
                WordOwner::Control(control) => {
                    for run in control.original_resource_runs() {
                        pending.push(WordOwner::Run(run));
                    }
                    for (_, revision) in &control.revisions {
                        pending.push(WordOwner::Revision(revision));
                    }
                    for child in &control.content {
                        let child = match child {
                            SdtContent::Paragraph(paragraph) => WordOwner::Paragraph(paragraph),
                            SdtContent::Table(table) => WordOwner::Table(table),
                            SdtContent::Row(row) => WordOwner::Row(row),
                            SdtContent::Cell(cell) => WordOwner::Cell(cell),
                            SdtContent::Run(run) => WordOwner::Run(run),
                            SdtContent::ContentControl(nested) => WordOwner::Control(nested),
                            SdtContent::RawXml(_) => continue,
                        };
                        pending.push(child);
                    }
                }
                WordOwner::Table(table) => {
                    for row in &table.rows {
                        pending.push(WordOwner::Row(row));
                    }
                    for (_, _, control) in &table.content_controls {
                        pending.push(WordOwner::Control(control));
                    }
                }
                WordOwner::Row(row) => {
                    for cell in &row.cells {
                        pending.push(WordOwner::Cell(cell));
                    }
                    for (_, _, control) in &row.content_controls {
                        pending.push(WordOwner::Control(control));
                    }
                }
                WordOwner::Cell(cell) => {
                    for child in &cell.content {
                        match child {
                            CellContent::Paragraph(paragraph) => {
                                pending.push(WordOwner::Paragraph(paragraph))
                            }
                            CellContent::Table(table) => pending.push(WordOwner::Table(table)),
                            CellContent::ContentControl(control) => {
                                pending.push(WordOwner::Control(control))
                            }
                        }
                    }
                }
                WordOwner::Field(_) => return false,
                WordOwner::Revision(revision) => {
                    if let Some(paragraph) = revision.content_paragraph() {
                        pending.push(WordOwner::Paragraph(paragraph));
                    }
                    match revision.content() {
                        RevisionContent::Runs(runs) => {
                            for run in runs {
                                pending.push(WordOwner::Run(run));
                            }
                        }
                        RevisionContent::PriorParagraphProperties(properties) => {
                            if properties
                                .tabs
                                .as_ref()
                                .is_some_and(|tabs| !tabs.tabs.is_empty())
                            {
                                return false;
                            }
                        }
                        RevisionContent::Marker
                        | RevisionContent::PriorRunProperties(_)
                        | RevisionContent::PriorTableProperties(_)
                        | RevisionContent::PriorSectionProperties(_) => {}
                    }
                    for (_, nested) in revision.nested_revisions() {
                        pending.push(WordOwner::Revision(nested));
                    }
                }
                WordOwner::Drawing(drawing) => {
                    if let Some(anchor) = &drawing.anchor {
                        pending.push(WordOwner::Anchor(anchor));
                    }
                    if let Some(shape) = drawing.inline.as_ref().and_then(|i| i.shape.as_ref()) {
                        if let Some(body) = &shape.text_body {
                            pending.push(WordOwner::Body(body));
                        }
                        for paragraph in &shape.text {
                            pending.push(WordOwner::Paragraph(paragraph));
                        }
                    }
                }
                WordOwner::Anchor(anchor) => {
                    if let Some(shape) = &anchor.shape
                        && (shape.text_body.is_some() || !shape.text.is_empty())
                    {
                        if let Some(body) = &shape.text_body {
                            pending.push(WordOwner::Body(body));
                        }
                        for paragraph in &shape.text {
                            pending.push(WordOwner::Paragraph(paragraph));
                        }
                    }
                }
            }
        }
        true
    }
}

/// `CT_Body` — The document body containing paragraphs, tables, and section properties.
#[derive(Debug, Clone, PartialEq)]
#[allow(non_snake_case)]
pub struct CT_Body {
    /// Mixed content: paragraphs and tables in document order.
    pub content: Vec<BodyContent>,
    pub sect_pr: Option<CT_SectPr>,
}

#[allow(non_snake_case)]
impl CT_Body {
    pub fn new() -> Self {
        CT_Body {
            content: Vec::new(),
            sect_pr: Some(CT_SectPr::default_letter()),
        }
    }

    /// Get an iterator over only the paragraphs.
    pub fn paragraphs(&self) -> impl Iterator<Item = &CT_P> {
        let mut paragraphs = Vec::new();
        for content in &self.content {
            match content {
                BodyContent::Paragraph(paragraph) => paragraphs.push(paragraph.as_ref()),
                BodyContent::ContentControl(sdt) => {
                    sdt.collect_paragraphs(SdtOwner::Body, &mut paragraphs)
                }
                BodyContent::Table(_) | BodyContent::RawXml(_) => {}
            }
        }
        paragraphs.into_iter()
    }

    /// The number of paragraphs [`Self::paragraphs`] visits in the body
    /// children before `index`.
    #[doc(hidden)]
    pub fn paragraph_count_before(&self, index: usize) -> usize {
        let mut nested = Vec::new();
        self.content[..index.min(self.content.len())]
            .iter()
            .map(|content| match content {
                BodyContent::Paragraph(_) => 1,
                BodyContent::ContentControl(sdt) => {
                    nested.clear();
                    sdt.collect_paragraphs(SdtOwner::Body, &mut nested);
                    nested.len()
                }
                _ => 0,
            })
            .sum()
    }

    /// Whether accepting every tracked change joins the body paragraph at
    /// `index` to the next body paragraph, as Word does when a paragraph mark
    /// is deleted or moved away (`w:del` or `w:moveFrom` in the mark `w:rPr`).
    ///
    /// The joined paragraph keeps the properties of the next paragraph, so a
    /// joining paragraph with no accepted content leaves nothing. The next
    /// body item must be a paragraph, the adjacency `Document::accept_all`
    /// requires before it merges a paragraph. Before a table, a content
    /// control or preserved XML, `accept_all` instead removes a paragraph
    /// that keeps nothing, merges across what it removes whole, or refuses;
    /// and it keeps the body's last mark. Those paragraphs keep their marks
    /// here. A paragraph that ends a section also keeps its mark here,
    /// although `accept_all` merges its section break away.
    #[doc(hidden)]
    pub fn accepted_paragraph_joins_next(&self, index: usize) -> bool {
        let Some(BodyContent::Paragraph(paragraph)) = self.content.get(index) else {
            return false;
        };
        let Some(properties) = paragraph.properties.as_ref() else {
            return false;
        };
        let Some(mark) = properties.rpr.as_ref() else {
            return false;
        };
        // The model keeps a moved-away mark as preserved XML, read with the
        // fixed `w` prefix and any binding the XML declares itself. A mark
        // whose Word prefix is bound only outside it is not recognized, so
        // that paragraph keeps its mark.
        let moved_away = || {
            mark.revision_xml.iter().any(|raw| {
                let mut reader = Reader::from_reader(raw.as_slice());
                matches!(
                    reader.read_event(),
                    Ok(Event::Start(start) | Event::Empty(start))
                        if word_prefixes_at(&start, &["w".to_owned()]).is_ok_and(|prefixes| {
                            is_word_element(start.name().as_ref(), b"moveFrom", &prefixes)
                        })
                )
            })
        };
        let removed = mark
            .revision_markers
            .iter()
            .any(|marker| marker.kind() == RevisionKind::Deletion)
            || moved_away();
        removed
            && properties.sect_pr.is_none()
            && matches!(self.content.get(index + 1), Some(BodyContent::Paragraph(_)))
    }

    /// Get a mutable iterator over only the paragraphs.
    pub fn paragraphs_mut(&mut self) -> impl Iterator<Item = &mut CT_P> {
        self.content.iter_mut().filter_map(|c| match c {
            BodyContent::Paragraph(p) => Some(p.as_mut()),
            _ => None,
        })
    }

    /// Get an iterator over only the tables.
    pub fn tables(&self) -> impl Iterator<Item = &CT_Tbl> {
        let mut tables = Vec::new();
        for content in &self.content {
            match content {
                BodyContent::Table(table) => tables.push(table.as_ref()),
                BodyContent::ContentControl(sdt) => sdt.collect_tables(SdtOwner::Body, &mut tables),
                BodyContent::Paragraph(_) | BodyContent::RawXml(_) => {}
            }
        }
        tables.into_iter()
    }

    /// Get a mutable iterator over only the tables.
    pub fn tables_mut(&mut self) -> impl Iterator<Item = &mut CT_Tbl> {
        self.content.iter_mut().filter_map(|c| match c {
            BodyContent::Table(t) => Some(t.as_mut()),
            _ => None,
        })
    }

    /// Add a paragraph to the body.
    pub fn add_paragraph(&mut self, p: CT_P) {
        self.content.push(BodyContent::Paragraph(Box::new(p)));
    }

    /// Add a table to the body.
    pub fn add_table(&mut self, tbl: CT_Tbl) {
        self.content.push(BodyContent::Table(Box::new(tbl)));
    }

    /// Get the number of body content elements (paragraphs + tables).
    pub fn content_count(&self) -> usize {
        self.content.len()
    }

    /// Insert a paragraph at the given index.
    ///
    /// Panics if `index > content_count()`.
    pub fn insert_paragraph(&mut self, index: usize, p: CT_P) {
        self.content
            .insert(index, BodyContent::Paragraph(Box::new(p)));
    }

    /// Insert a table at the given index.
    ///
    /// Panics if `index > content_count()`.
    pub fn insert_table(&mut self, index: usize, tbl: CT_Tbl) {
        self.content
            .insert(index, BodyContent::Table(Box::new(tbl)));
    }

    /// Find the index of the first paragraph whose text contains the given substring.
    pub fn find_paragraph_index(&self, text: &str) -> Option<usize> {
        self.find_paragraph_indices(text).into_iter().next()
    }

    /// Find every matching body-content index, preferring direct paragraphs.
    pub fn find_paragraph_indices(&self, text: &str) -> Vec<usize> {
        let mut direct = Vec::new();
        let mut enclosing = Vec::new();
        for (index, content) in self.content.iter().enumerate() {
            match content {
                BodyContent::Paragraph(paragraph) if paragraph.accepted_text().contains(text) => {
                    direct.push(index);
                }
                BodyContent::ContentControl(sdt) => {
                    let mut paragraphs = Vec::new();
                    sdt.collect_paragraphs(SdtOwner::Body, &mut paragraphs);
                    if paragraphs
                        .iter()
                        .any(|paragraph| paragraph.accepted_text().contains(text))
                    {
                        enclosing.push(index);
                    }
                }
                _ => {}
            }
        }
        direct.extend(enclosing);
        direct
    }

    /// Return every content control in document order, including nested controls.
    pub fn content_controls(&self) -> Vec<&CT_Sdt> {
        let mut controls = Vec::new();
        for content in &self.content {
            match content {
                BodyContent::Paragraph(paragraph) => paragraph.collect_controls(&mut controls),
                BodyContent::Table(table) => table.collect_controls(&mut controls),
                BodyContent::ContentControl(sdt) => {
                    controls.push(sdt);
                    sdt.collect_controls(SdtOwner::Body, &mut controls);
                }
                BodyContent::RawXml(_) => {}
            }
        }
        controls
    }

    /// Remove and return the content at the given index, or `None` if out of bounds.
    pub fn remove(&mut self, index: usize) -> Option<BodyContent> {
        if index < self.content.len() {
            Some(self.content.remove(index))
        } else {
            None
        }
    }

    /// Get a reference to the content at the given index.
    pub fn get(&self, index: usize) -> Option<&BodyContent> {
        self.content.get(index)
    }

    /// Get a mutable reference to the content at the given index.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut BodyContent> {
        self.content.get_mut(index)
    }

    pub fn from_xml(reader: &mut Reader<&[u8]>) -> Result<Self> {
        let admitted = admit_word_reader(reader, &["w".to_owned()], b"body", Some(W_NS))?;
        let mut controls = admitted.controls();
        Self::from_xml_with_prefixes_and_owner_bindings_until(
            reader,
            &["w".to_string()],
            &[],
            b"body",
            Some(&mut controls),
        )
    }

    pub(crate) fn from_xml_with_prefixes_and_owner_bindings_until(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
        end_local_name: &[u8],
        control_facts: Option<&mut ControlSourceCursor<'_>>,
    ) -> Result<Self> {
        Self::from_xml_with_property_output(
            reader,
            word_prefixes,
            owner_bindings,
            end_local_name,
            control_facts,
            None,
        )
    }

    fn from_xml_with_property_output(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
        end_local_name: &[u8],
        control_facts: Option<&mut ControlSourceCursor<'_>>,
        ordinary: Option<&mut Vec<WordOrdinaryFact>>,
    ) -> Result<Self> {
        Self::from_xml_with_read_output(
            reader,
            word_prefixes,
            owner_bindings,
            end_local_name,
            control_facts,
            ordinary,
            None,
        )
    }

    fn from_xml_with_read_output(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
        end_local_name: &[u8],
        mut control_facts: Option<&mut ControlSourceCursor<'_>>,
        mut ordinary: Option<&mut Vec<WordOrdinaryFact>>,
        mut read_output: Option<&mut ReturnedWordRead>,
    ) -> Result<Self> {
        let mut pending_reads = Vec::new();
        if let Some(output) = &mut ordinary {
            output.clear();
        }
        let mut pending_ordinary = Vec::new();
        let mut content = Vec::new();
        let mut sect_pr = None;
        let mut buf = Vec::new();

        loop {
            let event_start = reader.buffer_position();
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(name.as_ref(), b"p", &prefixes) {
                        let mut properties = Vec::new();
                        let mut extras = Vec::new();
                        let mut texts = Vec::new();
                        let mut maps = crate::text::WordBlockSourceMaps::default();
                        let paragraph = CT_P::from_xml_with_read_output(
                            reader,
                            &prefixes,
                            Some(e),
                            control_facts.as_deref_mut(),
                            crate::text::WordParagraphReadOutput {
                                ordinary: ordinary.as_ref().map(|_| &mut properties),
                                extra_maps: read_output.as_ref().map(|_| &mut extras),
                                text_facts: read_output.as_ref().map(|_| &mut texts),
                                nested_maps: read_output.as_ref().map(|_| &mut maps),
                                ..Default::default()
                            },
                        )?;
                        if read_output.is_some() {
                            maps.paragraph(extras);
                            maps.word_texts.push((
                                Vec::new(),
                                control_facts
                                    .as_deref()
                                    .and_then(|cursor| cursor.paragraph_word_facts(reader, texts)),
                            ));
                            pending_reads.push((content.len(), maps));
                        }
                        content.push(BodyContent::Paragraph(Box::new(paragraph)));
                        pending_ordinary.extend(properties);
                    } else if is_word_element(name.as_ref(), b"tbl", &prefixes) {
                        let local_bindings = local_namespace_overrides(e, word_prefixes)?;
                        let table_bindings = merged_owner_bindings(owner_bindings, &local_bindings);
                        let mut maps = (ordinary.is_some() || read_output.is_some()).then(|| {
                            crate::text::WordBlockSourceMaps {
                                collect_properties: ordinary.is_some(),
                                collect_word_texts: read_output.is_some(),
                                ..Default::default()
                            }
                        });
                        let mut table =
                            CT_Tbl::from_xml_with_prefixes_and_owner_bindings_at_depth_and_maps(
                                reader,
                                &prefixes,
                                &table_bindings,
                                0,
                                Some(e),
                                control_facts.as_deref_mut(),
                                maps.as_mut(),
                            )?;
                        table.retain_root_attributes(e, &prefixes)?;
                        if let Some(mut maps) = maps {
                            pending_ordinary.append(&mut maps.ordinary);
                            if read_output.is_some() {
                                pending_reads.push((content.len(), maps));
                            }
                        }
                        content.push(BodyContent::Table(Box::new(table)));
                    } else if is_word_element(name.as_ref(), b"sdt", &prefixes) {
                        let (captured, captured_map) =
                            if ordinary.is_some() || read_output.is_some() {
                                if let Some(cursor) = control_facts.as_deref() {
                                    let (raw, mapped) =
                                        cursor.capture_element(reader, e, event_start)?;
                                    let map = mapped.map(|input| {
                                        std::sync::Arc::new(input.projection_parts().0.clone())
                                    });
                                    (raw, map)
                                } else {
                                    (capture_element(reader, e)?, None)
                                }
                            } else {
                                let captured = capture_element(reader, e)?;
                                let map = control_facts.as_deref().and_then(|facts| {
                                    facts.captured_map(
                                        reader,
                                        event_start..reader.buffer_position(),
                                        &captured,
                                    )
                                });
                                (captured, map)
                            };
                        let (raw, captured_map) = crate::text::raw_with_external_bindings_and_map(
                            &captured,
                            owner_bindings,
                            None,
                            captured_map,
                        )?;
                        let mut maps = (ordinary.is_some() || read_output.is_some()).then(|| {
                            crate::text::WordBlockSourceMaps {
                                collect_properties: ordinary.is_some(),
                                collect_word_texts: read_output.is_some(),
                                ..Default::default()
                            }
                        });
                        if let Some(sdt) = CT_Sdt::from_raw_with_mapped_context_and_maps(
                            &raw,
                            &prefixes,
                            SdtOwner::Body,
                            control_facts.as_deref_mut(),
                            captured_map,
                            maps.as_mut(),
                        ) {
                            if let Some(mut maps) = maps {
                                pending_ordinary.append(&mut maps.ordinary);
                                if read_output.is_some() {
                                    pending_reads.push((content.len(), maps));
                                }
                            }
                            content.push(BodyContent::ContentControl(Box::new(sdt)));
                        } else {
                            content.push(BodyContent::RawXml(Box::new(raw)));
                        }
                    } else if is_word_element(name.as_ref(), b"sectPr", &prefixes) {
                        if sect_pr.is_none() {
                            let local_bindings = local_namespace_overrides(e, word_prefixes)?;
                            let section_bindings =
                                merged_owner_bindings(owner_bindings, &local_bindings);
                            sect_pr =
                                Some(CT_SectPr::from_xml_with_prefixes_owner_bindings_and_root(
                                    reader,
                                    &prefixes,
                                    &section_bindings,
                                    Some(e),
                                )?);
                        } else {
                            content.push(BodyContent::RawXml(Box::new(
                                crate::text::raw_with_external_bindings(
                                    &capture_element(reader, e)?,
                                    owner_bindings,
                                )?,
                            )));
                        }
                    } else {
                        // Capture unknown elements as raw XML
                        content.push(BodyContent::RawXml(Box::new(
                            crate::text::raw_with_external_bindings(
                                &capture_element(reader, e)?,
                                owner_bindings,
                            )?,
                        )));
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(name.as_ref(), b"p", &prefixes) {
                        content.push(BodyContent::Paragraph(Box::new(CT_P::from_empty_root(
                            e, &prefixes,
                        )?)));
                    } else if is_word_element(name.as_ref(), b"tbl", &prefixes) {
                        content.push(BodyContent::Table(Box::default()));
                    } else if is_word_element(name.as_ref(), b"sectPr", &prefixes) {
                        if sect_pr.is_none() {
                            sect_pr = Some(CT_SectPr::from_empty_root(e, &prefixes)?);
                        } else {
                            content.push(BodyContent::RawXml(Box::new(
                                crate::text::raw_with_external_bindings(
                                    &capture_empty_element(e)?,
                                    owner_bindings,
                                )?,
                            )));
                        }
                    } else {
                        content.push(BodyContent::RawXml(Box::new(
                            crate::text::raw_with_external_bindings(
                                &capture_empty_element(e)?,
                                owner_bindings,
                            )?,
                        )));
                    }
                }
                Ok(Event::End(ref e))
                    if is_word_element(e.name().as_ref(), end_local_name, word_prefixes) =>
                {
                    break;
                }
                Ok(Event::Eof) => {
                    let end = String::from_utf8_lossy(end_local_name);
                    return Err(OxmlError::MissingElement(format!("w:{end} end")));
                }
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        if let Some(output) = ordinary {
            *output = pending_ordinary;
        }
        if let Some(output) = read_output.as_mut() {
            output.body = pending_reads;
        }
        Ok(CT_Body { content, sect_pr })
    }

    /// Validate borrowed owners before cloning or serializing this body.
    #[doc(hidden)]
    pub fn validate_resource_limits(&self) -> Result<()> {
        admit_word_model(WordOwner::Body(self))
    }

    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.write_marked(writer, PartWrite::Whole, &mut |_| {}, None)
    }

    /// [`Self::to_xml`], showing `mark` the writer after the body's start
    /// tag and after each child. `PartWrite::Frame` omits the children and
    /// marks once, at that child boundary, before the final section.
    pub(crate) fn write_marked<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        mode: PartWrite,
        mark: &mut dyn FnMut(&Writer<W>),
        mut work: Option<&mut crate::text::WordOperationWork>,
    ) -> Result<()> {
        admit_word_model(WordOwner::Body(self))?;
        writer.write_event(Event::Start(BytesStart::new("w:body")))?;
        mark(writer);

        if mode == PartWrite::Whole {
            for item in &self.content {
                write_body_content(item, writer, work.as_deref_mut())?;
                mark(writer);
            }
        }

        if let Some(ref sect) = self.sect_pr {
            sect.to_xml(writer)?;
        }

        writer.write_event(Event::End(BytesEnd::new("w:body")))?;
        Ok(())
    }
}

/// The two part serializations that exist today. `Whole` writes children and
/// then the conditional `w14` declaration. `Frame` stops at the child
/// boundary and leaves `w14` for the caller.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PartWrite {
    Whole,
    Frame,
}

impl Default for CT_Body {
    fn default() -> Self {
        Self::new()
    }
}

fn write_body_content<W: std::io::Write>(
    item: &BodyContent,
    writer: &mut Writer<W>,
    work: Option<&mut crate::text::WordOperationWork>,
) -> Result<()> {
    match (item, work) {
        (BodyContent::Paragraph(p), Some(work)) => p.write_xml_core_in(
            writer,
            None,
            crate::text::XmlWriteRole::Publication,
            None,
            work,
            None,
        )?,
        (BodyContent::Table(t), Some(work)) => t.write_xml_role_in(
            writer,
            crate::text::XmlWriteRole::Publication,
            None,
            work,
            None,
        )?,
        (BodyContent::ContentControl(sdt), Some(work)) => sdt.write_xml_role_in(
            writer,
            crate::text::XmlWriteRole::Publication,
            None,
            work,
            None,
        )?,
        (BodyContent::Paragraph(p), None) => p.to_xml(writer)?,
        (BodyContent::Table(t), None) => t.to_xml(writer)?,
        (BodyContent::ContentControl(sdt), None) => sdt.to_xml(writer)?,
        (BodyContent::RawXml(raw), _) => writer.get_mut().write_all(raw)?,
    }
    Ok(())
}

fn paragraph_root_has_foreign_word(paragraph: &CT_P) -> Result<bool> {
    for (position, raw) in &paragraph.extra_xml {
        if !CT_P::raw_is_root_attributes(*position, raw) {
            continue;
        }
        let mut reader = Reader::from_reader(raw.as_slice());
        let Event::Empty(record) = reader.read_event()? else {
            continue;
        };
        let declarations = crate::text::raw_namespace_declarations(&record)?;
        if declarations
            .iter()
            .any(|(prefix, namespace)| prefix == "w" && namespace != W_NS)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

// The operation and actual Source observe the same substantive document reader.
// Owning metadata handles does not clone source bytes or activate input work.
struct ScopedDocumentInput {
    registry: std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>,
    initial_delivery: bool,
    publication: std::sync::Arc<std::sync::Mutex<WordPublicationCoverage>>,
    reads: std::sync::Arc<std::sync::Mutex<ReturnedWordRead>>,
}

/// `CT_Document` — The root document element.
#[derive(Debug, Clone, PartialEq)]
#[allow(non_snake_case)]
pub struct CT_Document {
    pub body: CT_Body,
    /// Extra namespace declarations captured from the original document element.
    /// Each entry is (prefix, uri), e.g. ("xmlns:wp14", "http://...").
    pub extra_namespaces: Vec<(String, String)>,
    /// Raw XML for `<w:background>` element if present.
    pub background_xml: Option<Vec<u8>>,
    /// Foreign same-local-name background children retained before the body.
    #[doc(hidden)]
    pub background_extra_xml: Vec<Vec<u8>>,
    /// Non-namespace attributes of the original document element, such as
    /// `mc:Ignorable`, in source order.
    #[doc(hidden)]
    pub root_attributes: Vec<(String, String)>,
}

#[allow(non_snake_case)]
impl CT_Document {
    /// Set the owned background color while retaining every producer byte outside its value.
    pub fn set_background_color(&mut self, color: &str) -> Result<()> {
        if color.len() != 6 || !color.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(OxmlError::InvalidValue(
                "page color must be six hexadecimal digits".to_owned(),
            ));
        }
        let mut prefixes = vec!["w".to_owned()];
        for (name, namespace) in &self.extra_namespaces {
            if namespace == W_NS {
                prefixes.push(name.strip_prefix("xmlns:").unwrap_or("").to_owned());
            }
        }
        self.background_xml = Some(match &self.background_xml {
            Some(raw) => {
                crate::settings::rewrite_word_root_attribute(raw, &prefixes, b"color", color)?
            }
            None => format!("<w:background xmlns:w=\"{W_NS}\" w:color=\"{color}\"/>").into_bytes(),
        });
        Ok(())
    }

    pub fn new() -> Self {
        CT_Document {
            body: CT_Body::new(),
            extra_namespaces: Vec::new(),
            background_xml: None,
            background_extra_xml: Vec::new(),
            root_attributes: Vec::new(),
        }
    }

    /// Parse from XML bytes (the content of word/document.xml).
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        let admitted = admit_word_source(xml, &[], None, None)?;
        Self::from_xml_admitted(xml, admitted, None)
    }

    fn from_xml_admitted(
        xml: &[u8],
        admitted: AdmittedWordSource,
        origins: Option<ScopedDocumentInput>,
    ) -> Result<Self> {
        Self::from_xml_admitted_with_properties(xml, admitted, origins, None)
    }

    fn from_xml_admitted_with_properties(
        xml: &[u8],
        admitted: AdmittedWordSource,
        origins: Option<ScopedDocumentInput>,
        mut property_output: Option<&mut Vec<WordOrdinaryFact>>,
    ) -> Result<Self> {
        if let Some(output) = &mut property_output {
            output.clear();
        }
        let mut pending_properties = Vec::new();
        let mut control_facts = admitted.controls_for(xml);
        if let Some(origins) = origins.as_ref() {
            control_facts = control_facts.with_field_origins(
                std::sync::Arc::clone(&origins.registry),
                origins.initial_delivery,
            );
        }
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(false);

        let mut ordinary = origins.as_ref().map(|_| Vec::new());
        let mut body = None;
        let mut extra_namespaces = Vec::new();
        let mut background_xml = None;
        let mut background_extra_xml = Vec::new();
        let mut root_attributes = Vec::new();
        let mut buf = Vec::new();
        let mut word_prefixes = Vec::new();
        let mut document_open = false;
        let mut document_closed = false;

        // Known namespace prefixes that we always emit ourselves
        let known_ns: &[&[u8]] = &[b"xmlns:w", b"xmlns:r", b"xmlns:mc", b"xmlns"];

        loop {
            let event_start = reader.buffer_position() as usize;
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, &word_prefixes)?;
                    if is_word_element(name.as_ref(), b"document", &prefixes) {
                        if document_open || document_closed {
                            return Err(OxmlError::UnexpectedElement("w:document".to_owned()));
                        }
                        for attr in e.attributes().flatten() {
                            let key = attr.key.as_ref();
                            let is_namespace = key.starts_with(b"xmlns:") || key == b"xmlns";
                            if is_namespace && known_ns.contains(&key) {
                                continue;
                            }
                            let key_str = std::str::from_utf8(key).unwrap_or("").to_string();
                            let val_str = attr
                                .decoded_and_normalized_value(XmlVersion::Implicit1_0, e.decoder())?
                                .into_owned();
                            if is_namespace {
                                extra_namespaces.push((key_str, val_str));
                            } else {
                                root_attributes.push((key_str, val_str));
                            }
                        }
                        document_open = true;
                        word_prefixes = prefixes.into_owned();
                        if let Some(ordinary) = &mut ordinary
                            && let Some(origin) = reader_input_origin(xml, &reader)
                        {
                            ordinary.push(
                                event_start + origin..reader.buffer_position() as usize + origin,
                            );
                        }
                    } else if !document_open || document_closed {
                        return Err(OxmlError::UnexpectedElement(
                            String::from_utf8_lossy(name.as_ref()).into_owned(),
                        ));
                    } else if is_word_element(name.as_ref(), b"body", &prefixes) {
                        if !document_open || document_closed || body.is_some() {
                            return Err(OxmlError::UnexpectedElement("w:body".to_owned()));
                        }
                        let opening = ordinary.as_ref().and_then(|_| {
                            let origin = reader_input_origin(xml, &reader)?;
                            Some(event_start + origin..reader.buffer_position() as usize + origin)
                        });
                        let owner_bindings = local_namespace_overrides(e, &word_prefixes)?;
                        let mut reads = origins.as_ref().map(|origins| {
                            origins.reads.lock().expect("returned Word facts poisoned")
                        });
                        body = Some(CT_Body::from_xml_with_read_output(
                            &mut reader,
                            &prefixes,
                            &owner_bindings,
                            b"body",
                            Some(&mut control_facts),
                            property_output.as_ref().map(|_| &mut pending_properties),
                            reads.as_deref_mut(),
                        )?);
                        if let (Some(ordinary), Some(opening)) = (&mut ordinary, opening) {
                            ordinary.push(opening);
                        }
                    } else if is_word_element(name.as_ref(), b"background", &prefixes) {
                        background_xml = Some(capture_element(&mut reader, e)?);
                    } else if matches_local_name(name.as_ref(), b"background") {
                        background_extra_xml.push(capture_element(&mut reader, e)?);
                    } else {
                        reader.read_to_end_into(name, &mut Vec::new())?;
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    let prefixes = word_prefixes_at(e, &word_prefixes)?;
                    if is_word_element(e.name().as_ref(), b"document", &prefixes)
                        || !document_open
                        || document_closed
                    {
                        return Err(OxmlError::UnexpectedElement(
                            String::from_utf8_lossy(e.name().as_ref()).into_owned(),
                        ));
                    } else if is_word_element(e.name().as_ref(), b"body", &prefixes) {
                        if body.is_some() {
                            return Err(OxmlError::UnexpectedElement("w:body".to_owned()));
                        }
                        body = Some(CT_Body::new());
                        if let Some(ordinary) = &mut ordinary
                            && let Some(origin) = reader_input_origin(xml, &reader)
                        {
                            ordinary.push(
                                event_start + origin..reader.buffer_position() as usize + origin,
                            );
                        }
                    } else if is_word_element(e.name().as_ref(), b"background", &prefixes) {
                        background_xml = Some(capture_empty_element(e)?);
                    } else if matches_local_name(e.name().as_ref(), b"background") {
                        background_extra_xml.push(capture_empty_element(e)?);
                    }
                }
                Ok(Event::End(ref e))
                    if is_word_element(e.name().as_ref(), b"document", &word_prefixes) =>
                {
                    if !document_open {
                        return Err(OxmlError::UnexpectedElement("w:document end".to_owned()));
                    }
                    document_open = false;
                    document_closed = true;
                    if let Some(ordinary) = &mut ordinary
                        && let Some(origin) = reader_input_origin(xml, &reader)
                    {
                        ordinary
                            .push(event_start + origin..reader.buffer_position() as usize + origin);
                    }
                }
                Ok(Event::Text(ref text)) if !text.as_ref().iter().all(u8::is_ascii_whitespace) => {
                    return Err(OxmlError::UnexpectedElement(
                        "text outside w:body".to_owned(),
                    ));
                }
                Ok(Event::GeneralRef(_)) => {
                    return Err(OxmlError::UnexpectedElement(
                        "text outside w:body".to_owned(),
                    ));
                }
                Ok(Event::Eof) => {
                    if document_open || !document_closed {
                        return Err(OxmlError::MissingElement("w:document end".to_owned()));
                    }
                    break;
                }
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        let document = CT_Document {
            body: body.ok_or_else(|| OxmlError::MissingElement("w:body".to_owned()))?,
            extra_namespaces,
            background_xml,
            background_extra_xml,
            root_attributes,
        };
        if let (Some(origins), Some(ordinary)) = (origins, ordinary) {
            origins
                .publication
                .lock()
                .expect("publication coverage poisoned")
                .add_ordinary_ranges(ordinary);
        }
        if let Some(output) = property_output {
            *output = pending_properties;
        }
        Ok(document)
    }

    /// Validate borrowed modeled owners before recursive work.
    #[doc(hidden)]
    pub fn validate_resource_limits(&self) -> Result<()> {
        admit_word_model(WordOwner::Body(&self.body))
    }

    /// Serialize to XML bytes.
    pub fn to_xml(&self) -> Result<Vec<u8>> {
        Ok(self.write_part(PartWrite::Whole, None)?.0)
    }

    /// [`Self::to_xml`], and where the body's children are in its bytes:
    /// the range between the body's start tag and its section properties or
    /// end tag, and the range of each child there, one after the other.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // The bytes, the children's range and each child's range.
    pub fn to_xml_with_body_spans(
        &self,
    ) -> Result<(Vec<u8>, std::ops::Range<usize>, Vec<std::ops::Range<usize>>)> {
        let mut marks = Vec::with_capacity(self.body.content.len() + 2);
        let xml = self.write_part(PartWrite::Whole, Some(&mut marks))?.0;
        // A `w14` declaration added to the root moves everything after it.
        let shift = xml.len() - marks.pop().unwrap_or(xml.len());
        let marks = marks
            .into_iter()
            .map(|mark| mark + shift)
            .collect::<Vec<_>>();
        let content = marks[0]..marks[marks.len() - 1];
        let children = marks.windows(2).map(|pair| pair[0]..pair[1]).collect();
        Ok((xml, content, children))
    }

    /// The part's bytes, with `marks`, when given, holding the offset after
    /// the body's start tag and after each child, and then the length before
    /// any `w14` declaration is added.
    fn write_part(
        &self,
        mode: PartWrite,
        mut marks: Option<&mut Vec<usize>>,
    ) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
        let mut writer = Writer::new(Vec::new());
        let mut frame_at = None;
        self.write_part_contents(
            &mut writer,
            mode,
            &mut |writer| {
                if mode == PartWrite::Frame {
                    frame_at = Some(writer.get_ref().len());
                } else if let Some(marks) = marks.as_deref_mut() {
                    marks.push(writer.get_ref().len());
                }
            },
            None,
        )?;
        if let Some(marks) = marks {
            marks.push(writer.get_ref().len());
        }
        let mut xml = writer.into_inner();
        if mode == PartWrite::Frame {
            let at = frame_at.ok_or_else(|| {
                OxmlError::InvalidValue("frame child boundary was not marked".to_owned())
            })?;
            let tail = xml.split_off(at);
            return Ok((xml, Some(tail)));
        }
        declare_w14_on_part_root(&mut xml)?;
        Ok((xml, None))
    }

    fn write_part_contents<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        mode: PartWrite,
        mark: &mut dyn FnMut(&Writer<W>),
        work: Option<&mut crate::text::WordOperationWork>,
    ) -> Result<()> {
        self.validate_resource_limits()?;
        let _binding_scope = self.root_binding_scope()?;
        writer.write_event(Event::Decl(BytesDecl::new(
            "1.0",
            Some("UTF-8"),
            Some("yes"),
        )))?;
        writer.write_event(Event::Start(self.root_start()))?;
        if let Some(bg) = &self.background_xml {
            writer.get_mut().write_all(bg)?;
        }
        for raw in &self.background_extra_xml {
            writer.get_mut().write_all(raw)?;
        }
        self.body.write_marked(writer, mode, mark, work)?;
        writer.write_event(Event::End(BytesEnd::new("w:document")))?;
        Ok(())
    }

    /// The bytes [`Self::to_xml`] writes before the first body child, through
    /// the body start, and after the last child, including the final section.
    /// This is before the conditional `w14` root declaration. A later block
    /// record applies that declaration to the stored frame.
    #[doc(hidden)]
    pub fn body_frame_xml(&self) -> Result<(Vec<u8>, Vec<u8>)> {
        let (head, tail) = self.write_part(PartWrite::Frame, None)?;
        tail.map(|tail| (head, tail))
            .ok_or_else(|| OxmlError::InvalidValue("frame tail was not written".to_owned()))
    }

    /// The canonical source bytes for `item` under this part root. A paragraph
    /// that keeps a root-local foreign `w` binding uses the explicit Source
    /// path, so its bytes can differ from [`Self::to_xml`], which stays
    /// Publication. The item need not belong to the body. Raw bytes pass
    /// through.
    #[doc(hidden)]
    pub fn body_item_xml(&self, item: &BodyContent) -> Result<Vec<u8>> {
        self.body_item_xml_with_ancestor_declarations(item, &[])
    }

    /// Serialize an item under this writer's original part root and actual
    /// non-root local ancestor declarations, ordered outer to inner.
    /// Callers must prove these frames from current writer coordinates.
    #[doc(hidden)]
    pub fn body_item_xml_with_ancestor_declarations(
        &self,
        item: &BodyContent,
        ancestors: &[&[(String, String)]],
    ) -> Result<Vec<u8>> {
        let mut writer = Writer::new(Vec::new());
        self.write_body_item_in(item, ancestors, &mut writer, None)?;
        Ok(writer.into_inner())
    }

    fn write_body_item_in<W: std::io::Write>(
        &self,
        item: &BodyContent,
        ancestors: &[&[(String, String)]],
        writer: &mut Writer<W>,
        mut work: Option<&mut crate::text::WordOperationWork>,
    ) -> Result<()> {
        // Census all borrowed input before the first owned frame copy. Ancestor
        // frames, the reconstructed writer-time root and the item's own source
        // bytes share one envelope, so a frame never receives a fresh
        // allowance after its guard copy.
        let mut ancestor_work = 0usize;
        let mut scope_bytes = 0usize;
        for frame in ancestors {
            ancestor_work = ancestor_work
                .checked_add(crate::text::retained_binding_work(0, frame, 0)?)
                .ok_or_else(|| {
                    OxmlError::InvalidValue("ancestor declaration work overflow".into())
                })?;
            for (prefix, namespace) in frame.iter() {
                scope_bytes = scope_bytes
                    .checked_add(prefix.len())
                    .and_then(|bytes| bytes.checked_add(namespace.len()))
                    .ok_or_else(|| {
                        OxmlError::InvalidValue("ancestor declaration scope overflow".into())
                    })?;
            }
        }
        let root_declarations = crate::text::raw_namespace_declarations(&self.root_start())?;
        let root_work = crate::text::retained_binding_work(0, &root_declarations, 0)?;
        for (prefix, namespace) in &root_declarations {
            scope_bytes = scope_bytes
                .checked_add(prefix.len())
                .and_then(|bytes| bytes.checked_add(namespace.len()))
                .ok_or_else(|| {
                    OxmlError::InvalidValue("ancestor declaration scope overflow".into())
                })?;
        }
        let item_bytes = match item {
            BodyContent::Paragraph(paragraph) => paragraph.retained_paragraph_source_bytes()?,
            BodyContent::Table(_) | BodyContent::ContentControl(_) | BodyContent::RawXml(_) => 0,
        };
        let allowance = crate::text::retained_source_envelope(scope_bytes, item_bytes)?;
        let needed = ancestor_work
            .checked_add(root_work)
            .and_then(|work| work.checked_add(item_bytes))
            .ok_or_else(|| OxmlError::InvalidValue("ancestor declaration work overflow".into()))?;
        if needed > allowance {
            return Err(OxmlError::InvalidValue(
                "ancestor source frame work exceeds admitted source work".into(),
            ));
        }
        match item {
            BodyContent::Paragraph(paragraph) => {
                admit_word_model(WordOwner::Paragraph(paragraph))?;
            }
            BodyContent::Table(table) => admit_word_model(WordOwner::Table(table))?,
            BodyContent::ContentControl(control) => {
                admit_word_model(WordOwner::Control(control))?;
            }
            BodyContent::RawXml(_) => {}
        }
        let _binding_scope = self.root_binding_scope()?;
        let mut ancestor_scopes = Vec::with_capacity(ancestors.len());
        for frame in ancestors {
            ancestor_scopes.push(crate::text::emitted_ancestor_scope(frame)?);
        }
        let result = match item {
            BodyContent::Paragraph(paragraph) if paragraph_root_has_foreign_word(paragraph)? => {
                // A fresh authored paragraph may keep a root-local foreign `w`
                // binding while its own Word content uses an existing alias.
                // Publication's canonical `w` would rebind that opaque root
                // fact. Emit the paragraph in its actual consumer frame instead,
                // so the foreign binding and QName-valued root attributes stay
                // authoritative. The paragraph's own retained declarations are
                // staged by `write_xml_core` after this ancestor frame.
                //
                // Decode the part root once, then borrow every inherited
                // prefix and URI from its owner. Do not clone full URI strings
                // per item; the approved D7 contract keeps these as borrowed
                // source facts and bounded overlays.
                let mut borrowed = Vec::with_capacity(
                    root_declarations.len()
                        + ancestors.iter().map(|frame| frame.len()).sum::<usize>(),
                );
                borrowed.extend(
                    root_declarations
                        .iter()
                        .map(|(prefix, namespace)| (prefix.as_str(), namespace.as_str())),
                );
                for frame in ancestors {
                    borrowed.extend(
                        frame
                            .iter()
                            .map(|(prefix, namespace)| (prefix.as_str(), namespace.as_str())),
                    );
                }
                if let Some(work) = work.as_deref_mut() {
                    paragraph.write_xml_core_in(
                        writer,
                        None,
                        crate::text::XmlWriteRole::Source,
                        Some(&borrowed),
                        work,
                        None,
                    )
                } else {
                    paragraph.write_xml_core(
                        writer,
                        None,
                        crate::text::XmlWriteRole::Source,
                        Some(&borrowed),
                    )
                }
            }
            _ => write_body_content(item, writer, work),
        };
        // Scoped overlays must restore in reverse installation order.
        while ancestor_scopes.pop().is_some() {}
        result?;
        Ok(())
    }

    /// Serialize `item` as the only child of this document's body, under the
    /// part root and with the bytes [`Self::to_xml`] writes for it there.
    ///
    /// The item need not belong to the body. As in [`Self::to_xml`], the root
    /// declares `w14` when the original root does or the written bytes use it,
    /// so a root without `w14` here does not say whether the whole part has it.
    #[doc(hidden)]
    pub fn standalone_body_item_xml(&self, item: &BodyContent) -> Result<Vec<u8>> {
        let _binding_scope = self.root_binding_scope()?;
        let mut writer = Writer::new(Vec::new());
        writer.write_event(Event::Start(self.root_start()))?;
        writer.write_event(Event::Start(BytesStart::new("w:body")))?;
        write_body_content(item, &mut writer, None)?;
        writer.write_event(Event::End(BytesEnd::new("w:body")))?;
        writer.write_event(Event::End(BytesEnd::new("w:document")))?;
        let mut xml = writer.into_inner();
        declare_w14_on_part_root(&mut xml)?;
        Ok(xml)
    }

    /// The part-root guarantees the body is serialized under.
    fn root_binding_scope(&self) -> Result<crate::text::RootBindingScope> {
        crate::text::emitted_root_binding_scope(
            &self.root_start(),
            ROOT_R_BINDING | ROOT_MC_BINDING | ROOT_WP_BINDING,
            true,
        )
    }

    /// The `w:document` start tag, before any `w14` declaration.
    fn root_start(&self) -> BytesStart<'_> {
        let wp_ns = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
        let mut doc_start = BytesStart::new("w:document");
        doc_start.push_attribute(("xmlns:w", W_NS));
        doc_start.push_attribute((
            "xmlns:r",
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
        ));
        doc_start.push_attribute((
            "xmlns:mc",
            "http://schemas.openxmlformats.org/markup-compatibility/2006",
        ));

        // Always emit xmlns:wp for drawing elements
        let mut has_wp = false;
        for (key, _) in &self.extra_namespaces {
            if key == "xmlns:wp" {
                has_wp = true;
                break;
            }
        }
        if !has_wp {
            doc_start.push_attribute(("xmlns:wp", wp_ns));
        }

        // Replay captured extra namespaces, then the root attributes that
        // may name their prefixes, such as `mc:Ignorable`.
        for (key, val) in self.extra_namespaces.iter().chain(&self.root_attributes) {
            doc_start.push_attribute((key.as_str(), val.as_str()));
        }
        doc_start
    }
}

impl Default for CT_Document {
    fn default() -> Self {
        Self::new()
    }
}

/// Section source ownership mirrors the actual section readers, including raw subsets.
pub(crate) fn source_modeled_attributes(
    parent: &[u8],
    local: &[u8],
) -> Option<&'static [&'static [u8]]> {
    Some(match (parent, local) {
        (b"sectPr", b"pgSz") => {
            crate::numbering::property_attributes(crate::numbering::PropertyKind::Paragraph, local)
        }
        (b"sectPr", b"pgMar") => {
            crate::numbering::property_attributes(crate::numbering::PropertyKind::Paragraph, local)
        }
        (b"sectPr", b"headerReference" | b"footerReference") => &[b"type"],
        (b"sectPr", b"type" | b"titlePg" | b"vAlign" | b"textDirection") => &[b"val"],
        (b"sectPr", b"cols") => &[b"num", b"space", b"equalWidth", b"sep"],
        (b"cols", b"col") => &[b"w", b"space"],
        // This reader owns only start. Format/chapter metadata remains source-backed.
        (b"sectPr", b"pgNumType") => &[b"start"],
        (b"sectPr", b"paperSrc") => &[b"first", b"other"],
        (b"sectPr", b"lnNumType") => &[b"countBy", b"start", b"distance", b"restart"],
        (b"sectPr", b"docGrid") => &[b"type", b"linePitch", b"charSpace"],
        (b"sectPr", b"footnotePr" | b"endnotePr" | b"sectPrChange") => &[],
        _ => return None,
    })
}

pub(crate) fn source_value_is_modeled(
    parent: &[u8],
    local: &[u8],
    key: &[u8],
    value: &str,
) -> bool {
    if parent != b"sectPr" {
        return true;
    }
    if matches!(local, b"headerReference" | b"footerReference") && key == b"type" {
        return HdrFtrType::from_str(value).to_str() == value;
    }
    if local == b"cols" && matches!(key, b"equalWidth" | b"sep") {
        return crate::shared::ST_OnOff::parse(value).is_some();
    }
    if local == b"pgSz" && key == b"orient" {
        return ST_PageOrientation::from_str(value).is_ok();
    }
    if local == b"vAlign" && key == b"val" {
        return ST_VerticalJc::from_str(value).to_str() == value;
    }
    if local == b"type" && key == b"val" {
        return ST_SectionType::from_str(value).is_ok();
    }
    if local == b"titlePg" && key == b"val" {
        return crate::shared::ST_OnOff::parse(value).is_some();
    }
    if local == b"docGrid" {
        return match key {
            b"type" => ST_DocGrid::from_str(value).is_some(),
            b"linePitch" | b"charSpace" => value.parse::<i32>().is_ok(),
            _ => false,
        };
    }
    if local == b"paperSrc" {
        return value.parse::<u32>().is_ok();
    }
    if local == b"lnNumType" {
        return match key {
            b"countBy" | b"start" => value.parse::<u32>().is_ok(),
            b"distance" => value.parse::<i32>().is_ok(),
            b"restart" => ST_LineNumberRestart::from_str(value).is_some(),
            _ => false,
        };
    }
    true
}
