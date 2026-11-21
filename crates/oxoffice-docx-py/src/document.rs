use std::collections::{BTreeMap, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;

use oxoffice_py_support::{
    ContentPath, PathSeg, PreparedRevision, RevisionCounter, RevisionPreparation, RevisionScope,
    RevisionState, StaleElementError, revision_to_pyerr,
};
use pyo3::exceptions::{PyIndexError, PyKeyError, PyOverflowError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{
    PyAny, PyBool, PyBytes, PyDict, PyFloat, PyInt, PyList, PyMapping, PyString, PyTuple,
};
use smallvec::smallvec;

use oxoffice_docx_oxml::{ST_PageOrientation, ST_SectionType};

use crate::docx_to_pyerr;
use crate::paragraph::{
    ParagraphLocation, PyParagraph, PyParagraphCollection, defined_style, style_id_of_type,
};
use crate::table::{PyTable, PyTableCollection};

mod annotations;
mod charts;
mod content_controls;
mod conversions;
mod fonts;
mod forms;
mod fragments;
mod headers_footers;
mod lists;
mod mail_merge;
mod notes;
mod pictures;
mod positions;
mod properties;
mod results;
mod sections;
mod settings;
mod stories;
mod styles;
mod templates;

pub use annotations::{PyComment, PyRevision};
use annotations::{revision_kind_name, story_link_layout};
use charts::checked_chart_inputs;
pub use content_controls::PyContentControl;
use content_controls::{
    content_control_appearance_from_name, content_control_kind_from_name,
    content_control_lock_from_name, content_control_placement_from_name, optional_python_bool,
};
pub use conversions::{
    PyEpubDiagnostic, PyHtmlDiagnostic, PyOdtDiagnostic, PyRtfDiagnostic, PyTextDiagnostic,
    PyTextReport, PyValidationFinding, PyValidationReport, validate_bytes, validate_file,
};
use conversions::{html_diagnostics, read_import_text};
pub use fonts::{PyEmbeddedFont, PyFontDefinition, PyFontEmbeddingLicense};
use fonts::{checked_font_input, checked_font_kind, checked_font_license, collect_caller_fonts};
pub use forms::{
    PyFormFieldAssignment, PyFormFieldRule, PyFormFieldTarget, PyFormFieldValue, PyFormResetValue,
    PyFormValidationIssue, PyLegacyFormField, PyLegacyFormFieldDefinition,
};
use forms::{copied_assignments, copied_rules, issue_to_py, require_carrier_sequence};
pub use lists::PyListLevel;
use mail_merge::copied_external_rows;
pub(crate) use mail_merge::{
    PyMailMergeFieldMapping, PyMailMergePackageDiagnostic, PyMailMergePackageSource,
    PyMailMergeRecipient, PyMailMergeSettings,
};
pub(crate) use pictures::picture_input;
pub use pictures::{PyPicture, PyPictureOptions};
pub use positions::{PyRunPosition, PyRunRange};
pub use properties::{PyCustomFileTime, PyCustomProperty, PyDefaultParagraphFormat};
use properties::{default_paragraph_format_snapshot, length_keyword, line_spacing_keyword};
pub use results::{PyBoundingBox, PyComparisonDiagnostic, PyLayoutPage, PySvgDiagnostic};
pub use sections::PySection;
use sections::checked_note_policy;
pub use styles::PyStyle;
use styles::{StyleLineSpacingArg, style_id_from_name};
use templates::{template_render_pyerr, template_value};

mod block_edits;
mod body_splice;
mod inventory_matches;
mod owner_facts;
mod owner_locations;
mod owner_pieces;
mod publication;

pub(crate) use block_edits::BlockShift;
#[cfg(debug_assertions)]
use block_edits::assert_kept_owners_name_their_elements;
use block_edits::block_retention;
pub(crate) use body_splice::{BodySplice, PreparedBodySplice, SplicedBodyItem, StagedAppend};
use inventory_matches::{
    append_word_owners_with_states, indexed_inventory_matches, property_owner_key,
    relocated_property_owners, word_owners_with_states,
};
use owner_facts::{
    FormOwnerRoute, FormSourceKey, FormStoryKey, WordOwnerSnapshots, WordParagraphFacts,
    form_owner_route, form_source_key, word_owner_facts, word_piece_owner_addresses,
    word_table_facts, word_table_read_facts,
};
use owner_locations::{
    IndexMap, WordReadFacts, same_content_owner, same_story_owner, word_owner_capture,
};
pub(crate) use owner_locations::{WordOwnerLocation, WordView};
use owner_pieces::{
    CellParagraphRoutes, WordPartPiece, aligned_block, placed_route, placed_story,
    plain_paragraph_owners, table_owners, validate_word_facts, word_block_owners,
    word_owner_addresses, word_owner_locations,
};
pub(crate) use publication::WordPublication;
use publication::{
    form_owner_certificates, modern_form_owner_certificates, signing_owner_certificates,
    unchanged_related_owner_parts,
};

mod arguments;
mod construction;
mod inventory_reads;
mod methods;
mod owner_edits;
mod replacements;
mod snapshots;

use arguments::{
    binary_stream_bytes, body_index_argument, check_fillable_forms, checked_heading_level,
    comment_date, given_or_current_length, open_document_input, parse_comparison_granularity,
    parse_comparison_story, parse_raster_format, parse_render_options,
};
use snapshots::{
    header_footer_kind_from_name, header_footer_variant_from_name, section_snapshot,
    story_item_kind_from_name, story_item_kind_name, story_kind_name, story_snapshot,
    style_snapshot,
};

#[pyclass(name = "Bookmark", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyBookmark {
    id: Option<i32>,
    name: Option<String>,
    range: Option<PyRunRange>,
    direct_range: Option<PyRunRange>,
    text: String,
    issue: Option<String>,
}

#[pyclass(name = "SvgRenderResult", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PySvgRenderResult {
    svg: String,
    diagnostics: Vec<PySvgDiagnostic>,
}

