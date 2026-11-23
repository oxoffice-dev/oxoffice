//! Pagination: distribute blocks across pages with constraints.
//!
//! Handles page breaks, widow/orphan control, keep-with-next,
//! keep-lines-together, and header/footer placement.

use crate::block::{
    AnchoredContent, AnchoredDrawing, CellBlockSemantics, FieldPaintOwner, LayoutBlock,
    LayoutBlockLike, PaintLayers, PaintPolicies, PaintedElements, ParagraphBlock, ParagraphView,
    RowSemantics, ShapePreset, SharedLayoutBlock, TableView, WordAnchorPaint, WordParagraphPaint,
    WordRowPaint,
};
use crate::table::{CellBlock, FloatingTable, TableRow};
use std::collections::HashMap;
use std::sync::Arc;

use oxoffice_layout::{
    Align, Color, FontManager, ForcedBreakKind, GlyphRun, GroupElement, LayoutLine, LineClear,
    LineItem, MediaId, MultilingualGlyphRun, NoteRef, NoteStream, OutlineEntry, PageFrame, Path,
    Point, PositionedElement, Rect, TabAlign, TabAlignedField, Transform,
};

use oxoffice_docx_oxml::borders::{CT_BorderEdge, CT_PBdr};
use oxoffice_docx_oxml::document::{
    ST_LineNumberRestart, ST_PageBorderDisplay, ST_PageBorderOffset,
};
use oxoffice_docx_oxml::drawing::{
    AnchorAlignH, AnchorAlignV, ST_RelativeFromH, ST_RelativeFromV, WrapType,
};
use oxoffice_docx_oxml::footnotes::NoteType;
use oxoffice_docx_oxml::shared::ST_Border;
use oxoffice_docx_oxml::table::ST_VerticalJc;

use crate::WordBodyLayoutFragment;
use crate::input::{ImageData, MediaRegistry};
use crate::notes::{
    NOTE_SEPARATOR_OFFSET, NoteLayout, NoteRegistry, NoteRenderParagraph, SEPARATOR_WIDTH_FRACTION,
};

mod anchors;
mod borders;
mod endnotes;
mod frames;
mod furniture;
mod geometry;
mod page_anchors;
mod page_finish;
mod page_notes;
mod pager;
mod paragraph_lines;
mod paragraphs;
mod pass;
mod reflow_sources;
mod sections;
mod table_rows;
mod wraps;

pub(crate) use anchors::inline_shape_elements;
use anchors::{
    AnchorPaintOwner, anchored_elements, anchored_rect, resolve_anchor_h, resolve_anchor_v,
    translate_element_tree,
};
use borders::{render_border_edges, render_cell_borders, render_paragraph_border_edges};
pub use endnotes::append_endnote_pages;
pub(crate) use endnotes::{
    EndnoteFurniture, append_endnote_pages_at_document_end, append_endnote_pages_for_references,
};
use endnotes::{append_ordered_endnote_pages, collect_endnote_refs};
use frames::{frame_rect, render_hf_blocks};
pub use furniture::HeaderFooterContent;
pub(crate) use furniture::{
    HeaderFooterBlockSemantics, HeaderFooterPlacement, HeaderFooterSemantics,
};
use furniture::{StoryVariant, prepare_furniture};
pub use geometry::{ColumnTrack, LineNumbering, PageBorderFrame, PageGeometry};
use page_finish::NumberedLine;
use page_notes::{draw_note, notes_in_line, page_foot_notes_in_line, table_note_references};
use paragraph_lines::{
    ReflowTextProvenanceKind, render_change_bar, render_change_bar_at, render_paragraph_lines,
};
use paragraphs::{paginate_paragraph, paragraph_line_baseline};
use pass::{paginate_pass_from, paginate_with_media, paginate_with_media_recorded};
use reflow_sources::{
    bookmark_target_name, conditional_hyphen_visual_items, logical_reflow_elements,
    word_field_owner,
};
pub(crate) use sections::{
    FurnitureFailure, PageFlowSpace, PagePaintBoundary, PagePaintRecords, PaginationCheckpoint,
    SectionParity, SharedSection, WordLineNumberStyle, paginate_shared_sections,
    paginate_shared_single_section_recorded,
};
pub use sections::{Section, paginate, paginate_sections};
use sections::{absorb_into_section_break, takes_no_room};
pub(crate) use table_rows::story_table_rows;
use table_rows::{
    paint_flowed_table_row, place_cell_anchored, render_table_row, repeat_table_headers,
    split_simple_table_row, wrap_paint_suffix,
};
use wraps::{
    ResolvedWraps, WrapReflowPolicy, floating_wrap, has_paragraph_relative_wrap,
    is_paragraph_relative_float, is_paragraph_relative_wrap, reflow_around_wraps, settle_float,
};

