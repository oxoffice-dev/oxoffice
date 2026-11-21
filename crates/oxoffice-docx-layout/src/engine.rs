//! Layout engine orchestrator: ties all phases together.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

use oxoffice_docx_oxml::borders::{CT_PBdr, CT_TabStop};
use oxoffice_docx_oxml::content_control::{CT_Sdt, SdtContent};
use oxoffice_docx_oxml::document::{
    BodyContent, CT_Body, CT_DocGrid, CT_Document, CT_SectPr, ST_DocGrid, ST_LineNumberRestart,
    ST_PageBorderDisplay, ST_PageBorderOffset, ST_PageBorderZOrder,
};
use oxoffice_docx_oxml::drawing::WrapType;
use oxoffice_docx_oxml::header_footer::{HdrFtrType, VmlWatermark};
use oxoffice_docx_oxml::numbering::{ST_LvlSuffix, ST_NumberFormat};
use oxoffice_docx_oxml::properties::{CT_EastAsianLayout, CT_PPr, CT_RPr, CT_Shd, ST_Em};
use oxoffice_docx_oxml::revision::{CT_Revision, RevisionContent, RevisionKind};
use oxoffice_docx_oxml::ruby::{CT_Ruby, ST_RubyAlign};
use oxoffice_docx_oxml::shared::ST_HighlightColor;
use oxoffice_docx_oxml::styles::CT_Styles;
use oxoffice_docx_oxml::table::{CT_Row, CT_Tbl, CT_Tc, CellContent, ST_VerticalJc};
use oxoffice_docx_oxml::text::{
    BookmarkMarker, BreakType, CT_P, CT_R, FieldArgument, FieldInstruction, RunContent,
    SpecialCharacter, hyperlink_revision_index,
};

use crate::block::{
    self, CellBlockSemantics, FieldPaintOwner, LayoutBlock, LayoutBlockLike, PaintPolicies,
    PaintedElements, ParagraphBlock, ParagraphSemantics, SharedLayoutBlock, TableSemantics,
    WordAnchorPaint, WordAnnotationKind, WordAnnotationRecipe, WordAnnotationRootKind,
    WordAnnotationStyle, WordFieldOccurrence, WordInputAnnotation, WordLegacyCache,
    WordLegacyProducer, WordParagraphPaint,
};
use crate::convert;
use crate::input::{
    EmbeddedFontSource, LayoutInput, MediaRegistry, PictureDrawOwners, RevisionStyle, RevisionView,
};
use crate::notes::NoteRegistry;
use crate::paginator::{
    self, ColumnTrack, HeaderFooterContent, HeaderFooterSemantics, LineNumbering, PageBorderFrame,
    PageGeometry,
};
use crate::style_resolver::{self, NumberingState, ResolvedNumbering};
use crate::table;
use crate::{WordBodyLayoutFragment, WordSourcePath, WordStory};
use oxoffice_layout::{
    Color, Diagnostic, DocumentMetadata, DocumentStructure, FieldKind, FieldSource, FontId,
    FontManager, FontTraceEvent, GlyphRun, GroupElement, InlineItem, LayoutError, LayoutResult,
    LineItem, NoteRef, NoteStream, PageFrame, Point, PositionedElement, Rect, Result, ShapedText,
    SourceNodeId, SourceSpan, StructureId, StructureNode, StructureRole, TextDirection,
    TextSegment, Transform, Underline, break_multilingual_into_lines,
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum WordInputOrigin {
    #[default]
    Text,
    PlainField,
    Structural,
    AtomicMarker,
    Annotation,
}

#[derive(Clone, Default)]
struct WordMultilingualStyle {
    occurrence: Option<WordFieldOccurrence>,
    annotation: Option<WordInputAnnotation>,
    kern: bool,
    origin: WordInputOrigin,
    requested_family: Option<String>,
    language: Option<String>,
    language_east_asia: Option<String>,
    language_bidi: Option<String>,
    direction: TextDirection,
    spacing: f64,
}

// Word positions exact-spaced text from a stable em baseline instead of each
// fallback font's hhea ascent. Keeping this Word-specific prevents script font
// metrics from moving otherwise identical lines vertically.
const WORD_EXACT_LINE_BASELINE_EM: f64 = 0.8;

#[derive(Clone, Copy)]
struct ProjectedRun<'a> {
    run: &'a CT_R,
    boundary: usize,
    raw_order: RawOrder,
    ordinary_run_index: Option<usize>,
    hyperlink_index: Option<usize>,
    force_underline: bool,
    force_strike: bool,
    /// The innermost insertion, deletion or move that marks the run in the
    /// tracked view.
    revision: Option<RevisionKind>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RawOrder {
    BeforeRaw,
    Raw(usize),
    AfterRaw,
}

pub(crate) enum MainStoryLayoutItem<'a> {
    Paragraph(&'a CT_P, Vec<usize>),
    Table(&'a CT_Tbl, Vec<usize>),
}

#[derive(Clone, Copy)]
enum BlockControlOwner {
    Body,
    Table,
    Row,
    Cell,
}

pub(crate) fn body_layout_items(body: &CT_Body) -> Vec<MainStoryLayoutItem<'_>> {
    let mut items = Vec::new();
    for (body_index, content) in body.content.iter().enumerate() {
        let path = vec![body_index];
        match content {
            BodyContent::Paragraph(paragraph) => {
                items.push(MainStoryLayoutItem::Paragraph(paragraph, path))
            }
            BodyContent::Table(table) => items.push(MainStoryLayoutItem::Table(table, path)),
            BodyContent::ContentControl(control) => {
                collect_body_control_layout_items(control, &path, &mut items)
            }
            BodyContent::RawXml(_) => {}
        }
    }
    items
}

/// Section-ending main-story paragraphs in layout order, followed by the final section.
/// Table-cell paragraphs do not introduce main-story sections.
#[doc(hidden)]
pub fn document_sections(document: &oxoffice_docx_oxml::document::CT_Document) -> Vec<&CT_SectPr> {
    body_layout_items(&document.body)
        .into_iter()
        .filter_map(|item| match item {
            MainStoryLayoutItem::Paragraph(paragraph, _) => paragraph
                .properties
                .as_ref()
                .and_then(|properties| properties.sect_pr.as_ref()),
            MainStoryLayoutItem::Table(_, _) => None,
        })
        .chain(document.body.sect_pr.iter())
        .collect()
}

fn join_accepted_layout_paragraphs(prefix: CT_P, paragraph: &CT_P) -> Result<CT_P> {
    let mut result = paragraph.clone();
    for (index, run) in prefix.runs.into_iter().enumerate() {
        if !result.insert_unwrapped_run(index, run) {
            return Err(LayoutError::Layout(
                "could not align a joined paragraph's run boundaries".to_owned(),
            ));
        }
    }
    result.hyperlinks.extend(prefix.hyperlinks);
    Ok(result)
}

/// The section properties that govern each main-story item, index for index.
///
/// A paragraph's `w:sectPr` ends its section, so it applies to that paragraph
/// and to every item since the previous break. The body `w:sectPr` applies to
/// the items after the last paragraph break (ECMA-376 17.6.17 and 17.6.18).
fn main_story_item_sections<'a>(
    items: &[MainStoryLayoutItem<'a>],
    final_section: &'a CT_SectPr,
) -> Vec<&'a CT_SectPr> {
    let mut sections = Vec::with_capacity(items.len());
    let mut open = 0;
    for item in items {
        open += 1;
        if let MainStoryLayoutItem::Paragraph(paragraph, _) = item
            && let Some(section) = paragraph
                .properties
                .as_ref()
                .and_then(|properties| properties.sect_pr.as_ref())
        {
            sections.extend(std::iter::repeat_n(section, open));
            open = 0;
        }
    }
    sections.extend(std::iter::repeat_n(final_section, open));
    sections
}

fn collect_body_control_layout_items<'a>(
    control: &'a CT_Sdt,
    path: &[usize],
    items: &mut Vec<MainStoryLayoutItem<'a>>,
) {
    for (content_index, content) in control.content.iter().enumerate() {
        let mut content_path = path.to_vec();
        content_path.push(content_index);
        match content {
            SdtContent::Paragraph(paragraph) => {
                items.push(MainStoryLayoutItem::Paragraph(paragraph, content_path))
            }
            SdtContent::Table(table) => items.push(MainStoryLayoutItem::Table(table, content_path)),
            SdtContent::ContentControl(control) => {
                collect_body_control_layout_items(control, &content_path, items)
            }
            SdtContent::Row(_)
            | SdtContent::Cell(_)
            | SdtContent::Run(_)
            | SdtContent::RawXml(_) => {}
        }
    }
}

enum TextBoxSourceBinding {
    Paragraphs(Vec<SourceNodeId>),
    Body(Vec<SourceNodeId>),
}

/// Immutable source identities allocated once before layout starts.
pub(crate) struct SourceRegistry {
    nodes: Vec<WordSourcePath>,
    navigation_sources: HashMap<SourceNodeId, crate::ParagraphNavigation>,
    ids: HashMap<WordSourcePath, SourceNodeId>,
    navigation_paths: BTreeMap<(WordStory, Vec<usize>), SourceNodeId>,
    body_ids: Vec<Option<SourceNodeId>>,
    field_descendants: HashMap<FieldSource, Vec<u32>>,
    field_source_xml: HashMap<FieldSource, Vec<u8>>,
    field_source_contexts: HashMap<FieldSource, (String, bool, bool)>,
    page_fields: HashMap<FieldSource, (FieldInstruction, String)>,
    styleref_requests: HashMap<FieldSource, FieldInstruction>,
    text_box_owner_indices: HashMap<WordStory, usize>,
    story_part_names: HashMap<WordStory, String>,
    story_owner_children: HashMap<WordStory, Vec<usize>>,
    revision_view: RevisionView,
    text_box_sources: HashMap<SourceNodeId, Vec<TextBoxSourceBinding>>,
    preparation_error: Option<String>,
}

impl SourceRegistry {
    fn for_body(input: &LayoutInput, body: &CT_Body) -> Result<Self> {
        let mut registry = Self {
            nodes: Vec::new(),
            navigation_sources: HashMap::new(),
            ids: HashMap::new(),
            navigation_paths: BTreeMap::new(),
            body_ids: Vec::new(),
            field_descendants: HashMap::new(),
            field_source_xml: HashMap::new(),
            field_source_contexts: HashMap::new(),
            page_fields: HashMap::new(),
            styleref_requests: HashMap::new(),
            text_box_owner_indices: HashMap::new(),
            story_part_names: input.story_part_names.clone(),
            story_owner_children: HashMap::new(),
            revision_view: input.revision_view,
            text_box_sources: HashMap::new(),
            preparation_error: None,
        };
        registry.collect_story_body(body, &WordStory::Document);
        if let Some(error) = registry.preparation_error.take() {
            return Err(LayoutError::Layout(error));
        }
        Ok(registry)
    }

    fn bind_sequence_shape_sources(
        &mut self,
        input: &LayoutInput,
        parent: SourceNodeId,
        run_index: usize,
        drawing_index: usize,
        physical_owner: Option<usize>,
    ) -> std::result::Result<HashMap<SourceNodeId, SourceNodeId>, String> {
        let snapshot = input
            .sequence_snapshot
            .as_ref()
            .ok_or("text-box source snapshot is unavailable")?;
        let parent = snapshot
            .source_node(parent)
            .ok_or("text-box parent source is unregistered")?;
        let part_name = match &parent.story {
            WordStory::TextBox { part_name, .. } => part_name.clone(),
            story => input
                .story_part_names
                .get(story)
                .cloned()
                .ok_or("text-box parent part is unavailable")?,
        };
        let mut owner_children = match &parent.story {
            WordStory::TextBox { owner_children, .. } => owner_children.clone(),
            WordStory::Footnote { id } | WordStory::Endnote { id } => {
                let notes = if matches!(parent.story, WordStory::Footnote { .. }) {
                    input.footnotes.as_ref()
                } else {
                    input.endnotes.as_ref()
                };
                vec![
                    notes
                        .and_then(|notes| notes.footnotes.iter().position(|note| note.id == *id))
                        .ok_or("text-box note owner is unavailable")?,
                ]
            }
            _ => Vec::new(),
        };
        owner_children.extend(&parent.children);
        owner_children.extend([run_index, drawing_index]);
        let story = WordStory::TextBox {
            part_name: part_name.clone(),
            owner_children: owner_children.clone(),
        };
        if physical_owner
            .is_some_and(|owner| snapshot.text_box_owner_indices.get(&story) != Some(&owner))
        {
            return Err(
                "text-box selected physical owner differs from its registered source".into(),
            );
        }
        let mut forward = HashMap::new();
        let mut inverse = HashMap::new();
        for (index, path) in self.nodes.iter().enumerate() {
            let global_story = match &path.story {
                WordStory::Document => story.clone(),
                WordStory::TextBox {
                    owner_children: nested,
                    ..
                } => {
                    let mut children = owner_children.clone();
                    children.extend(nested);
                    WordStory::TextBox {
                        part_name: part_name.clone(),
                        owner_children: children,
                    }
                }
                _ => return Err("text-box scope contains an unrelated source story".into()),
            };
            let global = snapshot
                .source_id(&WordSourcePath {
                    story: global_story,
                    children: path.children.clone(),
                })
                .ok_or("text-box child path is not registered in its selected physical owner")?;
            let local = SourceNodeId::new((index + 1) as u32)
                .ok_or("text-box local source exceeds its identity range")?;
            if inverse.insert(global, local).is_some() {
                return Err("text-box source mapping is not injective".into());
            }
            forward.insert(local, global);
        }
        for (source, (instruction, locked, cached)) in &self.field_source_contexts {
            let global = FieldSource {
                node: forward[&source.node],
                index: source.index,
            };
            let Some((expected, inherited_lock, generated)) = snapshot.source_field_context(global)
            else {
                return Err("text-box child field context is unregistered".into());
            };
            if expected != instruction || (*locked && !inherited_lock) || *cached != generated {
                return Err("text-box child field context differs from its physical source".into());
            }
            if let Some(raw) = self.field_source_xml.get(source)
                && snapshot.source_field_xml(global) != Some(raw.as_slice())
            {
                return Err("text-box child raw field differs from its physical source".into());
            }
        }
        for node in self.ids.values_mut() {
            *node = forward[node];
        }
        for node in self.navigation_paths.values_mut() {
            *node = forward[node];
        }
        for node in self.body_ids.iter_mut().flatten() {
            *node = forward[node];
        }
        self.field_descendants = std::mem::take(&mut self.field_descendants)
            .into_iter()
            .map(|(source, value)| {
                (
                    FieldSource {
                        node: forward[&source.node],
                        index: source.index,
                    },
                    value,
                )
            })
            .collect();
        self.field_source_xml = std::mem::take(&mut self.field_source_xml)
            .into_iter()
            .map(|(source, value)| {
                (
                    FieldSource {
                        node: forward[&source.node],
                        index: source.index,
                    },
                    value,
                )
            })
            .collect();
        self.field_source_contexts = std::mem::take(&mut self.field_source_contexts)
            .into_iter()
            .map(|(source, value)| {
                (
                    FieldSource {
                        node: forward[&source.node],
                        index: source.index,
                    },
                    value,
                )
            })
            .collect();
        self.page_fields = std::mem::take(&mut self.page_fields)
            .into_iter()
            .map(|(source, value)| {
                (
                    FieldSource {
                        node: forward[&source.node],
                        index: source.index,
                    },
                    value,
                )
            })
            .collect();
        self.text_box_sources = std::mem::take(&mut self.text_box_sources)
            .into_iter()
            .map(|(owner, mut bindings)| {
                for binding in &mut bindings {
                    let nodes = match binding {
                        TextBoxSourceBinding::Paragraphs(nodes)
                        | TextBoxSourceBinding::Body(nodes) => nodes,
                    };
                    for node in nodes {
                        *node = forward[node];
                    }
                }
                (forward[&owner], bindings)
            })
            .collect();
        Ok(inverse)
    }

    fn for_input(input: &LayoutInput) -> Result<Self> {
        let mut registry = Self {
            nodes: Vec::new(),
            navigation_sources: HashMap::new(),
            ids: HashMap::new(),
            navigation_paths: BTreeMap::new(),
            body_ids: Vec::with_capacity(input.document.body.content.len()),
            field_descendants: HashMap::new(),
            field_source_xml: HashMap::new(),
            field_source_contexts: HashMap::new(),
            page_fields: HashMap::new(),
            styleref_requests: HashMap::new(),
            text_box_owner_indices: HashMap::new(),
            story_part_names: input.story_part_names.clone(),
            story_owner_children: HashMap::new(),
            revision_view: input.revision_view,
            text_box_sources: HashMap::new(),
            preparation_error: None,
        };

        for (body_index, content) in input.document.body.content.iter().enumerate() {
            match content {
                BodyContent::Paragraph(paragraph) => {
                    let id = registry.insert_node(WordSourcePath {
                        story: WordStory::Document,
                        children: vec![body_index],
                    });
                    registry.register_fields(id, paragraph);
                    registry.body_ids.push(Some(id));
                }
                BodyContent::Table(table) => {
                    registry.body_ids.push(None);
                    registry.collect_table(table, &WordStory::Document, &[body_index])
                }
                BodyContent::ContentControl(control) => {
                    registry.body_ids.push(None);
                    let mut items = Vec::new();
                    collect_body_control_layout_items(control, &[body_index], &mut items);
                    for item in items {
                        match item {
                            MainStoryLayoutItem::Paragraph(paragraph, children) => {
                                registry.insert(
                                    WordSourcePath {
                                        story: WordStory::Document,
                                        children,
                                    },
                                    paragraph,
                                );
                            }
                            MainStoryLayoutItem::Table(table, children) => {
                                registry.collect_table(table, &WordStory::Document, &children)
                            }
                        }
                    }
                }
                BodyContent::RawXml(_) => {
                    registry.body_ids.push(None);
                }
            }
        }

        let mut headers = input.headers.iter().collect::<Vec<_>>();
        headers.sort_unstable_by_key(|(relationship_id, _)| *relationship_id);
        for (relationship_id, header) in headers {
            let story = WordStory::Header {
                relationship_id: relationship_id.clone(),
            };
            if let Some(body) = input.story_bodies.get(&story) {
                registry.collect_story_body(body, &story);
                continue;
            }
            for paragraph_index in 0..header.paragraphs.len() {
                registry.insert(
                    WordSourcePath {
                        story: story.clone(),
                        children: vec![paragraph_index],
                    },
                    &header.paragraphs[paragraph_index],
                );
            }
        }

        let mut footers = input.footers.iter().collect::<Vec<_>>();
        footers.sort_unstable_by_key(|(relationship_id, _)| *relationship_id);
        for (relationship_id, footer) in footers {
            let story = WordStory::Footer {
                relationship_id: relationship_id.clone(),
            };
            if let Some(body) = input.story_bodies.get(&story) {
                registry.collect_story_body(body, &story);
                continue;
            }
            for paragraph_index in 0..footer.paragraphs.len() {
                registry.insert(
                    WordSourcePath {
                        story: story.clone(),
                        children: vec![paragraph_index],
                    },
                    &footer.paragraphs[paragraph_index],
                );
            }
        }

        for (story_kind, stream) in [
            (NoteStream::Footnote, input.footnotes.as_ref()),
            (NoteStream::Endnote, input.endnotes.as_ref()),
        ]
        .into_iter()
        .filter_map(|(story, stream)| stream.map(|stream| (story, stream)))
        {
            for (note_index, note) in stream.footnotes.iter().enumerate() {
                if stream.get_by_id(note.id).is_none() {
                    continue;
                }
                let story = match story_kind {
                    NoteStream::Footnote => WordStory::Footnote { id: note.id },
                    NoteStream::Endnote => WordStory::Endnote { id: note.id },
                };
                registry
                    .story_owner_children
                    .insert(story.clone(), vec![note_index]);
                if let Some(body) = input.story_bodies.get(&story) {
                    registry.collect_story_body(body, &story);
                    continue;
                }
                for paragraph_index in 0..note.paragraphs.len() {
                    registry.insert(
                        WordSourcePath {
                            story: story.clone(),
                            children: vec![paragraph_index],
                        },
                        &note.paragraphs[paragraph_index],
                    );
                }
            }
        }

        if let Some(error) = registry.preparation_error.take() {
            return Err(LayoutError::Layout(error));
        }
        Ok(registry)
    }

    fn collect_story_body(&mut self, body: &CT_Body, story: &WordStory) {
        for item in body_layout_items(body) {
            match item {
                MainStoryLayoutItem::Paragraph(paragraph, children) => {
                    self.insert(
                        WordSourcePath {
                            story: story.clone(),
                            children,
                        },
                        paragraph,
                    );
                }
                MainStoryLayoutItem::Table(table, children) => {
                    self.collect_table(table, story, &children)
                }
            }
        }
    }

    fn collect_table(&mut self, table: &CT_Tbl, story: &WordStory, prefix: &[usize]) {
        for (row, row_path) in table::layout_table_rows(table, prefix) {
            for (cell, cell_path) in table::layout_row_cells(row, &row_path) {
                for (content_index, content) in cell.content.iter().enumerate() {
                    let mut content_path = cell_path.clone();
                    content_path.push(content_index);
                    self.collect_cell_content(content, story, &content_path);
                }
            }
        }
    }

    fn collect_cell_content(&mut self, content: &CellContent, story: &WordStory, path: &[usize]) {
        match content {
            CellContent::Paragraph(paragraph) => self.insert(
                WordSourcePath {
                    story: story.clone(),
                    children: path.to_vec(),
                },
                paragraph,
            ),
            CellContent::Table(table) => self.collect_table(table, story, path),
            CellContent::ContentControl(control) => self.collect_cell_control(control, story, path),
        }
    }

    fn collect_cell_control(&mut self, control: &CT_Sdt, story: &WordStory, path: &[usize]) {
        for (content_index, content) in control.content.iter().enumerate() {
            let mut content_path = path.to_vec();
            content_path.push(content_index);
            match content {
                SdtContent::Paragraph(paragraph) => self.insert(
                    WordSourcePath {
                        story: story.clone(),
                        children: content_path,
                    },
                    paragraph,
                ),
                SdtContent::Table(table) => self.collect_table(table, story, &content_path),
                SdtContent::ContentControl(control) => {
                    self.collect_cell_control(control, story, &content_path)
                }
                SdtContent::Row(_)
                | SdtContent::Cell(_)
                | SdtContent::Run(_)
                | SdtContent::RawXml(_) => {}
            }
        }
    }

    pub(crate) fn record_table_omission(
        &self,
        story: &WordStory,
        prefix: &[usize],
        state: &mut NumberingState,
    ) {
        // Sorted physical paths bound each lookup to the omitted subtree.
        for (_, node) in self
            .navigation_paths
            .range((story.clone(), prefix.to_vec())..)
            .take_while(|((owner, children), _)| owner == story && children.starts_with(prefix))
        {
            if let Some(original) = self.navigation_sources.get(node) {
                let mut facts = original.clone();
                facts.omitted_by_table = true;
                facts.level = None;
                facts.style = None;
                state.record_navigation(*node, facts);
            }
        }
    }

    fn register_fields(&mut self, node: SourceNodeId, paragraph: &CT_P) {
        let text = match projected_paragraph_text(paragraph, self.revision_view) {
            Ok(text) => text,
            Err(error) => {
                self.preparation_error.get_or_insert_with(|| {
                    format!(
                        "source paragraph {} preparation failed: {error}",
                        node.get()
                    )
                });
                String::new()
            }
        };
        self.navigation_sources.insert(
            node,
            crate::ParagraphNavigation {
                omitted_by_table: false,
                level: None,
                suppressed_numbering: false,
                accepted_runs: project_paragraph_runs(paragraph, RevisionView::Accepted).len(),
                text,
                style: paragraph
                    .properties
                    .as_ref()
                    .and_then(|p| p.style_id.clone()),
            },
        );
        fn register(
            field: &oxoffice_docx_oxml::text::Field,
            node: SourceNodeId,
            index: &mut u32,
            result: &mut HashMap<FieldSource, Vec<u32>>,
        ) -> Vec<u32> {
            let own = *index;
            *index += 1;
            let mut inherited = Vec::new();
            for nested in field.nested_fields_in_source_order() {
                let child = *index;
                let descendants = register(nested, node, index, result);
                inherited.push(child);
                inherited.extend(descendants);
            }
            for nested in field.cached_fields_in_source_order() {
                register(nested, node, index, result);
            }
            result.insert(FieldSource { node, index: own }, inherited.clone());
            inherited
        }
        fn remember(
            registry: &mut SourceRegistry,
            field: &oxoffice_docx_oxml::text::Field,
            node: SourceNodeId,
            index: &mut u32,
            locked: bool,
            cached: bool,
            retain_raw: bool,
        ) {
            let source = FieldSource {
                node,
                index: *index,
            };
            let locked = locked || field.locked() == Some(true);
            let instruction = field.effective_instruction();
            if !locked && !cached && supported_styleref(&instruction) {
                registry
                    .styleref_requests
                    .insert(source, instruction.clone());
            }
            registry
                .field_source_contexts
                .insert(source, (field.effective_instruction_text(), locked, cached));
            if matches!(
                instruction.name.as_str(),
                "PAGE" | "NUMPAGES" | "SECTION" | "SECTIONPAGES" | "PAGEREF"
            ) {
                registry.page_fields.insert(
                    source,
                    (instruction, field.effective_display().into_owned()),
                );
            }

            if retain_raw && let Ok(Some((_, source_xml))) = field.source_replacement() {
                registry
                    .field_source_xml
                    .insert(source, source_xml.to_vec());
            }
            *index += 1;
            for nested in field.nested_fields_in_source_order() {
                remember(registry, nested, node, index, locked, cached, retain_raw);
            }
            for nested in field.cached_fields_in_source_order() {
                remember(registry, nested, node, index, locked, true, retain_raw);
            }
        }
        let is_text_box = matches!(
            self.nodes[(node.get() - 1) as usize].story,
            WordStory::TextBox { .. }
        );
        let mut index = 0;
        let mut raw_index = 0;
        for field in paragraph
            .source_runs()
            .into_iter()
            .flat_map(|run| &run.content)
            .filter_map(|content| match content {
                RunContent::Field(field) => Some(field),
                _ => None,
            })
        {
            register(field, node, &mut index, &mut self.field_descendants);
            remember(self, field, node, &mut raw_index, false, false, is_text_box);
        }
        self.register_text_boxes(node, paragraph);
    }

    pub(crate) fn bind_text_boxes(
        &self,
        block: &mut ParagraphBlock,
        paint: &mut WordParagraphPaint,
        owner: Option<SourceNodeId>,
    ) -> Result<()> {
        let Some(children) = owner.and_then(|owner| self.text_box_sources.get(&owner)) else {
            return Ok(());
        };
        if paint.anchors.len() != block.anchored.len() {
            return Err(LayoutError::Layout(
                "text box paint owner count mismatch".into(),
            ));
        }
        for ((drawing, binding), anchor_paint) in block
            .anchored
            .iter_mut()
            .zip(children)
            .zip(&mut paint.anchors)
        {
            if let block::AnchoredContent::Shape { text, .. } = &mut drawing.content {
                let WordAnchorPaint::Shape { paragraphs } = anchor_paint else {
                    return Err(LayoutError::Layout(
                        "text box paint owner variant mismatch".into(),
                    ));
                };
                if paragraphs.len() != text.len() {
                    return Err(LayoutError::Layout(
                        "text box paragraph paint count mismatch".into(),
                    ));
                }
                match binding {
                    TextBoxSourceBinding::Paragraphs(nodes) => {
                        for ((paragraph, paint), node) in text.iter_mut().zip(paragraphs).zip(nodes)
                        {
                            rebind_paragraph_source(paragraph, Some(*node))?;
                            paint.rebind_source(Some(*node))?;
                            self.bind_text_boxes(paragraph, paint, Some(*node))?;
                        }
                    }
                    TextBoxSourceBinding::Body(nodes) => {
                        let map = nodes
                            .iter()
                            .enumerate()
                            .map(|(index, node)| {
                                (
                                    SourceNodeId::new((index + 1) as u32)
                                        .expect("registered scoped source"),
                                    *node,
                                )
                            })
                            .collect();
                        for (paragraph, paint) in text.iter_mut().zip(paragraphs) {
                            remap_text_box_sources(paragraph, &map)?;
                            paint.remap_sources(&map)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn register_text_boxes(&mut self, owner: SourceNodeId, paragraph: &CT_P) {
        let path = self.nodes[(owner.get() - 1) as usize].clone();
        let part_name = match &path.story {
            WordStory::TextBox { part_name, .. } => Some(part_name.clone()),
            story => self.story_part_names.get(story).cloned(),
        };
        let Some(part_name) = part_name else {
            return;
        };
        let mut children = Vec::new();
        // Logical projected paths include accepted revisions. Physical cache
        // ownership comes from the independently bound source owner, not this index.
        for (run_index, projected) in project_paragraph_runs(paragraph, self.revision_view)
            .into_iter()
            .enumerate()
        {
            let run = projected.run;
            let drawings = run
                .content
                .iter()
                .filter_map(|content| match content {
                    RunContent::Drawing(drawing) => Some(drawing),
                    _ => None,
                })
                .chain(run.alt_drawings.iter());
            for (drawing_index, drawing) in drawings.enumerate() {
                let Some(anchor) = drawing.anchor.as_ref() else {
                    continue;
                };
                if anchor.chart_rel_id.is_some() || !anchor.embed_id.is_empty() {
                    children.push(TextBoxSourceBinding::Paragraphs(Vec::new()));
                    continue;
                }
                let Some(shape) = anchor.shape.as_ref() else {
                    continue;
                };
                let mut owner_children = match &path.story {
                    WordStory::TextBox { owner_children, .. } => {
                        let mut result = owner_children.clone();
                        result.extend(&path.children);
                        result
                    }
                    story => {
                        let mut result = self
                            .story_owner_children
                            .get(story)
                            .cloned()
                            .unwrap_or_default();
                        result.extend(&path.children);
                        result
                    }
                };
                owner_children.extend([run_index, drawing_index]);
                let story = WordStory::TextBox {
                    part_name: part_name.clone(),
                    owner_children,
                };
                if let Some(owner) = shape.source_text_box_owner {
                    self.text_box_owner_indices.insert(story.clone(), owner);
                }
                if let Some(body) = &shape.text_body {
                    let start = self.nodes.len();
                    self.collect_story_body(body, &story);
                    let nodes = (start..self.nodes.len())
                        .map(|index| {
                            SourceNodeId::new((index + 1) as u32).expect("registered source node")
                        })
                        .collect();
                    children.push(TextBoxSourceBinding::Body(nodes));
                    continue;
                }
                let mut nodes = Vec::new();
                for (index, paragraph) in shape.text.iter().enumerate() {
                    let child_path = WordSourcePath {
                        story: story.clone(),
                        children: vec![index],
                    };
                    self.insert(child_path.clone(), paragraph);
                    nodes.push(self.ids[&child_path]);
                }
                children.push(TextBoxSourceBinding::Paragraphs(nodes));
            }
        }
        self.text_box_sources.insert(owner, children);
    }

    fn insert(&mut self, path: WordSourcePath, paragraph: &CT_P) {
        if self.ids.contains_key(&path) {
            return;
        }
        let id = self.insert_node(path.clone());
        self.register_fields(id, paragraph);
        self.navigation_paths
            .insert((path.story.clone(), path.children.clone()), id);
        self.ids.insert(path, id);
    }

    fn insert_node(&mut self, path: WordSourcePath) -> SourceNodeId {
        let index = u32::try_from(self.nodes.len() + 1)
            .expect("a layout result cannot contain more than u32::MAX source paragraphs");
        let id = SourceNodeId::new(index).expect("source ids are one based");
        self.nodes.push(path);
        id
    }

    fn body_id(&self, body_index: usize) -> Option<SourceNodeId> {
        self.body_ids.get(body_index).copied().flatten()
    }

    pub(crate) fn id(&self, story: &WordStory, children: &[usize]) -> Option<SourceNodeId> {
        self.ids
            .get(&WordSourcePath {
                story: story.clone(),
                children: children.to_vec(),
            })
            .copied()
    }
}

/// Apply the numeric picture phase, before civil date and general formatting.
pub fn format_numeric_field_picture(
    value: &str,
    picture: &str,
) -> std::result::Result<String, String> {
    let number = value
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
        .ok_or_else(|| "numeric field value is not a finite number".to_owned())?;
    format_numeric_picture(number, picture)
}

/// Apply ordered general switches after the numeric and civil date phases.
pub fn format_numeric_field_general(
    instruction: &FieldInstruction,
    value: &str,
) -> std::result::Result<String, String> {
    let mut output = value.to_owned();
    for switch in instruction
        .switches
        .iter()
        .filter(|switch| switch.name == "*")
    {
        let Some(FieldArgument::Text(format)) = &switch.argument else {
            return Err("general format requires a text argument".into());
        };
        output = if matches!(
            instruction.name.as_str(),
            "PAGE" | "NUMPAGES" | "SECTION" | "SECTIONPAGES" | "PAGEREF"
        ) && format.eq_ignore_ascii_case("alphabetic")
        {
            page_alphabetic(parse_positive_integer(&output)?, format == "ALPHABETIC")?
        } else {
            apply_general_format(&output, format)?
        };
    }
    Ok(output)
}

fn validate_page_field_instruction(
    instruction: &FieldInstruction,
) -> std::result::Result<(), String> {
    let page_ref = instruction.name == "PAGEREF";
    if !matches!(
        instruction.name.as_str(),
        "PAGE" | "NUMPAGES" | "SECTION" | "SECTIONPAGES" | "PAGEREF"
    ) {
        return Err("not a page-family field".into());
    }
    if !instruction.quotes_are_balanced() {
        return Err(format!("field {} has unclosed quoting", instruction.name));
    }
    if instruction.arguments.len() != usize::from(page_ref)
        || page_ref && !matches!(instruction.arguments.first(), Some(FieldArgument::Text(_)))
    {
        return Err(format!(
            "field {} requires {} positional operands",
            instruction.name,
            usize::from(page_ref)
        ));
    }
    for switch in &instruction.switches {
        let format = matches!(switch.name.as_str(), "*" | "#");
        if !format && !(page_ref && matches!(switch.name.as_str(), "h" | "p")) {
            return Err(format!("unsupported switch \\{}", switch.name));
        }
        if format && !matches!(switch.argument, Some(FieldArgument::Text(_))) {
            return Err(format!(
                "field {} switch \\{} requires a text argument",
                instruction.name, switch.name
            ));
        }
        if !format && switch.argument.is_some() {
            return Err(format!(
                "field {} switch \\{} does not take an argument",
                instruction.name, switch.name
            ));
        }
    }
    Ok(())
}

/// Shared bounded page-family policy for layout and saved-cache consumers.
#[doc(hidden)]
pub fn format_page_field_result(
    instruction: &FieldInstruction,
    value: usize,
    section_format: Option<&str>,
) -> std::result::Result<String, String> {
    validate_page_field_instruction(instruction)?;
    let section_format = section_format
        .filter(|format| supported_section_page_number_format(format))
        .ok_or_else(|| "unsupported section page number format".to_owned())?;
    let numeric_override = instruction.switches.iter().any(|switch| switch.name == "#" || switch.name == "*" && matches!(&switch.argument, Some(FieldArgument::Text(format)) if matches!(format.to_ascii_lowercase().as_str(), "arabic" | "roman" | "alphabetic" | "ordinal")));
    let mut base = value.to_string();
    if !numeric_override && section_format != "decimal" {
        base = match section_format {
            "upperRoman" => roman(parse_positive_integer(&base)?, true)?,
            "lowerRoman" => roman(parse_positive_integer(&base)?, false)?,
            "upperLetter" => page_alphabetic(parse_positive_integer(&base)?, true)?,
            "lowerLetter" => page_alphabetic(parse_positive_integer(&base)?, false)?,
            _ => unreachable!(),
        };
    }
    format_numeric_field_display(instruction, base)
}

/// Read effective imported numbering after qualifying the modeled first owner.
/// Qualified absence is decimal. Unsupported chapter or unqualified source is None.
/// Direct Word duplicates overlay explicit attributes without changing the model.
#[doc(hidden)]
pub fn section_page_number_format(section: &CT_SectPr) -> Option<String> {
    effective_section_page_number(section).format
}

struct EffectiveSectionPageNumber {
    start: Option<usize>,
    format: Option<String>,
}

fn supported_section_page_number_format(format: &str) -> bool {
    matches!(
        format,
        "decimal" | "upperRoman" | "lowerRoman" | "upperLetter" | "lowerLetter"
    )
}

fn effective_section_page_number(section: &CT_SectPr) -> EffectiveSectionPageNumber {
    let start = section
        .page_number
        .as_ref()
        .and_then(|number| number.start)
        .map(|value| value as usize);
    let unsupported = || EffectiveSectionPageNumber {
        start,
        format: None,
    };
    if section.page_number.is_none() {
        return EffectiveSectionPageNumber {
            start: None,
            format: Some("decimal".into()),
        };
    }
    // Positively admit the actual modeled owner before any opaque duplicate can
    // contribute. The writer owns canonical w and retained root binding records.
    let mut owner = section.clone();
    let root_records = section
        .extra_xml
        .iter()
        .zip(&section.extra_xml_positions)
        .filter(|(raw, position)| CT_SectPr::raw_position_is_root_attributes(position, raw))
        .map(|(raw, position)| (raw.clone(), position.clone()))
        .collect::<Vec<_>>();
    (owner.extra_xml, owner.extra_xml_positions) = root_records.into_iter().unzip();
    if read_effective_section_page_number(&owner, start).is_none() {
        return unsupported();
    }
    // This is an analysis-only projection of the current writer output. Typed
    // getters and writes still address the first owner. Invalid later state
    // discards the entire recovery and retains the legacy start as compatibility.
    read_effective_section_page_number(section, start).unwrap_or_else(unsupported)
}

fn read_effective_section_page_number(
    section: &CT_SectPr,
    modeled_start: Option<usize>,
) -> Option<EffectiveSectionPageNumber> {
    use quick_xml::{events::Event, name::ResolveResult, reader::NsReader};
    let mut writer = quick_xml::Writer::new(Vec::new());
    section.to_xml(&mut writer).ok()?;
    let mut xml =
        b"<pageSource xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">"
            .to_vec();
    xml.extend(writer.into_inner());
    xml.extend_from_slice(b"</pageSource>");
    let mut reader = NsReader::from_reader(xml.as_slice());
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut owners = 0usize;
    let mut start = modeled_start;
    let mut format = "decimal".to_owned();
    let mut unsupported_format = None;
    loop {
        let event = reader.read_event_into(&mut buffer).ok()?;
        let opens = matches!(&event, Event::Start(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                if depth == 2 && element.local_name().as_ref() == b"pgNumType" {
                    let namespace = reader.resolver().resolve_element(element.name()).0;
                    if matches!(namespace, ResolveResult::Unknown(_)) {
                        return None;
                    }
                    if matches!(namespace, ResolveResult::Bound(namespace) if namespace.as_ref() == b"http://schemas.openxmlformats.org/wordprocessingml/2006/main")
                    {
                        owners += 1;
                        for attribute in element.attributes() {
                            let attribute = attribute.ok()?;
                            if attribute.key.as_ref() == b"xmlns"
                                || attribute.key.as_ref().starts_with(b"xmlns:")
                            {
                                continue;
                            }
                            let namespace = reader.resolver().resolve_attribute(attribute.key).0;
                            if matches!(namespace, ResolveResult::Unknown(_)) {
                                return None;
                            }
                            if !matches!(namespace, ResolveResult::Bound(namespace) if namespace.as_ref() == b"http://schemas.openxmlformats.org/wordprocessingml/2006/main")
                            {
                                continue;
                            }
                            match attribute.key.local_name().as_ref() {
                                b"fmt" => {
                                    format = attribute
                                        .decoded_and_normalized_value(
                                            quick_xml::XmlVersion::Implicit1_0,
                                            element.decoder(),
                                        )
                                        .ok()?
                                        .into_owned();
                                    if !supported_section_page_number_format(&format)
                                        && unsupported_format.is_none()
                                    {
                                        unsupported_format = Some(format.clone());
                                    }
                                }
                                b"start" if owners > 1 => {
                                    let value = attribute
                                        .decoded_and_normalized_value(
                                            quick_xml::XmlVersion::Implicit1_0,
                                            element.decoder(),
                                        )
                                        .ok()?;
                                    start = Some(value.parse::<u32>().ok()? as usize);
                                }
                                b"chapStyle" | b"chapSep" => return None,
                                _ => {}
                            }
                        }
                    }
                }
                if opens {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth = depth.checked_sub(1)?;
            }
            Event::Eof => {
                return (owners > 0).then(|| EffectiveSectionPageNumber {
                    start,
                    format: Some(unsupported_format.unwrap_or(format)),
                });
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn apply_general_format(value: &str, format: &str) -> std::result::Result<String, String> {
    if format == "ALPHABETIC" {
        return parse_positive_integer(value).map(|value| alphabetic(value, true));
    }
    if format == "ROMAN" {
        return parse_positive_integer(value).and_then(|value| roman(value, true));
    }
    match format.to_ascii_lowercase().as_str() {
        "upper" => Ok(value.to_uppercase()),
        "lower" => Ok(value.to_lowercase()),
        "firstcap" => Ok(capitalize_first(value)),
        "caps" => Ok(value
            .split_inclusive(char::is_whitespace)
            .map(capitalize_first)
            .collect()),
        "arabic" => parse_positive_integer(value).map(|value| value.to_string()),
        "alphabetic" => parse_positive_integer(value).map(|value| alphabetic(value, false)),
        "roman" => parse_positive_integer(value).and_then(|value| roman(value, false)),
        "ordinal" => parse_positive_integer(value).map(ordinal),
        "mergeformat" | "charformat" => Ok(value.to_owned()),
        other => Err(format!("general format {other} is unsupported")),
    }
}

fn parse_positive_integer(value: &str) -> std::result::Result<u32, String> {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| "general numeric format requires a positive integer".to_owned())
}

fn capitalize_first(value: &str) -> String {
    let mut characters = value.chars();
    characters
        .next()
        .map(|first| first.to_uppercase().chain(characters).collect())
        .unwrap_or_default()
}

fn page_alphabetic(value: u32, upper: bool) -> std::result::Result<String, String> {
    // Word uses A..Z, AA..ZZ, AAA..ZZZ for alphabetic page fields.
    // Keep hostile page-number restarts from allocating an unbounded cache.
    let count = (value - 1) / 26 + 1;
    if count > 4096 {
        return Err("alphabetic page result exceeds 4096 characters".to_owned());
    }
    let base = if upper { b'A' } else { b'a' };
    Ok(char::from(base + ((value - 1) % 26) as u8)
        .to_string()
        .repeat(count as usize))
}

fn alphabetic(mut value: u32, upper: bool) -> String {
    let mut output = Vec::new();
    while value > 0 {
        value -= 1;
        let base = if upper { b'A' } else { b'a' };
        output.push((base + (value % 26) as u8) as char);
        value /= 26;
    }
    output.iter().rev().collect()
}

fn roman(mut value: u32, upper: bool) -> std::result::Result<String, String> {
    if value > 3999 {
        return Err("Roman format supports values from 1 through 3999".to_owned());
    }
    let values = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut output = String::new();
    for (number, numeral) in values {
        while value >= number {
            output.push_str(numeral);
            value -= number;
        }
    }
    Ok(if upper { output } else { output.to_lowercase() })
}

fn ordinal(value: u32) -> String {
    let suffix = if (11..=13).contains(&(value % 100)) {
        "th"
    } else {
        match value % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        }
    };
    format!("{value}{suffix}")
}

fn split_picture_sections(picture: &str) -> std::result::Result<Vec<String>, String> {
    let mut sections = vec![String::new()];
    let mut quote = None;
    for character in picture.chars() {
        match (character, quote) {
            ('"' | '\'', None) => {
                quote = Some(character);
                sections.last_mut().unwrap().push(character);
            }
            (character, Some(active)) if character == active => {
                quote = None;
                sections.last_mut().unwrap().push(character);
            }
            (';', None) => sections.push(String::new()),
            (other, _) => sections.last_mut().unwrap().push(other),
        }
    }
    if quote.is_some() || sections.len() > 3 {
        Err("numeric picture has invalid sections or quoting".to_owned())
    } else {
        Ok(sections)
    }
}

fn format_numeric_picture(value: f64, picture: &str) -> std::result::Result<String, String> {
    let sections = split_picture_sections(picture)?;
    let (section, magnitude, implicit_negative) = if value < 0.0 {
        if let Some(section) = sections.get(1) {
            (section.as_str(), -value, false)
        } else {
            (sections[0].as_str(), -value, true)
        }
    } else if value == 0.0 && sections.len() == 3 {
        (sections[2].as_str(), 0.0, false)
    } else {
        (sections[0].as_str(), value, false)
    };
    let placeholders = numeric_placeholder_indices(section);
    let Some(first) = placeholders.first().copied() else {
        return unquote_picture(section);
    };
    let last = placeholders.last().copied().unwrap() + 1;
    let prefix = unquote_picture(&section[..first])?;
    let suffix = unquote_picture(&section[last..])?;
    let (core, literals) = extract_numeric_literals(&section[first..last])?;
    let mut halves = core.split('.');
    let integer_picture = halves.next().unwrap_or_default();
    let decimal_picture = halves.next().unwrap_or_default();
    if halves.next().is_some()
        || integer_picture
            .chars()
            .any(|character| !matches!(character, '0' | '#' | ','))
        || decimal_picture
            .chars()
            .any(|character| !matches!(character, '0' | '#'))
    {
        return Err("numeric picture contains unsupported tokens".to_owned());
    }
    let maximum_decimals = decimal_picture.chars().count();
    let minimum_decimals = decimal_picture
        .chars()
        .filter(|character| *character == '0')
        .count();
    let integer_placeholders = integer_picture
        .chars()
        .filter(|character| matches!(character, '0' | '#'))
        .collect::<Vec<_>>();
    let decimal_placeholders = decimal_picture.chars().collect::<Vec<_>>();
    let formatted = format!("{magnitude:.maximum_decimals$}");
    let (integer, mut decimal) = formatted
        .split_once('.')
        .map(|(integer, decimal)| (integer.to_owned(), decimal.to_owned()))
        .unwrap_or((formatted, String::new()));
    while decimal.len() > minimum_decimals && decimal.ends_with('0') {
        decimal.pop();
    }
    let mut integer = integer;
    if !integer_placeholders.contains(&'0') && integer == "0" {
        integer.clear();
    }
    let missing_integer = integer_placeholders.len().saturating_sub(integer.len());
    let padding = integer_placeholders[..missing_integer]
        .iter()
        .map(|placeholder| if *placeholder == '0' { '0' } else { ' ' })
        .collect::<String>();
    if integer_picture.contains(',') {
        integer = group_digits(&integer);
    }
    let decimal_padding = decimal_placeholders[decimal.len()..]
        .iter()
        .map(|placeholder| if *placeholder == '0' { '0' } else { ' ' })
        .collect::<String>();
    let number = if decimal_placeholders.is_empty() {
        format!("{padding}{integer}")
    } else {
        format!("{padding}{integer}.{decimal}{decimal_padding}")
    };
    let number = insert_numeric_literals(
        &number,
        integer_picture
            .chars()
            .chain(decimal_picture.chars())
            .filter(|character| matches!(character, '0' | '#'))
            .count(),
        &literals,
    );
    Ok(format!(
        "{}{}{}{}",
        if implicit_negative { "-" } else { "" },
        prefix,
        number,
        suffix
    ))
}

fn numeric_placeholder_indices(value: &str) -> Vec<usize> {
    let mut quote = None;
    let mut indices = Vec::new();
    for (index, character) in value.char_indices() {
        match (character, quote) {
            ('"' | '\'', None) => quote = Some(character),
            (character, Some(active)) if character == active => quote = None,
            ('0' | '#', None) => indices.push(index),
            _ => {}
        }
    }
    indices
}

fn extract_numeric_literals(
    value: &str,
) -> std::result::Result<(String, Vec<(usize, String)>), String> {
    let mut pattern = String::new();
    let mut literals = Vec::new();
    let mut literal = String::new();
    let mut quote = None;
    let mut placeholder_count = 0usize;
    for character in value.chars() {
        match (character, quote) {
            ('"' | '\'', None) => quote = Some(character),
            (character, Some(active)) if character == active => {
                literals.push((placeholder_count, std::mem::take(&mut literal)));
                quote = None;
            }
            (character, Some(_)) => literal.push(character),
            (character @ ('0' | '#'), None) => {
                pattern.push(character);
                placeholder_count += 1;
            }
            (character, None) => pattern.push(character),
        }
    }
    if quote.is_some() {
        Err("numeric picture has unclosed quoting".to_owned())
    } else {
        Ok((pattern, literals))
    }
}

fn insert_numeric_literals(
    number: &str,
    placeholder_count: usize,
    literals: &[(usize, String)],
) -> String {
    if literals.is_empty() {
        return number.to_owned();
    }
    let slot_count = number
        .chars()
        .filter(|character| character.is_ascii_digit() || *character == ' ')
        .count();
    let extra_leading = slot_count.saturating_sub(placeholder_count);
    let mut output = String::new();
    let mut digits_written = 0usize;
    for (position, literal) in literals
        .iter()
        .filter(|(position, _)| extra_leading + position == 0)
    {
        let _ = position;
        output.push_str(literal);
    }
    for character in number.chars() {
        output.push(character);
        if character.is_ascii_digit() || character == ' ' {
            digits_written += 1;
            for (_, literal) in literals
                .iter()
                .filter(|(position, _)| extra_leading + position == digits_written)
            {
                output.push_str(literal);
            }
        }
    }
    output
}

fn unquote_picture(value: &str) -> std::result::Result<String, String> {
    let mut output = String::new();
    let mut quote = None;
    for character in value.chars() {
        match (character, quote) {
            ('"' | '\'', None) => quote = Some(character),
            (character, Some(active)) if character == active => quote = None,
            (character, _) => output.push(character),
        }
    }
    if quote.is_some() {
        Err("numeric picture has unclosed quoting".to_owned())
    } else {
        Ok(output)
    }
}

fn group_digits(value: &str) -> String {
    let mut output = String::new();
    for (index, character) in value.chars().enumerate() {
        if index > 0 && (value.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(character);
    }
    output
}

fn format_sequence_display(
    instruction: &FieldInstruction,
    value: String,
) -> std::result::Result<String, String> {
    if value == "Error! Main Document Only."
        || (value.is_empty()
            && field_has_switch(instruction, "h")
            && !field_has_switch(instruction, "*"))
    {
        return Ok(value);
    }
    format_numeric_field_display(instruction, value)
}

fn format_numeric_field_display(
    instruction: &FieldInstruction,
    value: String,
) -> std::result::Result<String, String> {
    let picture = instruction
        .switches
        .iter()
        .find(|switch| switch.name == "#")
        .and_then(|switch| switch.argument.as_ref())
        .and_then(|argument| match argument {
            FieldArgument::Text(value) => Some(value.as_str()),
            _ => None,
        });
    let value = if let Some(picture) = picture {
        format_numeric_field_picture(&value, picture)?
    } else {
        value
    };
    format_numeric_field_general(instruction, &value)
}

/// Evaluate sequence fields from accepted physical sources without layout.
pub fn evaluate_sequence_fields(input: &LayoutInput) -> Result<crate::WordSequenceSnapshot> {
    if input.revision_view != RevisionView::Accepted {
        return Err(LayoutError::Layout(
            "sequence evaluation requires the accepted revision view".into(),
        ));
    }
    let sources = SourceRegistry::for_input(input)?;
    sequence_snapshot(input, &sources)
}

#[derive(Default)]
struct SequenceCounter {
    value: Option<i64>,
    heading: Option<usize>,
}

struct SequenceCollector<'a> {
    input: &'a LayoutInput,
    snapshot: crate::WordSequenceSnapshot,
    counters: HashMap<String, SequenceCounter>,
    headings: [Option<usize>; 9],
    paragraph_index: usize,
    note_contexts: HashMap<WordStory, Vec<usize>>,
    bookmark_starts: Vec<(i32, String, WordStory, usize, usize)>,
    bookmark_ends: Vec<(i32, WordStory, usize, usize)>,
    bookmark_order: usize,
    bookmark_requests: Vec<(FieldSource, FieldInstruction)>,
}

pub(crate) fn validate_navigation_exclusions(
    input: &LayoutInput,
    facts: &HashMap<WordSourcePath, Vec<usize>>,
) -> Result<()> {
    if input.revision_view != RevisionView::Accepted
        || !input.story_part_names.contains_key(&WordStory::Document)
    {
        return Err(LayoutError::Layout(
            "navigation exclusions require a qualified accepted main story".into(),
        ));
    }
    let sources = SourceRegistry::for_input(input)?;
    let mut counts = HashMap::new();
    for (index, path) in sources.nodes.iter().enumerate() {
        let node = SourceNodeId::new(
            u32::try_from(index + 1)
                .map_err(|_| LayoutError::Layout("navigation source limit".into()))?,
        )
        .expect("one based source");
        if let Some(source) = sources.navigation_sources.get(&node)
            && counts.insert(path, source.accepted_runs).is_some()
        {
            return Err(LayoutError::Layout(
                "ambiguous navigation physical path".into(),
            ));
        }
    }
    for (path, runs) in facts {
        let count = counts.get(path);
        if path.story != WordStory::Document
            || count.is_none()
            || runs.windows(2).any(|pair| pair[0] >= pair[1])
            || runs
                .last()
                .is_some_and(|run| *run >= count.copied().unwrap_or(0))
        {
            return Err(LayoutError::Layout(
                "navigation exclusion lacks a unique bounded physical source".into(),
            ));
        }
    }
    Ok(())
}

fn sequence_snapshot(
    input: &LayoutInput,
    sources: &SourceRegistry,
) -> Result<crate::WordSequenceSnapshot> {
    let source_ids = sources
        .nodes
        .iter()
        .enumerate()
        .map(|(index, path)| {
            (
                path.clone(),
                SourceNodeId::new((index + 1) as u32).expect("registered source"),
            )
        })
        .collect();
    let navigation = input
        .sequence_snapshot
        .as_ref()
        .filter(|snapshot| snapshot.navigation_document.is_some());
    if let Some(snapshot) = navigation {
        if snapshot.navigation_document.as_ref() != Some(&input.document)
            || snapshot.navigation_part.as_ref() != input.story_part_names.get(&WordStory::Document)
        {
            return Err(LayoutError::Layout(
                "stale navigation exclusion source stamp".into(),
            ));
        }
        validate_navigation_exclusions(input, &snapshot.navigation_exclusions)?;
    }
    let mut collector = SequenceCollector {
        input,
        snapshot: crate::WordSequenceSnapshot {
            source_nodes: sources.nodes.clone(),
            navigation_document: navigation.and_then(|s| s.navigation_document.clone()),
            navigation_failure: navigation.and_then(|s| s.navigation_failure.clone()),
            navigation_part: navigation.and_then(|s| s.navigation_part.clone()),
            navigation_exclusions: navigation
                .map(|s| s.navigation_exclusions.clone())
                .unwrap_or_default(),
            navigation_owners: navigation
                .map(|s| s.navigation_owners.clone())
                .unwrap_or_default(),
            source_ids,
            field_source_xml: sources.field_source_xml.clone(),
            field_source_contexts: sources.field_source_contexts.clone(),
            text_box_owner_indices: sources.text_box_owner_indices.clone(),
            physical_story_owners: sources
                .nodes
                .iter()
                .filter_map(|path| {
                    let (part, owner) = match &path.story {
                        WordStory::Document => (String::new(), WordStory::Document),
                        WordStory::Header { .. } => (
                            input.story_part_names.get(&path.story)?.clone(),
                            WordStory::Header {
                                relationship_id: String::new(),
                            },
                        ),
                        WordStory::Footer { .. } => (
                            input.story_part_names.get(&path.story)?.clone(),
                            WordStory::Footer {
                                relationship_id: String::new(),
                            },
                        ),
                        WordStory::Footnote { .. } | WordStory::Endnote { .. } => (
                            input
                                .story_part_names
                                .get(&path.story)
                                .cloned()
                                .unwrap_or_default(),
                            path.story.clone(),
                        ),
                        WordStory::TextBox { part_name, .. } => (
                            part_name.clone(),
                            WordStory::TextBox {
                                part_name: part_name.clone(),
                                owner_children: vec![
                                    *sources.text_box_owner_indices.get(&path.story)?,
                                ],
                            },
                        ),
                    };
                    Some((path.story.clone(), (part, owner)))
                })
                .collect(),
            ..Default::default()
        },
        counters: HashMap::new(),
        headings: [None; 9],
        paragraph_index: 0,
        note_contexts: HashMap::new(),
        bookmark_starts: Vec::new(),
        bookmark_ends: Vec::new(),
        bookmark_order: 0,
        bookmark_requests: Vec::new(),
    };
    collector.body(&input.document.body, &WordStory::Document, true, None)?;
    for (header, parts) in [(true, &input.headers), (false, &input.footers)] {
        let mut entries = parts.iter().collect::<Vec<_>>();
        entries.sort_by_key(|(id, _)| *id);
        for (id, part) in entries {
            let story = if header {
                WordStory::Header {
                    relationship_id: id.clone(),
                }
            } else {
                WordStory::Footer {
                    relationship_id: id.clone(),
                }
            };
            if let Some(body) = input.story_bodies.get(&story) {
                collector.body(body, &story, false, None)?;
            } else {
                for (index, paragraph) in part.paragraphs.iter().enumerate() {
                    collector.paragraph(
                        paragraph,
                        &WordSourcePath {
                            story: story.clone(),
                            children: vec![index],
                        },
                        false,
                        None,
                    )?;
                }
            }
        }
    }
    for (endnote, stream) in [
        (false, input.footnotes.as_ref()),
        (true, input.endnotes.as_ref()),
    ] {
        let Some(stream) = stream else {
            continue;
        };
        for note in &stream.footnotes {
            if stream.get_by_id(note.id).is_none() {
                continue;
            }
            let story = if endnote {
                WordStory::Endnote { id: note.id }
            } else {
                WordStory::Footnote { id: note.id }
            };
            let context = collector
                .note_contexts
                .get(&story)
                .and_then(|contexts| (contexts.len() == 1).then(|| contexts[0]));
            if let Some(body) = input.story_bodies.get(&story) {
                collector.body(body, &story, false, context)?;
            } else {
                for (index, paragraph) in note.paragraphs.iter().enumerate() {
                    collector.paragraph(
                        paragraph,
                        &WordSourcePath {
                            story: story.clone(),
                            children: vec![index],
                        },
                        false,
                        context,
                    )?;
                }
            }
        }
    }
    collector.resolve_bookmark_requests();
    Ok(collector.snapshot)
}

impl SequenceCollector<'_> {
    fn body(
        &mut self,
        body: &CT_Body,
        story: &WordStory,
        main: bool,
        context: Option<usize>,
    ) -> Result<()> {
        for item in body_layout_items(body) {
            match item {
                MainStoryLayoutItem::Paragraph(paragraph, children) => self.paragraph(
                    paragraph,
                    &WordSourcePath {
                        story: story.clone(),
                        children,
                    },
                    main,
                    context,
                )?,
                MainStoryLayoutItem::Table(table, children) => {
                    self.table(table, story, &children, main, context)?
                }
            }
        }
        Ok(())
    }

    fn table(
        &mut self,
        table: &CT_Tbl,
        story: &WordStory,
        prefix: &[usize],
        main: bool,
        context: Option<usize>,
    ) -> Result<()> {
        for (row, row_path) in table::layout_table_rows(table, prefix) {
            for (cell, cell_path) in table::layout_row_cells(row, &row_path) {
                for (index, content) in cell.content.iter().enumerate() {
                    let mut path = cell_path.clone();
                    path.push(index);
                    self.cell_content(content, story, &path, main, context)?;
                }
            }
        }
        Ok(())
    }

    fn cell_content(
        &mut self,
        content: &CellContent,
        story: &WordStory,
        path: &[usize],
        main: bool,
        context: Option<usize>,
    ) -> Result<()> {
        match content {
            CellContent::Paragraph(paragraph) => self.paragraph(
                paragraph,
                &WordSourcePath {
                    story: story.clone(),
                    children: path.to_vec(),
                },
                main,
                context,
            ),
            CellContent::Table(table) => self.table(table, story, path, main, context),
            CellContent::ContentControl(control) => {
                self.cell_control(control, story, path, main, context)
            }
        }
    }

    fn cell_control(
        &mut self,
        control: &CT_Sdt,
        story: &WordStory,
        prefix: &[usize],
        main: bool,
        context: Option<usize>,
    ) -> Result<()> {
        for (index, content) in control.content.iter().enumerate() {
            let mut path = prefix.to_vec();
            path.push(index);
            match content {
                SdtContent::Paragraph(paragraph) => self.paragraph(
                    paragraph,
                    &WordSourcePath {
                        story: story.clone(),
                        children: path,
                    },
                    main,
                    context,
                )?,
                SdtContent::Table(table) => self.table(table, story, &path, main, context)?,
                SdtContent::ContentControl(control) => {
                    self.cell_control(control, story, &path, main, context)?
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn bookmark_boundaries(
        &mut self,
        markers: &[oxoffice_docx_oxml::text::BookmarkMarker],
        story: &WordStory,
        run: usize,
    ) {
        for marker in markers
            .iter()
            .filter(|marker| marker.projected_run_index() == run)
        {
            let Some(id) = marker.id() else {
                continue;
            };
            let boundary = self.snapshot.main_events.len();
            let order = self.bookmark_order;
            self.bookmark_order += 1;
            if marker.is_start() {
                if let Some(name) = marker.name() {
                    self.bookmark_starts.push((
                        id,
                        name.to_owned(),
                        story.clone(),
                        boundary,
                        order,
                    ));
                }
            } else {
                self.bookmark_ends
                    .push((id, story.clone(), boundary, order));
            }
        }
    }

    fn resolve_bookmark_requests(&mut self) {
        for (source, instruction) in std::mem::take(&mut self.bookmark_requests) {
            let target = field_text_argument(&instruction, 1).expect("validated bookmark operand");
            let starts = self
                .bookmark_starts
                .iter()
                .filter(|(_, name, _, _, _)| name == target)
                .collect::<Vec<_>>();
            let result = if starts.is_empty() {
                Ok("Error! Bookmark not defined.".into())
            } else if starts.len() != 1 {
                Err("SEQ bookmark target is ambiguous".into())
            } else {
                let (id, _, story, start, start_order) = starts[0];
                let ends = self
                    .bookmark_ends
                    .iter()
                    .filter(|(end_id, end_story, _, _)| end_id == id && end_story == story)
                    .collect::<Vec<_>>();
                if ends.len() != 1
                    || ends[0].3 < *start_order
                    || self
                        .bookmark_starts
                        .iter()
                        .filter(|(other_id, _, other_story, _, _)| {
                            other_id == id && other_story == story
                        })
                        .count()
                        != 1
                {
                    Err("SEQ bookmark target has no unique forward paired range".into())
                } else {
                    let end = ends[0].2;
                    let identifier = field_text_argument(&instruction, 0)
                        .expect("validated identifier")
                        .to_ascii_lowercase();
                    let indexes = self.snapshot.identifier_events.get(&identifier);
                    let in_range = indexes.and_then(|indexes| {
                        let upper = indexes.partition_point(|index| *index < end);
                        upper
                            .checked_sub(1)
                            .map(|index| indexes[index])
                            .filter(|index| *index >= *start)
                    });
                    let value = in_range
                        .map(|index| self.snapshot.main_events[index].value)
                        .or_else(|| self.snapshot.context_value(*start, &identifier))
                        .unwrap_or(0);
                    let display = if field_has_switch(&instruction, "h")
                        && !field_has_switch(&instruction, "*")
                    {
                        String::new()
                    } else {
                        value.to_string()
                    };
                    format_sequence_display(&instruction, display)
                }
            };
            self.snapshot.field_values.insert(source, result);
        }
    }

    fn paragraph(
        &mut self,
        paragraph: &CT_P,
        path: &WordSourcePath,
        main: bool,
        context: Option<usize>,
    ) -> Result<()> {
        let node = self.snapshot.source_id(path).ok_or_else(|| {
            LayoutError::Layout("sequence paragraph has no registered physical source".into())
        })?;
        if main {
            let mut properties = style_resolver::resolve_paragraph_properties(
                paragraph
                    .properties
                    .as_ref()
                    .and_then(|properties| properties.style_id.as_deref()),
                &self.input.styles,
            );
            if let Some(direct) = &paragraph.properties {
                properties.merge_from(direct);
            }
            let heading_level = properties.outline_lvl.or_else(|| {
                detect_heading_level(paragraph, &self.input.styles).map(|level| level - 1)
            });
            let cached = self
                .snapshot
                .navigation_exclusions
                .get(path)
                .is_some_and(|runs| {
                    runs.len() == project_paragraph_runs(paragraph, RevisionView::Accepted).len()
                });
            if !cached && let Some(level) = heading_level.filter(|level| *level < 9) {
                self.headings[level as usize] = Some(self.paragraph_index);
            }
            self.paragraph_index += 1;
        }
        let mut registered_fields = HashMap::new();
        let mut next_field = 0u32;
        for run in paragraph.source_runs() {
            for content in &run.content {
                if let RunContent::Field(field) = content {
                    registered_fields.insert(std::ptr::from_ref(field), next_field);
                    let mut pending = vec![field];
                    while let Some(field) = pending.pop() {
                        next_field += 1;
                        pending.extend(field.all_nested_fields_in_source_order().into_iter().rev());
                    }
                }
            }
        }
        let projected_runs = project_paragraph_runs(paragraph, RevisionView::Accepted);
        let projected_count = projected_runs.len();
        let current_bookmarks = paragraph.current_bookmark_projection();
        for (order, marker) in current_bookmarks.iter().enumerate() {
            let Some(id) = marker.id() else {
                continue;
            };
            let run = marker.projected_run_index();
            if run > projected_count {
                continue;
            }
            if marker.is_start() {
                if let Some(name) = marker.name() {
                    self.snapshot.position_bookmark_starts.push((
                        id,
                        name.to_owned(),
                        node,
                        run,
                        order,
                    ));
                }
            } else {
                self.snapshot
                    .position_bookmark_ends
                    .push((id, node, run, order));
            }
        }
        for (run_index, projected) in projected_runs.into_iter().enumerate() {
            if main {
                self.bookmark_boundaries(&current_bookmarks, &path.story, run_index);
            }
            for content in &projected.run.content {
                match content {
                    RunContent::Field(field) => {
                        if let Some(mut field_index) =
                            registered_fields.get(&std::ptr::from_ref(field)).copied()
                        {
                            self.field(
                                field,
                                node,
                                &mut field_index,
                                run_index,
                                main,
                                context,
                                false,
                                self.snapshot
                                    .navigation_exclusions
                                    .get(path)
                                    .is_some_and(|runs| runs.binary_search(&run_index).is_ok()),
                            );
                        }
                        if main {
                            for reference in
                                content_note_references(content, RevisionView::Accepted)?
                            {
                                let story = match reference {
                                    RunContent::FootnoteRef { id, .. } => {
                                        WordStory::Footnote { id }
                                    }
                                    RunContent::EndnoteRef { id, .. } => WordStory::Endnote { id },
                                    _ => continue,
                                };
                                self.note_contexts
                                    .entry(story)
                                    .or_default()
                                    .push(self.snapshot.main_events.len());
                            }
                        }
                    }
                    RunContent::FootnoteRef { id, .. } if main => self
                        .note_contexts
                        .entry(WordStory::Footnote { id: *id })
                        .or_default()
                        .push(self.snapshot.main_events.len()),
                    RunContent::EndnoteRef { id, .. } if main => self
                        .note_contexts
                        .entry(WordStory::Endnote { id: *id })
                        .or_default()
                        .push(self.snapshot.main_events.len()),
                    _ => {}
                }
            }
            // Selected typed anchors alone participate. Fallback drawings have
            // no second source and are not traversed alongside their Choice.
            let drawings = projected
                .run
                .content
                .iter()
                .filter_map(|content| match content {
                    RunContent::Drawing(drawing) => Some(drawing),
                    _ => None,
                })
                .chain(projected.run.alt_drawings.iter());
            for (drawing_index, drawing) in drawings.enumerate() {
                let Some(anchor) = drawing.anchor.as_ref() else {
                    continue;
                };
                if anchor.chart_rel_id.is_some() || !anchor.embed_id.is_empty() {
                    continue;
                }
                let Some(shape) = anchor.shape.as_ref() else {
                    continue;
                };
                let part_name = match &path.story {
                    WordStory::TextBox { part_name, .. } => Some(part_name.clone()),
                    story => self.input.story_part_names.get(story).cloned(),
                };
                let Some(part_name) = part_name else {
                    continue;
                };
                let mut owner_children = match &path.story {
                    WordStory::TextBox { owner_children, .. } => {
                        let mut result = owner_children.clone();
                        result.extend(&path.children);
                        result
                    }
                    WordStory::Footnote { id } | WordStory::Endnote { id } => {
                        let stream = if matches!(path.story, WordStory::Footnote { .. }) {
                            self.input.footnotes.as_ref()
                        } else {
                            self.input.endnotes.as_ref()
                        };
                        let mut result = stream
                            .and_then(|stream| {
                                stream.footnotes.iter().position(|note| note.id == *id)
                            })
                            .map(|index| vec![index])
                            .unwrap_or_default();
                        result.extend(&path.children);
                        result
                    }
                    _ => path.children.clone(),
                };
                owner_children.extend([run_index, drawing_index]);
                let story = WordStory::TextBox {
                    part_name,
                    owner_children,
                };
                if let Some(body) = &shape.text_body {
                    self.body(body, &story, main, context)?;
                } else {
                    for (index, paragraph) in shape.text.iter().enumerate() {
                        self.paragraph(
                            paragraph,
                            &WordSourcePath {
                                story: story.clone(),
                                children: vec![index],
                            },
                            main,
                            context,
                        )?;
                    }
                }
            }
        }
        if main {
            self.bookmark_boundaries(&current_bookmarks, &path.story, projected_count);
        }
        Ok(())
    }

    fn field(
        &mut self,
        field: &oxoffice_docx_oxml::text::Field,
        node: SourceNodeId,
        index: &mut u32,
        run: usize,
        main: bool,
        context: Option<usize>,
        locked: bool,
        cached: bool,
    ) {
        let source = FieldSource {
            node,
            index: *index,
        };
        *index += 1;
        let locked = locked || field.locked() == Some(true);
        if cached && let Some(context) = self.snapshot.field_source_contexts.get_mut(&source) {
            context.2 = true;
        }
        if !cached {
            self.snapshot.field_run_positions.insert(source, run);
        }
        for nested in field.nested_fields_in_source_order() {
            self.field(nested, node, index, run, main, context, locked, cached);
        }
        let instruction = field.effective_instruction();
        if instruction.name == "SEQ" {
            let value = if locked {
                Err("locked SEQ field retains its stored display".into())
            } else if cached {
                Err("cached SEQ result is not an independent sequence source".into())
            } else if !instruction.quotes_are_balanced() {
                Err("field SEQ has unclosed quoting".into())
            } else {
                self.sequence(&instruction, source, run, main, context)
            };
            if let Some((_, stored_display)) = self.snapshot.repeat_requests.get_mut(&source) {
                *stored_display = field.cached_result.clone();
            }
            let value = value.and_then(|value| format_sequence_display(&instruction, value));
            self.snapshot.field_values.insert(source, value);
        }
        for nested in field.cached_fields_in_source_order() {
            self.field(nested, node, index, run, main, context, locked, true);
        }
    }

    fn sequence(
        &mut self,
        instruction: &FieldInstruction,
        source: FieldSource,
        run: usize,
        main: bool,
        context: Option<usize>,
    ) -> std::result::Result<String, String> {
        let identifier = field_text_argument(instruction, 0)
            .filter(|identifier| !identifier.is_empty())
            .ok_or("SEQ requires an identifier")?
            .to_ascii_lowercase();
        if identifier.len() > 40
            || !identifier.as_bytes()[0].is_ascii_alphabetic()
            || !identifier
                .bytes()
                .all(|value| value.is_ascii_alphanumeric() || value == b'_')
        {
            return Err("SEQ identifier requires a letter followed by letters, digits or underscores, at most 40 characters".into());
        }
        if !(1..=2).contains(&instruction.arguments.len())
            || (instruction.arguments.len() == 2
                && field_text_argument(instruction, 1).is_none_or(str::is_empty))
        {
            return Err("SEQ requires an identifier and optional bookmark name".into());
        }
        for switch in &instruction.switches {
            let takes_argument = matches!(switch.name.as_str(), "r" | "s" | "*" | "#");
            if !matches!(
                switch.name.as_str(),
                "n" | "c" | "h" | "r" | "s" | "*" | "#"
            ) {
                return Err(format!("SEQ uses unsupported switch \\{}", switch.name));
            }
            if takes_argument != matches!(switch.argument, Some(FieldArgument::Text(_))) {
                return Err(format!(
                    "SEQ switch \\{} has an invalid operand",
                    switch.name
                ));
            }
        }
        if instruction.arguments.len() == 2 {
            if !main {
                return Err(
                    "SEQ bookmark reference lacks a supported related-story context".into(),
                );
            }
            self.bookmark_requests.push((source, instruction.clone()));
            return Err("SEQ bookmark reference awaits complete physical source traversal".into());
        }
        let repeat = field_has_switch(instruction, "c");
        if !main {
            if !repeat {
                return Ok("Error! Main Document Only.".into());
            }
            self.snapshot
                .repeat_requests
                .insert(source, (instruction.clone(), String::new()));
            let context = context.ok_or("SEQ repeat has no unique physical document context")?;
            let value = self
                .snapshot
                .context_value(context, &identifier)
                .ok_or("SEQ repeat has no preceding value")?;
            return Ok(value.to_string());
        }
        let heading = instruction
            .switches
            .iter()
            .find(|switch| switch.name == "s")
            .and_then(|switch| switch.argument.as_ref())
            .and_then(|argument| match argument {
                FieldArgument::Text(level) => level.parse::<usize>().ok(),
                _ => None,
            });
        let heading = if field_has_switch(instruction, "s") {
            Some(
                heading
                    .filter(|level| (1..=9).contains(level))
                    .and_then(|level| self.headings[level - 1])
                    .ok_or("SEQ heading restart has no matching heading")?,
            )
        } else {
            None
        };
        let counter = self.counters.entry(identifier.clone()).or_default();
        if heading.is_some() && counter.heading != heading {
            counter.value = None;
            counter.heading = heading;
        }
        let reset = instruction
            .switches
            .iter()
            .find(|switch| switch.name == "r")
            .and_then(|switch| switch.argument.as_ref());
        let value = if let Some(FieldArgument::Text(reset)) = reset {
            let value = reset
                .parse::<i64>()
                .map_err(|_| "SEQ reset value is not an integer")?;
            counter.value = Some(value);
            value
        } else if repeat {
            counter.value.ok_or("SEQ repeat has no preceding value")?
        } else {
            let value = counter
                .value
                .unwrap_or(0)
                .checked_add(1)
                .ok_or("SEQ value overflowed")?;
            counter.value = Some(value);
            value
        };
        if !repeat || reset.is_some() {
            let event = self.snapshot.main_events.len();
            self.snapshot
                .identifier_events
                .entry(identifier.clone())
                .or_default()
                .push(event);
            self.snapshot.event_sources.insert(source, event);
            self.snapshot.main_events.push(crate::WordSequenceEvent {
                source,
                accepted_run: run,
                identifier,
                value,
            });
        }
        if field_has_switch(instruction, "h") && !field_has_switch(instruction, "*") {
            Ok(String::new())
        } else {
            Ok(value.to_string())
        }
    }
}

fn project_paragraph_runs(para: &CT_P, view: RevisionView) -> Vec<ProjectedRun<'_>> {
    let mut projected = Vec::new();
    for boundary in 0..=para.runs.len() {
        let mut owners = para
            .content_controls
            .iter()
            .filter(|(at, _, _, _)| *at == boundary)
            .map(|(_, raw_before, _, control)| {
                (
                    RawOrder::Raw(*raw_before),
                    0u8,
                    ProjectedParagraphOwner::Control(control),
                )
            })
            .chain(
                para.revisions
                    .iter()
                    .filter(|(at, _, _)| *at == boundary)
                    .map(|(_, slot, revision)| {
                        (
                            paragraph_revision_raw_order(para, boundary, *slot),
                            1u8,
                            ProjectedParagraphOwner::Revision {
                                revision,
                                hyperlink_index: hyperlink_revision_index(*slot),
                            },
                        )
                    }),
            )
            .collect::<Vec<_>>();
        owners.sort_by_key(|(raw_order, kind, _)| (*raw_order, *kind));
        for (raw_order, _, owner) in owners {
            match owner {
                ProjectedParagraphOwner::Control(control) => {
                    project_control_runs(control, view, boundary, raw_order, &mut projected)
                }
                ProjectedParagraphOwner::Revision {
                    revision,
                    hyperlink_index,
                } => project_revision_runs(
                    revision,
                    view,
                    boundary,
                    raw_order,
                    hyperlink_index,
                    false,
                    false,
                    &mut projected,
                ),
            }
        }
        if let Some(run) = para.runs.get(boundary) {
            projected.push(ProjectedRun {
                run,
                boundary,
                raw_order: RawOrder::AfterRaw,
                ordinary_run_index: Some(boundary),
                hyperlink_index: None,
                force_underline: false,
                force_strike: false,
                revision: None,
            });
        }
    }
    projected
}

/// Whether the accepted view of `para` has nothing to lay out: no text, tab,
/// break, picture, field, symbol, special character or note reference, no
/// equation and no bookmark start.
///
/// A bookmark start keeps the paragraph, because a PAGEREF or REF field
/// reads the page, number and text of the paragraph that holds it, and
/// moving it onto the next paragraph, as accepting does, is not modeled.
fn accepted_paragraph_is_blank(para: &CT_P) -> bool {
    para.accepted_text().is_empty()
        && para.equations.is_empty()
        && !para.bookmark_markers.iter().any(|marker| marker.is_start())
        && project_paragraph_runs(para, RevisionView::Accepted)
            .iter()
            .all(|projected| {
                projected.run.alt_drawings.is_empty()
                    && projected.run.content.iter().all(|content| match content {
                        RunContent::Text(text) => text.text.is_empty(),
                        RunContent::DeletedText(_) | RunContent::CommentReference { .. } => true,
                        RunContent::Tab
                        | RunContent::Break(_)
                        | RunContent::Drawing(_)
                        | RunContent::Field(_)
                        | RunContent::FootnoteRef { .. }
                        | RunContent::EndnoteRef { .. }
                        | RunContent::Symbol { .. }
                        | RunContent::SpecialCharacter(_) => false,
                    })
            })
}

enum ProjectedParagraphOwner<'a> {
    Control(&'a CT_Sdt),
    Revision {
        revision: &'a CT_Revision,
        hyperlink_index: Option<usize>,
    },
}

fn paragraph_revision_raw_order(para: &CT_P, boundary: usize, slot: usize) -> RawOrder {
    let Some(index) = hyperlink_revision_index(slot) else {
        return RawOrder::Raw(slot);
    };
    if let Some(raw_before) = para
        .hyperlinks
        .get(index)
        .and_then(|hyperlink| hyperlink.preserved_raw_before)
    {
        RawOrder::Raw(raw_before)
    } else if para
        .hyperlinks
        .get(index)
        .is_some_and(|hyperlink| boundary == hyperlink.run_end)
    {
        RawOrder::BeforeRaw
    } else {
        RawOrder::AfterRaw
    }
}

fn project_control_runs<'a>(
    control: &'a CT_Sdt,
    view: RevisionView,
    boundary: usize,
    raw_order: RawOrder,
    projected: &mut Vec<ProjectedRun<'a>>,
) {
    for content_boundary in 0..=control.content.len() {
        for (_, revision) in control
            .revisions()
            .iter()
            .filter(|(at, _)| *at == content_boundary)
        {
            project_revision_runs(
                revision, view, boundary, raw_order, None, false, false, projected,
            );
        }
        if let Some(content) = control.content.get(content_boundary) {
            match content {
                SdtContent::Run(run) => projected.push(ProjectedRun {
                    run,
                    boundary,
                    raw_order,
                    ordinary_run_index: None,
                    hyperlink_index: None,
                    force_underline: false,
                    force_strike: false,
                    revision: None,
                }),
                SdtContent::ContentControl(control) => {
                    project_control_runs(control, view, boundary, raw_order, projected);
                }
                SdtContent::Paragraph(paragraph) => {
                    project_control_paragraph_runs(paragraph, view, boundary, raw_order, projected)
                }
                SdtContent::Table(table) => {
                    project_control_table_runs(table, view, boundary, raw_order, projected);
                }
                SdtContent::Row(row) => {
                    project_control_row_runs(row, view, boundary, raw_order, projected);
                }
                SdtContent::Cell(cell) => {
                    project_control_cell_runs(cell, view, boundary, raw_order, projected);
                }
                SdtContent::RawXml(_) => {}
            }
        }
    }
}

fn project_control_paragraph_runs<'a>(
    paragraph: &'a CT_P,
    view: RevisionView,
    boundary: usize,
    raw_order: RawOrder,
    projected: &mut Vec<ProjectedRun<'a>>,
) {
    for nested in project_paragraph_runs(paragraph, view) {
        projected.push(ProjectedRun {
            boundary,
            raw_order,
            ordinary_run_index: None,
            hyperlink_index: None,
            ..nested
        });
    }
}

fn project_control_table_runs<'a>(
    table: &'a CT_Tbl,
    view: RevisionView,
    boundary: usize,
    raw_order: RawOrder,
    projected: &mut Vec<ProjectedRun<'a>>,
) {
    for row_boundary in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == row_boundary)
        {
            project_control_runs(control, view, boundary, raw_order, projected);
        }
        if let Some(row) = table.rows.get(row_boundary) {
            project_control_row_runs(row, view, boundary, raw_order, projected);
        }
    }
}

fn project_control_row_runs<'a>(
    row: &'a CT_Row,
    view: RevisionView,
    boundary: usize,
    raw_order: RawOrder,
    projected: &mut Vec<ProjectedRun<'a>>,
) {
    for cell_boundary in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == cell_boundary)
        {
            project_control_runs(control, view, boundary, raw_order, projected);
        }
        if let Some(cell) = row.cells.get(cell_boundary) {
            project_control_cell_runs(cell, view, boundary, raw_order, projected);
        }
    }
}

fn project_control_cell_runs<'a>(
    cell: &'a CT_Tc,
    view: RevisionView,
    boundary: usize,
    raw_order: RawOrder,
    projected: &mut Vec<ProjectedRun<'a>>,
) {
    for content in &cell.content {
        match content {
            CellContent::Paragraph(paragraph) => {
                project_control_paragraph_runs(paragraph, view, boundary, raw_order, projected)
            }
            CellContent::Table(table) => {
                project_control_table_runs(table, view, boundary, raw_order, projected);
            }
            CellContent::ContentControl(control) => {
                project_control_runs(control, view, boundary, raw_order, projected);
            }
        }
    }
}

fn project_revision_runs<'a>(
    revision: &'a CT_Revision,
    view: RevisionView,
    boundary: usize,
    raw_order: RawOrder,
    hyperlink_index: Option<usize>,
    inherited_underline: bool,
    inherited_strike: bool,
    projected: &mut Vec<ProjectedRun<'a>>,
) {
    let included = match view {
        RevisionView::Tracked => true,
        RevisionView::Accepted => matches!(
            revision.kind(),
            RevisionKind::Insertion | RevisionKind::MoveTo
        ),
    };
    if !included {
        return;
    }

    let force_underline = inherited_underline
        || (view == RevisionView::Tracked
            && matches!(
                revision.kind(),
                RevisionKind::Insertion | RevisionKind::MoveTo
            ));
    let force_strike = inherited_strike
        || (view == RevisionView::Tracked
            && matches!(
                revision.kind(),
                RevisionKind::Deletion | RevisionKind::MoveFrom
            ));
    let marked = (view == RevisionView::Tracked).then(|| revision.kind());
    if let Some(paragraph) = revision.content_paragraph() {
        for nested in project_paragraph_runs(paragraph, view) {
            projected.push(ProjectedRun {
                boundary,
                raw_order,
                ordinary_run_index: None,
                hyperlink_index,
                force_underline: force_underline || nested.force_underline,
                force_strike: force_strike || nested.force_strike,
                revision: nested.revision.or(marked),
                ..nested
            });
        }
        return;
    }
    let runs = match revision.content() {
        RevisionContent::Runs(runs) => runs.as_slice(),
        RevisionContent::Marker => &[],
        RevisionContent::PriorRunProperties(_)
        | RevisionContent::PriorParagraphProperties(_)
        | RevisionContent::PriorTableProperties(_)
        | RevisionContent::PriorSectionProperties(_) => return,
    };

    for run_boundary in 0..=runs.len() {
        for (_, nested) in revision
            .nested_revisions()
            .iter()
            .filter(|(at, _)| *at == run_boundary)
        {
            project_revision_runs(
                nested,
                view,
                boundary,
                raw_order,
                hyperlink_index,
                force_underline,
                force_strike,
                projected,
            );
        }
        if let Some(run) = runs.get(run_boundary) {
            projected.push(ProjectedRun {
                run,
                boundary,
                raw_order,
                ordinary_run_index: None,
                hyperlink_index,
                force_underline,
                force_strike,
                revision: marked,
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_equations_before_order(
    paragraph: &CT_P,
    boundary: usize,
    order: RawOrder,
    cursor: &mut usize,
    inline_items: &mut Vec<InlineItem>,
    fm: &mut FontManager,
    font_size: f64,
    color: Color,
    available_width: f64,
    math_properties: Option<&oxoffice_docx_oxml::math::MathProperties>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    while let Some((equation_boundary, raw_before, equation)) = paragraph.equations.get(*cursor) {
        if *equation_boundary > boundary
            || (*equation_boundary == boundary
                && !match order {
                    RawOrder::BeforeRaw => false,
                    RawOrder::Raw(limit) => *raw_before < limit,
                    RawOrder::AfterRaw => true,
                })
        {
            break;
        }
        let source_path =
            format!("paragraph/run-boundary/{equation_boundary}/raw-child/{raw_before}");
        let (measured, display) = crate::math::layout_officemath(
            equation,
            fm,
            font_size,
            color,
            available_width,
            math_properties,
            &source_path,
            diagnostics,
        )?;
        if display
            && !inline_items.is_empty()
            && !matches!(inline_items.last(), Some(InlineItem::LineBreak))
        {
            inline_items.push(InlineItem::LineBreak);
        }
        inline_items.push(InlineItem::Group {
            width: measured.width,
            height: measured.height(),
            baseline: Some(measured.ascent),
            group: measured.group,
        });
        if display {
            inline_items.push(InlineItem::LineBreak);
        }
        *cursor += 1;
    }
    Ok(())
}

fn projected_paragraph_text(para: &CT_P, view: RevisionView) -> Result<String> {
    let mut text = String::new();
    for projected in project_paragraph_runs(para, view) {
        for content in &projected.run.content {
            if let RunContent::Field(field) = content {
                for stored in stored_note_cache_runs(field, None, view, None, false, None, false)? {
                    text.push_str(&stored.run.text());
                }
            } else {
                let run = CT_R {
                    content: vec![content.clone()],
                    ..CT_R::new("")
                };
                text.push_str(&run.text());
            }
        }
    }
    Ok(text)
}

fn projected_content_char_starts(run: &CT_R) -> Vec<usize> {
    let mut starts = Vec::with_capacity(run.content.len());
    let mut char_offset = 0usize;
    for content in &run.content {
        starts.push(char_offset);
        char_offset += match content {
            RunContent::Text(text) | RunContent::DeletedText(text) => text.text.chars().count(),
            RunContent::Tab | RunContent::Break(_) => 1,
            RunContent::Field(field) => field
                .effective_projected_text()
                .map_or(0, |text| text.chars().count()),
            RunContent::SpecialCharacter(SpecialCharacter::CarriageReturn) => 1,
            RunContent::FootnoteRef { custom_mark, .. }
            | RunContent::EndnoteRef { custom_mark, .. } => {
                custom_mark.as_ref().map_or(0, |mark| mark.chars().count())
            }
            RunContent::Drawing(_)
            | RunContent::CommentReference { .. }
            | RunContent::Symbol { .. }
            | RunContent::SpecialCharacter(_) => 0,
        };
    }
    debug_assert_eq!(char_offset, run.text().chars().count());
    starts
}

fn paragraph_has_visible_revision(para: &CT_P) -> bool {
    let property_revision = para.properties.as_ref().is_some_and(|properties| {
        properties.numbering_revision.is_some()
            || properties.change.is_some()
            || properties
                .sect_pr
                .as_ref()
                .is_some_and(|section| section.change.is_some())
            || properties
                .rpr
                .as_ref()
                .is_some_and(run_properties_have_revision)
    });
    property_revision
        || para
            .runs
            .iter()
            .filter_map(|run| run.properties.as_ref())
            .any(run_properties_have_revision)
        || para
            .content_controls
            .iter()
            .any(|(_, _, _, control)| control_has_visible_revision(control))
        || para
            .revisions
            .iter()
            .any(|(_, _, revision)| revision_is_visible(revision))
}

/// The colour of an insertion, deletion or move in the legal revision style.
fn legal_revision_color(kind: RevisionKind) -> Option<Color> {
    let (r, g, b) = match kind {
        RevisionKind::Insertion => (0, 0, 0xFF),
        RevisionKind::Deletion => (0xFF, 0, 0),
        RevisionKind::MoveFrom | RevisionKind::MoveTo => (0, 0xA0, 0),
        _ => return None,
    };
    Some(Color {
        r: f64::from(r) / 255.0,
        g: f64::from(g) / 255.0,
        b: f64::from(b) / 255.0,
        a: 1.0,
    })
}

/// The text colour and underline of `projected`, given those of its run:
/// the tracked view underlines an insertion or a move destination, double
/// in the legal style, which also gives the run the colour of its revision.
fn tracked_marks(
    projected: &ProjectedRun<'_>,
    style: RevisionStyle,
    color: Color,
    underline: Option<Underline>,
) -> (Color, Option<Underline>) {
    let legal = style == RevisionStyle::Legal;
    let underline = match (projected.force_underline, legal) {
        (true, true) => Some(Underline::Double),
        (true, false) => Some(Underline::Single),
        (false, _) => underline,
    };
    let color = projected
        .revision
        .filter(|_| legal)
        .and_then(legal_revision_color)
        .unwrap_or(color);
    (color, underline)
}

/// The colour of the change bar beside `para`, or `None` when the tracked
/// view shows no revision in it. In the legal style the bar takes the colour
/// of the first insertion, deletion or move, its runs' or its mark's.
fn change_bar_color(para: &CT_P, input: &LayoutInput) -> Option<Color> {
    if input.revision_view != RevisionView::Tracked || !paragraph_has_visible_revision(para) {
        return None;
    }
    if input.revision_style == RevisionStyle::Plain {
        return Some(Color::BLACK);
    }
    let mark = para
        .properties
        .as_ref()
        .and_then(|properties| properties.rpr.as_ref())
        .into_iter()
        .flat_map(|properties| properties.revision_markers.iter().map(CT_Revision::kind));
    project_paragraph_runs(para, RevisionView::Tracked)
        .iter()
        .filter(|projected| run_has_visible_content(projected.run))
        .filter_map(|projected| projected.revision)
        .chain(mark)
        .find_map(legal_revision_color)
        .or(Some(Color::BLACK))
}

fn run_properties_have_revision(properties: &oxoffice_docx_oxml::properties::CT_RPr) -> bool {
    properties.change.is_some() || !properties.revision_markers.is_empty()
}

fn run_has_visible_content(run: &CT_R) -> bool {
    run.content.iter().any(|content| match content {
        RunContent::Text(text) | RunContent::DeletedText(text) => !text.text.is_empty(),
        RunContent::CommentReference { .. } => false,
        RunContent::Tab
        | RunContent::Break(_)
        | RunContent::Drawing(_)
        | RunContent::Field(_)
        | RunContent::FootnoteRef { .. }
        | RunContent::EndnoteRef { .. }
        | RunContent::Symbol { .. }
        | RunContent::SpecialCharacter(_) => true,
    })
}

fn control_has_visible_revision(control: &CT_Sdt) -> bool {
    control
        .revisions()
        .iter()
        .any(|(_, revision)| revision_is_visible(revision))
        || control.content.iter().any(|content| match content {
            SdtContent::Paragraph(paragraph) => paragraph_has_visible_revision(paragraph),
            SdtContent::ContentControl(control) => control_has_visible_revision(control),
            SdtContent::Table(table) => {
                table
                    .content_controls
                    .iter()
                    .any(|(_, _, control)| control_has_visible_revision(control))
                    || table.rows.iter().any(row_has_visible_control_revision)
            }
            SdtContent::Row(row) => row_has_visible_control_revision(row),
            SdtContent::Cell(cell) => cell_has_visible_control_revision(cell),
            SdtContent::Run(run) => run
                .properties
                .as_ref()
                .is_some_and(run_properties_have_revision),
            SdtContent::RawXml(_) => false,
        })
}

fn row_has_visible_control_revision(row: &CT_Row) -> bool {
    row.content_controls
        .iter()
        .any(|(_, _, control)| control_has_visible_revision(control))
        || row.cells.iter().any(cell_has_visible_control_revision)
}

fn cell_has_visible_control_revision(cell: &CT_Tc) -> bool {
    cell.content.iter().any(|content| match content {
        CellContent::Paragraph(paragraph) => paragraph_has_visible_revision(paragraph),
        CellContent::Table(table) => {
            table
                .content_controls
                .iter()
                .any(|(_, _, control)| control_has_visible_revision(control))
                || table.rows.iter().any(row_has_visible_control_revision)
        }
        CellContent::ContentControl(control) => control_has_visible_revision(control),
    })
}

fn revision_is_visible(revision: &CT_Revision) -> bool {
    if let Some(paragraph) = revision.content_paragraph() {
        return project_paragraph_runs(paragraph, RevisionView::Tracked)
            .iter()
            .any(|projected| run_has_visible_content(projected.run))
            || paragraph_has_visible_revision(paragraph);
    }
    match revision.content() {
        RevisionContent::Runs(runs) => {
            runs.iter().any(run_has_visible_content)
                || revision
                    .nested_revisions()
                    .iter()
                    .any(|(_, nested)| revision_is_visible(nested))
        }
        RevisionContent::Marker => revision
            .nested_revisions()
            .iter()
            .any(|(_, nested)| revision_is_visible(nested)),
        RevisionContent::PriorRunProperties(_)
        | RevisionContent::PriorParagraphProperties(_)
        | RevisionContent::PriorTableProperties(_)
        | RevisionContent::PriorSectionProperties(_) => true,
    }
}

/// The layout engine.
pub struct Engine {
    font_manager: FontManager,
    caller_font_aliases: Vec<(String, String)>,
    paragraph_cache_context: Option<ReusableEngineContext>,
    paragraph_cache: VecDeque<ParagraphCacheEntry>,
    paragraph_cache_bytes: usize,
    pending_paragraph_cache: Option<VecDeque<ParagraphCacheEntry>>,
    pending_paragraph_cache_bytes: usize,
    paragraph_cache_reads_enabled: bool,
    paragraph_cache_notes_match: bool,
    retained_context_matches_full: bool,
    table_cache: VecDeque<TableCacheEntry>,
    table_cache_bytes: usize,
    pending_table_cache: Option<VecDeque<TableCacheEntry>>,
    pending_table_cache_bytes: usize,
    header_footer_cache: VecDeque<HeaderFooterCacheEntry>,
    header_footer_cache_bytes: usize,
    pending_header_footer_cache: Option<VecDeque<HeaderFooterCacheEntry>>,
    pending_header_footer_cache_bytes: usize,
    header_footer_cache_reads_enabled: bool,
    restart_cache: Option<RestartCache>,
    numbering_by_source: HashMap<SourceNodeId, ResolvedNumbering>,
    navigation_by_source: HashMap<SourceNodeId, crate::ParagraphNavigation>,
    bookmark_numbering: HashMap<String, ResolvedNumbering>,
    bookmark_suppressed: std::collections::HashSet<String>,
    last_body_fragments: Vec<Vec<WordBodyLayoutFragment>>,
    last_page_sections: Vec<crate::WordPageSection>,
    last_field_placements: Vec<crate::WordFieldPlacement>,
    last_bookmark_pages: HashMap<usize, crate::WordPageSection>,
    last_field_source_xml: HashMap<FieldSource, Vec<u8>>,
    last_text_box_owner_indices: HashMap<WordStory, usize>,
    last_picture_draw_owners: PictureDrawOwners,
}

#[derive(Clone, PartialEq)]
struct ReusableEngineContext {
    sequence_snapshot: Option<Arc<crate::WordSequenceSnapshot>>,
    revision_view: RevisionView,
    revision_style: RevisionStyle,
    automatic_hyphenation: bool,
    mirror_margins: bool,
    gutter_at_top: bool,
    do_not_use_html_paragraph_auto_spacing: bool,
    default_tab_stop: Option<oxoffice_docx_oxml::units::Twips>,
    clamp_tabs_past_margin: bool,
    legacy_table_positioning: bool,
    modern_footnote_layout: bool,
    footnote_layout_like_word8: bool,
    math_properties: Option<oxoffice_docx_oxml::math::MathProperties>,
    note_defaults: [Option<oxoffice_docx_oxml::document::CT_NoteProperties>; 2],
    has_wrapping_drawing: bool,
    styles: CT_Styles,
    numbering: Option<oxoffice_docx_oxml::numbering::CT_Numbering>,
    sections: Vec<CT_SectPr>,
    headers: HashMap<String, oxoffice_docx_oxml::header_footer::CT_HdrFtr>,
    footers: HashMap<String, oxoffice_docx_oxml::header_footer::CT_HdrFtr>,
    story_bodies: HashMap<WordStory, oxoffice_docx_oxml::document::CT_Body>,
    story_part_names: HashMap<WordStory, String>,
    images: HashMap<String, crate::input::ImageData>,
    charts: HashMap<String, std::result::Result<Box<oxoffice_chart::CT_ChartSpace>, String>>,
    chart_theme: oxoffice_drawing::theme::CT_OfficeStyleSheet,
    chart_color_map: oxoffice_drawing::color::ColorMap,
    /// Boxed, because every `Document` holds two engines inline and debug
    /// builds keep many copies of `Document` on deep stacks.
    core_properties: Option<Box<oxoffice_docx_oxml::core_properties::CoreProperties>>,
    hyperlink_urls: HashMap<String, String>,
    footnotes: Option<oxoffice_docx_oxml::footnotes::CT_Footnotes>,
    endnotes: Option<oxoffice_docx_oxml::footnotes::CT_Footnotes>,
    theme: Option<oxoffice_docx_oxml::theme::Theme>,
    fonts: Vec<oxoffice_layout::FontFile>,
    embedded_fonts: Vec<EmbeddedFontSource>,
    caller_font_aliases: Vec<(String, String)>,
    background_xml: Option<Vec<u8>>,
}

fn retained_context_fonts_match(
    retained: &[oxoffice_layout::FontFile],
    input: &[oxoffice_layout::FontFile],
) -> bool {
    retained == input
}

impl ReusableEngineContext {
    fn for_input_with_wrap(
        input: &LayoutInput,
        caller_font_aliases: &[(String, String)],
        has_wrapping_drawing: bool,
    ) -> Self {
        let sections = document_sections(&input.document)
            .into_iter()
            .cloned()
            .collect();
        Self {
            sequence_snapshot: input.sequence_snapshot.clone(),
            revision_view: input.revision_view,
            revision_style: input.revision_style,
            automatic_hyphenation: input.automatic_hyphenation,
            mirror_margins: input.mirror_margins,
            gutter_at_top: input.gutter_at_top,
            do_not_use_html_paragraph_auto_spacing: input.do_not_use_html_paragraph_auto_spacing,
            default_tab_stop: input.default_tab_stop,
            clamp_tabs_past_margin: input.clamp_tabs_past_margin,
            legacy_table_positioning: input.legacy_table_positioning,
            modern_footnote_layout: input.modern_footnote_layout,
            footnote_layout_like_word8: input.footnote_layout_like_word8,
            math_properties: input.math_properties.clone(),
            note_defaults: input.note_defaults.clone(),
            has_wrapping_drawing,
            styles: input.styles.clone(),
            numbering: input.numbering.clone(),
            sections,
            headers: input.headers.clone(),
            footers: input.footers.clone(),
            story_bodies: input.story_bodies.clone(),
            story_part_names: input.story_part_names.clone(),
            images: input.images.clone(),
            charts: input.charts.clone(),
            chart_theme: input.chart_theme.clone(),
            chart_color_map: input.chart_color_map.clone(),
            core_properties: input.core_properties.clone().map(Box::new),
            hyperlink_urls: input.hyperlink_urls.clone(),
            footnotes: input.footnotes.clone(),
            endnotes: input.endnotes.clone(),
            theme: input.theme.clone(),
            fonts: input.fonts.clone(),
            embedded_fonts: input.embedded_fonts.clone(),
            caller_font_aliases: bounded_caller_aliases(caller_font_aliases),
            background_xml: input.document.background_xml.clone(),
        }
    }

    fn matches_input(
        &self,
        input: &LayoutInput,
        caller_font_aliases: &[(String, String)],
        has_wrapping_drawing: bool,
    ) -> bool {
        self.matches_input_after_unchanged_fonts(input, caller_font_aliases, has_wrapping_drawing)
            && retained_context_fonts_match(&self.fonts, &input.fonts)
            && self.embedded_fonts == input.embedded_fonts
    }

    fn matches_input_after_unchanged_fonts(
        &self,
        input: &LayoutInput,
        caller_font_aliases: &[(String, String)],
        has_wrapping_drawing: bool,
    ) -> bool {
        self.matches_input_after_unchanged_fonts_ignoring_notes(
            input,
            caller_font_aliases,
            has_wrapping_drawing,
        ) && self.notes_match(input)
    }

    fn notes_match(&self, input: &LayoutInput) -> bool {
        self.footnotes == input.footnotes
            && self.endnotes == input.endnotes
            && self
                .story_bodies
                .iter()
                .filter(|(story, _)| {
                    matches!(
                        story,
                        WordStory::Footnote { .. } | WordStory::Endnote { .. }
                    )
                })
                .all(|(story, body)| input.story_bodies.get(story) == Some(body))
            && input
                .story_bodies
                .iter()
                .filter(|(story, _)| {
                    matches!(
                        story,
                        WordStory::Footnote { .. } | WordStory::Endnote { .. }
                    )
                })
                .all(|(story, body)| self.story_bodies.get(story) == Some(body))
    }

    fn matches_input_after_unchanged_fonts_ignoring_notes(
        &self,
        input: &LayoutInput,
        caller_font_aliases: &[(String, String)],
        has_wrapping_drawing: bool,
    ) -> bool {
        let sections_match = self.sections.iter().eq(document_sections(&input.document));
        self.sequence_snapshot == input.sequence_snapshot
            && self.revision_view == input.revision_view
            && self.revision_style == input.revision_style
            && self.automatic_hyphenation == input.automatic_hyphenation
            && self.mirror_margins == input.mirror_margins
            && self.gutter_at_top == input.gutter_at_top
            && self.do_not_use_html_paragraph_auto_spacing
                == input.do_not_use_html_paragraph_auto_spacing
            && self.default_tab_stop == input.default_tab_stop
            && self.clamp_tabs_past_margin == input.clamp_tabs_past_margin
            && self.legacy_table_positioning == input.legacy_table_positioning
            && self.modern_footnote_layout == input.modern_footnote_layout
            && self.footnote_layout_like_word8 == input.footnote_layout_like_word8
            && self.math_properties == input.math_properties
            && self.note_defaults == input.note_defaults
            && self.has_wrapping_drawing == has_wrapping_drawing
            && self.styles == input.styles
            && self.numbering == input.numbering
            && sections_match
            && self.headers == input.headers
            && self.footers == input.footers
            && self.story_part_names == input.story_part_names
            && self
                .story_bodies
                .iter()
                .filter(|(story, _)| {
                    !matches!(
                        story,
                        WordStory::Footnote { .. } | WordStory::Endnote { .. }
                    )
                })
                .all(|(story, body)| input.story_bodies.get(story) == Some(body))
            && input
                .story_bodies
                .iter()
                .filter(|(story, _)| {
                    !matches!(
                        story,
                        WordStory::Footnote { .. } | WordStory::Endnote { .. }
                    )
                })
                .all(|(story, body)| self.story_bodies.get(story) == Some(body))
            && self.images == input.images
            && self.charts == input.charts
            && self.chart_theme == input.chart_theme
            && self.chart_color_map == input.chart_color_map
            && self.core_properties.as_deref() == input.core_properties.as_ref()
            && self.hyperlink_urls == input.hyperlink_urls
            && self.theme == input.theme
            && self.caller_font_aliases == caller_font_aliases
            && self.background_xml == input.document.background_xml
    }
}

#[derive(Clone, PartialEq)]
struct ParagraphCacheKey {
    paragraph: CT_P,
    content_width_bits: u64,
    revision_view: RevisionView,
    revision_style: RevisionStyle,
    /// The section character grid the block was broken against.
    ///
    /// A gridded and an ungridded section give the same paragraph different
    /// line advance, so a cache keyed without this would serve one section's
    /// blocks to the other.
    doc_grid: Option<CT_DocGrid>,
}

struct ParagraphCacheEntry {
    fingerprint: u64,
    key: ParagraphCacheKey,
    block: Arc<ParagraphBlock>,
    paint: Arc<WordParagraphPaint>,
    diagnostics: Vec<Diagnostic>,
    font_trace: Vec<FontTraceEvent>,
    reflow_direction: TextDirection,
    bytes: usize,
}

#[derive(Clone, PartialEq)]
struct TableCacheKey {
    table: CT_Tbl,
    content_width_bits: u64,
    revision_view: RevisionView,
    revision_style: RevisionStyle,
    with_provenance: bool,
    /// The section character grid, for the reason `ParagraphCacheKey` states.
    doc_grid: Option<CT_DocGrid>,
}

struct TableCacheEntry {
    fingerprint: u64,
    key: TableCacheKey,
    block: Arc<table::TableBlock>,
    semantics: TableSemantics,
    diagnostics: Vec<Diagnostic>,
    font_trace: Vec<FontTraceEvent>,
    bytes: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HeaderFooterStoryKind {
    Header,
    Footer,
}

#[derive(Clone, PartialEq)]
struct HeaderFooterCacheKey {
    story: HeaderFooterStoryKind,
    variant: HdrFtrType,
    section: CT_SectPr,
    relationship_id: String,
    part: oxoffice_docx_oxml::header_footer::CT_HdrFtr,
    resolved_part_bytes: Vec<u8>,
    with_provenance: bool,
    rich_body: Option<oxoffice_docx_oxml::document::CT_Body>,
    source_part_name: Option<String>,
}

#[derive(Clone)]
struct HeaderFooterVariantContent {
    blocks: Vec<ParagraphBlock>,
    directions: Vec<paginator::HeaderFooterBlockSemantics>,
    watermark: Option<GroupElement>,
}

struct HeaderFooterCacheEntry {
    key: HeaderFooterCacheKey,
    content: HeaderFooterVariantContent,
    diagnostics: Vec<Diagnostic>,
    picture_draw_owners: PictureDrawOwners,
    font_trace: Vec<FontTraceEvent>,
    bytes: usize,
}

#[derive(Clone, PartialEq)]
enum RestartPaint {
    Paragraph(Arc<WordParagraphPaint>),
    Table(TableSemantics),
}

struct RestartCache {
    paint: Vec<RestartPaint>,
    body: Vec<RestartBodyEntry>,
    with_provenance: bool,
    body_fragments: Vec<Vec<WordBodyLayoutFragment>>,
    raw_pages: Vec<Arc<PageFrame>>,
    raw_policies: Vec<PaintPolicies>,
    pages: Vec<Arc<PageFrame>>,
    policies: Vec<PaintPolicies>,
    substitution_inputs: Vec<Option<FieldSubstitutionInputs>>,
    outlines: Vec<oxoffice_layout::OutlineEntry>,
    checkpoints: Vec<paginator::PaginationCheckpoint>,
    font_trace: Vec<FontId>,
    bytes: usize,
}

#[derive(Clone)]
enum RestartBodyEntry {
    Paragraph {
        fingerprint: u64,
        identity: Vec<u8>,
        note_references: Vec<NoteRef>,
        bytes: usize,
    },
    Table {
        fingerprint: u64,
        identity: Vec<u8>,
        bytes: usize,
    },
}

impl RestartBodyEntry {
    fn matches_memoized(
        &self,
        content: &BodyContent,
        index: usize,
        memo: &mut RestartBodyIdentityMemo,
    ) -> bool {
        if !self.fingerprint_matches(content) {
            return false;
        }
        self.identity_matches(memo.identity(index, content))
    }

    fn fingerprint_matches(&self, content: &BodyContent) -> bool {
        match (self, content) {
            (Self::Paragraph { fingerprint, .. }, BodyContent::Paragraph(paragraph)) => {
                *fingerprint == paragraph_fingerprint(paragraph)
            }
            (Self::Table { fingerprint, .. }, BodyContent::Table(table)) => {
                *fingerprint == table_fingerprint(table)
            }
            _ => false,
        }
    }

    fn identity_matches(&self, candidate: Option<&[u8]>) -> bool {
        match self {
            Self::Paragraph { identity, .. } | Self::Table { identity, .. } => {
                candidate == Some(identity.as_slice())
            }
        }
    }

    fn for_content_memoized(
        content: &BodyContent,
        view: RevisionView,
        index: usize,
        memo: &mut RestartBodyIdentityMemo,
    ) -> Option<Self> {
        let identity = memo.take_identity(index, content)?;
        Self::for_content_with_identity(content, view, identity)
    }

    fn for_content_with_identity(
        content: &BodyContent,
        view: RevisionView,
        identity: Vec<u8>,
    ) -> Option<Self> {
        match content {
            BodyContent::Paragraph(paragraph) => {
                let mut note_references = paragraph_note_references(paragraph, view).ok()?;
                note_references.shrink_to_fit();
                let bytes = identity.capacity().saturating_add(
                    note_references
                        .capacity()
                        .saturating_mul(std::mem::size_of::<NoteRef>()),
                );
                Some(Self::Paragraph {
                    fingerprint: paragraph_fingerprint(paragraph),
                    identity,
                    note_references,
                    bytes,
                })
            }
            BodyContent::Table(table) => Some(Self::Table {
                fingerprint: table_fingerprint(table),
                bytes: identity.capacity(),
                identity,
            }),
            BodyContent::ContentControl(_) | BodyContent::RawXml(_) => None,
        }
    }

    fn bytes(&self) -> usize {
        match self {
            Self::Paragraph { bytes, .. } | Self::Table { bytes, .. } => *bytes,
        }
    }

    fn note_references(&self) -> &[NoteRef] {
        match self {
            Self::Paragraph {
                note_references, ..
            } => note_references,
            Self::Table { .. } => &[],
        }
    }
}

enum RestartBodyIdentitySlot {
    NotComputed,
    Unserializable,
    Computed(Vec<u8>),
}

struct RestartBodyIdentityMemo {
    slots: Vec<RestartBodyIdentitySlot>,
}

impl RestartBodyIdentityMemo {
    fn new(len: usize) -> Self {
        Self {
            slots: std::iter::repeat_with(|| RestartBodyIdentitySlot::NotComputed)
                .take(len)
                .collect(),
        }
    }

    fn compute(&mut self, index: usize, content: &BodyContent) {
        let Some(slot) = self.slots.get(index) else {
            return;
        };
        if !matches!(slot, RestartBodyIdentitySlot::NotComputed) {
            return;
        }
        let identity = restart_body_identity(content);
        let replacement = match identity {
            Some(identity) => RestartBodyIdentitySlot::Computed(identity),
            None => RestartBodyIdentitySlot::Unserializable,
        };
        if let Some(slot) = self.slots.get_mut(index) {
            *slot = replacement;
        }
    }

    fn identity(&mut self, index: usize, content: &BodyContent) -> Option<&[u8]> {
        self.compute(index, content);
        match self.slots.get(index)? {
            RestartBodyIdentitySlot::Computed(identity) => Some(identity),
            RestartBodyIdentitySlot::NotComputed | RestartBodyIdentitySlot::Unserializable => None,
        }
    }

    fn take_identity(&mut self, index: usize, content: &BodyContent) -> Option<Vec<u8>> {
        self.compute(index, content);
        let RestartBodyIdentitySlot::Computed(identity) = self.slots.get_mut(index)? else {
            return None;
        };
        Some(std::mem::take(identity))
    }
}

fn restart_body_identity(content: &BodyContent) -> Option<Vec<u8>> {
    if !matches!(content, BodyContent::Paragraph(_) | BodyContent::Table(_)) {
        return None;
    }
    let mut document = CT_Document::new();
    document.body.sect_pr = None;
    document.body.content.push(content.clone());
    let mut identity = document.to_xml().ok()?;
    identity.shrink_to_fit();
    Some(identity)
}

/// One stored run from frozen operation-local prepared result storage.
struct StoredFieldRun {
    occurrence: Option<WordFieldOccurrence>,
    run: CT_R,
    force_underline: bool,
    force_strike: bool,
    revision: Option<RevisionKind>,
    source: Option<FieldSource>,
    field_kind: Option<FieldKind>,
}

/// Project each current stored owner once, retaining actual inner view marks.
/// Source association is qualified by the outer source and actual accepted
/// run/content slot before any prepared address is used for consumption.
fn stored_note_cache_runs(
    field: &oxoffice_docx_oxml::text::Field,
    inherited: Option<&CT_RPr>,
    view: RevisionView,
    source: Option<FieldSource>,
    inherited_locked: bool,
    input: Option<&LayoutInput>,
    cached_child: bool,
) -> Result<Vec<StoredFieldRun>> {
    let occurrence = source.map(|source| WordFieldOccurrence {
        source,
        owner_id: field.source_owner_id(),
        paint_scope: 0,
    });
    let locked = inherited_locked || field.locked() == Some(true);
    let field_kind = if cached_child && !locked && source.is_some() {
        let instruction = field.effective_instruction();
        if validate_page_field_instruction(&instruction).is_err() {
            None
        } else {
            match instruction.name.as_str() {
                "PAGE" => Some(FieldKind::Page),
                "NUMPAGES" => Some(FieldKind::NumPages),
                "SECTION" => Some(FieldKind::Section),
                "SECTIONPAGES" => Some(FieldKind::SectionPages),
                "PAGEREF" => input
                    .and_then(|input| {
                        field_text_argument(&instruction, 0)
                            .and_then(|name| page_ref_id(input, name))
                    })
                    .map(FieldKind::TargetPage),
                _ => None,
            }
        }
    } else {
        None
    };
    let prepared = field.cached_result_owner_projection().map_err(|error| {
        LayoutError::Layout(format!("stored field preparation failed: {error}"))
    })?;
    let runs = if let Some(paragraph) = &prepared {
        project_paragraph_runs(paragraph, view)
    } else if let Some(runs) = field.cached_result_runs() {
        runs.iter()
            .enumerate()
            .map(|(boundary, run)| ProjectedRun {
                run,
                boundary,
                raw_order: RawOrder::BeforeRaw,
                ordinary_run_index: Some(boundary),
                hyperlink_index: None,
                force_underline: false,
                force_strike: false,
                revision: None,
            })
            .collect()
    } else {
        let mut run = CT_R::new(&field.effective_display());
        run.properties = inherited.cloned();
        return Ok(vec![StoredFieldRun {
            occurrence,
            run,
            force_underline: false,
            force_strike: false,
            revision: None,
            source,
            field_kind,
        }]);
    };
    let accepted = if let Some(paragraph) = &prepared {
        paragraph
            .accepted_run_paths()
            .into_iter()
            .map(|path| paragraph.accepted_run(&path).expect("fresh accepted path"))
            .collect::<Vec<_>>()
    } else {
        field
            .cached_result_runs()
            .expect("borrowed cache selected")
            .iter()
            .collect()
    };
    let children = field.cached_fields_in_source_order();
    let mut next = source
        .map(|source| source.index)
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| LayoutError::Layout("stored field source index overflow".into()))?;
    for instruction_child in field.nested_fields_in_source_order() {
        let mut pending = vec![instruction_child];
        while let Some(child) = pending.pop() {
            next = next
                .checked_add(1)
                .ok_or_else(|| LayoutError::Layout("stored field source index overflow".into()))?;
            pending.extend(child.all_nested_fields_in_source_order());
        }
    }
    let mut slots = HashMap::new();
    let mut ordinal = 0usize;
    for run in accepted {
        for (slot, content) in run.content.iter().enumerate() {
            let RunContent::Field(child) = content else {
                continue;
            };
            let original = children.get(ordinal).ok_or_else(|| {
                LayoutError::Layout("stored accepted field occurrence is extra".into())
            })?;
            if original.source_owner_id() != child.source_owner_id()
                || original.form() != child.form()
            {
                return Err(LayoutError::Layout(
                    "stored accepted field source slot changed".into(),
                ));
            }
            if slots
                .insert(
                    (std::ptr::from_ref(run), slot),
                    source.map(|parent| FieldSource {
                        node: parent.node,
                        index: next,
                    }),
                )
                .is_some()
            {
                return Err(LayoutError::Layout(
                    "stored accepted field source slot is ambiguous".into(),
                ));
            }
            ordinal += 1;
            let mut pending = vec![*original];
            while let Some(child) = pending.pop() {
                next = next.checked_add(1).ok_or_else(|| {
                    LayoutError::Layout("stored field source index overflow".into())
                })?;
                pending.extend(child.all_nested_fields_in_source_order());
            }
        }
    }
    if ordinal != children.len() {
        return Err(LayoutError::Layout(
            "stored accepted field occurrence is missing".into(),
        ));
    }
    let mut result = Vec::new();
    for projected in runs {
        let run = projected.run;
        let mut segment = run.clone();
        segment.content.clear();
        if segment.properties.is_none() {
            segment.properties = inherited.cloned();
        }
        for (slot, content) in run.content.iter().enumerate() {
            if let RunContent::Field(child) = content {
                if !segment.content.is_empty() || !segment.extra_xml.is_empty() {
                    result.push(StoredFieldRun {
                        occurrence,
                        run: segment.clone(),
                        force_underline: projected.force_underline,
                        force_strike: projected.force_strike,
                        revision: projected.revision,
                        source,
                        field_kind,
                    });
                    segment.content.clear();
                    segment.extra_xml.clear();
                    segment.extra_xml_positions.clear();
                }
                let child_source = slots
                    .get(&(std::ptr::from_ref(run), slot))
                    .copied()
                    .flatten();
                for mut child in stored_note_cache_runs(
                    child,
                    run.properties.as_ref().or(inherited),
                    view,
                    child_source,
                    locked,
                    input,
                    true,
                )? {
                    child.force_underline |= projected.force_underline;
                    child.force_strike |= projected.force_strike;
                    child.revision = child.revision.or(projected.revision);
                    result.push(child);
                }
            } else {
                segment.content.push(content.clone());
            }
        }
        if !segment.content.is_empty() || !segment.extra_xml.is_empty() {
            result.push(StoredFieldRun {
                occurrence,
                run: segment,
                force_underline: projected.force_underline,
                force_strike: projected.force_strike,
                revision: projected.revision,
                source,
                field_kind,
            });
        }
    }
    if result.is_empty() && source.is_some() {
        let mut run = CT_R::new("");
        run.properties = inherited.cloned();
        result.push(StoredFieldRun {
            occurrence,
            run,
            force_underline: false,
            force_strike: false,
            revision: None,
            source,
            field_kind,
        });
    }
    Ok(result)
}

fn append_cached_note_contents(
    field: &oxoffice_docx_oxml::text::Field,
    view: RevisionView,
    contents: &mut Vec<RunContent>,
) -> Result<()> {
    for stored in stored_note_cache_runs(field, None, view, None, false, None, false)? {
        contents.extend(stored.run.content.into_iter().filter(|content| {
            matches!(
                content,
                RunContent::FootnoteRef { .. } | RunContent::EndnoteRef { .. }
            )
        }));
    }
    Ok(())
}

fn content_note_references(content: &RunContent, view: RevisionView) -> Result<Vec<RunContent>> {
    let mut contents = Vec::new();
    match content {
        RunContent::FootnoteRef { .. } | RunContent::EndnoteRef { .. } => {
            contents.push(content.clone())
        }
        RunContent::Field(field) => append_cached_note_contents(field, view, &mut contents)?,
        _ => {}
    }
    Ok(contents)
}

fn paragraph_note_references(paragraph: &CT_P, view: RevisionView) -> Result<Vec<NoteRef>> {
    let mut result = Vec::new();
    for projected in project_paragraph_runs(paragraph, view) {
        for content in &projected.run.content {
            for content in content_note_references(content, view)? {
                match content {
                    RunContent::FootnoteRef { id, .. } => result.push(NoteRef {
                        stream: NoteStream::Footnote,
                        id,
                    }),
                    RunContent::EndnoteRef { id, .. } => result.push(NoteRef {
                        stream: NoteStream::Endnote,
                        id,
                    }),
                    _ => {}
                }
            }
        }
    }
    Ok(result)
}

fn body_note_references(input: &LayoutInput) -> Result<Vec<NoteRef>> {
    let mut result = Vec::new();
    for content in &input.document.body.content {
        if let BodyContent::Paragraph(paragraph) = content {
            result.extend(paragraph_note_references(paragraph, input.revision_view)?);
        }
    }
    Ok(result)
}

const fn arc_allocation_bytes<T>() -> usize {
    std::mem::size_of::<T>() + 2 * std::mem::size_of::<usize>()
}

// Source family/placement is independent of formatting support. Only an
// unplaced bookmark target removes TargetPage qualification.
struct PageFieldValue {
    display: String,
    target_unplaced: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct FieldSubstitutionInputs {
    page_index: usize,
    page_number: usize,
    total_pages: usize,
    bookmark_pages: Vec<(usize, usize)>,
    page_format_context: String,
    font_identity: Vec<FontId>,
    revision_view: RevisionView,
    revision_style: RevisionStyle,
}

const CACHE_MAX_ENTRIES: usize = 5_216;
const CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;
const PARAGRAPH_CACHE_MAX_ENTRIES: usize = 4_096;
const PARAGRAPH_CACHE_MAX_BYTES: usize = 50 * 1024 * 1024;
const TABLE_CACHE_MAX_ENTRIES: usize = 32;
const TABLE_CACHE_MAX_BYTES: usize = 2 * 1024 * 1024;
const HEADER_FOOTER_CACHE_MAX_ENTRIES: usize = 64;
const HEADER_FOOTER_CACHE_MAX_BYTES: usize = 4 * 1024 * 1024;
const RESTART_CACHE_MAX_ENTRIES: usize = 1_024;
const CALLER_ALIAS_MAX_ENTRIES: usize = 256;
const CALLER_ALIAS_MAX_RETAINED_BYTES: usize = 64 * 1024;
const _: () = assert!(PARAGRAPH_CACHE_MAX_ENTRIES == 4_096);
const _: () = assert!(PARAGRAPH_CACHE_MAX_BYTES == 50 * 1024 * 1024);
const _: () = assert!(HEADER_FOOTER_CACHE_MAX_ENTRIES == 64);
const _: () = assert!(HEADER_FOOTER_CACHE_MAX_BYTES == 4 * 1024 * 1024);
const _: () = assert!(CACHE_MAX_ENTRIES == 5_216);
const _: () = assert!(CACHE_MAX_BYTES == 64 * 1024 * 1024);
const _: () = assert!(
    PARAGRAPH_CACHE_MAX_ENTRIES
        + TABLE_CACHE_MAX_ENTRIES
        + HEADER_FOOTER_CACHE_MAX_ENTRIES
        + RESTART_CACHE_MAX_ENTRIES
        <= CACHE_MAX_ENTRIES
);
const _: () = assert!(
    PARAGRAPH_CACHE_MAX_BYTES + TABLE_CACHE_MAX_BYTES + HEADER_FOOTER_CACHE_MAX_BYTES
        <= CACHE_MAX_BYTES
);
const CACHE_SOURCE_NODE: SourceNodeId = match SourceNodeId::new(1) {
    Some(node) => node,
    None => panic!("one is a valid source node id"),
};

/// Return the deterministic prefix that fits the private caller-alias bounds.
///
/// The byte accounting matches `oxoffice-layout`: requested and target strings in
/// the ordered identity plus the normalized requested key and target value in
/// the font lookup map.
fn bounded_caller_aliases(aliases: &[(String, String)]) -> Vec<(String, String)> {
    let mut bounded = Vec::with_capacity(aliases.len().min(CALLER_ALIAS_MAX_ENTRIES));
    let mut retained_bytes = 0usize;
    for (requested, target) in aliases {
        if bounded.len() == CALLER_ALIAS_MAX_ENTRIES {
            break;
        }
        let normalized_requested = requested.to_lowercase();
        let entry_bytes = requested
            .len()
            .saturating_add(target.len())
            .saturating_add(normalized_requested.len())
            .saturating_add(target.len());
        let next_retained_bytes = retained_bytes.saturating_add(entry_bytes);
        if next_retained_bytes > CALLER_ALIAS_MAX_RETAINED_BYTES {
            break;
        }
        bounded.push((requested.clone(), target.clone()));
        retained_bytes = next_retained_bytes;
    }
    bounded
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    fn with_font_manager(font_manager: FontManager) -> Self {
        Self {
            font_manager,
            caller_font_aliases: Vec::new(),
            paragraph_cache_context: None,
            paragraph_cache: VecDeque::new(),
            paragraph_cache_bytes: 0,
            pending_paragraph_cache: None,
            pending_paragraph_cache_bytes: 0,
            paragraph_cache_reads_enabled: false,
            paragraph_cache_notes_match: false,
            retained_context_matches_full: false,
            table_cache: VecDeque::new(),
            table_cache_bytes: 0,
            pending_table_cache: None,
            pending_table_cache_bytes: 0,
            header_footer_cache: VecDeque::new(),
            header_footer_cache_bytes: 0,
            pending_header_footer_cache: None,
            pending_header_footer_cache_bytes: 0,
            header_footer_cache_reads_enabled: false,
            restart_cache: None,
            numbering_by_source: HashMap::new(),
            navigation_by_source: HashMap::new(),
            bookmark_numbering: HashMap::new(),
            bookmark_suppressed: std::collections::HashSet::new(),
            last_body_fragments: Vec::new(),
            last_page_sections: Vec::new(),
            last_field_placements: Vec::new(),
            last_bookmark_pages: HashMap::new(),
            last_field_source_xml: HashMap::new(),
            last_text_box_owner_indices: HashMap::new(),
            last_picture_draw_owners: Vec::new(),
        }
    }

    pub fn new() -> Self {
        Self::with_font_manager(FontManager::new())
    }

    /// Create an engine that resolves fonts without system font discovery.
    pub fn new_deterministic() -> Result<Self> {
        Ok(Self::with_font_manager(FontManager::new_deterministic()?))
    }

    /// Create an engine whose font universe is supplied entirely by the
    /// layout input, without bundled or system-font discovery.
    pub(crate) fn new_with_caller_fonts() -> Self {
        Self::with_font_manager(FontManager::new_with_fonts(Vec::new()))
    }

    /// Set byte-free caller aliases from requested family to loaded family.
    pub fn set_caller_font_aliases(&mut self, aliases: &[(String, String)]) {
        let aliases = bounded_caller_aliases(aliases);
        if self.caller_font_aliases != aliases {
            self.caller_font_aliases = aliases;
        }
    }

    /// Take a reusable engine only when its complete retained-work context
    /// matches the proposed receiver input.
    #[doc(hidden)]
    pub fn take_if_compatible(source: &mut Option<Self>, input: &LayoutInput) -> Option<Self> {
        Self::take_if_compatible_with_caller_aliases(source, input, &[])
    }

    /// Take a reusable engine only when its complete caller-font and alias
    /// context matches the proposed receiver input.
    #[doc(hidden)]
    pub fn take_if_compatible_with_caller_aliases(
        source: &mut Option<Self>,
        input: &LayoutInput,
        caller_font_aliases: &[(String, String)],
    ) -> Option<Self> {
        let caller_font_aliases = bounded_caller_aliases(caller_font_aliases);
        let has_wrapping_drawing = document_has_wrapping_drawing(input);
        let compatible = source.as_ref().is_some_and(|engine| {
            engine.caller_font_aliases == caller_font_aliases
                && engine
                    .paragraph_cache_context
                    .as_ref()
                    .is_some_and(|context| {
                        context.matches_input(input, &caller_font_aliases, has_wrapping_drawing)
                    })
                && engine.pending_paragraph_cache.is_none()
                && engine.pending_header_footer_cache.is_none()
        });
        compatible.then(|| source.take()).flatten()
    }

    /// Lay out the entire document.
    pub fn layout(&mut self, input: &LayoutInput) -> Result<LayoutResult> {
        self.layout_inner(prepared_semantic_layout_input(input)?, None)
    }

    pub(crate) fn measure_content(
        &mut self,
        input: &LayoutInput,
        content: &BodyContent,
        available_width: f64,
        related_story_scope: Option<&str>,
    ) -> Result<(f64, Vec<Diagnostic>)> {
        self.font_manager
            .load_embedded_fonts(&input.embedded_fonts)?;
        self.font_manager.load_additional_fonts(&input.fonts);
        self.font_manager.begin_layout();
        let media = MediaRegistry::new(&input.images);
        let scoped_media = related_story_scope.map(|scope| media.scoped_to_part(scope));
        let media = scoped_media.as_ref().unwrap_or(&media);
        let mut numbering = NumberingState::new();
        let mut diagnostics = Vec::new();
        // Caller-width measurement loads what whole-document layout loads, so
        // it reads the body section's grid rather than measuring ungridded. A
        // related story measures ungridded, because that is what header,
        // footer and note layout does with the same content.
        let doc_grid = if related_story_scope.is_none() {
            input
                .document
                .body
                .sect_pr
                .as_ref()
                .and_then(|sect_pr| sect_pr.doc_grid.as_deref())
        } else {
            None
        };
        let height = match content {
            BodyContent::Paragraph(paragraph) => layout_paragraph_with_grid(
                paragraph,
                available_width,
                &input.styles,
                input,
                media,
                &mut self.font_manager,
                &mut numbering,
                &mut diagnostics,
                doc_grid,
            )?
            .total_height(),
            BodyContent::Table(table) => crate::table::layout_table(
                table,
                available_width,
                &input.styles,
                input,
                media,
                &mut self.font_manager,
                &mut numbering,
                &mut diagnostics,
                doc_grid,
            )?
            .total_height(),
            BodyContent::ContentControl(_) | BodyContent::RawXml(_) => {
                return Err(LayoutError::Layout(
                    "only paragraphs and tables can be measured".to_owned(),
                ));
            }
        };
        diagnostics.extend(self.font_manager.current_font_diagnostics().iter().cloned());
        diagnostics.extend(self.font_manager.word_substitution_diagnostics());
        Ok((height, diagnostics))
    }

    /// Lay out the document and retain its result-local Word source table.
    pub(crate) fn layout_with_provenance(
        &mut self,
        input: &LayoutInput,
    ) -> Result<(LayoutResult, Vec<WordSourcePath>)> {
        let input = prepared_semantic_layout_input(input)?;
        let sources = SourceRegistry::for_input(input.as_ref())?;
        let result = self.layout_inner(input, Some(&sources))?;
        self.last_field_source_xml = sources.field_source_xml;
        self.last_text_box_owner_indices = sources.text_box_owner_indices;
        Ok((result, sources.nodes))
    }

    pub(crate) fn take_field_snapshot(
        &mut self,
    ) -> (
        Vec<crate::WordPageSection>,
        Vec<crate::WordFieldPlacement>,
        HashMap<usize, crate::WordPageSection>,
    ) {
        (
            std::mem::take(&mut self.last_page_sections),
            std::mem::take(&mut self.last_field_placements),
            std::mem::take(&mut self.last_bookmark_pages),
        )
    }

    pub(crate) fn take_picture_draw_owners(&mut self) -> Vec<(String, Option<String>)> {
        std::mem::take(&mut self.last_picture_draw_owners)
    }

    pub(crate) fn take_text_box_owner_indices(&mut self) -> HashMap<WordStory, usize> {
        std::mem::take(&mut self.last_text_box_owner_indices)
    }

    pub(crate) fn take_field_source_xml(&mut self) -> HashMap<FieldSource, Vec<u8>> {
        std::mem::take(&mut self.last_field_source_xml)
    }

    pub(crate) fn take_body_fragments(&mut self) -> Vec<Vec<WordBodyLayoutFragment>> {
        std::mem::take(&mut self.last_body_fragments)
    }

    pub(crate) fn take_bookmark_suppressed(&mut self) -> std::collections::HashSet<String> {
        std::mem::take(&mut self.bookmark_suppressed)
    }

    pub(crate) fn take_bookmark_numbering(&mut self) -> HashMap<String, ResolvedNumbering> {
        std::mem::take(&mut self.bookmark_numbering)
    }

    pub(crate) fn take_navigation(&mut self) -> HashMap<SourceNodeId, crate::ParagraphNavigation> {
        std::mem::take(&mut self.navigation_by_source)
    }

    pub(crate) fn numbering_by_source(
        &self,
        source_count: usize,
    ) -> Vec<Option<ResolvedNumbering>> {
        (0..source_count)
            .map(|index| {
                let id = SourceNodeId::new(u32::try_from(index + 1).expect("source index fits"))
                    .expect("source ids are one based");
                self.numbering_by_source.get(&id).cloned()
            })
            .collect()
    }

    fn layout_inner(
        &mut self,
        mut prepared_input: Cow<'_, LayoutInput>,
        sources: Option<&SourceRegistry>,
    ) -> Result<LayoutResult> {
        let input = prepared_input.as_ref();
        self.navigation_by_source.clear();
        self.numbering_by_source.clear();
        self.bookmark_numbering.clear();
        self.bookmark_suppressed.clear();
        self.last_body_fragments.clear();
        let needs_ref_projection = document_has_ref_projection(input);
        let needs_sequences = document_has_sequence_fields(input);
        let generated_sources = (sources.is_none()
            && (needs_ref_projection
                || needs_sequences
                || document_has_section_fields(input)
                || input
                    .sequence_snapshot
                    .as_ref()
                    .is_some_and(|s| s.navigation_document.is_some())))
        .then(|| SourceRegistry::for_input(input))
        .transpose()?;
        let sources = sources.or(generated_sources.as_ref());
        // Caller hints never establish provenance or freshness. Derive once,
        // before any retained shaping, furniture or restart cache can be read.
        if needs_sequences || needs_ref_projection || input.sequence_snapshot.is_some() {
            let snapshot = if (needs_sequences
                || needs_ref_projection
                || input
                    .sequence_snapshot
                    .as_ref()
                    .is_some_and(|s| s.navigation_document.is_some()))
                && input.revision_view == RevisionView::Accepted
            {
                Some(Arc::new(sequence_snapshot(
                    input,
                    sources.expect("sequence source registry"),
                )?))
            } else {
                None
            };
            // Reuse the semantic-preparation clone when one was required.
            prepared_input.to_mut().sequence_snapshot = snapshot;
        }
        let input = prepared_input.as_ref();
        // Load user-provided / DOCX-embedded fonts (highest priority). An exact
        // unchanged set is a no-op in a reusable engine.
        let font_context_changed = self
            .font_manager
            .load_embedded_fonts(&input.embedded_fonts)?
            | self.font_manager.load_additional_fonts(&input.fonts)
            | self
                .font_manager
                .set_caller_aliases(&self.caller_font_aliases);
        self.font_manager.begin_layout();
        let line_number_context = prepare_word_line_numbers(input, &mut self.font_manager);
        let has_wrapping_drawing = document_has_wrapping_drawing(input);

        if font_context_changed {
            self.paragraph_cache.clear();
            self.paragraph_cache_bytes = 0;
            self.table_cache.clear();
            self.table_cache_bytes = 0;
            self.header_footer_cache.clear();
            self.header_footer_cache_bytes = 0;
        }
        let base_context_matches = !font_context_changed
            && self
                .paragraph_cache_context
                .as_ref()
                .is_some_and(|context| {
                    context.matches_input_after_unchanged_fonts_ignoring_notes(
                        input,
                        &self.caller_font_aliases,
                        has_wrapping_drawing,
                    )
                });
        let notes_match = self
            .paragraph_cache_context
            .as_ref()
            .is_some_and(|context| context.notes_match(input));
        let context_matches = base_context_matches && notes_match;
        let page_note_restart = input_uses_page_note_restart(input);
        self.paragraph_cache_reads_enabled = base_context_matches && !page_note_restart;
        self.paragraph_cache_notes_match = notes_match;
        self.retained_context_matches_full = context_matches && !page_note_restart;
        self.header_footer_cache_reads_enabled = context_matches && !page_note_restart;
        self.pending_paragraph_cache = Some(VecDeque::new());
        self.pending_paragraph_cache_bytes = 0;
        self.pending_table_cache = Some(VecDeque::new());
        self.pending_table_cache_bytes = 0;
        self.pending_header_footer_cache = Some(VecDeque::new());
        self.pending_header_footer_cache_bytes = 0;

        let mut reference_seed = None;
        let mut result = match self.layout_transaction(
            input,
            sources,
            has_wrapping_drawing,
            None,
            None,
            &line_number_context,
        ) {
            Ok((first, references)) if needs_ref_projection => {
                self.navigation_by_source.clear();
                self.numbering_by_source.clear();
                self.bookmark_numbering.clear();
                self.bookmark_suppressed.clear();
                if let Some(pending) = &mut self.pending_paragraph_cache {
                    pending.clear();
                }
                if let Some(pending) = &mut self.pending_table_cache {
                    pending.clear();
                }
                if let Some(pending) = &mut self.pending_header_footer_cache {
                    pending.clear();
                }
                self.pending_paragraph_cache_bytes = 0;
                self.pending_table_cache_bytes = 0;
                self.pending_header_footer_cache_bytes = 0;
                drop(first);
                let projected = self
                    .layout_transaction(
                        input,
                        sources,
                        has_wrapping_drawing,
                        Some(&references),
                        None,
                        &line_number_context,
                    )
                    .map(|(result, _)| result);
                reference_seed = Some(references);
                projected
            }
            Ok((result, _)) => Ok(result),
            Err(error) => Err(error),
        };
        if page_note_restart {
            result = result.and_then(|mut current| {
                for _ in 0..4 {
                    let pages = note_reference_pages(&current);
                    self.navigation_by_source.clear();
                    self.numbering_by_source.clear();
                    self.bookmark_numbering.clear();
                    self.bookmark_suppressed.clear();
                    self.pending_paragraph_cache.as_mut().map(VecDeque::clear);
                    self.pending_table_cache.as_mut().map(VecDeque::clear);
                    self.pending_header_footer_cache
                        .as_mut()
                        .map(VecDeque::clear);
                    self.pending_paragraph_cache_bytes = 0;
                    self.pending_table_cache_bytes = 0;
                    self.pending_header_footer_cache_bytes = 0;
                    let (next, _) = self.layout_transaction(
                        input,
                        sources,
                        has_wrapping_drawing,
                        reference_seed.as_ref(),
                        Some(&pages),
                        &line_number_context,
                    )?;
                    let stable = note_reference_pages(&next) == pages;
                    current = next;
                    if stable {
                        return Ok(current);
                    }
                }
                Err(LayoutError::Layout(
                    "page-restarted note labels did not converge".to_owned(),
                ))
            });
        }
        let pending = self.pending_paragraph_cache.take().unwrap_or_default();
        let pending_tables = self.pending_table_cache.take().unwrap_or_default();
        let pending_header_footers = self.pending_header_footer_cache.take().unwrap_or_default();
        self.pending_paragraph_cache_bytes = 0;
        self.pending_table_cache_bytes = 0;
        self.pending_header_footer_cache_bytes = 0;
        self.paragraph_cache_reads_enabled = false;
        self.paragraph_cache_notes_match = false;
        self.retained_context_matches_full = false;
        self.header_footer_cache_reads_enabled = false;
        if result.is_ok() {
            if !base_context_matches {
                self.paragraph_cache.clear();
                self.paragraph_cache_bytes = 0;
            } else if !notes_match {
                self.paragraph_cache.retain(|entry| {
                    paragraph_note_references(&entry.key.paragraph, entry.key.revision_view)
                        .is_ok_and(|references| references.is_empty())
                });
                self.paragraph_cache_bytes =
                    self.paragraph_cache.iter().map(|entry| entry.bytes).sum();
            }
            if !context_matches {
                self.table_cache.clear();
                self.table_cache_bytes = 0;
                self.header_footer_cache.clear();
                self.header_footer_cache_bytes = 0;
                self.paragraph_cache_context = Some(ReusableEngineContext::for_input_with_wrap(
                    input,
                    &self.caller_font_aliases,
                    has_wrapping_drawing,
                ));
            }
            for entry in pending {
                self.publish_paragraph_cache_entry(entry);
            }
            for entry in pending_tables {
                self.publish_table_cache_entry(entry);
            }
            for entry in pending_header_footers {
                self.publish_header_footer_cache_entry(entry);
            }
        }
        let current_fonts = self
            .font_manager
            .current_layout_fonts()
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        self.paragraph_cache.retain(|entry| {
            entry.font_trace.iter().all(|event| {
                current_fonts.contains(match event {
                    FontTraceEvent::Resolution(id) => id,
                    FontTraceEvent::Contribution(contribution) => &contribution.font_id,
                })
            })
        });
        self.paragraph_cache_bytes = self.paragraph_cache.iter().map(|entry| entry.bytes).sum();
        self.table_cache.retain(|entry| {
            entry.font_trace.iter().all(|event| {
                current_fonts.contains(match event {
                    FontTraceEvent::Resolution(id) => id,
                    FontTraceEvent::Contribution(contribution) => &contribution.font_id,
                })
            })
        });
        self.table_cache_bytes = self.table_cache.iter().map(|entry| entry.bytes).sum();
        self.header_footer_cache.retain(|entry| {
            entry.font_trace.iter().all(|event| {
                current_fonts.contains(match event {
                    FontTraceEvent::Resolution(id) => id,
                    FontTraceEvent::Contribution(contribution) => &contribution.font_id,
                })
            })
        });
        self.header_footer_cache_bytes = self
            .header_footer_cache
            .iter()
            .map(|entry| entry.bytes)
            .sum();
        self.font_manager.retain_current_fonts();
        result
    }

    fn layout_transaction(
        &mut self,
        input: &LayoutInput,
        sources: Option<&SourceRegistry>,
        document_wraps: bool,
        reference_seed: Option<&NumberingState>,
        note_pages: Option<&HashMap<NoteRef, usize>>,
        line_number_context: &(Option<paginator::WordLineNumberStyle>, Vec<Diagnostic>),
    ) -> Result<(LayoutResult, NumberingState)> {
        let retained_context_matches = self.retained_context_matches_full;
        let styles = &input.styles;
        let mut num_state = reference_seed
            .map(NumberingState::references_only)
            .unwrap_or_default();
        num_state.set_note_labels(note_labels(input, note_pages)?);
        if let Some(sources) = sources {
            for (index, path) in sources.nodes.iter().enumerate() {
                if path.story == WordStory::Document {
                    let source = SourceNodeId::new(
                        u32::try_from(index + 1).expect("source index fits in u32"),
                    )
                    .expect("source ids are one based");
                    num_state.record_main_story_source(source);
                }
            }
        }
        let media = MediaRegistry::new(&input.images);
        let mut diagnostics = line_number_context.1.clone();
        if let Some(failure) = input
            .sequence_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.navigation_failure.as_deref())
        {
            diagnostics.push(Diagnostic {
                message: format!("numbering-aware navigation unavailable: {failure}"),
            });
        }

        // Re-breaking a paragraph around a floating drawing needs its line
        // breaking inputs kept alive past layout. Nearly no document has a
        // drawing that wraps, so the state is dropped again unless one does.
        // Get final section properties (body-level sectPr)
        let final_sect_pr = input
            .document
            .body
            .sect_pr
            .as_ref()
            .cloned()
            .unwrap_or_else(CT_SectPr::default_letter);

        // Build sections: each section has blocks + geometry + header/footer
        let mut sections: Vec<paginator::SharedSection> = Vec::new();
        let mut current_blocks: Vec<SharedLayoutBlock> = Vec::new();
        let mut body_heading_sources = Vec::new();
        let items = body_layout_items(&input.document.body);
        let item_sections = main_story_item_sections(&items, &final_sect_pr);

        let mut carried: Option<CT_P> = None;
        for (content, sect_pr_for_layout) in items.into_iter().zip(item_sections) {
            match content {
                // Accepting a deleted or moved-away paragraph mark joins the
                // paragraph to the next one, so one with no accepted content
                // leaves no block, spacing or list number behind.
                MainStoryLayoutItem::Paragraph(para, path)
                    if input.revision_view == RevisionView::Accepted
                        && path.len() == 1
                        && input.document.body.accepted_paragraph_joins_next(path[0])
                        && para.equations.is_empty()
                        && para.bookmark_markers.is_empty() =>
                {
                    if !accepted_paragraph_is_blank(para) {
                        carried = Some(match carried.take() {
                            Some(prefix) => join_accepted_layout_paragraphs(prefix, para)?,
                            None => para.accepted_view().into_owned(),
                        });
                    }
                }
                MainStoryLayoutItem::Paragraph(para, path) => {
                    let joined = carried
                        .take()
                        .map(|prefix| join_accepted_layout_paragraphs(prefix, para))
                        .transpose()?;
                    let para = joined.as_ref().unwrap_or(para);
                    // Check if this paragraph ends a section (has sect_pr)
                    let para_sect_pr = para.properties.as_ref().and_then(|p| p.sect_pr.as_ref());
                    let geometry = sect_pr_to_geometry(sect_pr_for_layout);

                    let source = sources.and_then(|sources| {
                        if path.len() == 1 {
                            sources.body_id(path[0])
                        } else {
                            sources.id(&WordStory::Document, &path)
                        }
                    });
                    let mut block = self.layout_body_paragraph(
                        para,
                        geometry.body_measure(),
                        styles,
                        input,
                        &media,
                        &mut num_state,
                        &mut diagnostics,
                        source,
                        sect_pr_for_layout.doc_grid.as_deref(),
                    )?;

                    if let (Some(registry), Some(source)) = (sources, source)
                        && let SharedLayoutBlock::Owned {
                            block,
                            semantics: Some(semantics),
                            ..
                        } = &mut block
                        && let LayoutBlock::Paragraph(paragraph) = block.as_mut()
                    {
                        registry.bind_text_boxes(
                            paragraph,
                            Arc::make_mut(&mut semantics.paint),
                            Some(source),
                        )?;
                    }

                    if sect_pr_for_layout.text_direction.as_deref() == Some("tbRl") {
                        let direct = para.properties.as_ref();
                        let mut effective = style_resolver::resolve_paragraph_properties(
                            direct.and_then(|p| p.style_id.as_deref()),
                            styles,
                        );
                        let num_id = direct.and_then(|p| p.num_id).or(effective.num_id);
                        let level = direct
                            .and_then(|p| p.num_ilvl)
                            .or(effective.num_ilvl)
                            .unwrap_or(0);
                        if let (Some(id), Some(numbering)) = (num_id, input.numbering.as_ref())
                            && let Some(properties) =
                                style_resolver::level_paragraph_properties(id, level, numbering)
                        {
                            merge_direct_ppr(&mut effective, properties);
                        }
                        if let Some(direct) = direct {
                            merge_direct_ppr(&mut effective, direct);
                        }
                        let policy = (effective.line_rule.as_deref() == Some("exact"))
                            .then_some(effective.line_spacing)
                            .flatten()
                            .filter(|height| height.0 > 0)
                            .and_then(|height| match effective.text_alignment.as_deref() {
                                None | Some("auto" | "center") => Some((height.to_pt(), true)),
                                Some("baseline") => Some((height.to_pt(), false)),
                                _ => None,
                            });
                        if let Some(policy) = policy {
                            block = match block {
                                SharedLayoutBlock::Paragraph {
                                    block,
                                    mut semantics,
                                    body_index,
                                } => {
                                    semantics.tb_rl_exact_baseline = Some(policy);
                                    SharedLayoutBlock::Paragraph {
                                        block,
                                        semantics,
                                        body_index,
                                    }
                                }
                                SharedLayoutBlock::Owned {
                                    block,
                                    border_group_before,
                                    reflow_direction,
                                    body_index,
                                    mut semantics,
                                } => {
                                    // Preserve list and anchor ownership. Owned paragraphs
                                    // already carry the same baseline policy into pagination.
                                    semantics
                                        .as_mut()
                                        .expect("owned Word paragraph retains its paint")
                                        .tb_rl_exact_baseline = Some(policy);
                                    SharedLayoutBlock::Owned {
                                        block,
                                        border_group_before,
                                        reflow_direction,
                                        body_index,
                                        semantics,
                                    }
                                }
                                SharedLayoutBlock::Table { .. } => unreachable!(),
                            };
                        }
                    }

                    if let Some(source) = source {
                        let paragraph = match &block {
                            SharedLayoutBlock::Owned { block, .. } => match block.as_ref() {
                                LayoutBlock::Paragraph(paragraph) => paragraph,
                                LayoutBlock::Table(_) => unreachable!(),
                            },
                            SharedLayoutBlock::Paragraph { block, .. } => block,
                            SharedLayoutBlock::Table { .. } => unreachable!(),
                        };
                        let mut facts = num_state.remove_navigation(source).unwrap_or_else(|| {
                            crate::ParagraphNavigation {
                                omitted_by_table: false,
                                level: None,
                                suppressed_numbering: false,
                                accepted_runs: 0,
                                text: String::new(),
                                style: None,
                            }
                        });
                        facts.level = paragraph.heading_level;
                        // Cache hits return before layout records this count.
                        facts.accepted_runs =
                            project_paragraph_runs(para, input.revision_view).len();
                        if !excluded_owner_for_source(input, source) {
                            facts.text = projected_paragraph_text(para, input.revision_view)?;
                            facts.style = para.properties.as_ref().and_then(|p| p.style_id.clone());
                        }
                        if facts.level.is_some() {
                            body_heading_sources.push(source);
                        }
                        num_state.record_navigation(source, facts);
                    }
                    if sources.is_some() {
                        block.set_body_index(path[0]);
                    }

                    current_blocks.push(block);

                    // If this paragraph has sect_pr, it ends a section
                    if let Some(sect_pr) = para_sect_pr {
                        let geometry = section_page_geometry(
                            sect_pr,
                            input,
                            line_number_context.0,
                            &mut diagnostics,
                        );
                        let header_footer = layout_header_footer(
                            self,
                            sect_pr,
                            input,
                            styles,
                            &media,
                            &mut diagnostics,
                            sources,
                            &mut num_state,
                        )?;
                        let title_pg = sect_pr.title_pg.unwrap_or(false);
                        let (header_footer, header_footer_semantics) = header_footer
                            .map_or((None, None), |(content, semantics)| {
                                (Some(content), Some(semantics))
                            });
                        block::collapse_shared_border_groups(&mut current_blocks);
                        sections.push(paginator::SharedSection {
                            line_number_style: line_number_context.0,
                            written_tb_rl_band: sect_pr.text_direction.as_deref() == Some("tbRl"),
                            continuous: sect_pr.section_type
                                == Some(oxoffice_docx_oxml::shared::ST_SectionType::Continuous),
                            blocks: std::mem::take(&mut current_blocks),
                            modern_footnote_layout: input.modern_footnote_layout,
                            footnote_layout_like_word8: input.footnote_layout_like_word8,
                            geometry,
                            header_footer,
                            header_footer_semantics,
                            title_pg,
                            page_number_start: section_page_number_start(sect_pr),
                            endnotes_at_section_end: sect_pr
                                .endnote_pr
                                .as_deref()
                                .and_then(|policy| policy.pos.as_deref())
                                .or_else(|| {
                                    input.note_defaults[1]
                                        .as_ref()
                                        .and_then(|policy| policy.pos.as_deref())
                                })
                                == Some("sectEnd"),
                            footnotes_beneath_text: sect_pr
                                .footnote_pr
                                .as_deref()
                                .and_then(|policy| policy.pos.as_deref())
                                .or_else(|| {
                                    input.note_defaults[0]
                                        .as_ref()
                                        .and_then(|policy| policy.pos.as_deref())
                                })
                                == Some("beneathText"),
                        });
                        if let Some(numbering) = input.numbering.as_ref() {
                            num_state.restart_after_section_break(numbering);
                        }
                    }
                }
                // Accepting every tracked change removes a table whose rows
                // are all deleted, so it leaves no block or spacing behind.
                MainStoryLayoutItem::Table(tbl, path)
                    if input.revision_view == RevisionView::Accepted
                        && tbl.accepted_view_removes() =>
                {
                    if let Some(registry) = sources.as_ref() {
                        registry.record_table_omission(&WordStory::Document, &path, &mut num_state);
                    }
                }
                MainStoryLayoutItem::Table(tbl, path) => {
                    let geometry = sect_pr_to_geometry(sect_pr_for_layout);

                    let mut table_block = self.layout_body_table(
                        tbl,
                        geometry.body_measure(),
                        styles,
                        input,
                        &media,
                        &mut num_state,
                        &mut diagnostics,
                        sources,
                        &WordStory::Document,
                        &path,
                        sect_pr_for_layout.doc_grid.as_deref(),
                    )?;
                    if let (
                        Some(registry),
                        SharedLayoutBlock::Table {
                            block, semantics, ..
                        },
                    ) = (sources, &table_block)
                    {
                        record_table_navigation(block, semantics, registry, &mut num_state);
                    }
                    if sources.is_some() {
                        table_block.set_body_index(path[0]);
                    }
                    current_blocks.push(table_block);
                }
            }
        }

        // Remaining blocks belong to the final section
        let final_geometry = section_page_geometry(
            &final_sect_pr,
            input,
            line_number_context.0,
            &mut diagnostics,
        );
        let final_hf = layout_header_footer(
            self,
            &final_sect_pr,
            input,
            styles,
            &media,
            &mut diagnostics,
            sources,
            &mut num_state,
        )?;
        let final_title_pg = final_sect_pr.title_pg.unwrap_or(false);
        let (final_hf, final_hf_semantics) = final_hf
            .map_or((None, None), |(content, semantics)| {
                (Some(content), Some(semantics))
            });
        block::collapse_shared_border_groups(&mut current_blocks);
        sections.push(paginator::SharedSection {
            line_number_style: line_number_context.0,
            written_tb_rl_band: final_sect_pr.text_direction.as_deref() == Some("tbRl"),
            continuous: final_sect_pr.section_type
                == Some(oxoffice_docx_oxml::shared::ST_SectionType::Continuous),
            blocks: current_blocks,
            modern_footnote_layout: input.modern_footnote_layout,
            footnote_layout_like_word8: input.footnote_layout_like_word8,
            geometry: final_geometry.clone(),
            header_footer: final_hf,
            header_footer_semantics: final_hf_semantics,
            title_pg: final_title_pg,
            page_number_start: section_page_number_start(&final_sect_pr),
            endnotes_at_section_end: final_sect_pr
                .endnote_pr
                .as_deref()
                .and_then(|policy| policy.pos.as_deref())
                .or_else(|| {
                    input.note_defaults[1]
                        .as_ref()
                        .and_then(|policy| policy.pos.as_deref())
                })
                == Some("sectEnd"),
            footnotes_beneath_text: final_sect_pr
                .footnote_pr
                .as_deref()
                .and_then(|policy| policy.pos.as_deref())
                .or_else(|| {
                    input.note_defaults[0]
                        .as_ref()
                        .and_then(|policy| policy.pos.as_deref())
                })
                == Some("beneathText"),
        });
        let structure = assign_shared_document_structure(&mut sections);

        // Lay the notes out once per width, before pagination, so the paginator
        // can reserve exactly the height it will later draw. A note is broken to
        // the measure of the section carrying its reference, so every section's
        // width is registered. The endnote pages that follow the last body page
        // are drawn against `final_geometry`, which belongs to the section
        // pushed just above and is therefore already in this list.
        let content_widths: Vec<f64> = sections
            .iter()
            .map(|section| section.geometry.content_width())
            .collect();
        let notes = NoteRegistry::build(
            input,
            styles,
            &media,
            &mut self.font_manager,
            &mut num_state,
            &content_widths,
            &mut diagnostics,
            sources,
        )?;

        let mut font_trace = self.font_manager.current_layout_fonts().to_vec();
        let producer_paints = if sections.len() == 1 {
            sections[0]
                .blocks
                .iter()
                .map(|block| match block {
                    SharedLayoutBlock::Paragraph { semantics, .. } => {
                        Some(RestartPaint::Paragraph(Arc::clone(&semantics.paint)))
                    }
                    SharedLayoutBlock::Owned {
                        block,
                        semantics: Some(semantics),
                        ..
                    } if matches!(block.as_ref(), LayoutBlock::Paragraph(_)) => {
                        Some(RestartPaint::Paragraph(Arc::clone(&semantics.paint)))
                    }
                    SharedLayoutBlock::Table { semantics, .. } => {
                        Some(RestartPaint::Table(semantics.clone()))
                    }
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
        } else {
            None
        };
        let restart_record_eligible = sections.len() == 1
            && producer_paints.is_some()
            && sections[0].blocks.len() == input.document.body.content.len()
            && input.document.background_xml.is_none()
            && !document_wraps
            && input
                .document
                .body
                .content
                .iter()
                .zip(&sections[0].blocks)
                .all(|(content, block)| match content {
                    BodyContent::Paragraph(paragraph) if block.paragraph().is_some() => {
                        paragraph_is_restart_record_source_safe(paragraph, styles)
                            && restart_record_block_is_safe(block)
                    }
                    BodyContent::Table(table) if block.table().is_some() => {
                        table_is_cache_safe(table, styles)
                    }
                    _ => false,
                });
        let restart_eligible = restart_record_eligible
            && input
                .document
                .body
                .content
                .iter()
                .zip(&sections[0].blocks)
                .all(|(content, block)| {
                    !matches!(content, BodyContent::Paragraph(paragraph) if paragraph_has_field(paragraph))
                        && restart_block_is_safe(block)
                });
        let current_body_notes = body_note_references(input)?;
        let reusable_restart_record = restart_record_eligible
            && retained_context_matches
            && self.restart_cache.as_ref().is_some_and(|cache| {
                cache.paint.len() == cache.body.len()
                    && cache.font_trace == font_trace
                    && cache.with_provenance == sources.is_some()
                    && cache
                        .body
                        .iter()
                        .flat_map(|entry| entry.note_references().iter().copied())
                        .eq(current_body_notes.iter().copied())
            });
        let reusable_restart = restart_eligible && reusable_restart_record;
        let mut restart_identity_memo =
            RestartBodyIdentityMemo::new(input.document.body.content.len());
        let body_unchanged = reusable_restart_record
            && self.restart_cache.as_ref().is_some_and(|cache| {
                cache.paint.as_slice() == producer_paints.as_deref().unwrap_or(&[])
                    && cache.body.len() == input.document.body.content.len()
                    && input
                        .document
                        .body
                        .content
                        .iter()
                        .zip(&cache.body)
                        .enumerate()
                        .all(|(index, (content, retained))| {
                            retained.matches_memoized(content, index, &mut restart_identity_memo)
                        })
            });
        let first_changed = reusable_restart.then(|| {
            let cache = self.restart_cache.as_ref().expect("restart cache exists");
            input
                .document
                .body
                .content
                .iter()
                .zip(&cache.body)
                .enumerate()
                .position(|(index, (current, previous))| {
                    cache.paint.get(index)
                        != producer_paints.as_ref().and_then(|paint| paint.get(index))
                        || !previous.matches_memoized(current, index, &mut restart_identity_memo)
                })
                .unwrap_or_else(|| input.document.body.content.len().min(cache.body.len()))
        });
        let common_suffix = reusable_restart.then(|| {
            let cache = self.restart_cache.as_ref().expect("restart cache exists");
            input
                .document
                .body
                .content
                .iter()
                .enumerate()
                .rev()
                .zip(cache.body.iter().enumerate().rev())
                .take_while(|((index, current), (old_index, previous))| {
                    cache.paint.get(*old_index)
                        == producer_paints.as_ref().and_then(|paint| paint.get(*index))
                        && previous.matches_memoized(current, *index, &mut restart_identity_memo)
                })
                .count()
        });
        // The restart begins strictly before the first changed block. A page
        // boundary right before it may have been chosen by that block, which
        // moved there whole or broke the page before itself, and an edit can
        // undo that choice. The document start is always a safe restart.
        let restart_checkpoint = first_changed.and_then(|first_changed| {
            self.restart_cache
                .as_ref()
                .expect("restart cache exists")
                .checkpoints
                .iter()
                .rev()
                .find(|checkpoint| {
                    checkpoint.next_block_index < first_changed || checkpoint.page_count == 0
                })
                .copied()
        });
        let tail_reusable = sources.is_none()
            || self
                .restart_cache
                .as_ref()
                .is_some_and(|cache| cache.body.len() == input.document.body.content.len());
        let tail_source = restart_checkpoint
            .filter(|_| tail_reusable)
            .and_then(|restart| {
                let cache = self.restart_cache.as_ref().expect("restart cache exists");
                let common_suffix = common_suffix.expect("reusable restart has an exact suffix");
                let new_tail = input.document.body.content.len() - common_suffix;
                let old_tail = cache.body.len() - common_suffix;
                let block_delta = new_tail as isize - old_tail as isize;
                cache
                    .checkpoints
                    .iter()
                    .find(|checkpoint| {
                        checkpoint.next_block_index >= old_tail
                            && checkpoint
                                .next_block_index
                                .checked_add_signed(block_delta)
                                .is_some_and(|next| next > restart.next_block_index)
                    })
                    .copied()
                    .map(|old| {
                        (
                            paginator::PaginationCheckpoint {
                                next_block_index: old
                                    .next_block_index
                                    .checked_add_signed(block_delta)
                                    .expect("common suffix block index remains in range"),
                                page_count: old.page_count,
                                next_header_page_number: old.next_header_page_number,
                            },
                            old,
                        )
                    })
            });

        let mut page_sections = Vec::new();
        let (mut pages, mut outlines, mut checkpoints, mut body_fragments, mut page_policies) =
            if restart_eligible {
                let mut recorded = paginator::paginate_shared_single_section_recorded(
                    &sections[0],
                    &self.font_manager,
                    &media,
                    &notes,
                    restart_checkpoint,
                    tail_source.map(|(stop, _)| stop),
                );
                if let Some(failure) = recorded.furniture_failure.take() {
                    return Err(LayoutError::Layout(format!(
                        "section {} page {}: {}",
                        failure.section + 1,
                        failure.page,
                        failure.reason
                    )));
                }
                if recorded.stopped_at.is_none() {
                    // Completing a changed region must include references retained
                    // in the prefix, while continuing in the actual final flow band.
                    let references = body_note_references(input)?;
                    paginator::append_endnote_pages_for_references(
                        &mut recorded.pages,
                        &mut recorded.paint,
                        &references,
                        &notes,
                        final_geometry.clone(),
                        restart_checkpoint.map_or(0, |checkpoint| checkpoint.page_count),
                        restart_checkpoint
                            .map_or(1, |checkpoint| checkpoint.next_header_page_number),
                        recorded.last_flow_space,
                        Some(&paginator::EndnoteFurniture {
                            section: &sections[0],
                            section_index: 0,
                            fm: &self.font_manager,
                            media: media.media(),
                        }),
                    )
                    .map_err(|failure| {
                        LayoutError::Layout(format!(
                            "section {} page {}: {}",
                            failure.section + 1,
                            failure.page,
                            failure.reason
                        ))
                    })?;
                }
                recorded.paint.validate(&recorded.pages)?;
                recorded.paint.boundaries.clear();
                for (page, policies) in recorded.pages.iter_mut().zip(&mut recorded.paint.policies)
                {
                    mark_remaining_artifacts(&mut page.elements, policies)?;
                }
                let mut body_fragments = vec![Vec::new(); input.document.body.content.len()];
                if let Some(checkpoint) = restart_checkpoint
                    && let Some(cache) = self.restart_cache.as_ref()
                {
                    for (body_index, fragments) in cache
                        .body_fragments
                        .iter()
                        .enumerate()
                        .take(checkpoint.next_block_index)
                    {
                        body_fragments[body_index].clone_from(fragments);
                    }
                }
                for (body_index, fragment) in recorded.body_fragments.drain(..) {
                    if let Some(fragments) = body_fragments.get_mut(body_index) {
                        fragments.push(fragment);
                    }
                }
                let mut pages = restart_checkpoint.map_or_else(Vec::new, |checkpoint| {
                    self.restart_cache
                        .as_ref()
                        .expect("a restart checkpoint belongs to retained pages")
                        .raw_pages[..checkpoint.page_count]
                        .iter()
                        .map(Arc::clone)
                        .collect()
                });
                let mut policies = restart_checkpoint.map_or_else(Vec::new, |checkpoint| {
                    self.restart_cache
                        .as_ref()
                        .expect("retained prefix owns policies")
                        .raw_policies[..checkpoint.page_count]
                        .to_vec()
                });
                policies.append(&mut recorded.paint.policies);
                pages.extend(recorded.pages.into_iter().map(Arc::new));
                let mut outlines = restart_checkpoint.map_or_else(Vec::new, |checkpoint| {
                    self.restart_cache
                        .as_ref()
                        .expect("a restart checkpoint belongs to retained outlines")
                        .outlines
                        .iter()
                        .filter(|outline| outline.page_index < checkpoint.page_count)
                        .cloned()
                        .collect()
                });
                outlines.extend(recorded.outlines);
                let mut checkpoints = restart_checkpoint.map_or_else(Vec::new, |checkpoint| {
                    self.restart_cache
                        .as_ref()
                        .expect("a restart checkpoint belongs to retained state")
                        .checkpoints
                        .iter()
                        .copied()
                        .filter(|candidate| candidate.page_count <= checkpoint.page_count)
                        .collect()
                });
                checkpoints.extend(recorded.checkpoints);
                if let (Some(stopped), Some((_, old_tail))) = (recorded.stopped_at, tail_source) {
                    debug_assert_eq!(stopped.page_count, old_tail.page_count);
                    let cache = self.restart_cache.as_ref().expect("restart cache exists");
                    policies.extend_from_slice(&cache.raw_policies[old_tail.page_count..]);
                    pages.extend(
                        cache.raw_pages[old_tail.page_count..]
                            .iter()
                            .map(Arc::clone),
                    );
                    outlines.extend(
                        cache
                            .outlines
                            .iter()
                            .filter(|outline| outline.page_index >= old_tail.page_count)
                            .cloned(),
                    );
                    let block_delta =
                        stopped.next_block_index as isize - old_tail.next_block_index as isize;
                    checkpoints.extend(
                        cache
                            .checkpoints
                            .iter()
                            .filter(|candidate| candidate.page_count > old_tail.page_count)
                            .map(|candidate| paginator::PaginationCheckpoint {
                                next_block_index: candidate
                                    .next_block_index
                                    .checked_add_signed(block_delta)
                                    .expect("common suffix block index remains in range"),
                                page_count: candidate.page_count,
                                next_header_page_number: candidate.next_header_page_number,
                            }),
                    );
                    for (body_index, fragments) in cache
                        .body_fragments
                        .iter()
                        .enumerate()
                        .skip(old_tail.next_block_index)
                    {
                        if let Some(destination) = body_fragments.get_mut(body_index) {
                            destination.clone_from(fragments);
                        }
                    }
                }
                checkpoints.sort_unstable_by_key(|checkpoint| checkpoint.next_block_index);
                checkpoints.dedup();
                (pages, outlines, checkpoints, body_fragments, policies)
            } else {
                let mut pagination = paginator::paginate_shared_sections(
                    &sections,
                    &self.font_manager,
                    &media,
                    &notes,
                );
                if let Some(failure) = pagination.furniture_failure.take() {
                    return Err(LayoutError::Layout(format!(
                        "section {} page {}: {}",
                        failure.section + 1,
                        failure.page,
                        failure.reason
                    )));
                }
                // Endnotes read at the end of the document, so they follow the last
                // body page rather than sitting at the foot of their reference's page.
                if input.note_defaults[1]
                    .as_ref()
                    .and_then(|policy| policy.pos.as_deref())
                    .unwrap_or("docEnd")
                    == "docEnd"
                {
                    paginator::append_endnote_pages_at_document_end(
                        &mut pagination.pages,
                        &mut pagination.paint,
                        &notes,
                        final_geometry.clone(),
                        pagination.last_flow_space,
                        &pagination.endnotes_placed,
                        sections
                            .last()
                            .map(|section| paginator::EndnoteFurniture {
                                section,
                                section_index: sections.len() - 1,
                                fm: &self.font_manager,
                                media: media.media(),
                            })
                            .as_ref(),
                    )
                    .map_err(|failure| {
                        LayoutError::Layout(format!(
                            "section {} page {}: {}",
                            failure.section + 1,
                            failure.page,
                            failure.reason
                        ))
                    })?;
                } else if !pagination.endnotes_placed.is_empty() {
                    paginator::append_endnote_pages_at_document_end(
                        &mut pagination.pages,
                        &mut pagination.paint,
                        &notes,
                        final_geometry.clone(),
                        None,
                        &pagination.endnotes_placed,
                        sections
                            .last()
                            .map(|section| paginator::EndnoteFurniture {
                                section,
                                section_index: sections.len() - 1,
                                fm: &self.font_manager,
                                media: media.media(),
                            })
                            .as_ref(),
                    )
                    .map_err(|failure| {
                        LayoutError::Layout(format!(
                            "section {} page {}: {}",
                            failure.section + 1,
                            failure.page,
                            failure.reason
                        ))
                    })?;
                } else {
                    paginator::append_endnote_pages_at_document_end(
                        &mut pagination.pages,
                        &mut pagination.paint,
                        &notes,
                        final_geometry.clone(),
                        None,
                        &[],
                        sections
                            .last()
                            .map(|section| paginator::EndnoteFurniture {
                                section,
                                section_index: sections.len() - 1,
                                fm: &self.font_manager,
                                media: media.media(),
                            })
                            .as_ref(),
                    )
                    .map_err(|failure| {
                        LayoutError::Layout(format!(
                            "section {} page {}: {}",
                            failure.section + 1,
                            failure.page,
                            failure.reason
                        ))
                    })?;
                }
                apply_page_background(&mut pagination.pages, &mut pagination.paint, input)?;
                pagination.paint.validate(&pagination.pages)?;
                pagination.paint.boundaries.clear();
                for (page, policies) in pagination
                    .pages
                    .iter_mut()
                    .zip(&mut pagination.paint.policies)
                {
                    mark_remaining_artifacts(&mut page.elements, policies)?;
                }
                page_sections = pagination.page_sections;
                let mut body_fragments = vec![Vec::new(); input.document.body.content.len()];
                for (body_index, fragment) in pagination.body_fragments {
                    if let Some(fragments) = body_fragments.get_mut(body_index) {
                        fragments.push(fragment);
                    }
                }
                (
                    pagination.pages.into_iter().map(Arc::new).collect(),
                    pagination.outlines,
                    Vec::new(),
                    body_fragments,
                    pagination.paint.policies,
                )
            };
        if body_unchanged
            && let Some(cache) = self.restart_cache.as_ref()
            && cache.raw_pages.len() == pages.len()
            && cache.raw_policies == page_policies
        {
            for (page, retained) in pages.iter_mut().zip(&cache.raw_pages) {
                *page = Arc::clone(retained);
            }
            if cache.body_fragments.len() == body_fragments.len() {
                body_fragments.clone_from(&cache.body_fragments);
            }
        }
        if sources.is_none() {
            body_fragments.clear();
        }
        if page_policies.len() != pages.len() {
            return Err(LayoutError::Layout(
                "page paint companion count mismatch".into(),
            ));
        }
        for (page, policies) in pages.iter().zip(&page_policies) {
            policies.validate(&page.elements)?;
        }
        let mut raw_policies = restart_record_eligible.then(|| page_policies.clone());
        let mut raw_pages = restart_record_eligible.then(|| pages.clone());

        if page_sections.is_empty() {
            page_sections.extend(pages.iter().map(|page| crate::WordPageSection {
                physical_page: page.page_number,
                displayed_page: page.displayed_page_number,
                section_index: 0,
            }));
        }
        // Document-end note overflow pages belong to the final section.
        for page in &pages {
            if !page_sections
                .iter()
                .any(|record| record.physical_page == page.page_number)
            {
                page_sections.push(crate::WordPageSection {
                    physical_page: page.page_number,
                    displayed_page: page.displayed_page_number,
                    section_index: sections.len().saturating_sub(1),
                });
            }
        }
        let mut section_paths = Vec::new();
        let mut section_index = 0;
        for item in body_layout_items(&input.document.body) {
            match item {
                MainStoryLayoutItem::Paragraph(paragraph, path) => {
                    section_paths.push((path, section_index));
                    if paragraph
                        .properties
                        .as_ref()
                        .is_some_and(|properties| properties.sect_pr.is_some())
                    {
                        section_index += 1;
                    }
                }
                MainStoryLayoutItem::Table(_, path) => section_paths.push((path, section_index)),
            }
        }
        let owner_section = |node: SourceNodeId, fallback: usize| {
            sources
                .and_then(|registry| registry.nodes.get(node.get() as usize - 1))
                .and_then(|path| {
                    let children = match &path.story {
                        WordStory::Document => &path.children,
                        WordStory::TextBox {
                            part_name,
                            owner_children,
                        } if input.story_part_names.get(&WordStory::Document)
                            == Some(part_name) =>
                        {
                            owner_children
                        }
                        _ => return None,
                    };
                    section_paths
                        .iter()
                        .find(|(prefix, _)| children.starts_with(prefix))
                        .map(|(_, index)| *index)
                })
                .unwrap_or(fallback)
        };
        let mut note_owners = HashMap::<NoteRef, Option<crate::WordPageSection>>::new();
        for page in &pages {
            let fallback = page_sections
                .iter()
                .find(|record| record.physical_page == page.page_number)
                .map_or(0, |record| record.section_index);
            oxoffice_layout::walk(&page.elements, &mut |element, _| {
                let (reference, span) = match element {
                    PositionedElement::Text(run) => (run.note, run.note_reference_source),
                    PositionedElement::MultilingualText(run) => {
                        (run.note, run.note_reference_source)
                    }
                    _ => return,
                };
                let Some(reference) = reference else {
                    return;
                };
                let Some(span) = span else {
                    return;
                };
                let Some(path) =
                    sources.and_then(|sources| sources.nodes.get(span.node.get() as usize - 1))
                else {
                    return;
                };
                if path.story != WordStory::Document {
                    return;
                }
                let record = crate::WordPageSection {
                    physical_page: page.page_number,
                    displayed_page: page.displayed_page_number,
                    section_index: owner_section(span.node, fallback),
                };
                note_owners
                    .entry(reference)
                    .and_modify(|owner| {
                        if *owner != Some(record) {
                            *owner = None;
                        }
                    })
                    .or_insert(Some(record));
            });
        }
        let note_for_node = |node: SourceNodeId| {
            let sources = sources?;
            let path = sources.nodes.get(node.get() as usize - 1)?;
            let story = match &path.story {
                WordStory::TextBox {
                    part_name,
                    owner_children,
                } => sources
                    .story_owner_children
                    .iter()
                    .filter(|(story, prefix)| {
                        sources.story_part_names.get(*story) == Some(part_name)
                            && owner_children.starts_with(prefix)
                    })
                    .max_by_key(|(_, prefix)| prefix.len())
                    .map(|(story, _)| story)?,
                story => story,
            };
            match story {
                WordStory::Footnote { id } => Some(NoteRef {
                    stream: NoteStream::Footnote,
                    id: *id,
                }),
                WordStory::Endnote { id } => Some(NoteRef {
                    stream: NoteStream::Endnote,
                    id: *id,
                }),
                _ => None,
            }
        };
        let mut field_placements = Vec::new();
        for page in &pages {
            let section_index = page_sections
                .iter()
                .find(|record| record.physical_page == page.page_number)
                .map_or(0, |record| record.section_index);
            oxoffice_layout::walk(&page.elements, &mut |element, _| {
                let source = match element {
                    PositionedElement::Text(run) => run.field_source,
                    PositionedElement::MultilingualText(run) => run.field_source,
                    _ => None,
                };
                if let Some(source) = source {
                    let reference_owner = note_for_node(source.node)
                        .map(|note| note_owners.get(&note).copied().flatten());
                    if reference_owner == Some(None) {
                        return;
                    }
                    let reference_owner = reference_owner.flatten();
                    let placement = crate::WordFieldPlacement {
                        source,
                        physical_page: page.page_number,
                        displayed_page: reference_owner
                            .map_or(page.displayed_page_number, |owner| owner.displayed_page),
                        section_index: reference_owner.map_or_else(
                            || owner_section(source.node, section_index),
                            |owner| owner.section_index,
                        ),
                    };
                    if !field_placements.contains(&placement) {
                        field_placements.push(placement);
                    }
                    if let Some(descendants) =
                        sources.and_then(|registry| registry.field_descendants.get(&source))
                    {
                        for &index in descendants {
                            let child = crate::WordFieldPlacement {
                                source: FieldSource {
                                    node: source.node,
                                    index,
                                },
                                ..placement
                            };
                            if !field_placements.contains(&child) {
                                field_placements.push(child);
                            }
                        }
                    }
                }
            });
        }
        self.last_page_sections = page_sections;
        self.last_field_placements = field_placements;
        let styleref_values = sources.map_or_else(HashMap::new, |registry| {
            placed_styleref_values(
                input,
                registry,
                &num_state,
                &pages,
                &self.last_field_placements,
            )
        });
        // Post-pagination pass: record bookmark targets and substitute fields.
        let total_pages = pages.len();
        let bookmark_pages = pages
            .iter()
            .flat_map(|page| {
                let mut targets = Vec::new();
                oxoffice_layout::walk(&page.elements, &mut |element, _| {
                    if let PositionedElement::Text(run) = element
                        && let Some(FieldKind::Target(target)) = run.field_kind
                    {
                        targets.push((target, page.displayed_page_number));
                    }
                });
                targets
            })
            .collect::<HashMap<_, _>>();
        let mut bookmark_owners = HashMap::new();
        for page in &pages {
            let fallback = self
                .last_page_sections
                .iter()
                .find(|record| record.physical_page == page.page_number)
                .map_or(0, |record| record.section_index);
            oxoffice_layout::walk(&page.elements, &mut |element, _| {
                if let PositionedElement::Text(run) = element
                    && let Some(FieldKind::Target(target)) = run.field_kind
                {
                    bookmark_owners.insert(
                        target,
                        crate::WordPageSection {
                            physical_page: page.page_number,
                            displayed_page: page.displayed_page_number,
                            section_index: run
                                .source
                                .map_or(fallback, |span| owner_section(span.node, fallback)),
                        },
                    );
                }
            });
        }
        self.last_bookmark_pages = bookmark_owners;
        let page_reference_names = page_reference_names(input);
        let mut unresolved_targets = Vec::new();
        for page in &pages {
            oxoffice_layout::walk(&page.elements, &mut |element, _| {
                if let PositionedElement::Text(run) = element
                    && let Some(FieldKind::TargetPage(target)) = run.field_kind
                    && !bookmark_pages.contains_key(&target)
                {
                    unresolved_targets.push(target);
                }
            });
        }
        unresolved_targets.sort_unstable();
        unresolved_targets.dedup();
        for target in unresolved_targets {
            let name = page_reference_names
                .get(target)
                .cloned()
                .unwrap_or_else(|| format!("#{target}"));
            diagnostics.push(Diagnostic {
                message: format!(
                    "PAGEREF target {name} did not reach pagination, unresolved placeholder retained"
                ),
            });
        }
        let mut bookmark_identity = bookmark_pages
            .iter()
            .map(|(&target, &page_number)| (target, page_number))
            .collect::<Vec<_>>();
        bookmark_identity.sort_unstable();
        let mut sequence_boundary = 0;
        let sequence_boundaries = pages
            .iter()
            .map(|page| {
                sequence_boundary = sequence_event_boundary(&page.elements, sequence_boundary);
                sequence_boundary
            })
            .collect::<Vec<_>>();
        let mut section_formats = document_sections(&input.document)
            .into_iter()
            .map(section_page_number_format)
            .collect::<Vec<_>>();
        // Pagination supplies this same implicit final section when no body
        // sectPr exists. It is qualified absence, not an unknown owner index.
        if input.document.body.sect_pr.is_none() {
            section_formats.push(Some("decimal".into()));
        }
        let mut section_pages = HashMap::<usize, std::collections::HashSet<usize>>::new();
        for record in &self.last_page_sections {
            section_pages
                .entry(record.section_index)
                .or_default()
                .insert(record.physical_page);
        }
        let section_page_counts = section_pages
            .into_iter()
            .map(|(section, pages)| (section, pages.len()))
            .collect::<HashMap<_, _>>();
        let mut placements_by_page = HashMap::<usize, Vec<&crate::WordFieldPlacement>>::new();
        for placement in &self.last_field_placements {
            placements_by_page
                .entry(placement.physical_page)
                .or_default()
                .push(placement);
        }
        let mut substitution_inputs = Vec::with_capacity(pages.len());
        let mut reuse_result_pages = vec![false; pages.len()];
        for (page_index, page) in pages.iter_mut().enumerate() {
            if !page_has_substitution_state(page)
                && !styleref_values.contains_key(&page.page_number)
            {
                reuse_result_pages[page_index] = self.restart_cache.as_ref().is_some_and(|cache| {
                    cache.substitution_inputs.get(page_index) == Some(&None)
                        && cache.raw_policies.get(page_index) == page_policies.get(page_index)
                        && cache.policies.len() == cache.pages.len()
                        && cache
                            .raw_pages
                            .get(page_index)
                            .is_some_and(|retained| Arc::ptr_eq(page, retained))
                });
                substitution_inputs.push(None);
                continue;
            }
            let placements = placements_by_page
                .get(&page.page_number)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let mut page_field_values = HashMap::new();
            let mut format_context = Vec::new();
            if let Some(sources) = sources {
                for placement in placements {
                    let Some((instruction, stored)) = sources.page_fields.get(&placement.source)
                    else {
                        continue;
                    };
                    let locked = sources
                        .field_source_contexts
                        .get(&placement.source)
                        .is_some_and(|(_, locked, _)| *locked);
                    let target = (instruction.name == "PAGEREF")
                        .then(|| {
                            field_text_argument(instruction, 0)
                                .and_then(|name| page_ref_id(input, name))
                                .and_then(|target| self.last_bookmark_pages.get(&target))
                        })
                        .flatten();
                    let owner =
                        target.map_or(placement.section_index, |target| target.section_index);
                    let section_format = if matches!(instruction.name.as_str(), "PAGE" | "PAGEREF")
                    {
                        section_formats.get(owner).and_then(Option::as_deref)
                    } else {
                        Some("decimal")
                    };
                    let value = match instruction.name.as_str() {
                        "PAGE" => Some(placement.displayed_page),
                        "NUMPAGES" => Some(total_pages),
                        "SECTION" => Some(placement.section_index + 1),
                        "SECTIONPAGES" => Some(
                            *section_page_counts
                                .get(&placement.section_index)
                                .unwrap_or(&0),
                        ),
                        "PAGEREF" => target.map(|target| target.displayed_page),
                        _ => None,
                    };
                    format_context.push((placement.source.node.get(), placement.source.index, format!("{instruction:?}{owner:?}{section_format:?}{target:?}{locked:?}{value:?}{stored:?}")));
                    let result = if locked {
                        Err("locked owner".into())
                    } else {
                        value
                            .ok_or_else(|| {
                                "bookmark target is missing, ambiguous or unplaced".to_owned()
                            })
                            .and_then(|value| {
                                format_page_field_result(instruction, value, section_format)
                            })
                    };
                    let value = result.unwrap_or_else(|reason| {
                        diagnostics.push(Diagnostic {
                            message: format!(
                                "{} field {:?} retained stored display: {reason}",
                                instruction.name, placement.source
                            ),
                        });
                        stored.clone()
                    });
                    page_field_values.insert(
                        placement.source,
                        PageFieldValue {
                            display: value,
                            target_unplaced: instruction.name == "PAGEREF" && target.is_none(),
                        },
                    );
                }
            }
            format_context.sort_unstable();
            let page_format_context = format!("{format_context:?}");
            let inputs = FieldSubstitutionInputs {
                page_index,
                page_number: page.displayed_page_number,
                total_pages,
                bookmark_pages: bookmark_identity.clone(),
                page_format_context,
                font_identity: font_trace.clone(),
                revision_view: input.revision_view,
                revision_style: input.revision_style,
            };
            let reusable = input.sequence_snapshot.is_none()
                && self.restart_cache.as_ref().is_some_and(|cache| {
                    cache
                        .substitution_inputs
                        .get(page_index)
                        .and_then(Option::as_ref)
                        == Some(&inputs)
                        && cache.raw_policies.get(page_index) == page_policies.get(page_index)
                        && cache.policies.len() == cache.pages.len()
                        && cache
                            .raw_pages
                            .get(page_index)
                            .is_some_and(|retained| Arc::ptr_eq(page, retained))
                });
            if reusable {
                page_policies[page_index] = self
                    .restart_cache
                    .as_ref()
                    .expect("reusable page owns final policy")
                    .policies[page_index]
                    .clone();
                reuse_result_pages[page_index] = true;
                substitution_inputs.push(Some(inputs));
                continue;
            }
            let page = Arc::make_mut(page);
            let page_num = page.displayed_page_number;
            let section_index = self
                .last_page_sections
                .iter()
                .find(|record| record.physical_page == page.page_number)
                .map_or(0, |record| record.section_index);
            let section_pages = *section_page_counts.get(&section_index).unwrap_or(&0);
            let field_sections = placements
                .iter()
                .map(|placement| {
                    (
                        placement.source,
                        (
                            placement.section_index + 1,
                            *section_page_counts
                                .get(&placement.section_index)
                                .unwrap_or(&0),
                            placement.displayed_page,
                        ),
                    )
                })
                .collect();
            let mut painted = PaintedElements::from_parts(
                std::mem::take(&mut page.elements),
                std::mem::take(&mut page_policies[page_index]),
            )?;
            let mut occurrence_state = substitute_fields(
                &mut painted,
                page_num,
                total_pages,
                (section_index + 1, section_pages),
                &field_sections,
                &bookmark_pages,
                &page_field_values,
                input.sequence_snapshot.as_deref(),
                sequence_boundaries[page_index],
                &mut self.font_manager,
            )?;
            if let Some(values) = styleref_values.get(&page.page_number) {
                let mut changes = Vec::new();
                substitute_styleref(
                    &mut painted,
                    values,
                    &mut std::collections::HashSet::new(),
                    &mut self.font_manager,
                    &mut changes,
                    &mut occurrence_state,
                )?;
                if !changes.is_empty() {
                    realign_tab_aligned_text(painted.coordinate_elements_mut(), &changes);
                }
            }
            for diagnostic in occurrence_state.diagnostics {
                push_unique_diagnostic(&mut diagnostics, diagnostic.message);
            }
            let (elements, mut policies) = painted.into_parts()?;
            policies.retire_annotations();
            policies.validate(&elements)?;
            (page.elements, page_policies[page_index]) = (elements, policies);
            substitution_inputs.push(Some(inputs));
        }

        if restart_checkpoint.is_some()
            && let Some(cache) = self.restart_cache.as_ref()
            && cache.font_trace == font_trace
        {
            for (page_index, (page, retained)) in pages.iter().zip(&cache.raw_pages).enumerate() {
                if Arc::ptr_eq(page, retained)
                    && raw_policies.as_ref().and_then(|maps| maps.get(page_index))
                        == cache.raw_policies.get(page_index)
                {
                    reuse_result_pages[page_index] = true;
                }
            }
        }
        // Metrics-only empty carriers must still resolve through the result,
        // but they do not get to move a glyph-bearing font earlier in the
        // deterministic result order.
        let mut carrier_fonts = Vec::new();
        fn collect_carrier_fonts(elements: &[PositionedElement], fonts: &mut Vec<FontId>) {
            for element in elements {
                match element {
                    PositionedElement::Text(run)
                        if run.text.is_empty() && run.glyph_ids.is_empty() =>
                    {
                        if !fonts.contains(&run.font_id) {
                            fonts.push(run.font_id);
                        }
                    }
                    PositionedElement::Group(group) => {
                        collect_carrier_fonts(&group.children, fonts)
                    }
                    PositionedElement::MarkedContent { children, .. } => {
                        collect_carrier_fonts(children, fonts)
                    }
                    _ => {}
                }
            }
        }
        for page in &pages {
            collect_carrier_fonts(&page.elements, &mut carrier_fonts);
        }
        self.font_manager.replay_layout_font_trace(&carrier_fonts);

        // Remap persistent manager ids to result-local ids and omit faces that
        // are no longer present in the current layout.
        let fonts = if self.font_manager.every_loaded_font_is_current() {
            self.font_manager.all_font_data()
        } else {
            let current_fonts = self.font_manager.current_layout_fonts().to_vec();
            canonicalize_layout_fonts(&mut pages, &self.font_manager, &current_fonts)?
        };
        if let Some(cache) = self.restart_cache.as_ref() {
            for (page_index, reuse) in reuse_result_pages.into_iter().enumerate() {
                if reuse
                    && cache.policies.get(page_index) == page_policies.get(page_index)
                    && let Some(retained) = cache.pages.get(page_index)
                {
                    pages[page_index] = Arc::clone(retained);
                }
            }
        }
        let mut retained_pages = restart_record_eligible.then(|| pages.clone());

        // Convert core properties to document metadata
        let metadata = input.core_properties.as_ref().map(|cp| DocumentMetadata {
            title: cp.title.clone(),
            author: cp.creator.clone(),
            subject: cp.subject.clone(),
            keywords: cp.keywords.clone(),
            creator: Some("oxoffice".to_string()),
        });

        if restart_record_eligible
            && pages.len().max(checkpoints.len()) <= RESTART_CACHE_MAX_ENTRIES
            && let Some(raw_pages) = raw_pages.as_mut()
            && let Some(retained_pages) = retained_pages.as_mut()
        {
            let old_body = self
                .restart_cache
                .as_ref()
                .map(|cache| cache.body.as_slice());
            let prefix = first_changed.unwrap_or(0);
            let suffix = common_suffix.unwrap_or(0);
            let new_len = input.document.body.content.len();
            let old_len = old_body.map_or(0, <[RestartBodyEntry]>::len);
            let mut body = input
                .document
                .body
                .content
                .iter()
                .enumerate()
                .filter_map(|(index, content)| {
                    if index < prefix {
                        return old_body.and_then(|body| body.get(index)).cloned();
                    }
                    if index >= new_len.saturating_sub(suffix) {
                        let old_index = old_len.saturating_sub(new_len - index);
                        return old_body.and_then(|body| body.get(old_index)).cloned();
                    }
                    RestartBodyEntry::for_content_memoized(
                        content,
                        input.revision_view,
                        index,
                        &mut restart_identity_memo,
                    )
                })
                .collect::<Vec<_>>();
            let body_complete = body.len() == new_len;
            body.shrink_to_fit();
            raw_pages.shrink_to_fit();
            retained_pages.shrink_to_fit();
            substitution_inputs.shrink_to_fit();
            outlines.shrink_to_fit();
            checkpoints.shrink_to_fit();
            font_trace.shrink_to_fit();
            for fragments in &mut body_fragments {
                fragments.shrink_to_fit();
            }
            body_fragments.shrink_to_fit();
            let mut candidate = RestartCache {
                paint: producer_paints.expect("restart candidate has actual Word producers"),
                body,
                with_provenance: sources.is_some(),
                body_fragments: body_fragments.clone(),
                raw_pages: std::mem::take(raw_pages),
                raw_policies: raw_policies
                    .take()
                    .expect("raw artifacts retain companion maps"),
                policies: page_policies.clone(),
                pages: std::mem::take(retained_pages),
                substitution_inputs,
                outlines: outlines.clone(),
                checkpoints,
                font_trace,
                bytes: 0,
            };
            candidate.outlines.shrink_to_fit();
            for inputs in candidate.substitution_inputs.iter_mut().flatten() {
                inputs.bookmark_pages.shrink_to_fit();
                inputs.font_identity.shrink_to_fit();
                inputs.page_format_context.shrink_to_fit();
            }
            let bytes = if body_complete {
                restart_cache_bytes(&candidate)
            } else {
                usize::MAX
            };
            let entries = restart_cache_entries(&candidate);
            if self.restart_candidate_fits_aggregate(entries, bytes) {
                candidate.bytes = bytes;
                self.restart_cache = Some(candidate);
            } else {
                self.restart_cache = None;
            }
        } else {
            self.restart_cache = None;
        }
        drop(restart_identity_memo);
        self.navigation_by_source
            .extend(num_state.take_navigation());
        if let Some(registry) = sources
            && input
                .sequence_snapshot
                .as_ref()
                .is_none_or(|snapshot| snapshot.navigation_failure.is_none())
        {
            let mut positions = HashMap::new();
            for (page_index, page) in pages.iter().enumerate() {
                oxoffice_layout::walk(&page.elements, &mut |element, transform| {
                    let (source, origin) = match element {
                        PositionedElement::Text(run) => (run.source, run.origin),
                        PositionedElement::MultilingualText(run) => (run.source, run.origin),
                        _ => return,
                    };
                    if let Some(source) = source {
                        positions
                            .entry(source.node)
                            .or_insert((page_index, transform.apply(origin).y));
                    }
                });
            }
            for (node, outline) in body_heading_sources.iter().zip(&outlines) {
                positions.insert(*node, (outline.page_index, outline.y_position));
            }
            outlines = registry
                .nodes
                .iter()
                .enumerate()
                .filter_map(|(index, path)| {
                    if path.story != WordStory::Document {
                        return None;
                    }
                    let node = SourceNodeId::new(u32::try_from(index + 1).ok()?)?;
                    let facts = self.navigation_by_source.get(&node)?;
                    let level = facts.level?;
                    let (page_index, y_position) = positions.get(&node).copied()?;
                    let title = match num_state.resolved_for_source(node) {
                        Some(number)
                            if !number.marker_text.is_empty()
                                && number.marker_rpr.vanish != Some(true) =>
                        {
                            format!("{} {}", number.marker_text, facts.text)
                        }
                        _ => facts.text.clone(),
                    };
                    Some(oxoffice_layout::OutlineEntry {
                        title,
                        level,
                        page_index,
                        y_position,
                    })
                })
                .collect();
        }
        let mut result = LayoutResult::new(pages, fonts, metadata, outlines);
        diagnostics.extend(self.font_manager.current_font_diagnostics().iter().cloned());
        diagnostics.extend(self.font_manager.word_substitution_diagnostics());
        result.diagnostics = diagnostics;
        self.last_picture_draw_owners = media.picture_draw_owners_since(0);
        result.structure = Some(structure);
        let references = num_state.references_only();
        self.bookmark_numbering
            .extend(
                num_state
                    .bookmark_numbering()
                    .filter_map(|(name, source, numbering)| {
                        let registered = sources.is_some_and(|registry| {
                            registry.nodes.get(source.get() as usize - 1).is_some()
                        });
                        (registered && bookmark_text(input, name).is_some())
                            .then(|| (name.to_owned(), numbering.clone()))
                    }),
            );
        self.bookmark_suppressed.extend(
            num_state
                .bookmark_sources()
                .filter(|(name, source)| {
                    self.navigation_by_source
                        .get(source)
                        .is_some_and(|facts| facts.suppressed_numbering)
                        && bookmark_text(input, name).is_some()
                })
                .map(|(name, _)| name.to_owned()),
        );
        self.numbering_by_source.extend(num_state.take_resolved());
        self.last_body_fragments = body_fragments;
        Ok((result, references))
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_body_paragraph(
        &mut self,
        paragraph: &CT_P,
        content_width: f64,
        styles: &CT_Styles,
        input: &LayoutInput,
        media: &MediaRegistry,
        numbering: &mut NumberingState,
        diagnostics: &mut Vec<Diagnostic>,
        source_node: Option<SourceNodeId>,
        doc_grid: Option<&CT_DocGrid>,
    ) -> Result<SharedLayoutBlock> {
        let excluded_owner = source_node
            .and_then(|node| input.sequence_snapshot.as_ref()?.source_node(node))
            .is_some_and(|path| {
                input
                    .sequence_snapshot
                    .as_ref()
                    .is_some_and(|s| s.navigation_exclusions.contains_key(path))
            });
        if !paragraph_is_cache_safe(paragraph, styles) || excluded_owner {
            // Traversal-sensitive content can change generated state consumed
            // by later blocks. The conservative boundary is the first such
            // block, after which no retained block is read in this layout.
            self.paragraph_cache_reads_enabled = false;
            let (block, reflow_direction, paint) = layout_paragraph_with_source_and_direction(
                paragraph,
                content_width,
                styles,
                input,
                media,
                &mut self.font_manager,
                numbering,
                diagnostics,
                source_node,
                source_node,
                doc_grid,
            )?;
            return Ok(SharedLayoutBlock::Owned {
                semantics: Some(ParagraphSemantics {
                    tb_rl_exact_baseline: None,
                    border_group_before: false,
                    source_node,
                    structure_id: block.structure_id,
                    reflow_direction,
                    paint: Arc::new(paint),
                }),
                border_group_before: false,
                block: Box::new(LayoutBlock::Paragraph(block)),
                reflow_direction,
                body_index: None,
            });
        }

        let fingerprint = paragraph_fingerprint(paragraph);
        let notes_allow_reuse = self.paragraph_cache_notes_match
            || paragraph_note_references(paragraph, input.revision_view)?.is_empty();
        if self.paragraph_cache_reads_enabled
            && notes_allow_reuse
            && let Some(entry) = self.paragraph_cache.iter().find(|entry| {
                entry.fingerprint == fingerprint
                    && entry.key.paragraph == *paragraph
                    && entry.key.content_width_bits == content_width.to_bits()
                    && entry.key.revision_view == input.revision_view
                    && entry.key.revision_style == input.revision_style
                    && entry.key.doc_grid.as_ref() == doc_grid
            })
        {
            diagnostics.extend(entry.diagnostics.iter().cloned());
            self.font_manager
                .replay_font_trace_events(&entry.font_trace)?;
            return Ok(SharedLayoutBlock::Paragraph {
                block: Arc::clone(&entry.block),
                semantics: ParagraphSemantics {
                    tb_rl_exact_baseline: None,
                    border_group_before: false,
                    source_node,
                    structure_id: None,
                    reflow_direction: entry.reflow_direction,
                    paint: Arc::clone(&entry.paint),
                },
                body_index: None,
            });
        }

        let diagnostics_start = diagnostics.len();
        self.font_manager.begin_paragraph_font_trace();
        let block_result = layout_paragraph_with_source_and_direction(
            paragraph,
            content_width,
            styles,
            input,
            media,
            &mut self.font_manager,
            numbering,
            diagnostics,
            Some(CACHE_SOURCE_NODE),
            source_node,
            doc_grid,
        );
        let font_trace = self.font_manager.finish_paragraph_font_events();
        let (mut block, reflow_direction, paint) = block_result?;
        let paint = Arc::new(paint);

        let cached_diagnostics = diagnostics[diagnostics_start..].to_vec();
        if let Some(font_trace) = font_trace {
            let bytes = paragraph_cache_entry_bytes(
                paragraph,
                &block,
                Some(&paint),
                &cached_diagnostics,
                font_contribution_trace_bytes(&font_trace),
            );
            let block = Arc::new(block);
            self.stage_paragraph_cache_entry(ParagraphCacheEntry {
                fingerprint,
                key: ParagraphCacheKey {
                    paragraph: paragraph.clone(),
                    content_width_bits: content_width.to_bits(),
                    revision_view: input.revision_view,
                    revision_style: input.revision_style,
                    doc_grid: doc_grid.cloned(),
                },
                block: Arc::clone(&block),
                paint: Arc::clone(&paint),
                diagnostics: cached_diagnostics,
                font_trace,
                reflow_direction,
                bytes,
            });
            return Ok(SharedLayoutBlock::Paragraph {
                block,
                semantics: ParagraphSemantics {
                    tb_rl_exact_baseline: None,
                    border_group_before: false,
                    source_node,
                    structure_id: None,
                    reflow_direction,
                    paint: Arc::clone(&paint),
                },
                body_index: None,
            });
        }

        rebind_paragraph_source(&mut block, source_node)?;
        Ok(SharedLayoutBlock::Owned {
            semantics: Some(ParagraphSemantics {
                tb_rl_exact_baseline: None,
                border_group_before: false,
                source_node,
                structure_id: block.structure_id,
                reflow_direction,
                paint,
            }),
            border_group_before: false,
            block: Box::new(LayoutBlock::Paragraph(block)),
            reflow_direction,
            body_index: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_body_table(
        &mut self,
        table: &CT_Tbl,
        content_width: f64,
        styles: &CT_Styles,
        input: &LayoutInput,
        media: &MediaRegistry,
        numbering: &mut NumberingState,
        diagnostics: &mut Vec<Diagnostic>,
        sources: Option<&SourceRegistry>,
        story: &WordStory,
        path: &[usize],
        doc_grid: Option<&CT_DocGrid>,
    ) -> Result<SharedLayoutBlock> {
        let excluded_owner = input.sequence_snapshot.as_ref().is_some_and(|s| {
            s.navigation_owners.contains(&WordSourcePath {
                story: story.clone(),
                children: path.to_vec(),
            })
        });
        if !table_is_cache_safe(table, styles) || excluded_owner {
            self.paragraph_cache_reads_enabled = false;
            return table::layout_table_with_provenance(
                table,
                content_width,
                styles,
                input,
                media,
                &mut self.font_manager,
                numbering,
                diagnostics,
                sources,
                story,
                path,
                doc_grid,
            )
            .map(|(block, semantics)| SharedLayoutBlock::Table {
                block: Arc::new(block),
                semantics,
                body_index: None,
            });
        }

        let fingerprint = table_fingerprint(table);
        if self.paragraph_cache_reads_enabled
            && self.retained_context_matches_full
            && let Some(entry) = self.table_cache.iter().find(|entry| {
                entry.fingerprint == fingerprint
                    && entry.key.table == *table
                    && entry.key.content_width_bits == content_width.to_bits()
                    && entry.key.revision_view == input.revision_view
                    && entry.key.revision_style == input.revision_style
                    && entry.key.with_provenance == sources.is_some()
                    && entry.key.doc_grid.as_ref() == doc_grid
            })
        {
            diagnostics.extend(entry.diagnostics.iter().cloned());
            self.font_manager
                .replay_font_trace_events(&entry.font_trace)?;
            if let Some(sources) = sources {
                record_cached_table_omissions(table, &entry.block, sources, story, path, numbering);
            }
            return Ok(SharedLayoutBlock::Table {
                block: Arc::clone(&entry.block),
                semantics: table_semantics(
                    table,
                    entry.block.as_ref(),
                    &entry.semantics,
                    sources,
                    story,
                    path,
                ),
                body_index: None,
            });
        }

        let diagnostics_start = diagnostics.len();
        self.font_manager.begin_paragraph_font_trace();
        let block_result = table::layout_table_with_provenance(
            table,
            content_width,
            styles,
            input,
            media,
            &mut self.font_manager,
            numbering,
            diagnostics,
            sources,
            story,
            path,
            doc_grid,
        );
        let font_trace = self.font_manager.finish_paragraph_font_events();
        let (mut block, semantics) = block_result?;

        let cached_diagnostics = diagnostics[diagnostics_start..].to_vec();
        if let Some(font_trace) = font_trace {
            canonicalize_table_sources(&mut block)?;
            let key = TableCacheKey {
                table: table.clone(),
                content_width_bits: content_width.to_bits(),
                revision_view: input.revision_view,
                revision_style: input.revision_style,
                with_provenance: sources.is_some(),
                doc_grid: doc_grid.cloned(),
            };
            let bytes = table_cache_entry_bytes(
                &key,
                &block,
                &semantics,
                &cached_diagnostics,
                font_contribution_trace_bytes(&font_trace),
            );
            let block = Arc::new(block);
            self.stage_table_cache_entry(TableCacheEntry {
                fingerprint,
                key,
                block: Arc::clone(&block),
                semantics: semantics.clone(),
                diagnostics: cached_diagnostics,
                font_trace,
                bytes,
            });
            return Ok(SharedLayoutBlock::Table {
                block,
                semantics,
                body_index: None,
            });
        }
        Ok(SharedLayoutBlock::Table {
            block: Arc::new(block),
            semantics,
            body_index: None,
        })
    }

    fn publish_paragraph_cache_entry(&mut self, entry: ParagraphCacheEntry) {
        if entry.bytes > PARAGRAPH_CACHE_MAX_BYTES {
            return;
        }
        while self.paragraph_cache.len() >= PARAGRAPH_CACHE_MAX_ENTRIES
            || self.paragraph_cache_bytes.saturating_add(entry.bytes) > PARAGRAPH_CACHE_MAX_BYTES
        {
            let Some(evicted) = self.paragraph_cache.pop_front() else {
                break;
            };
            self.paragraph_cache_bytes = self.paragraph_cache_bytes.saturating_sub(evicted.bytes);
        }
        let Some(bytes) = self.paragraph_cache_bytes.checked_add(entry.bytes) else {
            return;
        };
        if bytes > PARAGRAPH_CACHE_MAX_BYTES {
            return;
        }
        self.paragraph_cache_bytes = bytes;
        self.paragraph_cache.push_back(entry);
    }

    fn restart_candidate_fits_aggregate(
        &self,
        candidate_entries: usize,
        candidate_bytes: usize,
    ) -> bool {
        let pending_paragraph_entries = self
            .pending_paragraph_cache
            .as_ref()
            .map_or(0, VecDeque::len);
        let pending_table_entries = self.pending_table_cache.as_ref().map_or(0, VecDeque::len);
        let pending_header_footer_entries = self
            .pending_header_footer_cache
            .as_ref()
            .map_or(0, VecDeque::len);
        let entries = self
            .paragraph_cache
            .len()
            .checked_add(self.table_cache.len())
            .and_then(|entries| entries.checked_add(self.header_footer_cache.len()))
            .and_then(|entries| entries.checked_add(pending_paragraph_entries))
            .and_then(|entries| entries.checked_add(pending_table_entries))
            .and_then(|entries| entries.checked_add(pending_header_footer_entries))
            .and_then(|entries| entries.checked_add(candidate_entries));
        let bytes = self
            .paragraph_cache_bytes
            .checked_add(self.table_cache_bytes)
            .and_then(|bytes| bytes.checked_add(self.header_footer_cache_bytes))
            .and_then(|bytes| bytes.checked_add(self.pending_paragraph_cache_bytes))
            .and_then(|bytes| bytes.checked_add(self.pending_table_cache_bytes))
            .and_then(|bytes| bytes.checked_add(self.pending_header_footer_cache_bytes))
            .and_then(|bytes| bytes.checked_add(candidate_bytes));
        entries.is_some_and(|entries| entries <= CACHE_MAX_ENTRIES)
            && bytes.is_some_and(|bytes| bytes <= CACHE_MAX_BYTES)
    }

    fn stage_paragraph_cache_entry(&mut self, entry: ParagraphCacheEntry) {
        if entry.bytes > PARAGRAPH_CACHE_MAX_BYTES {
            return;
        }
        let Some(pending) = self.pending_paragraph_cache.as_mut() else {
            return;
        };
        while pending.len() >= PARAGRAPH_CACHE_MAX_ENTRIES
            || self
                .pending_paragraph_cache_bytes
                .saturating_add(entry.bytes)
                > PARAGRAPH_CACHE_MAX_BYTES
        {
            let Some(evicted) = pending.pop_front() else {
                break;
            };
            self.pending_paragraph_cache_bytes = self
                .pending_paragraph_cache_bytes
                .saturating_sub(evicted.bytes);
        }
        self.pending_paragraph_cache_bytes += entry.bytes;
        pending.push_back(entry);
    }

    fn publish_table_cache_entry(&mut self, entry: TableCacheEntry) {
        if entry.bytes > TABLE_CACHE_MAX_BYTES {
            return;
        }
        while self.table_cache.len() >= TABLE_CACHE_MAX_ENTRIES
            || self.table_cache_bytes.saturating_add(entry.bytes) > TABLE_CACHE_MAX_BYTES
        {
            let Some(evicted) = self.table_cache.pop_front() else {
                break;
            };
            self.table_cache_bytes = self.table_cache_bytes.saturating_sub(evicted.bytes);
        }
        self.table_cache_bytes += entry.bytes;
        self.table_cache.push_back(entry);
        let restart_entries = self.restart_cache.as_ref().map_or(0, restart_cache_entries);
        let restart_bytes = self.restart_cache.as_ref().map_or(0, |cache| cache.bytes);
        debug_assert!(
            self.paragraph_cache.len() + self.table_cache.len() + restart_entries
                <= CACHE_MAX_ENTRIES
        );
        debug_assert!(
            self.paragraph_cache_bytes + self.table_cache_bytes + restart_bytes <= CACHE_MAX_BYTES
        );
    }

    fn stage_table_cache_entry(&mut self, entry: TableCacheEntry) {
        if entry.bytes > TABLE_CACHE_MAX_BYTES {
            return;
        }
        let Some(pending) = self.pending_table_cache.as_mut() else {
            return;
        };
        while pending.len() >= TABLE_CACHE_MAX_ENTRIES
            || self.pending_table_cache_bytes.saturating_add(entry.bytes) > TABLE_CACHE_MAX_BYTES
        {
            let Some(evicted) = pending.pop_front() else {
                break;
            };
            self.pending_table_cache_bytes =
                self.pending_table_cache_bytes.saturating_sub(evicted.bytes);
        }
        self.pending_table_cache_bytes += entry.bytes;
        pending.push_back(entry);
    }

    fn publish_header_footer_cache_entry(&mut self, entry: HeaderFooterCacheEntry) {
        if entry.bytes > HEADER_FOOTER_CACHE_MAX_BYTES {
            return;
        }
        while self.header_footer_cache.len() >= HEADER_FOOTER_CACHE_MAX_ENTRIES
            || self.header_footer_cache_bytes.saturating_add(entry.bytes)
                > HEADER_FOOTER_CACHE_MAX_BYTES
        {
            let Some(evicted) = self.header_footer_cache.pop_front() else {
                break;
            };
            self.header_footer_cache_bytes =
                self.header_footer_cache_bytes.saturating_sub(evicted.bytes);
        }
        self.header_footer_cache_bytes += entry.bytes;
        self.header_footer_cache.push_back(entry);
        let restart_entries = self.restart_cache.as_ref().map_or(0, restart_cache_entries);
        let restart_bytes = self.restart_cache.as_ref().map_or(0, |cache| cache.bytes);
        debug_assert!(
            self.paragraph_cache.len()
                + self.table_cache.len()
                + self.header_footer_cache.len()
                + restart_entries
                <= CACHE_MAX_ENTRIES
        );
        debug_assert!(
            self.paragraph_cache_bytes
                + self.table_cache_bytes
                + self.header_footer_cache_bytes
                + restart_bytes
                <= CACHE_MAX_BYTES
        );
    }

    fn stage_header_footer_cache_entry(&mut self, entry: HeaderFooterCacheEntry) {
        if entry.bytes > HEADER_FOOTER_CACHE_MAX_BYTES {
            return;
        }
        let Some(pending) = self.pending_header_footer_cache.as_mut() else {
            return;
        };
        while pending.len() >= HEADER_FOOTER_CACHE_MAX_ENTRIES
            || self
                .pending_header_footer_cache_bytes
                .saturating_add(entry.bytes)
                > HEADER_FOOTER_CACHE_MAX_BYTES
        {
            let Some(evicted) = pending.pop_front() else {
                break;
            };
            self.pending_header_footer_cache_bytes = self
                .pending_header_footer_cache_bytes
                .saturating_sub(evicted.bytes);
        }
        self.pending_header_footer_cache_bytes += entry.bytes;
        pending.push_back(entry);
    }
}

fn paragraph_source_is_cache_safe(
    paragraph: &CT_P,
    styles: &CT_Styles,
    allow_fields: bool,
    allow_bookmarks: bool,
) -> bool {
    let extra_xml_is_represented = paragraph.extra_xml.is_empty()
        || (allow_bookmarks && paragraph_bookmark_raw_is_exact(paragraph));
    if !paragraph.hyperlinks.is_empty()
        || !paragraph.comment_ranges.is_empty()
        || (!allow_bookmarks && !paragraph.bookmark_markers.is_empty())
        || !extra_xml_is_represented
        || !paragraph.content_controls.is_empty()
        || !paragraph.revisions.is_empty()
    {
        return false;
    }

    let style_id = paragraph
        .properties
        .as_ref()
        .and_then(|properties| properties.style_id.as_deref());
    let resolved = style_resolver::resolve_paragraph_properties(style_id, styles);
    if resolved.num_id.is_some()
        || paragraph.properties.as_ref().is_some_and(|properties| {
            properties.num_id.is_some()
                || properties.sect_pr.is_some()
                || properties.numbering_revision.is_some()
                || !properties.numbering_revision_xml.is_empty()
                || properties.change.is_some()
                || !properties.revision_xml.is_empty()
                || properties.rpr.as_ref().is_some_and(|rpr| {
                    !rpr.revision_markers.is_empty()
                        || rpr.change.is_some()
                        || !rpr.revision_xml.is_empty()
                        || !rpr.revision_xml_positions.is_empty()
                })
        })
    {
        return false;
    }

    paragraph.runs.iter().all(|run| {
        run.alt_drawings.is_empty()
            && run.extra_xml.is_empty()
            && run.extra_xml_positions.is_empty()
            && run.properties.as_ref().is_none_or(|rpr| {
                rpr.revision_markers.is_empty()
                    && rpr.change.is_none()
                    && rpr.revision_xml.is_empty()
                    && rpr.revision_xml_positions.is_empty()
            })
            && run.content.iter().all(|content| match content {
                RunContent::Text(_)
                | RunContent::Tab
                | RunContent::Break(_)
                | RunContent::FootnoteRef { .. }
                | RunContent::EndnoteRef { .. } => true,
                RunContent::Field(_) => allow_fields,
                _ => false,
            })
    })
}

fn paragraph_is_restart_record_source_safe(paragraph: &CT_P, styles: &CT_Styles) -> bool {
    paragraph_source_is_cache_safe(paragraph, styles, true, true)
        && paragraph.runs.iter().all(|run| {
            run.properties
                .as_ref()
                .is_none_or(|properties| properties.language.is_none())
        })
}

// Drawing content is excluded by paragraph_source_is_cache_safe, including
// through table_is_cache_safe. Picture owners therefore execute fresh on these
// paths. Only the header/footer VML cache captures and replays picture owners.
fn paragraph_is_cache_safe(paragraph: &CT_P, styles: &CT_Styles) -> bool {
    paragraph_source_is_cache_safe(paragraph, styles, false, false)
}

fn paragraph_bookmark_raw_is_exact(paragraph: &CT_P) -> bool {
    paragraph.extra_xml.len() == paragraph.bookmark_markers.len()
        && paragraph
            .extra_xml
            .iter()
            .enumerate()
            .zip(&paragraph.bookmark_markers)
            .all(|((raw_index, (run_index, raw)), marker)| {
                let raw_before = paragraph.extra_xml[..raw_index]
                    .iter()
                    .filter(|(at, _)| at == run_index)
                    .count();
                *run_index == marker.run_index()
                    && raw_before == marker.raw_before()
                    && raw_xml_is_exact_word_bookmark(raw, marker)
            })
}

fn raw_xml_is_exact_word_bookmark(raw: &[u8], marker: &BookmarkMarker) -> bool {
    let Some(start) = raw.iter().position(|byte| !byte.is_ascii_whitespace()) else {
        return false;
    };
    let end = raw
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .expect("non-empty raw XML has a final byte");
    let raw = &raw[start..=end];
    if !raw.ends_with(b"/>") {
        return false;
    }
    let Some((root_name, raw_attributes)) = raw_root_start_tag(raw) else {
        return false;
    };
    let Some(attributes) = parse_raw_attributes(raw_attributes) else {
        return false;
    };
    let expected_local_name = if marker.is_start() {
        b"bookmarkStart".as_slice()
    } else {
        b"bookmarkEnd".as_slice()
    };
    if xml_local_name(root_name) != expected_local_name
        || !raw_name_has_namespace(
            root_name,
            &attributes,
            oxoffice_docx_oxml::namespace::W_NS,
            false,
        )
    {
        return false;
    }
    let word_attribute_count = |local: &[u8]| {
        attributes
            .iter()
            .filter(|(name, _)| {
                xml_local_name(name) == local
                    && raw_name_has_namespace(
                        name,
                        &attributes,
                        oxoffice_docx_oxml::namespace::W_NS,
                        true,
                    )
            })
            .count()
    };
    if word_attribute_count(b"id") != 1
        || word_attribute_count(b"name") != usize::from(marker.is_start())
    {
        return false;
    }

    let mut document_xml = Vec::with_capacity(raw.len() + 160);
    document_xml.extend_from_slice(
        br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p>"#,
    );
    document_xml.extend_from_slice(raw);
    document_xml.extend_from_slice(b"</w:p></w:body></w:document>");
    let Ok(document) = CT_Document::from_xml(&document_xml) else {
        return false;
    };
    let [BodyContent::Paragraph(parsed)] = document.body.content.as_slice() else {
        return false;
    };
    let [(run_index, parsed_raw)] = parsed.extra_xml.as_slice() else {
        return false;
    };
    let [parsed_marker] = parsed.bookmark_markers.as_slice() else {
        return false;
    };
    parsed.properties.is_none()
        && parsed.runs.is_empty()
        && parsed.hyperlinks.is_empty()
        && parsed.comment_ranges.is_empty()
        && parsed.content_controls.is_empty()
        && parsed.revisions.is_empty()
        && *run_index == 0
        && parsed_raw == raw
        && parsed_marker.raw_before() == 0
        && parsed_marker.is_start() == marker.is_start()
        && parsed_marker.id().is_some()
        && parsed_marker.id() == marker.id()
        && parsed_marker.name() == marker.name()
        && if parsed_marker.is_start() {
            parsed_marker.name().is_some()
        } else {
            parsed_marker.name().is_none()
        }
}

fn paragraph_has_field(paragraph: &CT_P) -> bool {
    paragraph
        .runs
        .iter()
        .flat_map(|run| &run.content)
        .any(|content| matches!(content, RunContent::Field(_)))
}

fn paragraph_has_note_reference(paragraph: &CT_P) -> bool {
    paragraph.runs.iter().any(|run| {
        run.content.iter().any(|content| {
            !content_note_references(content, RevisionView::Tracked)
                .is_ok_and(|references| references.is_empty())
        })
    })
}

fn header_footer_part_is_cache_safe(
    part: &oxoffice_docx_oxml::header_footer::CT_HdrFtr,
    styles: &CT_Styles,
) -> bool {
    if !part.extra_xml.is_empty() {
        return false;
    }
    let raw_watermark_count = part
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.runs)
        .flat_map(|run| &run.extra_xml)
        .filter(|raw| raw_xml_root_is_word_pict(raw, &part.extra_namespaces))
        .count();
    if raw_watermark_count != part.watermarks().len() {
        return false;
    }
    part.paragraphs.iter().all(|paragraph| {
        if !paragraph.extra_xml.is_empty() {
            return false;
        }
        let mut projected = paragraph.clone();
        for run in &mut projected.runs {
            if run.extra_xml.len() != run.extra_xml_positions.len()
                || !run
                    .extra_xml
                    .iter()
                    .all(|raw| raw_xml_root_is_word_pict(raw, &part.extra_namespaces))
            {
                return false;
            }
            run.extra_xml.clear();
            run.extra_xml_positions.clear();
        }
        !paragraph_has_note_reference(&projected) && paragraph_is_cache_safe(&projected, styles)
    })
}

fn raw_xml_root_is_word_pict(raw: &[u8], namespaces: &[(String, String)]) -> bool {
    let raw = raw
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map_or(raw, |start| &raw[start..]);
    let Some((name, attributes)) = raw.strip_prefix(b"<").and_then(|raw| {
        let name_end = raw
            .iter()
            .position(|byte| byte.is_ascii_whitespace() || matches!(byte, b'>' | b'/'))?;
        Some((&raw[..name_end], &raw[name_end..]))
    }) else {
        return false;
    };
    let mut components = name.rsplitn(2, |byte| *byte == b':');
    if components.next() != Some(b"pict".as_slice()) {
        return false;
    }
    let prefix = components.next();
    let declaration = prefix.map_or_else(
        || "xmlns".to_owned(),
        |prefix| format!("xmlns:{}", String::from_utf8_lossy(prefix)),
    );
    if let Some(namespace) = raw_xml_start_attribute(attributes, declaration.as_bytes()) {
        return namespace == oxoffice_docx_oxml::namespace::W_NS.as_bytes();
    }
    let Some(prefix) = prefix.and_then(|prefix| std::str::from_utf8(prefix).ok()) else {
        return false;
    };
    if prefix == "w" {
        return !namespaces.iter().any(|(name, namespace)| {
            name != "xmlns:w" && namespace == oxoffice_docx_oxml::namespace::W_NS
        });
    }
    namespaces.iter().any(|(name, namespace)| {
        name.strip_prefix("xmlns:") == Some(prefix)
            && namespace == oxoffice_docx_oxml::namespace::W_NS
    })
}

fn raw_xml_start_attribute<'a>(mut input: &'a [u8], expected: &[u8]) -> Option<&'a [u8]> {
    loop {
        input = input
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
            .map_or(input, |start| &input[start..]);
        if input.first().is_none_or(|byte| matches!(byte, b'>' | b'/')) {
            return None;
        }
        let name_end = input
            .iter()
            .position(|byte| byte.is_ascii_whitespace() || matches!(byte, b'=' | b'>' | b'/'))?;
        let name = &input[..name_end];
        input = &input[name_end..];
        input = input
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
            .map_or(input, |start| &input[start..]);
        if input.first() != Some(&b'=') {
            return None;
        }
        input = &input[1..];
        input = input
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
            .map_or(input, |start| &input[start..]);
        let quote = *input.first()?;
        if !matches!(quote, b'\'' | b'"') {
            return None;
        }
        input = &input[1..];
        let value_end = input.iter().position(|byte| *byte == quote)?;
        let value = &input[..value_end];
        input = &input[value_end + 1..];
        if name == expected {
            return Some(value);
        }
    }
}

fn header_footer_section_is_cache_safe(section: &CT_SectPr) -> bool {
    section.change.is_none()
        && section.extra_xml.len() == section.extra_xml_positions.len()
        && section
            .extra_xml
            .iter()
            .zip(&section.extra_xml_positions)
            .all(|(raw, position)| CT_SectPr::raw_position_is_root_attributes(position, raw))
}

fn table_is_cache_safe(table: &CT_Tbl, styles: &CT_Styles) -> bool {
    table.extra_xml.is_empty()
        && table.content_controls.is_empty()
        && table.properties.as_ref().is_none_or(|properties| {
            properties.change.is_none() && properties.revision_xml.is_empty()
        })
        && table.rows.iter().all(|row| {
            row.extra_xml
                .iter()
                .all(|(position, raw)| CT_Row::raw_is_root_attributes(*position, raw))
                && row.content_controls.is_empty()
                // A row the accepted view leaves out has no block row to
                // pair with on a cache hit.
                && !row.accepted_view_removes()
                && row.properties.as_ref().is_none_or(|properties| {
                    properties.revision_markers.is_empty() && properties.revision_xml.is_empty()
                })
                && row.cells.iter().all(|cell| {
                    cell.extra_xml.is_empty()
                        && cell
                            .properties
                            .as_ref()
                            .is_none_or(|properties| properties.extra_xml.is_empty())
                        && cell.content.iter().all(|content| match content {
                            CellContent::Paragraph(paragraph) => {
                                !paragraph_has_note_reference(paragraph)
                                    && paragraph_is_cache_safe(paragraph, styles)
                            }
                            CellContent::Table(table) => table_is_cache_safe(table, styles),
                            CellContent::ContentControl(_) => false,
                        })
                })
        })
}

fn table_semantics(
    table: &CT_Tbl,
    block: &table::TableBlock,
    retained: &TableSemantics,
    sources: Option<&SourceRegistry>,
    story: &WordStory,
    table_path: &[usize],
) -> TableSemantics {
    let rows = table
        .rows
        .iter()
        .zip(&block.rows)
        .zip(&retained.rows)
        .enumerate()
        .map(|(row_index, ((row, block_row), retained_row))| {
            let cells = row
                .cells
                .iter()
                .zip(&block_row.cells)
                .zip(&retained_row.cells)
                .enumerate()
                .map(|(cell_index, ((cell, block_cell), retained_cell))| {
                    let blocks = cell
                        .content
                        .iter()
                        .zip(&block_cell.blocks)
                        .zip(&retained_cell.blocks)
                        .enumerate()
                        .map(|(content_index, ((content, block_item), retained_item))| {
                            let mut source_path = table_path.to_vec();
                            source_path.extend([row_index, cell_index, content_index]);
                            match (content, block_item, retained_item) {
                                (
                                    CellContent::Paragraph(_),
                                    table::CellBlock::Paragraph(_),
                                    CellBlockSemantics::Paragraph(retained),
                                ) => CellBlockSemantics::Paragraph(ParagraphSemantics {
                                    paint: Arc::clone(&retained.paint),
                                    tb_rl_exact_baseline: None,
                                    border_group_before: retained.border_group_before,
                                    source_node: sources
                                        .and_then(|sources| sources.id(story, &source_path)),
                                    structure_id: None,
                                    reflow_direction: retained.reflow_direction,
                                }),
                                (
                                    CellContent::Table(table),
                                    table::CellBlock::Table(block),
                                    CellBlockSemantics::Table(retained),
                                ) => CellBlockSemantics::Table(table_semantics(
                                    table,
                                    block,
                                    retained,
                                    sources,
                                    story,
                                    &source_path,
                                )),
                                _ => unreachable!("cache-safe table topology stays aligned"),
                            }
                        })
                        .collect();
                    block::CellSemantics { blocks }
                })
                .collect();
            block::RowSemantics { cells }
        })
        .collect();
    TableSemantics { rows }
}

fn canonicalize_table_sources(block: &mut table::TableBlock) -> Result<()> {
    for row in &mut block.rows {
        for cell in &mut row.cells {
            for block in &mut cell.blocks {
                match block {
                    table::CellBlock::Paragraph(paragraph) => {
                        rebind_paragraph_source(paragraph, Some(CACHE_SOURCE_NODE))?;
                    }
                    table::CellBlock::Table(table) => canonicalize_table_sources(table)?,
                }
            }
        }
    }
    Ok(())
}

fn font_contribution_trace_bytes(trace: &Vec<FontTraceEvent>) -> usize {
    trace.iter().fold(
        trace
            .capacity()
            .saturating_mul(std::mem::size_of::<FontTraceEvent>()),
        |total, event| {
            total.saturating_add(match event {
                FontTraceEvent::Resolution(_) => 0,
                FontTraceEvent::Contribution(contribution) => contribution
                    .requested_family
                    .as_ref()
                    .map_or(0, String::capacity),
            })
        },
    )
}

fn table_cache_entry_bytes(
    key: &TableCacheKey,
    block: &table::TableBlock,
    semantics: &TableSemantics,
    diagnostics: &[Diagnostic],
    font_trace_bytes: usize,
) -> usize {
    let diagnostic_bytes = diagnostics
        .len()
        .saturating_mul(std::mem::size_of::<Diagnostic>())
        .saturating_add(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.capacity())
                .fold(0usize, usize::saturating_add),
        );
    std::mem::size_of::<TableCacheEntry>()
        .saturating_add(table_key_retained_bytes(&key.table))
        .saturating_add(table_block_retained_bytes(block, Some(semantics)))
        .saturating_add(table_semantics_retained_bytes(semantics))
        .saturating_add(2 * std::mem::size_of::<usize>())
        .saturating_add(font_trace_bytes)
        .saturating_add(diagnostic_bytes)
}

fn table_semantics_retained_bytes(semantics: &TableSemantics) -> usize {
    let mut bytes = semantics
        .rows
        .capacity()
        .saturating_mul(std::mem::size_of::<block::RowSemantics>());
    for row in &semantics.rows {
        bytes = bytes.saturating_add(
            row.cells
                .capacity()
                .saturating_mul(std::mem::size_of::<block::CellSemantics>()),
        );
        for cell in &row.cells {
            bytes = bytes.saturating_add(
                cell.blocks
                    .capacity()
                    .saturating_mul(std::mem::size_of::<CellBlockSemantics>()),
            );
            for block in &cell.blocks {
                match block {
                    CellBlockSemantics::Table(table) => {
                        bytes = bytes.saturating_add(table_semantics_retained_bytes(table))
                    }
                    CellBlockSemantics::Paragraph(paragraph) => {
                        bytes = bytes.saturating_add(word_paint_arc_bytes(&paragraph.paint))
                    }
                }
            }
        }
    }
    bytes
}

/// Heap bytes held by a retained ordered attribute vector.
fn attribute_pairs_bytes(values: &[(String, String)]) -> usize {
    values
        .len()
        .saturating_mul(std::mem::size_of::<(String, String)>())
        .saturating_add(
            values
                .iter()
                .map(|(name, value)| name.capacity().saturating_add(value.capacity()))
                .fold(0usize, usize::saturating_add),
        )
}

fn paragraph_key_retained_bytes(paragraph: &CT_P) -> usize {
    fn option_string_bytes(value: &Option<String>) -> usize {
        value.as_ref().map_or(0, String::capacity)
    }
    fn raw_vectors_bytes(values: &[Vec<u8>]) -> usize {
        values
            .len()
            .saturating_mul(std::mem::size_of::<Vec<u8>>())
            .saturating_add(
                values
                    .iter()
                    .map(Vec::capacity)
                    .fold(0usize, usize::saturating_add),
            )
    }
    fn run_properties_bytes(properties: &CT_RPr) -> usize {
        [
            &properties.style_id,
            &properties.font_ascii,
            &properties.font_hansi,
            &properties.font_east_asia,
            &properties.font_cs,
            &properties.font_ascii_theme,
            &properties.font_hansi_theme,
            &properties.font_east_asia_theme,
            &properties.font_cs_theme,
            &properties.font_hint,
            &properties.color,
            &properties.color_theme,
            &properties.vert_align,
        ]
        .into_iter()
        .map(option_string_bytes)
        .fold(0usize, usize::saturating_add)
        .saturating_add(attribute_pairs_bytes(&properties.font_extra_attributes))
        .saturating_add(attribute_pairs_bytes(&properties.color_extra_attributes))
        .saturating_add(
            properties
                .shading
                .as_ref()
                .map_or(0, |shading| shading_bytes(shading)),
        )
        .saturating_add(
            properties
                .revision_markers
                .capacity()
                .saturating_mul(std::mem::size_of::<CT_Revision>()),
        )
        .saturating_add(raw_vectors_bytes(&properties.revision_xml))
        .saturating_add(
            properties
                .revision_xml_positions
                .capacity()
                .saturating_mul(std::mem::size_of::<(u8, usize)>()),
        )
    }
    fn shading_bytes(shading: &CT_Shd) -> usize {
        shading
            .val
            .capacity()
            .saturating_add(option_string_bytes(&shading.color))
            .saturating_add(option_string_bytes(&shading.fill))
            .saturating_add(option_string_bytes(&shading.theme_color))
            .saturating_add(option_string_bytes(&shading.theme_fill))
            .saturating_add(attribute_pairs_bytes(&shading.extra_attributes))
    }
    fn paragraph_border_bytes(borders: &CT_PBdr) -> usize {
        [
            &borders.top,
            &borders.bottom,
            &borders.left,
            &borders.right,
            &borders.between,
            &borders.bar,
        ]
        .into_iter()
        .filter_map(Option::as_ref)
        .map(|edge| option_string_bytes(&edge.color))
        .fold(0usize, usize::saturating_add)
    }
    fn paragraph_properties_bytes(properties: &CT_PPr) -> usize {
        option_string_bytes(&properties.style_id)
            .saturating_add(option_string_bytes(&properties.line_rule))
            .saturating_add(
                properties
                    .borders
                    .as_ref()
                    .map_or(0, |borders| paragraph_border_bytes(borders)),
            )
            .saturating_add(properties.tabs.as_ref().map_or(0, |tabs| {
                tabs.tabs
                    .capacity()
                    .saturating_mul(std::mem::size_of::<CT_TabStop>())
            }))
            .saturating_add(
                properties
                    .shading
                    .as_ref()
                    .map_or(0, |shading| shading_bytes(shading)),
            )
            .saturating_add(properties.rpr.as_ref().map_or(0, run_properties_bytes))
            .saturating_add(raw_vectors_bytes(&properties.numbering_revision_xml))
            .saturating_add(raw_vectors_bytes(&properties.revision_xml))
    }

    let run_bytes = paragraph
        .runs
        .capacity()
        .saturating_mul(std::mem::size_of::<CT_R>())
        .saturating_add(
            paragraph
                .runs
                .iter()
                .map(|run| {
                    run.content
                        .capacity()
                        .saturating_mul(std::mem::size_of::<RunContent>())
                        .saturating_add(
                            run.content
                                .iter()
                                .map(|content| match content {
                                    RunContent::Text(text) | RunContent::DeletedText(text) => {
                                        text.text.capacity()
                                    }
                                    RunContent::Symbol { font, .. } => font.capacity(),
                                    RunContent::Tab
                                    | RunContent::Break(_)
                                    | RunContent::Drawing(_)
                                    | RunContent::Field(_)
                                    | RunContent::FootnoteRef { .. }
                                    | RunContent::EndnoteRef { .. }
                                    | RunContent::CommentReference { .. }
                                    | RunContent::SpecialCharacter(_) => 0,
                                })
                                .fold(0usize, usize::saturating_add),
                        )
                        .saturating_add(run.properties.as_ref().map_or(0, run_properties_bytes))
                        .saturating_add(raw_vectors_bytes(&run.extra_xml))
                        .saturating_add(
                            run.extra_xml_positions
                                .capacity()
                                .saturating_mul(std::mem::size_of::<usize>()),
                        )
                })
                .fold(0usize, usize::saturating_add),
        );
    let paragraph_vectors =
        paragraph
            .hyperlinks
            .capacity()
            .saturating_mul(std::mem::size_of::<oxoffice_docx_oxml::text::HyperlinkSpan>())
            .saturating_add(paragraph.comment_ranges.capacity().saturating_mul(
                std::mem::size_of::<oxoffice_docx_oxml::text::CommentRangeMarker>(),
            ))
            .saturating_add(
                paragraph
                    .extra_xml
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(usize, Vec<u8>)>()),
            )
            .saturating_add(
                paragraph
                    .extra_xml
                    .iter()
                    .map(|(_, raw)| raw.capacity())
                    .fold(0usize, usize::saturating_add),
            );
    run_bytes.saturating_add(paragraph_vectors).saturating_add(
        paragraph
            .properties
            .as_ref()
            .map_or(0, paragraph_properties_bytes),
    )
}

fn table_key_retained_bytes(table: &CT_Tbl) -> usize {
    fn option_string_bytes(value: &Option<String>) -> usize {
        value.as_ref().map_or(0, String::capacity)
    }
    fn raw_entries_bytes(values: &[(usize, Vec<u8>)], capacity: usize) -> usize {
        capacity
            .saturating_mul(std::mem::size_of::<(usize, Vec<u8>)>())
            .saturating_add(
                values
                    .iter()
                    .map(|(_, raw)| raw.capacity())
                    .fold(0usize, usize::saturating_add),
            )
    }
    fn raw_vectors_bytes(values: &[Vec<u8>]) -> usize {
        values
            .len()
            .saturating_mul(std::mem::size_of::<Vec<u8>>())
            .saturating_add(
                values
                    .iter()
                    .map(Vec::capacity)
                    .fold(0usize, usize::saturating_add),
            )
    }
    fn shading_bytes(shading: &CT_Shd) -> usize {
        shading
            .val
            .capacity()
            .saturating_add(option_string_bytes(&shading.color))
            .saturating_add(option_string_bytes(&shading.fill))
            .saturating_add(option_string_bytes(&shading.theme_color))
            .saturating_add(option_string_bytes(&shading.theme_fill))
            .saturating_add(attribute_pairs_bytes(&shading.extra_attributes))
    }
    fn table_border_bytes(borders: &oxoffice_docx_oxml::table::CT_TblBorders) -> usize {
        [
            &borders.top,
            &borders.bottom,
            &borders.left,
            &borders.right,
            &borders.inside_h,
            &borders.inside_v,
        ]
        .into_iter()
        .filter_map(Option::as_ref)
        .map(|edge| option_string_bytes(&edge.color))
        .fold(0usize, usize::saturating_add)
    }
    fn width_bytes(width: &oxoffice_docx_oxml::table::CT_TblWidth) -> usize {
        width.width_type.capacity()
    }
    fn table_properties_bytes(properties: &oxoffice_docx_oxml::table::CT_TblPr) -> usize {
        option_string_bytes(&properties.style_id)
            .saturating_add(option_string_bytes(&properties.layout))
            .saturating_add(properties.width.as_ref().map_or(0, width_bytes))
            .saturating_add(properties.indent.as_ref().map_or(0, width_bytes))
            .saturating_add(properties.borders.as_ref().map_or(0, table_border_bytes))
            .saturating_add(properties.shading.as_ref().map_or(0, shading_bytes))
            .saturating_add(
                properties
                    .look
                    .as_ref()
                    .map_or(0, |look| option_string_bytes(&look.val)),
            )
            .saturating_add(raw_vectors_bytes(&properties.revision_xml))
    }
    fn row_properties_bytes(properties: &oxoffice_docx_oxml::table::CT_TrPr) -> usize {
        option_string_bytes(&properties.height_rule)
            .saturating_add(option_string_bytes(&properties.cnf_style))
            .saturating_add(
                properties
                    .revision_markers
                    .capacity()
                    .saturating_mul(std::mem::size_of::<CT_Revision>()),
            )
            .saturating_add(raw_vectors_bytes(&properties.revision_xml))
    }
    fn cell_properties_bytes(properties: &oxoffice_docx_oxml::table::CT_TcPr) -> usize {
        properties
            .width
            .as_ref()
            .map_or(0, width_bytes)
            .saturating_add(properties.borders.as_ref().map_or(0, table_border_bytes))
            .saturating_add(properties.shading.as_ref().map_or(0, shading_bytes))
            .saturating_add(option_string_bytes(&properties.text_direction))
            .saturating_add(option_string_bytes(&properties.cnf_style))
            .saturating_add(raw_entries_bytes(
                &properties.extra_xml,
                properties.extra_xml.capacity(),
            ))
    }

    let grid_bytes = table.grid.as_ref().map_or(0, |grid| {
        grid.columns
            .capacity()
            .saturating_mul(std::mem::size_of::<oxoffice_docx_oxml::table::CT_TblGridCol>())
    });
    table
        .rows
        .capacity()
        .saturating_mul(std::mem::size_of::<CT_Row>())
        .saturating_add(grid_bytes)
        .saturating_add(
            table
                .rows
                .iter()
                .map(|row| {
                    row.properties
                        .as_ref()
                        .map_or(0, row_properties_bytes)
                        .saturating_add(raw_entries_bytes(&row.extra_xml, row.extra_xml.capacity()))
                        .saturating_add(
                            row.content_controls
                                .capacity()
                                .saturating_mul(std::mem::size_of::<(usize, usize, CT_Sdt)>()),
                        )
                        .saturating_add(
                            row.cells
                                .capacity()
                                .saturating_mul(std::mem::size_of::<CT_Tc>())
                                .saturating_add(
                                    row.cells
                                        .iter()
                                        .map(|cell| {
                                            cell.properties
                                        .as_ref()
                                        .map_or(0, cell_properties_bytes)
                                        .saturating_add(raw_entries_bytes(
                                            &cell.extra_xml,
                                            cell.extra_xml.capacity(),
                                        ))
                                        .saturating_add(cell.content
                                        .capacity()
                                        .saturating_mul(std::mem::size_of::<CellContent>())
                                        .saturating_add(
                                            cell.content
                                                .iter()
                                                .map(|content| match content {
                                                    CellContent::Paragraph(paragraph) => {
                                                        paragraph_key_retained_bytes(paragraph)
                                                    }
                                                    CellContent::Table(table) => {
                                                        table_key_retained_bytes(table)
                                                    }
                                                    CellContent::ContentControl(_) => usize::MAX,
                                                })
                                                .fold(0usize, usize::saturating_add),
                                        ))
                                        })
                                        .fold(0usize, usize::saturating_add),
                                ),
                        )
                })
                .fold(0usize, usize::saturating_add),
        )
        .saturating_add(table.properties.as_ref().map_or(0, table_properties_bytes))
        .saturating_add(raw_entries_bytes(
            &table.extra_xml,
            table.extra_xml.capacity(),
        ))
        .saturating_add(
            table
                .content_controls
                .capacity()
                .saturating_mul(std::mem::size_of::<(usize, usize, CT_Sdt)>()),
        )
}

fn table_block_retained_bytes(
    block: &table::TableBlock,
    semantics: Option<&TableSemantics>,
) -> usize {
    fn border_bytes(borders: &oxoffice_docx_oxml::table::CT_TblBorders) -> usize {
        [
            &borders.top,
            &borders.bottom,
            &borders.left,
            &borders.right,
            &borders.inside_h,
            &borders.inside_v,
        ]
        .into_iter()
        .map(|edge| {
            edge.as_ref()
                .and_then(|edge| edge.color.as_ref())
                .map_or(0, String::capacity)
        })
        .fold(0usize, usize::saturating_add)
    }

    let rows = block
        .rows
        .iter()
        .enumerate()
        .map(|(row_index, row)| {
            row.cells
                .capacity()
                .saturating_mul(std::mem::size_of::<table::TableCell>())
                .saturating_add(
                    row.cells
                        .iter()
                        .enumerate()
                        .map(|(cell_index, cell)| {
                            cell.blocks
                                .capacity()
                                .saturating_mul(std::mem::size_of::<table::CellBlock>())
                                .saturating_add(
                                    cell.blocks
                                        .iter()
                                        .enumerate()
                                        .map(|(block_index, block)| {
                                            let companion = semantics
                                                .and_then(|semantics| semantics.rows.get(row_index))
                                                .and_then(|row| row.cells.get(cell_index))
                                                .and_then(|cell| cell.blocks.get(block_index));
                                            match block {
                                                table::CellBlock::Paragraph(paragraph) => {
                                                    paragraph_cache_entry_bytes(
                                                        &CT_P::new(),
                                                        paragraph,
                                                        match companion {
                                                            Some(
                                                                CellBlockSemantics::Paragraph(
                                                                    semantics,
                                                                ),
                                                            ) => Some(semantics.paint.as_ref()),
                                                            _ => None,
                                                        },
                                                        &[],
                                                        0,
                                                    )
                                                }
                                                table::CellBlock::Table(table) => {
                                                    table_block_retained_bytes(
                                                        table,
                                                        match companion {
                                                            Some(CellBlockSemantics::Table(
                                                                semantics,
                                                            )) => Some(semantics),
                                                            _ => None,
                                                        },
                                                    )
                                                }
                                            }
                                        })
                                        .fold(0usize, usize::saturating_add),
                                )
                                .saturating_add(cell.borders.as_ref().map_or(0, border_bytes))
                        })
                        .fold(0usize, usize::saturating_add),
                )
        })
        .fold(0usize, usize::saturating_add);
    std::mem::size_of::<table::TableBlock>()
        .saturating_add(
            block
                .col_widths
                .capacity()
                .saturating_mul(std::mem::size_of::<f64>()),
        )
        .saturating_add(
            block
                .rows
                .capacity()
                .saturating_mul(std::mem::size_of::<table::TableRow>()),
        )
        .saturating_add(
            block
                .header_row_indices
                .capacity()
                .saturating_mul(std::mem::size_of::<usize>()),
        )
        .saturating_add(rows)
        .saturating_add(block.borders.as_ref().map_or(0, border_bytes))
}

fn canonicalize_layout_fonts(
    pages: &mut [Arc<PageFrame>],
    font_manager: &FontManager,
    current_fonts: &[FontId],
) -> Result<Vec<oxoffice_layout::FontData>> {
    fn collect(
        elements: &[PositionedElement],
        remap: &mut HashMap<FontId, FontId>,
        order: &mut Vec<FontId>,
    ) {
        for element in elements {
            match element {
                PositionedElement::Text(run) => {
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        remap.entry(run.font_id)
                    {
                        let local = FontId(order.len() as u32);
                        entry.insert(local);
                        order.push(run.font_id);
                    }
                }
                PositionedElement::MultilingualText(run) => {
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        remap.entry(run.font_id)
                    {
                        let local = FontId(order.len() as u32);
                        entry.insert(local);
                        order.push(run.font_id);
                    }
                }
                PositionedElement::Group(group) => collect(&group.children, remap, order),
                PositionedElement::MarkedContent { children, .. } => {
                    collect(children, remap, order)
                }
                _ => {}
            }
        }
    }

    fn rewrite(elements: &mut [PositionedElement], remap: &HashMap<FontId, FontId>) {
        for element in elements {
            match element {
                PositionedElement::Text(run) => {
                    run.font_id = remap[&run.font_id];
                }
                PositionedElement::MultilingualText(run) => {
                    run.font_id = remap[&run.font_id];
                }
                PositionedElement::Group(group) => rewrite(&mut group.children, remap),
                PositionedElement::MarkedContent { children, .. } => rewrite(children, remap),
                _ => {}
            }
        }
    }

    let mut remap = HashMap::new();
    let mut order = Vec::with_capacity(current_fonts.len());
    for &font_id in current_fonts {
        if let std::collections::hash_map::Entry::Vacant(entry) = remap.entry(font_id) {
            let local = FontId(order.len() as u32);
            entry.insert(local);
            order.push(font_id);
        }
    }
    for page in pages.iter() {
        collect(&page.elements, &mut remap, &mut order);
    }
    let mut fonts = Vec::with_capacity(order.len());
    for persistent_id in order {
        let mut font = font_manager.font_data(persistent_id)?;
        font.id = remap[&persistent_id];
        fonts.push(font);
    }
    for page in pages {
        rewrite(&mut Arc::make_mut(page).elements, &remap);
    }
    Ok(fonts)
}

fn restart_block_is_safe<B: LayoutBlockLike>(block: &B) -> bool {
    if let Some(paragraph) = block.paragraph() {
        restart_record_block_is_safe(block)
            && paragraph.lines.iter().all(|line| {
                line.items.iter().all(|item| match item {
                    LineItem::Text(text) | LineItem::Marker(text) => {
                        text.field_kind.is_none() && text.field_source.is_none()
                    }
                    LineItem::MultilingualText(text) => {
                        text.base().field_kind.is_none() && text.base().field_source.is_none()
                    }
                    LineItem::Tab {
                        leader: Some(text), ..
                    } => text.field_kind.is_none() && text.field_source.is_none(),
                    _ => true,
                })
            })
    } else {
        restart_record_block_is_safe(block)
    }
}

fn restart_record_block_is_safe<B: LayoutBlockLike>(block: &B) -> bool {
    if let Some(paragraph) = block.paragraph() {
        paragraph.anchored.is_empty()
            && paragraph.lines.iter().all(|line| {
                line.items.iter().all(|item| match item {
                    LineItem::Text(_) | LineItem::Marker(_) => true,
                    LineItem::MultilingualText(span) => paragraph
                        .paint
                        .or_else(|| {
                            paragraph
                                .semantics
                                .map(|semantics| semantics.paint.as_ref())
                        })
                        .and_then(|paint| paint.legacy_cache.as_ref())
                        .is_some_and(|proof| {
                            multilingual_cache_bytes(span, Some(proof)) != usize::MAX
                        }),
                    LineItem::Tab {
                        leader: Some(_), ..
                    } => true,
                    LineItem::Tab { leader: None, .. } => true,
                    LineItem::Image { .. } | LineItem::Group { .. } => false,
                    _ => false,
                })
            })
    } else {
        block.table().is_some_and(|table| {
            table_block_retained_bytes(table.block, table.semantics) != usize::MAX
        })
    }
}

fn page_has_substitution_state(page: &PageFrame) -> bool {
    let mut found = false;
    oxoffice_layout::walk(&page.elements, &mut |element, _| {
        found |= match element {
            PositionedElement::Text(run) => run.field_kind.is_some() || run.field_source.is_some(),
            PositionedElement::MultilingualText(run) => {
                run.field_kind.is_some() || run.field_source.is_some()
            }
            _ => false,
        };
    });
    found
}

fn restart_cache_entries(cache: &RestartCache) -> usize {
    cache.raw_pages.len().max(cache.checkpoints.len())
}

fn restart_cache_bytes(cache: &RestartCache) -> usize {
    let vector_bytes = cache
        .body
        .capacity()
        .saturating_mul(std::mem::size_of::<RestartBodyEntry>())
        .saturating_add(
            cache
                .body_fragments
                .capacity()
                .saturating_mul(std::mem::size_of::<Vec<WordBodyLayoutFragment>>()),
        )
        .saturating_add(
            cache
                .body_fragments
                .iter()
                .map(|fragments| {
                    fragments
                        .capacity()
                        .saturating_mul(std::mem::size_of::<WordBodyLayoutFragment>())
                })
                .fold(0usize, usize::saturating_add),
        )
        .saturating_add(
            cache
                .raw_pages
                .capacity()
                .saturating_add(cache.pages.capacity())
                .saturating_mul(std::mem::size_of::<Arc<PageFrame>>()),
        )
        .saturating_add(
            cache
                .substitution_inputs
                .capacity()
                .saturating_mul(std::mem::size_of::<Option<FieldSubstitutionInputs>>()),
        )
        .saturating_add(
            cache
                .outlines
                .capacity()
                .saturating_mul(std::mem::size_of::<oxoffice_layout::OutlineEntry>()),
        )
        .saturating_add(
            cache
                .checkpoints
                .capacity()
                .saturating_mul(std::mem::size_of::<paginator::PaginationCheckpoint>()),
        )
        .saturating_add(
            cache
                .font_trace
                .capacity()
                .saturating_mul(std::mem::size_of::<FontId>()),
        );
    let paint_bytes = cache
        .paint
        .capacity()
        .saturating_mul(std::mem::size_of::<RestartPaint>())
        .saturating_add(
            cache
                .paint
                .iter()
                .map(|paint| match paint {
                    RestartPaint::Paragraph(paint) => word_paint_arc_bytes(paint),
                    RestartPaint::Table(semantics) => table_semantics_retained_bytes(semantics),
                })
                .fold(0usize, usize::saturating_add),
        );
    let policy_bytes = [&cache.raw_policies, &cache.policies]
        .iter()
        .map(|maps| {
            maps.capacity()
                .saturating_mul(std::mem::size_of::<PaintPolicies>())
                .saturating_add(
                    maps.iter()
                        .map(|map| {
                            map.retained_bytes()
                                .and_then(|bytes| {
                                    bytes.checked_sub(std::mem::size_of::<PaintPolicies>())
                                })
                                .unwrap_or(usize::MAX)
                        })
                        .fold(0usize, usize::saturating_add),
                )
        })
        .fold(0usize, usize::saturating_add);
    let body_bytes = paint_bytes.saturating_add(policy_bytes).saturating_add(
        cache
            .body
            .iter()
            .map(RestartBodyEntry::bytes)
            .fold(0usize, usize::saturating_add),
    );
    debug_assert_eq!(cache.raw_pages.len(), cache.pages.len());
    let page_bytes = cache
        .raw_pages
        .iter()
        .zip(&cache.pages)
        .map(|(pristine, substituted)| {
            let substituted_bytes = if Arc::ptr_eq(pristine, substituted) {
                0
            } else {
                page_frame_retained_bytes(substituted)
            };
            page_frame_retained_bytes(pristine)
                .saturating_add(2 * std::mem::size_of::<usize>())
                .saturating_add(substituted_bytes)
                .saturating_add(if Arc::ptr_eq(pristine, substituted) {
                    0
                } else {
                    2 * std::mem::size_of::<usize>()
                })
        })
        .fold(0usize, usize::saturating_add);
    let outline_bytes = cache
        .outlines
        .iter()
        .map(|outline| outline.title.capacity())
        .fold(0usize, usize::saturating_add);
    let substitution_bytes = cache
        .substitution_inputs
        .iter()
        .filter_map(Option::as_ref)
        .map(|inputs| {
            inputs
                .bookmark_pages
                .capacity()
                .saturating_mul(std::mem::size_of::<(usize, usize)>())
                .saturating_add(
                    inputs
                        .font_identity
                        .capacity()
                        .saturating_mul(std::mem::size_of::<FontId>()),
                )
                .saturating_add(inputs.page_format_context.capacity())
        })
        .fold(0usize, usize::saturating_add);
    std::mem::size_of::<RestartCache>()
        .saturating_add(vector_bytes)
        .saturating_add(body_bytes)
        .saturating_add(page_bytes)
        .saturating_add(substitution_bytes)
        .saturating_add(outline_bytes)
}

fn page_frame_retained_bytes(page: &PageFrame) -> usize {
    fn glyph_bytes(run: &GlyphRun) -> usize {
        run.text
            .capacity()
            .saturating_add(
                run.glyph_ids
                    .capacity()
                    .saturating_mul(std::mem::size_of::<u16>()),
            )
            .saturating_add(
                run.advances
                    .capacity()
                    .saturating_mul(std::mem::size_of::<f64>()),
            )
    }

    fn multilingual_glyph_bytes(run: &oxoffice_layout::MultilingualGlyphRun) -> usize {
        run.logical_text
            .capacity()
            .saturating_add(run.language.as_ref().map_or(0, String::capacity))
            .saturating_add(
                run.glyph_ids
                    .capacity()
                    .saturating_mul(std::mem::size_of::<u16>()),
            )
            .saturating_add(
                [
                    run.x_advances.capacity(),
                    run.y_advances.capacity(),
                    run.x_offsets.capacity(),
                    run.y_offsets.capacity(),
                ]
                .into_iter()
                .sum::<usize>()
                .saturating_mul(std::mem::size_of::<f64>()),
            )
            .saturating_add(
                run.clusters
                    .capacity()
                    .saturating_mul(std::mem::size_of::<oxoffice_layout::GlyphCluster>()),
            )
    }

    fn element_bytes(element: &PositionedElement) -> usize {
        match element {
            PositionedElement::Text(run) => glyph_bytes(run),
            PositionedElement::MultilingualText(run) => multilingual_glyph_bytes(run),
            PositionedElement::Image {
                data, content_type, ..
            } => data.capacity().saturating_add(content_type.capacity()),
            PositionedElement::LinkAnnotation { url, .. } => url.capacity(),
            PositionedElement::Group(group) => group
                .children
                .capacity()
                .saturating_mul(std::mem::size_of::<PositionedElement>())
                .saturating_add(
                    group
                        .children
                        .iter()
                        .map(element_bytes)
                        .fold(0usize, usize::saturating_add),
                )
                .saturating_add(format!("{:?}", group.effects).len()),
            PositionedElement::MarkedContent { children, .. } => children
                .capacity()
                .saturating_mul(std::mem::size_of::<PositionedElement>())
                .saturating_add(
                    children
                        .iter()
                        .map(element_bytes)
                        .fold(0usize, usize::saturating_add),
                ),
            PositionedElement::Path(path) => format!("{path:?}").len(),
            _ => 0,
        }
    }

    std::mem::size_of::<PageFrame>()
        .saturating_add(
            page.elements
                .capacity()
                .saturating_mul(std::mem::size_of::<PositionedElement>()),
        )
        .saturating_add(
            page.elements
                .iter()
                .map(element_bytes)
                .fold(0usize, usize::saturating_add),
        )
        .saturating_add(format!("{:?}", page.background).len())
}

fn rebind_text_source(text: &mut TextSegment, source_node: Option<SourceNodeId>) {
    match (text.source.as_mut(), source_node) {
        (Some(source), Some(node)) => source.node = node,
        (Some(_), None) => text.source = None,
        (None, _) => {}
    }
    match (text.note_reference_source.as_mut(), source_node) {
        (Some(source), Some(node)) => source.node = node,
        (Some(_), None) => text.note_reference_source = None,
        (None, _) => {}
    }
    match (text.field_source.as_mut(), source_node) {
        (Some(field_source), Some(node)) => field_source.node = node,
        (Some(_), None) => text.field_source = None,
        (None, _) => {}
    }
}

fn rebind_multilingual_source(
    text: &mut oxoffice_layout::MultilingualTextSegment,
    source_node: Option<SourceNodeId>,
) -> Result<()> {
    let mut base = text.base().clone();
    rebind_text_source(&mut base, source_node);
    *text = text
        .clone()
        .with_base_and_advances(base, text.x_advances().to_vec())?;
    Ok(())
}

fn rebind_positioned_sources(elements: &mut [PositionedElement], node: Option<SourceNodeId>) {
    for element in elements {
        match element {
            PositionedElement::Text(run) => {
                if let Some(source) = &mut run.source {
                    if let Some(node) = node {
                        source.node = node;
                    } else {
                        run.source = None;
                    }
                }
                if let Some(source) = &mut run.field_source {
                    if let Some(node) = node {
                        source.node = node;
                    } else {
                        run.field_source = None;
                    }
                }
                if let Some(source) = &mut run.note_reference_source {
                    if let Some(node) = node {
                        source.node = node;
                    } else {
                        run.note_reference_source = None;
                    }
                }
            }
            PositionedElement::MultilingualText(run) => {
                if let Some(source) = &mut run.source {
                    if let Some(node) = node {
                        source.node = node;
                    } else {
                        run.source = None;
                    }
                }
                if let Some(source) = &mut run.field_source {
                    if let Some(node) = node {
                        source.node = node;
                    } else {
                        run.field_source = None;
                    }
                }
                if let Some(source) = &mut run.note_reference_source {
                    if let Some(node) = node {
                        source.node = node;
                    } else {
                        run.note_reference_source = None;
                    }
                }
            }
            PositionedElement::Group(group) => rebind_positioned_sources(&mut group.children, node),
            PositionedElement::MarkedContent { children, .. } => {
                rebind_positioned_sources(children, node)
            }
            _ => {}
        }
    }
}

fn rebind_paragraph_source(
    block: &mut ParagraphBlock,
    source_node: Option<SourceNodeId>,
) -> Result<()> {
    for line in &mut block.lines {
        for item in &mut line.items {
            match item {
                LineItem::Text(text) | LineItem::Marker(text) => {
                    rebind_text_source(text, source_node)
                }
                LineItem::MultilingualText(text) => rebind_multilingual_source(text, source_node)?,
                LineItem::Tab {
                    leader: Some(leader),
                    ..
                } => rebind_text_source(leader, source_node),
                LineItem::Group { group, .. } => {
                    rebind_positioned_sources(&mut group.children, source_node)
                }
                _ => {}
            }
        }
    }
    if let Some(reflow) = block.reflow.as_mut() {
        for item in &mut reflow.items {
            match item {
                InlineItem::Text(text) | InlineItem::Marker(text) => {
                    rebind_text_source(text, source_node)
                }
                InlineItem::MultilingualText(text) => {
                    rebind_multilingual_source(text, source_node)?
                }
                InlineItem::HyphenatedText { segment, .. } => {
                    rebind_text_source(segment, source_node)
                }
                InlineItem::Group { group, .. } => {
                    rebind_positioned_sources(&mut group.children, source_node)
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn remap_text_box_sources(
    block: &mut ParagraphBlock,
    nodes: &HashMap<SourceNodeId, SourceNodeId>,
) -> Result<()> {
    fn node(
        node: SourceNodeId,
        nodes: &HashMap<SourceNodeId, SourceNodeId>,
    ) -> Result<SourceNodeId> {
        nodes.get(&node).copied().ok_or_else(|| {
            LayoutError::Layout("rich text-box source identity escaped its physical body".into())
        })
    }
    fn text(text: &mut TextSegment, nodes: &HashMap<SourceNodeId, SourceNodeId>) -> Result<()> {
        if let Some(source) = &mut text.source {
            source.node = node(source.node, nodes)?;
        }
        if let Some(source) = &mut text.note_reference_source {
            source.node = node(source.node, nodes)?;
        }
        if let Some(source) = &mut text.field_source {
            source.node = node(source.node, nodes)?;
        }
        Ok(())
    }
    fn multilingual(
        value: &mut oxoffice_layout::MultilingualTextSegment,
        nodes: &HashMap<SourceNodeId, SourceNodeId>,
    ) -> Result<()> {
        let mut base = value.base().clone();
        text(&mut base, nodes)?;
        *value = value
            .clone()
            .with_base_and_advances(base, value.x_advances().to_vec())?;
        Ok(())
    }
    fn remap_elements(
        elements: &mut [PositionedElement],
        nodes: &HashMap<SourceNodeId, SourceNodeId>,
    ) -> Result<()> {
        for element in elements {
            match element {
                PositionedElement::Text(run) => {
                    if let Some(source) = &mut run.source {
                        source.node = node(source.node, nodes)?;
                    }
                    if let Some(source) = &mut run.note_reference_source {
                        source.node = node(source.node, nodes)?;
                    }
                    if let Some(source) = &mut run.field_source {
                        source.node = node(source.node, nodes)?;
                    }
                }
                PositionedElement::MultilingualText(run) => {
                    if let Some(source) = &mut run.source {
                        source.node = node(source.node, nodes)?;
                    }
                    if let Some(source) = &mut run.note_reference_source {
                        source.node = node(source.node, nodes)?;
                    }
                    if let Some(source) = &mut run.field_source {
                        source.node = node(source.node, nodes)?;
                    }
                }
                PositionedElement::Group(group) => remap_elements(&mut group.children, nodes)?,
                PositionedElement::MarkedContent { children, .. } => {
                    remap_elements(children, nodes)?
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn line(item: &mut LineItem, nodes: &HashMap<SourceNodeId, SourceNodeId>) -> Result<()> {
        match item {
            LineItem::Text(value) | LineItem::Marker(value) => text(value, nodes)?,
            LineItem::MultilingualText(value) => multilingual(value, nodes)?,
            LineItem::Tab {
                leader: Some(value),
                ..
            } => text(value, nodes)?,
            LineItem::Group { group, .. } => remap_elements(&mut group.children, nodes)?,
            LineItem::Figure { item, .. } => line(item, nodes)?,
            _ => {}
        }
        Ok(())
    }
    fn inline(item: &mut InlineItem, nodes: &HashMap<SourceNodeId, SourceNodeId>) -> Result<()> {
        match item {
            InlineItem::Text(value)
            | InlineItem::Marker(value)
            | InlineItem::HyphenatedText { segment: value, .. } => text(value, nodes)?,
            InlineItem::MultilingualText(value) => multilingual(value, nodes)?,
            InlineItem::Group { group, .. } => remap_elements(&mut group.children, nodes)?,
            InlineItem::Figure { item, .. } => inline(item, nodes)?,
            _ => {}
        }
        Ok(())
    }
    for value in &mut block.lines {
        for item in &mut value.items {
            line(item, nodes)?;
        }
    }
    if let Some(reflow) = &mut block.reflow {
        for item in &mut reflow.items {
            inline(item, nodes)?;
        }
    }
    for drawing in &mut block.anchored {
        match &mut drawing.content {
            block::AnchoredContent::Shape { text, .. } => {
                for paragraph in text {
                    remap_text_box_sources(paragraph, nodes)?;
                }
            }
            block::AnchoredContent::Group(group) => remap_elements(&mut group.children, nodes)?,
            block::AnchoredContent::Image { .. } => {}
        }
    }
    Ok(())
}

fn rebind_header_footer_sources(
    story_kind: HeaderFooterStoryKind,
    relationship_id: &str,
    part: &oxoffice_docx_oxml::header_footer::CT_HdrFtr,
    blocks: &mut [ParagraphBlock],
    facts: &mut [paginator::HeaderFooterBlockSemantics],
    sources: Option<&SourceRegistry>,
) -> Result<()> {
    let story = match story_kind {
        HeaderFooterStoryKind::Header => WordStory::Header {
            relationship_id: relationship_id.to_owned(),
        },
        HeaderFooterStoryKind::Footer => WordStory::Footer {
            relationship_id: relationship_id.to_owned(),
        },
    };
    if blocks.len() != facts.len() {
        return Err(LayoutError::Layout(
            "furniture source paint count mismatch".into(),
        ));
    }
    for (paragraph_index, (block, facts)) in blocks.iter_mut().zip(facts).enumerate() {
        debug_assert!(paragraph_index < part.paragraphs.len());
        let source = sources.and_then(|sources| sources.id(&story, &[paragraph_index]));
        rebind_paragraph_source(block, source)?;
        let paint = Arc::make_mut(&mut facts.paint);
        paint.rebind_source(source)?;
        if let Some(registry) = sources {
            registry.bind_text_boxes(block, paint, source)?;
        }
    }
    Ok(())
}

fn picture_draw_owner_bytes(owners: &Vec<(String, Option<String>)>) -> usize {
    owners
        .capacity()
        .saturating_mul(std::mem::size_of::<(String, Option<String>)>())
        .saturating_add(
            owners
                .iter()
                .map(|(id, story)| {
                    id.capacity()
                        .saturating_add(story.as_ref().map_or(0, String::capacity))
                })
                .fold(0, usize::saturating_add),
        )
}

fn header_footer_cache_entry_bytes(
    key: &HeaderFooterCacheKey,
    content: &HeaderFooterVariantContent,
    diagnostics: &Vec<Diagnostic>,
    font_trace: &Vec<FontTraceEvent>,
) -> usize {
    let block_bytes = key
        .part
        .paragraphs
        .iter()
        .zip(&content.blocks)
        .zip(&content.directions)
        .map(|((paragraph, block), facts)| {
            paragraph_cache_entry_bytes(paragraph, block, Some(&facts.paint), &[], 0)
        })
        .fold(0usize, usize::saturating_add);
    let direction_bytes = content
        .directions
        .iter()
        .map(paginator::HeaderFooterBlockSemantics::retained_bytes)
        .fold(0usize, usize::saturating_add)
        .saturating_add(
            content
                .directions
                .capacity()
                .saturating_sub(content.directions.len())
                .saturating_mul(std::mem::size_of::<paginator::HeaderFooterBlockSemantics>()),
        );
    let diagnostic_bytes = diagnostics
        .capacity()
        .saturating_mul(std::mem::size_of::<Diagnostic>())
        .saturating_add(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.capacity())
                .fold(0usize, usize::saturating_add),
        );
    let watermark_bytes = content.watermark.as_ref().map_or(0, |watermark| {
        page_frame_retained_bytes(&PageFrame::new(
            1,
            0.0,
            0.0,
            vec![PositionedElement::Group(watermark.clone())],
        ))
    });
    let section_capacity = key
        .section
        .header_refs
        .capacity()
        .saturating_mul(std::mem::size_of::<
            oxoffice_docx_oxml::header_footer::HdrFtrRef,
        >())
        .saturating_add(
            key.section
                .footer_refs
                .capacity()
                .saturating_mul(std::mem::size_of::<
                    oxoffice_docx_oxml::header_footer::HdrFtrRef,
                >()),
        )
        .saturating_add(key.section.columns.as_ref().map_or(0, |columns| {
            columns
                .columns
                .capacity()
                .saturating_mul(std::mem::size_of::<oxoffice_docx_oxml::document::CT_Column>())
        }))
        .saturating_add(
            key.section
                .extra_xml
                .capacity()
                .saturating_mul(std::mem::size_of::<Vec<u8>>()),
        )
        .saturating_add(
            key.section
                .header_refs
                .iter()
                .chain(&key.section.footer_refs)
                .map(|reference| reference.rel_id.capacity())
                .fold(0usize, usize::saturating_add),
        )
        .saturating_add(
            key.section
                .extra_xml
                .iter()
                .map(Vec::capacity)
                .fold(0usize, usize::saturating_add),
        )
        .saturating_add(section_child_retained_bytes(&key.section));
    let paragraph_raw_capacity = key
        .part
        .paragraphs
        .iter()
        .map(|paragraph| {
            paragraph
                .extra_xml
                .capacity()
                .saturating_mul(std::mem::size_of::<(usize, Vec<u8>)>())
                .saturating_add(
                    paragraph
                        .extra_xml
                        .iter()
                        .map(|(_, raw)| raw.capacity())
                        .fold(0usize, usize::saturating_add),
                )
                .saturating_add(
                    paragraph
                        .runs
                        .iter()
                        .map(|run| {
                            run.extra_xml
                                .capacity()
                                .saturating_mul(std::mem::size_of::<Vec<u8>>())
                                .saturating_add(
                                    run.extra_xml
                                        .iter()
                                        .map(Vec::capacity)
                                        .fold(0usize, usize::saturating_add),
                                )
                                .saturating_add(
                                    run.extra_xml_positions
                                        .capacity()
                                        .saturating_mul(std::mem::size_of::<usize>()),
                                )
                        })
                        .fold(0usize, usize::saturating_add),
                )
        })
        .fold(0usize, usize::saturating_add);
    let watermark_capacity = key
        .part
        .watermarks()
        .iter()
        .map(|watermark| {
            std::mem::size_of::<VmlWatermark>().saturating_add(match watermark {
                VmlWatermark::Text {
                    text,
                    color,
                    font_family,
                    ..
                } => text
                    .capacity()
                    .saturating_add(color.capacity())
                    .saturating_add(font_family.as_ref().map_or(0, String::capacity)),
                VmlWatermark::Image {
                    relationship_id, ..
                } => relationship_id.capacity(),
            })
        })
        .fold(0usize, usize::saturating_add);
    let part_capacity = key
        .part
        .paragraphs
        .capacity()
        .saturating_mul(std::mem::size_of::<CT_P>())
        .saturating_add(
            content
                .blocks
                .capacity()
                .saturating_mul(std::mem::size_of::<ParagraphBlock>()),
        )
        .saturating_add(
            key.part
                .extra_namespaces
                .capacity()
                .saturating_mul(std::mem::size_of::<(String, String)>()),
        )
        .saturating_add(
            key.part
                .extra_xml
                .capacity()
                .saturating_mul(std::mem::size_of::<Vec<u8>>()),
        )
        .saturating_add(
            key.part
                .extra_namespaces
                .iter()
                .map(|(prefix, namespace)| prefix.capacity().saturating_add(namespace.capacity()))
                .fold(0usize, usize::saturating_add),
        )
        .saturating_add(
            key.part
                .extra_xml
                .iter()
                .map(Vec::capacity)
                .fold(0usize, usize::saturating_add),
        );
    std::mem::size_of::<HeaderFooterCacheEntry>()
        .saturating_add(key.relationship_id.capacity())
        .saturating_add(key.resolved_part_bytes.capacity())
        .saturating_add(format!("{:?}", key.section).len())
        .saturating_add(format!("{:?}", key.part).len())
        .saturating_add(key.rich_body.as_ref().map_or(0, |body| {
            format!("{body:?}")
                .len()
                .saturating_add(
                    body.content
                        .capacity()
                        .saturating_mul(std::mem::size_of::<BodyContent>()),
                )
                .saturating_add(
                    body.content
                        .iter()
                        .map(|content| match content {
                            BodyContent::Paragraph(paragraph) => {
                                paragraph_key_retained_bytes(paragraph)
                            }
                            _ => 0,
                        })
                        .fold(0usize, usize::saturating_add),
                )
        }))
        .saturating_add(key.source_part_name.as_ref().map_or(0, String::capacity))
        .saturating_add(section_capacity)
        .saturating_add(part_capacity)
        .saturating_add(paragraph_raw_capacity)
        .saturating_add(watermark_capacity)
        .saturating_add(block_bytes)
        .saturating_add(direction_bytes)
        .saturating_add(watermark_bytes)
        .saturating_add(diagnostic_bytes)
        .saturating_add(font_contribution_trace_bytes(font_trace))
}

fn word_paint_arc_bytes(paint: &WordParagraphPaint) -> usize {
    paint
        .retained_bytes()
        .and_then(|bytes| bytes.checked_add(2 * std::mem::size_of::<usize>()))
        .unwrap_or(usize::MAX)
}

fn multilingual_cache_bytes(
    span: &oxoffice_layout::MultilingualTextSegment,
    proof: Option<&WordLegacyCache>,
) -> usize {
    let Some(proof) = proof else {
        return usize::MAX;
    };
    if !span.uses_legacy_shaping() || span.kerning_override().is_none() {
        return usize::MAX;
    }
    let base = span.base();
    let bytes = (|| {
        let mut bytes = base
            .text
            .capacity()
            .checked_add(
                base.glyph_ids
                    .capacity()
                    .checked_mul(std::mem::size_of::<u16>())?,
            )?
            .checked_add(
                base.advances
                    .capacity()
                    .checked_mul(std::mem::size_of::<f64>())?,
            )?
            .checked_add(base.hyperlink_url.as_ref().map_or(0, String::capacity))?;
        for values in [
            span.x_advances(),
            span.y_advances(),
            span.x_offsets(),
            span.y_offsets(),
        ] {
            bytes = bytes.checked_add(values.len().checked_mul(std::mem::size_of::<f64>())?)?;
        }
        bytes
            .checked_add(
                span.clusters()
                    .len()
                    .checked_mul(std::mem::size_of::<oxoffice_layout::GlyphCluster>())?,
            )?
            .checked_add(span.language().map_or(0, str::len))?
            .checked_add(proof.language_bytes_per_span)
    })();
    bytes.unwrap_or(usize::MAX)
}

fn paragraph_cache_entry_bytes(
    paragraph: &CT_P,
    block: &ParagraphBlock,
    paint: Option<&WordParagraphPaint>,
    diagnostics: &[Diagnostic],
    font_trace_bytes: usize,
) -> usize {
    fn option_string_bytes(value: &Option<String>) -> usize {
        value.as_ref().map_or(0, String::capacity)
    }
    fn shading_bytes(shading: &CT_Shd) -> usize {
        shading
            .val
            .capacity()
            .saturating_add(option_string_bytes(&shading.color))
            .saturating_add(option_string_bytes(&shading.fill))
            .saturating_add(option_string_bytes(&shading.theme_color))
            .saturating_add(option_string_bytes(&shading.theme_fill))
            .saturating_add(attribute_pairs_bytes(&shading.extra_attributes))
    }
    fn run_properties_bytes(properties: &CT_RPr) -> usize {
        [
            &properties.style_id,
            &properties.font_ascii,
            &properties.font_hansi,
            &properties.font_east_asia,
            &properties.font_cs,
            &properties.font_ascii_theme,
            &properties.font_hansi_theme,
            &properties.font_east_asia_theme,
            &properties.font_cs_theme,
            &properties.font_hint,
            &properties.color,
            &properties.color_theme,
            &properties.vert_align,
        ]
        .into_iter()
        .map(option_string_bytes)
        .fold(0usize, usize::saturating_add)
        .saturating_add(attribute_pairs_bytes(&properties.font_extra_attributes))
        .saturating_add(attribute_pairs_bytes(&properties.color_extra_attributes))
        .saturating_add(
            properties
                .shading
                .as_ref()
                .map_or(0, |shading| shading_bytes(shading)),
        )
    }
    fn border_bytes(borders: &CT_PBdr) -> usize {
        [
            &borders.top,
            &borders.bottom,
            &borders.left,
            &borders.right,
            &borders.between,
            &borders.bar,
        ]
        .into_iter()
        .map(|edge| {
            edge.as_ref()
                .and_then(|edge| edge.color.as_ref())
                .map_or(0, String::capacity)
        })
        .fold(0usize, usize::saturating_add)
    }
    fn paragraph_properties_bytes(properties: &CT_PPr) -> usize {
        option_string_bytes(&properties.style_id)
            .saturating_add(option_string_bytes(&properties.line_rule))
            .saturating_add(
                properties
                    .borders
                    .as_ref()
                    .map_or(0, |borders| border_bytes(borders)),
            )
            .saturating_add(properties.tabs.as_ref().map_or(0, |tabs| {
                tabs.tabs
                    .capacity()
                    .saturating_mul(std::mem::size_of::<CT_TabStop>())
            }))
            .saturating_add(
                properties
                    .shading
                    .as_ref()
                    .map_or(0, |shading| shading_bytes(shading)),
            )
            .saturating_add(properties.rpr.as_ref().map_or(0, run_properties_bytes))
    }
    fn paragraph_key_bytes(paragraph: &CT_P) -> usize {
        paragraph
            .runs
            .capacity()
            .saturating_mul(std::mem::size_of::<CT_R>())
            .saturating_add(
                paragraph
                    .runs
                    .iter()
                    .map(|run| {
                        run.content
                            .capacity()
                            .saturating_mul(std::mem::size_of::<RunContent>())
                            .saturating_add(
                                run.content
                                    .iter()
                                    .map(|content| match content {
                                        RunContent::Text(text) => text.text.capacity(),
                                        _ => 0,
                                    })
                                    .fold(0usize, usize::saturating_add),
                            )
                            .saturating_add(run.properties.as_ref().map_or(0, run_properties_bytes))
                    })
                    .fold(0usize, usize::saturating_add),
            )
            .saturating_add(
                paragraph
                    .properties
                    .as_ref()
                    .map_or(0, paragraph_properties_bytes),
            )
    }
    fn text_bytes(text: &TextSegment) -> usize {
        text.text.capacity()
            + text.glyph_ids.capacity() * std::mem::size_of::<u16>()
            + text.advances.capacity() * std::mem::size_of::<f64>()
            + text.hyperlink_url.as_ref().map_or(0, String::capacity)
    }
    fn inline_bytes(item: &InlineItem, proof: Option<&WordLegacyCache>) -> usize {
        match item {
            InlineItem::Text(text) | InlineItem::Marker(text) => text_bytes(text),
            InlineItem::MultilingualText(span) => multilingual_cache_bytes(span, proof),
            InlineItem::Group { .. } => usize::MAX,
            _ => 0,
        }
    }
    fn line_item_bytes(item: &LineItem, proof: Option<&WordLegacyCache>) -> usize {
        match item {
            LineItem::Text(text) | LineItem::Marker(text) => text_bytes(text),
            LineItem::MultilingualText(span) => multilingual_cache_bytes(span, proof),
            LineItem::Tab { leader, .. } => leader.as_ref().map_or(0, text_bytes),
            LineItem::Group { .. } => usize::MAX,
            _ => 0,
        }
    }

    let proof = paint.and_then(|paint| paint.legacy_cache.as_ref());
    let paragraph_bytes = paragraph_key_bytes(paragraph);
    let line_bytes = block
        .lines
        .capacity()
        .saturating_mul(std::mem::size_of::<oxoffice_layout::LayoutLine>())
        .saturating_add(
            block
                .lines
                .iter()
                .map(|line| {
                    line.items
                        .capacity()
                        .saturating_mul(std::mem::size_of::<LineItem>())
                        .saturating_add(
                            line.items
                                .iter()
                                .map(|item| line_item_bytes(item, proof))
                                .fold(0usize, usize::saturating_add),
                        )
                })
                .fold(0usize, usize::saturating_add),
        );
    let reflow_bytes = block.reflow.as_ref().map_or(0, |reflow| {
        std::mem::size_of_val(reflow.as_ref())
            .saturating_add(
                reflow
                    .items
                    .capacity()
                    .saturating_mul(std::mem::size_of::<InlineItem>()),
            )
            .saturating_add(
                reflow
                    .items
                    .iter()
                    .map(|item| inline_bytes(item, proof))
                    .fold(0usize, usize::saturating_add),
            )
            .saturating_add(
                reflow
                    .params
                    .tab_stops
                    .capacity()
                    .saturating_mul(std::mem::size_of::<oxoffice_layout::TabStop>()),
            )
            .saturating_add(
                reflow
                    .params
                    .line_prefix_widths
                    .capacity()
                    .saturating_mul(std::mem::size_of::<f64>()),
            )
            .saturating_add(
                reflow
                    .params
                    .line_suffix_widths
                    .capacity()
                    .saturating_mul(std::mem::size_of::<f64>()),
            )
    });
    let diagnostic_bytes = diagnostics
        .len()
        .saturating_mul(std::mem::size_of::<Diagnostic>())
        .saturating_add(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.capacity())
                .fold(0usize, usize::saturating_add),
        );
    std::mem::size_of::<ParagraphCacheEntry>()
        .saturating_add(arc_allocation_bytes::<ParagraphBlock>())
        .saturating_add(paint.map_or(0, word_paint_arc_bytes))
        .saturating_add(paragraph_bytes)
        .saturating_add(line_bytes)
        .saturating_add(reflow_bytes)
        .saturating_add(if block.anchored.is_empty() {
            0
        } else {
            usize::MAX
        })
        .saturating_add(block.heading_text.as_ref().map_or(0, String::capacity))
        .saturating_add(block.borders.as_ref().map_or(0, border_bytes))
        .saturating_add(font_trace_bytes)
        .saturating_add(diagnostic_bytes)
}

struct StableFingerprint(u64);

impl StableFingerprint {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write_bytes(&mut self, value: &[u8]) {
        for byte in value {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn write_usize(&mut self, value: usize) {
        self.write_bytes(&value.to_le_bytes());
    }

    fn write_tag(&mut self, value: u8) {
        self.write_bytes(&[value]);
    }
}

fn paragraph_fingerprint(paragraph: &CT_P) -> u64 {
    let mut fingerprint = StableFingerprint::new();
    fingerprint.write_usize(paragraph.runs.len());
    fingerprint.write_tag(u8::from(paragraph.properties.is_some()));
    // A ruby annotation changes the painted page without changing any run,
    // so a paragraph that carries one must not collide with the same runs
    // unannotated. Nothing is written when there is none, which keeps every
    // existing fingerprint where it was.
    for ruby in &paragraph.rubies {
        fingerprint.write_tag(7);
        fingerprint.write_usize(ruby.base_start);
        fingerprint.write_usize(ruby.base_end);
        for run in &ruby.ruby_text {
            fingerprint.write_bytes(run.text().as_bytes());
        }
    }
    for run in &paragraph.runs {
        fingerprint.write_tag(u8::from(run.properties.is_some()));
        fingerprint.write_usize(run.content.len());
        for content in &run.content {
            match content {
                RunContent::Text(text) => {
                    fingerprint.write_tag(0);
                    fingerprint.write_bytes(text.text.as_bytes());
                    fingerprint.write_tag(u8::from(text.preserve_space));
                }
                RunContent::DeletedText(text) => {
                    fingerprint.write_tag(1);
                    fingerprint.write_bytes(text.text.as_bytes());
                    fingerprint.write_tag(u8::from(text.preserve_space));
                }
                RunContent::Tab => fingerprint.write_tag(2),
                RunContent::Break(kind) => {
                    fingerprint.write_tag(3);
                    fingerprint.write_tag(match kind {
                        BreakType::Line => 0,
                        BreakType::Page => 1,
                        BreakType::Column => 2,
                    });
                }
                RunContent::Symbol { font, char_code } => {
                    fingerprint.write_tag(5);
                    fingerprint.write_bytes(font.as_bytes());
                    fingerprint.write_usize(usize::from(*char_code));
                }
                RunContent::SpecialCharacter(character) => {
                    fingerprint.write_tag(6);
                    fingerprint.write_tag(match character {
                        SpecialCharacter::CarriageReturn => 0,
                        SpecialCharacter::NoBreakHyphen => 1,
                        SpecialCharacter::SoftHyphen => 2,
                        SpecialCharacter::PositionalTab { .. } => 3,
                    });
                }
                RunContent::Drawing(_)
                | RunContent::Field(_)
                | RunContent::FootnoteRef { .. }
                | RunContent::EndnoteRef { .. }
                | RunContent::CommentReference { .. } => fingerprint.write_tag(4),
            }
        }
    }
    fingerprint.0
}

fn table_fingerprint(table: &CT_Tbl) -> u64 {
    fn write_table(table: &CT_Tbl, fingerprint: &mut StableFingerprint) {
        fingerprint.write_tag(u8::from(table.properties.is_some()));
        fingerprint.write_usize(table.grid.as_ref().map_or(0, |grid| grid.columns.len()));
        fingerprint.write_usize(table.rows.len());
        for row in &table.rows {
            fingerprint.write_tag(u8::from(row.properties.is_some()));
            fingerprint.write_usize(row.cells.len());
            for cell in &row.cells {
                fingerprint.write_tag(u8::from(cell.properties.is_some()));
                fingerprint.write_usize(cell.content.len());
                for content in &cell.content {
                    match content {
                        CellContent::Paragraph(paragraph) => {
                            fingerprint.write_tag(0);
                            fingerprint
                                .write_bytes(&paragraph_fingerprint(paragraph).to_le_bytes());
                        }
                        CellContent::Table(table) => {
                            fingerprint.write_tag(1);
                            write_table(table, fingerprint);
                        }
                        CellContent::ContentControl(_) => fingerprint.write_tag(2),
                    }
                }
            }
        }
    }

    let mut fingerprint = StableFingerprint::new();
    write_table(table, &mut fingerprint);
    fingerprint.0
}

/// Apply page background color from `w:background` element to all pages.
fn apply_page_background(
    pages: &mut [PageFrame],
    paint: &mut paginator::PagePaintRecords,
    input: &LayoutInput,
) -> Result<()> {
    let bg_xml = match &input.document.background_xml {
        Some(xml) => xml,
        None => return Ok(()),
    };

    // Parse w:color attribute from background XML
    let xml_str = std::str::from_utf8(bg_xml).unwrap_or("");
    let color = extract_background_color(xml_str);
    let color = match color {
        Some(c) => c,
        None => return Ok(()),
    };

    // Insert a full-page FilledRect at position 0 on every page (renders underneath everything)
    paint.validate(pages)?;
    for (index, page) in pages.iter_mut().enumerate() {
        let mut painted = PaintedElements::from_parts(
            std::mem::take(&mut page.elements),
            std::mem::take(&mut paint.policies[index]),
        )?;
        let mut background = PaintedElements::non_field(vec![PositionedElement::FilledRect {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: page.width,
                height: page.height,
            },
            color,
        }])?;
        painted.splice_at(&[], 0..0, &mut background)?;
        (page.elements, paint.policies[index]) = painted.into_parts()?;
        let boundary = &mut paint.boundaries[index];
        boundary.behind_end = boundary
            .behind_end
            .checked_add(1)
            .ok_or_else(|| LayoutError::Layout("background paint overflow".into()))?;
        boundary.regular_end = boundary
            .regular_end
            .checked_add(1)
            .ok_or_else(|| LayoutError::Layout("background paint overflow".into()))?;
        boundary.body_front_end = boundary
            .body_front_end
            .checked_add(1)
            .ok_or_else(|| LayoutError::Layout("background paint overflow".into()))?;
        boundary.furniture_front_end = boundary
            .furniture_front_end
            .checked_add(1)
            .ok_or_else(|| LayoutError::Layout("background paint overflow".into()))?;
        boundary.elements_len = boundary
            .elements_len
            .checked_add(1)
            .ok_or_else(|| LayoutError::Layout("background paint overflow".into()))?;
    }
    paint.validate(pages)
}

/// Extract the background color hex from w:background XML.
fn extract_background_color(xml: &str) -> Option<Color> {
    // Look for w:color="RRGGBB" or color="RRGGBB"
    for attr in ["w:color=\"", "color=\""] {
        if let Some(start) = xml.find(attr) {
            let val_start = start + attr.len();
            if let Some(end) = xml[val_start..].find('"') {
                let hex = &xml[val_start..val_start + end];
                if hex.len() == 6 && hex != "auto" {
                    return Some(Color::from_hex(hex));
                }
            }
        }
    }
    None
}

fn supported_styleref(instruction: &FieldInstruction) -> bool {
    instruction.name == "STYLEREF"
        && instruction.quotes_are_balanced()
        && instruction.arguments.len() == 1
        && field_text_argument(instruction, 0).is_some()
        && instruction
            .switches
            .iter()
            .all(|switch| match switch.name.as_str() {
                "*" | "#" => matches!(switch.argument, Some(FieldArgument::Text(_))),
                "l" | "n" | "r" | "w" | "t" | "p" => switch.argument.is_none(),
                _ => false,
            })
}

fn placed_styleref_values(
    input: &LayoutInput,
    registry: &SourceRegistry,
    numbering: &NumberingState,
    pages: &[Arc<oxoffice_layout::PageFrame>],
    placements: &[crate::WordFieldPlacement],
) -> HashMap<usize, HashMap<FieldSource, String>> {
    if input
        .sequence_snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.navigation_failure.is_some())
    {
        return HashMap::new();
    }
    let mut source_pages = HashMap::new();
    for page in pages {
        oxoffice_layout::walk(&page.elements, &mut |element, _| {
            let source = match element {
                PositionedElement::Text(run) => run.source,
                PositionedElement::MultilingualText(run) => run.source,
                _ => None,
            };
            if let Some(source) = source {
                source_pages.entry(source.node).or_insert(page.page_number);
            }
        });
    }
    let mut aliases: HashMap<&str, Vec<String>> = HashMap::new();
    for style in &input.styles.styles {
        let names = aliases.entry(&style.style_id).or_default();
        names.push(style.style_id.to_lowercase());
        if let Some(name) = &style.name {
            names.push(name.to_lowercase());
        }
        names.sort();
        names.dedup();
    }
    let mut styles: HashMap<String, Vec<SourceNodeId>> = HashMap::new();
    let mut style_pages: HashMap<String, std::collections::BTreeMap<usize, Vec<SourceNodeId>>> =
        HashMap::new();
    for (index, path) in registry.nodes.iter().enumerate() {
        if path.story != WordStory::Document {
            continue;
        }
        let Some(node) = u32::try_from(index + 1).ok().and_then(SourceNodeId::new) else {
            continue;
        };
        let style = numbering
            .navigation
            .get(&node)
            .and_then(|facts| facts.style.as_deref());
        for alias in style
            .and_then(|style| aliases.get(style))
            .into_iter()
            .flatten()
        {
            styles.entry(alias.clone()).or_default().push(node);
            if let Some(page) = source_pages.get(&node) {
                style_pages
                    .entry(alias.clone())
                    .or_default()
                    .entry(*page)
                    .or_default()
                    .push(node);
            }
        }
    }
    let mut values: HashMap<usize, HashMap<FieldSource, String>> = HashMap::new();
    for placement in placements {
        let Some(instruction) = registry.styleref_requests.get(&placement.source) else {
            continue;
        };
        let Some(target) = field_text_argument(instruction, 0) else {
            continue;
        };
        let key = target.to_lowercase();
        let caller = &registry.nodes[placement.source.node.get() as usize - 1];
        let last = field_has_switch(instruction, "l");
        let selected = if caller.story == WordStory::Document && !last {
            styles.get(&key).and_then(|nodes| {
                let boundary =
                    nodes.partition_point(|node| node.get() < placement.source.node.get());
                boundary
                    .checked_sub(1)
                    .and_then(|i| nodes.get(i))
                    .or_else(|| nodes.iter().find(|node| **node != placement.source.node))
                    .copied()
            })
        } else {
            style_pages.get(&key).and_then(|pages| {
                pages
                    .get(&placement.physical_page)
                    .and_then(|nodes| {
                        if last {
                            nodes
                                .iter()
                                .rev()
                                .find(|node| **node != placement.source.node)
                        } else {
                            nodes.iter().find(|node| **node != placement.source.node)
                        }
                    })
                    .copied()
                    .or_else(|| {
                        pages
                            .range(..placement.physical_page)
                            .next_back()
                            .and_then(|(_, nodes)| nodes.last())
                            .copied()
                    })
                    .or_else(|| {
                        pages
                            .range((
                                std::ops::Bound::Excluded(placement.physical_page),
                                std::ops::Bound::Unbounded,
                            ))
                            .next()
                            .and_then(|(_, nodes)| nodes.first())
                            .copied()
                    })
            })
        };
        let Some(selected) = selected.filter(|node| source_pages.contains_key(node)) else {
            continue;
        };
        let Some(facts) = numbering.navigation.get(&selected) else {
            continue;
        };
        let numbered = ["n", "r", "w"]
            .into_iter()
            .any(|name| field_has_switch(instruction, name));
        let value = if numbered {
            match numbering.source(selected) {
                Some(number) => number
                    .numbered_reference_text(instruction, numbering.source(placement.source.node)),
                None if facts.suppressed_numbering => Ok("0".into()),
                None => continue,
            }
        } else {
            Ok(facts.text.clone())
        };
        let Ok(mut value) = value else { continue };
        if field_has_switch(instruction, "p") {
            if caller.story != WordStory::Document {
                continue;
            }
            value.push_str(if selected.get() < placement.source.node.get() {
                " above"
            } else {
                " below"
            });
        }
        let Ok(value) = format_numeric_field_general(instruction, &value) else {
            continue;
        };
        values
            .entry(placement.physical_page)
            .or_default()
            .insert(placement.source, value);
    }
    values
}

fn render_annotation_replacement(
    recipe: Arc<WordAnnotationRecipe>,
    value: &str,
    placement: [u64; 4],
    fonts: &mut FontManager,
) -> Result<(PaintedElements, Vec<Diagnostic>)> {
    let decode_color = |bits: [u64; 4]| Color {
        r: f64::from_bits(bits[0]),
        g: f64::from_bits(bits[1]),
        b: f64::from_bits(bits[2]),
        a: f64::from_bits(bits[3]),
    };
    let style = &recipe.style;
    let base = AnnotationBase {
        occurrence: Some(recipe.occurrence),
        field_kind: recipe.field_kind,
        kern: style.kern,
        font_id: style.font_id,
        font_size: f64::from_bits(style.font_size),
        color: decode_color(style.color),
        bold: style.bold,
        italic: style.italic,
        baseline_offset: f64::from_bits(style.baseline_offset),
        spacing: f64::from_bits(style.spacing),
        underline: style.underline,
        strike: style.strike,
        dstrike: style.dstrike,
        highlight: style.highlight.map(decode_color),
        font_family: style.font_family.as_deref(),
        fallback_emphasis: recipe.fallback_emphasis.as_ref(),
    };
    let mut items = Vec::new();
    let mut styles = HashMap::new();
    let mut diagnostics = Vec::new();
    if value.is_empty() {
        push_empty_field_annotation(
            &mut items,
            &mut styles,
            recipe.kind.clone(),
            &base,
            recipe.source_span,
        )?;
    } else {
        let mut applied = match &recipe.kind {
            WordAnnotationKind::EastAsian(layout) => push_east_asian_layout_text(
                &mut items,
                &mut styles,
                fonts,
                layout,
                value,
                &base,
                recipe.source_span,
            )?,
            WordAnnotationKind::Emphasis(mark) => push_emphasis_marked_text(
                &mut items,
                &mut styles,
                fonts,
                &mut diagnostics,
                mark,
                value,
                &base,
            )?,
        };
        if !applied && let Some(mark) = &recipe.fallback_emphasis {
            applied = push_emphasis_marked_text(
                &mut items,
                &mut styles,
                fonts,
                &mut diagnostics,
                mark,
                value,
                &base,
            )?;
        }
        if !applied {
            let mut shaped =
                fonts.shape_text_with_kerning(base.font_id, value, base.font_size, base.kern)?;
            for advance in &mut shaped.advances {
                *advance += base.spacing;
            }
            shaped.width = shaped.advances.iter().sum();
            let baseline = f64::from_bits(placement[1]) - base.baseline_offset;
            let mut output = PaintedElements::default();
            let mut children = Vec::new();
            if let Some(color) = base.highlight {
                children.push(PositionedElement::FilledRect {
                    rect: Rect {
                        x: f64::from_bits(placement[0]),
                        y: f64::from_bits(placement[2]),
                        width: shaped.width,
                        height: f64::from_bits(placement[3]),
                    },
                    color,
                });
            }
            let mut policies = PaintPolicies::default();
            policies.emit(
                &mut children,
                PositionedElement::Text(GlyphRun {
                    origin: Point {
                        x: f64::from_bits(placement[0]),
                        y: baseline,
                    },
                    font_id: base.font_id,
                    font_size: base.font_size,
                    glyph_ids: shaped.glyph_ids,
                    advances: shaped.advances,
                    text: value.to_owned(),
                    source: recipe.source_span,
                    color: base.color,
                    bold: base.bold,
                    italic: base.italic,
                    field_kind: base.field_kind,
                    field_source: Some(recipe.occurrence.source),
                    note: None,
                    note_reference_source: None,
                    tab_aligned: None,
                }),
                annotation_field_owner(&base),
            )?;
            let metrics = fonts.metrics(base.font_id, base.font_size)?;
            let mut decorations = Vec::new();
            push_annotation_decorations(
                &mut decorations,
                &base,
                baseline,
                shaped.width,
                metrics.ascent,
                metrics.descent,
            );
            for decoration in &mut decorations {
                let PositionedElement::Line { start, end, .. } = decoration else {
                    return Err(LayoutError::Layout(
                        "annotation fallback decoration kind changed".into(),
                    ));
                };
                start.x += f64::from_bits(placement[0]);
                end.x += f64::from_bits(placement[0]);
            }
            children.extend(decorations);
            output.append(&mut PaintedElements::from_parts(children, policies)?)?;
            let (roots, mut policies) = output
                .wrap(PositionedElement::Group(GroupElement {
                    transform: Transform::IDENTITY,
                    clip: None,
                    opacity: 1.0,
                    effects: Vec::new(),
                    children: Vec::new(),
                }))?
                .into_parts()?;
            let count = policies.field_count();
            policies.attach_annotation(
                &roots,
                &[0],
                recipe,
                WordAnnotationRootKind::Group,
                count,
                placement,
            )?;
            return Ok((PaintedElements::from_parts(roots, policies)?, diagnostics));
        }
    }
    let mut output = PaintedElements::default();
    let mut x = f64::from_bits(placement[0]);
    for (index, item) in items.into_iter().enumerate() {
        let InlineItem::Group {
            width,
            baseline,
            mut group,
            ..
        } = item
        else {
            return Err(LayoutError::Layout(
                "annotation replacement lost its atomic producer".into(),
            ));
        };
        let owner = styles
            .get(&index)
            .and_then(|style| style.annotation.as_ref())
            .ok_or_else(|| {
                LayoutError::Layout("annotation replacement lost its private companion".into())
            })?;
        let y = baseline.map_or(f64::from_bits(placement[2]), |baseline| {
            f64::from_bits(placement[1]) - baseline
        });
        group.transform = group.transform.then(Transform {
            e: x,
            f: y,
            ..Transform::IDENTITY
        });
        let children = PaintedElements::from_parts(
            std::mem::take(&mut group.children),
            owner.policies.clone(),
        )?;
        output.append(&mut children.wrap(PositionedElement::Group(group))?)?;
        x += width;
    }
    let (roots, mut policies) = output
        .wrap(PositionedElement::Group(GroupElement {
            transform: Transform::IDENTITY,
            clip: None,
            opacity: 1.0,
            effects: Vec::new(),
            children: Vec::new(),
        }))?
        .into_parts()?;
    let count = policies.field_count();
    policies.attach_annotation(
        &roots,
        &[0],
        recipe,
        if value.is_empty() {
            WordAnnotationRootKind::EmptySpan
        } else {
            WordAnnotationRootKind::Group
        },
        count,
        placement,
    )?;
    Ok((PaintedElements::from_parts(roots, policies)?, diagnostics))
}

fn field_replacement(
    element: &PositionedElement,
    value: &str,
    policy: Option<(bool, bool)>,
    occurrence: Option<WordFieldOccurrence>,
    fonts: &mut FontManager,
) -> Result<PaintedElements> {
    let owner = policy.map_or(FieldPaintOwner::Neutral, |(kern, legacy)| {
        FieldPaintOwner::Word {
            kern,
            legacy,
            occurrence,
        }
    });
    let mut output = PaintedElements::default();
    match element {
        PositionedElement::Text(run) => {
            let shaped = if let Some((kern, _)) = policy {
                fonts.shape_text_with_kerning(run.font_id, value, run.font_size, kern)?
            } else {
                fonts.shape_text(run.font_id, value, run.font_size)?
            };
            let mut replacement = run.clone();
            replacement.text = value.to_owned();
            replacement.glyph_ids = shaped.glyph_ids;
            replacement.advances = shaped.advances;
            let element = PositionedElement::Text(replacement);
            if run.field_kind.is_some() || run.field_source.is_some() {
                output.push_field(element, owner)?;
            } else {
                output.push_non_field(element)?;
            }
        }
        PositionedElement::MultilingualText(run) => {
            if policy.is_some_and(|(_, legacy)| legacy) {
                return Err(LayoutError::Layout(
                    "legacy field owner reached a rich positioned leaf".into(),
                ));
            }
            if value.is_empty() {
                let mut replacement = run.clone();
                replacement.logical_text.clear();
                replacement.glyph_ids.clear();
                replacement.x_advances.clear();
                replacement.y_advances.clear();
                replacement.x_offsets.clear();
                replacement.y_offsets.clear();
                replacement.clusters.clear();
                let element = PositionedElement::MultilingualText(replacement);
                if run.field_kind.is_some() || run.field_source.is_some() {
                    output.push_field(element, owner)?;
                } else {
                    output.push_non_field(element)?;
                }
                return Ok(output);
            }
            let segment = TextSegment {
                text: value.to_owned(),
                direction: run.direction,
                source: run.source,
                font_id: run.font_id,
                font_size: run.font_size,
                glyph_ids: Vec::new(),
                advances: Vec::new(),
                width: 0.0,
                ascent: 0.0,
                descent: 0.0,
                line_gap: 0.0,
                color: run.color,
                bold: run.bold,
                italic: run.italic,
                underline: None,
                strike: false,
                dstrike: false,
                highlight: None,
                baseline_offset: 0.0,
                hyperlink_url: None,
                field_kind: run.field_kind,
                field_source: run.field_source,
                note: run.note,
                note_reference_source: run.note_reference_source,
            };
            let mut spans = if let Some((kern, legacy)) = policy {
                fonts.shape_paragraph_with_kerning(
                    vec![(segment, run.language.clone())],
                    run.direction,
                    true,
                    &[kern],
                    legacy,
                )?
            } else {
                fonts.shape_multilingual_text(
                    segment,
                    run.language.as_deref(),
                    run.direction,
                    true,
                )?
            };
            if spans.len() == 1 {
                let mut child = run.clone();
                apply_multilingual_field_span(
                    &mut child,
                    &spans.pop().expect("one field span"),
                    run.logical_index,
                );
                if child.field_kind.is_some() || child.field_source.is_some() {
                    output.push_field(PositionedElement::MultilingualText(child), owner)?;
                } else {
                    output.push_non_field(PositionedElement::MultilingualText(child))?;
                }
            } else {
                let natural_width = spans.iter().map(|span| span.width()).sum::<f64>();
                if spans.is_empty() || !natural_width.is_finite() {
                    return Err(LayoutError::Layout("invalid rich field replacement".into()));
                }
                let items = spans
                    .into_iter()
                    .map(InlineItem::MultilingualText)
                    .collect::<Vec<_>>();
                let params = oxoffice_layout::LineBreakParams {
                    available_width: natural_width.max(f64::EPSILON),
                    wrap: false,
                    ..Default::default()
                };
                let lines = break_multilingual_into_lines(&items, &params, fonts, run.direction)?;
                let [line] = lines.as_slice() else {
                    return Err(LayoutError::Layout(
                        "rich field replacement crossed a line".into(),
                    ));
                };
                let mut x = run.origin.x;
                for item in &line.items {
                    let LineItem::MultilingualText(span) = item else {
                        return Err(LayoutError::Layout(
                            "rich field replacement lost its producer".into(),
                        ));
                    };
                    let mut child = run.clone();
                    apply_multilingual_field_span(&mut child, span, run.logical_index);
                    child.origin.x = x;
                    x += span.width();
                    if child.field_kind.is_some() || child.field_source.is_some() {
                        output.push_field(PositionedElement::MultilingualText(child), owner)?;
                    } else {
                        output.push_non_field(PositionedElement::MultilingualText(child))?;
                    }
                }
                output = output.wrap(PositionedElement::Group(GroupElement {
                    transform: Transform::IDENTITY,
                    clip: None,
                    opacity: 1.0,
                    effects: Vec::new(),
                    children: Vec::new(),
                }))?;
            }
        }
        _ => {
            return Err(LayoutError::Layout(
                "field replacement requires a glyph leaf".into(),
            ));
        }
    }
    Ok(output)
}

struct FieldOccurrenceState {
    annotated: std::collections::HashSet<WordFieldOccurrence>,
    consumed: std::collections::HashSet<WordFieldOccurrence>,
    diagnostics: Vec<Diagnostic>,
}

impl FieldOccurrenceState {
    fn new(painted: &PaintedElements) -> Result<Self> {
        let occurrences = painted.policies().annotation_occurrences()?;
        let mut annotated = std::collections::HashSet::new();
        annotated
            .try_reserve(occurrences.len())
            .map_err(|_| LayoutError::Layout("annotation occurrence allocation refused".into()))?;
        annotated.extend(occurrences);
        Ok(Self {
            annotated,
            consumed: std::collections::HashSet::new(),
            diagnostics: Vec::new(),
        })
    }

    fn consume(&mut self, occurrence: WordFieldOccurrence) -> Result<()> {
        self.consumed
            .try_reserve(1)
            .map_err(|_| LayoutError::Layout("annotation traversal allocation refused".into()))?;
        self.consumed.insert(occurrence);
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn late_field_value(
    kind: FieldKind,
    source: Option<FieldSource>,
    page_number: usize,
    total_pages: usize,
    section_values: (usize, usize),
    field_sections: &HashMap<FieldSource, (usize, usize, usize)>,
    bookmark_pages: &HashMap<usize, usize>,
    page_field_values: &HashMap<FieldSource, PageFieldValue>,
    sequence: Option<&crate::WordSequenceSnapshot>,
    sequence_boundary: usize,
) -> (Option<String>, bool) {
    let sections = source
        .and_then(|source| field_sections.get(&source))
        .copied()
        .unwrap_or((section_values.0, section_values.1, page_number));
    let result = source.and_then(|source| page_field_values.get(&source));
    let value = if let Some(value) = result {
        Some(value.display.clone())
    } else {
        match kind {
            FieldKind::Page => Some(sections.2.to_string()),
            FieldKind::NumPages => Some(total_pages.to_string()),
            FieldKind::Section => Some(sections.0.to_string()),
            FieldKind::SectionPages => Some(sections.1.to_string()),
            FieldKind::TargetPage(target) => bookmark_pages.get(&target).map(ToString::to_string),
            FieldKind::SequenceRepeat => {
                sequence_repeat_display(sequence, source, sequence_boundary)
            }
            FieldKind::Target(_) | FieldKind::SequenceContext(_) => None,
        }
    };
    let clear = matches!(kind, FieldKind::TargetPage(_))
        && (result.is_some_and(|value| value.target_unplaced) || value.is_none());
    (value, clear)
}

fn substitute_styleref(
    painted: &mut PaintedElements,
    values: &HashMap<FieldSource, String>,
    seen: &mut std::collections::HashSet<FieldSource>,
    fonts: &mut FontManager,
    changes: &mut Vec<FieldWidthChange>,
    state: &mut FieldOccurrenceState,
) -> Result<()> {
    painted.policies().validate(painted.elements())?;
    let mut index = 0;
    while index < painted.len() {
        if let Some((recipe, _, placement)) = painted.policies().annotation(&[index]) {
            let source = recipe.occurrence.source;
            let Some(value) = values.get(&source) else {
                index += 1;
                continue;
            };
            if state.consumed.contains(&recipe.occurrence) || value.is_empty() {
                painted.splice_at(&[], index..index + 1, &mut PaintedElements::default())?;
                state.consume(recipe.occurrence)?;
                continue;
            }
            let (mut replacement, diagnostics) =
                render_annotation_replacement(Arc::clone(&recipe), value, placement, fonts)?;
            state.consume(recipe.occurrence)?;
            painted.splice_at(&[], index..index + 1, &mut replacement)?;
            state.diagnostics.extend(diagnostics);
            index += 1;
            continue;
        }
        let element = &painted.elements()[index];
        if let Some(children_len) = match element {
            PositionedElement::Group(group) => Some(group.children.len()),
            PositionedElement::MarkedContent { children, .. } => Some(children.len()),
            _ => None,
        } {
            let mut children =
                painted.splice_at(&[index], 0..children_len, &mut PaintedElements::default())?;
            substitute_styleref(&mut children, values, seen, fonts, changes, state)?;
            painted.splice_at(&[index], 0..0, &mut children)?;
            index += 1;
            continue;
        }
        let source = match element {
            PositionedElement::Text(run) => run.field_source,
            PositionedElement::MultilingualText(run) => run.field_source,
            _ => None,
        };
        let Some((source, value)) =
            source.and_then(|source| values.get(&source).map(|value| (source, value)))
        else {
            index += 1;
            continue;
        };
        let policy = painted.policies().required(&[index])?;
        let occurrence = painted.policies().field_occurrence(&[index])?;
        let annotated_owner = occurrence.filter(|occurrence| state.annotated.contains(occurrence));
        let consumed = annotated_owner.map_or_else(
            || seen.contains(&source),
            |occurrence| state.consumed.contains(&occurrence),
        );
        let value = if consumed { "" } else { value.as_str() };
        if let Some(occurrence) = annotated_owner {
            state.consume(occurrence)?;
        } else {
            seen.try_reserve(1)
                .map_err(|_| LayoutError::Layout("field traversal allocation refused".into()))?;
            seen.insert(source);
        }
        if value.is_empty() {
            if let PositionedElement::Text(old) = element
                && let Some(aligned) = old.tab_aligned
            {
                changes.push(FieldWidthChange {
                    aligned,
                    baseline: old.origin.y,
                    font_size: old.font_size,
                    x: old.origin.x,
                    delta: -old.advances.iter().sum::<f64>(),
                });
            }
            painted.splice_at(&[], index..index + 1, &mut PaintedElements::default())?;
            continue;
        }
        let mut replacement = field_replacement(element, value, policy, occurrence, fonts)?;
        if let (PositionedElement::Text(old), [PositionedElement::Text(new)]) =
            (element, replacement.elements())
            && let Some(aligned) = old.tab_aligned
        {
            changes.push(FieldWidthChange {
                aligned,
                baseline: old.origin.y,
                font_size: old.font_size,
                x: old.origin.x,
                delta: new.advances.iter().sum::<f64>() - old.advances.iter().sum::<f64>(),
            });
        }
        painted.splice_at(&[], index..index + 1, &mut replacement)?;
        index += 1;
    }
    Ok(())
}

fn sequence_event_boundary(elements: &[PositionedElement], previous: usize) -> usize {
    elements
        .iter()
        .fold(previous, |boundary, element| match element {
            PositionedElement::Text(run) => match run.field_kind {
                Some(FieldKind::SequenceContext(event)) => boundary.max(event + 1),
                _ => boundary,
            },
            PositionedElement::MultilingualText(run) => match run.field_kind {
                Some(FieldKind::SequenceContext(event)) => boundary.max(event + 1),
                _ => boundary,
            },
            PositionedElement::Group(group) => sequence_event_boundary(&group.children, boundary),
            PositionedElement::MarkedContent { children, .. } => {
                sequence_event_boundary(children, boundary)
            }
            _ => boundary,
        })
}

fn sequence_repeat_display(
    snapshot: Option<&crate::WordSequenceSnapshot>,
    source: Option<FieldSource>,
    boundary: usize,
) -> Option<String> {
    let snapshot = snapshot?;
    let (instruction, stored_display) = snapshot.repeat_requests.get(&source?)?;
    let value = field_text_argument(instruction, 0)
        .and_then(|identifier| snapshot.context_value(boundary, identifier))
        .and_then(|value| format_sequence_display(instruction, value.to_string()).ok());
    Some(value.unwrap_or_else(|| stored_display.clone()))
}

/// Replace field placeholder GlyphRuns with actual values.
///
/// Text aligned on a right, centre or decimal tab stop is then moved so it
/// stays on its stop with the value in place of the placeholder.
#[allow(clippy::too_many_arguments)]
fn substitute_fields(
    elements: &mut PaintedElements,
    page_number: usize,
    total_pages: usize,
    section_values: (usize, usize),
    field_sections: &HashMap<FieldSource, (usize, usize, usize)>,
    bookmark_pages: &HashMap<usize, usize>,
    page_field_values: &HashMap<FieldSource, PageFieldValue>,
    sequence: Option<&crate::WordSequenceSnapshot>,
    sequence_boundary: usize,
    fm: &mut FontManager,
) -> Result<FieldOccurrenceState> {
    let mut state = FieldOccurrenceState::new(elements)?;
    let mut changes = Vec::new();
    substitute_field_values(
        elements,
        page_number,
        total_pages,
        section_values,
        field_sections,
        bookmark_pages,
        page_field_values,
        sequence,
        sequence_boundary,
        fm,
        &mut changes,
        &mut state,
    )?;
    if !changes.is_empty() {
        realign_tab_aligned_text(elements.coordinate_elements_mut(), &changes);
    }
    Ok(state)
}

/// A tab-aligned field whose value is wider or narrower than its placeholder.
struct FieldWidthChange {
    aligned: oxoffice_layout::TabAlignedField,
    baseline: f64,
    font_size: f64,
    /// Where the placeholder started.
    x: f64,
    delta: f64,
}

impl FieldWidthChange {
    fn same_text(&self, other: &Self) -> bool {
        (self.baseline - other.baseline).abs() < 0.01
            && (self.aligned.start - other.aligned.start).abs() < 0.01
    }
}

/// How far a position moves once the fields of tab-aligned text hold their
/// values, or `None` outside such text.
///
/// Text after a right, centre or decimal tab moves back by the share of the
/// changes its stop aligns on, as far as its tab allows, and past each field
/// before the position by that field's change. A tab with no width left
/// starts its text where the text before it now ends.
fn realigned_offset(changes: &[FieldWidthChange], x: f64, baseline: f64) -> Option<f64> {
    let mut starts = changes
        .iter()
        .filter(|change| (change.baseline - baseline).abs() < 0.01)
        .collect::<Vec<_>>();
    starts.sort_by(|a, b| a.aligned.start.total_cmp(&b.aligned.start));
    starts.dedup_by(|a, b| a.same_text(b));
    // Where the previous aligned text ended, and how far that end moved.
    let mut carry = None::<(f64, f64)>;
    for first in starts {
        let aligned = first.aligned;
        let fields = changes.iter().filter(|change| change.same_text(first));
        let shift: f64 = fields
            .clone()
            .map(|change| change.aligned.shift * change.delta)
            .sum();
        let old_width = aligned.gap.max(0.0);
        let moved = carry
            .filter(|(end, _)| (aligned.start - old_width - end).abs() < 0.01)
            .map_or(0.0, |(_, moved)| moved);
        let start = moved + (aligned.gap - moved - shift).max(0.0) - old_width;
        if x > aligned.start - 0.01 && x < aligned.end - 0.001 {
            return Some(
                start
                    + fields
                        .filter(|change| change.x < x - 0.001)
                        .map(|change| change.delta)
                        .sum::<f64>(),
            );
        }
        carry = Some((
            aligned.end,
            start + fields.map(|change| change.delta).sum::<f64>(),
        ));
    }
    None
}

fn realign_tab_aligned_text(elements: &mut [PositionedElement], changes: &[FieldWidthChange]) {
    for element in elements.iter_mut() {
        match element {
            PositionedElement::Text(run) => {
                if let Some(offset) = realigned_offset(changes, run.origin.x, run.origin.y) {
                    run.origin.x += offset;
                } else {
                    realign_tab_leader(run, changes);
                }
            }
            PositionedElement::MultilingualText(run) => {
                if let Some(offset) = realigned_offset(changes, run.origin.x, run.origin.y) {
                    run.origin.x += offset;
                }
            }
            PositionedElement::Line { start, end, .. } => {
                // An underline or a strike lies between the ascent and the
                // descent of the text it marks.
                let marks = |change: &&FieldWidthChange| {
                    start.y > change.baseline - change.font_size
                        && start.y < change.baseline + change.font_size / 2.0
                };
                if let Some(offset) = changes
                    .iter()
                    .filter(marks)
                    .find_map(|change| realigned_offset(changes, start.x, change.baseline))
                {
                    end.x += offset + own_width_change(changes, start.x, marks);
                    start.x += offset;
                }
            }
            PositionedElement::FilledRect { rect, .. }
            | PositionedElement::LinkAnnotation { rect, .. } => {
                let covers = |change: &&FieldWidthChange| {
                    rect.y <= change.baseline && change.baseline <= rect.y + rect.height
                };
                if let Some(offset) = changes
                    .iter()
                    .filter(covers)
                    .find_map(|change| realigned_offset(changes, rect.x, change.baseline))
                {
                    rect.width += own_width_change(changes, rect.x, covers);
                    rect.x += offset;
                }
            }
            PositionedElement::MarkedContent { children, .. } => {
                realign_tab_aligned_text(children, changes)
            }
            _ => {}
        }
    }
}

/// The change in width of the field that starts where a mark starts, so a
/// field's own highlight or underline takes its new width.
fn own_width_change(
    changes: &[FieldWidthChange],
    x: f64,
    on_line: impl Fn(&&FieldWidthChange) -> bool,
) -> f64 {
    changes
        .iter()
        .filter(on_line)
        .find(|change| (change.x - x).abs() < 0.01)
        .map_or(0.0, |change| change.delta)
}

/// Lengthen or shorten a tab leader that ends where tab-aligned text starts,
/// so it still reaches the text once the text moves.
fn realign_tab_leader(run: &mut GlyphRun, changes: &[FieldWidthChange]) {
    let end = run.origin.x + run.advances.iter().sum::<f64>();
    // Leader glyphs step evenly, and the last one has no spacing after it,
    // so a leader stops short of its tab's end by less than two steps.
    let last = run.advances.last().copied().unwrap_or(0.0);
    let step = if run.advances.len() > 1 {
        run.advances[0]
    } else {
        last
    };
    if step <= 0.0 {
        return;
    }
    let Some(change) = changes.iter().find(|change| {
        (run.origin.y - change.baseline).abs() < 0.01
            && end < change.aligned.start + 0.01
            && change.aligned.start - end < 2.0 * step
    }) else {
        return;
    };
    let Some(&glyph) = run.glyph_ids.first() else {
        return;
    };
    let Some(ch) = run.text.chars().next() else {
        return;
    };
    if run.glyph_ids.iter().any(|id| *id != glyph)
        || run.text.chars().any(|other| other != ch)
        || run.text.chars().count() != run.glyph_ids.len()
    {
        return;
    }
    let Some(offset) = realigned_offset(changes, change.aligned.start, change.baseline) else {
        return;
    };
    let target = change.aligned.start + offset;
    let mut end = end;
    while end > target + 0.01 && run.glyph_ids.len() > 1 {
        run.glyph_ids.pop();
        run.advances.pop();
        run.text.pop();
        if let Some(previous) = run.advances.last_mut() {
            *previous = last;
        }
        end -= step;
    }
    while end + step <= target + 0.01 {
        if let Some(previous) = run.advances.last_mut() {
            *previous = step;
        }
        run.glyph_ids.push(glyph);
        run.advances.push(last);
        run.text.push(ch);
        end += step;
    }
}

#[allow(clippy::too_many_arguments)]
fn substitute_field_values(
    painted: &mut PaintedElements,
    page_number: usize,
    total_pages: usize,
    section_values: (usize, usize),
    field_sections: &HashMap<FieldSource, (usize, usize, usize)>,
    bookmark_pages: &HashMap<usize, usize>,
    page_field_values: &HashMap<FieldSource, PageFieldValue>,
    sequence: Option<&crate::WordSequenceSnapshot>,
    sequence_boundary: usize,
    fm: &mut FontManager,
    changes: &mut Vec<FieldWidthChange>,
    state: &mut FieldOccurrenceState,
) -> Result<()> {
    painted.policies().validate(painted.elements())?;
    let mut index = 0;
    while index < painted.len() {
        if let Some((recipe, _, placement)) = painted.policies().annotation(&[index]) {
            let Some(kind) = recipe.field_kind else {
                index += 1;
                continue;
            };
            if matches!(kind, FieldKind::Target(_) | FieldKind::SequenceContext(_)) {
                return Err(LayoutError::Layout(
                    "structural marker became a visible annotation".into(),
                ));
            }
            if state.consumed.contains(&recipe.occurrence) {
                painted.splice_at(&[], index..index + 1, &mut PaintedElements::default())?;
                continue;
            }
            let (value, clear_kind) = late_field_value(
                kind,
                Some(recipe.occurrence.source),
                page_number,
                total_pages,
                section_values,
                field_sections,
                bookmark_pages,
                page_field_values,
                sequence,
                sequence_boundary,
            );
            if let Some(value) = value {
                let recipe = if clear_kind {
                    let mut changed = recipe.as_ref().clone();
                    changed.field_kind = None;
                    Arc::new(changed)
                } else {
                    recipe
                };
                let (mut replacement, diagnostics) =
                    render_annotation_replacement(Arc::clone(&recipe), &value, placement, fm)?;
                state.consume(recipe.occurrence)?;
                painted.splice_at(&[], index..index + 1, &mut replacement)?;
                state.diagnostics.extend(diagnostics);
            } else if clear_kind {
                painted.clear_annotation_kind(&[index])?;
            }
            index += 1;
            continue;
        }
        let element = &painted.elements()[index];
        let (children_len, group) = match element {
            PositionedElement::Group(group) => (Some(group.children.len()), true),
            PositionedElement::MarkedContent { children, .. } => (Some(children.len()), false),
            _ => (None, false),
        };
        if let Some(children_len) = children_len {
            let mut children =
                painted.splice_at(&[index], 0..children_len, &mut PaintedElements::default())?;
            if group {
                let mut local_changes = Vec::new();
                substitute_field_values(
                    &mut children,
                    page_number,
                    total_pages,
                    section_values,
                    field_sections,
                    bookmark_pages,
                    page_field_values,
                    sequence,
                    sequence_boundary,
                    fm,
                    &mut local_changes,
                    state,
                )?;
                if !local_changes.is_empty() {
                    realign_tab_aligned_text(children.coordinate_elements_mut(), &local_changes);
                }
            } else {
                substitute_field_values(
                    &mut children,
                    page_number,
                    total_pages,
                    section_values,
                    field_sections,
                    bookmark_pages,
                    page_field_values,
                    sequence,
                    sequence_boundary,
                    fm,
                    changes,
                    state,
                )?;
            }
            let empty = children.len() == 0;
            painted.splice_at(&[index], 0..0, &mut children)?;
            if empty && !group {
                painted.splice_at(&[], index..index + 1, &mut PaintedElements::default())?;
            } else {
                index += 1;
            }
            continue;
        }
        let (kind, source) = match element {
            PositionedElement::Text(run) => (run.field_kind, run.field_source),
            PositionedElement::MultilingualText(run) => (run.field_kind, run.field_source),
            _ => (None, None),
        };
        let Some(kind) = kind else {
            index += 1;
            continue;
        };
        let policy = painted.policies().required(&[index])?;
        let occurrence = painted.policies().field_occurrence(&[index])?;
        if matches!(kind, FieldKind::Target(_) | FieldKind::SequenceContext(_)) {
            painted.splice_at(&[], index..index + 1, &mut PaintedElements::default())?;
            continue;
        }
        let annotated_owner = occurrence.filter(|occurrence| state.annotated.contains(occurrence));
        if annotated_owner.is_some_and(|occurrence| state.consumed.contains(&occurrence)) {
            painted.splice_at(&[], index..index + 1, &mut PaintedElements::default())?;
            continue;
        }
        let (value, clear_kind) = late_field_value(
            kind,
            source,
            page_number,
            total_pages,
            section_values,
            field_sections,
            bookmark_pages,
            page_field_values,
            sequence,
            sequence_boundary,
        );
        let mut leaf = match element {
            PositionedElement::Text(run) => PositionedElement::Text(run.clone()),
            PositionedElement::MultilingualText(run) => {
                PositionedElement::MultilingualText(run.clone())
            }
            _ => unreachable!(),
        };
        if clear_kind {
            match &mut leaf {
                PositionedElement::Text(run) => run.field_kind = None,
                PositionedElement::MultilingualText(run) => run.field_kind = None,
                _ => unreachable!(),
            }
        }
        if let Some(value) = value {
            let mut replacement = field_replacement(&leaf, &value, policy, occurrence, fm)?;
            if let (PositionedElement::Text(old), [PositionedElement::Text(new)]) =
                (element, replacement.elements())
                && let Some(aligned) = old.tab_aligned
            {
                let delta = new.advances.iter().sum::<f64>() - old.advances.iter().sum::<f64>();
                if delta.abs() > 1e-9 {
                    changes.push(FieldWidthChange {
                        aligned,
                        baseline: old.origin.y,
                        font_size: old.font_size,
                        x: old.origin.x,
                        delta,
                    });
                }
            }
            if let Some(occurrence) = annotated_owner {
                state.consume(occurrence)?;
            }
            painted.splice_at(&[], index..index + 1, &mut replacement)?;
        } else if clear_kind {
            let mut replacement = PaintedElements::default();
            let still_field = match &leaf {
                PositionedElement::Text(run) => {
                    run.field_kind.is_some() || run.field_source.is_some()
                }
                PositionedElement::MultilingualText(run) => {
                    run.field_kind.is_some() || run.field_source.is_some()
                }
                _ => false,
            };
            if still_field {
                replacement.push_field(
                    leaf,
                    policy.map_or(FieldPaintOwner::Neutral, |(kern, legacy)| {
                        FieldPaintOwner::Word {
                            kern,
                            legacy,
                            occurrence,
                        }
                    }),
                )?;
            } else {
                replacement.push_non_field(leaf)?;
            }
            painted.splice_at(&[], index..index + 1, &mut replacement)?;
        }
        index += 1;
    }
    Ok(())
}

fn apply_multilingual_field_span(
    run: &mut oxoffice_layout::MultilingualGlyphRun,
    span: &oxoffice_layout::MultilingualTextSegment,
    logical_base: usize,
) {
    run.font_id = span.font_id();
    run.glyph_ids = span.glyph_ids().to_vec();
    run.x_advances = span.x_advances().to_vec();
    run.y_advances = span.y_advances().to_vec();
    run.x_offsets = span.x_offsets().to_vec();
    run.y_offsets = span.y_offsets().to_vec();
    run.clusters = span.clusters().to_vec();
    run.logical_text = span.text().to_owned();
    run.logical_index = logical_base + span.logical_index();
    run.script = span.script();
    run.language = span.language().map(str::to_owned);
    run.direction = span.direction();
    run.bidi_level = span.bidi_level();
}

fn mark_remaining_artifacts(
    elements: &mut Vec<PositionedElement>,
    policies: &mut PaintPolicies,
) -> Result<()> {
    policies.validate(elements)?;
    let mut painted =
        PaintedElements::from_parts(std::mem::take(elements), std::mem::take(policies))?;
    for index in 0..painted.len() {
        if matches!(
            &painted.elements()[index],
            PositionedElement::MarkedContent { .. } | PositionedElement::LinkAnnotation { .. }
        ) {
            continue;
        }
        let child = painted.splice_at(&[], index..index + 1, &mut PaintedElements::default())?;
        painted.splice_at(
            &[],
            index..index,
            &mut child.wrap(PositionedElement::MarkedContent {
                structure: None,
                children: Vec::new(),
            })?,
        )?;
    }
    (*elements, *policies) = painted.into_parts()?;
    Ok(())
}

struct StructureBuilder {
    nodes: Vec<StructureNode>,
}

impl StructureBuilder {
    fn add(&mut self, role: StructureRole, parent: Option<StructureId>) -> StructureId {
        let id = StructureId::new(self.nodes.len() as u32 + 1)
            .expect("a structure node index is always non-zero");
        self.nodes.push(StructureNode {
            id,
            role,
            children: Vec::new(),
            alternate_text: None,
        });
        if let Some(parent) = parent
            && let Some(node) = self.nodes.get_mut(parent.get() as usize - 1)
        {
            node.children.push(id);
        }
        id
    }

    fn set_alternate_text(&mut self, id: StructureId, text: String) {
        if let Some(node) = self.nodes.get_mut(id.get() as usize - 1) {
            node.alternate_text = Some(text);
        }
    }
}

#[derive(Clone, Copy)]
struct ListFrame {
    num_id: u32,
    level: u8,
    list: StructureId,
    last_item: Option<StructureId>,
}

fn assign_shared_document_structure(
    sections: &mut [paginator::SharedSection],
) -> DocumentStructure {
    let mut builder = StructureBuilder { nodes: Vec::new() };
    let root = builder.add(StructureRole::Document, None);

    for section in sections {
        let mut lists: Vec<ListFrame> = Vec::new();
        for block in &mut section.blocks {
            match block {
                SharedLayoutBlock::Paragraph {
                    block, semantics, ..
                } => {
                    lists.clear();
                    let role = block
                        .heading_level
                        .map(|level| StructureRole::Heading(level.min(6) as u8))
                        .unwrap_or(StructureRole::Paragraph);
                    let paragraph_id = builder.add(role, Some(root));
                    semantics.structure_id = Some(paragraph_id);
                    debug_assert!(block.list.is_none());
                    debug_assert!(block.anchored.is_empty());
                }
                SharedLayoutBlock::Table {
                    block, semantics, ..
                } => {
                    lists.clear();
                    assign_shared_table_structure(&mut builder, block, semantics, root);
                }
                SharedLayoutBlock::Owned {
                    block, semantics, ..
                } => match block.as_mut() {
                    LayoutBlock::Paragraph(paragraph) => {
                        assign_owned_paragraph_structure(&mut builder, paragraph, root, &mut lists);
                        if let Some(semantics) = semantics {
                            semantics.structure_id = paragraph.structure_id;
                        }
                    }
                    LayoutBlock::Table(table) => {
                        lists.clear();
                        assign_owned_table_structure(&mut builder, table, root);
                    }
                },
            }
        }
    }

    DocumentStructure {
        root,
        nodes: builder.nodes,
    }
}

fn assign_owned_paragraph_structure(
    builder: &mut StructureBuilder,
    paragraph: &mut ParagraphBlock,
    root: StructureId,
    lists: &mut Vec<ListFrame>,
) {
    if let Some((num_id, level)) = paragraph.list {
        if lists.last().is_some_and(|frame| frame.num_id != num_id) {
            lists.clear();
        }
        while lists.last().is_some_and(|frame| frame.level > level) {
            lists.pop();
        }
        if lists.is_empty() || lists.last().is_some_and(|frame| frame.level < level) {
            let parent = lists
                .last()
                .and_then(|frame| frame.last_item)
                .unwrap_or(root);
            let list = builder.add(StructureRole::List, Some(parent));
            lists.push(ListFrame {
                num_id,
                level,
                list,
                last_item: None,
            });
        }
        let frame = lists
            .last_mut()
            .expect("the requested list level has been allocated");
        let item = builder.add(StructureRole::ListItem, Some(frame.list));
        frame.last_item = Some(item);
        let paragraph_id = builder.add(StructureRole::Paragraph, Some(item));
        paragraph.structure_id = Some(paragraph_id);
        assign_paragraph_figures(builder, paragraph, paragraph_id, item);
    } else {
        lists.clear();
        let role = paragraph
            .heading_level
            .map(|level| StructureRole::Heading(level.min(6) as u8))
            .unwrap_or(StructureRole::Paragraph);
        let paragraph_id = builder.add(role, Some(root));
        paragraph.structure_id = Some(paragraph_id);
        assign_paragraph_figures(builder, paragraph, paragraph_id, root);
    }
}

fn assign_owned_table_structure(
    builder: &mut StructureBuilder,
    table: &mut table::TableBlock,
    parent: StructureId,
) {
    let table_id = builder.add(StructureRole::Table, Some(parent));
    table.structure_id = Some(table_id);
    for row in &mut table.rows {
        let row_id = builder.add(StructureRole::TableRow, Some(table_id));
        row.structure_id = Some(row_id);
        for cell in &mut row.cells {
            let role = if row.is_header {
                StructureRole::TableHeaderCell
            } else {
                StructureRole::TableCell
            };
            let cell_id = builder.add(role, Some(row_id));
            cell.structure_id = Some(cell_id);
            for block in &mut cell.blocks {
                assign_cell_block_structure(builder, block, cell_id);
            }
        }
    }
}

fn assign_shared_table_structure(
    builder: &mut StructureBuilder,
    table: &table::TableBlock,
    semantics: &mut TableSemantics,
    parent: StructureId,
) {
    let table_id = builder.add(StructureRole::Table, Some(parent));
    for (row, row_semantics) in table.rows.iter().zip(&mut semantics.rows) {
        let row_id = builder.add(StructureRole::TableRow, Some(table_id));
        for (cell, cell_semantics) in row.cells.iter().zip(&mut row_semantics.cells) {
            let role = if row.is_header {
                StructureRole::TableHeaderCell
            } else {
                StructureRole::TableCell
            };
            let cell_id = builder.add(role, Some(row_id));
            for (block, block_semantics) in cell.blocks.iter().zip(&mut cell_semantics.blocks) {
                match (block, block_semantics) {
                    (
                        table::CellBlock::Paragraph(paragraph),
                        CellBlockSemantics::Paragraph(semantics),
                    ) => {
                        let role = paragraph
                            .heading_level
                            .map(|level| StructureRole::Heading(level.min(6) as u8))
                            .unwrap_or(StructureRole::Paragraph);
                        semantics.structure_id = Some(builder.add(role, Some(cell_id)));
                    }
                    (table::CellBlock::Table(table), CellBlockSemantics::Table(semantics)) => {
                        assign_shared_table_structure(builder, table, semantics, cell_id)
                    }
                    _ => unreachable!("shared table semantics stay aligned"),
                }
            }
        }
    }
}

fn assign_cell_block_structure(
    builder: &mut StructureBuilder,
    block: &mut table::CellBlock,
    parent: StructureId,
) {
    match block {
        table::CellBlock::Paragraph(paragraph) => {
            let role = paragraph
                .heading_level
                .map(|level| StructureRole::Heading(level.min(6) as u8))
                .unwrap_or(StructureRole::Paragraph);
            let paragraph_id = builder.add(role, Some(parent));
            paragraph.structure_id = Some(paragraph_id);
            assign_paragraph_figures(builder, paragraph, paragraph_id, parent);
        }
        table::CellBlock::Table(table) => {
            let table_id = builder.add(StructureRole::Table, Some(parent));
            table.structure_id = Some(table_id);
            for row in &mut table.rows {
                let row_id = builder.add(StructureRole::TableRow, Some(table_id));
                row.structure_id = Some(row_id);
                for cell in &mut row.cells {
                    let role = if row.is_header {
                        StructureRole::TableHeaderCell
                    } else {
                        StructureRole::TableCell
                    };
                    let cell_id = builder.add(role, Some(row_id));
                    cell.structure_id = Some(cell_id);
                    for child in &mut cell.blocks {
                        assign_cell_block_structure(builder, child, cell_id);
                    }
                }
            }
        }
    }
}

fn assign_paragraph_figures(
    builder: &mut StructureBuilder,
    paragraph: &mut ParagraphBlock,
    inline_parent: StructureId,
    anchored_parent: StructureId,
) {
    for line in &mut paragraph.lines {
        for item in &mut line.items {
            let (alternate_text, structure_id) = match item {
                LineItem::Figure {
                    alternate_text,
                    structure_id,
                    ..
                } => (alternate_text, structure_id),
                _ => continue,
            };
            let figure = builder.add(StructureRole::Figure, Some(inline_parent));
            builder.set_alternate_text(figure, alternate_text.clone());
            *structure_id = Some(figure);
        }
    }
    for drawing in &mut paragraph.anchored {
        if let Some(text) = drawing
            .alternate_text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            let figure = builder.add(StructureRole::Figure, Some(anchored_parent));
            builder.set_alternate_text(figure, text.to_owned());
            drawing.structure_id = Some(figure);
        }
    }
}

fn excluded_owner_for_source(input: &LayoutInput, source: SourceNodeId) -> bool {
    input.sequence_snapshot.as_ref().is_some_and(|snapshot| {
        snapshot
            .source_node(source)
            .is_some_and(|path| snapshot.navigation_exclusions.contains_key(path))
    })
}

// Cache-safe tables have direct rows/cells and no removed revisions or controls.
// Replay the returned layout's actual omission decisions against current source paths.
fn record_cached_table_omissions(
    source: &CT_Tbl,
    block: &table::TableBlock,
    sources: &SourceRegistry,
    story: &WordStory,
    path: &[usize],
    state: &mut NumberingState,
) {
    for (ri, (row, laid_out)) in source.rows.iter().zip(&block.rows).enumerate() {
        for (ci, (cell, placed)) in row.cells.iter().zip(&laid_out.cells).enumerate() {
            let mut cell_path = path.to_vec();
            cell_path.extend([ri, ci]);
            if placed.is_vmerge_continue {
                sources.record_table_omission(story, &cell_path, state);
                continue;
            }
            for (index, (content, block)) in cell.content.iter().zip(&placed.blocks).enumerate() {
                if let (CellContent::Table(source), table::CellBlock::Table(block)) =
                    (content, block)
                {
                    let mut nested = cell_path.clone();
                    nested.push(index);
                    record_cached_table_omissions(source, block, sources, story, &nested, state);
                }
            }
        }
    }
}

fn record_table_navigation(
    table: &crate::table::TableBlock,
    semantics: &TableSemantics,
    sources: &SourceRegistry,
    state: &mut NumberingState,
) {
    for (row, row_semantics) in table.rows.iter().zip(&semantics.rows) {
        for (cell, cell_semantics) in row.cells.iter().zip(&row_semantics.cells) {
            for (block, semantics) in cell.blocks.iter().zip(&cell_semantics.blocks) {
                match (block, semantics) {
                    (
                        crate::table::CellBlock::Paragraph(paragraph),
                        CellBlockSemantics::Paragraph(semantics),
                    ) => {
                        if let Some((source, original)) = semantics.source_node.and_then(|node| {
                            sources
                                .navigation_sources
                                .get(&node)
                                .map(|facts| (node, facts))
                        }) {
                            let mut facts = state
                                .remove_navigation(source)
                                .unwrap_or_else(|| original.clone());
                            facts.level = paragraph.heading_level;
                            state.record_navigation(source, facts);
                        }
                    }
                    (
                        crate::table::CellBlock::Table(table),
                        CellBlockSemantics::Table(semantics),
                    ) => {
                        record_table_navigation(table, semantics, sources, state);
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Detect if a paragraph has a heading style, returning the level (1-9).
fn detect_heading_level(para: &CT_P, styles: &CT_Styles) -> Option<u32> {
    let style_id = para.properties.as_ref()?.style_id.as_deref()?;
    // Check if style ID matches "Heading1" .. "Heading9"
    if let Some(rest) = style_id.strip_prefix("Heading") {
        return rest.parse::<u32>().ok().filter(|n| (1..=9).contains(n));
    }
    // Also check style name in the styles definitions
    if let Some(style_def) = styles.get_by_id(style_id)
        && let Some(ref name) = style_def.name
        && let Some(rest) = name.strip_prefix("heading ")
    {
        return rest.parse::<u32>().ok().filter(|n| (1..=9).contains(n));
    }
    None
}

/// Whether `style_id` names a table of contents level, "toc 1" to "toc 9".
fn is_toc_paragraph_style(style_id: &str, styles: &CT_Styles) -> bool {
    let id_level = |id: &str| {
        id.get(..3)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("toc"))
            && id[3..]
                .trim()
                .parse::<u32>()
                .is_ok_and(|n| (1..=9).contains(&n))
    };
    // Match the existing TOC style-name lookup: the whole name is trimmed and
    // compared to "toc N". A producer name such as "toc 1 " therefore matches.
    let name_level =
        |name: &str| (1..=9).any(|n| name.trim().eq_ignore_ascii_case(&format!("toc {n}")));
    id_level(style_id)
        || styles
            .get_by_id(style_id)
            .and_then(|style| style.name.as_deref())
            .is_some_and(name_level)
}

/// Whether `style_id` is the Hyperlink character style.
fn is_hyperlink_character_style(style_id: &str, styles: &CT_Styles) -> bool {
    style_id == "Hyperlink"
        || styles
            .get_by_id(style_id)
            .and_then(|style| style.name.as_deref())
            .is_some_and(|name| name.eq_ignore_ascii_case("Hyperlink"))
}

fn computed_layout_field(
    field: &oxoffice_docx_oxml::text::Field,
    source: Option<FieldSource>,
    source_node: Option<SourceNodeId>,
    input: &LayoutInput,
    num_state: &NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Option<String>, Option<FieldKind>) {
    let instruction = field.effective_instruction();
    let page_family = matches!(
        instruction.name.as_str(),
        "PAGE" | "NUMPAGES" | "SECTION" | "SECTIONPAGES" | "PAGEREF"
    );
    let (computed_value, field_kind) = if page_family && field.locked() == Some(true) {
        (None, None)
    } else if page_family && let Err(reason) = validate_page_field_instruction(&instruction) {
        diagnostics.push(Diagnostic {
            message: format!("{} retained stored display: {reason}", instruction.name),
        });
        (None, None)
    } else {
        match instruction.name.as_str() {
            "PAGE" => (Some("99".to_owned()), Some(FieldKind::Page)),
            "NUMPAGES" => (Some("99".to_owned()), Some(FieldKind::NumPages)),
            "SECTION" => (Some("99".to_owned()), Some(FieldKind::Section)),
            "SECTIONPAGES" => (Some("99".to_owned()), Some(FieldKind::SectionPages)),
            "SEQ" => {
                let result =
                    source.and_then(|source| input.sequence_snapshot.as_ref()?.field_value(source));
                match result {
                    Some(Ok(value)) => (Some(value.to_owned()), None),
                    Some(Err(message)) => {
                        if source.is_some_and(|source| {
                            input.sequence_snapshot.as_ref().is_some_and(|snapshot| {
                                snapshot.repeat_requests.contains_key(&source)
                            })
                        }) {
                            (Some(String::new()), Some(FieldKind::SequenceRepeat))
                        } else {
                            diagnostics.push(Diagnostic {
                                message: message.to_owned(),
                            });
                            (None, None)
                        }
                    }
                    None => {
                        diagnostics.push(Diagnostic { message: "SEQ retains its stored display without accepted source-qualified context".into() });
                        (None, None)
                    }
                }
            }
            "REF" => {
                let Some(bookmark) = field_text_argument(&instruction, 0) else {
                    return (None, None);
                };
                if let Some(text) = bookmark_text(input, bookmark) {
                    match numbered_ref_text(
                        &instruction,
                        bookmark,
                        text,
                        num_state,
                        source_node,
                        if field_has_switch(&instruction, "p") {
                            input
                                .sequence_snapshot
                                .as_ref()
                                .zip(source)
                                .ok_or_else(|| {
                                    "REF relative source lacks an accepted physical binding"
                                        .to_owned()
                                })
                                .and_then(|(snapshot, source)| {
                                    snapshot
                                        .bookmark_relative_position(bookmark, source)
                                        .map(Some)
                                })
                        } else {
                            Ok(None)
                        },
                    )
                    .and_then(|value| format_numeric_field_display(&instruction, value))
                    {
                        Ok(value) => (Some(value), None),
                        Err(message) => {
                            diagnostics.push(Diagnostic {
                                message: format!("{message}, stored REF display retained"),
                            });
                            (None, None)
                        }
                    }
                } else {
                    diagnostics.push(Diagnostic {
                        message: format!(
                            "REF target {bookmark} was not found, stored display retained"
                        ),
                    });
                    (None, None)
                }
            }
            "PAGEREF" => {
                let Some(bookmark) = field_text_argument(&instruction, 0) else {
                    return (None, None);
                };
                if bookmark_text(input, bookmark).is_none() {
                    diagnostics.push(Diagnostic {
                        message: format!(
                            "PAGEREF target {bookmark} was not found, stored display retained"
                        ),
                    });
                    (None, None)
                } else if let Some(target) = page_ref_id(input, bookmark) {
                    (Some("99".to_owned()), Some(FieldKind::TargetPage(target)))
                } else {
                    (None, None)
                }
            }
            _ => (None, None),
        }
    };
    // Word draws a legacy checkbox itself from its state, locked or not;
    // any stored result only gives the box its formatting.
    if let Some(glyph) = drawn_legacy_checkbox(field) {
        return (Some(glyph.to_owned()), None);
    }
    let (computed_value, field_kind) = if field.locked() == Some(true) {
        (None, None)
    } else {
        (computed_value, field_kind)
    };
    (computed_value, field_kind)
}

/// The box a legacy checkbox draws from its state, as Word does whatever
/// result it stores: ☒ when checked, ☐ when not.
fn drawn_legacy_checkbox(field: &oxoffice_docx_oxml::text::Field) -> Option<&'static str> {
    let data = field.legacy_form.as_ref()?;
    let oxoffice_docx_oxml::text::LegacyFormFieldValue::Checked(checked) = data.value else {
        return None;
    };
    Some(if checked { "\u{2612}" } else { "\u{2610}" })
}

/// Lay out a single paragraph into a ParagraphBlock.
pub fn layout_paragraph(
    para: &CT_P,
    available_width: f64,
    styles: &CT_Styles,
    input: &LayoutInput,
    media: &MediaRegistry,
    fm: &mut FontManager,
    num_state: &mut NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<ParagraphBlock> {
    layout_paragraph_with_source(
        para,
        available_width,
        styles,
        input,
        media,
        fm,
        num_state,
        diagnostics,
        None,
    )
}

/// Lay out one paragraph on a section character grid.
#[allow(clippy::too_many_arguments)]
pub(crate) fn layout_paragraph_with_grid(
    para: &CT_P,
    available_width: f64,
    styles: &CT_Styles,
    input: &LayoutInput,
    media: &MediaRegistry,
    fm: &mut FontManager,
    num_state: &mut NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
    doc_grid: Option<&CT_DocGrid>,
) -> Result<ParagraphBlock> {
    layout_paragraph_with_source_and_table(
        para,
        available_width,
        styles,
        input,
        media,
        fm,
        num_state,
        diagnostics,
        None,
        None,
        None,
        None,
        None,
        None,
        doc_grid,
        false,
    )
    .map(|(block, _)| block)
}

pub(crate) fn layout_paragraph_with_source(
    para: &CT_P,
    available_width: f64,
    styles: &CT_Styles,
    input: &LayoutInput,
    media: &MediaRegistry,
    fm: &mut FontManager,
    num_state: &mut NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
    source_node: Option<SourceNodeId>,
) -> Result<ParagraphBlock> {
    layout_paragraph_with_source_and_table(
        para,
        available_width,
        styles,
        input,
        media,
        fm,
        num_state,
        diagnostics,
        source_node,
        source_node,
        None,
        None,
        None,
        None,
        None,
        false,
    )
    .map(|(block, _)| block)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn layout_paragraph_with_source_and_direction(
    para: &CT_P,
    available_width: f64,
    styles: &CT_Styles,
    input: &LayoutInput,
    media: &MediaRegistry,
    fm: &mut FontManager,
    num_state: &mut NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
    source_node: Option<SourceNodeId>,
    navigation_node: Option<SourceNodeId>,
    doc_grid: Option<&CT_DocGrid>,
) -> Result<(ParagraphBlock, TextDirection, WordParagraphPaint)> {
    let mut direction = TextDirection::Auto;
    let (block, paint) = layout_paragraph_with_source_and_table(
        para,
        available_width,
        styles,
        input,
        media,
        fm,
        num_state,
        diagnostics,
        source_node,
        navigation_node,
        None,
        None,
        None,
        Some(&mut direction),
        doc_grid,
        false,
    )?;
    Ok((block, direction, paint))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn layout_paragraph_with_source_in_table(
    para: &CT_P,
    available_width: f64,
    styles: &CT_Styles,
    input: &LayoutInput,
    media: &MediaRegistry,
    fm: &mut FontManager,
    num_state: &mut NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
    source_node: Option<SourceNodeId>,
    table_properties: Option<&oxoffice_docx_oxml::properties::CT_PPr>,
    table_run_properties: Option<&oxoffice_docx_oxml::properties::CT_RPr>,
    cell_shading: Option<Color>,
    doc_grid: Option<&CT_DocGrid>,
) -> Result<(ParagraphBlock, TextDirection, WordParagraphPaint)> {
    let mut direction = TextDirection::Auto;
    let (block, paint) = layout_paragraph_with_source_and_table(
        para,
        available_width,
        styles,
        input,
        media,
        fm,
        num_state,
        diagnostics,
        source_node,
        source_node,
        table_properties,
        table_run_properties,
        cell_shading,
        Some(&mut direction),
        doc_grid,
        true,
    )?;
    Ok((block, direction, paint))
}

#[allow(clippy::too_many_arguments)]
fn layout_paragraph_with_source_and_table(
    para: &CT_P,
    available_width: f64,
    styles: &CT_Styles,
    input: &LayoutInput,
    media: &MediaRegistry,
    fm: &mut FontManager,
    num_state: &mut NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
    source_node: Option<SourceNodeId>,
    navigation_node: Option<SourceNodeId>,
    table_properties: Option<&oxoffice_docx_oxml::properties::CT_PPr>,
    table_run_properties: Option<&oxoffice_docx_oxml::properties::CT_RPr>,
    cell_shading: Option<Color>,
    reflow_direction_out: Option<&mut TextDirection>,
    doc_grid: Option<&CT_DocGrid>,
    in_table_cell: bool,
) -> Result<(ParagraphBlock, WordParagraphPaint)> {
    // Resolve paragraph properties
    let para_style_id = para.properties.as_ref().and_then(|p| p.style_id.as_deref());

    let resolved_ppr = style_resolver::resolve_paragraph_properties_in_table(
        para_style_id,
        styles,
        table_properties,
    );

    let mut effective_ppr = resolved_ppr;

    // A numbering level carries paragraph properties of its own, mainly the
    // indentation for that level. They sit between the style and direct
    // formatting, so merge them before the direct properties rather than
    // after. Without this every level of a list draws at the same indent.
    let direct_ppr = para.properties.as_ref();
    let list_num_id = direct_ppr.and_then(|p| p.num_id).or(effective_ppr.num_id);
    let list_ilvl = direct_ppr
        .and_then(|p| p.num_ilvl)
        .or(effective_ppr.num_ilvl)
        .unwrap_or(0);
    if let (Some(num_id), Some(numbering)) = (list_num_id, input.numbering.as_ref())
        && let Some(lvl_ppr) =
            style_resolver::level_paragraph_properties(num_id, list_ilvl, numbering)
    {
        merge_direct_ppr(&mut effective_ppr, lvl_ppr);
    }

    // Merge direct paragraph properties
    if let Some(direct_ppr) = direct_ppr {
        merge_direct_ppr(&mut effective_ppr, direct_ppr);
    }

    let exclusions = navigation_node.and_then(|node| {
        let snapshot = input.sequence_snapshot.as_ref()?;
        snapshot
            .navigation_exclusions
            .get(snapshot.source_node(node)?)
    });
    let projected = project_paragraph_runs(para, input.revision_view);
    let heading_text = projected.iter().enumerate().filter(|(index, _)| exclusions.is_none_or(|runs| runs.binary_search(index).is_err()))
        .map(|(_, projected)| {
            let mut run = projected.run.clone();
            if exclusions.is_some() {
                run.content.retain(|content| !matches!(content, RunContent::Field(field) if matches!(field.effective_instruction().name.as_str(), "TOC" | "INDEX" | "TOA" | "BIBLIOGRAPHY")));
            }
            run.text()
        }).collect::<String>();
    let cache_only = exclusions.is_some() && heading_text.is_empty();
    let heading_level = effective_ppr
        .outline_lvl
        .or_else(|| detect_heading_level(para, styles).map(|level| level - 1))
        .filter(|level| *level < 9 && !cache_only)
        .map(|level| level + 1);
    if let Some(source) = navigation_node {
        num_state.record_navigation(
            source,
            crate::ParagraphNavigation {
                omitted_by_table: false,
                level: heading_level,
                text: heading_text.clone(),
                style: (!cache_only)
                    .then(|| para_style_id.map(str::to_owned))
                    .flatten(),
                suppressed_numbering: effective_ppr.num_id == Some(0),
                accepted_runs: projected.len(),
            },
        );
    }

    // Convert paragraph properties to layout values
    let space_before = effective_ppr.space_before.map(|t| t.to_pt()).unwrap_or(0.0);
    let space_after = effective_ppr.space_after.map(|t| t.to_pt()).unwrap_or(0.0);
    let base_direction = match effective_ppr.bidi {
        Some(true) => TextDirection::RightToLeft,
        Some(false) => TextDirection::LeftToRight,
        None => TextDirection::Auto,
    };
    let keep_next = effective_ppr.keep_next.unwrap_or(false);
    let keep_lines = effective_ppr.keep_lines.unwrap_or(false);
    let page_break_before = effective_ppr.page_break_before.unwrap_or(false);
    let widow_control = effective_ppr.widow_control.unwrap_or(true);
    let automatic_hyphenation =
        input.automatic_hyphenation && effective_ppr.suppress_auto_hyphens != Some(true);

    // The section character grid, gated by the paragraph's own `w:snapToGrid`.
    // A `default` grid, and an absent one, leave both values at their
    // ungridded form, so an ungridded paragraph takes the branch it always
    // took with no new arithmetic on it.
    let doc_grid = doc_grid.filter(|_| effective_ppr.snap_to_grid != Some(false));
    let grid_char_space_pt = doc_grid
        .filter(|grid| grid.grid_type.is_some_and(ST_DocGrid::snaps_characters))
        .and_then(|grid| grid.char_space)
        .map_or(0.0, |space| oxoffice_docx_oxml::units::Twips(space).to_pt());
    let grid_line_pitch_pt = doc_grid
        .filter(|grid| grid.grid_type.is_some_and(ST_DocGrid::snaps_lines))
        .and_then(|grid| grid.line_pitch)
        .map(|pitch| pitch.to_pt())
        .filter(|pitch| *pitch > 0.0);

    // Parse shading color
    let shading = effective_ppr
        .shading
        .as_ref()
        .and_then(|shd| shd.fill.as_ref())
        .filter(|f| f != &"auto")
        .map(|f| Color::from_hex(f));
    // Text in the automatic colour contrasts with the fill behind it, the
    // paragraph's own or else the cell's.
    let automatic_color = automatic_text_color(shading.or(cell_shading));
    let toc_paragraph =
        para_style_id.is_some_and(|style_id| is_toc_paragraph_style(style_id, styles));

    // Convert runs to inline items
    let mut inline_items = Vec::new();
    let mut multilingual_styles = HashMap::<usize, WordMultilingualStyle>::new();
    let mut requested_context_bytes = 0usize;
    let mut requested_context_entries = 0usize;
    let current_bookmarks = para.current_bookmark_projection();

    if let Some(source_node) = source_node {
        for name in para
            .bookmark_markers
            .iter()
            .filter(|bookmark| bookmark.is_start())
            .filter_map(|bookmark| bookmark.name())
        {
            num_state.record_bookmark_source(name, source_node);
        }
    }

    // Handle numbering marker
    if let (Some(num_id), Some(numbering)) = (effective_ppr.num_id, input.numbering.as_ref()) {
        let ilvl = effective_ppr.num_ilvl.unwrap_or(0);
        if let Some(marker) = style_resolver::generate_marker(num_id, ilvl, numbering, num_state) {
            if let Some(source_node) = source_node {
                num_state.record(source_node, &marker);
                for name in para
                    .bookmark_markers
                    .iter()
                    .filter(|bookmark| bookmark.is_start())
                    .filter_map(|bookmark| bookmark.name())
                {
                    num_state.record_bookmark(name, source_node, &marker);
                }
            }
            // Shape the marker text
            let marker_rpr = marker.marker_rpr;
            let marker_font_size = marker_rpr.sz.map(|hp| hp.to_pt()).unwrap_or_else(|| {
                style_resolver::resolve_run_properties(
                    para_style_id,
                    None,
                    styles,
                    table_run_properties,
                )
                .sz
                .map(|hp| hp.to_pt())
                .unwrap_or(11.0)
            });
            let marker_bold = marker_rpr.bold.unwrap_or(false);
            let marker_italic = marker_rpr.italic.unwrap_or(false);
            let marker_font_family = marker_rpr.font_ascii.as_deref();

            // Bullet glyphs are not in every font either, so the marker gets
            // the same coverage check as body text.
            let display_selection = if marker
                .marker_text
                .chars()
                .any(|character| (0xf000..=0xf0ff).contains(&(character as u32)))
            {
                fm.resolve_word_symbol_text(
                    marker_font_family,
                    marker_bold,
                    marker_italic,
                    &marker.marker_text,
                    false,
                )
            } else {
                fm.resolve_font_for_text(
                    marker_font_family,
                    marker_bold,
                    marker_italic,
                    &marker.marker_text,
                )
                .map(|id| (marker.marker_text.clone(), id))
            };
            if let Ok((display_text, font_id)) = display_selection
                && let Ok(shaped) = fm.shape_text_with_kerning(
                    font_id,
                    &display_text,
                    marker_font_size,
                    marker_rpr
                        .kern
                        .is_some_and(|threshold| threshold.to_pt() <= marker_font_size),
                )
            {
                let metrics = fm.metrics(font_id, marker_font_size)?;
                for message in fm.unknown_word_symbol_diagnostics(
                    font_id,
                    marker_font_family,
                    &marker.marker_text,
                    &display_text,
                ) {
                    push_unique_diagnostic(diagnostics, message);
                }
                let color = marker_rpr
                    .color
                    .as_ref()
                    .filter(|c| c.as_str() != "auto")
                    .map(|c| Color::from_hex(c))
                    .unwrap_or(automatic_color);

                // The level's run properties affect numbering ink, not the
                // counter or paragraph heading. The suffix keeps its normal
                // layout policy independently of hidden marker glyphs.
                if marker_rpr.vanish != Some(true) {
                    multilingual_styles.insert(
                        inline_items.len(),
                        WordMultilingualStyle {
                            kern: marker_rpr
                                .kern
                                .is_some_and(|threshold| threshold.to_pt() <= marker_font_size),
                            origin: WordInputOrigin::AtomicMarker,
                            requested_family: fm.copy_font_request_context(
                                marker_font_family,
                                &mut requested_context_bytes,
                                &mut requested_context_entries,
                            ),
                            ..Default::default()
                        },
                    );
                    inline_items.push(InlineItem::Marker(TextSegment {
                        text: display_text,
                        direction: TextDirection::Auto,
                        source: None,
                        font_id,
                        font_size: marker_font_size,
                        glyph_ids: shaped.glyph_ids,
                        advances: shaped.advances,
                        width: shaped.width,
                        ascent: metrics.ascent,
                        descent: metrics.descent,
                        line_gap: 0.0,
                        color,
                        bold: marker_bold,
                        italic: marker_italic,
                        underline: None,
                        strike: false,
                        dstrike: false,
                        highlight: None,
                        baseline_offset: 0.0,
                        hyperlink_url: None,
                        field_kind: None,
                        field_source: None,
                        note: None,
                        note_reference_source: None,
                    }));
                }

                match marker.suffix {
                    ST_LvlSuffix::Tab => inline_items.push(InlineItem::Tab),
                    ST_LvlSuffix::Space => {
                        multilingual_styles.insert(
                            inline_items.len(),
                            WordMultilingualStyle {
                                kern: marker_rpr
                                    .kern
                                    .is_some_and(|threshold| threshold.to_pt() <= marker_font_size),
                                ..Default::default()
                            },
                        );
                        let shaped = fm.shape_text_with_kerning(
                            font_id,
                            " ",
                            marker_font_size,
                            marker_rpr
                                .kern
                                .is_some_and(|threshold| threshold.to_pt() <= marker_font_size),
                        )?;
                        inline_items.push(InlineItem::Text(TextSegment {
                            text: " ".to_owned(),
                            direction: TextDirection::Auto,
                            source: None,
                            font_id,
                            font_size: marker_font_size,
                            glyph_ids: shaped.glyph_ids,
                            advances: shaped.advances,
                            width: shaped.width,
                            ascent: metrics.ascent,
                            descent: metrics.descent,
                            line_gap: 0.0,
                            color,
                            bold: marker_bold,
                            italic: marker_italic,
                            underline: None,
                            strike: false,
                            dstrike: false,
                            highlight: None,
                            baseline_offset: 0.0,
                            hyperlink_url: None,
                            field_kind: None,
                            field_source: None,
                            note: None,
                            note_reference_source: None,
                        }));
                    }
                    ST_LvlSuffix::Nothing => {}
                }
            }
        }
    }

    // Build hyperlink URL map: run index → URL
    let mut run_hyperlink_url: std::collections::HashMap<usize, String> =
        std::collections::HashMap::new();
    for hl in &para.hyperlinks {
        if let Some(ref rel_id) = hl.rel_id
            && let Some(url) = input.hyperlink_urls.get(rel_id)
        {
            for run_idx in hl.run_start..hl.run_end {
                run_hyperlink_url.insert(run_idx, url.clone());
            }
        }
    }

    // Physical typed runs reserve stable field identity before accepted
    // selection, including deleted and inserted revision owners.
    let field_indices = source_node
        .map(|_| {
            fn collect<'a>(
                field: &'a oxoffice_docx_oxml::text::Field,
                fields: &mut Vec<&'a oxoffice_docx_oxml::text::Field>,
            ) {
                fields.push(field);
                for nested in field.all_nested_fields_in_source_order() {
                    collect(nested, fields);
                }
            }
            let mut fields = Vec::new();
            for field in para
                .source_runs()
                .into_iter()
                .flat_map(|run| &run.content)
                .filter_map(|content| match content {
                    RunContent::Field(field) => Some(field),
                    _ => None,
                })
            {
                collect(field, &mut fields);
            }
            fields
                .into_iter()
                .map(std::ptr::from_ref)
                .zip(0u32..)
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();

    // Process ordinary and revision-wrapped runs in their preserved order.
    let mut projection_char_offset = 0usize;
    let mut equation_cursor = 0usize;
    let original_runs = project_paragraph_runs(para, input.revision_view);
    let projected_run_count = original_runs.len();
    let mut prepared_runs = Vec::with_capacity(original_runs.len());
    let mut original_indices = Vec::with_capacity(original_runs.len());
    for projected in &original_runs {
        let mut indices = Vec::new();
        for (slot, content) in projected.run.content.iter().enumerate() {
            if let RunContent::Field(field) = content {
                field
                    .validate_for_attachment()
                    .map_err(|error| LayoutError::Layout(error.to_string()))?;
                indices.push((slot, field_indices.get(&std::ptr::from_ref(field)).copied()));
            }
        }
        let prepared = if indices.is_empty() {
            None
        } else {
            let mut run = projected.run.clone();
            for (slot, _) in &indices {
                let RunContent::Field(field) = &mut run.content[*slot] else {
                    unreachable!()
                };
                field
                    .normalize_cached_result_owner()
                    .map_err(|error| LayoutError::Layout(error.to_string()))?;
            }
            Some(run)
        };
        prepared_runs.push(prepared);
        original_indices.push(indices);
    }
    let mut field_indices = field_indices;
    let projected_runs = original_runs
        .iter()
        .enumerate()
        .map(|(index, projected)| {
            let run = prepared_runs[index].as_ref().unwrap_or(projected.run);
            for (slot, source_index) in &original_indices[index] {
                if let (RunContent::Field(field), Some(source_index)) =
                    (&run.content[*slot], source_index)
                {
                    let mut pending = vec![field];
                    let mut next = *source_index;
                    while let Some(field) = pending.pop() {
                        field_indices.insert(std::ptr::from_ref(field), next);
                        next += 1;
                        pending.extend(field.all_nested_fields_in_source_order().into_iter().rev());
                    }
                }
            }
            ProjectedRun { run, ..*projected }
        })
        .collect::<Vec<_>>();
    let mut computed_fields = HashMap::new();
    for projected in &projected_runs {
        for content in &projected.run.content {
            if let RunContent::Field(field) = content {
                let source = source_node
                    .zip(field_indices.get(&std::ptr::from_ref(field)).copied())
                    .map(|(node, index)| FieldSource { node, index });
                let computed = if matches!(
                    projected.revision,
                    Some(RevisionKind::Deletion | RevisionKind::MoveFrom)
                ) {
                    (None, None)
                } else {
                    computed_layout_field(field, source, source_node, input, num_state, diagnostics)
                };
                computed_fields.insert(std::ptr::from_ref(field), computed);
            }
        }
    }
    let mut stored_caches = Vec::with_capacity(projected_runs.len());
    let mut stored_char_lengths = Vec::with_capacity(projected_runs.len());
    for projected in &projected_runs {
        let cache = if let [RunContent::Field(field)] = projected.run.content.as_slice() {
            let source = source_node
                .zip(field_indices.get(&std::ptr::from_ref(field)).copied())
                .map(|(node, index)| FieldSource { node, index });
            let cache = stored_note_cache_runs(
                field,
                projected.run.properties.as_ref(),
                input.revision_view,
                source,
                matches!(
                    projected.revision,
                    Some(RevisionKind::Deletion | RevisionKind::MoveFrom)
                ),
                Some(input),
                false,
            )?;
            stored_char_lengths.push(
                cache
                    .iter()
                    .map(|stored| stored.run.text().chars().count())
                    .sum::<usize>(),
            );
            let has_notes = cache.iter().any(|stored| {
                stored.run.content.iter().any(|content| {
                    matches!(
                        content,
                        RunContent::FootnoteRef { .. } | RunContent::EndnoteRef { .. }
                    )
                })
            });
            let stored = computed_fields
                .get(&std::ptr::from_ref(field))
                .is_none_or(|(value, _)| value.is_none());
            (has_notes || stored).then_some(cache)
        } else {
            stored_char_lengths.push(projected.run.text().chars().count());
            None
        };
        stored_caches.push(cache);
    }
    let mut expanded = Vec::new();
    for (index, (projected, cache)) in projected_runs.into_iter().zip(&stored_caches).enumerate() {
        let start = projection_char_offset;
        projection_char_offset += stored_char_lengths[index];
        if let Some(cache) = cache {
            let mut stored_start = start;
            for (cache_index, stored) in cache.iter().enumerate() {
                expanded.push((
                    index,
                    ProjectedRun {
                        run: &stored.run,
                        force_underline: projected.force_underline || stored.force_underline,
                        force_strike: projected.force_strike || stored.force_strike,
                        revision: stored.revision.or(projected.revision),
                        ..projected
                    },
                    stored_start,
                    true,
                    cache_index == 0,
                    stored.source,
                    stored.field_kind,
                    stored.occurrence,
                ));
                stored_start += stored.run.text().chars().count();
            }
        } else {
            expanded.push((index, projected, start, false, true, None, None, None));
        }
    }
    for (
        projected_index,
        projected,
        projected_run_start,
        stored_cache,
        first,
        stored_source,
        stored_field_kind,
        stored_occurrence,
    ) in expanded
    {
        let run = projected.run;
        if first {
            push_targeted_bookmark_markers(
                &mut inline_items,
                &mut multilingual_styles,
                &current_bookmarks,
                projected_index,
                input,
                fm,
                source_node,
            )?;
        }

        let current_hyperlink_url = projected
            .ordinary_run_index
            .and_then(|run_index| run_hyperlink_url.get(&run_index).cloned())
            .or_else(|| {
                projected
                    .hyperlink_index
                    .and_then(|index| para.hyperlinks.get(index))
                    .and_then(|hyperlink| hyperlink.rel_id.as_deref())
                    .and_then(|rel_id| input.hyperlink_urls.get(rel_id).cloned())
            });

        // Word writes each entry of a table of contents as a hyperlink to a
        // `_Toc` bookmark whose runs carry the Hyperlink character style, and
        // draws the entry in its TOC paragraph style without that style's
        // colour and underline. LibreOffice does the same.
        let toc_entry = toc_paragraph
            && projected
                .hyperlink_index
                .and_then(|index| para.hyperlinks.get(index))
                .or_else(|| {
                    let run_index = projected.ordinary_run_index?;
                    para.hyperlinks.iter().find(|hyperlink| {
                        (hyperlink.run_start..hyperlink.run_end).contains(&run_index)
                    })
                })
                .and_then(|hyperlink| hyperlink.anchor.as_deref())
                .is_some_and(|anchor| anchor.starts_with("_Toc"));
        let run_style_id = run
            .properties
            .as_ref()
            .and_then(|p| p.style_id.as_deref())
            .filter(|style_id| !(toc_entry && is_hyperlink_character_style(style_id, styles)));

        let resolved_rpr = style_resolver::resolve_run_properties(
            para_style_id,
            run_style_id,
            styles,
            table_run_properties,
        );

        // Merge direct run properties
        let mut effective_rpr = resolved_rpr;
        if let Some(ref direct_rpr) = run.properties {
            effective_rpr.merge_from(direct_rpr);
        }

        push_equations_before_order(
            para,
            projected.boundary,
            projected.raw_order,
            &mut equation_cursor,
            &mut inline_items,
            fm,
            effective_rpr.sz.map(|hp| hp.to_pt()).unwrap_or(11.0),
            resolve_run_color(&effective_rpr, input.theme.as_ref(), automatic_color),
            available_width,
            input.math_properties.as_ref(),
            diagnostics,
        )?;

        // A ruby annotation owns its base runs, so the span is laid out once
        // at its first run and the rest of the span is skipped rather than
        // painted a second time on the ordinary run path.
        if let Some(run_index) = projected.ordinary_run_index
            && let Some(ruby) = para
                .rubies
                .iter()
                .find(|ruby| ruby.base_range().contains(&run_index))
        {
            if ruby.base_start == run_index
                && let Some(base_runs) = para.runs.get(ruby.base_range())
                && let Some(item) = ruby_inline_item(
                    ruby,
                    base_runs,
                    &AnnotationContext {
                        styles,
                        input,
                        para_style_id,
                        table_run_properties,
                        automatic_color,
                    },
                    fm,
                )?
            {
                inline_items.push(item);
            }
            continue;
        }

        // Skip hidden text
        if effective_rpr.vanish == Some(true) {
            continue;
        }

        let mut font_size = effective_rpr.sz.map(|hp| hp.to_pt()).unwrap_or(11.0);
        let bold = effective_rpr.bold.unwrap_or(false);
        let italic = effective_rpr.italic.unwrap_or(false);

        // Resolve font family: theme font takes priority when no explicit font is set
        let run_text = run.text();
        let font_family = resolve_font_family(
            &effective_rpr,
            input.theme.as_ref(),
            word_font_slot_for_text(&run_text, effective_rpr.font_hint.as_deref()),
        );

        // Resolve color: theme color takes priority over literal color value
        let (color, underline) = tracked_marks(
            &projected,
            input.revision_style,
            resolve_run_color(&effective_rpr, input.theme.as_ref(), automatic_color),
            convert::underline(effective_rpr.underline),
        );
        let strike = projected.force_strike || effective_rpr.strike.unwrap_or(false);
        let dstrike = effective_rpr.dstrike.unwrap_or(false);
        let highlight = effective_rpr.highlight.and_then(highlight_to_color);

        // Superscript/subscript handling
        let mut baseline_offset = 0.0;
        if let Some(ref va) = effective_rpr.vert_align {
            match va.as_str() {
                "superscript" => {
                    // Reduce font size to ~58% and raise baseline
                    let original_size = font_size;
                    font_size *= 0.58;
                    baseline_offset = original_size * 0.33; // raise by 1/3 of original size
                }
                "subscript" => {
                    // Reduce font size to ~58% and lower baseline
                    let original_size = font_size;
                    font_size *= 0.58;
                    baseline_offset = -(original_size * 0.14); // lower
                }
                _ => {}
            }
        }

        // Position offset (in half-points, positive=raise)
        if let Some(pos) = effective_rpr.position {
            baseline_offset += pos as f64 / 2.0; // half-points to points
        }

        // Resolved against the run's own text, so a family without glyphs for
        // this script is replaced by one that has them.
        let font_id = fm.resolve_font_for_text(font_family.as_deref(), bold, italic, &run_text)?;
        let metrics = fm.metrics(font_id, font_size)?;

        // Stored-display expansion replaces the sole Field run with its
        // cache runs. Its instruction events still belong before that display.
        if stored_cache && first {
            let owner_run = prepared_runs[projected_index]
                .as_ref()
                .unwrap_or(original_runs[projected_index].run);
            if let [RunContent::Field(field)] = owner_run.content.as_slice() {
                push_sequence_context_markers(
                    field,
                    input.sequence_snapshot.as_deref().zip(source_node),
                    &field_indices,
                    font_id,
                    &mut inline_items,
                    &mut multilingual_styles,
                );
            }
        }
        let run_inline_start = inline_items.len();
        let content_char_starts = projected_content_char_starts(run);
        let note_events = if num_state.note_body_context.is_some() {
            crate::notes::run_note_events(run)?
        } else {
            Vec::new()
        };
        for (content_index, content) in run
            .content
            .iter()
            .map(Some)
            .chain(std::iter::once(None))
            .enumerate()
        {
            for (_, event) in note_events
                .iter()
                .filter(|(position, _)| *position == content_index)
            {
                if matches!(event.as_str(), "separator" | "continuationSeparator") {
                    let width = available_width
                        * if event == "continuationSeparator" {
                            1.0
                        } else {
                            crate::notes::SEPARATOR_WIDTH_FRACTION
                        };
                    inline_items.push(InlineItem::Group {
                        width,
                        height: 0.0,
                        baseline: None,
                        group: GroupElement {
                            transform: Transform::IDENTITY,
                            clip: None,
                            opacity: 1.0,
                            effects: Vec::new(),
                            children: vec![PositionedElement::Line {
                                start: Point { x: 0.0, y: 0.0 },
                                end: Point { x: width, y: 0.0 },
                                width: 0.5,
                                color,
                                dash_pattern: None,
                            }],
                        },
                    });
                } else if let Some(note) = num_state.note_body_context
                    && ((note.stream == NoteStream::Footnote && event == "footnoteRef")
                        || (note.stream == NoteStream::Endnote && event == "endnoteRef"))
                {
                    let text = num_state.note_label(note);
                    let (size, rise) = if matches!(
                        effective_rpr.vert_align.as_deref(),
                        Some("superscript" | "subscript")
                    ) {
                        (font_size, baseline_offset)
                    } else {
                        (font_size * 0.58, font_size * 0.33 + baseline_offset)
                    };
                    let marker_font =
                        fm.resolve_font_for_text(font_family.as_deref(), bold, italic, &text)?;
                    let shaped = fm.shape_text_with_kerning(
                        marker_font,
                        &text,
                        size,
                        style_resolver::word_kerning_enabled(&effective_rpr),
                    )?;
                    let marker_metrics = fm.metrics(marker_font, size)?;
                    inline_items.push(InlineItem::Text(TextSegment {
                        text,
                        direction: TextDirection::Auto,
                        source: None,
                        font_id: marker_font,
                        font_size: size,
                        glyph_ids: shaped.glyph_ids,
                        advances: shaped.advances,
                        width: shaped.width,
                        ascent: marker_metrics.ascent,
                        descent: marker_metrics.descent,
                        line_gap: 0.0,
                        color,
                        bold,
                        italic,
                        underline,
                        strike,
                        dstrike,
                        highlight,
                        baseline_offset: rise,
                        hyperlink_url: current_hyperlink_url.clone(),
                        field_kind: None,
                        field_source: None,
                        note: None,
                        note_reference_source: None,
                    }));
                }
            }
            let Some(content) = content else {
                break;
            };
            let content_char_start = projected_run_start + content_char_starts[content_index];
            match content {
                RunContent::Text(ct_text) | RunContent::DeletedText(ct_text) => {
                    let text = if effective_rpr.caps == Some(true) {
                        ct_text.text.to_uppercase()
                    } else {
                        ct_text.text.clone()
                    };

                    if text.is_empty() && stored_source.is_none() && stored_field_kind.is_none() {
                        continue;
                    }

                    let source = if !stored_cache && text == ct_text.text {
                        source_node.and_then(|node| {
                            let char_start = u32::try_from(content_char_start).ok()?;
                            let char_end =
                                u32::try_from(content_char_start + ct_text.text.chars().count())
                                    .ok()?;
                            Some(SourceSpan {
                                node,
                                char_start,
                                char_end,
                            })
                        })
                    } else {
                        None
                    };

                    let annotation_base = AnnotationBase {
                        fallback_emphasis: effective_rpr.emphasis_mark.as_ref(),
                        occurrence: stored_occurrence,
                        field_kind: stored_field_kind,
                        kern: style_resolver::word_kerning_enabled(&effective_rpr),
                        font_id,
                        font_size,
                        color,
                        bold,
                        italic,
                        baseline_offset,
                        spacing: word_character_advance_pt(&effective_rpr, grid_char_space_pt),
                        underline,
                        strike,
                        dstrike,
                        highlight,
                        font_family: font_family.as_deref(),
                    };

                    if text.is_empty() && stored_occurrence.is_some() {
                        let kind = effective_rpr
                            .east_asian_layout
                            .as_deref()
                            .filter(|layout| {
                                layout.combine == Some(true) || layout.vert == Some(true)
                            })
                            .map(|layout| WordAnnotationKind::EastAsian(layout.clone()))
                            .or_else(|| {
                                effective_rpr
                                    .emphasis_mark
                                    .as_ref()
                                    .filter(|mark| emphasis_mark_glyph(mark).is_some())
                                    .map(|mark| WordAnnotationKind::Emphasis(mark.clone()))
                            });
                        if let Some(kind) = kind {
                            push_empty_field_annotation(
                                &mut inline_items,
                                &mut multilingual_styles,
                                kind,
                                &annotation_base,
                                source,
                            )?;
                            continue;
                        }
                    }

                    if let Some(layout) = effective_rpr.east_asian_layout.as_deref()
                        && push_east_asian_layout_text(
                            &mut inline_items,
                            &mut multilingual_styles,
                            fm,
                            layout,
                            &text,
                            &annotation_base,
                            source,
                        )?
                    {
                        continue;
                    }

                    if let Some(ref mark) = effective_rpr.emphasis_mark
                        && push_emphasis_marked_text(
                            &mut inline_items,
                            &mut multilingual_styles,
                            fm,
                            diagnostics,
                            mark,
                            &text,
                            &annotation_base,
                        )?
                    {
                        continue;
                    }

                    let mut shaped = fm.shape_text_with_kerning(
                        font_id,
                        &text,
                        font_size,
                        style_resolver::word_kerning_enabled(&effective_rpr),
                    )?;

                    // Apply character spacing from run properties, plus the
                    // section character grid when one is in force.
                    let extra = word_character_advance_pt(&effective_rpr, grid_char_space_pt);
                    if extra != 0.0 {
                        for advance in &mut shaped.advances {
                            *advance += extra;
                        }
                        shaped.width += extra * shaped.advances.len() as f64;
                    }

                    let segment = TextSegment {
                        text,
                        direction: TextDirection::Auto,
                        source,
                        font_id,
                        font_size,
                        glyph_ids: shaped.glyph_ids,
                        advances: shaped.advances,
                        width: shaped.width,
                        ascent: metrics.ascent,
                        descent: metrics.descent,
                        line_gap: 0.0,
                        color,
                        bold,
                        italic,
                        underline,
                        strike,
                        dstrike,
                        highlight,
                        baseline_offset,
                        hyperlink_url: current_hyperlink_url.clone(),
                        field_kind: stored_field_kind,
                        field_source: stored_source,
                        note: None,
                        note_reference_source: None,
                    };
                    let item_index = inline_items.len();
                    multilingual_styles.insert(
                        item_index,
                        WordMultilingualStyle {
                            occurrence: stored_occurrence,
                            annotation: None,
                            kern: style_resolver::word_kerning_enabled(&effective_rpr),
                            origin: if stored_field_kind.is_some() {
                                WordInputOrigin::PlainField
                            } else {
                                WordInputOrigin::Text
                            },
                            requested_family: fm.copy_font_request_context(
                                font_family.as_deref(),
                                &mut requested_context_bytes,
                                &mut requested_context_entries,
                            ),
                            language: effective_rpr.language.clone(),
                            language_east_asia: effective_rpr.language_east_asia.clone(),
                            language_bidi: effective_rpr.language_bidi.clone(),
                            direction: word_text_direction(effective_rpr.rtl),
                            spacing: word_character_advance_pt(&effective_rpr, grid_char_space_pt),
                        },
                    );
                    if automatic_hyphenation
                        && !stored_cache
                        && let Some(language) = effective_rpr.language.as_ref()
                    {
                        inline_items.push(InlineItem::HyphenatedText {
                            segment,
                            language: language.clone(),
                        });
                    } else {
                        inline_items.push(InlineItem::Text(segment));
                    }
                }
                RunContent::Tab => {
                    inline_items.push(InlineItem::Tab);
                }
                // Word drops a page or column break inside a table cell, so
                // the text on either side of it joins on one line.
                RunContent::Break(bt) => match bt {
                    BreakType::Line => inline_items.push(InlineItem::LineBreak),
                    BreakType::Page | BreakType::Column if in_table_cell => {}
                    BreakType::Page => inline_items.push(InlineItem::PageBreak),
                    BreakType::Column => inline_items.push(InlineItem::ColumnBreak),
                },
                RunContent::Drawing(drawing) => {
                    if let Some(ref inline) = drawing.inline {
                        let width = inline.extent_cx.to_pt();
                        let height = inline.extent_cy.to_pt();
                        let item = if let Some(relationship_id) = inline.chart_rel_id.as_deref() {
                            InlineItem::Group {
                                width,
                                height,
                                baseline: None,
                                group: render_word_chart(
                                    relationship_id,
                                    width,
                                    height,
                                    input,
                                    fm,
                                    diagnostics,
                                )?,
                            }
                        } else {
                            media.record_picture_draw_owner(&inline.embed_id);
                            InlineItem::Image {
                                width,
                                height,
                                media_id: media.id_for_relationship_with_diagnostic(
                                    &inline.embed_id,
                                    diagnostics,
                                ),
                            }
                        };
                        if let Some(alternate_text) = inline
                            .description
                            .as_deref()
                            .map(str::trim)
                            .filter(|text| !text.is_empty())
                        {
                            inline_items.push(InlineItem::Figure {
                                item: Box::new(item),
                                alternate_text: alternate_text.to_owned(),
                                structure_id: None,
                            });
                        } else {
                            inline_items.push(item);
                        }
                    }
                }
                RunContent::Field(field) => {
                    push_sequence_context_markers(
                        field,
                        input.sequence_snapshot.as_deref().zip(source_node),
                        &field_indices,
                        font_id,
                        &mut inline_items,
                        &mut multilingual_styles,
                    );
                    let (computed_value, field_kind) = computed_fields
                        .get(&std::ptr::from_ref(field))
                        .cloned()
                        .unwrap_or((None, None));
                    let stored_segments = field.effective_display_field_segments();
                    // A legacy checkbox without formatted result runs takes the
                    // formatting of the run that opens it.
                    let checkbox_properties =
                        drawn_legacy_checkbox(field).and_then(|_| field.begin_run_properties());
                    let segments: Vec<(
                        Cow<'_, str>,
                        Option<&CT_RPr>,
                        &oxoffice_docx_oxml::text::Field,
                    )> = if let Some(value) = computed_value.as_deref() {
                        let stored_properties = stored_segments
                            .first()
                            .and_then(|(_, properties, _)| *properties)
                            .or(checkbox_properties.as_ref());
                        vec![(Cow::Borrowed(value), stored_properties, field)]
                    } else {
                        stored_segments
                    };
                    for (value, stored_properties, display_owner) in segments {
                        let value = value.as_ref();
                        let segment_field_source = source_node
                            .zip(
                                field_indices
                                    .get(&std::ptr::from_ref(display_owner))
                                    .copied(),
                            )
                            .map(|(node, index)| FieldSource { node, index });
                        let segment_field_kind = if std::ptr::eq(display_owner, field) {
                            field_kind
                        } else if field.cached_display_owner_is_locked(display_owner) {
                            None
                        } else {
                            match display_owner.instruction.name.as_str() {
                                "PAGE" => Some(FieldKind::Page),
                                "NUMPAGES" => Some(FieldKind::NumPages),
                                "SECTION" => Some(FieldKind::Section),
                                "SECTIONPAGES" => Some(FieldKind::SectionPages),
                                "PAGEREF" => field_text_argument(&display_owner.instruction, 0)
                                    .and_then(|name| page_ref_id(input, name))
                                    .map(FieldKind::TargetPage),
                                _ => None,
                            }
                        };
                        let segment_style_id = stored_properties
                            .and_then(|properties| properties.style_id.as_deref())
                            .filter(|style_id| {
                                !(toc_entry && is_hyperlink_character_style(style_id, styles))
                            });
                        let mut segment_rpr = if stored_properties.is_some() {
                            style_resolver::resolve_run_properties(
                                para_style_id,
                                segment_style_id,
                                styles,
                                table_run_properties,
                            )
                        } else {
                            effective_rpr.clone()
                        };
                        if let Some(properties) = stored_properties {
                            segment_rpr.merge_from(properties);
                        }
                        if computed_value.is_some() && drawn_legacy_checkbox(field).is_some() {
                            // The box's own size, unless automatic.
                            if let Some(oxoffice_docx_oxml::text::LegacyCheckBoxSize::HalfPoints(
                                size,
                            )) = field
                                .legacy_form
                                .as_ref()
                                .and_then(|data| data.checkbox_size)
                            {
                                let size = oxoffice_docx_oxml::HalfPoint(
                                    u32::try_from(size).unwrap_or(u32::MAX),
                                );
                                segment_rpr.sz = Some(size);
                                segment_rpr.sz_cs = Some(size);
                            }
                        }
                        if segment_rpr.vanish == Some(true) {
                            continue;
                        }
                        let mut segment_font_size =
                            segment_rpr.sz.map(|hp| hp.to_pt()).unwrap_or(11.0);
                        let segment_bold = segment_rpr.bold.unwrap_or(false);
                        let segment_italic = segment_rpr.italic.unwrap_or(false);
                        let segment_font_family = resolve_font_family(
                            &segment_rpr,
                            input.theme.as_ref(),
                            word_font_slot_for_text(value, segment_rpr.font_hint.as_deref()),
                        );
                        let (segment_color, segment_underline) = tracked_marks(
                            &projected,
                            input.revision_style,
                            resolve_run_color(&segment_rpr, input.theme.as_ref(), automatic_color),
                            convert::underline(segment_rpr.underline),
                        );
                        let segment_strike =
                            projected.force_strike || segment_rpr.strike.unwrap_or(false);
                        let segment_dstrike = segment_rpr.dstrike.unwrap_or(false);
                        let segment_highlight = segment_rpr.highlight.and_then(highlight_to_color);
                        let mut segment_baseline_offset = 0.0;
                        if let Some(vertical) = segment_rpr.vert_align.as_deref() {
                            match vertical {
                                "superscript" => {
                                    let original_size = segment_font_size;
                                    segment_font_size *= 0.58;
                                    segment_baseline_offset = original_size * 0.33;
                                }
                                "subscript" => {
                                    let original_size = segment_font_size;
                                    segment_font_size *= 0.58;
                                    segment_baseline_offset = -(original_size * 0.14);
                                }
                                _ => {}
                            }
                        }
                        if let Some(position) = segment_rpr.position {
                            segment_baseline_offset += position as f64 / 2.0;
                        }
                        let segment_font_id = fm.resolve_font_for_text(
                            segment_font_family.as_deref(),
                            segment_bold,
                            segment_italic,
                            value,
                        )?;
                        let segment_metrics = fm.metrics(segment_font_id, segment_font_size)?;

                        let mut start = 0usize;
                        for (index, character) in value
                            .char_indices()
                            .chain(std::iter::once((value.len(), '\0')))
                        {
                            let control = match character {
                                '\t' => Some(InlineItem::Tab),
                                '\n' => Some(InlineItem::LineBreak),
                                '\u{000c}' => Some(InlineItem::PageBreak),
                                '\u{000b}' => Some(InlineItem::ColumnBreak),
                                '\0' if index == value.len() => None,
                                _ => continue,
                            };
                            if start < index
                                || (value.is_empty()
                                    && segment_field_kind == Some(FieldKind::SequenceRepeat))
                            {
                                let mut text = value[start..index].to_owned();
                                if segment_rpr.caps == Some(true) {
                                    text = text.to_uppercase();
                                }
                                let mut shaped = fm.shape_text_with_kerning(
                                    segment_font_id,
                                    &text,
                                    segment_font_size,
                                    style_resolver::word_kerning_enabled(&segment_rpr),
                                )?;
                                let extra =
                                    word_character_advance_pt(&segment_rpr, grid_char_space_pt);
                                if extra != 0.0 {
                                    for advance in &mut shaped.advances {
                                        *advance += extra;
                                    }
                                    shaped.width += extra * shaped.advances.len() as f64;
                                }
                                let item_index = inline_items.len();
                                multilingual_styles.insert(
                                    item_index,
                                    WordMultilingualStyle {
                                        occurrence: None,
                                        annotation: None,
                                        kern: style_resolver::word_kerning_enabled(&segment_rpr),
                                        origin: if segment_field_kind.is_some() {
                                            WordInputOrigin::PlainField
                                        } else {
                                            WordInputOrigin::Text
                                        },
                                        requested_family: fm.copy_font_request_context(
                                            segment_font_family.as_deref(),
                                            &mut requested_context_bytes,
                                            &mut requested_context_entries,
                                        ),
                                        language: segment_rpr.language.clone(),
                                        language_east_asia: segment_rpr.language_east_asia.clone(),
                                        language_bidi: segment_rpr.language_bidi.clone(),
                                        direction: word_text_direction(segment_rpr.rtl),
                                        spacing: word_character_advance_pt(
                                            &segment_rpr,
                                            grid_char_space_pt,
                                        ),
                                    },
                                );
                                inline_items.push(InlineItem::Text(TextSegment {
                                    text,
                                    direction: word_text_direction(segment_rpr.rtl),
                                    source: None,
                                    font_id: segment_font_id,
                                    font_size: segment_font_size,
                                    glyph_ids: shaped.glyph_ids,
                                    advances: shaped.advances,
                                    width: shaped.width,
                                    ascent: segment_metrics.ascent,
                                    descent: segment_metrics.descent,
                                    line_gap: 0.0,
                                    color: segment_color,
                                    bold: segment_bold,
                                    italic: segment_italic,
                                    underline: segment_underline,
                                    strike: segment_strike,
                                    dstrike: segment_dstrike,
                                    highlight: segment_highlight,
                                    baseline_offset: segment_baseline_offset,
                                    hyperlink_url: current_hyperlink_url.clone(),
                                    field_kind: segment_field_kind,
                                    field_source: segment_field_source,
                                    note: None,
                                    note_reference_source: None,
                                }));
                            }
                            if let Some(control) = control {
                                let dropped = in_table_cell
                                    && matches!(
                                        control,
                                        InlineItem::PageBreak | InlineItem::ColumnBreak
                                    );
                                if !dropped {
                                    inline_items.push(control);
                                }
                                start = index + character.len_utf8();
                            }
                        }
                    }
                }
                RunContent::FootnoteRef { id, .. } | RunContent::EndnoteRef { id, .. } => {
                    // The two streams number independently, so the marker has
                    // to carry which one it came from.
                    let stream = match content {
                        RunContent::EndnoteRef { .. } => NoteStream::Endnote,
                        _ => NoteStream::Footnote,
                    };
                    // Render as superscript number
                    let marker = num_state
                        .note_label(NoteRef { stream, id: *id })
                        .to_string();
                    // Explicit vertical alignment has already resolved size and
                    // rise above. Default note superscripting must happen once,
                    // while retaining an explicit half-point position shift.
                    let (sup_size, sup_offset) = if matches!(
                        effective_rpr.vert_align.as_deref(),
                        Some("superscript" | "subscript")
                    ) {
                        (font_size, baseline_offset)
                    } else {
                        (font_size * 0.58, font_size * 0.33 + baseline_offset)
                    };
                    let shaped = fm.shape_text_with_kerning(
                        font_id,
                        &marker,
                        sup_size,
                        style_resolver::word_kerning_enabled(&effective_rpr),
                    )?;
                    let sup_metrics = fm.metrics(font_id, sup_size)?;
                    let revision_marker = input.revision_view == RevisionView::Tracked
                        && projected.ordinary_run_index.is_none();
                    inline_items.push(InlineItem::Text(TextSegment {
                        text: marker,
                        direction: TextDirection::Auto,
                        source: None,
                        font_id,
                        font_size: sup_size,
                        glyph_ids: shaped.glyph_ids,
                        advances: shaped.advances,
                        width: shaped.width,
                        ascent: sup_metrics.ascent,
                        descent: sup_metrics.descent,
                        line_gap: 0.0,
                        color,
                        bold,
                        italic,
                        underline: revision_marker.then_some(underline).flatten(),
                        strike: revision_marker && strike,
                        dstrike: revision_marker && dstrike,
                        highlight: revision_marker.then_some(highlight).flatten(),
                        baseline_offset: sup_offset,
                        hyperlink_url: None,
                        field_kind: None,
                        field_source: None,
                        note: Some(NoteRef { stream, id: *id }),
                        note_reference_source: source_node.and_then(|node| {
                            let offset = u32::try_from(if stored_cache {
                                projected_run_start
                            } else {
                                content_char_start
                            })
                            .ok()?;
                            Some(SourceSpan {
                                node,
                                char_start: offset,
                                char_end: offset,
                            })
                        }),
                    }));
                }
                RunContent::Symbol { font, char_code } => {
                    // The code point is the symbol font's own, typically in
                    // the F020 to F0FF private-use block Word writes. Font
                    // resolution runs against that text so a family without
                    // the glyph is replaced by one that has it.
                    let Some(symbol) = char::from_u32(u32::from(*char_code)) else {
                        continue;
                    };
                    let (text, symbol_font_id) = fm.resolve_word_symbol_text(
                        Some(font.as_str()),
                        bold,
                        italic,
                        &symbol.to_string(),
                        false,
                    )?;
                    let symbol_metrics = fm.metrics(symbol_font_id, font_size)?;
                    let shaped = fm.shape_text_with_kerning(
                        symbol_font_id,
                        &text,
                        font_size,
                        style_resolver::word_kerning_enabled(&effective_rpr),
                    )?;
                    for message in fm.unknown_word_symbol_diagnostics(
                        symbol_font_id,
                        Some(font),
                        &symbol.to_string(),
                        &text,
                    ) {
                        push_unique_diagnostic(diagnostics, message);
                    }
                    multilingual_styles.insert(
                        inline_items.len(),
                        WordMultilingualStyle {
                            occurrence: stored_occurrence,
                            annotation: None,
                            kern: style_resolver::word_kerning_enabled(&effective_rpr),
                            origin: if stored_field_kind.is_some() {
                                WordInputOrigin::PlainField
                            } else {
                                WordInputOrigin::Text
                            },
                            requested_family: fm.copy_font_request_context(
                                Some(font),
                                &mut requested_context_bytes,
                                &mut requested_context_entries,
                            ),
                            ..Default::default()
                        },
                    );
                    inline_items.push(InlineItem::Text(TextSegment {
                        text,
                        direction: TextDirection::Auto,
                        source: None,
                        font_id: symbol_font_id,
                        font_size,
                        glyph_ids: shaped.glyph_ids,
                        advances: shaped.advances,
                        width: shaped.width,
                        ascent: symbol_metrics.ascent,
                        descent: symbol_metrics.descent,
                        line_gap: 0.0,
                        color,
                        bold,
                        italic,
                        underline,
                        strike,
                        dstrike,
                        highlight,
                        baseline_offset,
                        hyperlink_url: current_hyperlink_url.clone(),
                        field_kind: None,
                        field_source: None,
                        note: None,
                        note_reference_source: None,
                    }));
                }
                RunContent::SpecialCharacter(character) => match character {
                    SpecialCharacter::CarriageReturn => {
                        inline_items.push(InlineItem::LineBreak);
                    }
                    SpecialCharacter::NoBreakHyphen => {
                        // U+2011 carries Unicode line-break class GL, so the
                        // hyphen is drawn without becoming a break
                        // opportunity. A plain U+002D would become one.
                        let text = "\u{2011}".to_owned();
                        let shaped = fm.shape_text_with_kerning(
                            font_id,
                            &text,
                            font_size,
                            style_resolver::word_kerning_enabled(&effective_rpr),
                        )?;
                        inline_items.push(InlineItem::Text(TextSegment {
                            text,
                            direction: TextDirection::Auto,
                            source: None,
                            font_id,
                            font_size,
                            glyph_ids: shaped.glyph_ids,
                            advances: shaped.advances,
                            width: shaped.width,
                            ascent: metrics.ascent,
                            descent: metrics.descent,
                            line_gap: 0.0,
                            color,
                            bold,
                            italic,
                            underline,
                            strike,
                            dstrike,
                            highlight,
                            baseline_offset,
                            hyperlink_url: current_hyperlink_url.clone(),
                            field_kind: None,
                            field_source: None,
                            note: None,
                            note_reference_source: None,
                        }));
                    }
                    // A soft hyphen is drawn only on the line it breaks, and
                    // the line breaker has no discretionary-break input, so it
                    // is round-tripped without a render projection.
                    SpecialCharacter::SoftHyphen => {}
                    // The absolute position is a paragraph-relative tab stop
                    // the tab resolver already places.
                    SpecialCharacter::PositionalTab { .. } => {
                        inline_items.push(InlineItem::Tab);
                    }
                },
                RunContent::CommentReference { .. } => {}
            }
        }
        for index in run_inline_start..inline_items.len() {
            multilingual_styles
                .entry(index)
                .or_insert_with(|| WordMultilingualStyle {
                    kern: style_resolver::word_kerning_enabled(&effective_rpr),
                    requested_family: fm.copy_font_request_context(
                        font_family.as_deref(),
                        &mut requested_context_bytes,
                        &mut requested_context_entries,
                    ),
                    ..Default::default()
                });
        }
    }

    let mut equation_rpr =
        style_resolver::resolve_run_properties(para_style_id, None, styles, table_run_properties);
    if let Some(paragraph_mark_rpr) = direct_ppr.and_then(|ppr| ppr.rpr.as_ref()) {
        equation_rpr.merge_from(paragraph_mark_rpr);
    }
    push_equations_before_order(
        para,
        usize::MAX,
        RawOrder::AfterRaw,
        &mut equation_cursor,
        &mut inline_items,
        fm,
        equation_rpr.sz.map(|hp| hp.to_pt()).unwrap_or(11.0),
        resolve_run_color(&equation_rpr, input.theme.as_ref(), automatic_color),
        available_width,
        input.math_properties.as_ref(),
        diagnostics,
    )?;

    push_targeted_bookmark_markers(
        &mut inline_items,
        &mut multilingual_styles,
        &current_bookmarks,
        projected_run_count,
        input,
        fm,
        source_node,
    )?;

    let attributed_empty_paragraph = inline_items.is_empty()
        || inline_items.iter().enumerate().all(|(index, _)| {
            multilingual_styles
                .get(&index)
                .and_then(|style| style.annotation.as_ref())
                .is_some_and(|annotation| annotation.kind == WordAnnotationRootKind::EmptySpan)
        });
    if attributed_empty_paragraph {
        let mut caret_rpr = style_resolver::resolve_run_properties(
            para_style_id,
            None,
            styles,
            table_run_properties,
        );
        if let Some(paragraph_mark_rpr) = direct_ppr.and_then(|ppr| ppr.rpr.as_ref()) {
            caret_rpr.merge_from(paragraph_mark_rpr);
        }
        let font_size = caret_rpr.sz.map(|hp| hp.to_pt()).unwrap_or(11.0);
        let bold = caret_rpr.bold.unwrap_or(false);
        let italic = caret_rpr.italic.unwrap_or(false);
        // The caret of an empty paragraph draws no character, so it has no
        // slot of its own and keeps the ASCII one it has always used.
        let font_family =
            resolve_font_family(&caret_rpr, input.theme.as_ref(), WordFontSlot::Ascii);
        let font_id = fm.resolve_font_for_metrics(font_family.as_deref(), bold, italic)?;
        let metrics = fm.metrics(font_id, font_size)?;
        if inline_items.is_empty() {
            multilingual_styles.insert(
                inline_items.len(),
                WordMultilingualStyle {
                    kern: style_resolver::word_kerning_enabled(&caret_rpr),
                    ..Default::default()
                },
            );
            inline_items.push(InlineItem::Text(TextSegment {
                text: String::new(),
                direction: TextDirection::Auto,
                source: source_node.map(|node| SourceSpan {
                    node,
                    char_start: 0,
                    char_end: 0,
                }),
                font_id,
                font_size,
                glyph_ids: Vec::new(),
                advances: Vec::new(),
                width: 0.0,
                ascent: metrics.ascent,
                descent: metrics.descent,
                line_gap: 0.0,
                color: resolve_run_color(&caret_rpr, input.theme.as_ref(), automatic_color),
                bold,
                italic,
                underline: None,
                strike: false,
                dstrike: false,
                highlight: None,
                baseline_offset: 0.0,
                hyperlink_url: None,
                field_kind: None,
                field_source: None,
                note: None,
                note_reference_source: None,
            }));
        } else {
            // These atomic roots stand in for the ordinary empty caret. A
            // baseline-bearing Group retains its supplied metrics during Word
            // restoration, so include the same external leading here. Exact
            // spacing retains the line breaker's original font metrics.
            let (ascent, descent) = if matches!(
                convert::line_spacing(&effective_ppr),
                oxoffice_layout::LineSpacing::Exact(_)
            ) {
                (metrics.ascent, metrics.descent)
            } else {
                let word = fm.word_line_metrics(font_id, font_size)?;
                (word.ascent + word.line_gap, word.descent)
            };
            for item in &mut inline_items {
                let InlineItem::Group {
                    height,
                    baseline,
                    group,
                    ..
                } = item
                else {
                    return Err(LayoutError::Layout(
                        "empty annotation lost its atomic root".into(),
                    ));
                };
                *height = ascent + descent;
                *baseline = Some(ascent);
                let [PositionedElement::Text(base)] = group.children.as_mut_slice() else {
                    return Err(LayoutError::Layout(
                        "empty annotation lost its base leaf".into(),
                    ));
                };
                base.origin.y = ascent;
            }
        }
    }

    let layout_direction = match base_direction {
        TextDirection::Auto => inferred_word_base_direction(&inline_items),
        direction => direction,
    };
    if let Some(reflow_direction_out) = reflow_direction_out {
        *reflow_direction_out = layout_direction;
    }
    let logical_left = match layout_direction {
        TextDirection::RightToLeft => effective_ppr.ind_end,
        TextDirection::Auto | TextDirection::LeftToRight => effective_ppr.ind_start,
    };
    let logical_right = match layout_direction {
        TextDirection::RightToLeft => effective_ppr.ind_start,
        TextDirection::Auto | TextDirection::LeftToRight => effective_ppr.ind_end,
    };
    let ind_left = effective_ppr
        .ind_left
        .or(logical_left)
        .map(|t| t.to_pt())
        .unwrap_or(0.0);
    let ind_right = effective_ppr
        .ind_right
        .or(logical_right)
        .map(|t| t.to_pt())
        .unwrap_or(0.0);
    let jc = convert::alignment_for_direction(effective_ppr.jc, layout_direction);

    // Line breaking
    let mut line_params =
        convert::line_break_params(&effective_ppr, available_width, input.default_tab_stop);
    line_params.clamp_tabs_past_margin = input.clamp_tabs_past_margin;
    line_params.ind_left = ind_left;
    line_params.ind_right = ind_right;
    line_params.jc = jc;

    // A line holding only inline objects takes its proportional spacing from
    // the paragraph mark, whose run properties `equation_rpr` already are. A
    // mark whose font does not resolve adds none rather than failing a
    // paragraph whose runs all resolved.
    let paragraph_mark = {
        let family = resolve_font_family(&equation_rpr, input.theme.as_ref(), WordFontSlot::Ascii);
        fm.resolve_font_for_metrics(
            family.as_deref(),
            equation_rpr.bold.unwrap_or(false),
            equation_rpr.italic.unwrap_or(false),
        )
        .and_then(|font_id| {
            fm.word_line_metrics(font_id, equation_rpr.sz.map_or(11.0, |hp| hp.to_pt()))
        })
        .ok()
    };
    // An exact `w:lineRule` is an author's absolute height and stays off the
    // grid, which is what Word does with the two together.
    let grid_line_pitch_pt =
        grid_line_pitch_pt.filter(|_| effective_ppr.line_rule.as_deref() != Some("exact"));

    let uses_multilingual_layout = base_direction != TextDirection::Auto
        || multilingual_styles
            .values()
            .any(|style| style.direction != TextDirection::Auto)
        || inline_items.iter().any(|item| {
            multilingual_candidate(item)
                .is_some_and(|segment| needs_word_multilingual_layout(&segment.text))
        });
    let uses_exact_word_baseline = uses_multilingual_layout
        && effective_ppr.line_rule.as_deref() == Some("exact")
        && effective_ppr.line_spacing.is_some();
    let structural_count = multilingual_styles
        .values()
        .filter(|style| style.origin == WordInputOrigin::Structural)
        .count();
    let mut producers = Vec::new();
    let mut language_bytes = None;
    let mut uniform_language_bytes = true;
    for (index, item) in inline_items.iter().enumerate() {
        if let InlineItem::Text(_) | InlineItem::Marker(_) | InlineItem::HyphenatedText { .. } =
            item
        {
            let style = multilingual_styles
                .get(&index)
                .ok_or_else(|| LayoutError::Layout("Word text lacks producer proof".into()))?;
            let language = match item {
                InlineItem::HyphenatedText { language, .. } => Some(language.clone()),
                _ => None,
            };
            let bytes = language.as_ref().map_or(0, String::len);
            if language_bytes.is_some_and(|previous| previous != bytes) {
                uniform_language_bytes = false;
            }
            language_bytes = Some(bytes);
            producers.push(WordLegacyProducer {
                kern: style.kern,
                language,
                structural: style.origin == WordInputOrigin::Structural,
                atomic_marker: style.origin == WordInputOrigin::AtomicMarker,
            });
        }
    }
    let mut paint = WordParagraphPaint {
        rich_layout: uses_multilingual_layout,
        plain_field_policy: Some((false, true)),
        structural_count,
        legacy_cache: (!uses_multilingual_layout && uniform_language_bytes).then_some(
            WordLegacyCache {
                language_bytes_per_span: language_bytes.unwrap_or(0),
                producers,
            },
        ),
        ..Default::default()
    };
    let shaped = shape_word_multilingual_items(
        fm,
        inline_items,
        &multilingual_styles,
        layout_direction,
        !line_params.wrap,
        uses_exact_word_baseline,
        !uses_multilingual_layout,
    )?;
    inline_items = shaped.items;
    paint.input_annotations = shaped.annotations;
    paint.field_occurrences = shaped.occurrences;
    let (mut lines, origins) = oxoffice_layout::line::break_into_lines_with_opaque_origins(
        &inline_items,
        &line_params,
        fm,
        uses_multilingual_layout.then_some(layout_direction),
    )?;
    paint.bind_annotation_origins(&lines, &origins)?;
    let actual_structural = lines
        .iter()
        .flat_map(|line| &line.items)
        .filter(|item| match item {
            LineItem::Text(base) | LineItem::Marker(base) => {
                base.field_kind.is_some() || base.field_source.is_some()
            }
            _ => false,
        })
        .count();
    if actual_structural != paint.structural_count {
        return Err(LayoutError::Layout(
            "Word structural producer count changed in line breaking".into(),
        ));
    }
    convert::restore_word_line_heights(
        &mut lines,
        line_params.line_spacing,
        grid_line_pitch_pt,
        fm,
        paragraph_mark,
    );

    let mut result = block::build_paragraph_block(
        lines,
        space_before,
        space_after,
        effective_ppr.borders.map(|borders| *borders),
        shading,
        ind_left,
        ind_right,
        jc,
        keep_next,
        keep_lines,
        page_break_before,
        widow_control,
    );
    result.heading_level = heading_level;
    result.heading_text = heading_level.map(|_| heading_text);
    result.change_bar = change_bar_color(para, input);
    result.list = list_num_id.map(|num_id| (num_id, list_ilvl.min(8) as u8));
    let (anchored, anchors) = collect_anchored_drawings(
        para,
        styles,
        input,
        media,
        fm,
        num_state,
        diagnostics,
        source_node,
    )?;
    result.anchored = anchored;
    paint.anchors = anchors;
    // `inline_items` is finished with here and would otherwise be dropped, so
    // handing it to the reflow costs nothing but the memory it already holds.
    // `Engine::layout` frees it again unless the document wraps.
    result.reflow = Some(Box::new(block::ParagraphReflow {
        items: inline_items,
        params: line_params,
        grid_line_pitch_pt,
        paragraph_mark,
    }));
    Ok((result, paint))
}

fn push_targeted_bookmark_markers(
    items: &mut Vec<InlineItem>,
    styles: &mut HashMap<usize, WordMultilingualStyle>,
    markers: &[oxoffice_docx_oxml::text::BookmarkMarker],
    projected_run_index: usize,
    input: &LayoutInput,
    fm: &mut FontManager,
    source_node: Option<SourceNodeId>,
) -> Result<()> {
    let mut font_id = None;
    for marker in markers.iter().filter(|marker| {
        let marker_run_index = match input.revision_view {
            RevisionView::Accepted => marker.projected_run_index(),
            RevisionView::Tracked => marker.tracked_run_index(),
        };
        marker.is_start()
            && marker_run_index == projected_run_index
            && marker.name().is_some_and(|name| {
                document_has_page_ref(input, name) && bookmark_text(input, name).is_some()
            })
    }) {
        if let Some(target) = marker.name().and_then(|name| page_ref_id(input, name)) {
            let resolved_font = match font_id {
                Some(font_id) => font_id,
                None => {
                    let resolved = fm.resolve_font_for_text(None, false, false, " ")?;
                    font_id = Some(resolved);
                    resolved
                }
            };
            push_structural_field_marker(
                items,
                styles,
                FieldKind::Target(target),
                resolved_font,
                source_node,
                None,
            );
        }
    }
    Ok(())
}

fn push_sequence_context_markers(
    field: &oxoffice_docx_oxml::text::Field,
    source: Option<(&crate::WordSequenceSnapshot, SourceNodeId)>,
    field_indices: &HashMap<*const oxoffice_docx_oxml::text::Field, u32>,
    font_id: oxoffice_layout::FontId,
    items: &mut Vec<InlineItem>,
    styles: &mut HashMap<usize, WordMultilingualStyle>,
) {
    let Some((snapshot, node)) = source else {
        return;
    };
    let mut pending = vec![field];
    while let Some(owner) = pending.pop() {
        if let Some(index) = field_indices.get(&std::ptr::from_ref(owner)).copied() {
            let source = FieldSource { node, index };
            if let Some(event) = snapshot.event_sources.get(&source) {
                push_structural_field_marker(
                    items,
                    styles,
                    FieldKind::SequenceContext(*event),
                    font_id,
                    Some(node),
                    Some(source),
                );
            }
        }
        pending.extend(owner.nested_fields_in_source_order().into_iter().rev());
    }
}

fn push_structural_field_marker(
    items: &mut Vec<InlineItem>,
    styles: &mut HashMap<usize, WordMultilingualStyle>,
    kind: FieldKind,
    font_id: oxoffice_layout::FontId,
    source_node: Option<SourceNodeId>,
    field_source: Option<FieldSource>,
) {
    styles.insert(
        items.len(),
        WordMultilingualStyle {
            origin: WordInputOrigin::Structural,
            ..Default::default()
        },
    );
    let segment = TextSegment {
        text: "\u{2060}".to_owned(),
        direction: TextDirection::Auto,
        source: source_node.map(|node| oxoffice_layout::SourceSpan {
            node,
            char_start: 0,
            char_end: 0,
        }),
        font_id,
        font_size: 1.0,
        glyph_ids: vec![0],
        advances: vec![0.0],
        width: 0.0,
        ascent: 0.0,
        descent: 0.0,
        line_gap: 0.0,
        color: Color::BLACK,
        bold: false,
        italic: false,
        underline: None,
        strike: false,
        dstrike: false,
        highlight: None,
        baseline_offset: 0.0,
        hyperlink_url: None,
        field_kind: Some(kind),
        field_source,
        note: None,
        note_reference_source: None,
    };
    items.push(if matches!(kind, FieldKind::SequenceContext(_)) {
        InlineItem::Marker(segment)
    } else {
        InlineItem::Text(segment)
    });
}

fn page_ref_id(input: &LayoutInput, name: &str) -> Option<usize> {
    page_reference_names(input)
        .iter()
        .position(|candidate| candidate == name)
}

pub(crate) fn page_reference_names(input: &LayoutInput) -> Vec<String> {
    let mut names = Vec::<String>::new();
    visit_input_paragraphs(input, &mut |paragraph| {
        for projected in project_paragraph_runs(paragraph, input.revision_view) {
            for content in &projected.run.content {
                let RunContent::Field(field) = content else {
                    continue;
                };
                let mut fields = vec![field];
                while let Some(field) = fields.pop() {
                    let instruction = field.effective_instruction();
                    if instruction.name == "PAGEREF"
                        && let Some(bookmark) = field_text_argument(&instruction, 0)
                        && !names.iter().any(|candidate| candidate == bookmark)
                    {
                        names.push(bookmark.to_owned());
                    }
                    fields.extend(field.all_nested_fields_in_source_order().into_iter().rev());
                }
            }
        }
    });
    names
}

fn document_has_sequence_fields(input: &LayoutInput) -> bool {
    let mut found = false;
    visit_input_paragraphs(input, &mut |paragraph| {
        for projected in project_paragraph_runs(paragraph, input.revision_view) {
            for content in &projected.run.content {
                if let RunContent::Field(field) = content {
                    found |= field.effective_instruction().name == "SEQ"
                        || field
                            .all_nested_fields_in_source_order()
                            .iter()
                            .any(|nested| nested.effective_instruction().name == "SEQ");
                }
            }
        }
    });
    found
}

fn document_has_section_fields(input: &LayoutInput) -> bool {
    let mut found = false;
    visit_input_paragraphs(input, &mut |paragraph| {
        found |= project_paragraph_runs(paragraph, input.revision_view).iter().any(|projected|
            projected.run.content.iter().any(|content| matches!(content, RunContent::Field(field)
                if std::iter::once(field).chain(field.all_nested_fields_in_source_order()).any(|owner| matches!(owner.effective_instruction().name.as_str(), "PAGE" | "NUMPAGES" | "SECTION" | "SECTIONPAGES" | "PAGEREF")))));
    });
    found
}

/// Preserve each physical story boundary while visiting selected drawing owners.
fn input_story_paragraphs(input: &LayoutInput) -> Vec<Vec<&CT_P>> {
    fn body_paragraphs(body: &CT_Body, accepted: bool) -> Vec<&CT_P> {
        let mut paragraphs = Vec::new();
        for item in body_layout_items(body) {
            match item {
                MainStoryLayoutItem::Paragraph(paragraph, _) => paragraphs.push(paragraph),
                MainStoryLayoutItem::Table(table, _) => {
                    visit_table_paragraphs(table, accepted, &mut |p| paragraphs.push(p));
                }
            }
        }
        paragraphs
    }
    let accepted = input.revision_view == RevisionView::Accepted;
    let mut main = Vec::new();
    visit_document_paragraphs(input, &mut |paragraph| main.push(paragraph));
    let mut stories = vec![main];
    let mut physical_parts = std::collections::HashSet::new();
    for (header, parts) in [(true, &input.headers), (false, &input.footers)] {
        let mut parts = parts.iter().collect::<Vec<_>>();
        parts.sort_unstable_by_key(|(id, _)| *id);
        for (id, part) in parts {
            let story = if header {
                WordStory::Header {
                    relationship_id: id.clone(),
                }
            } else {
                WordStory::Footer {
                    relationship_id: id.clone(),
                }
            };
            if let Some(part) = input.story_part_names.get(&story)
                && !physical_parts.insert((header, part))
            {
                continue;
            }
            stories.push(input.story_bodies.get(&story).map_or_else(
                || part.paragraphs.iter().collect(),
                |body| body_paragraphs(body, accepted),
            ));
        }
    }
    for (footnote, stream) in [
        (true, input.footnotes.as_ref()),
        (false, input.endnotes.as_ref()),
    ] {
        if let Some(stream) = stream {
            for note in &stream.footnotes {
                if note.note_type != oxoffice_docx_oxml::footnotes::NoteType::Normal {
                    continue;
                }
                let story = if footnote {
                    WordStory::Footnote { id: note.id }
                } else {
                    WordStory::Endnote { id: note.id }
                };
                stories.push(input.story_bodies.get(&story).map_or_else(
                    || note.paragraphs.iter().collect(),
                    |body| body_paragraphs(body, accepted),
                ));
            }
        }
    }
    let mut owner = 0;
    while owner < stories.len() {
        let mut drawings = Vec::new();
        for paragraph in &stories[owner] {
            for projected in project_paragraph_runs(paragraph, input.revision_view) {
                for drawing in projected
                    .run
                    .content
                    .iter()
                    .filter_map(|content| match content {
                        RunContent::Drawing(drawing) => Some(drawing),
                        _ => None,
                    })
                    .chain(projected.run.alt_drawings.iter())
                {
                    if let Some(shape) = drawing
                        .anchor
                        .as_ref()
                        .and_then(|anchor| anchor.shape.as_ref())
                    {
                        drawings.push(shape.text_body.as_ref().map_or_else(
                            || shape.text.iter().collect(),
                            |body| body_paragraphs(body, accepted),
                        ));
                    }
                }
            }
        }
        stories.extend(drawings);
        owner += 1;
    }
    stories
}

/// Visit every authoritative modeled paragraph without joining story ranges.
fn visit_input_paragraphs<'a>(input: &'a LayoutInput, visit: &mut impl FnMut(&'a CT_P)) {
    for paragraphs in input_story_paragraphs(input) {
        for paragraph in paragraphs {
            visit(paragraph);
        }
    }
}

fn field_text_argument(instruction: &FieldInstruction, index: usize) -> Option<&str> {
    match instruction.arguments.get(index) {
        Some(FieldArgument::Text(value)) => Some(value),
        Some(FieldArgument::Nested(_)) | None => None,
    }
}

fn field_has_switch(instruction: &FieldInstruction, name: &str) -> bool {
    instruction
        .switches
        .iter()
        .any(|field_switch| field_switch.name == name)
}

fn numbered_ref_text(
    instruction: &FieldInstruction,
    bookmark: &str,
    bookmark_text: String,
    numbering: &NumberingState,
    source: Option<SourceNodeId>,
    position: std::result::Result<Option<&str>, String>,
) -> std::result::Result<String, String> {
    let position = position?;
    let mode = if field_has_switch(instruction, "w") {
        Some("full")
    } else if field_has_switch(instruction, "r") {
        Some("relative")
    } else if field_has_switch(instruction, "n") {
        Some("level")
    } else {
        None
    };
    if mode.is_none() && !field_has_switch(instruction, "p") {
        return Ok(bookmark_text);
    }
    let source_is_main_story = source.is_some_and(|source| numbering.is_main_story_source(source));
    let marker = numbering
        .bookmark_source(bookmark)
        .and_then(|_| numbering.bookmark(bookmark))
        .map(|(_, marker)| marker);
    let mut value = match (mode, marker) {
        (Some(_), Some(marker)) => marker.numbered_reference_text(
            instruction,
            source
                .filter(|_| source_is_main_story)
                .and_then(|source| numbering.source(source)),
        )?,
        (Some(_), None)
            if numbering.bookmark_source(bookmark).is_some_and(|source| {
                numbering
                    .navigation
                    .get(&source)
                    .is_some_and(|facts| facts.suppressed_numbering)
            }) =>
        {
            "0".into()
        }
        (Some(_), None) => bookmark_text,
        (None, _) => String::new(),
    };
    if let Some(position) = position {
        if !value.is_empty() {
            value.push(' ');
        }
        value.push_str(position);
    }
    Ok(value)
}

// Borrowed admission precedes the recursive predicate, any clone and all
// deferred semantic factories. The caller's source model remains untouched.
fn prepared_semantic_layout_input(input: &LayoutInput) -> Result<Cow<'_, LayoutInput>> {
    let validate = || -> oxoffice_docx_oxml::Result<()> {
        input.document.validate_resource_limits()?;
        for body in input.story_bodies.values() {
            body.validate_resource_limits()?;
        }
        for part in input.headers.values().chain(input.footers.values()) {
            for paragraph in &part.paragraphs {
                paragraph.validate_resource_limits()?;
            }
        }
        for notes in [input.footnotes.as_ref(), input.endnotes.as_ref()]
            .into_iter()
            .flatten()
        {
            for note in &notes.footnotes {
                for paragraph in &note.paragraphs {
                    paragraph.validate_resource_limits()?;
                }
            }
        }
        Ok(())
    };
    validate().map_err(|error| LayoutError::Layout(error.to_string()))?;
    let mut needs_projection = false;
    // This is the existing authoritative story selection, including its
    // drawing children. Raw foreign payload is never promoted to a model.
    visit_input_paragraphs(input, &mut |p| {
        needs_projection |= paragraph_has_semantic_revision(p);
    });
    // Direct-run revisions can belong to a block control without a paragraph.
    needs_projection |= body_has_semantic_revision(&input.document.body.content)
        || input
            .story_bodies
            .values()
            .any(|b| body_has_semantic_revision(&b.content));
    if !needs_projection {
        return Ok(Cow::Borrowed(input));
    }
    let mut prepared = input.clone();
    prepare_layout_body_projection(&mut prepared.document.body.content)
        .map_err(|error| LayoutError::Layout(error.to_string()))?;
    for (header, parts) in [
        (true, &mut prepared.headers),
        (false, &mut prepared.footers),
    ] {
        for (id, part) in parts {
            let story = if header {
                WordStory::Header {
                    relationship_id: id.clone(),
                }
            } else {
                WordStory::Footer {
                    relationship_id: id.clone(),
                }
            };
            if let Some(body) = prepared.story_bodies.get_mut(&story) {
                prepare_layout_body_projection(&mut body.content)
                    .map_err(|error| LayoutError::Layout(error.to_string()))?;
            } else {
                for paragraph in &mut part.paragraphs {
                    prepare_layout_paragraph_projection(paragraph)
                        .map_err(|error| LayoutError::Layout(error.to_string()))?;
                }
            }
        }
    }
    for (footnote, notes) in [
        (true, &mut prepared.footnotes),
        (false, &mut prepared.endnotes),
    ] {
        if let Some(notes) = notes {
            for note in &mut notes.footnotes {
                if note.note_type != oxoffice_docx_oxml::footnotes::NoteType::Normal {
                    continue;
                }
                let story = if footnote {
                    WordStory::Footnote { id: note.id }
                } else {
                    WordStory::Endnote { id: note.id }
                };
                if let Some(body) = prepared.story_bodies.get_mut(&story) {
                    prepare_layout_body_projection(&mut body.content)
                        .map_err(|error| LayoutError::Layout(error.to_string()))?;
                } else {
                    for paragraph in &mut note.paragraphs {
                        prepare_layout_paragraph_projection(paragraph)
                            .map_err(|error| LayoutError::Layout(error.to_string()))?;
                    }
                }
            }
        }
    }
    Ok(Cow::Owned(prepared))
}

fn paragraph_has_semantic_revision(p: &CT_P) -> bool {
    !p.revisions.is_empty()
        || p.content_controls
            .iter()
            .any(|(_, _, _, c)| control_has_semantic_revision(c))
        || p.runs.iter().any(run_has_semantic_revision)
}

fn run_has_semantic_revision(run: &CT_R) -> bool {
    run.content
        .iter()
        .filter_map(|c| match c {
            RunContent::Drawing(d) => Some(d),
            _ => None,
        })
        .chain(run.alt_drawings.iter())
        .any(|drawing| {
            drawing
                .anchor
                .as_ref()
                .and_then(|a| a.shape.as_ref())
                .is_some_and(|shape| {
                    shape.text_body.as_ref().map_or_else(
                        || shape.text.iter().any(paragraph_has_semantic_revision),
                        |b| body_has_semantic_revision(&b.content),
                    )
                })
        })
}

fn control_has_semantic_revision(control: &CT_Sdt) -> bool {
    !control.revisions().is_empty()
        || control.content.iter().any(|c| match c {
            SdtContent::Paragraph(p) => paragraph_has_semantic_revision(p),
            SdtContent::Table(t) => table_has_semantic_revision(t),
            SdtContent::Row(r) => row_has_semantic_revision(r),
            SdtContent::Cell(c) => cell_has_semantic_revision(c),
            SdtContent::Run(r) => run_has_semantic_revision(r),
            SdtContent::ContentControl(c) => control_has_semantic_revision(c),
            SdtContent::RawXml(_) => false,
        })
}

fn table_has_semantic_revision(table: &CT_Tbl) -> bool {
    table
        .content_controls
        .iter()
        .any(|(_, _, c)| control_has_semantic_revision(c))
        || table.rows.iter().any(row_has_semantic_revision)
}

fn row_has_semantic_revision(row: &CT_Row) -> bool {
    row.content_controls
        .iter()
        .any(|(_, _, c)| control_has_semantic_revision(c))
        || row.cells.iter().any(cell_has_semantic_revision)
}

fn cell_has_semantic_revision(cell: &CT_Tc) -> bool {
    cell.content.iter().any(|c| match c {
        CellContent::Paragraph(p) => paragraph_has_semantic_revision(p),
        CellContent::Table(t) => table_has_semantic_revision(t),
        CellContent::ContentControl(c) => control_has_semantic_revision(c),
    })
}

fn body_has_semantic_revision(body: &[BodyContent]) -> bool {
    body.iter().any(|c| match c {
        BodyContent::Paragraph(p) => paragraph_has_semantic_revision(p),
        BodyContent::Table(t) => table_has_semantic_revision(t),
        BodyContent::ContentControl(c) => control_has_semantic_revision(c),
        BodyContent::RawXml(_) => false,
    })
}

fn prepare_layout_paragraph_projection(value: &mut CT_P) -> oxoffice_docx_oxml::Result<()> {
    for (_, _, revision) in &mut value.revisions {
        if let Some(paragraph) = revision.physical_content_paragraph_mut()? {
            prepare_layout_paragraph_projection(paragraph)?;
        }
    }
    for (_, _, _, control) in &mut value.content_controls {
        prepare_layout_control_projection(control)?;
    }
    for run in &mut value.runs {
        prepare_layout_run_projection(run)?;
    }
    Ok(())
}

fn prepare_layout_run_projection(value: &mut CT_R) -> oxoffice_docx_oxml::Result<()> {
    for drawing in value
        .content
        .iter_mut()
        .filter_map(|content| {
            if let RunContent::Drawing(drawing) = content {
                Some(drawing)
            } else {
                None
            }
        })
        .chain(value.alt_drawings.iter_mut())
    {
        let shape = drawing
            .anchor
            .as_mut()
            .and_then(|anchor| anchor.shape.as_mut());
        if let Some(shape) = shape {
            if let Some(body) = &mut shape.text_body {
                prepare_layout_body_projection(&mut body.content)?;
            } else {
                for paragraph in &mut shape.text {
                    prepare_layout_paragraph_projection(paragraph)?;
                }
            }
        }
    }
    Ok(())
}

fn prepare_layout_control_projection(value: &mut CT_Sdt) -> oxoffice_docx_oxml::Result<()> {
    for (_, revision) in value.physical_revisions_mut() {
        if let Some(paragraph) = revision.physical_content_paragraph_mut()? {
            prepare_layout_paragraph_projection(paragraph)?;
        }
    }
    for child in &mut value.content {
        match child {
            SdtContent::Paragraph(p) => prepare_layout_paragraph_projection(p)?,
            SdtContent::Table(t) => prepare_layout_table_projection(t)?,
            SdtContent::Row(r) => prepare_layout_row_projection(r)?,
            SdtContent::Cell(c) => prepare_layout_cell_projection(c)?,
            SdtContent::Run(r) => prepare_layout_run_projection(r)?,
            SdtContent::ContentControl(c) => prepare_layout_control_projection(c)?,
            SdtContent::RawXml(_) => {}
        }
    }
    Ok(())
}

fn prepare_layout_table_projection(value: &mut CT_Tbl) -> oxoffice_docx_oxml::Result<()> {
    for (_, _, control) in &mut value.content_controls {
        prepare_layout_control_projection(control)?;
    }
    for row in &mut value.rows {
        prepare_layout_row_projection(row)?;
    }
    Ok(())
}

fn prepare_layout_row_projection(value: &mut CT_Row) -> oxoffice_docx_oxml::Result<()> {
    for (_, _, control) in &mut value.content_controls {
        prepare_layout_control_projection(control)?;
    }
    for cell in &mut value.cells {
        prepare_layout_cell_projection(cell)?;
    }
    Ok(())
}

fn prepare_layout_cell_projection(value: &mut CT_Tc) -> oxoffice_docx_oxml::Result<()> {
    for content in &mut value.content {
        match content {
            CellContent::Paragraph(p) => prepare_layout_paragraph_projection(p)?,
            CellContent::Table(t) => prepare_layout_table_projection(t)?,
            CellContent::ContentControl(c) => prepare_layout_control_projection(c)?,
        }
    }
    Ok(())
}

fn prepare_layout_body_projection(values: &mut [BodyContent]) -> oxoffice_docx_oxml::Result<()> {
    for value in values {
        match value {
            BodyContent::Paragraph(p) => prepare_layout_paragraph_projection(p)?,
            BodyContent::Table(t) => prepare_layout_table_projection(t)?,
            BodyContent::ContentControl(c) => prepare_layout_control_projection(c)?,
            BodyContent::RawXml(_) => {}
        }
    }
    Ok(())
}

fn document_has_ref_projection(input: &LayoutInput) -> bool {
    let paragraph_has_ref = |paragraph: &CT_P| {
        project_paragraph_runs(paragraph, input.revision_view)
            .into_iter()
            .any(|projected| {
                projected.run.content.iter().any(|content| {
                    let RunContent::Field(field) = content else {
                        return false;
                    };
                    let instruction = field.effective_instruction();
                    instruction.name == "STYLEREF"
                        || instruction.name == "REF"
                            && ["n", "r", "w", "p"]
                                .into_iter()
                                .any(|name| field_has_switch(&instruction, name))
                })
            })
    };
    let mut found = false;
    visit_input_paragraphs(input, &mut |paragraph| {
        found |= paragraph_has_ref(paragraph);
    });
    found
}

fn document_has_page_ref(input: &LayoutInput, name: &str) -> bool {
    page_ref_id(input, name).is_some()
}

/// Visit every paragraph of the main story. The accepted view leaves out the
/// rows that `CT_Row::accepted_view_removes` reports, as accepting does.
fn visit_document_paragraphs<'a>(input: &'a LayoutInput, visit: &mut impl FnMut(&'a CT_P)) {
    let accepted = input.revision_view == RevisionView::Accepted;
    for content in &input.document.body.content {
        match content {
            BodyContent::Paragraph(paragraph) => visit(paragraph),
            BodyContent::Table(table) => visit_table_paragraphs(table, accepted, visit),
            BodyContent::ContentControl(control) => {
                visit_control_paragraphs(control, BlockControlOwner::Body, accepted, visit)
            }
            BodyContent::RawXml(_) => {}
        }
    }
}

fn input_uses_page_note_restart(input: &LayoutInput) -> bool {
    input.note_defaults.iter().flatten().any(|policy| policy.num_restart.as_deref() == Some("eachPage"))
        || input.document.body.content.iter().any(|content| {
            matches!(content, BodyContent::Paragraph(paragraph) if paragraph.properties.as_ref().and_then(|properties| properties.sect_pr.as_ref()).is_some_and(|section| {
                [section.footnote_pr.as_deref(), section.endnote_pr.as_deref()].into_iter().flatten().any(|policy| policy.num_restart.as_deref() == Some("eachPage"))
            }))
        })
        || input.document.body.sect_pr.as_ref().is_some_and(|section| {
            [section.footnote_pr.as_deref(), section.endnote_pr.as_deref()].into_iter().flatten().any(|policy| policy.num_restart.as_deref() == Some("eachPage"))
        })
}

fn note_reference_pages(result: &LayoutResult) -> HashMap<NoteRef, usize> {
    let mut pages = HashMap::new();
    for page in &result.pages {
        oxoffice_layout::walk(&page.elements, &mut |element, _| {
            let reference = match element {
                PositionedElement::Text(run) => run.note,
                PositionedElement::MultilingualText(run) => run.note,
                _ => None,
            };
            if let Some(reference) = reference {
                pages.entry(reference).or_insert(page.page_number);
            }
        });
    }
    pages
}

fn note_labels(
    input: &LayoutInput,
    note_pages: Option<&HashMap<NoteRef, usize>>,
) -> Result<HashMap<NoteRef, String>> {
    let mut labels = HashMap::new();
    let mut preparation_error = None;
    let mut counts = [None::<u32>, None::<u32>];
    let mut last_pages = [None::<usize>, None::<usize>];
    let mut section_ends = input
        .document
        .body
        .content
        .iter()
        .enumerate()
        .filter_map(|(index, content)| match content {
            BodyContent::Paragraph(paragraph) => paragraph
                .properties
                .as_ref()
                .and_then(|properties| properties.sect_pr.as_ref())
                .map(|section| (index, Some(section))),
            _ => None,
        })
        .collect::<Vec<_>>();
    section_ends.push((
        input.document.body.content.len(),
        input.document.body.sect_pr.as_ref(),
    ));
    let mut first_index = 0;
    for (end, section) in section_ends {
        let policies = [
            section.and_then(|section| section.footnote_pr.as_deref()),
            section.and_then(|section| section.endnote_pr.as_deref()),
        ];
        for (family, policy) in policies.iter().enumerate() {
            let restart = policy
                .and_then(|policy| policy.num_restart.as_deref())
                .or_else(|| {
                    input.note_defaults[family]
                        .as_ref()
                        .and_then(|policy| policy.num_restart.as_deref())
                });
            if first_index > 0 && restart == Some("eachSect") && policy.is_some() {
                counts[family] = None;
            }
        }
        for content in input
            .document
            .body
            .content
            .iter()
            .take(end + 1)
            .skip(first_index)
        {
            let mut visit = |paragraph: &CT_P| {
                for projected in project_paragraph_runs(paragraph, input.revision_view) {
                    let references = projected
                        .run
                        .content
                        .iter()
                        .map(|content| content_note_references(content, input.revision_view))
                        .collect::<Result<Vec<_>>>();
                    let references = match references {
                        Ok(references) => references,
                        Err(error) => {
                            preparation_error = Some(error);
                            return;
                        }
                    };
                    for item in references.into_iter().flatten() {
                        let (family, stream, id, custom_mark) = match &item {
                            RunContent::FootnoteRef { id, custom_mark } => {
                                (0, NoteStream::Footnote, *id, custom_mark.as_deref())
                            }
                            RunContent::EndnoteRef { id, custom_mark } => {
                                (1, NoteStream::Endnote, *id, custom_mark.as_deref())
                            }
                            _ => continue,
                        };
                        if let std::collections::hash_map::Entry::Vacant(entry) =
                            labels.entry(NoteRef { stream, id })
                        {
                            if let Some(mark) = custom_mark {
                                entry.insert(mark.to_owned());
                                continue;
                            }
                            let section_policy = policies[family];
                            let default_policy = input.note_defaults[family].as_ref();
                            let restart = section_policy
                                .and_then(|policy| policy.num_restart.as_deref())
                                .or_else(|| {
                                    default_policy.and_then(|policy| policy.num_restart.as_deref())
                                });
                            let page = note_pages
                                .and_then(|pages| pages.get(&NoteRef { stream, id }))
                                .copied();
                            if restart == Some("eachPage")
                                && page.is_some()
                                && last_pages[family] != page
                            {
                                counts[family] = None;
                            }
                            if page.is_some() {
                                last_pages[family] = page;
                            }
                            let start = section_policy
                                .and_then(|policy| policy.num_start)
                                .or_else(|| default_policy.and_then(|policy| policy.num_start))
                                .unwrap_or(1);
                            let ordinal =
                                counts[family].map_or(start, |value| value.saturating_add(1));
                            counts[family] = Some(ordinal);
                            let format = section_policy
                                .and_then(|policy| policy.num_fmt.as_deref())
                                .or_else(|| {
                                    default_policy.and_then(|policy| policy.num_fmt.as_deref())
                                })
                                .map(ST_NumberFormat::from_str)
                                .unwrap_or(ST_NumberFormat::Decimal);
                            entry.insert(style_resolver::format_number(ordinal, format));
                        }
                    }
                }
            };
            match content {
                BodyContent::Paragraph(paragraph) => visit(paragraph),
                BodyContent::Table(table) => visit_table_paragraphs(
                    table,
                    input.revision_view == RevisionView::Accepted,
                    &mut visit,
                ),
                BodyContent::ContentControl(control) => visit_control_paragraphs(
                    control,
                    BlockControlOwner::Body,
                    input.revision_view == RevisionView::Accepted,
                    &mut visit,
                ),
                BodyContent::RawXml(_) => {}
            }
        }
        first_index = end.saturating_add(1);
    }
    if let Some(error) = preparation_error {
        return Err(error);
    }
    Ok(labels)
}

fn visit_table_paragraphs<'a>(table: &'a CT_Tbl, accepted: bool, visit: &mut impl FnMut(&'a CT_P)) {
    for boundary in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(position, _, _)| *position == boundary)
        {
            visit_control_paragraphs(control, BlockControlOwner::Table, accepted, visit);
        }
        if let Some(row) = table.rows.get(boundary) {
            visit_row_paragraphs(row, accepted, visit);
        }
    }
}

fn visit_row_paragraphs<'a>(row: &'a CT_Row, accepted: bool, visit: &mut impl FnMut(&'a CT_P)) {
    if accepted && row.accepted_view_removes() {
        return;
    }
    for boundary in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter()
            .filter(|(position, _, _)| *position == boundary)
        {
            visit_control_paragraphs(control, BlockControlOwner::Row, accepted, visit);
        }
        if let Some(cell) = row.cells.get(boundary) {
            visit_cell_paragraphs(cell, accepted, visit);
        }
    }
}

fn visit_cell_paragraphs<'a>(cell: &'a CT_Tc, accepted: bool, visit: &mut impl FnMut(&'a CT_P)) {
    for content in &cell.content {
        match content {
            CellContent::Paragraph(paragraph) => visit(paragraph),
            CellContent::Table(table) => visit_table_paragraphs(table, accepted, visit),
            CellContent::ContentControl(control) => {
                visit_control_paragraphs(control, BlockControlOwner::Cell, accepted, visit)
            }
        }
    }
}

fn visit_control_paragraphs<'a>(
    control: &'a CT_Sdt,
    owner: BlockControlOwner,
    accepted: bool,
    visit: &mut impl FnMut(&'a CT_P),
) {
    for content in &control.content {
        match (owner, content) {
            (
                BlockControlOwner::Body | BlockControlOwner::Cell,
                SdtContent::Paragraph(paragraph),
            ) => visit(paragraph),
            (BlockControlOwner::Body | BlockControlOwner::Cell, SdtContent::Table(table)) => {
                visit_table_paragraphs(table, accepted, visit)
            }
            (BlockControlOwner::Table, SdtContent::Row(row)) => {
                visit_row_paragraphs(row, accepted, visit)
            }
            (BlockControlOwner::Row, SdtContent::Cell(cell)) => {
                visit_cell_paragraphs(cell, accepted, visit)
            }
            (_, SdtContent::ContentControl(control)) => {
                visit_control_paragraphs(control, owner, accepted, visit)
            }
            _ => {}
        }
    }
}

fn bookmark_range_in_paragraphs(
    input: &LayoutInput,
    name: &str,
    paragraphs: &[&CT_P],
) -> Option<((usize, usize), (usize, usize))> {
    type OrderedBodyRunPosition = (usize, usize, usize);
    type BookmarkStart<'a> = (Option<&'a str>, OrderedBodyRunPosition);
    let mut starts: HashMap<i32, Vec<BookmarkStart<'_>>> = HashMap::new();
    let mut ends: HashMap<i32, Vec<OrderedBodyRunPosition>> = HashMap::new();
    let mut encounter = 0usize;
    let current_bookmarks = paragraphs
        .iter()
        .map(|paragraph| paragraph.current_bookmark_projection())
        .collect::<Vec<_>>();
    for (paragraph_index, paragraph) in paragraphs.iter().enumerate() {
        let projected_run_count = project_paragraph_runs(paragraph, input.revision_view).len();
        for marker in &current_bookmarks[paragraph_index] {
            let Some(id) = marker.id() else {
                continue;
            };
            let marker_run_index = match input.revision_view {
                RevisionView::Accepted => marker.projected_run_index(),
                RevisionView::Tracked => marker.tracked_run_index(),
            };
            if marker_run_index > projected_run_count {
                return None;
            }
            let position = (paragraph_index, marker_run_index, encounter);
            encounter += 1;
            if marker.is_start() {
                starts
                    .entry(id)
                    .or_default()
                    .push((marker.name(), position));
            } else {
                ends.entry(id).or_default().push(position);
            }
        }
    }
    let candidates = starts
        .iter()
        .filter_map(|(id, starts)| {
            let ends = ends.get(id)?;
            (starts.len() == 1 && starts[0].0 == Some(name) && ends.len() == 1)
                .then_some((starts[0].1, ends[0]))
        })
        .collect::<Vec<_>>();
    if candidates.len() != 1 {
        return None;
    }
    let (ordered_start, ordered_end) = candidates[0];
    if ordered_start > ordered_end {
        return None;
    }
    let start = (ordered_start.0, ordered_start.1);
    let end = (ordered_end.0, ordered_end.1);
    Some((start, end))
}

fn bookmark_text(input: &LayoutInput, name: &str) -> Option<String> {
    let stories = input_story_paragraphs(input);
    let named_starts = stories
        .iter()
        .flatten()
        .flat_map(|p| &p.bookmark_markers)
        .filter(|marker| marker.is_start() && marker.name() == Some(name))
        .count();
    if named_starts != 1 {
        return None;
    }
    let mut matches = Vec::new();
    for paragraphs in stories {
        if let Some((start, end)) = bookmark_range_in_paragraphs(input, name, &paragraphs) {
            let mut parts = Vec::new();
            for (paragraph_index, paragraph) in
                paragraphs.iter().enumerate().take(end.0 + 1).skip(start.0)
            {
                parts.push(
                    project_paragraph_runs(paragraph, input.revision_view)
                        .into_iter()
                        .enumerate()
                        .filter(|(index, _)| {
                            (paragraph_index, *index) >= start && (paragraph_index, *index) < end
                        })
                        .map(|(_, projected)| projected.run.text())
                        .collect::<String>(),
                );
            }
            matches.push(parts.join("\n"));
        }
    }
    (matches.len() == 1).then(|| matches.remove(0))
}

/// Whether any drawing or floating table in the document body wraps text
/// around itself.
///
/// A document without one can never reach the reflow path, so it does not pay
/// for it, and it also never needs the second pagination pass.
fn document_has_wrapping_drawing(input: &LayoutInput) -> bool {
    /// Whether a `w:tblpPr` on the table itself floats it.
    ///
    /// The rule belongs to the lowering, so this asks it rather than repeating
    /// it. Direct properties only: a float declared by a table style is as rare
    /// as a drawing inside a nested table, and the conservative answer there is
    /// the same, look no deeper and lose the reflow rather than the table.
    fn table_floats(table: &oxoffice_docx_oxml::table::CT_Tbl) -> bool {
        table
            .properties
            .as_ref()
            .is_some_and(|properties| crate::table::floating_table(properties).is_some())
    }

    fn paragraph_wraps(para: &CT_P, view: RevisionView) -> bool {
        fn run_wraps(run: &CT_R) -> bool {
            run.content
                .iter()
                .filter_map(|rc| match rc {
                    RunContent::Drawing(d) => Some(d),
                    _ => None,
                })
                .chain(run.alt_drawings.iter())
                .any(|drawing| {
                    drawing
                        .anchor
                        .as_ref()
                        .is_some_and(|anchor| anchor.wrap != WrapType::None)
                })
        }

        if para.revisions.is_empty() && para.content_controls.is_empty() {
            para.runs.iter().any(run_wraps)
        } else {
            project_paragraph_runs(para, view)
                .iter()
                .any(|projected| run_wraps(projected.run))
        }
    }

    input
        .document
        .body
        .content
        .iter()
        .any(|content| match content {
            BodyContent::Paragraph(para) => paragraph_wraps(para, input.revision_view),
            BodyContent::Table(table) => {
                table_floats(table)
                    || table
                        .rows
                        .iter()
                        .flat_map(|row| row.cells.iter())
                        .flat_map(|cell| cell.content.iter())
                        .any(|content| match content {
                            oxoffice_docx_oxml::table::CellContent::Paragraph(para) => {
                                paragraph_wraps(para, input.revision_view)
                            }
                            // A drawing inside a nested table is rare enough that
                            // the conservative answer is to look no deeper.
                            oxoffice_docx_oxml::table::CellContent::Table(_) => false,
                            oxoffice_docx_oxml::table::CellContent::ContentControl(_) => false,
                        })
            }
            _ => false,
        })
}

fn render_word_chart(
    relationship_id: &str,
    width: f64,
    height: f64,
    input: &LayoutInput,
    fm: &mut FontManager,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<GroupElement> {
    let bounds = Rect {
        x: 0.0,
        y: 0.0,
        width,
        height,
    };
    let rendered = match input.charts.get(relationship_id) {
        Some(Ok(chart)) => oxoffice_chart::render_chart(
            &chart.chart,
            bounds,
            &input.chart_theme,
            &input.chart_color_map,
            fm,
        )
        .map_err(|error| error.to_string()),
        Some(Err(message)) => Err(message.clone()),
        None => Err("relationship was not resolved from the document part".to_owned()),
    };
    match rendered {
        Ok(group) => {
            if let Some(Ok(chart)) = input.charts.get(relationship_id) {
                for code in oxoffice_chart::unsupported_number_formats(&chart.chart)
                    .map_err(|error| LayoutError::Layout(error.to_string()))?
                {
                    push_unique_diagnostic(
                        diagnostics,
                        format!(
                            "Word chart relationship {relationship_id}: number format `{code}` is not supported and its labels use General"
                        ),
                    );
                }
            }
            Ok(group)
        }
        Err(detail) => {
            diagnostics.push(Diagnostic {
                message: format!("Word chart relationship {relationship_id}: {detail}"),
            });
            oxoffice_chart::render_chart_placeholder(bounds, fm)
                .map_err(|error| oxoffice_layout::LayoutError::Layout(error.to_string()))
        }
    }
}

/// Collect the floating drawings anchored to a paragraph.
///
/// The offsets stay paired with the frame they are measured from. Resolving
/// them here is not possible: a paragraph-relative offset needs the laid-out
/// position of the paragraph, which only the paginator knows.
///
/// A shape's text box is laid out here rather than later, because breaking it
/// into lines needs the font manager.
#[allow(clippy::too_many_arguments)]
fn collect_anchored_drawings(
    para: &CT_P,
    styles: &CT_Styles,
    input: &LayoutInput,
    media: &MediaRegistry,
    fm: &mut FontManager,
    num_state: &mut NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
    source_node: Option<SourceNodeId>,
) -> Result<(Vec<block::AnchoredDrawing>, Vec<WordAnchorPaint>)> {
    let mut out = Vec::new();
    let mut paints = Vec::new();

    // Drawings written plainly, and drawings recovered from an
    // mc:AlternateContent block, are both anchored the same way.
    for (run_index, projected) in project_paragraph_runs(para, input.revision_view)
        .into_iter()
        .enumerate()
    {
        let run = projected.run;
        let plain = run.content.iter().filter_map(|rc| match rc {
            RunContent::Drawing(d) => Some(d),
            _ => None,
        });
        for (drawing_index, drawing) in plain.chain(run.alt_drawings.iter()).enumerate() {
            let Some(anchor) = drawing.anchor.as_ref() else {
                continue;
            };

            // A picture also carries a pic:spPr, so a parsed shape alone does
            // not mean this is a shape. An embed id is what makes it a
            // picture, and that takes precedence.
            let shape = if anchor.embed_id.is_empty() && anchor.chart_rel_id.is_none() {
                anchor.shape.as_ref()
            } else {
                None
            };

            let mut content_paint = WordAnchorPaint::Image;
            let content = if let Some(relationship_id) = anchor.chart_rel_id.as_deref() {
                content_paint = WordAnchorPaint::Group {
                    policies: Default::default(),
                };
                block::AnchoredContent::Group(render_word_chart(
                    relationship_id,
                    anchor.extent_cx.to_pt(),
                    anchor.extent_cy.to_pt(),
                    input,
                    fm,
                    diagnostics,
                )?)
            } else {
                match shape {
                    Some(shape) => {
                        // A shape's text box wraps at the shape width.
                        let mut text = Vec::new();
                        let mut paragraph_paints = Vec::new();
                        if let Some(body) = &shape.text_body {
                            let mut local_sources = source_node
                                .map(|_| SourceRegistry::for_body(input, body))
                                .transpose()?;
                            let mut inverse = None;
                            if input.sequence_snapshot.is_some()
                                && let Some(parent) = source_node
                                && let Some(sources) = &mut local_sources
                            {
                                match sources.bind_sequence_shape_sources(
                                    input,
                                    parent,
                                    run_index,
                                    drawing_index,
                                    shape.source_text_box_owner,
                                ) {
                                    Ok(map) => inverse = Some(map),
                                    Err(message) => {
                                        diagnostics.push(Diagnostic { message });
                                        local_sources = None;
                                    }
                                }
                            }
                            for item in body_layout_items(body) {
                                match item {
                                    MainStoryLayoutItem::Paragraph(paragraph, path) => {
                                        let node = local_sources.as_ref().and_then(|sources| {
                                            sources.id(&WordStory::Document, &path)
                                        });
                                        let (mut block, _, mut paint) =
                                            layout_paragraph_with_source_and_direction(
                                                paragraph,
                                                anchor.extent_cx.to_pt(),
                                                styles,
                                                input,
                                                media,
                                                fm,
                                                num_state,
                                                diagnostics,
                                                node,
                                                node,
                                                None,
                                            )?;
                                        if let Some(sources) = &local_sources {
                                            sources
                                                .bind_text_boxes(&mut block, &mut paint, node)?;
                                        }
                                        text.push(block);
                                        paragraph_paints.push(paint);
                                    }
                                    MainStoryLayoutItem::Table(table, path) => {
                                        let (table, semantics) =
                                            table::layout_table_with_provenance(
                                                table,
                                                anchor.extent_cx.to_pt(),
                                                styles,
                                                input,
                                                media,
                                                fm,
                                                num_state,
                                                diagnostics,
                                                local_sources.as_ref(),
                                                &WordStory::Document,
                                                &path,
                                                None,
                                            )?;
                                        let geometry = PageGeometry {
                                            page_width: anchor.extent_cx.to_pt(),
                                            page_height: anchor.extent_cy.to_pt(),
                                            ..PageGeometry::default()
                                        };
                                        for (block, paint) in paginator::story_table_rows(
                                            &table,
                                            &semantics,
                                            &geometry,
                                            media.media(),
                                        )? {
                                            text.push(block);
                                            paragraph_paints.push(paint);
                                        }
                                    }
                                }
                            }
                            if let Some(inverse) = &inverse {
                                for (paragraph, paint) in text.iter_mut().zip(&mut paragraph_paints)
                                {
                                    remap_text_box_sources(paragraph, inverse)?;
                                    paint.remap_sources(inverse)?;
                                }
                            }
                        } else {
                            for p in &shape.text {
                                let (block, _, paint) = layout_paragraph_with_source_and_direction(
                                    p,
                                    anchor.extent_cx.to_pt(),
                                    styles,
                                    input,
                                    media,
                                    fm,
                                    num_state,
                                    diagnostics,
                                    source_node.map(|_| CACHE_SOURCE_NODE),
                                    source_node.map(|_| CACHE_SOURCE_NODE),
                                    None,
                                )?;
                                text.push(block);
                                paragraph_paints.push(paint);
                            }
                        }
                        content_paint = WordAnchorPaint::Shape {
                            paragraphs: paragraph_paints,
                        };
                        block::AnchoredContent::Shape {
                            preset: block::ShapePreset::from_prst(shape.preset.as_deref()),
                            fill: shape.solid_fill.as_deref().map(Color::from_hex),
                            text,
                        }
                    }
                    None if anchor.embed_id.is_empty() => continue,
                    None => {
                        media.record_picture_draw_owner(&anchor.embed_id);
                        let media_id = media
                            .id_for_relationship_with_diagnostic(&anchor.embed_id, diagnostics);
                        if let Some((opacity, rotation)) = anchor.picture_effects() {
                            let width = anchor.extent_cx.to_pt();
                            let height = anchor.extent_cy.to_pt();
                            let image = media.media().get(&media_id);
                            content_paint = WordAnchorPaint::Group {
                                policies: Default::default(),
                            };
                            block::AnchoredContent::Group(GroupElement {
                                transform: Transform::rotate_about(
                                    rotation,
                                    width / 2.0,
                                    height / 2.0,
                                ),
                                clip: None,
                                opacity,
                                effects: Vec::new(),
                                children: vec![PositionedElement::Image {
                                    rect: Rect {
                                        x: 0.0,
                                        y: 0.0,
                                        width,
                                        height,
                                    },
                                    data: image.map_or_else(Vec::new, |image| image.data.clone()),
                                    content_type: image.map_or_else(String::new, |image| {
                                        image.content_type.clone()
                                    }),
                                    media_id,
                                }],
                            })
                        } else {
                            block::AnchoredContent::Image { media_id }
                        }
                    }
                }
            };

            paints.push(content_paint);
            out.push(block::AnchoredDrawing {
                rotation_degrees: if shape.is_some() {
                    anchor.shape_rotation_degrees().unwrap_or(0.0)
                } else {
                    0.0
                },
                behind_doc: anchor.behind_doc,
                rel_h: anchor.pos_h_relative_from,
                off_h: anchor.pos_h_offset.to_pt(),
                rel_v: anchor.pos_v_relative_from,
                off_v: anchor.pos_v_offset.to_pt(),
                width: anchor.extent_cx.to_pt(),
                height: anchor.extent_cy.to_pt(),
                alternate_text: anchor.description.clone(),
                structure_id: None,
                wrap: anchor.wrap,
                dist_top: anchor.dist_t.to_pt(),
                dist_bottom: anchor.dist_b.to_pt(),
                dist_left: anchor.dist_l.to_pt(),
                dist_right: anchor.dist_r.to_pt(),
                align_h: anchor.pos_h_align,
                align_v: anchor.pos_v_align,
                content,
            });
        }
    }
    Ok((out, paints))
}

/// Merge direct paragraph properties (only fields explicitly set in the XML).
fn merge_direct_ppr(effective: &mut CT_PPr, direct: &CT_PPr) {
    if direct.outline_lvl.is_some() {
        effective.outline_lvl = direct.outline_lvl;
    }
    // Don't merge style_id — that was already used for resolution
    if direct.jc.is_some() {
        effective.jc = direct.jc;
    }
    if direct.space_before.is_some() {
        effective.space_before = direct.space_before;
    }
    if direct.space_after.is_some() {
        effective.space_after = direct.space_after;
    }
    if direct.line_spacing.is_some() {
        effective.line_spacing = direct.line_spacing;
    }
    if direct.text_alignment.is_some() {
        effective.text_alignment = direct.text_alignment.clone();
    }
    if direct.line_rule.is_some() {
        effective.line_rule = direct.line_rule.clone();
    }
    if direct.ind_left.is_some() {
        effective.ind_left = direct.ind_left;
    }
    if direct.ind_right.is_some() {
        effective.ind_right = direct.ind_right;
    }
    if direct.ind_start.is_some() {
        effective.ind_start = direct.ind_start;
    }
    if direct.ind_end.is_some() {
        effective.ind_end = direct.ind_end;
    }
    // `w:firstLine` and `w:hanging` are one property, the first-line indent,
    // so a level that sets either replaces both.
    if direct.ind_first_line.is_some() || direct.ind_hanging.is_some() {
        effective.ind_first_line = direct.ind_first_line;
        effective.ind_hanging = direct.ind_hanging;
    }
    if direct.keep_next.is_some() {
        effective.keep_next = direct.keep_next;
    }
    if direct.keep_lines.is_some() {
        effective.keep_lines = direct.keep_lines;
    }
    if direct.page_break_before.is_some() {
        effective.page_break_before = direct.page_break_before;
    }
    if direct.widow_control.is_some() {
        effective.widow_control = direct.widow_control;
    }
    if direct.suppress_auto_hyphens.is_some() {
        effective.suppress_auto_hyphens = direct.suppress_auto_hyphens;
    }
    if direct.bidi.is_some() {
        effective.bidi = direct.bidi;
    }
    if direct.borders.is_some() {
        effective.borders = direct.borders.clone();
    }
    style_resolver::merge_tab_stops(&mut effective.tabs, direct.tabs.as_ref());
    if direct.shading.is_some() {
        effective.shading = direct.shading.clone();
    }
    if direct.num_id.is_some() {
        effective.num_id = direct.num_id;
    }
    if direct.num_ilvl.is_some() {
        effective.num_ilvl = direct.num_ilvl;
    }
}

/// Word's line-number gap when `w:lnNumType` leaves `w:distance` out.
const AUTOMATIC_LINE_NUMBER_DISTANCE: f64 = 18.0;

/// Bytes the modeled `w:sectPr` children own outside `CT_SectPr` itself.
///
/// The note, paper-source, page-border and line-number members are boxed, so
/// the struct's own size does not account for them, and each of them owns
/// retained attribute and raw-child storage of its own.
fn section_child_retained_bytes(section: &CT_SectPr) -> usize {
    let retained_attributes = |attributes: &Vec<(String, String)>| {
        attributes
            .capacity()
            .saturating_mul(std::mem::size_of::<(String, String)>())
            .saturating_add(
                attributes
                    .iter()
                    .map(|(name, value)| name.capacity().saturating_add(value.capacity()))
                    .fold(0usize, usize::saturating_add),
            )
    };
    let note_bytes = |notes: &oxoffice_docx_oxml::document::CT_NoteProperties| {
        std::mem::size_of::<oxoffice_docx_oxml::document::CT_NoteProperties>()
            .saturating_add(
                [&notes.pos, &notes.num_fmt, &notes.num_restart]
                    .into_iter()
                    .map(|value| value.as_ref().map_or(0, String::capacity))
                    .fold(0usize, usize::saturating_add),
            )
            .saturating_add(
                notes
                    .extra_xml
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(usize, Vec<u8>)>()),
            )
            .saturating_add(
                notes
                    .extra_xml
                    .iter()
                    .map(|(_, raw)| raw.capacity())
                    .fold(0usize, usize::saturating_add),
            )
    };
    let edge_bytes = |edge: &Option<oxoffice_docx_oxml::borders::CT_BorderEdge>| {
        edge.as_ref().map_or(0, |edge| {
            edge.color
                .as_ref()
                .map_or(0, String::capacity)
                .saturating_add(retained_attributes(&edge.extra_attributes))
        })
    };

    let mut bytes = section.text_direction.as_ref().map_or(0, String::capacity);
    if let Some(notes) = section.footnote_pr.as_deref() {
        bytes = bytes.saturating_add(note_bytes(notes));
    }
    if let Some(notes) = section.endnote_pr.as_deref() {
        bytes = bytes.saturating_add(note_bytes(notes));
    }
    if let Some(source) = section.paper_source.as_deref() {
        bytes = bytes
            .saturating_add(std::mem::size_of::<
                oxoffice_docx_oxml::document::CT_PaperSource,
            >())
            .saturating_add(retained_attributes(&source.extra_attributes));
    }
    if let Some(numbering) = section.line_numbers.as_deref() {
        bytes = bytes
            .saturating_add(std::mem::size_of::<
                oxoffice_docx_oxml::document::CT_LineNumber,
            >())
            .saturating_add(retained_attributes(&numbering.extra_attributes));
    }
    if let Some(borders) = section.page_borders.as_deref() {
        bytes = bytes
            .saturating_add(std::mem::size_of::<
                oxoffice_docx_oxml::document::CT_PageBorders,
            >())
            .saturating_add(retained_attributes(&borders.extra_attributes))
            .saturating_add(
                borders
                    .extra_xml
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Vec<u8>>()),
            )
            .saturating_add(
                borders
                    .extra_xml
                    .iter()
                    .map(Vec::capacity)
                    .fold(0usize, usize::saturating_add),
            )
            .saturating_add(edge_bytes(&borders.top))
            .saturating_add(edge_bytes(&borders.left))
            .saturating_add(edge_bytes(&borders.bottom))
            .saturating_add(edge_bytes(&borders.right));
    }
    bytes
}

/// Convert section properties to page geometry.
pub(crate) fn sect_pr_to_geometry(sect_pr: &CT_SectPr) -> PageGeometry {
    let geometry = PageGeometry {
        page_width: sect_pr.page_width.map(|t| t.to_pt()).unwrap_or(612.0),
        page_height: sect_pr.page_height.map(|t| t.to_pt()).unwrap_or(792.0),
        margin_top: sect_pr.margin_top.map(|t| t.to_pt()).unwrap_or(72.0),
        margin_right: sect_pr.margin_right.map(|t| t.to_pt()).unwrap_or(72.0),
        margin_bottom: sect_pr.margin_bottom.map(|t| t.to_pt()).unwrap_or(72.0),
        margin_left: sect_pr.margin_left.map(|t| t.to_pt()).unwrap_or(72.0),
        header_distance: sect_pr.header_distance.map(|t| t.to_pt()).unwrap_or(36.0),
        footer_distance: sect_pr.footer_distance.map(|t| t.to_pt()).unwrap_or(36.0),
        columns: Vec::new(),
        column_separator: false,
        page_borders: sect_pr_page_borders(sect_pr),
        line_numbers: sect_pr_line_numbers(sect_pr),
        vertical_alignment: sect_pr.vertical_alignment,
        mirror_margins: false,
        do_not_use_html_paragraph_auto_spacing: false,
        // `w:sectPr/w:textDirection` is authored and preserved. This is the
        // render projection over it, and it writes nothing back.
        body_rotation: sect_pr
            .text_direction
            .as_deref()
            .and_then(crate::table::text_direction_rotation),
    };
    resolve_column_tracks(sect_pr, geometry)
}

/// Resolve a section's column tracks onto its geometry.
///
/// A section that resolves to one column is returned untouched, so its content
/// width stays the exact expression it evaluated before columns existed. A
/// neutral one-track form would reassociate the arithmetic and move every
/// recorded baseline in the workspace, which is why this bypasses rather than
/// generalises.
fn resolve_column_tracks(sect_pr: &CT_SectPr, geometry: PageGeometry) -> PageGeometry {
    let Some(columns) = sect_pr.columns.as_ref() else {
        return geometry;
    };
    let explicit = columns.equal_width == Some(false) || !columns.columns.is_empty();
    let widths: Vec<(f64, f64)> = if explicit
        && columns.columns.len() > 1
        && columns.columns.iter().all(|column| column.width.is_some())
    {
        columns
            .columns
            .iter()
            .map(|column| {
                (
                    column.width.map_or(0.0, |value| value.to_pt()),
                    column.space.map_or(0.0, |value| value.to_pt()),
                )
            })
            .collect()
    } else {
        let count = columns.num.unwrap_or(1);
        if count < 2 {
            return geometry;
        }
        let space = columns.space.map_or(0.0, |value| value.to_pt());
        let measure = geometry.text_measure();
        let width = (measure - space * f64::from(count - 1)) / f64::from(count);
        (0..count).map(|_| (width, space)).collect()
    };

    if widths.iter().any(|(width, _)| *width <= 0.0) {
        return geometry;
    }

    let mut tracks = Vec::with_capacity(widths.len());
    let mut x = geometry.margin_left;
    for (width, space) in widths {
        tracks.push(ColumnTrack { x, width });
        x += width + space;
    }
    PageGeometry {
        column_separator: columns.sep.unwrap_or(false),
        columns: tracks,
        ..geometry
    }
}

/// Resolve `w:pgBorders` into the frame the paginator draws.
fn sect_pr_page_borders(sect_pr: &CT_SectPr) -> Option<PageBorderFrame> {
    let borders = sect_pr.page_borders.as_deref()?;
    if borders.top.is_none()
        && borders.left.is_none()
        && borders.bottom.is_none()
        && borders.right.is_none()
    {
        return None;
    }
    Some(PageBorderFrame {
        display: borders.display.unwrap_or(ST_PageBorderDisplay::AllPages),
        offset_from: borders.offset_from.unwrap_or(ST_PageBorderOffset::Text),
        in_front: borders.z_order != Some(ST_PageBorderZOrder::Back),
        edges: CT_PBdr {
            top: borders.top.clone(),
            left: borders.left.clone(),
            bottom: borders.bottom.clone(),
            right: borders.right.clone(),
            between: None,
            bar: None,
        },
    })
}

/// Resolve `w:lnNumType` into the numbering the paginator draws.
fn sect_pr_line_numbers(sect_pr: &CT_SectPr) -> Option<LineNumbering> {
    let numbering = sect_pr.line_numbers.as_deref()?;
    Some(LineNumbering {
        count_by: numbering.count_by?.max(1),
        start: numbering.start.unwrap_or(0),
        distance: numbering
            .distance
            .map_or(AUTOMATIC_LINE_NUMBER_DISTANCE, |value| value.to_pt()),
        restart: numbering.restart.unwrap_or(ST_LineNumberRestart::NewPage),
        font_id: None,
    })
}

fn prepare_word_line_numbers(
    input: &LayoutInput,
    fonts: &mut FontManager,
) -> (Option<paginator::WordLineNumberStyle>, Vec<Diagnostic>) {
    if !document_sections(&input.document)
        .into_iter()
        .any(|section| sect_pr_line_numbers(section).is_some())
    {
        return (None, Vec::new());
    }
    let mut diagnostics = Vec::new();
    let properties = style_resolver::line_number_properties(&input.styles, &mut diagnostics);
    let size = properties.sz.map_or(11.0, |value| value.to_pt());
    let bold = properties.bold.unwrap_or(false);
    let italic = properties.italic.unwrap_or(false);
    let family = resolve_font_family(&properties, input.theme.as_ref(), WordFontSlot::Ascii);
    let font_id = if size.is_finite() && size > 0.0 {
        match fonts.resolve_font(family.as_deref(), bold, italic) {
            Ok(font) => Some(font),
            Err(error) => {
                diagnostics.push(Diagnostic {
                    message: format!("line-number font could not be resolved: {error}"),
                });
                None
            }
        }
    } else {
        diagnostics.push(Diagnostic {
            message: "line-number nominal font size must be finite and positive".into(),
        });
        None
    };
    (
        Some(paginator::WordLineNumberStyle {
            font_id,
            size,
            bold,
            italic,
            kerning: style_resolver::word_kerning_enabled(&properties),
        }),
        diagnostics,
    )
}

/// Page geometry for one whole section, with the state `sect_pr_to_geometry`
/// cannot reach on its own.
///
/// The settings part owns mirrored margins, and the line-number font must be
/// resolved before pagination because the paginator borrows the font manager
/// immutably. Both are read only where a section is finished, so a document
/// that uses neither takes exactly the path it took before.
fn section_page_geometry(
    sect_pr: &CT_SectPr,
    input: &LayoutInput,
    line_number_style: Option<paginator::WordLineNumberStyle>,
    diagnostics: &mut Vec<Diagnostic>,
) -> PageGeometry {
    let mut geometry = sect_pr_to_geometry(sect_pr);
    // A vertical section fills one transposed band. Composing it with column
    // tracks would transpose each track about its own centre while the page
    // rotates about one, so the tracks are dropped and the fact is recorded
    // rather than painted wrong.
    if geometry.body_rotation.is_some() && !geometry.columns.is_empty() {
        push_unique_diagnostic(
            diagnostics,
            "vertical section text is laid out in one column track".to_owned(),
        );
        geometry.columns = Vec::new();
        geometry.column_separator = false;
    }
    if geometry.vertical_alignment == Some(ST_VerticalJc::Both) {
        diagnostics.push(Diagnostic {
            message: "section vertical alignment both is laid out as top, vertical distribution is not implemented"
                .to_owned(),
        });
    }
    if input.mirror_margins {
        geometry.mirror_margins = true;
        let gutter = sect_pr.gutter.map_or(0.0, |value| value.to_pt());
        if input.gutter_at_top {
            geometry.margin_top += gutter;
        } else {
            geometry.margin_left += gutter;
        }
        // The gutter moved the text measure, so the tracks resolved against
        // the ungutttered page are stale. Clearing them first keeps a bypass
        // in the second pass from leaving the first pass's answer behind.
        geometry.columns = Vec::new();
        geometry.column_separator = false;
        geometry = resolve_column_tracks(sect_pr, geometry);
    }
    geometry.do_not_use_html_paragraph_auto_spacing = input.do_not_use_html_paragraph_auto_spacing;
    if let Some(line_numbers) = geometry.line_numbers.as_mut() {
        line_numbers.font_id = line_number_style.and_then(|style| style.font_id);
    }
    geometry
}

fn section_page_number_start(sect_pr: &CT_SectPr) -> Option<usize> {
    effective_section_page_number(sect_pr).start
}

fn xml_local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn raw_root_start_tag(raw: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut cursor = 0usize;
    while cursor < raw.len() && raw[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    if raw.get(cursor) != Some(&b'<') {
        return None;
    }
    cursor += 1;
    if matches!(raw.get(cursor), Some(b'!' | b'?' | b'/')) {
        return None;
    }
    let name_start = cursor;
    while cursor < raw.len()
        && !raw[cursor].is_ascii_whitespace()
        && !matches!(raw[cursor], b'>' | b'/')
    {
        cursor += 1;
    }
    if cursor == name_start {
        return None;
    }
    let name_end = cursor;
    let attributes_start = cursor;
    let mut quote = None;
    while cursor < raw.len() {
        match (quote, raw[cursor]) {
            (None, b'\'' | b'"') => quote = Some(raw[cursor]),
            (Some(expected), found) if expected == found => quote = None,
            (None, b'>') => {
                return Some((&raw[name_start..name_end], &raw[attributes_start..cursor]));
            }
            _ => {}
        }
        cursor += 1;
    }
    None
}

fn parse_raw_attributes(attributes: &[u8]) -> Option<Vec<(&[u8], &[u8])>> {
    let mut parsed = Vec::new();
    let mut cursor = 0usize;
    while cursor < attributes.len() {
        while cursor < attributes.len() && attributes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor == attributes.len() || attributes[cursor] == b'/' {
            break;
        }
        let name_start = cursor;
        while cursor < attributes.len()
            && !attributes[cursor].is_ascii_whitespace()
            && attributes[cursor] != b'='
        {
            cursor += 1;
        }
        let name_end = cursor;
        while cursor < attributes.len() && attributes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if attributes.get(cursor) != Some(&b'=') {
            cursor = cursor.saturating_add(1);
            continue;
        }
        cursor += 1;
        while cursor < attributes.len() && attributes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let quote = *attributes.get(cursor)?;
        if !matches!(quote, b'\'' | b'"') {
            return None;
        }
        cursor += 1;
        let value_start = cursor;
        while cursor < attributes.len() && attributes[cursor] != quote {
            cursor += 1;
        }
        let value_end = cursor;
        cursor += 1;
        parsed.push((
            &attributes[name_start..name_end],
            &attributes[value_start..value_end],
        ));
    }
    Some(parsed)
}

fn raw_name_has_namespace(
    name: &[u8],
    attributes: &[(&[u8], &[u8])],
    expected: &str,
    is_attribute: bool,
) -> bool {
    let prefix = name
        .iter()
        .rposition(|byte| *byte == b':')
        .map(|separator| &name[..separator]);
    let namespace = match prefix {
        Some(prefix) => attributes.iter().find_map(|(attribute_name, value)| {
            attribute_name
                .strip_prefix(b"xmlns:")
                .is_some_and(|declared| declared == prefix)
                .then_some(*value)
        }),
        None if !is_attribute => attributes
            .iter()
            .find_map(|(attribute_name, value)| (*attribute_name == b"xmlns").then_some(*value)),
        None => None,
    };
    match namespace {
        Some(namespace) => decode_xml_attribute(namespace).is_some_and(|value| value == expected),
        None => prefix == Some(b"w".as_slice()) && expected == oxoffice_docx_oxml::namespace::W_NS,
    }
}

fn decode_xml_attribute(value: &[u8]) -> Option<String> {
    let value = std::str::from_utf8(value).ok()?;
    let mut decoded = String::with_capacity(value.len());
    let mut cursor = 0usize;
    while let Some(relative_start) = value[cursor..].find('&') {
        let entity_start = cursor + relative_start;
        decoded.push_str(&value[cursor..entity_start]);
        let entity_end = entity_start + value[entity_start..].find(';')?;
        let entity = &value[entity_start + 1..entity_end];
        match entity {
            "amp" => decoded.push('&'),
            "apos" => decoded.push('\''),
            "gt" => decoded.push('>'),
            "lt" => decoded.push('<'),
            "quot" => decoded.push('"'),
            numeric if numeric.starts_with("#x") => {
                decoded.push(char::from_u32(
                    u32::from_str_radix(&numeric[2..], 16).ok()?,
                )?);
            }
            numeric if numeric.starts_with('#') => {
                decoded.push(char::from_u32(numeric[1..].parse().ok()?)?);
            }
            _ => return None,
        }
        cursor = entity_end + 1;
    }
    decoded.push_str(&value[cursor..]);
    Some(decoded)
}

/// Lay out header and footer content (both Default and First-page).
fn layout_header_footer(
    engine: &mut Engine,
    sect_pr: &CT_SectPr,
    input: &LayoutInput,
    styles: &CT_Styles,
    media: &MediaRegistry,
    diagnostics: &mut Vec<Diagnostic>,
    sources: Option<&SourceRegistry>,
    reference_state: &mut NumberingState,
) -> Result<Option<(HeaderFooterContent, HeaderFooterSemantics)>> {
    let mut has_content = false;
    let mut header_blocks = Vec::new();
    let mut footer_blocks = Vec::new();
    let mut first_header_blocks = Vec::new();
    let mut first_footer_blocks = Vec::new();
    let mut even_header_blocks = Vec::new();
    let mut even_footer_blocks = Vec::new();
    let mut header_directions = Vec::new();
    let mut footer_directions = Vec::new();
    let mut first_header_directions = Vec::new();
    let mut first_footer_directions = Vec::new();
    let mut even_header_directions = Vec::new();
    let mut even_footer_directions = Vec::new();
    let mut watermark = None;
    let mut first_watermark = None;
    let mut even_watermark = None;
    let even_headers_active = sect_pr
        .header_refs
        .iter()
        .any(|reference| reference.hdr_ftr_type == HdrFtrType::Even);

    let geometry = sect_pr_to_geometry(sect_pr);
    // Headers and footers span the text measure whatever the body's columns do.
    let width = geometry.text_measure();

    for href in &sect_pr.header_refs {
        let (target_blocks, target_directions, target_watermark) = match href.hdr_ftr_type {
            HdrFtrType::Default => (&mut header_blocks, &mut header_directions, &mut watermark),
            HdrFtrType::First => (
                &mut first_header_blocks,
                &mut first_header_directions,
                &mut first_watermark,
            ),
            HdrFtrType::Even => (
                &mut even_header_blocks,
                &mut even_header_directions,
                &mut even_watermark,
            ),
        };
        if let Some(hdr) = input.headers.get(&href.rel_id) {
            let mut story_num_state = reference_state.references_only();
            let content = layout_header_footer_variant(
                engine,
                HeaderFooterStoryKind::Header,
                href.hdr_ftr_type,
                sect_pr,
                &href.rel_id,
                hdr,
                input,
                styles,
                media,
                &mut story_num_state,
                diagnostics,
                sources,
                width,
                geometry.clone(),
            )?;
            reference_state.merge_references(&story_num_state);
            engine
                .numbering_by_source
                .extend(story_num_state.take_resolved());
            target_blocks.extend(content.blocks);
            target_directions.extend(content.directions);
            if target_watermark.is_none() {
                *target_watermark = content.watermark;
            }
            has_content = true;
        }
    }

    for fref in &sect_pr.footer_refs {
        let (target_blocks, target_directions) = match fref.hdr_ftr_type {
            HdrFtrType::Default => (&mut footer_blocks, &mut footer_directions),
            HdrFtrType::First => (&mut first_footer_blocks, &mut first_footer_directions),
            HdrFtrType::Even => (&mut even_footer_blocks, &mut even_footer_directions),
        };
        if let Some(ftr) = input.footers.get(&fref.rel_id) {
            let mut story_num_state = reference_state.references_only();
            let content = layout_header_footer_variant(
                engine,
                HeaderFooterStoryKind::Footer,
                fref.hdr_ftr_type,
                sect_pr,
                &fref.rel_id,
                ftr,
                input,
                styles,
                media,
                &mut story_num_state,
                diagnostics,
                sources,
                width,
                geometry.clone(),
            )?;
            reference_state.merge_references(&story_num_state);
            engine
                .numbering_by_source
                .extend(story_num_state.take_resolved());
            target_blocks.extend(content.blocks);
            target_directions.extend(content.directions);
            has_content = true;
        }
    }

    if has_content {
        Ok(Some((
            HeaderFooterContent {
                header_blocks,
                footer_blocks,
                first_header_blocks,
                first_footer_blocks,
                even_header_blocks,
                even_footer_blocks,
                even_headers_active,
                watermark,
                first_watermark,
                even_watermark,
            },
            HeaderFooterSemantics {
                header_directions,
                footer_directions,
                first_header_directions,
                first_footer_directions,
                even_header_directions,
                even_footer_directions,
            },
        )))
    } else {
        Ok(None)
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_header_footer_variant(
    engine: &mut Engine,
    story_kind: HeaderFooterStoryKind,
    variant: HdrFtrType,
    sect_pr: &CT_SectPr,
    relationship_id: &str,
    part: &oxoffice_docx_oxml::header_footer::CT_HdrFtr,
    input: &LayoutInput,
    styles: &CT_Styles,
    media: &MediaRegistry,
    num_state: &mut NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
    sources: Option<&SourceRegistry>,
    width: f64,
    geometry: PageGeometry,
) -> Result<HeaderFooterVariantContent> {
    let rich_story = match story_kind {
        HeaderFooterStoryKind::Header => WordStory::Header {
            relationship_id: relationship_id.to_owned(),
        },
        HeaderFooterStoryKind::Footer => WordStory::Footer {
            relationship_id: relationship_id.to_owned(),
        },
    };
    let rich_body = input.story_bodies.get(&rich_story);
    let cache_safe = rich_body.is_none_or(|body| {
        body.content.iter().all(|content| match content {
            BodyContent::Paragraph(paragraph) => paragraph_is_cache_safe(paragraph, styles),
            _ => false,
        })
    }) && header_footer_section_is_cache_safe(sect_pr)
        && header_footer_part_is_cache_safe(part, styles);
    let resolved_part_bytes = match story_kind {
        HeaderFooterStoryKind::Header => part.to_xml_header(),
        HeaderFooterStoryKind::Footer => part.to_xml_footer(),
    };
    if !cache_safe || resolved_part_bytes.is_err() {
        return layout_header_footer_variant_uncached(
            story_kind,
            relationship_id,
            part,
            input,
            styles,
            media,
            &mut engine.font_manager,
            num_state,
            diagnostics,
            sources,
            false,
            width,
            geometry,
        );
    }

    let key = HeaderFooterCacheKey {
        story: story_kind,
        variant,
        section: sect_pr.clone(),
        relationship_id: relationship_id.to_owned(),
        part: part.clone(),
        resolved_part_bytes: resolved_part_bytes.expect("checked resolved part bytes"),
        with_provenance: sources.is_some(),
        rich_body: rich_body.cloned(),
        source_part_name: input.story_part_names.get(&rich_story).cloned(),
    };
    let hit = engine
        .header_footer_cache_reads_enabled
        .then(|| {
            engine
                .header_footer_cache
                .iter()
                .find(|entry| entry.key == key)
                .map(|entry| {
                    (
                        entry.content.clone(),
                        entry.diagnostics.clone(),
                        entry.picture_draw_owners.clone(),
                        entry.font_trace.clone(),
                    )
                })
        })
        .flatten();
    if let Some((mut content, cached_diagnostics, picture_draw_owners, font_trace)) = hit {
        rebind_header_footer_sources(
            story_kind,
            relationship_id,
            part,
            &mut content.blocks,
            &mut content.directions,
            sources,
        )?;
        diagnostics.extend(cached_diagnostics);
        media.replay_picture_draw_owners(&picture_draw_owners);
        engine.font_manager.replay_font_trace_events(&font_trace)?;
        return Ok(content);
    }

    let diagnostics_start = diagnostics.len();
    let owners_start = media.picture_draw_owner_checkpoint();
    engine.font_manager.begin_paragraph_font_trace();
    let content_result = layout_header_footer_variant_uncached(
        story_kind,
        relationship_id,
        part,
        input,
        styles,
        media,
        &mut engine.font_manager,
        num_state,
        diagnostics,
        None,
        true,
        width,
        geometry,
    );
    let font_trace = engine.font_manager.finish_paragraph_font_events();
    let mut content = content_result?;
    let cached_diagnostics = diagnostics[diagnostics_start..].to_vec();
    let picture_draw_owners = media.picture_draw_owners_since(owners_start);
    if let Some(font_trace) = font_trace {
        let bytes =
            header_footer_cache_entry_bytes(&key, &content, &cached_diagnostics, &font_trace);
        let bytes = bytes.saturating_add(picture_draw_owner_bytes(&picture_draw_owners));
        engine.stage_header_footer_cache_entry(HeaderFooterCacheEntry {
            key,
            content: content.clone(),
            diagnostics: cached_diagnostics,
            picture_draw_owners,
            font_trace,
            bytes,
        });
    }
    rebind_header_footer_sources(
        story_kind,
        relationship_id,
        part,
        &mut content.blocks,
        &mut content.directions,
        sources,
    )?;
    Ok(content)
}

#[allow(clippy::too_many_arguments)]
fn layout_header_footer_variant_uncached(
    story_kind: HeaderFooterStoryKind,
    relationship_id: &str,
    part: &oxoffice_docx_oxml::header_footer::CT_HdrFtr,
    input: &LayoutInput,
    styles: &CT_Styles,
    media: &MediaRegistry,
    fm: &mut FontManager,
    num_state: &mut NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
    sources: Option<&SourceRegistry>,
    cache_source: bool,
    width: f64,
    geometry: PageGeometry,
) -> Result<HeaderFooterVariantContent> {
    let watermark = if story_kind == HeaderFooterStoryKind::Header {
        match part.watermarks().first() {
            Some(projected) => layout_watermark(
                projected,
                relationship_id,
                input,
                media,
                fm,
                geometry.clone(),
                diagnostics,
            )?,
            None => None,
        }
    } else {
        None
    };
    let story = match story_kind {
        HeaderFooterStoryKind::Header => WordStory::Header {
            relationship_id: relationship_id.to_owned(),
        },
        HeaderFooterStoryKind::Footer => WordStory::Footer {
            relationship_id: relationship_id.to_owned(),
        },
    };
    let part_media = media.scoped_to_part(relationship_id);
    let mut blocks = Vec::with_capacity(part.paragraphs.len());
    let mut directions = Vec::with_capacity(part.paragraphs.len());
    let items = input
        .story_bodies
        .get(&story)
        .map(body_layout_items)
        .unwrap_or_else(|| {
            part.paragraphs
                .iter()
                .enumerate()
                .map(|(index, paragraph)| MainStoryLayoutItem::Paragraph(paragraph, vec![index]))
                .collect()
        });
    for item in items {
        match item {
            MainStoryLayoutItem::Paragraph(paragraph, path) => {
                let source = if cache_source {
                    Some(CACHE_SOURCE_NODE)
                } else {
                    sources.and_then(|sources| sources.id(&story, &path))
                };
                let mut properties = style_resolver::resolve_paragraph_properties(
                    paragraph
                        .properties
                        .as_ref()
                        .and_then(|properties| properties.style_id.as_deref()),
                    styles,
                );
                if let Some(direct) = &paragraph.properties {
                    properties.merge_from(direct);
                }
                let frame = properties
                    .frame
                    .filter(|frame| frame.drop_cap.as_deref().is_none_or(|kind| kind == "none"));
                let measure = frame
                    .as_ref()
                    .and_then(|frame| frame.w)
                    .map_or(width, |value| value.to_pt().max(1.0));
                let (mut block, direction, mut paint) = layout_paragraph_with_source_and_direction(
                    paragraph,
                    measure,
                    styles,
                    input,
                    &part_media,
                    fm,
                    num_state,
                    diagnostics,
                    source,
                    sources.and_then(|registry| registry.id(&story, &path)),
                    None,
                )?;
                if !cache_source && let Some(registry) = sources {
                    registry.bind_text_boxes(&mut block, &mut paint, source)?;
                }
                blocks.push(block);
                directions.push(paginator::HeaderFooterBlockSemantics {
                    paint: Arc::new(paint),
                    direction,
                    placement: frame.map_or(paginator::HeaderFooterPlacement::Flow, |properties| {
                        paginator::HeaderFooterPlacement::Frame {
                            properties,
                            paragraphs: 1,
                        }
                    }),
                    ..Default::default()
                });
            }
            MainStoryLayoutItem::Table(table, path) => {
                let (table, semantics) = table::layout_table_with_provenance(
                    table,
                    width,
                    styles,
                    input,
                    &part_media,
                    fm,
                    num_state,
                    diagnostics,
                    sources,
                    &story,
                    &path,
                    None,
                )?;
                let rows =
                    paginator::story_table_rows(&table, &semantics, &geometry, part_media.media())?;
                let table = Arc::new(table);
                let semantics = Arc::new(semantics);
                directions.extend(rows.iter().enumerate().map(|(row, (_, paint))| {
                    paginator::HeaderFooterBlockSemantics {
                        paint: Arc::new(paint.clone()),
                        placement: paginator::HeaderFooterPlacement::TableRow {
                            table: Arc::clone(&table),
                            semantics: Arc::clone(&semantics),
                            row,
                        },
                        ..Default::default()
                    }
                }));
                blocks.extend(rows.into_iter().map(|(block, _)| block));
            }
        }
    }
    let mut index = 0;
    while index < blocks.len() {
        if let paginator::HeaderFooterPlacement::Frame { properties, .. } =
            &directions[index].placement
        {
            let properties = properties.clone();
            let mut count = 1;
            while index + count < blocks.len()
                && matches!(&directions[index + count].placement, paginator::HeaderFooterPlacement::Frame { properties: next, .. } if *next == properties)
            {
                directions[index + count].placement =
                    paginator::HeaderFooterPlacement::FrameContinuation;
                count += 1;
            }
            directions[index].placement = paginator::HeaderFooterPlacement::Frame {
                properties,
                paragraphs: count,
            };
            price_furniture_paragraphs(
                &mut blocks[index..index + count],
                &mut directions[index..index + count],
                input.do_not_use_html_paragraph_auto_spacing,
            );
            index += count;
        } else {
            index += 1;
        }
    }
    let ordinary = blocks
        .iter()
        .zip(&directions)
        .map(|(block, facts)| {
            matches!(facts.placement, paginator::HeaderFooterPlacement::Flow).then_some(block)
        })
        .collect::<Vec<_>>();
    let groups = block::paragraph_border_groups(&ordinary);
    let mut after: f64 = 0.0;
    for index in 0..blocks.len() {
        let flow = matches!(
            &directions[index].placement,
            paginator::HeaderFooterPlacement::Flow
        ) || matches!(&directions[index].placement, paginator::HeaderFooterPlacement::TableRow { table, .. } if table.floating.is_none());
        if !flow {
            continue;
        }
        let gap = if input.do_not_use_html_paragraph_auto_spacing {
            after + blocks[index].space_before
        } else {
            after.max(blocks[index].space_before)
        };
        directions[index].flow_gap = gap;
        if matches!(
            directions[index].placement,
            paginator::HeaderFooterPlacement::Flow
        ) {
            directions[index].border_gap = if groups[index].0 { gap } else { 0.0 };
            blocks[index].remove_group_border_edges(groups[index].0, groups[index].1);
        }
        after = blocks[index].space_after;
    }
    Ok(HeaderFooterVariantContent {
        blocks,
        directions,
        watermark,
    })
}

fn price_furniture_paragraphs(
    blocks: &mut [ParagraphBlock],
    facts: &mut [paginator::HeaderFooterBlockSemantics],
    additive: bool,
) {
    let groups = block::paragraph_border_groups(&blocks.iter().map(Some).collect::<Vec<_>>());
    let mut after: f64 = 0.0;
    for (index, block) in blocks.iter_mut().enumerate() {
        let gap = if additive {
            after + block.space_before
        } else {
            after.max(block.space_before)
        };
        facts[index].flow_gap = gap;
        facts[index].border_gap = if groups[index].0 { gap } else { 0.0 };
        block.remove_group_border_edges(groups[index].0, groups[index].1);
        after = block.space_after;
    }
}

fn layout_watermark(
    watermark: &VmlWatermark,
    header_relationship_id: &str,
    input: &LayoutInput,
    media: &MediaRegistry,
    fm: &mut FontManager,
    geometry: PageGeometry,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Option<GroupElement>> {
    let (width, height, rotation, opacity) = match watermark {
        VmlWatermark::Text {
            width_pt,
            height_pt,
            rotation_degrees,
            opacity,
            ..
        }
        | VmlWatermark::Image {
            width_pt,
            height_pt,
            rotation_degrees,
            opacity,
            ..
        } => (*width_pt, *height_pt, *rotation_degrees, *opacity),
    };
    let translate = Transform {
        e: geometry.margin_left + (geometry.content_width() - width) / 2.0,
        f: geometry.margin_top + (geometry.content_height() - height) / 2.0,
        ..Transform::IDENTITY
    };
    let transform = Transform::rotate_about(rotation, width / 2.0, height / 2.0).then(translate);
    let children = match watermark {
        VmlWatermark::Text {
            text,
            color,
            font_family,
            ..
        } => {
            let Some(color) = vml_color(color) else {
                diagnostics.push(Diagnostic {
                    message: format!("VML watermark colour {color:?} is unsupported"),
                });
                return Ok(None);
            };
            let estimated = width / (text.chars().count().max(1) as f64 * 0.62);
            let font_size = (height * 0.62).min(estimated).max(1.0);
            let font_id = fm.resolve_font_for_text(
                font_family.as_deref().or(Some("Calibri")),
                false,
                false,
                text,
            )?;
            let shaped = fm.shape_text(font_id, text, font_size)?;
            let metrics = fm.metrics(font_id, font_size)?;
            vec![PositionedElement::Text(GlyphRun {
                origin: Point {
                    x: (width - shaped.width) / 2.0,
                    y: (height + metrics.ascent - metrics.descent) / 2.0,
                },
                font_id,
                font_size,
                glyph_ids: shaped.glyph_ids,
                advances: shaped.advances,
                text: text.clone(),
                source: None,
                color,
                bold: false,
                italic: false,
                field_kind: None,
                field_source: None,
                note: None,
                note_reference_source: None,
                tab_aligned: None,
            })]
        }
        VmlWatermark::Image {
            relationship_id, ..
        } => {
            let scoped_id = format!("{header_relationship_id}\0{relationship_id}");
            let Some(image) = input.images.get(&scoped_id) else {
                diagnostics.push(Diagnostic {
                    message: format!(
                        "VML watermark image relationship {relationship_id} in header {header_relationship_id} was not resolved"
                    ),
                });
                return Ok(None);
            };
            media.record_related_picture_draw_owner(relationship_id, header_relationship_id);
            let data = image.data.clone();
            vec![PositionedElement::Image {
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    width,
                    height,
                },
                content_type: image.content_type.clone(),
                media_id: media.id_for_relationship(&scoped_id),
                data,
            }]
        }
    };
    Ok(Some(GroupElement {
        transform,
        clip: None,
        opacity,
        effects: Vec::new(),
        children,
    }))
}

fn vml_color(value: &str) -> Option<Color> {
    let normalized = value.trim().to_ascii_lowercase();
    let hex = match normalized.as_str() {
        "black" => "000000",
        "silver" => "c0c0c0",
        "gray" | "grey" => "808080",
        "white" => "ffffff",
        "maroon" => "800000",
        "red" => "ff0000",
        "purple" => "800080",
        "fuchsia" | "magenta" => "ff00ff",
        "green" => "008000",
        "lime" => "00ff00",
        "olive" => "808000",
        "yellow" => "ffff00",
        "navy" => "000080",
        "blue" => "0000ff",
        "teal" => "008080",
        "aqua" | "cyan" => "00ffff",
        _ => normalized.trim_start_matches('#'),
    };
    (hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| Color::from_hex(hex))
}

/// One of the four `w:rFonts` script slots, as this engine resolves them.
///
/// `oxoffice_docx::RunFontSlot` is the same four slots on the authoring side, but
/// `oxoffice-docx-layout` sits below `oxoffice-docx` and cannot name it. The shape here
/// deliberately mirrors `WordLanguageSlot` next to it, so a reader meets one
/// pattern rather than two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordFontSlot {
    /// ASCII characters using the `w:ascii` slot.
    Ascii,
    /// Other Latin characters using the `w:hAnsi` slot.
    HighAnsi,
    /// East Asian characters using the `w:eastAsia` slot.
    EastAsia,
    /// Complex-script characters using the `w:cs` slot.
    ComplexScript,
}

/// Word's East Asian codepoint set.
///
/// Three tables ask this question, the font slot, the language slot and the
/// rich-path gate, and asking it three times is how they drifted apart. They
/// now ask it once. It is a superset of every codepoint `script_for_char`
/// calls Hangul, Kana or Han, because Word draws whole CJK-adjacent blocks
/// from `w:eastAsia` that need no complex shaping of their own.
fn is_east_asian(codepoint: u32) -> bool {
    matches!(
        codepoint,
        // Hangul jamo, including Extended-A and Extended-B, and the
        // precomposed syllables and halfwidth jamo.
        0x1100..=0x11ff
            | 0xa960..=0xa97f
            | 0xac00..=0xd7ff
            // CJK radicals, Kangxi radicals and the ideographic description
            // characters.
            | 0x2e80..=0x2eff
            | 0x2f00..=0x2fdf
            | 0x2ff0..=0x2fff
            // CJK punctuation, Kana, Bopomofo, Kanbun, the CJK strokes and
            // the enclosed and compatibility blocks.
            | 0x3000..=0x33ff
            // Han, its extension A, the compatibility ideographs and its
            // supplementary-plane extensions.
            | 0x3400..=0x9fff
            | 0xf900..=0xfaff
            | 0x20000..=0x2fa1f
            // CJK compatibility forms, small form variants, and the
            // halfwidth and fullwidth forms.
            | 0xfe30..=0xfe6f
            | 0xff00..=0xffef
    )
}

/// The `w:rFonts` slot one character resolves through.
///
/// The slot boundaries are not the [`word_language_slot`] boundaries. Word
/// routes Devanagari and Thai through `w:cs` while their language still comes
/// from `w:lang/@w:val`, which is why these are two tables and not one.
///
/// A character whose codepoint settles its own script decides its own slot and
/// ignores `w:rFonts/@w:hint`. Everything else is Word's ambiguous set, the
/// punctuation, symbols, digits, Greek and Cyrillic that belong to no script
/// in particular, and there the hint decides. That is the whole reason the
/// attribute exists, so a table that classified the ambiguous set by codepoint
/// would leave the hint inert on exactly the characters it is written for.
fn word_font_slot(character: char, hint: Option<&str>) -> WordFontSlot {
    let codepoint = character as u32;
    match codepoint {
        // Hebrew, Arabic, Syriac, Thaana and the Arabic presentation forms,
        // plus the Devanagari and Thai ranges Word also draws from `w:cs`.
        0x0590..=0x08ff | 0x0900..=0x097f | 0x0e00..=0x0e7f | 0xfb1d..=0xfdff | 0xfe70..=0xfeff => {
            WordFontSlot::ComplexScript
        }
        // Every CJK-adjacent block, which Word draws from `w:eastAsia`.
        codepoint if is_east_asian(codepoint) => WordFontSlot::EastAsia,
        // The Latin letters Word draws from `w:ascii`.
        0x0041..=0x005a | 0x0061..=0x007a => WordFontSlot::Ascii,
        // The Latin supplement and extensions, the IPA extensions and
        // spacing modifier letters, Latin Extended-D and the Latin ligatures,
        // which Word draws from `w:hAnsi`. The IPA and modifier blocks are
        // Latin script by Unicode even though this workspace's shaping table
        // stops at U+024F, so their slot is not in doubt either.
        0x00c0..=0x02ff | 0x1e00..=0x1eff | 0xa720..=0xa7ff | 0xfb00..=0xfb1c => {
            WordFontSlot::HighAnsi
        }
        // Word's ambiguous set. Only here does `w:hint` decide, and without
        // one the seven-bit range keeps `w:ascii` and everything above it
        // takes `w:hAnsi`, which is what each did before slots existed.
        _ => match hint {
            Some("eastAsia") => WordFontSlot::EastAsia,
            Some("cs") => WordFontSlot::ComplexScript,
            _ if codepoint <= 0x007f => WordFontSlot::Ascii,
            _ => WordFontSlot::HighAnsi,
        },
    }
}

/// The `w:rFonts` slot a whole run resolves through.
///
/// One family is resolved per run, because `resolve_font_for_text` already
/// picks one face for the run's entire text and replaces it wholesale when it
/// cannot draw the run.
///
/// Only an alphabetic character claims a slot. Spaces, digits and punctuation
/// take whatever the letters take, so they do not decide it while a letter is
/// present. A run with no letter at all has nothing to follow, so its own
/// characters decide instead, which is how a run of East Asian punctuation or
/// fullwidth digits reaches `w:eastAsia`. A run of exactly that shape is
/// ordinary in Japanese and Chinese prose, because Word splits runs at
/// formatting, proofing and revision boundaries.
///
/// Both passes take the same consensus rule, so a run's answer never turns on
/// which character the author typed first. Any disagreement resolves to
/// `w:ascii`, because preferring either side would be wrong for the other and
/// `w:ascii` is what such a run resolved through before any slot existed. A
/// run of ASCII therefore resolves exactly as it always has.
///
/// The second pass drops its `w:ascii` candidates before counting them, since
/// `w:ascii` is also the answer a disagreement gives, so counting them would
/// make every run of ideographic punctuation around a space answer `w:ascii`
/// twice over.
fn word_font_slot_for_text(text: &str, hint: Option<&str>) -> WordFontSlot {
    /// The one slot every candidate agrees on, `Ascii` if they disagree, or
    /// None if there was no candidate. Generic over the iterator because it is
    /// called with two different ones just below.
    fn consensus(slots: impl Iterator<Item = WordFontSlot>) -> Option<WordFontSlot> {
        let mut claimed: Option<WordFontSlot> = None;
        for slot in slots {
            match claimed {
                None => claimed = Some(slot),
                Some(existing) if existing == slot => {}
                Some(_) => return Some(WordFontSlot::Ascii),
            }
        }
        claimed
    }

    if let Some(slot) = consensus(
        text.chars()
            .filter(|character| character.is_alphabetic())
            .map(|character| word_font_slot(character, hint)),
    ) {
        return slot;
    }

    // `w:ascii` candidates are dropped rather than counted, because `w:ascii`
    // is also the answer a disagreement gives, so counting them would make
    // every mixed run of punctuation answer `w:ascii` twice over.
    consensus(
        text.chars()
            .map(|character| word_font_slot(character, hint))
            .filter(|slot| *slot != WordFontSlot::Ascii),
    )
    .unwrap_or(WordFontSlot::Ascii)
}

/// The explicit `w:rFonts` family for one slot.
///
/// Word falls back to the `w:ascii` family when the slot the character wants
/// is absent, which is also what keeps an `ascii`-only run resolving the way
/// it always has.
fn word_font_for_slot(
    rpr: &oxoffice_docx_oxml::properties::CT_RPr,
    theme: Option<&oxoffice_docx_oxml::theme::Theme>,
    slot: WordFontSlot,
) -> Option<String> {
    // Explicit font name takes priority over the theme attribute beside it.
    //
    // Word prefers the theme attribute when a producer presents both for one
    // slot, so this is a deliberate divergence, pre-declared under rule 5 of
    // .claude/skills/differential-testing.md. A document authored through this
    // facade never presents both for one slot, because setting either form
    // clears the other, so the divergence is reachable only on a producer
    // document the caller never edited. Leaving those bytes as written is what
    // the no-op save contract requires.
    let explicit = match slot {
        WordFontSlot::Ascii => &rpr.font_ascii,
        WordFontSlot::HighAnsi => &rpr.font_hansi,
        WordFontSlot::EastAsia => &rpr.font_east_asia,
        WordFontSlot::ComplexScript => &rpr.font_cs,
    };
    if explicit.is_some() {
        return explicit.clone();
    }

    // Each slot reads its own theme attribute. Reading `w:asciiTheme` for
    // every slot made `w:eastAsiaTheme` and `w:cstheme` inert and collapsed
    // majorEastAsia, minorEastAsia, majorBidi and minorBidi onto whatever the
    // ASCII slot named.
    let theme_ref = match slot {
        WordFontSlot::Ascii => &rpr.font_ascii_theme,
        WordFontSlot::HighAnsi => &rpr.font_hansi_theme,
        WordFontSlot::EastAsia => &rpr.font_east_asia_theme,
        WordFontSlot::ComplexScript => &rpr.font_cs_theme,
    }
    .as_deref()?;
    let theme = theme?;
    match theme_ref {
        // These four name the theme's `a:latin` typeface, which is the one
        // `oxoffice_docx_oxml::theme::Theme` carries.
        "majorAscii" | "majorHAnsi" => theme.major_font.as_deref(),
        "minorAscii" | "minorHAnsi" => theme.minor_font.as_deref(),
        // majorEastAsia, minorEastAsia, majorBidi and minorBidi name the
        // theme's `a:ea` and `a:cs` typefaces. `Theme` models neither, so
        // there is no entry to read and the answer here is None, which falls
        // through to the run's own `w:ascii` family. Word behaves the same
        // way, since `a:cs` is empty in every stock Office theme.
        //
        // Declining *here* is what stops a Latin face, which usually cannot
        // draw the text, outranking the family the author named.
        // `resolve_font_family` answers with that face only after both slots
        // have declined, where there is nothing else left to answer with.
        _ => None,
    }
    .map(str::to_owned)
}

/// Resolve a font family from already effective, cascaded run properties.
///
/// Callers must apply document defaults, style inheritance and direct run
/// properties before calling this function. It resolves font slots and theme
/// references only and does not perform that cascade itself.
///
/// Five steps, in order. The slot's explicit family, the slot's theme font,
/// then the same two for the `w:ascii` slot, then the Latin typeface for a
/// non-Latin theme reference as a last resort, then None so the default
/// applies.
///
/// The character's own slot is resolved completely before the `w:ascii`
/// fallback, because returning the explicit `w:ascii` family first would leave
/// `w:eastAsiaTheme` and `w:cstheme` exactly as inert as they were before they
/// were read at all.
///
/// The last resort exists because `majorEastAsia`, `minorEastAsia`,
/// `majorBidi` and `minorBidi` decline inside a slot, and a run whose only
/// font property is one of those four would otherwise lose its theme typeface
/// entirely. Declining inside the slot and answering outside it is what makes
/// the Latin face reachable without ever letting it outrank a family the
/// author named.
pub fn resolve_font_family(
    rpr: &oxoffice_docx_oxml::properties::CT_RPr,
    theme: Option<&oxoffice_docx_oxml::theme::Theme>,
    slot: WordFontSlot,
) -> Option<String> {
    if let Some(family) = word_font_for_slot(rpr, theme, slot)
        .or_else(|| word_font_for_slot(rpr, theme, WordFontSlot::Ascii))
    {
        return Some(family);
    }

    // Last resort. The four non-Latin references decline above so they can
    // never outrank a family the author named, but a run whose only font
    // property is one of them has nothing else to fall back to, and the Latin
    // typeface is a better answer for it than the engine default. This is
    // reachable only from a producer document, since Word writes those four
    // on `w:eastAsiaTheme` and `w:cstheme` where the run's own `w:ascii`
    // family answers first.
    let theme = theme?;
    let reference = match slot {
        WordFontSlot::Ascii => &rpr.font_ascii_theme,
        WordFontSlot::HighAnsi => &rpr.font_hansi_theme,
        WordFontSlot::EastAsia => &rpr.font_east_asia_theme,
        WordFontSlot::ComplexScript => &rpr.font_cs_theme,
    }
    .as_deref()
    .or(rpr.font_ascii_theme.as_deref())?;
    match reference {
        "majorEastAsia" | "majorBidi" => theme.major_font.as_deref(),
        "minorEastAsia" | "minorBidi" => theme.minor_font.as_deref(),
        _ => None,
    }
    .map(str::to_owned)
}

/// East Asian annotation, the shared geometry behind `w:em` and `w:ruby`.
///
/// Both place a second, smaller line against a base line without changing
/// the base advance, so both are measured as one `InlineItem::Group` whose
/// width is the base width and whose ascent already includes the annotation.
/// `item_metrics` turns that ascent into line height, which is how a marked
/// or annotated line becomes taller and reaches the paginator.
///
/// The mark and phonetic glyphs are painted inside the group rather than
/// carried on the text segment, because a segment field would have to be
/// sliced in step with glyph clusters at every line break and under bidi
/// reordering, which is exactly the case count the structural rules reject.
/// The cost is that the annotated text is one unbreakable item, so an
/// emphasis-marked run is split at whitespace first to keep its break
/// opportunities.
const EMPHASIS_MARK_SCALE: f64 = 0.5;

/// The glyph Word draws for one `w:em` value, and whether it sits above the
/// base characters rather than below them.
fn emphasis_mark_glyph(mark: &ST_Em) -> Option<(char, bool)> {
    match mark {
        // U+2022 BULLET for the solid dot Word draws above the character,
        // U+FE45 SESAME DOT for the comma mark, U+25CB WHITE CIRCLE for the
        // open circle. `underDot` is the same solid dot below the base line.
        ST_Em::Dot => Some(('\u{2022}', true)),
        ST_Em::Comma => Some(('\u{FE45}', true)),
        ST_Em::Circle => Some(('\u{25CB}', true)),
        ST_Em::UnderDot => Some(('\u{2022}', false)),
        // `none` draws nothing, and a producer token outside the inventory
        // names a mark this renderer has no glyph for.
        ST_Em::None | ST_Em::Other(_) => None,
    }
}

/// Split text into maximal whitespace and non-whitespace chunks.
fn emphasis_chunks(text: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut current = None;
    for (index, character) in text.char_indices() {
        let is_space = character.is_whitespace();
        match current {
            Some(previous) if previous == is_space => {}
            Some(_) => {
                chunks.push(&text[start..index]);
                start = index;
            }
            None => start = index,
        }
        current = Some(is_space);
    }
    if start < text.len() {
        chunks.push(&text[start..]);
    }
    chunks
}

/// The bracket pair `w:combineBrackets` names, drawn around a combined run.
///
/// The ASCII pair is deliberate. Every bundled face carries it, so a combined
/// run draws the same brackets on every host, which a fullwidth pair present
/// in only some East Asian faces would not.
fn combine_bracket_pair(brackets: Option<&str>) -> (&'static str, &'static str) {
    match brackets {
        Some("round") => ("(", ")"),
        Some("square") => ("[", "]"),
        Some("angle") => ("<", ">"),
        Some("curly") => ("{", "}"),
        _ => ("", ""),
    }
}

fn annotation_recipe(
    kind: WordAnnotationKind,
    base: &AnnotationBase<'_>,
    source_span: Option<SourceSpan>,
) -> Result<Option<Arc<WordAnnotationRecipe>>> {
    let Some(occurrence) = base.occurrence else {
        return Ok(None);
    };
    let color = |color: Color| {
        [
            color.r.to_bits(),
            color.g.to_bits(),
            color.b.to_bits(),
            color.a.to_bits(),
        ]
    };
    if [base.font_size, base.baseline_offset, base.spacing]
        .iter()
        .any(|value| !value.is_finite())
    {
        return Err(LayoutError::Layout(
            "nonfinite Word annotation producer".into(),
        ));
    }
    let font_family = base
        .font_family
        .map(|family| {
            let mut value = String::new();
            value
                .try_reserve_exact(family.len())
                .map_err(|_| LayoutError::Layout("annotation family allocation refused".into()))?;
            value.push_str(family);
            Ok(value)
        })
        .transpose()?;
    Ok(Some(Arc::new(WordAnnotationRecipe {
        occurrence,
        fallback_emphasis: if matches!(kind, WordAnnotationKind::EastAsian(_)) {
            base.fallback_emphasis.cloned()
        } else {
            None
        },
        field_kind: base.field_kind,
        source_span,
        kind,
        style: WordAnnotationStyle {
            font_id: base.font_id,
            font_size: base.font_size.to_bits(),
            color: color(base.color),
            bold: base.bold,
            italic: base.italic,
            baseline_offset: base.baseline_offset.to_bits(),
            spacing: base.spacing.to_bits(),
            underline: base.underline,
            strike: base.strike,
            dstrike: base.dstrike,
            highlight: base.highlight.map(color),
            font_family,
            kern: base.kern,
        },
    })))
}

fn annotation_field_owner(base: &AnnotationBase<'_>) -> FieldPaintOwner {
    base.occurrence
        .map_or(FieldPaintOwner::Neutral, |occurrence| {
            FieldPaintOwner::Word {
                kern: base.kern,
                legacy: true,
                occurrence: Some(occurrence),
            }
        })
}

fn register_inline_annotation(
    styles: &mut HashMap<usize, WordMultilingualStyle>,
    slot: usize,
    recipe: Option<Arc<WordAnnotationRecipe>>,
    policies: PaintPolicies,
    kind: WordAnnotationRootKind,
) {
    if let Some(recipe) = recipe {
        styles.insert(
            slot,
            WordMultilingualStyle {
                kern: recipe.style.kern,
                origin: WordInputOrigin::Annotation,
                occurrence: Some(recipe.occurrence),
                annotation: Some(WordInputAnnotation {
                    slot,
                    recipe,
                    policies,
                    kind,
                }),
                ..Default::default()
            },
        );
    }
}

fn push_empty_field_annotation(
    items: &mut Vec<InlineItem>,
    styles: &mut HashMap<usize, WordMultilingualStyle>,
    kind: WordAnnotationKind,
    base: &AnnotationBase<'_>,
    source: Option<SourceSpan>,
) -> Result<()> {
    let recipe = annotation_recipe(kind, base, source)?;
    let occurrence = base.occurrence.ok_or_else(|| {
        LayoutError::Layout("empty annotation lacks its known field owner".into())
    })?;
    let mut children = PaintedElements::default();
    children.push_field(
        PositionedElement::Text(GlyphRun {
            origin: Point { x: 0.0, y: 0.0 },
            font_id: base.font_id,
            font_size: base.font_size,
            glyph_ids: Vec::new(),
            advances: Vec::new(),
            text: String::new(),
            source,
            color: base.color,
            bold: base.bold,
            italic: base.italic,
            field_kind: base.field_kind,
            field_source: Some(occurrence.source),
            note: None,
            note_reference_source: None,
            tab_aligned: None,
        }),
        annotation_field_owner(base),
    )?;
    let (children, policies) = children.into_parts()?;
    register_inline_annotation(
        styles,
        items.len(),
        recipe,
        policies,
        WordAnnotationRootKind::EmptySpan,
    );
    items.push(InlineItem::Group {
        width: 0.0,
        height: 0.0,
        baseline: Some(0.0),
        group: GroupElement {
            transform: Transform::IDENTITY,
            clip: None,
            opacity: 1.0,
            effects: Vec::new(),
            children,
        },
    });
    Ok(())
}

/// Lay one run out under `w:eastAsianLayout`, or report that it does not apply.
///
/// `w:combine` compresses the run into one base-character advance inside the
/// bracket pair `w:combineBrackets` names. `w:vert` rotates the run 90 degrees
/// within its line, and `w:vertCompress` narrows the rotated run to one base
/// advance. A run that sets neither returns `false` and takes the ordinary
/// path. A run that sets this and `w:em` takes this, because combining and
/// rotating change the run's advance while an emphasis mark decorates it.
#[allow(clippy::too_many_arguments)]
fn push_east_asian_layout_text(
    inline_items: &mut Vec<InlineItem>,
    styles: &mut HashMap<usize, WordMultilingualStyle>,
    fm: &mut FontManager,
    layout: &CT_EastAsianLayout,
    text: &str,
    base: &AnnotationBase<'_>,
    source: Option<SourceSpan>,
) -> Result<bool> {
    let recipe = annotation_recipe(WordAnnotationKind::EastAsian(layout.clone()), base, source)?;
    let combine = layout.combine == Some(true);
    let vert = layout.vert == Some(true);
    if !combine && !vert {
        return Ok(false);
    }
    let metrics = fm.metrics(base.font_id, base.font_size)?;
    let ascent = metrics.ascent.max(0.0);
    let descent = metrics.descent.max(0.0);
    let mut shaped = fm.shape_text_with_kerning(base.font_id, text, base.font_size, base.kern)?;
    if base.spacing != 0.0 {
        for advance in &mut shaped.advances {
            *advance += base.spacing;
        }
        shaped.width += base.spacing * shaped.advances.len() as f64;
    }
    if shaped.width <= 0.0 {
        return Ok(false);
    }

    // One base-character advance is one em of the run's own size, which is
    // the character cell `w:combine` fits a run into and the advance
    // `w:vertCompress` narrows a rotated run to. It is deliberately not the
    // run's first shaped advance: shaping returns visual order, so a
    // right-to-left run's first advance is its logically last character, and a
    // run that begins with a space would collapse into the space.
    let base_advance = base.font_size;

    let raise = base.baseline_offset;
    let group_ascent = ascent + raise.max(0.0);
    let group_descent = descent + (-raise).max(0.0);
    let baseline = group_ascent - raise;
    let glyph_run = |x: f64, y: f64, shaped: &ShapedText, text: &str, field_base: bool| {
        PositionedElement::Text(GlyphRun {
            origin: Point { x, y },
            font_id: base.font_id,
            font_size: base.font_size,
            glyph_ids: shaped.glyph_ids.clone(),
            advances: shaped.advances.clone(),
            text: text.to_owned(),
            source,
            color: base.color,
            bold: base.bold,
            italic: base.italic,
            field_kind: if field_base { base.field_kind } else { None },
            field_source: if field_base {
                base.occurrence.map(|owner| owner.source)
            } else {
                None
            },
            note: None,
            note_reference_source: None,
            tab_aligned: None,
        })
    };

    if combine {
        let cell = base_advance;
        let (open, close) = combine_bracket_pair(layout.combine_brackets.as_deref());
        let open_shaped = match open.is_empty() {
            true => None,
            false => {
                Some(fm.shape_text_with_kerning(base.font_id, open, base.font_size, base.kern)?)
            }
        };
        let close_shaped = match close.is_empty() {
            true => None,
            false => {
                Some(fm.shape_text_with_kerning(base.font_id, close, base.font_size, base.kern)?)
            }
        };
        let open_width = open_shaped.as_ref().map_or(0.0, |shaped| shaped.width);
        let close_width = close_shaped.as_ref().map_or(0.0, |shaped| shaped.width);
        let width = open_width + cell + close_width;
        let mut children = Vec::new();
        let mut policies = PaintPolicies::default();
        if let Some(highlight) = base.highlight {
            children.push(PositionedElement::FilledRect {
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    width,
                    height: group_ascent + group_descent,
                },
                color: highlight,
            });
        }
        if let Some(ref open_shaped) = open_shaped {
            children.push(glyph_run(0.0, baseline, open_shaped, open, false));
        }
        let mut base_children = PaintedElements::default();
        let base_glyph = glyph_run(0.0, baseline, &shaped, text, true);
        if base.occurrence.is_some() {
            base_children.push_field(base_glyph, annotation_field_owner(base))?;
        } else {
            base_children.push_non_field(base_glyph)?;
        }
        policies.append(
            &mut children,
            base_children.wrap(PositionedElement::Group(GroupElement {
                transform: Transform {
                    a: cell / shaped.width,
                    e: open_width,
                    ..Transform::IDENTITY
                },
                clip: None,
                opacity: 1.0,
                effects: Vec::new(),
                children: Vec::new(),
            }))?,
        )?;
        if let Some(ref close_shaped) = close_shaped {
            children.push(glyph_run(
                open_width + cell,
                baseline,
                close_shaped,
                close,
                false,
            ));
        }
        push_annotation_decorations(&mut children, base, baseline, width, ascent, descent);
        register_inline_annotation(
            styles,
            inline_items.len(),
            recipe.clone(),
            policies,
            WordAnnotationRootKind::Group,
        );
        inline_items.push(InlineItem::Group {
            width,
            height: group_ascent + group_descent,
            baseline: Some(group_ascent),
            group: GroupElement {
                transform: Transform::IDENTITY,
                clip: None,
                opacity: 1.0,
                effects: Vec::new(),
                children,
            },
        });
        fm.record_font_contribution(base.font_id, base.font_family, text)?;
        if open_shaped.is_some() {
            fm.record_font_contribution(base.font_id, base.font_family, open)?;
        }
        if close_shaped.is_some() {
            fm.record_font_contribution(base.font_id, base.font_family, close)?;
        }
        return Ok(true);
    }

    // `w:vert`. The run is laid out horizontally and rotated 90 degrees, so
    // its own line height becomes the advance it takes along the line and its
    // text length becomes the height it needs.
    let thickness = group_ascent + group_descent;
    if thickness <= 0.0 {
        return Ok(false);
    }
    // A compressed run is narrowed to one base-character advance, and never
    // widened, because compression may only take space away.
    let compressed = layout.vert_compress == Some(true) && base_advance < thickness;
    let width = if compressed { base_advance } else { thickness };
    let mut children = Vec::new();
    let mut policies = PaintPolicies::default();
    if let Some(highlight) = base.highlight {
        children.push(PositionedElement::FilledRect {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: shaped.width,
                height: thickness,
            },
            color: highlight,
        });
    }
    policies.emit(
        &mut children,
        glyph_run(0.0, baseline, &shaped, text, true),
        annotation_field_owner(base),
    )?;
    push_annotation_decorations(&mut children, base, baseline, shaped.width, ascent, descent);
    // Rotate about the group origin, then bring the rotated box back into
    // positive x, then narrow it when `w:vertCompress` asked for that.
    let mut transform = Transform::rotate_about(90.0, 0.0, 0.0).then(Transform {
        e: thickness,
        ..Transform::IDENTITY
    });
    if compressed {
        transform = transform.then(Transform {
            a: width / thickness,
            ..Transform::IDENTITY
        });
    }
    register_inline_annotation(
        styles,
        inline_items.len(),
        recipe,
        policies,
        WordAnnotationRootKind::Group,
    );
    inline_items.push(InlineItem::Group {
        width,
        height: shaped.width,
        baseline: None,
        group: GroupElement {
            transform,
            clip: None,
            opacity: 1.0,
            effects: Vec::new(),
            children,
        },
    });
    fm.record_font_contribution(base.font_id, base.font_family, text)?;
    Ok(true)
}

/// The base run style an annotation draws against.
struct AnnotationBase<'a> {
    fallback_emphasis: Option<&'a ST_Em>,
    occurrence: Option<WordFieldOccurrence>,
    field_kind: Option<FieldKind>,
    kern: bool,
    font_id: FontId,
    font_size: f64,
    color: Color,
    bold: bool,
    italic: bool,
    baseline_offset: f64,
    spacing: f64,
    underline: Option<Underline>,
    strike: bool,
    dstrike: bool,
    highlight: Option<Color>,
    font_family: Option<&'a str>,
}

/// Record a diagnostic once, matching the media registry's dedupe by message.
pub(crate) fn push_unique_diagnostic(diagnostics: &mut Vec<Diagnostic>, message: String) {
    if !diagnostics.iter().any(|entry| entry.message == message) {
        diagnostics.push(Diagnostic { message });
    }
}

/// Draw the decorations a `TextSegment` would have carried, in group-local
/// coordinates against a baseline at `baseline`.
fn push_annotation_decorations(
    children: &mut Vec<PositionedElement>,
    base: &AnnotationBase<'_>,
    baseline: f64,
    width: f64,
    ascent: f64,
    descent: f64,
) {
    let line_at = |y: f64, thickness: f64, children: &mut Vec<PositionedElement>| {
        children.push(PositionedElement::Line {
            start: Point { x: 0.0, y },
            end: Point { x: width, y },
            width: thickness,
            color: base.color,
            dash_pattern: None,
        });
    };
    if let Some(style) = base.underline {
        let underline_y = baseline + descent * 0.3;
        let (thickness, second) = convert::underline_strokes(style, base.font_size);
        line_at(underline_y, thickness, children);
        if let Some(offset) = second {
            line_at(underline_y + offset, thickness, children);
        }
    }
    let thickness = base.font_size / 24.0;
    let strike_y = baseline - ascent * 0.3;
    if base.strike {
        line_at(strike_y, thickness, children);
    }
    if base.dstrike {
        let gap = thickness * 2.0;
        line_at(strike_y - gap / 2.0, thickness, children);
        line_at(strike_y + gap / 2.0, thickness, children);
    }
}

/// Project one emphasis-marked text run into inline items.
///
/// Reports `false` when the mark cannot be drawn, which leaves the caller on
/// the ordinary text path so the base text is painted unchanged. An
/// undrawable mark codepoint records a diagnostic and paints nothing, which
/// is the policy the uncovered-character path already follows.
fn push_emphasis_marked_text(
    inline_items: &mut Vec<InlineItem>,
    styles: &mut HashMap<usize, WordMultilingualStyle>,
    fm: &mut FontManager,
    diagnostics: &mut Vec<Diagnostic>,
    mark: &ST_Em,
    text: &str,
    base: &AnnotationBase<'_>,
) -> Result<bool> {
    let recipe = annotation_recipe(WordAnnotationKind::Emphasis(mark.clone()), base, None)?;
    let Some((glyph, above)) = emphasis_mark_glyph(mark) else {
        return Ok(false);
    };
    let mark_text = glyph.to_string();
    let mark_size = base.font_size * EMPHASIS_MARK_SCALE;
    let mark_font =
        fm.resolve_font_for_text(base.font_family, base.bold, base.italic, &mark_text)?;
    let mark_shaped = fm.shape_text_with_kerning(mark_font, &mark_text, mark_size, base.kern)?;
    if mark_shaped.glyph_ids.is_empty() || mark_shaped.glyph_ids.contains(&0) {
        push_unique_diagnostic(
            diagnostics,
            format!(
                "emphasis mark {} has no glyph in the resolved font and is not painted",
                mark.as_str()
            ),
        );
        return Ok(false);
    }
    let mark_metrics = fm.metrics(mark_font, mark_size)?;
    let metrics = fm.metrics(base.font_id, base.font_size)?;
    let ascent = metrics.ascent.max(0.0);
    let descent = metrics.descent.max(0.0);
    let mark_ascent = mark_metrics.ascent.max(0.0);
    let mark_descent = mark_metrics.descent.max(0.0);
    let mark_height = mark_ascent + mark_descent;

    let raise = base.baseline_offset;
    let group_ascent = if above { ascent + mark_height } else { ascent } + raise.max(0.0);
    let group_descent = if above {
        descent
    } else {
        descent + mark_height
    } + (-raise).max(0.0);
    let baseline = group_ascent - raise;
    let mark_baseline = if above {
        baseline - ascent - mark_descent
    } else {
        baseline + descent + mark_ascent
    };

    for chunk in emphasis_chunks(text) {
        let mut shaped =
            fm.shape_text_with_kerning(base.font_id, chunk, base.font_size, base.kern)?;
        if base.spacing != 0.0 {
            for advance in &mut shaped.advances {
                *advance += base.spacing;
            }
            shaped.width += base.spacing * shaped.advances.len() as f64;
        }
        let characters = chunk.chars().collect::<Vec<_>>();
        let mut children = Vec::new();
        let mut policies = PaintPolicies::default();
        if let Some(highlight) = base.highlight {
            children.push(PositionedElement::FilledRect {
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: shaped.width,
                    height: group_ascent + group_descent,
                },
                color: highlight,
            });
        }
        policies.emit(
            &mut children,
            PositionedElement::Text(GlyphRun {
                origin: Point {
                    x: 0.0,
                    y: baseline,
                },
                font_id: base.font_id,
                font_size: base.font_size,
                glyph_ids: shaped.glyph_ids.clone(),
                advances: shaped.advances.clone(),
                text: chunk.to_owned(),
                source: None,
                color: base.color,
                bold: base.bold,
                italic: base.italic,
                field_kind: base.field_kind,
                field_source: base.occurrence.map(|owner| owner.source),
                note: None,
                note_reference_source: None,
                tab_aligned: None,
            }),
            annotation_field_owner(base),
        )?;
        push_annotation_decorations(&mut children, base, baseline, shaped.width, ascent, descent);

        // One mark per base character. Glyphs and characters correspond one
        // to one for the scripts `w:em` applies to, and when shaping has
        // merged them the marks fall back to an even share of the chunk so
        // the count still matches the characters.
        let advances = if shaped.advances.len() == characters.len() {
            shaped.advances.clone()
        } else if characters.is_empty() {
            Vec::new()
        } else {
            vec![shaped.width / characters.len() as f64; characters.len()]
        };
        let mut x = 0.0;
        for (character, advance) in characters.iter().zip(&advances) {
            if !character.is_whitespace() {
                children.push(PositionedElement::Text(GlyphRun {
                    origin: Point {
                        x: x + (advance - mark_shaped.width) / 2.0,
                        y: mark_baseline,
                    },
                    font_id: mark_font,
                    font_size: mark_size,
                    glyph_ids: mark_shaped.glyph_ids.clone(),
                    advances: mark_shaped.advances.clone(),
                    text: mark_text.clone(),
                    source: None,
                    color: base.color,
                    bold: false,
                    italic: false,
                    field_kind: None,
                    field_source: None,
                    note: None,
                    note_reference_source: None,
                    tab_aligned: None,
                }));
            }
            x += advance;
        }

        fm.record_font_contribution(base.font_id, base.font_family, chunk)?;
        if characters
            .iter()
            .any(|character| !character.is_whitespace())
        {
            fm.record_font_contribution(mark_font, base.font_family, &mark_text)?;
        }
        register_inline_annotation(
            styles,
            inline_items.len(),
            recipe.clone(),
            policies,
            WordAnnotationRootKind::Group,
        );
        inline_items.push(InlineItem::Group {
            width: shaped.width,
            height: group_ascent + group_descent,
            baseline: Some(group_ascent),
            group: GroupElement {
                transform: Transform::IDENTITY,
                clip: None,
                opacity: 1.0,
                effects: Vec::new(),
                children,
            },
        });
    }
    Ok(true)
}

/// One measured annotation line, with its glyph runs laid out from x = 0 on
/// a baseline at y = 0.
struct AnnotationLine {
    width: f64,
    ascent: f64,
    descent: f64,
    /// The size the first painted run resolved to, which is what a ruby
    /// annotation halves when `w:hps` is absent.
    primary_font_size: f64,
    runs: Vec<GlyphRun>,
}

/// The paragraph context an annotation resolves its runs against.
struct AnnotationContext<'a> {
    styles: &'a CT_Styles,
    input: &'a LayoutInput,
    para_style_id: Option<&'a str>,
    table_run_properties: Option<&'a CT_RPr>,
    /// The colour of text in the automatic colour, black or white.
    automatic_color: Color,
}

/// Shape a sequence of runs onto one baseline.
///
/// `size_override` is the size `w:hps` and `w:hpsBaseText` fix for the whole
/// line, which is why it wins over the run's own `w:sz`.
fn measure_annotation_line(
    runs: &[CT_R],
    context: &AnnotationContext<'_>,
    fm: &mut FontManager,
    size_override: Option<f64>,
) -> Result<AnnotationLine> {
    let mut line = AnnotationLine {
        width: 0.0,
        ascent: 0.0,
        descent: 0.0,
        primary_font_size: size_override.unwrap_or(0.0),
        runs: Vec::new(),
    };
    for run in runs {
        let run_style_id = run.properties.as_ref().and_then(|p| p.style_id.as_deref());
        let mut effective_rpr = style_resolver::resolve_run_properties(
            context.para_style_id,
            run_style_id,
            context.styles,
            context.table_run_properties,
        );
        if let Some(ref direct_rpr) = run.properties {
            effective_rpr.merge_from(direct_rpr);
        }
        if effective_rpr.vanish == Some(true) {
            continue;
        }
        let text = run.text();
        if text.is_empty() {
            continue;
        }
        let font_size =
            size_override.unwrap_or_else(|| effective_rpr.sz.map(|hp| hp.to_pt()).unwrap_or(11.0));
        let bold = effective_rpr.bold.unwrap_or(false);
        let italic = effective_rpr.italic.unwrap_or(false);
        let family = resolve_font_family(
            &effective_rpr,
            context.input.theme.as_ref(),
            word_font_slot_for_text(&text, effective_rpr.font_hint.as_deref()),
        );
        let font_id = fm.resolve_font_for_text(family.as_deref(), bold, italic, &text)?;
        let shaped = fm.shape_text_with_kerning(
            font_id,
            &text,
            font_size,
            style_resolver::word_kerning_enabled(&effective_rpr),
        )?;
        let metrics = fm.metrics(font_id, font_size)?;
        fm.record_font_contribution(font_id, family.as_deref(), &text)?;
        if line.runs.is_empty() {
            line.primary_font_size = font_size;
        }
        line.ascent = line.ascent.max(metrics.ascent.max(0.0));
        line.descent = line.descent.max(metrics.descent.max(0.0));
        line.runs.push(GlyphRun {
            origin: Point {
                x: line.width,
                y: 0.0,
            },
            font_id,
            font_size,
            glyph_ids: shaped.glyph_ids,
            advances: shaped.advances,
            text,
            source: None,
            color: resolve_run_color(
                &effective_rpr,
                context.input.theme.as_ref(),
                context.automatic_color,
            ),
            bold,
            italic,
            field_kind: None,
            field_source: None,
            note: None,
            note_reference_source: None,
            tab_aligned: None,
        });
        line.width += shaped.width;
    }
    Ok(line)
}

/// Distribute a phonetic line over the base width per `w:rubyAlign`.
///
/// The two distribute modes widen the gaps between phonetic glyphs, which is
/// what the mode names describe, so they act on the glyph advances rather
/// than on the line origin the other modes move.
fn distribute_ruby_line(line: &mut AnnotationLine, align: Option<&ST_RubyAlign>, width: f64) {
    let slack = width - line.width;
    if slack <= 0.0 {
        return;
    }
    let glyphs = line
        .runs
        .iter()
        .map(|run| run.advances.len())
        .sum::<usize>();
    let (start, gap) = match align.unwrap_or(&ST_RubyAlign::Center) {
        ST_RubyAlign::Left => (0.0, 0.0),
        ST_RubyAlign::Right | ST_RubyAlign::RightVertical => (slack, 0.0),
        ST_RubyAlign::DistributeLetter if glyphs > 1 => (0.0, slack / (glyphs - 1) as f64),
        ST_RubyAlign::DistributeSpace if glyphs > 0 => {
            let gap = slack / (glyphs + 1) as f64;
            (gap, gap)
        }
        // Centre is the fallback for the remaining tokens, including a
        // producer value this renderer has no distribution rule for.
        _ => (slack / 2.0, 0.0),
    };
    let mut x = start;
    let mut remaining = glyphs;
    for run in &mut line.runs {
        run.origin.x = x;
        for advance in &mut run.advances {
            remaining -= 1;
            if remaining > 0 {
                *advance += gap;
            }
            x += *advance;
        }
    }
    line.width = width;
}

/// Project one ruby annotation into an inline item.
///
/// The base line and the phonetic line are measured independently, the
/// phonetic line is placed above the base at `w:hpsRaise` in the `w:hps`
/// size, and the annotation's ascent carries both, which is what makes a
/// ruby-bearing line taller for the paginator.
fn ruby_inline_item(
    ruby: &CT_Ruby,
    base_runs: &[CT_R],
    context: &AnnotationContext<'_>,
    fm: &mut FontManager,
) -> Result<Option<InlineItem>> {
    let properties = ruby.properties.as_ref();
    let base_override = properties
        .and_then(|properties| properties.hps_base_text)
        .map(|size| size.to_pt());
    let mut base = measure_annotation_line(base_runs, context, fm, base_override)?;
    if base.runs.is_empty() {
        return Ok(None);
    }
    let phonetic_size = properties
        .and_then(|properties| properties.hps)
        .map(|size| size.to_pt())
        .unwrap_or(base.primary_font_size / 2.0);
    let mut phonetic = measure_annotation_line(&ruby.ruby_text, context, fm, Some(phonetic_size))?;

    let width = base.width.max(phonetic.width);
    let raise = properties
        .and_then(|properties| properties.hps_raise)
        .map(|size| size.to_pt())
        .unwrap_or(base.ascent + phonetic.descent);
    let group_ascent = base.ascent.max(raise + phonetic.ascent);
    let group_descent = base.descent;

    distribute_ruby_line(&mut base, Some(&ST_RubyAlign::Center), width);
    distribute_ruby_line(
        &mut phonetic,
        properties.and_then(|properties| properties.align.as_ref()),
        width,
    );

    let mut children = Vec::new();
    for mut run in base.runs {
        run.origin.y = group_ascent;
        children.push(PositionedElement::Text(run));
    }
    for mut run in phonetic.runs {
        run.origin.y = group_ascent - raise;
        children.push(PositionedElement::Text(run));
    }

    Ok(Some(InlineItem::Group {
        width,
        height: group_ascent + group_descent,
        baseline: Some(group_ascent),
        group: GroupElement {
            transform: Transform::IDENTITY,
            clip: None,
            opacity: 1.0,
            effects: Vec::new(),
            children,
        },
    }))
}

/// Resolve the effective color for a run, considering theme colors.
///
/// Priority: solid text fill > resolved theme color > literal fallback > `automatic`, the
/// automatic colour that [`automatic_text_color`] gives for the fill behind
/// the text.
fn resolve_run_color(
    rpr: &oxoffice_docx_oxml::properties::CT_RPr,
    theme: Option<&oxoffice_docx_oxml::theme::Theme>,
    automatic: Color,
) -> Color {
    if let Some((hex, alpha)) = rpr.solid_text_fill() {
        let mut color = Color::from_hex(&hex);
        color.a = alpha;
        return color;
    }

    // If theme color is specified, resolve it from the theme
    if let Some(ref theme_name) = rpr.color_theme
        && let Some(theme) = theme
        && let Some(hex) = theme.colors.get(theme_name)
    {
        // `w:themeTint` and `w:themeShade` are Word's 0-255 byte convention,
        // which is exactly what `oxoffice_docx_oxml::theme::apply_tint_shade` takes.
        // Its arithmetic is Word's, deliberately not the spec-correct
        // DrawingML one, and it is called unchanged. See "Things that are
        // deliberately wrong" in docs/ARCHITECTURE.md.
        if rpr.color_theme_tint.is_some() || rpr.color_theme_shade.is_some() {
            return Color::from_hex(&oxoffice_docx_oxml::theme::apply_tint_shade(
                hex,
                rpr.color_theme_tint,
                rpr.color_theme_shade,
            ));
        }
        return Color::from_hex(hex);
    }

    // Fall back to literal color value
    rpr.color
        .as_ref()
        .filter(|c| c.as_str() != "auto")
        .map(|c| Color::from_hex(c))
        .unwrap_or(automatic)
}

/// The automatic text colour on `fill`: white on a dark fill, black on a
/// light one or on none. Word draws automatic text in whichever contrasts
/// with the shading behind it. The coefficients and the luminance floor of
/// 156 in 255 are the donor's chosen policy, not a pinned LibreOffice source.
fn automatic_text_color(fill: Option<Color>) -> Color {
    let Some(fill) = fill else {
        return Color::BLACK;
    };
    let byte = |channel: f64| (channel.clamp(0.0, 1.0) * 255.0).round();
    let luminance = (76.0 * byte(fill.r) + 151.0 * byte(fill.g) + 29.0 * byte(fill.b)) / 256.0;
    if luminance.floor() <= 156.0 {
        Color::WHITE
    } else {
        Color::BLACK
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WordLanguageSlot {
    Direct,
    EastAsia,
    Bidi,
}

fn word_language_slot(character: char) -> Option<WordLanguageSlot> {
    match character as u32 {
        0x0590..=0x08ff | 0xfb1d..=0xfdff | 0xfe70..=0xfeff => Some(WordLanguageSlot::Bidi),
        // A character East Asian to one table and absent from this one would
        // be attached to whichever slot preceded it, and at the start of a
        // run that is the direct slot, so it would silently take
        // `w:lang/@w:val` where Word takes `@w:eastAsia`.
        codepoint if is_east_asian(codepoint) => Some(WordLanguageSlot::EastAsia),
        // The Latin letters, including the IPA extensions, which are letters
        // in their own right.
        //
        // This stops short of the spacing modifier letters and modifier
        // symbols at 0x02b0..=0x02ff, which `word_font_slot` does claim for
        // `w:hAnsi`. The two tables have different residual cases, so
        // mirroring a boundary between them is the wrong operation. `None`
        // here means inherit, and `word_language_ranges` leaves an unclaimed
        // character inside the range it fell in, which is what a modifier
        // should do. Claiming them would split a Bopomofo syllable from its
        // tone mark, since `U+02C7`, `U+02CA`, `U+02CB` and `U+02D9` all live
        // there, and hand the mark `w:lang/@w:val` where Word keeps the
        // syllable's `@w:eastAsia`.
        0x0041..=0x02af | 0x0900..=0x097f | 0x0e00..=0x0e7f | 0x1e00..=0x1eff => {
            Some(WordLanguageSlot::Direct)
        }
        _ => None,
    }
}

fn word_language_for_slot(style: &WordMultilingualStyle, slot: WordLanguageSlot) -> Option<String> {
    match slot {
        WordLanguageSlot::Direct => style.language.clone(),
        WordLanguageSlot::EastAsia => style
            .language_east_asia
            .clone()
            .or_else(|| style.language.clone()),
        WordLanguageSlot::Bidi => style
            .language_bidi
            .clone()
            .or_else(|| style.language.clone()),
    }
}

/// The advance one character adds beyond its own glyph advance.
///
/// `w:spacing` is the run's own character spacing. A `linesAndChars` or
/// `snapToChars` section grid adds `w:charSpace` on top of it, and a run that
/// opts out with `w:snapToGrid w:val="0"` keeps only its own spacing.
fn word_character_advance_pt(rpr: &CT_RPr, grid_char_space_pt: f64) -> f64 {
    let spacing = rpr.spacing.map_or(0.0, |value| value.to_pt());
    if rpr.snap_to_grid == Some(false) {
        spacing
    } else {
        spacing + grid_char_space_pt
    }
}

fn word_text_direction(rtl: Option<bool>) -> TextDirection {
    match rtl {
        Some(true) => TextDirection::RightToLeft,
        Some(false) => TextDirection::LeftToRight,
        None => TextDirection::Auto,
    }
}

fn word_language_ranges(text: &str) -> Vec<(usize, usize, WordLanguageSlot)> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    let mut slot = WordLanguageSlot::Direct;
    for (offset, character) in text.char_indices() {
        let Some(next_slot) = word_language_slot(character) else {
            continue;
        };
        if next_slot != slot && offset > start {
            ranges.push((start, offset, slot));
            start = offset;
        }
        slot = next_slot;
    }
    if start < text.len() {
        ranges.push((start, text.len(), slot));
    }
    ranges
}

fn word_multilingual_segment_slice(
    segment: &TextSegment,
    byte_start: usize,
    byte_end: usize,
) -> Result<TextSegment> {
    let mut slice = segment.clone();
    slice.text = segment.text[byte_start..byte_end].to_owned();
    if let Some(source) = segment.source {
        let char_start =
            u32::try_from(segment.text[..byte_start].chars().count()).map_err(|_| {
                oxoffice_layout::LayoutError::Layout(
                    "Word multilingual source offset exceeds the supported range".to_owned(),
                )
            })?;
        let char_len = u32::try_from(slice.text.chars().count()).map_err(|_| {
            oxoffice_layout::LayoutError::Layout(
                "Word multilingual source length exceeds the supported range".to_owned(),
            )
        })?;
        slice.source = Some(SourceSpan {
            node: source.node,
            char_start: source.char_start.checked_add(char_start).ok_or_else(|| {
                oxoffice_layout::LayoutError::Layout(
                    "Word multilingual source offset overflowed".to_owned(),
                )
            })?,
            char_end: source
                .char_start
                .checked_add(char_start)
                .and_then(|start| start.checked_add(char_len))
                .ok_or_else(|| {
                    oxoffice_layout::LayoutError::Layout(
                        "Word multilingual source range overflowed".to_owned(),
                    )
                })?,
        });
    }
    slice.glyph_ids.clear();
    slice.advances.clear();
    slice.width = 0.0;
    Ok(slice)
}

/// Whether this text needs the rich shaping path rather than the legacy one.
///
/// The East Asian half is the same shared set the font and language tables
/// use, so no character is East Asian to one of the three and invisible to
/// another. The rest is every range `script_for_char` gives a non-Latin
/// script identity, which is why Hangul is here at all. Without it Korean
/// text never reached the shaper, and giving Hangul a script identity one
/// layer down would have changed nothing.
///
/// A paragraph that lands on this path leaves the paragraph block cache, since
/// `inline_bytes` cannot bound the retained size of a rich inline item and
/// scores it `usize::MAX`. That has always been true of Arabic, Hebrew and
/// CJK, and is now true of Korean.
fn needs_word_multilingual_layout(text: &str) -> bool {
    text.chars().any(|character| {
        let codepoint = character as u32;
        is_east_asian(codepoint)
            || matches!(
                codepoint,
                0x0590..=0x08ff
                    | 0x0900..=0x097f
                    | 0x0e00..=0x0e7f
                    | 0xa8e0..=0xa8ff
                    | 0xfb1d..=0xfdff
                    | 0xfe70..=0xfeff
            )
    })
}

fn inferred_word_base_direction(items: &[InlineItem]) -> TextDirection {
    for character in items
        .iter()
        .filter_map(|item| match item {
            InlineItem::Text(segment)
            | InlineItem::HyphenatedText { segment, .. }
            | InlineItem::Marker(segment) => Some(segment.text.as_str()),
            _ => None,
        })
        .flat_map(str::chars)
    {
        if character == '\u{200e}' {
            return TextDirection::LeftToRight;
        }
        if character == '\u{200f}' {
            return TextDirection::RightToLeft;
        }
        if !character.is_alphabetic() {
            continue;
        }
        return if matches!(
            character as u32,
            0x0590..=0x08ff | 0xfb1d..=0xfdff | 0xfe70..=0xfeff
        ) {
            TextDirection::RightToLeft
        } else {
            TextDirection::LeftToRight
        };
    }
    TextDirection::LeftToRight
}

fn multilingual_candidate(item: &InlineItem) -> Option<&TextSegment> {
    match item {
        InlineItem::Text(segment) | InlineItem::HyphenatedText { segment, .. }
            if !segment.text.is_empty() && segment.field_kind.is_none() =>
        {
            Some(segment)
        }
        _ => None,
    }
}

fn apply_word_multilingual_spacing(
    segment: oxoffice_layout::MultilingualTextSegment,
    spacing: f64,
    exact_word_baseline: bool,
) -> Result<oxoffice_layout::MultilingualTextSegment> {
    if spacing == 0.0 && !exact_word_baseline {
        return Ok(segment);
    }
    let mut base = segment.base().clone();
    let x_advances = segment
        .x_advances()
        .iter()
        .map(|advance| advance + spacing)
        .collect::<Vec<_>>();
    base.advances = x_advances.clone();
    base.width = x_advances.iter().sum();
    if exact_word_baseline {
        base.ascent = base.font_size * WORD_EXACT_LINE_BASELINE_EM;
    }
    segment.with_base_and_advances(base, x_advances)
}

struct WordShapedItems {
    items: Vec<InlineItem>,
    annotations: Vec<WordInputAnnotation>,
    occurrences: Vec<(usize, WordFieldOccurrence)>,
}

fn shape_word_multilingual_items(
    font_manager: &mut FontManager,
    legacy_items: Vec<InlineItem>,
    styles: &HashMap<usize, WordMultilingualStyle>,
    base_direction: TextDirection,
    no_wrap: bool,
    exact_word_baseline: bool,
    legacy_shaping: bool,
) -> Result<WordShapedItems> {
    let mut styled = Vec::new();
    let mut kern = Vec::new();
    for (index, item) in legacy_items.iter().enumerate() {
        let segment = match item {
            InlineItem::Text(segment) | InlineItem::HyphenatedText { segment, .. } => segment,
            _ => continue,
        };
        let style = styles
            .get(&index)
            .ok_or_else(|| LayoutError::Layout("Word text lacks its producer context".into()))?;
        if style.origin == WordInputOrigin::Structural
            || (!legacy_shaping
                && (style.origin == WordInputOrigin::PlainField || segment.text.is_empty()))
        {
            continue;
        }
        if legacy_shaping {
            styled.push((segment.clone(), None));
            kern.push(style.kern);
        } else {
            for (byte_start, byte_end, slot) in word_language_ranges(&segment.text) {
                let mut slice = word_multilingual_segment_slice(segment, byte_start, byte_end)?;
                slice.direction = style.direction;
                styled.push((slice, word_language_for_slot(style, slot)));
                kern.push(style.kern);
            }
        }
    }
    let mut shaped = VecDeque::from(font_manager.shape_paragraph_with_kerning(
        styled,
        base_direction,
        no_wrap,
        &kern,
        legacy_shaping,
    )?);
    let mut items = Vec::with_capacity(legacy_items.len());
    let mut annotations = Vec::new();
    let mut occurrences = Vec::new();
    let mut logical_index = 0usize;
    for (index, mut item) in legacy_items.into_iter().enumerate() {
        let style = styles.get(&index);
        let requested = style.and_then(|style| style.requested_family.as_deref());
        let segment = match &item {
            InlineItem::Text(segment) | InlineItem::HyphenatedText { segment, .. }
                if style.is_some_and(|style| {
                    matches!(
                        style.origin,
                        WordInputOrigin::Text | WordInputOrigin::PlainField
                    )
                }) =>
            {
                Some(segment)
            }
            _ => None,
        };
        let Some(segment) = segment else {
            if let InlineItem::Text(segment) | InlineItem::Marker(segment) = &item {
                font_manager.record_font_contribution(segment.font_id, requested, &segment.text)?;
            }
            if exact_word_baseline {
                match &mut item {
                    InlineItem::Text(segment) | InlineItem::Marker(segment) => {
                        segment.ascent = segment.font_size * WORD_EXACT_LINE_BASELINE_EM
                    }
                    _ => {}
                }
            }
            if let Some(annotation) = style.and_then(|style| style.annotation.as_ref()) {
                let mut annotation = annotation.clone();
                annotation.slot = items.len();
                annotations.push(annotation);
            }
            items.push(item);
            continue;
        };
        // Former plain field inputs were not part of the rich batch. Their
        // local factory preserves their own legacy Unicode shaping context.
        let mut owner_shaped = if !legacy_shaping
            && (style.is_some_and(|style| style.origin == WordInputOrigin::PlainField)
                || segment.text.is_empty())
        {
            Some(VecDeque::from(font_manager.shape_paragraph_with_kerning(
                vec![(
                    segment.clone(),
                    style.and_then(|style| style.language.clone()),
                )],
                base_direction,
                no_wrap,
                &[style.is_some_and(|style| style.kern)],
                true,
            )?))
        } else {
            None
        };
        let target_bytes = segment.text.len();
        let mut consumed_bytes = 0usize;
        loop {
            let span = owner_shaped
                .as_mut()
                .unwrap_or(&mut shaped)
                .pop_front()
                .ok_or_else(|| LayoutError::Layout("Word shaping lost a styled text run".into()))?;
            consumed_bytes = consumed_bytes
                .checked_add(span.text().len())
                .filter(|n| *n <= target_bytes)
                .ok_or_else(|| {
                    LayoutError::Layout("Word shaping crossed a styled text boundary".into())
                })?;
            let span = apply_word_multilingual_spacing(
                span,
                style.map_or(0.0, |style| style.spacing),
                exact_word_baseline,
            )?;
            let legacy = span.uses_legacy_shaping()
                || (matches!(item, InlineItem::HyphenatedText { .. })
                    && !needs_word_multilingual_layout(span.text()));
            let span = if let InlineItem::HyphenatedText { language, .. } = &item {
                span.with_hyphenation_language(Some(language.clone()))
            } else {
                span
            };
            let span = span.with_logical_index_and_legacy_shaping(logical_index, legacy);
            logical_index = logical_index
                .checked_add(1)
                .ok_or_else(|| LayoutError::Layout("Word logical span ordinal overflow".into()))?;
            font_manager.record_font_contribution(span.base().font_id, requested, span.text())?;
            if let Some(occurrence) = style.and_then(|style| style.occurrence) {
                occurrences.push((span.logical_index(), occurrence));
            }
            items.push(InlineItem::MultilingualText(span));
            if consumed_bytes == target_bytes {
                break;
            }
        }
        if owner_shaped.is_some_and(|owner| !owner.is_empty()) {
            return Err(LayoutError::Layout(
                "Word shaping left an unmatched plain owner span".into(),
            ));
        }
    }
    if !shaped.is_empty() {
        return Err(LayoutError::Layout(
            "Word shaping produced an unmatched text run".into(),
        ));
    }
    Ok(WordShapedItems {
        items,
        annotations,
        occurrences,
    })
}

/// Convert a highlight color enum to an RGBA Color.
fn highlight_to_color(h: ST_HighlightColor) -> Option<Color> {
    match h {
        ST_HighlightColor::None => None,
        ST_HighlightColor::Black => Some(Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }),
        ST_HighlightColor::Blue => Some(Color {
            r: 0.0,
            g: 0.0,
            b: 1.0,
            a: 1.0,
        }),
        ST_HighlightColor::Cyan => Some(Color {
            r: 0.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        }),
        ST_HighlightColor::DarkBlue => Some(Color {
            r: 0.0,
            g: 0.0,
            b: 0.545,
            a: 1.0,
        }),
        ST_HighlightColor::DarkCyan => Some(Color {
            r: 0.0,
            g: 0.545,
            b: 0.545,
            a: 1.0,
        }),
        ST_HighlightColor::DarkGray => Some(Color {
            r: 0.663,
            g: 0.663,
            b: 0.663,
            a: 1.0,
        }),
        ST_HighlightColor::DarkGreen => Some(Color {
            r: 0.0,
            g: 0.392,
            b: 0.0,
            a: 1.0,
        }),
        ST_HighlightColor::DarkMagenta => Some(Color {
            r: 0.545,
            g: 0.0,
            b: 0.545,
            a: 1.0,
        }),
        ST_HighlightColor::DarkRed => Some(Color {
            r: 0.545,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }),
        ST_HighlightColor::DarkYellow => Some(Color {
            r: 0.545,
            g: 0.545,
            b: 0.0,
            a: 1.0,
        }),
        ST_HighlightColor::Green => Some(Color {
            r: 0.0,
            g: 1.0,
            b: 0.0,
            a: 1.0,
        }),
        ST_HighlightColor::LightGray => Some(Color {
            r: 0.827,
            g: 0.827,
            b: 0.827,
            a: 1.0,
        }),
        ST_HighlightColor::Magenta => Some(Color {
            r: 1.0,
            g: 0.0,
            b: 1.0,
            a: 1.0,
        }),
        ST_HighlightColor::Red => Some(Color {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }),
        ST_HighlightColor::White => Some(Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        }),
        ST_HighlightColor::Yellow => Some(Color {
            r: 1.0,
            g: 1.0,
            b: 0.0,
            a: 1.0,
        }),
    }
}