#[pyclass(name = "LayoutFragment", frozen, eq, skip_from_py_object)]
#[derive(Clone, Copy, PartialEq)]
pub struct PyLayoutFragment {
    body_index: usize,
    physical_page: usize,
    displayed_page: usize,
    bounds: PyBoundingBox,
}

#[pyclass(name = "TocRebuildReport", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyTocRebuildReport {
    entry_count: usize,
    bookmark_count: usize,
    diagnostics: Vec<String>,
}

#[pyclass(
    name = "LayoutBackedFieldUpdateReport",
    frozen,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyLayoutBackedFieldUpdateReport {
    page_fields: usize,
    num_pages_fields: usize,
    page_reference_fields: usize,
    section_fields: usize,
    section_pages_fields: usize,
    diagnostics: Vec<String>,
}

#[pyclass(name = "FieldEvaluation", frozen, get_all, skip_from_py_object)]
pub struct PyFieldEvaluation {
    field_index: usize,
    instruction: String,
    cached_result: String,
    outcome: Py<PyDict>,
}

#[pyclass(name = "FieldsUpdateReport", frozen, get_all, skip_from_py_object)]
pub struct PyFieldsUpdateReport {
    ordinary_updated: usize,
    layout: Py<PyLayoutBackedFieldUpdateReport>,
    evaluations: Py<PyTuple>,
    diagnostics: Py<PyTuple>,
    total_updated: usize,
    saved_cache_scope: String,
}

#[pyclass(name = "Story", frozen, eq, skip_from_py_object)]
#[derive(Clone)]
pub struct PyStory {
    pub kind: String,
    pub part_name: String,
    pub owner_index: usize,
    binding: Option<oxoffice_py_support::ContentPath>,
}

#[pyclass(name = "StoryItem", frozen, eq, skip_from_py_object)]
#[derive(Clone)]
pub struct PyStoryItem {
    story: PyStory,
    kind: String,
    index_path: Vec<usize>,
    direct_body_index: Option<usize>,
    text: Option<String>,
    xml: Vec<u8>,
    revision: u64,
    binding: Option<oxoffice_py_support::ContentPath>,
    /// The document that listed this item. A caller-built item has none.
    document: Option<std::sync::Arc<Py<PyDocument>>>,
}

/// One paragraph of a story, as `Document.story_paragraphs` reads it.
#[pyclass(name = "StoryParagraph", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyStoryParagraph {
    kind: String,
    story: PyStory,
    index_path: Vec<usize>,
    style: Option<String>,
    text: String,
}

#[pyclass(name = "StoryRunPosition", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyStoryRunPosition {
    item: PyStoryItem,
    run_index: usize,
}

#[pyclass(name = "StoryRunRange", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyStoryRunRange {
    start: PyStoryRunPosition,
    end: PyStoryRunPosition,
}

#[pyclass(name = "ContentFragment", frozen, skip_from_py_object)]
pub struct PyContentFragment {
    inner: oxoffice_docx::ContentFragment,
}