/// A wrapping drawing that has been placed on the page being built.
#[derive(Debug, Clone, Copy)]
struct PlacedWrap {
    rect: Rect,
    wrap: WrapType,
    dist_top: f64,
    dist_bottom: f64,
    dist_left: f64,
    dist_right: f64,
}

impl PlacedWrap {
    /// Top of the band this drawing keeps text out of.
    fn keep_out_top(&self) -> f64 {
        self.rect.y - self.dist_top
    }

    /// Bottom of the band this drawing keeps text out of.
    fn keep_out_bottom(&self) -> f64 {
        self.rect.y + self.rect.height + self.dist_bottom
    }
}

/// One selected story at one demonstrated origin. The same prepared blocks and
/// placements price the body band and paint the page. Cached source blocks stay
/// immutable throughout candidate preparation.
#[derive(Debug, Clone)]
struct PreparedFurniture {
    paints: Vec<Option<Arc<WordParagraphPaint>>>,
    blocks: Vec<ParagraphBlock>,
    before_lines: Vec<Vec<f64>>,
    origin: f64,
    height: f64,
    starts: Vec<f64>,
    placements: Vec<Option<Rect>>,
    wraps: Vec<PlacedWrap>,
    // Paragraph frames wrap main body, not ordinary flow in their own story.
    body_frame_wraps: Vec<PlacedWrap>,
}

/// One pagination pass, and what it learned about paragraph-relative wraps.
struct PassResult {
    paint: PagePaintRecords,
    furniture_failure: Option<FurnitureFailure>,
    pages: Vec<PageFrame>,
    outlines: Vec<OutlineEntry>,
    body_fragments: Vec<(usize, WordBodyLayoutFragment)>,
    resolved: ResolvedWraps,
    checkpoints: Vec<PaginationCheckpoint>,
    stopped_at: Option<PaginationCheckpoint>,
    last_flow_space: Option<PageFlowSpace>,
}

/// Everything a pass needs that is the same for both passes.
///
/// Built once and borrowed twice, so the eight-value argument list appears in
/// one place rather than at each call.
struct PassContext<'a> {
    line_number_style: Option<WordLineNumberStyle>,
    written_tb_rl_band: bool,
    geometry: PageGeometry,
    header_footer: Option<&'a HeaderFooterContent>,
    header_footer_semantics: Option<&'a HeaderFooterSemantics>,
    title_pg: bool,
    footnotes_beneath_text: bool,
    fm: &'a FontManager,
    media: &'a HashMap<MediaId, ImageData>,
    notes: &'a NoteRegistry,
    first_page_number: usize,
    first_header_page_number: usize,
    continuous_sections: Option<&'a [SharedSection]>,
}

