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
    BlockBookmarks, BookmarkMarker, BreakClear, BreakType, CT_P, CT_R, FieldArgument,
    FieldInstruction, RunContent, SpecialCharacter, hyperlink_revision_index,
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
    LineClear, LineItem, NoteRef, NoteStream, PageFrame, Point, PositionedElement, Rect, Result,
    ShapedText, SourceNodeId, SourceSpan, StructureId, StructureNode, StructureRole, TextDirection,
    TextSegment, Transform, Underline, break_multilingual_into_lines,
};

mod drawings;
pub(crate) use drawings::paragraph_anchors;
mod field_format;
mod fonts_annotations;
mod multilingual;
mod revisions;
mod section_geometry;
mod sequence;

use drawings::{collect_anchored_drawings, document_has_wrapping_drawing, render_word_chart};
use field_format::{
    effective_section_page_number, format_numeric_field_display, format_sequence_display,
    validate_page_field_instruction,
};
pub use field_format::{
    format_numeric_field_general, format_numeric_field_picture, format_page_field_result,
    page_reference_keeps_stored_display, section_page_number_format,
};
pub use fonts_annotations::{
    WordFontSlot, automatic_text_color, resolve_font_family, run_fill, shading_color,
};
use fonts_annotations::{
    WordLanguageSlot, annotation_field_owner, emphasis_mark_glyph, is_east_asian,
    push_annotation_decorations, push_east_asian_layout_text, push_emphasis_marked_text,
    push_empty_field_annotation, ruby_inline_item, word_font_slot_for_text, word_language_slot,
};
pub(crate) use fonts_annotations::{push_unique_diagnostic, resolve_run_color};
use multilingual::{
    highlight_to_color, inferred_word_base_direction, multilingual_candidate,
    needs_word_multilingual_layout, shape_word_multilingual_items, word_character_advance_pt,
    word_text_direction,
};
use revisions::{
    accepted_paragraph_is_blank, change_bar_color, draws_nothing, prepared_semantic_layout_input,
    project_paragraph_runs, projected_content_char_starts, projected_paragraph_text,
    push_equations_before_order, tracked_marks,
};
pub(crate) use revisions::{accepted_runs, paragraph_revision_raw_order};
pub(crate) use section_geometry::sect_pr_to_geometry;
use section_geometry::{
    parse_raw_attributes, prepare_word_line_numbers, raw_name_has_namespace, raw_root_start_tag,
    section_child_retained_bytes, section_page_geometry, section_page_number_start, xml_local_name,
};
pub use sequence::evaluate_sequence_fields;
use sequence::sequence_snapshot;
pub(crate) use sequence::validate_navigation_exclusions;

mod cache_accounting;
mod field_substitution;
mod header_footer;
mod inline_drawings;
mod input_scan;
mod note_cache;
mod paragraph;
mod sources;
mod structure;
mod transaction;

use cache_accounting::{
    canonicalize_layout_fonts, canonicalize_table_sources, font_contribution_trace_bytes,
    header_footer_cache_entry_bytes, header_footer_part_is_cache_safe,
    header_footer_section_is_cache_safe, page_has_substitution_state, paragraph_cache_entry_bytes,
    paragraph_fingerprint, paragraph_has_field, paragraph_is_cache_safe,
    paragraph_is_restart_record_source_safe, picture_draw_owner_bytes,
    rebind_header_footer_sources, rebind_paragraph_source, remap_text_box_sources,
    restart_block_is_safe, restart_cache_bytes, restart_cache_entries,
    restart_record_block_is_safe, table_cache_entry_bytes, table_fingerprint, table_is_cache_safe,
    table_semantics, table_takes_block_markers,
};
use field_substitution::{
    apply_page_background, mark_remaining_artifacts, placed_styleref_values,
    realign_tab_aligned_text, sequence_event_boundary, substitute_fields, substitute_styleref,
    supported_styleref,
};
use header_footer::layout_header_footer;
pub(crate) use input_scan::page_reference_names;
use input_scan::{
    bookmark_exists, bookmark_spelling, bookmark_text, document_has_page_ref,
    document_has_ref_projection, document_has_section_fields, document_has_sequence_fields,
    field_has_switch, field_link, field_text_argument, input_uses_page_note_restart, note_labels,
    note_reference_pages, numbered_ref_text, page_ref_id, visit_input_paragraphs,
};
pub(crate) use note_cache::paragraph_note_references;
use note_cache::{body_note_references, content_note_references, stored_note_cache_runs};
pub(crate) use paragraph::{
    layout_paragraph_with_grid, layout_paragraph_with_source_and_direction,
    layout_paragraph_with_source_in_table, source_fields,
};
use sources::TextBoxSourceBinding;
use structure::{
    assign_shared_document_structure, detect_heading_level, excluded_owner_for_source,
    is_hyperlink_character_style, is_toc_paragraph_style, record_cached_table_omissions,
    record_table_navigation,
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
    /// The face the run's East Asian characters measure on, when the run
    /// holds any: Word draws them from `w:eastAsia` whatever slot the rest
    /// of the run takes.
    east_asian_measure_font: Option<FontId>,
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
    /// The innermost inline content control around the run.
    control: Option<&'a CT_Sdt>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RawOrder {
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

fn join_accepted_layout_paragraphs(
    prefix: CT_P,
    paragraph: &CT_P,
    block_bookmarks: &BlockBookmarks,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<CT_P> {
    let mut result = paragraph.clone();
    // The copy has another address: the block markers the paragraph takes
    // go into it, at its start (after the prefix once it is inserted) and
    // its end.
    // A marker that cannot be written is left out, as before, with a
    // diagnostic: the layout goes on.
    let (before, after) = block_bookmarks.markers(paragraph);
    if !(before.is_empty() && after.is_empty()) && !result.take_block_markers(before, after) {
        diagnostics.push(Diagnostic {
            message: "a joined paragraph's block bookmarks could not be carried".to_owned(),
        });
    }
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
pub(crate) fn main_story_item_sections<'a>(
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
    /// The story of each anchored drawing of a paragraph, aligned with its
    /// `text_box_sources`: `None` for a chart or a picture.
    text_box_stories: HashMap<SourceNodeId, Vec<Option<WordStory>>>,
    preparation_error: Option<String>,
}

struct EffectiveSectionPageNumber {
    start: Option<usize>,
    format: Option<String>,
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
    even_and_odd_headers: bool,
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
    form_fields: bool,
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
            even_and_odd_headers: input.even_and_odd_headers,
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
            form_fields: input.form_fields,
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
            && self.even_and_odd_headers == input.even_and_odd_headers
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
            && self.form_fields == input.form_fields
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

struct FieldOccurrenceState {
    annotated: std::collections::HashSet<WordFieldOccurrence>,
    consumed: std::collections::HashSet<WordFieldOccurrence>,
    diagnostics: Vec<Diagnostic>,
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
    /// Each run's fill (highlight or shading), drawn behind it.
    fills: Vec<Option<Color>>,
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

struct WordShapedItems {
    items: Vec<InlineItem>,
    annotations: Vec<WordInputAnnotation>,
    occurrences: Vec<(usize, WordFieldOccurrence)>,
}