#[pyclass(name = "Hyperlink", frozen, eq, skip_from_py_object)]
#[derive(Clone)]
pub struct PyHyperlink {
    story: PyStory,
    index_path: Vec<usize>,
    text: String,
    url: Option<String>,
    anchor: Option<String>,
    relationship_id: Option<String>,
    /// The position among the native `story_links` of `story`, which
    /// selects one of several identical links, and the story's link layout
    /// when the snapshot was taken. A constructed record has neither.
    position: Option<(usize, u64)>,
    owner: Option<(ContentPath, RevisionState)>,
}

#[pyclass(name = "HeaderFooterVariant", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyHeaderFooterVariant {
    section_index: usize,
    kind: String,
    variant: String,
    story: Option<PyStory>,
    source_section: Option<usize>,
    inherited: bool,
}

#[pyclass(name = "SectionHandle")]
pub struct PySectionHandle {
    document: Py<PyDocument>,
    index: usize,
    path: oxoffice_py_support::ContentPath,
    owner: RevisionState,
}

fn related_table_fragment(rows: usize, cols: usize) -> PyResult<oxoffice_docx::ContentFragment> {
    use oxoffice_docx_oxml::table::{
        CT_Row, CT_Tbl, CT_TblGrid, CT_TblGridCol, CT_TblPr, CT_TblWidth, CT_Tc,
    };
    if rows == 0 || cols == 0 {
        return Err(PyValueError::new_err("table dimensions must be positive"));
    }
    let columns = i32::try_from(cols)
        .map_err(|_| PyOverflowError::new_err("table column count is too large"))?;
    let width = 9360 / columns;
    if width == 0 {
        return Err(PyValueError::new_err(
            "table columns must be at least one twip wide",
        ));
    }
    let mut table = CT_Tbl::new();
    table.properties = Some(CT_TblPr {
        width: Some(CT_TblWidth::dxa(width * columns)),
        ..Default::default()
    });
    table.grid = Some(CT_TblGrid {
        columns: (0..cols)
            .map(|_| CT_TblGridCol {
                width: oxoffice_docx_oxml::units::Twips(width),
            })
            .collect(),
        ..Default::default()
    });
    for _ in 0..rows {
        let mut row = CT_Row::new();
        row.cells.extend((0..cols).map(|_| CT_Tc::new()));
        table.rows.push(row);
    }
    oxoffice_docx::ContentFragment::table(table)
        .map_err(|error| PyValueError::new_err(error.to_string()))
}

#[pyclass(name = "HeaderFooter")]
pub struct PyHeaderFooter {
    document: Py<PyDocument>,
    index: usize,
    kind: oxoffice_docx::HeaderFooterKind,
    variant: oxoffice_docx::HdrFtrType,
    path: oxoffice_py_support::ContentPath,
}

#[pyclass(name = "FootnoteEndnote")]
pub struct PyNote {
    document: Py<PyDocument>,
    id: i32,
    endnote: bool,
    path: oxoffice_py_support::ContentPath,
}

#[pyclass(name = "Settings")]
pub struct PySettings {
    document: Py<PyDocument>,
}

/// Owned, read-only snapshot of one qualified direct-body chart occurrence.
#[pyclass(name = "Chart", frozen)]
pub struct PyChart {
    chart_type: Option<i64>,
    data: oxoffice_docx::ChartData,
}

/// The formatting `Document.add_style` gives a new style. Lengths and the
/// font size are EMU, as `Length` values are, and `None` leaves a property to
/// the style's base.
struct StyleFormatting {
    /// `Document.add_style` or `Document.set_style`, for messages.
    method: &'static str,
    font_name: Option<String>,
    font_size: Option<i64>,
    bold: Option<bool>,
    italic: Option<bool>,
    color: Option<(u8, u8, u8)>,
    space_before: Option<i64>,
    space_after: Option<i64>,
    left_indent: Option<i64>,
    right_indent: Option<i64>,
    first_line_indent: Option<i64>,
    alignment: Option<i32>,
    /// None omits the write. `Some(None)` clears the line and rule.
    /// `Some(Some(value))` assigns a length or a line multiple.
    line_spacing: Option<Option<Py<PyAny>>>,
    underline: Option<Py<PyAny>>,
    keep_with_next: Option<bool>,
    keep_together: Option<bool>,
    page_break_before: Option<bool>,
    borders: Option<Py<PyAny>>,
    shading: Option<Py<PyAny>>,
    tab_stops: Option<Py<PyAny>>,
}