/// Helper struct to track page state during pagination.
struct Pager<'a> {
    next_repetition_scope: u64,
    element_policies: PaintPolicies,
    behind_policies: PaintPolicies,
    front_elements: Vec<PositionedElement>,
    front_policies: PaintPolicies,
    page_policies: Vec<PaintPolicies>,
    page_boundaries: Vec<PagePaintBoundary>,
    line_number_style: Option<WordLineNumberStyle>,
    written_tb_rl_band: bool,
    pages: Vec<PageFrame>,
    elements: Vec<PositionedElement>,
    /// Anchored drawings marked behindDoc. Held apart from the normal element
    /// list so they can be emitted before everything else on the page, which
    /// is what puts them underneath the text.
    behind_elements: Vec<PositionedElement>,
    cursor_y: f64,
    page_number: usize,
    header_page_number: usize,
    content_height: f64,
    /// Geometry of the body band, narrowed to the active column track.
    geometry: PageGeometry,
    /// The section as authored, before a page mirrors it or a track narrows it.
    section_geometry: PageGeometry,
    /// The page being built, mirrored for its displayed number, at full text
    /// measure. Its top and bottom margins are moved clear of the header and
    /// footer the page draws, so the body band and whatever is placed
    /// against the text margins start from these.
    page_geometry: PageGeometry,
    /// The same page with the section's own margins. Headers, footers and
    /// page borders are drawn against this.
    furniture_geometry: PageGeometry,
    prepared_header: Option<PreparedFurniture>,
    prepared_footer: Option<PreparedFurniture>,
    pending_furniture_failure: Option<String>,
    furniture_failure: Option<FurnitureFailure>,
    /// Column tracks of the page being built, left to right. Empty for a
    /// single-column section, which never leaves the untouched path.
    tracks: Vec<ColumnTrack>,
    /// Which track the body is filling.
    track_index: usize,
    /// Body lines placed on this page, in placement order, for line numbering.
    numbered_lines: Vec<NumberedLine>,
    /// The number the next body line of this section receives.
    next_line_number: u64,
    continuous_sections: Option<&'a [SharedSection]>,
    active_member: usize,
    pending_continuous_footnote_break: bool,
    page_member: Option<usize>,
    member_first_pages: HashMap<usize, usize>,
    header_footer: Option<&'a HeaderFooterContent>,
    header_footer_semantics: Option<&'a HeaderFooterSemantics>,
    has_content_flag: bool,
    outlines: Vec<OutlineEntry>,
    /// The heading whose outline entry waits for the page and height its
    /// first line lands on, which keep with next or widow control may move.
    pending_outline: Option<(String, u32)>,
    /// Whether the current page is the first page of the section.
    is_first_page: bool,
    /// Whether this section uses different first page header/footer.
    title_pg: bool,
    footnotes_beneath_text: bool,
    media: &'a HashMap<MediaId, ImageData>,
    /// Every note the document defines, laid out once before pagination.
    notes: &'a NoteRegistry,
    /// Notes first referenced by a line placed on the page being built, in
    /// reference order. Line counts are decided when the page is finished,
    /// since that is when the leftover height is known.
    page_note_ids: Vec<NoteRef>,
    /// Note content that did not fit on the previous page, as (id, next line).
    /// Placed before this page's own notes, and drawn without a marker.
    pending_notes: Vec<(NoteRef, usize)>,
    /// Re-breaking a paragraph around a drawing needs the shaper.
    fm: &'a FontManager,
    /// Rectangles of the wrapping drawings already placed on this page, with
    /// the wrap mode and text distances each one asks for.
    page_wraps: Vec<PlacedWrap>,
    /// Floating tables already placed on this page, in body order, each with
    /// whether `w:tblOverlap` lets another float overlap it.
    ///
    /// Separate from `page_wraps` because `w:tblOverlap` is a rule between
    /// floating tables. A drawing is not a float in that sense, and folding the
    /// flag into `PlacedWrap` would make every drawing a party to the rule.
    page_floats: Vec<(PlacedWrap, bool)>,
    /// Where the body's last mark sits, ignoring trailing paragraph spacing.
    ///
    /// `cursor_y` includes the space after the final paragraph, and that space
    /// collapses at a page break. Measuring the note area from `cursor_y`
    /// would let it eat into the height that was reserved, which is enough to
    /// push a note off the page its own reference sits on.
    ink_bottom: f64,
    page_flow_spaces: Vec<PageFlowSpace>,
    /// Space after of the paragraph that ends the flow above the cursor, which
    /// `cursor_y` already includes. Zero after a table row, because Word
    /// collapses spacing only between two paragraphs of the same flow.
    previous_space_after: f64,
    /// Where the previous pass placed each paragraph-relative wrapping drawing.
    /// Empty on the first pass, which is what makes that pass identical to a
    /// single-pass run.
    resolved_in: &'a ResolvedWraps,
    /// Where this pass is placing them, for the pass that follows.
    resolved_out: ResolvedWraps,
    body_fragments: Vec<(usize, WordBodyLayoutFragment)>,
    checkpoints: Vec<PaginationCheckpoint>,
    stop_at: Option<PaginationCheckpoint>,
    stopped_at: Option<PaginationCheckpoint>,
    /// The next block may share the page transition made by a run-level page
    /// break at the end of the preceding paragraph.
    trailing_run_page_break_before: Option<usize>,
}

struct WrapReflow {
    paint: Option<WordParagraphPaint>,
    paragraph: ParagraphBlock,
    before_lines: Vec<f64>,
}

#[derive(Clone, Copy)]
struct ReflowTextProvenance {
    element_position: usize,
    visual_item: usize,
    kind: ReflowTextProvenanceKind,
}

#[derive(Clone, Copy)]
struct ReflowTabProvenance {
    visual_item: usize,
}