#[pyclass(name = "CustomProperties")]
pub struct PyCustomProperties {
    document: Py<PyDocument>,
}
#[pyclass(name = "AppProperties")]
pub struct PyAppProperties {
    document: Py<PyDocument>,
}

/// The package core properties (`docProps/core.xml`) under python-docx's
/// attribute names.
///
/// Text properties read as an empty string when absent, `revision` as zero,
/// and dates as `None`. Assigning `None` or empty text removes a property.
/// A write replaces the native model and creates the part, its package
/// relationship and its content type when the document has none. It changes
/// no content, so handles stay valid.
#[pyclass(name = "CoreProperties")]
pub struct PyCoreProperties {
    document: Py<PyDocument>,
}

#[cfg_attr(debug_assertions, derive(Debug, PartialEq, Eq))]
struct WordOwnerAddress {
    location: WordOwnerLocation,
    parent: Option<usize>,
    body_slot: Option<usize>,
    projection_ordinal: Option<usize>,
    read_facts: WordReadFacts,
}

// Debug checks keep the inventory a local edit replaces, to compare.
#[cfg_attr(debug_assertions, derive(Clone))]
pub(crate) struct WordOwner {
    location: WordOwnerLocation,
    parent: Option<usize>,
    body_slot: Option<usize>,
    projection_ordinal: Option<usize>,
    read_facts: WordReadFacts,
    owner: RevisionState,
    content: Option<RevisionState>,
    properties: Option<RevisionState>,
    grid: Option<RevisionState>,
}

struct WordOwnerIndexes {
    by_generation: IndexMap<u64, usize>,
    by_route: IndexMap<u64, Vec<usize>>,
    direct_body_paragraphs: IndexMap<usize, usize>,
    body_paragraphs: Option<Vec<Option<oxoffice_docx::ContentLocation>>>,
    body_tables: Vec<Option<oxoffice_docx::ContentLocation>>,
    cell_paragraphs: HashMap<(usize, usize, usize), Vec<oxoffice_docx::ContentLocation>>,
    paragraphs_by_parent: IndexMap<usize, Vec<usize>>,
    tables_by_parent: IndexMap<usize, Vec<usize>>,
    cells_by_parent: IndexMap<usize, Vec<usize>>,
    runs_by_parent: IndexMap<usize, Vec<usize>>,
}

pub(crate) struct PreparedPlainParagraphXml {
    revision: PreparedRevision,
    states: Vec<RevisionState>,
    runs: Vec<usize>,
    newborn: Vec<WordOwner>,
    route_buckets: HashMap<u64, Vec<usize>>,
}

pub(crate) struct PreparedWordRevision {
    pub(crate) revision: PreparedRevision,
    owners: Vec<WordOwner>,
    indexes: WordOwnerIndexes,
    snapshot_scope: RevisionState,
    /// Pieces for the candidate, or `None` when this publication did not
    /// assemble them and the cache must reset.
    pieces: Option<WordPieces>,
}

/// Under debug assertions, check a direct read against the whole-source
/// read: the same bytes up to the root `w14` declaration, which a direct
/// read decides from the root and the read item alone.
#[cfg(debug_assertions)]
pub(crate) fn assert_direct_read(
    direct: &[u8],
    whole: impl FnOnce() -> PyResult<Vec<u8>>,
    owner: &str,
) {
    let without_w14 = |xml: &[u8]| {
        let xml = String::from_utf8_lossy(xml).into_owned();
        let root = xml.find('>').unwrap_or(xml.len());
        let declaration = r#" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml""#;
        format!(
            "{}{}",
            xml[..root].replacen(declaration, "", 1),
            &xml[root..]
        )
    };
    let whole = whole().expect("a direct read has a whole-source read");
    assert_eq!(
        without_w14(direct),
        without_w14(&whole),
        "direct {owner} XML"
    );
}

/// Under debug assertions, check a handle's route bound from an owner
/// inventory against the route the document resolves, story fingerprints
/// aside.
#[cfg(debug_assertions)]
pub(crate) fn assert_indexed_route(
    indexed: &oxoffice_docx::ContentLocation,
    native: impl FnOnce() -> oxoffice_docx::Result<oxoffice_docx::ContentLocation>,
    owner: &str,
) {
    let native = native().expect("an indexed route resolves in the document");
    assert_eq!(
        unfingerprinted_location(indexed),
        unfingerprinted_location(&native),
        "indexed {owner} route"
    );
}

#[cfg(debug_assertions)]
fn unfingerprinted_location(
    location: &oxoffice_docx::ContentLocation,
) -> oxoffice_docx::ContentLocation {
    let story = location.story();
    oxoffice_docx::ContentLocation::new(
        oxoffice_docx::StoryId::new(story.kind(), story.part_name(), story.owner_index()),
        location.item_kind(),
        location.index_path().to_vec(),
    )
}

#[cfg(debug_assertions)]
fn unfingerprinted_owner_location(location: &WordOwnerLocation) -> WordOwnerLocation {
    match location {
        WordOwnerLocation::Paragraph(location) => {
            WordOwnerLocation::Paragraph(unfingerprinted_location(location))
        }
        WordOwnerLocation::Table(location) => {
            WordOwnerLocation::Table(unfingerprinted_location(location))
        }
        WordOwnerLocation::Run(location, path) => {
            WordOwnerLocation::Run(unfingerprinted_location(location), path.clone())
        }
        WordOwnerLocation::Hyperlink(location, index) => {
            WordOwnerLocation::Hyperlink(unfingerprinted_location(location), *index)
        }
        WordOwnerLocation::Cell {
            table,
            row,
            cell,
            story,
        } => WordOwnerLocation::Cell {
            table: unfingerprinted_location(table),
            row: *row,
            cell: *cell,
            story: oxoffice_docx::StoryId::new(
                story.kind(),
                story.part_name(),
                story.owner_index(),
            ),
        },
        WordOwnerLocation::Section(index) => WordOwnerLocation::Section(*index),
    }
}

#[derive(Default)]
struct FormSources {
    xml: HashMap<FormOwnerRoute, Option<Vec<u8>>>,
    cells: HashMap<FormOwnerRoute, usize>,
    cell_stories: HashMap<FormStoryKey, usize>,
}

/// Owner facts in snapshot order: tables, paragraphs and content controls,
/// physical cells, range paragraphs, and hyperlink items.
#[derive(Clone, Default)]
struct WordOwnerFacts {
    items: Vec<(oxoffice_docx::ContentLocation, Option<usize>, WordReadFacts)>,
    cells: Vec<(
        oxoffice_docx::ContentLocation,
        usize,
        usize,
        oxoffice_docx::StoryId,
    )>,
    paragraphs: Vec<WordParagraphFacts>,
    links: Vec<oxoffice_docx::ContentLocation>,
}

struct WordIndexProjections {
    body_paragraphs: Option<Vec<Option<oxoffice_docx::ContentLocation>>>,
    cell_paragraphs: CellParagraphRoutes,
    table_slots: Vec<Option<usize>>,
}

struct WordBlockOwners {
    facts: WordOwnerFacts,
    relationships: Vec<String>,
    body_items: usize,
    cell_stories: usize,
    text_boxes: usize,
    paragraphs: Vec<Option<Vec<usize>>>,
    tables: usize,
    /// Complete native physical-to-indexed membership and standalone story base.
    local_table_membership: bool,
    cell_paragraphs: Option<(CellParagraphRoutes, bool)>,
    holds_field: bool,
}

struct WordBlockPiece {
    block: std::sync::Arc<oxoffice_docx_oxml::BodyContent>,
    owners: Option<WordBlockOwners>,
}

/// Cached body blocks and related story parts.
#[derive(Clone, Default)]
pub(crate) struct WordPieces {
    context: Option<oxoffice_docx::BodyItemContext>,
    blocks: Vec<std::sync::Arc<WordBlockPiece>>,
    parts: Vec<std::sync::Arc<WordPartPiece>>,
}

#[cfg(debug_assertions)]
fn owner_location_without_body(location: &WordOwnerLocation) -> WordOwnerLocation {
    let content = |location: &oxoffice_docx::ContentLocation| {
        if location.story().kind() == oxoffice_docx::StoryKind::Body {
            unfingerprinted_location(location)
        } else {
            location.clone()
        }
    };
    match location {
        WordOwnerLocation::Paragraph(location) => WordOwnerLocation::Paragraph(content(location)),
        WordOwnerLocation::Table(location) => WordOwnerLocation::Table(content(location)),
        WordOwnerLocation::Run(location, path) => {
            WordOwnerLocation::Run(content(location), path.clone())
        }
        WordOwnerLocation::Hyperlink(location, index) => {
            WordOwnerLocation::Hyperlink(content(location), *index)
        }
        WordOwnerLocation::Cell {
            table,
            row,
            cell,
            story,
        } => WordOwnerLocation::Cell {
            table: content(table),
            row: *row,
            cell: *cell,
            story: story.clone(),
        },
        WordOwnerLocation::Section(index) => WordOwnerLocation::Section(*index),
    }
}

#[cfg(debug_assertions)]
fn body_unfingerprinted_address(address: &WordOwnerAddress) -> WordOwnerAddress {
    WordOwnerAddress {
        location: owner_location_without_body(&address.location),
        parent: address.parent,
        body_slot: address.body_slot,
        projection_ordinal: address.projection_ordinal,
        read_facts: address.read_facts.clone(),
    }
}

/// Debug proof that a piece inventory matches the full inventory.
/// Body story fingerprints are set aside. Other fields and projections compare
/// as the full path publishes them.
#[cfg(debug_assertions)]
fn assert_word_pieces_rebuild(
    document: &oxoffice_docx::Document,
    locations: &[WordOwnerAddress],
    projections: &WordIndexProjections,
) {
    let expected =
        word_owner_locations(document, None).expect("an inventory from pieces has a rebuild");
    assert_eq!(
        locations
            .iter()
            .map(body_unfingerprinted_address)
            .collect::<Vec<_>>(),
        expected
            .iter()
            .map(body_unfingerprinted_address)
            .collect::<Vec<_>>(),
        "Word owner inventory from pieces"
    );
    let expected = WordIndexProjections::of(document);
    let paragraphs = |projections: &WordIndexProjections| {
        projections.body_paragraphs.as_ref().map(|locations| {
            locations
                .iter()
                .map(|location| location.as_ref().map(unfingerprinted_location))
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(
        paragraphs(projections),
        paragraphs(&expected),
        "body paragraph projection from pieces"
    );
    assert_eq!(
        &projections.cell_paragraphs, &expected.cell_paragraphs,
        "cell paragraph projection from pieces"
    );
    assert_eq!(
        &projections.table_slots, &expected.table_slots,
        "body table projection from pieces"
    );
}

#[pyclass(name = "Document")]
pub struct PyDocument {
    pub(crate) inner: oxoffice_docx::Document,
    pub(crate) revisions: RevisionCounter,
    owners: Vec<WordOwner>,
    indexes: WordOwnerIndexes,
    removal_scope: Option<RevisionState>,
    snapshot_scope: Option<RevisionState>,
    pieces: Option<WordPieces>,
    /// Installed by the next `install_inner` after a successful publication.
    pending_pieces: Option<WordPieces>,
}

#[pyclass(name = "FragmentConflictPolicy", frozen, skip_from_py_object)]
#[derive(Clone, Copy)]
pub struct PyFragmentConflictPolicy {
    inner: oxoffice_docx::FragmentConflictPolicy,
}

#[pyclass(name = "ContentLocation", frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyContentLocation {
    story: PyStory,
    kind: String,
    index_path: Vec<usize>,
    is_end: bool,
}

#[pyclass(name = "DocumentFragment", frozen, skip_from_py_object)]
pub struct PyDocumentFragment {
    inner: oxoffice_docx::DocumentFragment,
}

/// Caller-supplied `w:documentProtection` metadata. No password is derived.
#[pyclass(name = "DocumentProtection", frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyDocumentProtection {
    inner: oxoffice_docx::DocumentProtection,
}

#[pyclass(frozen, name = "MailMergePackageOptions")]
pub(crate) struct PyMailMergePackageOptions {
    inner: oxoffice_docx::MailMergePackageOptions,
}

#[pyclass(frozen, name = "MailMergePreparedData")]
pub(crate) struct PyMailMergePreparedData {
    inner: oxoffice_docx::MailMergePreparedData,
}

/// Type payload for `Document.set_content_control_type`.
#[pyclass(name = "ContentControlTypePayload", frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyContentControlTypePayload {
    inner: oxoffice_docx::ContentControlTypePayload,
}

#[pyclass(name = "OdtExportResult", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyOdtExportResult {
    bytes: Vec<u8>,
    diagnostics: Vec<PyOdtDiagnostic>,
}

#[pyclass(name = "RtfExportResult", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyRtfExportResult {
    bytes: Vec<u8>,
    diagnostics: Vec<PyRtfDiagnostic>,
}

#[pyclass(name = "EpubExportResult", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyEpubExportResult {
    bytes: Vec<u8>,
    diagnostics: Vec<PyEpubDiagnostic>,
}
