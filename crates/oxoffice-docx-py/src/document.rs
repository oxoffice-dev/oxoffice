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

fn binary_stream_bytes(value: &Bound<'_, PyAny>, seek: bool) -> PyResult<Vec<u8>> {
    if seek {
        value.call_method1("seek", (0,)).map_err(|error| {
            if error.is_instance_of::<pyo3::exceptions::PyAttributeError>(value.py()) {
                PyTypeError::new_err(
                    "Document requires a seekable binary stream; use Document.from_bytes(stream.read())",
                )
            } else {
                error
            }
        })?;
    }
    let bytes = value.call_method0("read").map_err(|error| {
        if error.is_instance_of::<pyo3::exceptions::PyAttributeError>(value.py()) {
            PyTypeError::new_err(
                "use a binary stream, filesystem path or Document.from_bytes(bytes)",
            )
        } else {
            error
        }
    })?;
    bytes
        .cast::<PyBytes>()
        .map(|bytes| bytes.as_bytes().to_vec())
        .map_err(|_| {
            PyTypeError::new_err(
                "stream.read() must return binary bytes; use Document.from_bytes(bytes)",
            )
        })
}

fn open_document_input(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
) -> PyResult<oxoffice_docx::Document> {
    if let Ok(path) = value.extract::<PathBuf>() {
        return oxoffice_docx::Document::open(path).map_err(|error| docx_to_pyerr(py, error));
    }
    oxoffice_docx::Document::from_bytes(&binary_stream_bytes(value, true)?)
        .map_err(|error| docx_to_pyerr(py, error))
}

fn checked_heading_level(value: &Bound<'_, PyAny>) -> PyResult<i64> {
    if value.is_instance_of::<PyBool>() {
        return Err(PyTypeError::new_err(
            "heading level must be an integer from 0 through 9, not bool",
        ));
    }
    value.extract()
}

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

// Only current physical owners are retained. A committed mutation supplies the
// replacement inventory before any state is published.
#[derive(Clone)]
#[cfg_attr(debug_assertions, derive(Debug, PartialEq, Eq))]
pub(crate) enum WordOwnerLocation {
    Paragraph(oxoffice_docx::ContentLocation),
    Run(
        oxoffice_docx::ContentLocation,
        oxoffice_docx_oxml::text::AcceptedRunPath,
    ),
    Table(oxoffice_docx::ContentLocation),
    Cell {
        table: oxoffice_docx::ContentLocation,
        row: usize,
        cell: usize,
        story: oxoffice_docx::StoryId,
    },
    Section(usize),
    Hyperlink(oxoffice_docx::ContentLocation, usize),
}

#[derive(Clone, Default)]
#[cfg_attr(debug_assertions, derive(Debug, PartialEq, Eq))]
enum WordReadFacts {
    #[default]
    None,
    Paragraph(String),
    // One entry per modeled row/cell, containing the facade paragraph count.
    Table(Vec<Vec<usize>>),
}

#[cfg_attr(debug_assertions, derive(Debug, PartialEq, Eq))]
struct WordOwnerAddress {
    location: WordOwnerLocation,
    parent: Option<usize>,
    body_slot: Option<usize>,
    projection_ordinal: Option<usize>,
    read_facts: WordReadFacts,
}

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

#[derive(Clone, Copy)]
pub(crate) enum WordView {
    Owner,
    Content,
    Properties,
    Grid,
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

/// A multiply-rotate hasher over machine words, for owner routes and the
/// owner index maps. Their keys are process-local integers and route
/// hashes whose bucket members are still compared with `same_route`, so
/// they need no keyed hash, and a body edit indexes every owner again.
#[derive(Default)]
struct IndexHasher(u64);

/// A map of the owner indexes.
type IndexMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<IndexHasher>>;

impl Hasher for IndexHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(word));
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(0x517c_c1b7_2722_0a95);
    }

    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }
}

impl WordOwnerLocation {
    fn route_hash(&self) -> u64 {
        let mut hash = IndexHasher::default();
        std::mem::discriminant(self).hash(&mut hash);
        let mut content = |location: &oxoffice_docx::ContentLocation| {
            let story = location.story();
            story.kind().hash(&mut hash);
            // Equal routes share their part, so its length tells parts apart
            // well enough for buckets.
            story.part_name().len().hash(&mut hash);
            story.owner_index().hash(&mut hash);
            location.item_kind().hash(&mut hash);
            location.index_path().hash(&mut hash);
        };
        match self {
            Self::Paragraph(location) | Self::Table(location) => content(location),
            Self::Run(location, path) => {
                content(location);
                path.hash(&mut hash);
            }
            Self::Cell {
                table, row, cell, ..
            } => {
                content(table);
                row.hash(&mut hash);
                cell.hash(&mut hash);
            }
            Self::Section(index) => index.hash(&mut hash),
            Self::Hyperlink(location, index) => {
                content(location);
                index.hash(&mut hash);
            }
        }
        hash.finish()
    }
}

impl WordOwnerIndexes {
    fn prepare(owners: &[WordOwner], projections: WordIndexProjections) -> PyResult<Self> {
        let WordIndexProjections {
            body_paragraphs,
            cell_paragraphs,
            table_slots,
        } = projections;
        let mut indexes = Self {
            by_generation: IndexMap::default(),
            by_route: IndexMap::default(),
            direct_body_paragraphs: IndexMap::default(),
            body_paragraphs,
            body_tables: Vec::new(),
            cell_paragraphs,
            paragraphs_by_parent: IndexMap::default(),
            tables_by_parent: IndexMap::default(),
            cells_by_parent: IndexMap::default(),
            runs_by_parent: IndexMap::default(),
        };
        let fail =
            |_| pyo3::exceptions::PyMemoryError::new_err("Word owner index allocation failed");
        indexes
            .by_generation
            .try_reserve(owners.len())
            .map_err(fail)?;
        indexes.by_route.try_reserve(owners.len()).map_err(fail)?;
        indexes
            .direct_body_paragraphs
            .try_reserve(owners.len())
            .map_err(fail)?;
        let body_tables = indexes.index_owners(owners);
        indexes.body_tables = table_slots
            .into_iter()
            .map(|slot| slot.and_then(|slot| body_tables.get(&slot).cloned()))
            .collect();
        Ok(indexes)
    }

    fn prepare_addresses(
        locations: &[WordOwnerAddress],
        projections: WordIndexProjections,
    ) -> PyResult<Self> {
        let WordIndexProjections {
            body_paragraphs,
            cell_paragraphs,
            table_slots,
        } = projections;
        let mut indexes = Self::prepare(
            &[],
            WordIndexProjections {
                body_paragraphs,
                cell_paragraphs,
                table_slots: Vec::new(),
            },
        )?;
        let fail =
            |_| pyo3::exceptions::PyMemoryError::new_err("Word owner index allocation failed");
        indexes
            .by_generation
            .try_reserve(locations.len())
            .map_err(fail)?;
        indexes
            .by_route
            .try_reserve(locations.len())
            .map_err(fail)?;
        let mut tables = HashMap::new();
        for (index, address) in locations.iter().enumerate() {
            indexes.index_location(
                index,
                &address.location,
                address.parent,
                address.body_slot,
                &mut tables,
            );
        }
        indexes.body_tables = table_slots
            .into_iter()
            .map(|slot| slot.and_then(|slot| tables.get(&slot).cloned()))
            .collect();
        Ok(indexes)
    }

    /// Index `owners` again after they moved in place, keeping the document
    /// projections, which the caller moves with them.
    fn index_owners_again(&mut self, owners: &[WordOwner]) {
        self.by_generation.clear();
        self.by_generation.reserve(owners.len());
        // Route buckets keep their allocations, and those left empty go.
        self.by_route.values_mut().for_each(Vec::clear);
        self.by_route
            .reserve(owners.len().saturating_sub(self.by_route.len()));
        self.direct_body_paragraphs.clear();
        self.paragraphs_by_parent.clear();
        self.tables_by_parent.clear();
        self.cells_by_parent.clear();
        self.runs_by_parent.clear();
        self.index_owners(owners);
        self.by_route.retain(|_, bucket| !bucket.is_empty());
    }

    /// Index every owner, and return the first body table owner of each
    /// direct body slot.
    fn index_owners(
        &mut self,
        owners: &[WordOwner],
    ) -> HashMap<usize, oxoffice_docx::ContentLocation> {
        let indexes = self;
        let mut body_tables = HashMap::new();
        for (index, owner) in owners.iter().enumerate() {
            indexes
                .by_generation
                .insert(owner.owner.generation(), index);
            indexes.index_location(
                index,
                &owner.location,
                owner.parent,
                owner.body_slot,
                &mut body_tables,
            );
        }
        body_tables
    }

    /// The same route/parent classification is used before and after scope initialization.
    fn index_location(
        &mut self,
        index: usize,
        location: &WordOwnerLocation,
        parent: Option<usize>,
        body_slot: Option<usize>,
        body_tables: &mut HashMap<usize, oxoffice_docx::ContentLocation>,
    ) {
        self.by_route
            .entry(location.route_hash())
            .or_default()
            .push(index);
        match location {
            WordOwnerLocation::Paragraph(location) => {
                if location.story().kind() == oxoffice_docx::StoryKind::Body
                    && location.index_path().len() == 1
                    && let Some(slot) = body_slot
                {
                    self.direct_body_paragraphs.entry(slot).or_insert(index);
                }
                if let Some(parent) = parent {
                    self.paragraphs_by_parent
                        .entry(parent)
                        .or_default()
                        .push(index);
                }
            }
            WordOwnerLocation::Table(location) => {
                if location.story().kind() == oxoffice_docx::StoryKind::Body
                    && let Some(slot) = body_slot
                {
                    body_tables.entry(slot).or_insert_with(|| location.clone());
                }
                if let Some(parent) = parent {
                    self.tables_by_parent.entry(parent).or_default().push(index);
                }
            }
            WordOwnerLocation::Cell { .. } => {
                if let Some(parent) = parent {
                    self.cells_by_parent.entry(parent).or_default().push(index);
                }
            }
            WordOwnerLocation::Run(..) => {
                if let Some(parent) = parent {
                    self.runs_by_parent.entry(parent).or_default().push(index);
                }
            }
            _ => {}
        }
    }

    fn find(&self, owners: &[WordOwner], location: &WordOwnerLocation) -> Option<usize> {
        self.by_route
            .get(&location.route_hash())?
            .iter()
            .copied()
            .find(|index| owners[*index].location.same_route(location))
    }
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

fn word_owner_capture(
    counter: &RevisionCounter,
    owners: &[WordOwner],
    document_scopes: (RevisionState, &RevisionState),
    index: usize,
    segments: smallvec::SmallVec<[PathSeg; 5]>,
    view: WordView,
    furniture_scope: Option<&RevisionState>,
) -> PyResult<(ContentPath, RevisionState)> {
    let (document_scope, removal_scope) = document_scopes;
    let target = &owners[index];
    // Every structural publisher retires unproved current-owner states. Actual
    // Run handles retain the conservative document epoch and its first cause.
    let mut states = vec![removal_scope.clone(), target.owner.clone()];
    if matches!(&target.location, WordOwnerLocation::Run(..)) {
        states.push(document_scope);
    }
    if let Some(scope) = furniture_scope {
        states.push(scope.clone());
    }
    let child = match view {
        WordView::Owner => None,
        WordView::Content => target.content.as_ref(),
        WordView::Properties => target.properties.as_ref(),
        WordView::Grid => target.grid.as_ref(),
    };
    if let Some(child) = child {
        states.push(child.clone());
    }
    let mut parent = target.parent;
    while let Some(index) = parent {
        let ancestor = &owners[index];
        states.push(ancestor.owner.clone());
        if let Some(content) = &ancestor.content {
            states.push(content.clone());
        }
        if let Some(grid) = &ancestor.grid {
            states.push(grid.clone());
        }
        parent = ancestor.parent;
    }
    let path = counter
        .capture_scopes(segments, &states)
        .map_err(revision_to_pyerr)?;
    Ok((path, target.owner.clone()))
}

fn same_story_owner(a: &oxoffice_docx::StoryId, b: &oxoffice_docx::StoryId) -> bool {
    a.kind() == b.kind() && a.part_name() == b.part_name() && a.owner_index() == b.owner_index()
}

fn same_content_owner(
    a: &oxoffice_docx::ContentLocation,
    b: &oxoffice_docx::ContentLocation,
) -> bool {
    same_story_owner(a.story(), b.story())
        && a.item_kind() == b.item_kind()
        && a.index_path() == b.index_path()
}

impl WordOwnerLocation {
    fn same_route(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Paragraph(a), Self::Paragraph(b)) | (Self::Table(a), Self::Table(b)) => {
                same_content_owner(a, b)
            }
            (Self::Run(a, ap), Self::Run(b, bp)) => same_content_owner(a, b) && ap == bp,
            (
                Self::Cell {
                    table: a,
                    row: ar,
                    cell: ac,
                    ..
                },
                Self::Cell {
                    table: b,
                    row: br,
                    cell: bc,
                    ..
                },
            ) => same_content_owner(a, b) && ar == br && ac == bc,
            (Self::Section(a), Self::Section(b)) => a == b,
            (Self::Hyperlink(a, ai), Self::Hyperlink(b, bi)) => {
                same_content_owner(a, b) && ai == bi
            }
            _ => false,
        }
    }

    fn scopes(&self) -> &'static [RevisionScope] {
        match self {
            Self::Paragraph(_) => &[
                RevisionScope::Owner,
                RevisionScope::Text,
                RevisionScope::Text,
            ],
            Self::Table(_) => &[RevisionScope::Owner, RevisionScope::Grid],
            Self::Hyperlink(..) => &[RevisionScope::Owner],
            Self::Run(..) | Self::Cell { .. } | Self::Section(_) => {
                &[RevisionScope::Owner, RevisionScope::Text]
            }
        }
    }
}

// Source keys include the complete physical route, not projected text ordinals.
type FormSourceKey = (
    oxoffice_docx::StoryKind,
    String,
    usize,
    oxoffice_docx::StoryItemKind,
    Vec<usize>,
);
type FormStoryKey = (oxoffice_docx::StoryKind, String, usize);

#[derive(Clone, PartialEq, Eq, Hash)]
enum FormOwnerRoute {
    Paragraph(FormSourceKey),
    Table(FormSourceKey),
    Cell(FormSourceKey, usize, usize, FormStoryKey),
    Section(usize),
}

#[derive(Default)]
struct FormSources {
    xml: HashMap<FormOwnerRoute, Option<Vec<u8>>>,
    cells: HashMap<FormOwnerRoute, usize>,
    cell_stories: HashMap<FormStoryKey, usize>,
}

fn form_source_key(location: &oxoffice_docx::ContentLocation) -> FormSourceKey {
    let story = location.story();
    (
        story.kind(),
        story.part_name().to_owned(),
        story.owner_index(),
        location.item_kind(),
        location.index_path().to_vec(),
    )
}

fn form_owner_route(location: &WordOwnerLocation) -> Option<FormOwnerRoute> {
    match location {
        WordOwnerLocation::Paragraph(location) => {
            Some(FormOwnerRoute::Paragraph(form_source_key(location)))
        }
        WordOwnerLocation::Table(location) => {
            Some(FormOwnerRoute::Table(form_source_key(location)))
        }
        WordOwnerLocation::Cell {
            table,
            row,
            cell,
            story,
        } => Some(FormOwnerRoute::Cell(
            form_source_key(table),
            *row,
            *cell,
            (
                story.kind(),
                story.part_name().to_owned(),
                story.owner_index(),
            ),
        )),
        WordOwnerLocation::Section(index) => Some(FormOwnerRoute::Section(*index)),
        WordOwnerLocation::Run(..) | WordOwnerLocation::Hyperlink(..) => None,
    }
}

fn word_table_read_facts(
    document: &oxoffice_docx::Document,
) -> HashMap<oxoffice_docx::ContentLocation, WordReadFacts> {
    // Optional acceleration only. The checked native reader remains the fallback
    // for source that the standalone checked table parser cannot project.
    document
        .standalone_story_item_snapshots()
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item.location().item_kind() == oxoffice_docx::StoryItemKind::Table)
        .filter_map(|item| Some((item.location().clone(), word_table_facts(item.xml())?)))
        .collect()
}

/// The facade paragraph count of each modeled cell of a standalone table.
fn word_table_facts(xml: &[u8]) -> Option<WordReadFacts> {
    let table = oxoffice_docx_oxml::table::CT_Tbl::from_xml_fragment_checked(xml).ok()?;
    let rows = table
        .rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|cell| cell.paragraphs().len())
                .collect()
        })
        .collect();
    Some(WordReadFacts::Table(rows))
}

/// The four owner snapshot families of [`oxoffice_docx::Document::story_owner_snapshots`].
type WordOwnerSnapshots = (
    Vec<oxoffice_docx::StoryItemSnapshot>,
    Vec<(
        oxoffice_docx::ContentLocation,
        usize,
        usize,
        oxoffice_docx::StoryId,
    )>,
    Vec<(oxoffice_docx::ContentLocation, Vec<u8>)>,
    Vec<(oxoffice_docx::ContentLocation, oxoffice_docx::LinkInfo)>,
);

type WordParagraphFacts = (
    oxoffice_docx::ContentLocation,
    String,
    Vec<oxoffice_docx_oxml::text::AcceptedRunPath>,
);

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

impl WordOwnerFacts {
    /// Prove every native physical cell has one indexed direct coordinate.
    /// Compute once for immutable standalone-piece facts, never per setter.
    fn has_complete_table_membership(&self, physical_cells: usize) -> bool {
        let Some((root, Some(0), WordReadFacts::Table(rows))) = self.items.first() else {
            return false;
        };
        if root.story().kind() != oxoffice_docx::StoryKind::Body
            || root.index_path() != [0]
            || self
                .items
                .iter()
                .filter(|(location, _, _)| {
                    location.item_kind() == oxoffice_docx::StoryItemKind::Table
                })
                .count()
                != 1
            || self.cells.len() != physical_cells
        {
            return false;
        }
        let mut coordinates = std::collections::HashSet::new();
        for (ordinal, (table, row, cell, story)) in self.cells.iter().enumerate() {
            if table != root
                || story.kind() != oxoffice_docx::StoryKind::TableCell
                || story.part_name() != root.story().part_name()
                || story.owner_index() != ordinal
                || rows.get(*row).and_then(|cells| cells.get(*cell)).is_none()
                || !coordinates.insert((*row, *cell))
            {
                return false;
            }
        }
        // Native cell_stories includes controlled and empty physical cells.
        // A projected-cell count alone cannot certify their membership/base.
        self.paragraphs.iter().all(|(location, _, _)| {
            let story = location.story();
            story.kind() == oxoffice_docx::StoryKind::TableCell
                && story.part_name() == root.story().part_name()
                && story.owner_index() < physical_cells
        })
    }
}

fn word_owner_facts(
    (snapshots, cells, paragraphs, links): WordOwnerSnapshots,
    table_facts: impl Fn(&oxoffice_docx::StoryItemSnapshot) -> Option<WordReadFacts>,
) -> oxoffice_docx::Result<Option<WordOwnerFacts>> {
    let mut items = Vec::new();
    for snapshot in snapshots.iter().filter(|snapshot| {
        matches!(
            snapshot.location().item_kind(),
            oxoffice_docx::StoryItemKind::Table
                | oxoffice_docx::StoryItemKind::Paragraph
                | oxoffice_docx::StoryItemKind::ContentControl
        )
    }) {
        let read_facts = if snapshot.location().item_kind() == oxoffice_docx::StoryItemKind::Table {
            match table_facts(snapshot) {
                Some(facts) => facts,
                None => return Ok(None),
            }
        } else {
            WordReadFacts::None
        };
        let slot = snapshot
            .direct_body_index()
            .filter(|_| snapshot.is_direct_child());
        items.push((snapshot.location().clone(), slot, read_facts));
    }
    let mut parsed = Vec::new();
    for (location, xml) in paragraphs {
        let paragraph = oxoffice_docx_oxml::CT_P::from_xml_fragment(&xml)?;
        parsed.push((
            location,
            paragraph.accepted_text(),
            paragraph.accepted_run_paths(),
        ));
    }
    Ok(Some(WordOwnerFacts {
        items,
        cells,
        paragraphs: parsed,
        links: links.into_iter().map(|(location, _)| location).collect(),
    }))
}

/// Addresses from piece facts. The full path stays [`word_owner_addresses`].
fn word_piece_owner_addresses(facts: WordOwnerFacts, sections: usize) -> Vec<WordOwnerAddress> {
    let WordOwnerFacts {
        items,
        cells,
        paragraphs,
        links,
    } = facts;
    let mut result = Vec::new();
    let mut table_ordinals = HashMap::new();
    let mut body_slots = HashMap::new();
    let mut cell_parents = HashMap::new();
    let mut table_cells = HashMap::<oxoffice_docx::ContentLocation, Vec<_>>::new();
    for (table, row, cell, story) in cells {
        table_cells
            .entry(table)
            .or_default()
            .push((row, cell, story));
    }
    for (location, slot, _) in &items {
        if location.story().kind() == oxoffice_docx::StoryKind::Body
            && matches!(
                location.item_kind(),
                oxoffice_docx::StoryItemKind::Paragraph
                    | oxoffice_docx::StoryItemKind::ContentControl
            )
            && let Some(slot) = slot
        {
            body_slots.insert(location.clone(), *slot);
        }
    }
    for (location, slot, read_facts) in items {
        if location.item_kind() != oxoffice_docx::StoryItemKind::Table {
            continue;
        }
        let ordinal = table_ordinals
            .entry(location.story().clone())
            .or_insert(0usize);
        let projection_ordinal = Some(*ordinal);
        *ordinal += 1;
        let table_index = result.len();
        let cells = table_cells.remove(&location).unwrap_or_default();
        result.push(WordOwnerAddress {
            location: WordOwnerLocation::Table(location.clone()),
            parent: None,
            projection_ordinal,
            read_facts,
            body_slot: slot,
        });
        for (row, cell, story) in cells {
            result.push(WordOwnerAddress {
                location: WordOwnerLocation::Cell {
                    table: location.clone(),
                    row,
                    cell,
                    story,
                },
                parent: Some(table_index),
                body_slot: None,
                projection_ordinal: None,
                read_facts: WordReadFacts::None,
            });
        }
    }
    for (index, address) in result.iter().enumerate() {
        if let WordOwnerLocation::Cell { story, .. } = &address.location {
            cell_parents
                .entry((
                    story.kind(),
                    story.part_name().to_owned(),
                    story.owner_index(),
                ))
                .or_insert(index);
        }
    }
    for address in &mut result {
        if let WordOwnerLocation::Table(location) = &address.location {
            let story = location.story();
            address.parent = cell_parents
                .get(&(
                    story.kind(),
                    story.part_name().to_owned(),
                    story.owner_index(),
                ))
                .copied();
        }
    }
    let mut paragraph_ordinals = HashMap::new();
    for (location, text, runs) in paragraphs {
        let story = location.story();
        let parent = cell_parents
            .get(&(
                story.kind(),
                story.part_name().to_owned(),
                story.owner_index(),
            ))
            .copied();
        let ordinal = paragraph_ordinals
            .entry(location.story().clone())
            .or_insert(0usize);
        let projection_ordinal = Some(*ordinal);
        *ordinal += 1;
        let index = result.len();
        let body_slot = if location.story().kind() == oxoffice_docx::StoryKind::Body {
            match location.index_path() {
                [_] => body_slots.get(&location).copied(),
                [control, _] => body_slots
                    .get(&oxoffice_docx::ContentLocation::new(
                        location.story().clone(),
                        oxoffice_docx::StoryItemKind::ContentControl,
                        vec![*control],
                    ))
                    .copied(),
                _ => None,
            }
        } else {
            None
        };
        result.push(WordOwnerAddress {
            location: WordOwnerLocation::Paragraph(location.clone()),
            parent,
            body_slot,
            projection_ordinal,
            read_facts: WordReadFacts::Paragraph(text),
        });
        for path in runs {
            result.push(WordOwnerAddress {
                location: WordOwnerLocation::Run(location.clone(), path),
                parent: Some(index),
                body_slot: None,
                projection_ordinal: None,
                read_facts: WordReadFacts::None,
            });
        }
    }
    for index in 0..sections {
        result.push(WordOwnerAddress {
            location: WordOwnerLocation::Section(index),
            parent: None,
            body_slot: None,
            projection_ordinal: None,
            read_facts: WordReadFacts::None,
        });
    }
    let mut local_indexes = HashMap::<oxoffice_docx::ContentLocation, usize>::new();
    let mut story_indexes = HashMap::<oxoffice_docx::StoryId, usize>::new();
    for location in links {
        let local = local_indexes.entry(location.clone()).or_default();
        let position = story_indexes.entry(location.story().clone()).or_default();
        result.push(WordOwnerAddress {
            location: WordOwnerLocation::Hyperlink(location.clone(), *local),
            parent: None,
            body_slot: None,
            projection_ordinal: Some(*position),
            read_facts: WordReadFacts::None,
        });
        *local += 1;
        *position += 1;
    }
    result
}

struct WordIndexProjections {
    body_paragraphs: Option<Vec<Option<oxoffice_docx::ContentLocation>>>,
    cell_paragraphs: CellParagraphRoutes,
    table_slots: Vec<Option<usize>>,
}

type CellParagraphRoutes = HashMap<(usize, usize, usize), Vec<oxoffice_docx::ContentLocation>>;

impl WordIndexProjections {
    fn of(document: &oxoffice_docx::Document) -> Self {
        Self {
            body_paragraphs: document.paragraph_story_locations().ok(),
            cell_paragraphs: document
                .cell_paragraph_story_locations()
                .unwrap_or_default(),
            table_slots: (0..document.table_count())
                .map(|index| document.content_index_of_table(index))
                .collect(),
        }
    }
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

fn word_block_owners(
    document: &oxoffice_docx::Document,
    block: &oxoffice_docx_oxml::BodyContent,
) -> oxoffice_docx::Result<Option<WordBlockOwners>> {
    if let oxoffice_docx_oxml::BodyContent::Paragraph(paragraph) = block
        && let Ok(Some((text, runs))) = document.direct_body_paragraph_facts(paragraph)
    {
        let context = document.body_item_context();
        let story =
            oxoffice_docx::StoryId::new(oxoffice_docx::StoryKind::Body, context.part_name(), 0);
        return Ok(Some(plain_paragraph_owners(&story, &text, &runs)));
    }
    let oxoffice_docx::BodyItemOwners {
        snapshots,
        cells,
        paragraphs,
        links,
        cell_stories,
        text_boxes,
        paragraph_routes,
        tables,
        cell_paragraphs,
        holds_field,
    } = document.body_item_owners(block)?;
    let body = |snapshot: &&oxoffice_docx::StoryItemSnapshot| {
        snapshot.location().story().kind() == oxoffice_docx::StoryKind::Body
    };
    let direct = snapshots
        .iter()
        .filter(body)
        .filter(|snapshot| snapshot.is_direct_child())
        .map(oxoffice_docx::StoryItemSnapshot::direct_body_index)
        .collect::<Vec<_>>();
    if direct.first() != Some(&Some(0)) || direct.len() != 1 {
        return Ok(None);
    }
    let body_items = snapshots.iter().filter(body).count();
    let relationships = link_relationships(&links);
    let Some(facts) = word_owner_facts((snapshots, cells, paragraphs, links), |snapshot| {
        word_table_facts(snapshot.xml())
    })?
    else {
        return Ok(None);
    };
    Ok(Some(WordBlockOwners {
        local_table_membership: facts.has_complete_table_membership(cell_stories),
        facts,
        relationships,
        body_items,
        cell_stories,
        text_boxes,
        paragraphs: paragraph_routes,
        tables,
        cell_paragraphs,
        holds_field,
    }))
}

fn link_relationships(
    links: &[(oxoffice_docx::ContentLocation, oxoffice_docx::LinkInfo)],
) -> Vec<String> {
    let mut relationships = links
        .iter()
        .filter_map(|(_, link)| link.rel_id.clone())
        .collect::<Vec<_>>();
    relationships.sort_unstable();
    relationships.dedup();
    relationships
}

fn plain_paragraph_owners(
    body: &oxoffice_docx::StoryId,
    text: &str,
    runs: &[oxoffice_docx_oxml::text::AcceptedRunPath],
) -> WordBlockOwners {
    let location = oxoffice_docx::ContentLocation::new(
        body.clone(),
        oxoffice_docx::StoryItemKind::Paragraph,
        vec![0],
    );
    WordBlockOwners {
        facts: WordOwnerFacts {
            items: vec![(location.clone(), Some(0), WordReadFacts::None)],
            cells: Vec::new(),
            paragraphs: vec![(location, text.to_owned(), runs.to_vec())],
            links: Vec::new(),
        },
        relationships: Vec::new(),
        body_items: 1,
        cell_stories: 0,
        text_boxes: 0,
        paragraphs: vec![Some(vec![0])],
        tables: 0,
        local_table_membership: false,
        cell_paragraphs: Some((HashMap::new(), true)),
        holds_field: false,
    }
}

fn table_owners(facts: WordOwnerFacts, cells: usize) -> Option<WordBlockOwners> {
    let Some((_, _, WordReadFacts::Table(rows))) = facts.items.first() else {
        return None;
    };
    let mut stories = HashMap::new();
    let mut routes = CellParagraphRoutes::new();
    for (_, row, cell, story) in &facts.cells {
        stories.insert(story.owner_index(), (*row, *cell));
        routes.insert((0, *row, *cell), Vec::new());
    }
    for (location, _, _) in &facts.paragraphs {
        if location.story().kind() == oxoffice_docx::StoryKind::TableCell
            && let Some((row, cell)) = stories.get(&location.story().owner_index())
        {
            routes
                .entry((0, *row, *cell))
                .or_default()
                .push(location.clone());
        }
    }
    routes.retain(|(_, row, cell), locations| {
        rows.get(*row).and_then(|row| row.get(*cell)) == Some(&locations.len())
    });
    Some(WordBlockOwners {
        local_table_membership: facts.has_complete_table_membership(cells),
        relationships: Vec::new(),
        body_items: 1,
        cell_stories: cells,
        text_boxes: 0,
        paragraphs: Vec::new(),
        tables: 1,
        cell_paragraphs: Some((routes, true)),
        facts,
        holds_field: false,
    })
}

fn placed_route(
    location: &oxoffice_docx::ContentLocation,
    body: &oxoffice_docx::StoryId,
    (items, stories): (usize, (usize, usize)),
) -> oxoffice_docx::ContentLocation {
    let story = location.story();
    if story.kind() == oxoffice_docx::StoryKind::Body {
        let mut placed = oxoffice_docx::ContentLocation::new(
            body.clone(),
            location.item_kind(),
            location.index_path().to_vec(),
        );
        if let Some(head) = location.index_path().first() {
            placed.set_index_path_head(head + items);
        }
        placed
    } else {
        oxoffice_docx::ContentLocation::new(
            placed_story(story, stories),
            location.item_kind(),
            location.index_path().to_vec(),
        )
    }
}

fn placed_story(
    story: &oxoffice_docx::StoryId,
    (cells, boxes): (usize, usize),
) -> oxoffice_docx::StoryId {
    let mut placed = story.clone();
    let before = if story.kind() == oxoffice_docx::StoryKind::TextBox {
        boxes
    } else {
        cells
    };
    placed.set_owner_index(story.owner_index() + before);
    placed
}

struct WordBlockPiece {
    block: std::sync::Arc<oxoffice_docx_oxml::BodyContent>,
    owners: Option<WordBlockOwners>,
}

struct WordPartPiece {
    kind: oxoffice_docx::StoryKind,
    part_name: String,
    xml: Vec<u8>,
    owners: Option<(WordOwnerFacts, Vec<String>)>,
}

/// Cached body blocks and related story parts.
#[derive(Clone, Default)]
pub(crate) struct WordPieces {
    context: Option<oxoffice_docx::BodyItemContext>,
    blocks: Vec<std::sync::Arc<WordBlockPiece>>,
    parts: Vec<std::sync::Arc<WordPartPiece>>,
}

fn aligned_block(piece: &WordBlockPiece, block: &oxoffice_docx_oxml::BodyContent) -> bool {
    piece.block.as_ref() == block
}

fn qualified_block(piece: &WordBlockPiece, block: &oxoffice_docx_oxml::BodyContent) -> bool {
    aligned_block(piece, block)
        && piece
            .owners
            .as_ref()
            .is_some_and(|owners| !owners.holds_field)
        && piece.block.owner_facts_reusable_by_value()
        && block.owner_facts_reusable_by_value()
}

impl WordPieces {
    fn assemble(
        &self,
        document: &oxoffice_docx::Document,
    ) -> (Self, Option<(Vec<WordOwnerAddress>, WordIndexProjections)>) {
        let context = document.body_item_context();
        let blocks = document.body_children();
        let Ok(sources) = document.story_part_sources() else {
            return (self.clone(), None);
        };
        // Sources have already resolved dot segments. OPC physical identity is
        // case insensitive even when relationship targets retain their spelling.
        let mut names = std::collections::HashSet::from([context.part_name().to_ascii_lowercase()]);
        if !sources
            .iter()
            .all(|(_, name, _)| names.insert(name.to_ascii_lowercase()))
        {
            return (self.clone(), None);
        }
        let opened_paragraphs = if self.blocks.is_empty() {
            document
                .opened_plain_body_paragraph_facts()
                .ok()
                .flatten()
                .filter(|facts| facts.len() == blocks.len())
        } else {
            None
        };
        let same_context = self
            .context
            .as_ref()
            .is_some_and(|old| old.read_facts_compatible(&context));
        let changed_existing_context = self.context.is_some() && !same_context;
        let old = if same_context {
            self.blocks.as_slice()
        } else {
            &[]
        };
        let prefix = old
            .iter()
            .zip(blocks)
            .take_while(|(piece, block)| aligned_block(piece, block))
            .count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(blocks[prefix..].iter().rev())
            .take_while(|(piece, block)| aligned_block(piece, block))
            .count();
        let mut pieces = Self {
            context: Some(context),
            blocks: Vec::with_capacity(blocks.len()),
            parts: Vec::new(),
        };
        for (piece, block) in old[..prefix].iter().zip(&blocks[..prefix]) {
            if qualified_block(piece, block) {
                pieces.blocks.push(std::sync::Arc::clone(piece));
            } else {
                let owners = word_block_owners(document, block).ok().flatten();
                pieces.blocks.push(std::sync::Arc::new(WordBlockPiece {
                    block: std::sync::Arc::new(block.clone()),
                    owners,
                }));
            }
        }
        for (offset, block) in blocks[prefix..blocks.len() - suffix].iter().enumerate() {
            let owners = opened_paragraphs
                .as_ref()
                .and_then(|facts| facts.get(prefix + offset))
                .map(|(_, text, runs)| {
                    let story = oxoffice_docx::StoryId::new(
                        oxoffice_docx::StoryKind::Body,
                        pieces
                            .context
                            .as_ref()
                            .expect("current body context")
                            .part_name(),
                        0,
                    );
                    plain_paragraph_owners(&story, text, runs)
                })
                .or_else(|| word_block_owners(document, block).ok().flatten());
            pieces.blocks.push(std::sync::Arc::new(WordBlockPiece {
                block: opened_paragraphs
                    .as_ref()
                    .and_then(|facts| facts.get(prefix + offset))
                    .and_then(|(model, _, _)| model.clone())
                    .unwrap_or_else(|| std::sync::Arc::new(block.clone())),
                owners,
            }));
        }
        for (piece, block) in old[old.len() - suffix..]
            .iter()
            .zip(&blocks[blocks.len() - suffix..])
        {
            if qualified_block(piece, block) {
                pieces.blocks.push(std::sync::Arc::clone(piece));
            } else {
                let owners = word_block_owners(document, block).ok().flatten();
                pieces.blocks.push(std::sync::Arc::new(WordBlockPiece {
                    block: std::sync::Arc::new(block.clone()),
                    owners,
                }));
            }
        }
        let old_parts = if same_context {
            self.parts.as_slice()
        } else {
            &[]
        };
        for (kind, part_name, xml) in sources {
            if let Some(piece) = old_parts.iter().find(|piece| {
                piece.owners.is_some()
                    && piece.kind == kind
                    && piece.part_name == part_name
                    && piece.xml == *xml
            }) {
                pieces.parts.push(piece.clone());
                continue;
            }
            let owners = document
                .story_part_owner_snapshots(kind, &part_name, &xml)
                .ok()
                .and_then(|snapshots| {
                    let relationships = link_relationships(&snapshots.3);
                    word_owner_facts(snapshots, |snapshot| word_table_facts(snapshot.xml()))
                        .ok()
                        .flatten()
                        .map(|facts| (facts, relationships))
                });
            pieces.parts.push(std::sync::Arc::new(WordPartPiece {
                kind,
                part_name,
                xml: xml.into_owned(),
                owners,
            }));
        }
        let inventory = if changed_existing_context {
            None
        } else {
            pieces.inventory(document)
        };
        (pieces, inventory)
    }

    /// Keep body pieces aligned with an in-place splice of `document`.
    ///
    /// `removed` pieces at `slot` go. `inserted` is the block now at `slot`.
    /// A count or slot that does not match the document drops the body pieces
    /// and leaves related parts in place. Context is unchanged.
    fn splice(
        &mut self,
        document: &oxoffice_docx::Document,
        slot: usize,
        removed: usize,
        inserted: Option<WordBlockOwners>,
    ) {
        let blocks = document.body_children();
        let added = usize::from(inserted.is_some());
        if self.blocks.len() + added != blocks.len() + removed || slot + removed > self.blocks.len()
        {
            self.blocks.clear();
            return;
        }
        let piece = inserted.map(|owners| {
            std::sync::Arc::new(WordBlockPiece {
                block: std::sync::Arc::new(blocks[slot].clone()),
                owners: Some(owners),
            })
        });
        self.blocks.splice(slot..slot + removed, piece);
    }

    fn inventory(
        &self,
        document: &oxoffice_docx::Document,
    ) -> Option<(Vec<WordOwnerAddress>, WordIndexProjections)> {
        let main = self.context.as_ref()?.part_name();
        let body = oxoffice_docx::StoryId::new(oxoffice_docx::StoryKind::Body, main, 0);
        let mut links = None;
        let mut facts = WordOwnerFacts::default();
        let mut cells = WordOwnerFacts::default();
        let mut projections = WordIndexProjections {
            body_paragraphs: Some(Vec::new()),
            cell_paragraphs: HashMap::new(),
            table_slots: Vec::new(),
        };
        let mut cell_paragraphs = Some(HashMap::new());
        let mut routed = true;
        let (mut items, mut stories, mut tables) = (0, (0, 0), 0);
        for (slot, piece) in self.blocks.iter().enumerate() {
            let owners = piece.owners.as_ref()?;
            if !owners.relationships.is_empty() {
                let valid =
                    links.get_or_insert_with(|| document.story_hyperlink_relationships(main));
                if !owners
                    .relationships
                    .iter()
                    .all(|relationship| valid.contains(relationship.as_str()))
                {
                    return None;
                }
            }
            let place = |location: &oxoffice_docx::ContentLocation| {
                placed_route(location, &body, (items, stories))
            };
            let in_body = |location: &oxoffice_docx::ContentLocation| {
                location.story().kind() == oxoffice_docx::StoryKind::Body
            };
            for (location, direct, read_facts) in &owners.facts.items {
                let family = if in_body(location) {
                    &mut facts.items
                } else {
                    &mut cells.items
                };
                family.push((place(location), direct.map(|_| slot), read_facts.clone()));
            }
            for (table, row, cell, story) in &owners.facts.cells {
                facts
                    .cells
                    .push((place(table), *row, *cell, placed_story(story, stories)));
            }
            for (location, text, runs) in &owners.facts.paragraphs {
                let family = if in_body(location) {
                    &mut facts.paragraphs
                } else {
                    &mut cells.paragraphs
                };
                family.push((place(location), text.clone(), runs.clone()));
            }
            for location in &owners.facts.links {
                let family = if in_body(location) {
                    &mut facts.links
                } else {
                    &mut cells.links
                };
                family.push(place(location));
            }
            if let Some(paragraphs) = &mut projections.body_paragraphs {
                paragraphs.extend(owners.paragraphs.iter().map(|path| {
                    path.as_ref().map(|path| {
                        let mut location = oxoffice_docx::ContentLocation::new(
                            body.clone(),
                            oxoffice_docx::StoryItemKind::Paragraph,
                            path.clone(),
                        );
                        location.set_index_path_head(path[0] + items);
                        location
                    })
                }));
            }
            if matches!(
                piece.block.as_ref(),
                oxoffice_docx_oxml::BodyContent::Table(_)
            ) {
                projections.table_slots.push(Some(slot));
            } else {
                projections
                    .table_slots
                    .extend(std::iter::repeat_n(None, owners.tables));
            }
            if routed {
                match &owners.cell_paragraphs {
                    Some((routes, complete)) => {
                        if let Some(all) = &mut cell_paragraphs {
                            for ((table, row, cell), locations) in routes {
                                all.insert(
                                    (table + tables, *row, *cell),
                                    locations.iter().map(&place).collect(),
                                );
                            }
                        }
                        routed = *complete;
                    }
                    None => {
                        cell_paragraphs = None;
                        routed = false;
                    }
                }
            }
            items += owners.body_items;
            stories.0 += owners.cell_stories;
            stories.1 += owners.text_boxes;
            tables += owners.tables;
        }
        projections.cell_paragraphs = cell_paragraphs.unwrap_or_default();
        facts.items.append(&mut cells.items);
        facts.paragraphs.append(&mut cells.paragraphs);
        facts.links.append(&mut cells.links);
        for piece in &self.parts {
            let (owners, relationships) = piece.owners.as_ref()?;
            if !relationships.is_empty() {
                let valid = document.story_hyperlink_relationships(&piece.part_name);
                if !relationships
                    .iter()
                    .all(|relationship| valid.contains(relationship.as_str()))
                {
                    return None;
                }
            }
            facts.items.extend_from_slice(&owners.items);
            facts.cells.extend_from_slice(&owners.cells);
            facts.paragraphs.extend_from_slice(&owners.paragraphs);
            facts.links.extend_from_slice(&owners.links);
        }
        Some((
            word_piece_owner_addresses(facts, document.section_count()),
            projections,
        ))
    }
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

fn word_owner_locations(
    document: &oxoffice_docx::Document,
    form_sources: Option<&mut FormSources>,
) -> oxoffice_docx::Result<Vec<WordOwnerAddress>> {
    let table_facts = word_table_read_facts(document);
    word_owner_addresses(
        document.story_owner_snapshots()?,
        &table_facts,
        Some(document),
        form_sources,
    )
}

/// The owner inventory of the given snapshots. Sections are those of
/// `document`, and a snapshot of a detached item has none.
fn word_owner_addresses(
    (snapshots, cell_snapshots, paragraph_snapshots, link_snapshots): WordOwnerSnapshots,
    table_facts: &HashMap<oxoffice_docx::ContentLocation, WordReadFacts>,
    document: Option<&oxoffice_docx::Document>,
    mut form_sources: Option<&mut FormSources>,
) -> oxoffice_docx::Result<Vec<WordOwnerAddress>> {
    let mut result = Vec::new();
    let mut table_ordinals = HashMap::new();
    if let Some(sources) = form_sources.as_deref_mut() {
        for snapshot in &snapshots {
            if snapshot.location().item_kind() == oxoffice_docx::StoryItemKind::Table {
                let key = FormOwnerRoute::Table(form_source_key(snapshot.location()));
                sources
                    .xml
                    .entry(key)
                    .and_modify(|xml| *xml = None)
                    .or_insert_with(|| Some(snapshot.xml().to_vec()));
            }
        }
        // Preserve raw multiplicity before the legacy cell-parent map collapses it.
        for (table, row, cell, story) in &cell_snapshots {
            let story_key = (
                story.kind(),
                story.part_name().to_owned(),
                story.owner_index(),
            );
            *sources.cell_stories.entry(story_key.clone()).or_default() += 1;
            *sources
                .cells
                .entry(FormOwnerRoute::Cell(
                    form_source_key(table),
                    *row,
                    *cell,
                    story_key,
                ))
                .or_default() += 1;
        }
    }
    let mut body_slots = HashMap::new();
    let mut cell_parents = HashMap::new();
    let mut table_cells = HashMap::<oxoffice_docx::ContentLocation, Vec<_>>::new();
    for (table, row, cell, story) in cell_snapshots {
        table_cells
            .entry(table)
            .or_default()
            .push((row, cell, story));
    }
    for snapshot in &snapshots {
        let location = snapshot.location();
        if location.story().kind() == oxoffice_docx::StoryKind::Body
            && snapshot.is_direct_child()
            && matches!(
                location.item_kind(),
                oxoffice_docx::StoryItemKind::Paragraph
                    | oxoffice_docx::StoryItemKind::ContentControl
            )
        {
            body_slots.insert(location.clone(), snapshot.direct_body_index());
        }
    }
    for snapshot in &snapshots {
        if snapshot.location().item_kind() != oxoffice_docx::StoryItemKind::Table {
            continue;
        }
        let location = snapshot.location().clone();
        let ordinal = table_ordinals
            .entry(location.story().clone())
            .or_insert(0usize);
        let projection_ordinal = Some(*ordinal);
        *ordinal += 1;
        let table_index = result.len();
        result.push(WordOwnerAddress {
            location: WordOwnerLocation::Table(location.clone()),
            parent: None,
            projection_ordinal,
            read_facts: table_facts.get(&location).cloned().unwrap_or_default(),
            body_slot: if snapshot.is_direct_child() {
                snapshot.direct_body_index()
            } else {
                None
            },
        });
        for (row, cell, story) in table_cells.remove(&location).unwrap_or_default() {
            result.push(WordOwnerAddress {
                location: WordOwnerLocation::Cell {
                    table: location.clone(),
                    row,
                    cell,
                    story,
                },
                parent: Some(table_index),
                body_slot: None,
                projection_ordinal: None,
                read_facts: WordReadFacts::None,
            });
        }
    }
    for (index, address) in result.iter().enumerate() {
        if let WordOwnerLocation::Cell { story, .. } = &address.location {
            cell_parents
                .entry((
                    story.kind(),
                    story.part_name().to_owned(),
                    story.owner_index(),
                ))
                .or_insert(index);
        }
    }
    // Join nested table routes through their actual physical cell owner. An
    // unresolved route stays conservative rather than acquiring an ordinal parent.
    for address in &mut result {
        if let WordOwnerLocation::Table(location) = &address.location {
            let story = location.story();
            let parent = cell_parents
                .get(&(
                    story.kind(),
                    story.part_name().to_owned(),
                    story.owner_index(),
                ))
                .copied();
            address.parent = parent;
        }
    }
    let mut paragraph_ordinals = HashMap::new();
    for (location, xml) in paragraph_snapshots {
        if let Some(sources) = form_sources.as_deref_mut() {
            let key = FormOwnerRoute::Paragraph(form_source_key(&location));
            sources
                .xml
                .entry(key)
                .and_modify(|source| *source = None)
                .or_insert_with(|| Some(xml.clone()));
        }
        let story = location.story();
        let parent = cell_parents
            .get(&(
                story.kind(),
                story.part_name().to_owned(),
                story.owner_index(),
            ))
            .copied();
        let ordinal = paragraph_ordinals
            .entry(location.story().clone())
            .or_insert(0usize);
        let projection_ordinal = Some(*ordinal);
        *ordinal += 1;
        let index = result.len();
        let paragraph = oxoffice_docx_oxml::CT_P::from_xml_fragment(&xml)?;
        // Reuse the containing item's fact from this exact immutable source
        // inventory, matching the native paragraph snapshot's containing route.
        let body_slot = if location.story().kind() == oxoffice_docx::StoryKind::Body {
            let containing = match location.index_path() {
                [_] => Some(location.clone()),
                [control, _] => Some(oxoffice_docx::ContentLocation::new(
                    location.story().clone(),
                    oxoffice_docx::StoryItemKind::ContentControl,
                    vec![*control],
                )),
                _ => None,
            };
            containing.and_then(|containing| body_slots.get(&containing).copied().flatten())
        } else {
            None
        };
        result.push(WordOwnerAddress {
            location: WordOwnerLocation::Paragraph(location.clone()),
            parent,
            body_slot,
            projection_ordinal,
            read_facts: WordReadFacts::Paragraph(paragraph.accepted_text()),
        });
        for path in paragraph.accepted_run_paths() {
            result.push(WordOwnerAddress {
                location: WordOwnerLocation::Run(location.clone(), path),
                parent: Some(index),
                body_slot: None,
                projection_ordinal: None,
                read_facts: WordReadFacts::None,
            });
        }
    }
    for index in 0..document.map_or(0, oxoffice_docx::Document::section_count) {
        if let (Some(sources), Some(document)) = (form_sources.as_deref_mut(), document) {
            sources.xml.insert(
                FormOwnerRoute::Section(index),
                Some(document.section_properties_xml(index)?),
            );
        }
        result.push(WordOwnerAddress {
            location: WordOwnerLocation::Section(index),
            parent: None,
            body_slot: None,
            projection_ordinal: None,
            read_facts: WordReadFacts::None,
        });
    }
    let mut local_indexes = HashMap::<oxoffice_docx::ContentLocation, usize>::new();
    let mut story_indexes = HashMap::<oxoffice_docx::StoryId, usize>::new();
    for (location, _) in link_snapshots {
        let local = local_indexes.entry(location.clone()).or_default();
        let position = story_indexes.entry(location.story().clone()).or_default();
        result.push(WordOwnerAddress {
            location: WordOwnerLocation::Hyperlink(location.clone(), *local),
            parent: None,
            body_slot: None,
            projection_ordinal: Some(*position),
            read_facts: WordReadFacts::None,
        });
        *local += 1;
        *position += 1;
    }
    Ok(result)
}

/// One direct body item that a splice inserts, with what the owner
/// inventory reads for it.
pub(crate) enum SplicedBodyItem {
    /// A plain paragraph's accepted text and run paths.
    Paragraph(String, Vec<oxoffice_docx_oxml::text::AcceptedRunPath>),
    /// A table's namespace-complete XML and its owner snapshots in a body
    /// that holds only that table.
    Table(Vec<u8>, WordOwnerSnapshots),
}

/// Plain direct body paragraphs removed from a body slot, or one item
/// inserted at it.
pub(crate) struct BodySplice {
    pub(crate) slot: usize,
    pub(crate) removed: usize,
    pub(crate) inserted: Option<SplicedBodyItem>,
}

/// A splice of the current owner inventory for one direct body edit.
///
/// The inserted item is one body story item, a plain paragraph or a table
/// with no nested table, text box or link, and the removed items are plain
/// paragraphs. So only body routes after the slot move, by the items inserted
/// less those removed, and only the cell stories after an inserted table,
/// by its cells. Every other owner keeps its place in the inventory.
pub(crate) struct BodySplicePlan {
    slot: usize,
    removed: usize,
    // The body story item index of the splice slot.
    first: usize,
    // The inserted owners, in blocks that each precede a current inventory
    // index, with next-inventory parents.
    blocks: Vec<(usize, Vec<WordOwnerAddress>)>,
    // The current inventory range of the removed paragraphs and their runs.
    removed_owners: std::ops::Range<usize>,
    // Cell story owners from the first index on move by the second.
    cells: (usize, usize),
    inserted_paragraph: bool,
    inserted_table: bool,
    // The facade paragraph and table indexes of the splice slot.
    pub(crate) paragraph: usize,
    pub(crate) table: usize,
    // The inserted table's route and the routes of its cell paragraphs.
    table_entry: Option<(oxoffice_docx::ContentLocation, Vec<CellParagraphs>)>,
    piece: Option<WordBlockOwners>,
    kept_scopes: usize,
}

/// The facade coordinates of a cell and the routes of its paragraphs.
type CellParagraphs = ((usize, usize), Vec<oxoffice_docx::ContentLocation>);

impl BodySplicePlan {
    fn added(&self) -> usize {
        self.blocks.iter().map(|(_, block)| block.len()).sum()
    }

    /// The body story item index that `item` moves to.
    fn moved(&self, item: usize) -> usize {
        if item >= self.first + self.removed {
            item + usize::from(self.inserted_paragraph || self.inserted_table) - self.removed
        } else {
            item
        }
    }

    /// The next inventory index of current owner `index`, unless removed.
    fn next_index(&self, index: usize) -> Option<usize> {
        if self.removed_owners.contains(&index) {
            return None;
        }
        let mut next = index;
        if index >= self.removed_owners.end {
            next -= self.removed_owners.len();
        }
        for (at, block) in &self.blocks {
            if index >= *at {
                next += block.len();
            }
        }
        Some(next)
    }

    /// The next inventory, in order: kept current owners and inserted ones.
    fn next_owners(&self, current: usize) -> impl Iterator<Item = NextWordOwner<'_>> + '_ {
        (0..=current).flat_map(move |index| {
            self.blocks
                .iter()
                .filter(move |(at, _)| *at == index)
                .flat_map(|(_, block)| block.iter().map(NextWordOwner::Inserted))
                .chain(
                    (index < current && !self.removed_owners.contains(&index))
                        .then_some(NextWordOwner::Kept(index)),
                )
        })
    }

    /// Move the cell stories of a kept owner's routes.
    fn move_cell_stories(&self, location: &mut WordOwnerLocation, part: &str) {
        let (first, count) = self.cells;
        if count == 0 {
            return;
        }
        let route = |location: &mut oxoffice_docx::ContentLocation| {
            let story = location.story();
            if story.kind() == oxoffice_docx::StoryKind::TableCell
                && story.part_name() == part
                && story.owner_index() >= first
            {
                let mut story = story.clone();
                story.set_owner_index(story.owner_index() + count);
                *location = oxoffice_docx::ContentLocation::new(
                    story,
                    location.item_kind(),
                    location.index_path().to_vec(),
                );
            }
        };
        match location {
            WordOwnerLocation::Paragraph(location)
            | WordOwnerLocation::Table(location)
            | WordOwnerLocation::Run(location, _)
            | WordOwnerLocation::Hyperlink(location, _) => route(location),
            WordOwnerLocation::Cell { table, story, .. } => {
                route(table);
                if story.kind() == oxoffice_docx::StoryKind::TableCell
                    && story.part_name() == part
                    && story.owner_index() >= first
                {
                    story.set_owner_index(story.owner_index() + count);
                }
            }
            WordOwnerLocation::Section(_) => {}
        }
    }
}

/// One owner of the next inventory.
enum NextWordOwner<'a> {
    Kept(usize),
    Inserted(&'a WordOwnerAddress),
}

/// A prepared body splice: its revision and the scope states of the next
/// inventory, in order.
pub(crate) struct PreparedBodySplice {
    pub(crate) revision: PreparedRevision,
    pub(crate) plan: BodySplicePlan,
    states: Vec<RevisionState>,
    inserted: Vec<WordOwner>,
    // The index in `states` of the first inserted owner's scopes.
    inserted_state: usize,
    snapshot_scope: RevisionState,
}

/// The body story item index of an owner's content route.
fn word_owner_body_item(location: &WordOwnerLocation) -> Option<usize> {
    let route = match location {
        WordOwnerLocation::Paragraph(location)
        | WordOwnerLocation::Table(location)
        | WordOwnerLocation::Run(location, _)
        | WordOwnerLocation::Hyperlink(location, _) => location,
        WordOwnerLocation::Cell { table, .. } => table,
        WordOwnerLocation::Section(_) => return None,
    };
    (route.story().kind() == oxoffice_docx::StoryKind::Body)
        .then(|| route.index_path().first().copied())
        .flatten()
}

/// Move the body story item index of an owner's content route.
fn move_word_owner_body_item(location: &mut WordOwnerLocation, item: usize) {
    match location {
        WordOwnerLocation::Paragraph(location)
        | WordOwnerLocation::Table(location)
        | WordOwnerLocation::Run(location, _)
        | WordOwnerLocation::Hyperlink(location, _)
        | WordOwnerLocation::Cell {
            table: location, ..
        } => location.set_index_path_head(item),
        WordOwnerLocation::Section(_) => {}
    }
}

/// Binary search over projection records: the index of the first one `pred`
/// rejects, as `slice::partition_point` finds it.
fn location_partition_point(
    items: &[Option<oxoffice_docx::ContentLocation>],
    mut pred: impl FnMut(&Option<oxoffice_docx::ContentLocation>) -> bool,
) -> usize {
    let mut left = 0;
    let mut right = items.len();
    while left < right {
        let mid = left + (right - left) / 2;
        if pred(&items[mid]) {
            left = mid + 1;
        } else {
            right = mid;
        }
    }
    left
}

/// The story of an owner's content route.
fn word_owner_story(location: &WordOwnerLocation) -> Option<&oxoffice_docx::StoryId> {
    match location {
        WordOwnerLocation::Paragraph(location)
        | WordOwnerLocation::Table(location)
        | WordOwnerLocation::Run(location, _)
        | WordOwnerLocation::Hyperlink(location, _) => Some(location.story()),
        WordOwnerLocation::Cell { table, .. } => Some(table.story()),
        WordOwnerLocation::Section(_) => None,
    }
}

impl PyDocument {
    /// Plan `splice` from the current inventory alone. `None` keeps the
    /// rebuild for every case this does not prove, and debug builds check
    /// each committed splice against the rebuild.
    pub(crate) fn plan_body_splice(
        &mut self,
        py: Python<'_>,
        splice: BodySplice,
    ) -> PyResult<Option<BodySplicePlan>> {
        self.ensure_word_authority(py)?;
        let Some(body_paragraphs) = &self.indexes.body_paragraphs else {
            return Ok(None);
        };
        let content_count = self.inner.content_count();
        let Some(end_slot) = splice.slot.checked_add(splice.removed) else {
            return Ok(None);
        };
        let inserted_table = matches!(splice.inserted, Some(SplicedBodyItem::Table(..)));
        if end_slot > content_count || (splice.removed == 0) == splice.inserted.is_none() {
            return Ok(None);
        }
        let mut tables_indexed = true;
        if inserted_table {
            for location in &self.indexes.body_tables {
                if location.is_none() {
                    tables_indexed = false;
                    break;
                }
            }
        }
        if !tables_indexed
            || !(splice.slot..end_slot).all(|slot| self.inner.plain_removable_paragraph(slot))
        {
            return Ok(None);
        }
        let mut story = None;
        let mut current_slot_item = None;
        let mut previous_slot_item = None;
        let mut kept_scopes = 0;
        let mut family = 0;
        let mut table = None;
        let mut paragraph = None;
        let mut body_item = Some(0);
        for (index, owner) in self.owners.iter().enumerate() {
            kept_scopes += owner.location.scopes().len();
            let (next, parent) = match &owner.location {
                WordOwnerLocation::Table(_) => {
                    table = Some(index);
                    (0, owner.parent)
                }
                WordOwnerLocation::Cell { .. } => (0, table),
                WordOwnerLocation::Paragraph(location) => {
                    paragraph = Some(index);
                    if location.story().kind() == oxoffice_docx::StoryKind::Body {
                        let item = location.index_path().first().copied();
                        if body_item.is_none() || item < body_item {
                            return Ok(None);
                        }
                        body_item = item;
                    } else {
                        body_item = None;
                    }
                    (1, owner.parent)
                }
                WordOwnerLocation::Run(..) => (1, paragraph),
                WordOwnerLocation::Section(_) => (2, None),
                WordOwnerLocation::Hyperlink(..) => (3, None),
            };
            let child = matches!(
                owner.location,
                WordOwnerLocation::Cell { .. } | WordOwnerLocation::Run(..)
            );
            if next < family || (child && (parent.is_none() || owner.parent != parent)) {
                return Ok(None);
            }
            family = next;
            if let WordOwnerLocation::Paragraph(location) | WordOwnerLocation::Table(location) =
                &owner.location
                && location.story().kind() == oxoffice_docx::StoryKind::Body
            {
                story.get_or_insert(location.story());
                if let (Some(slot), Some(item)) = (owner.body_slot, location.index_path().first()) {
                    if slot == splice.slot {
                        current_slot_item.get_or_insert(*item);
                    }
                    if splice.slot > 0 && slot == splice.slot - 1 {
                        previous_slot_item.get_or_insert(*item);
                    }
                }
            }
        }
        let Some(story) = story.cloned() else {
            return Ok(None);
        };
        let part = story.part_name().to_owned();
        let first = if let Some(item) = current_slot_item {
            item
        } else if splice.removed == 0 && splice.slot > 0 {
            let Some(previous) = previous_slot_item else {
                return Ok(None);
            };
            let Some(count) = self
                .inner
                .direct_body_story_item_count(splice.slot - 1)
                .ok()
                .flatten()
            else {
                return Ok(None);
            };
            previous + count
        } else {
            return Ok(None);
        };
        // Each removed slot is the direct paragraph owner at its own item.
        for offset in 0..splice.removed {
            let Some(owner) = self
                .indexes
                .direct_body_paragraphs
                .get(&(splice.slot + offset))
                .and_then(|index| self.owners.get(*index))
            else {
                return Ok(None);
            };
            match &owner.location {
                WordOwnerLocation::Paragraph(location)
                    if location.index_path() == [first + offset] => {}
                _ => return Ok(None),
            }
        }
        let Some(cells_before) = (if inserted_table {
            self.cell_stories_before(first, &part)
        } else {
            Some(0)
        }) else {
            return Ok(None);
        };
        let removed_items = first..first + splice.removed;
        let mut removed_owners = None::<std::ops::Range<usize>>;
        let mut paragraph_at = None;
        let mut table_at = None;
        let mut cells_at = None;
        let mut paragraph_ordinal = 0;
        let mut table_ordinal = 0;
        for (index, owner) in self.owners.iter().enumerate() {
            let item = word_owner_body_item(&owner.location);
            let story = word_owner_story(&owner.location);
            let body = story.is_some_and(|story| story.kind() == oxoffice_docx::StoryKind::Body);
            let (paragraph, table) = match &owner.location {
                WordOwnerLocation::Paragraph(_) => (true, false),
                WordOwnerLocation::Table(_) => (false, true),
                _ => (false, false),
            };
            let later = matches!(
                owner.location,
                WordOwnerLocation::Section(_) | WordOwnerLocation::Hyperlink(..)
            );
            if paragraph_at.is_none() && (later || (paragraph && !(body && item < Some(first)))) {
                paragraph_at = Some(index);
            }
            if table_at.is_none()
                && (later
                    || paragraph
                    || matches!(owner.location, WordOwnerLocation::Run(..))
                    || (table && !(body && item < Some(first))))
            {
                table_at = Some(index);
            }
            let later_cell_story = story.is_some_and(|story| {
                story.part_name() != part
                    || (story.kind() == oxoffice_docx::StoryKind::TableCell
                        && story.owner_index() >= cells_before)
            });
            if cells_at.is_none() && (later || (paragraph && later_cell_story)) {
                cells_at = Some(index);
            }
            if item.is_some_and(|item| removed_items.contains(&item)) {
                // Only the removed paragraphs and their runs, contiguously.
                let contiguous = removed_owners
                    .as_ref()
                    .is_none_or(|range| range.end == index);
                if !contiguous
                    || !matches!(
                        owner.location,
                        WordOwnerLocation::Paragraph(_) | WordOwnerLocation::Run(..)
                    )
                {
                    return Ok(None);
                }
                removed_owners =
                    Some(removed_owners.map_or(index..index + 1, |range| range.start..index + 1));
                continue;
            }
            if owner.parent.is_some_and(|parent| {
                removed_owners
                    .as_ref()
                    .is_some_and(|range| range.contains(&parent))
            }) {
                return Ok(None);
            }
            if body && item < Some(first) {
                paragraph_ordinal += usize::from(paragraph);
                table_ordinal += usize::from(table);
            }
        }
        if splice.removed > 0 && removed_owners.is_none() {
            return Ok(None);
        }
        let current = self.owners.len();
        // Facade paragraphs and body tables keep source order, so the splice
        // is one contiguous run of their entries.
        let head = |location: &Option<oxoffice_docx::ContentLocation>| {
            location
                .as_ref()
                .and_then(|location| location.index_path().first().copied())
        };
        let paragraph = self.inner.paragraph_count_before_content(splice.slot);
        let Some(after) = body_paragraphs.get(paragraph..) else {
            return Ok(None);
        };
        for (offset, location) in after.iter().enumerate() {
            let fits = if offset < splice.removed {
                location
                    .as_ref()
                    .map(oxoffice_docx::ContentLocation::index_path)
                    == Some(&[first + offset])
            } else {
                location.is_none() || head(location) >= Some(removed_items.end)
            };
            if !fits {
                return Ok(None);
            }
        }
        if body_paragraphs[..paragraph]
            .iter()
            .any(|location| location.is_some() && head(location) >= Some(first))
        {
            return Ok(None);
        }
        let table = location_partition_point(&self.indexes.body_tables, |location| {
            head(location) < Some(first)
        });
        for location in &self.indexes.body_tables {
            if location.as_ref().is_some_and(|location| {
                location.story().kind() == oxoffice_docx::StoryKind::Body
                    && removed_items.contains(&location.index_path()[0])
            }) {
                return Ok(None);
            }
        }
        let mut plan = BodySplicePlan {
            slot: splice.slot,
            removed: splice.removed,
            first,
            blocks: Vec::new(),
            removed_owners: removed_owners.unwrap_or(0..0),
            cells: (cells_before, 0),
            inserted_paragraph: false,
            inserted_table: false,
            paragraph,
            table,
            table_entry: None,
            piece: None,
            kept_scopes,
        };
        match splice.inserted {
            None => {}
            Some(SplicedBodyItem::Paragraph(text, runs)) => {
                plan.piece = Some(plain_paragraph_owners(&story, &text, &runs));
                let at = paragraph_at.unwrap_or(current);
                let location = oxoffice_docx::ContentLocation::new(
                    story,
                    oxoffice_docx::StoryItemKind::Paragraph,
                    vec![first],
                );
                let mut block = vec![WordOwnerAddress {
                    location: WordOwnerLocation::Paragraph(location.clone()),
                    parent: None,
                    body_slot: Some(splice.slot),
                    projection_ordinal: Some(paragraph_ordinal),
                    read_facts: WordReadFacts::Paragraph(text),
                }];
                block.extend(runs.into_iter().map(|path| WordOwnerAddress {
                    location: WordOwnerLocation::Run(location.clone(), path),
                    parent: Some(at),
                    body_slot: None,
                    projection_ordinal: None,
                    read_facts: WordReadFacts::None,
                }));
                plan.blocks.push((at, block));
                plan.inserted_paragraph = true;
            }
            Some(SplicedBodyItem::Table(xml, snapshots)) => {
                let table_at = table_at.unwrap_or(current);
                let cells_at = cells_at.unwrap_or(current);
                let Some((blocks, cells, entry, piece)) = spliced_table_blocks(
                    xml,
                    snapshots,
                    &story,
                    (splice.slot, first, table_ordinal),
                    (table_at, cells_at, cells_before),
                ) else {
                    return Ok(None);
                };
                plan.blocks = blocks;
                plan.cells = (cells_before, cells);
                plan.inserted_table = true;
                plan.table_entry = Some(entry);
                plan.piece = piece;
            }
        }
        Ok(Some(plan))
    }

    /// The number of cell stories of the main part before body story item
    /// `first`, when the inventory holds every cell story of that part and
    /// can place each one: no text box story, and every cell under a body
    /// table, so that cell story owner indexes count exactly those cells.
    fn cell_stories_before(&self, first: usize, part: &str) -> Option<usize> {
        let mut cells = Vec::new();
        let mut referenced = std::collections::HashSet::new();
        for owner in &self.owners {
            if let Some(story) = word_owner_story(&owner.location)
                && story.part_name() == part
            {
                match story.kind() {
                    oxoffice_docx::StoryKind::TextBox => return None,
                    oxoffice_docx::StoryKind::TableCell => {
                        referenced.insert(story.owner_index());
                    }
                    _ => {}
                }
            }
            let WordOwnerLocation::Cell { story, .. } = &owner.location else {
                continue;
            };
            if story.part_name() != part {
                continue;
            }
            // The body table that holds this cell, through nested tables.
            let mut table = owner.parent;
            let root = loop {
                let holder = self.owners.get(table?)?;
                match &holder.location {
                    WordOwnerLocation::Table(location)
                        if location.story().kind() == oxoffice_docx::StoryKind::Body =>
                    {
                        break location.index_path().first().copied()?;
                    }
                    WordOwnerLocation::Table(_) => {
                        let parent = holder.parent?;
                        table = self.owners.get(parent)?.parent;
                    }
                    _ => return None,
                }
            };
            cells.push((story.owner_index(), root));
        }
        cells.sort_unstable();
        let mut dense = true;
        for (index, (owner_index, _)) in cells.iter().enumerate() {
            if index != *owner_index {
                dense = false;
                break;
            }
        }
        let mut referenced_known = true;
        for owner_index in &referenced {
            if *owner_index >= cells.len() {
                referenced_known = false;
                break;
            }
        }
        let mut before = 0;
        for (_, root) in &cells {
            if *root < first {
                before += 1;
            }
        }
        let mut ordered = true;
        for (owner_index, root) in &cells {
            if (*root < first) != (*owner_index < before) {
                ordered = false;
                break;
            }
        }
        (dense && referenced_known && ordered).then_some(before)
    }

    /// Prepare a conservative or append publication of `plan`, with the
    /// retirements and births, in inventory order, that the rebuild would
    /// give, before the native edit.
    pub(crate) fn prepare_body_splice(
        &mut self,
        plan: BodySplicePlan,
        operation: &'static str,
        publication: &WordPublication,
    ) -> PyResult<PreparedBodySplice> {
        // An append keeps every owner. A conservative publication retires
        // every owner and gives the next inventory new scopes.
        let keep = match publication {
            WordPublication::AppendBody => true,
            WordPublication::Conservative => false,
            _ => return Err(PyValueError::new_err("body splice publication")),
        };
        let fail =
            |_| pyo3::exceptions::PyMemoryError::new_err("Word owner splice allocation failed");
        let current = self.owners.len();
        let scopes = |next: &NextWordOwner<'_>| match next {
            NextWordOwner::Kept(index) => self.owners[*index].location.scopes().len(),
            NextWordOwner::Inserted(address) => address.location.scopes().len(),
        };
        let kept_scopes = plan.kept_scopes;
        let mut inserted_scopes = 0;
        for (_, block) in &plan.blocks {
            for address in block {
                inserted_scopes += address.location.scopes().len();
            }
        }
        let mut births = Vec::new();
        births
            .try_reserve(inserted_scopes + if keep { 0 } else { kept_scopes } + 1)
            .map_err(fail)?;
        let mut retired = Vec::new();
        retired
            .try_reserve(if keep { 1 } else { kept_scopes + 1 })
            .map_err(fail)?;
        if !keep {
            for owner in &self.owners {
                retired.extend(owner.scope_states().cloned());
            }
        }
        for next in plan.next_owners(current) {
            match next {
                NextWordOwner::Kept(_) if keep => {}
                NextWordOwner::Kept(index) => {
                    births.extend_from_slice(self.owners[index].location.scopes());
                }
                NextWordOwner::Inserted(address) => {
                    births.extend_from_slice(address.location.scopes());
                }
            }
        }
        retired.push(
            self.snapshot_scope
                .as_ref()
                .expect("active Word authority")
                .clone(),
        );
        births.push(RevisionScope::Text);
        let revision = if keep {
            self.revisions.prepare_scopes(operation, &retired, &births)
        } else {
            self.revisions.prepare_revision(
                Some(operation),
                &retired,
                &births,
                RevisionPreparation::Replacement,
            )
        }
        .map_err(revision_to_pyerr)?;
        let mut states = Vec::new();
        states
            .try_reserve_exact(inserted_scopes + kept_scopes)
            .map_err(fail)?;
        let mut inserted = Vec::new();
        inserted.try_reserve_exact(plan.added()).map_err(fail)?;
        let mut born = 0;
        let mut inserted_state = None;
        for next in plan.next_owners(current) {
            let start = states.len();
            let count = scopes(&next);
            match next {
                NextWordOwner::Kept(index) if keep => {
                    for state in self.owners[index].scope_states() {
                        states.push(state.clone());
                    }
                    continue;
                }
                NextWordOwner::Inserted(_) => {
                    inserted_state.get_or_insert(states.len());
                }
                NextWordOwner::Kept(_) => {}
            }
            for _ in 0..count {
                states.push(revision.born(born).expect("prepared owner birth"));
                born += 1;
            }
            if let NextWordOwner::Inserted(address) = next {
                inserted.extend(word_owners_with_states(
                    vec![WordOwnerAddress {
                        location: address.location.clone(),
                        parent: address.parent,
                        body_slot: address.body_slot,
                        projection_ordinal: address.projection_ordinal,
                        read_facts: address.read_facts.clone(),
                    }],
                    &states[start..start + count],
                ));
            }
        }
        let snapshot_scope = revision.born(born).expect("prepared snapshot birth");
        // Capacity for the commit, which moves owners and indexes them again.
        let added = plan.added();
        self.owners.try_reserve(added).map_err(fail)?;
        if let Some(paragraphs) = &mut self.indexes.body_paragraphs {
            paragraphs.try_reserve(1).map_err(fail)?;
        }
        self.indexes.body_tables.try_reserve(1).map_err(fail)?;
        Ok(PreparedBodySplice {
            revision,
            plan,
            states,
            inserted,
            inserted_state: inserted_state.unwrap_or(0),
            snapshot_scope,
        })
    }

    /// Capture the first owner a prepared splice inserts, the paragraph or
    /// table it places, as [`Self::capture_prepared_owner`] captures a
    /// prepared owner.
    pub(crate) fn capture_prepared_splice_owner(
        &self,
        prepared: &PreparedBodySplice,
        segments: smallvec::SmallVec<[PathSeg; 5]>,
    ) -> PyResult<(ContentPath, RevisionState)> {
        self.revisions
            .capture_prepared(&prepared.revision, smallvec![])
            .map_err(revision_to_pyerr)?;
        let unbound = || PyValueError::new_err("element has no current checked physical owner");
        let address = prepared
            .plan
            .blocks
            .first()
            .and_then(|(_, block)| block.first())
            .filter(|address| address.parent.is_none())
            .ok_or_else(unbound)?;
        let at = prepared.inserted_state;
        let states = prepared
            .states
            .get(at..at + address.location.scopes().len())
            .ok_or_else(unbound)?;
        let owners = word_owners_with_states(
            vec![WordOwnerAddress {
                location: address.location.clone(),
                parent: None,
                body_slot: address.body_slot,
                projection_ordinal: address.projection_ordinal,
                read_facts: WordReadFacts::None,
            }],
            states,
        );
        let (mut path, owner) = word_owner_capture(
            &self.revisions,
            &owners,
            (
                prepared.revision.document_scope(),
                self.removal_scope.as_ref().expect("active Word authority"),
            ),
            0,
            segments,
            WordView::Owner,
            None,
        )?;
        path.revision = prepared.revision.committed_serial();
        Ok((path, owner))
    }

    /// Publish a prepared splice after its native edit: move the kept owners
    /// in place, place the inserted ones, and index the inventory again.
    pub(crate) fn commit_body_splice(
        &mut self,
        prepared: PreparedBodySplice,
        publication: &WordPublication,
    ) -> PyResult<()> {
        #[cfg(debug_assertions)]
        if matches!(publication, WordPublication::AppendBody) {
            self.assert_body_append_matches(&prepared.plan, publication);
        }
        #[cfg(not(debug_assertions))]
        let _ = publication;
        let PreparedBodySplice {
            revision,
            mut plan,
            states,
            inserted,
            snapshot_scope,
            ..
        } = prepared;
        self.revisions
            .commit_scopes(revision)
            .map_err(revision_to_pyerr)?;
        let mut part = String::new();
        for owner in &self.owners {
            if let Some(story) = word_owner_story(&owner.location)
                .filter(|story| story.kind() == oxoffice_docx::StoryKind::Body)
            {
                part = story.part_name().to_owned();
                break;
            }
        }
        self.owners.drain(plan.removed_owners.clone());
        let mut inserted = inserted.into_iter();
        let mut spans = Vec::with_capacity(plan.blocks.len());
        let mut placed = 0;
        for (at, block) in &plan.blocks {
            let start = *at + placed;
            self.owners
                .splice(start..start, inserted.by_ref().take(block.len()));
            spans.push(start..start + block.len());
            placed += block.len();
        }
        let after = plan.first + plan.removed;
        let paragraph_delta = isize::from(plan.inserted_paragraph) - plan.removed as isize;
        let moved_ordinal = |ordinal: usize, delta: isize| ordinal.saturating_add_signed(delta);
        // Kept owners take their next states, in inventory order, by move.
        let mut states = states.into_iter();
        for (index, owner) in self.owners.iter_mut().enumerate() {
            let mut inserted_here = false;
            for span in &spans {
                if span.contains(&index) {
                    inserted_here = true;
                    break;
                }
            }
            if inserted_here {
                for _ in 0..owner.location.scopes().len() {
                    states.next();
                }
                continue;
            }
            let item = word_owner_body_item(&owner.location);
            let body_route = matches!(
                &owner.location,
                WordOwnerLocation::Paragraph(location) | WordOwnerLocation::Table(location)
                    if location.story().kind() == oxoffice_docx::StoryKind::Body
            );
            if let Some(item) = item.filter(|item| *item >= after) {
                move_word_owner_body_item(&mut owner.location, plan.moved(item));
                if body_route {
                    let delta = match owner.location {
                        WordOwnerLocation::Paragraph(_) => paragraph_delta,
                        _ => isize::from(plan.inserted_table),
                    };
                    owner.projection_ordinal = owner
                        .projection_ordinal
                        .map(|ordinal| moved_ordinal(ordinal, delta));
                }
            }
            if body_route {
                owner.body_slot = owner.body_slot.map(|slot| {
                    if slot >= plan.slot + plan.removed {
                        slot + usize::from(plan.inserted_paragraph || plan.inserted_table)
                            - plan.removed
                    } else {
                        slot
                    }
                });
            }
            plan.move_cell_stories(&mut owner.location, &part);
            owner.parent = owner.parent.map(|parent| {
                plan.next_index(parent)
                    .expect("a kept owner's parent is kept")
            });
            let mut next = || states.next().expect("a prepared state for every scope");
            owner.owner = next();
            for state in [&mut owner.content, &mut owner.properties, &mut owner.grid]
                .into_iter()
                .flatten()
            {
                *state = next();
            }
        }
        self.indexes.index_owners_again(&self.owners);
        let moved = |location: &mut oxoffice_docx::ContentLocation| {
            if location.story().kind() == oxoffice_docx::StoryKind::Body {
                let item = location.index_path()[0];
                location.set_index_path_head(plan.moved(item));
            }
        };
        if let Some(paragraphs) = &mut self.indexes.body_paragraphs {
            paragraphs.drain(plan.paragraph..plan.paragraph + plan.removed);
            for location in paragraphs.iter_mut().flatten() {
                moved(location);
            }
            if plan.inserted_paragraph
                && let Some(WordOwnerLocation::Paragraph(location)) = plan
                    .blocks
                    .first()
                    .and_then(|(_, block)| block.first())
                    .map(|address| &address.location)
            {
                paragraphs.insert(plan.paragraph, Some(location.clone()));
            }
        }
        for location in self.indexes.body_tables.iter_mut().flatten() {
            moved(location);
        }
        if let Some((location, cells)) = &plan.table_entry {
            self.indexes
                .body_tables
                .insert(plan.table, Some(location.clone()));
            let mut shifted =
                HashMap::with_capacity(self.indexes.cell_paragraphs.len() + cells.len());
            for ((table, row, cell), mut locations) in self.indexes.cell_paragraphs.drain() {
                for location in &mut locations {
                    let mut route = WordOwnerLocation::Paragraph(location.clone());
                    plan.move_cell_stories(&mut route, &part);
                    if let WordOwnerLocation::Paragraph(route) = route {
                        *location = route;
                    }
                }
                let table = if table >= plan.table {
                    table + 1
                } else {
                    table
                };
                shifted.insert((table, row, cell), locations);
            }
            for ((row, cell), locations) in cells {
                shifted.insert((plan.table, *row, *cell), locations.clone());
            }
            self.indexes.cell_paragraphs = shifted;
        }
        self.snapshot_scope = Some(snapshot_scope);
        let inserted = plan.piece.take();
        self.pieces.as_mut().expect("active pieces").splice(
            &self.inner,
            plan.slot,
            plan.removed,
            inserted,
        );
        #[cfg(debug_assertions)]
        self.assert_word_inventory_rebuilds(true);
        Ok(())
    }

    /// Under debug assertions, prove that an append keeps exactly the owners
    /// the rebuild's matcher keeps. Runs after the native edit.
    #[cfg(debug_assertions)]
    fn assert_body_append_matches(&self, plan: &BodySplicePlan, publication: &WordPublication) {
        let rebuilt =
            word_owner_locations(&self.inner, None).expect("a spliced inventory has a rebuild");
        let mut expected = vec![None; self.owners.len() + plan.added()];
        for index in 0..self.owners.len() {
            if let Some(next) = plan.next_index(index) {
                expected[next] = Some(index);
            }
        }
        assert_eq!(
            indexed_inventory_matches(&self.owners, &rebuilt, publication),
            expected,
            "spliced Word owner matches"
        );
    }

    /// Under debug assertions, prove the inventory and its document indexes
    /// equal their rebuild from the current document, story fingerprints
    /// aside, which local edits leave stale in the inventory too. Unless
    /// `in_order`, the owners may come in any order, each known by its route
    /// and its parent's: a local cell edit appends the owners it adds.
    #[cfg(debug_assertions)]
    pub(crate) fn assert_word_inventory_rebuilds(&self, in_order: bool) {
        let address = |location: &WordOwnerLocation,
                       parent: Option<usize>,
                       body_slot: Option<usize>,
                       projection_ordinal: Option<usize>,
                       read_facts: &WordReadFacts| WordOwnerAddress {
            location: unfingerprinted_owner_location(location),
            parent,
            body_slot,
            projection_ordinal,
            read_facts: read_facts.clone(),
        };
        let actual = self
            .owners
            .iter()
            .map(|owner| {
                address(
                    &owner.location,
                    owner.parent,
                    owner.body_slot,
                    owner.projection_ordinal,
                    &owner.read_facts,
                )
            })
            .collect::<Vec<_>>();
        let expected = word_owner_locations(&self.inner, None)
            .expect("a spliced inventory has a rebuild")
            .iter()
            .map(|owner| {
                address(
                    &owner.location,
                    owner.parent,
                    owner.body_slot,
                    owner.projection_ordinal,
                    &owner.read_facts,
                )
            })
            .collect::<Vec<_>>();
        if in_order {
            assert_eq!(actual, expected, "spliced Word owner inventory");
        } else {
            let records = |addresses: &[WordOwnerAddress]| {
                let mut records = addresses
                    .iter()
                    .map(|address| {
                        format!(
                            "{:?}",
                            (
                                &address.location,
                                address.parent.map(|parent| &addresses[parent].location),
                                address.body_slot,
                                address.projection_ordinal,
                                &address.read_facts,
                            )
                        )
                    })
                    .collect::<Vec<_>>();
                records.sort();
                records
            };
            assert_eq!(records(&actual), records(&expected), "Word owner inventory");
        }
        let indexes =
            WordOwnerIndexes::prepare(&self.owners, WordIndexProjections::of(&self.inner))
                .expect("a spliced inventory has rebuilt indexes");
        let locations = |locations: &[Option<oxoffice_docx::ContentLocation>]| {
            locations
                .iter()
                .map(|location| location.as_ref().map(unfingerprinted_location))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            self.indexes.body_paragraphs.as_deref().map(locations),
            indexes.body_paragraphs.as_deref().map(locations),
            "spliced body paragraph index"
        );
        assert_eq!(
            locations(&self.indexes.body_tables),
            locations(&indexes.body_tables),
            "spliced body table index"
        );
        let cells =
            |cells: &HashMap<(usize, usize, usize), Vec<oxoffice_docx::ContentLocation>>| {
                cells
                    .iter()
                    .map(|(key, locations)| {
                        (
                            *key,
                            locations
                                .iter()
                                .map(unfingerprinted_location)
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect::<BTreeMap<_, _>>()
            };
        assert_eq!(
            cells(&self.indexes.cell_paragraphs),
            cells(&indexes.cell_paragraphs),
            "spliced cell paragraph index"
        );
        assert_eq!(
            self.indexes.direct_body_paragraphs, indexes.direct_body_paragraphs,
            "spliced direct body paragraph index"
        );
    }
}

/// The inserted blocks of a detached table placed at body story item
/// `first` of slot `slot`: the table and its cells before current owner
/// `table_at`, its cell paragraphs and runs before `cells_at`, with cell
/// story owners counted from `cells_before`. Also the number of cells, and
/// the table route with the routes of each cell's paragraphs.
#[allow(clippy::type_complexity)] // The blocks, the cell count and the index entry of one table.
fn spliced_table_blocks(
    xml: Vec<u8>,
    snapshots: WordOwnerSnapshots,
    story: &oxoffice_docx::StoryId,
    (slot, first, ordinal): (usize, usize, usize),
    (table_at, cells_at, cells_before): (usize, usize, usize),
) -> Option<(
    Vec<(usize, Vec<WordOwnerAddress>)>,
    usize,
    (oxoffice_docx::ContentLocation, Vec<CellParagraphs>),
    Option<WordBlockOwners>,
)> {
    // The detached body holds this table as its only item.
    let table_location = {
        let mut items = snapshots
            .0
            .iter()
            .filter(|item| item.location().story().kind() == oxoffice_docx::StoryKind::Body);
        let item = items.next()?;
        if items.next().is_some()
            || item.location().item_kind() != oxoffice_docx::StoryItemKind::Table
            || item.location().index_path() != [0]
        {
            return None;
        }
        item.location().clone()
    };
    let piece_facts = word_table_facts(&xml).and_then(|rows| {
        let mut facts = WordOwnerFacts {
            cells: snapshots.1.clone(),
            ..WordOwnerFacts::default()
        };
        facts.items.push((table_location.clone(), Some(0), rows));
        for (location, paragraph_xml) in &snapshots.2 {
            let paragraph = oxoffice_docx_oxml::CT_P::from_xml_fragment(paragraph_xml).ok()?;
            facts.paragraphs.push((
                location.clone(),
                paragraph.accepted_text(),
                paragraph.accepted_run_paths(),
            ));
        }
        Some(facts)
    });
    let facts = word_table_facts(&xml)
        .map(|facts| (table_location, facts))
        .into_iter()
        .collect::<HashMap<_, _>>();
    let addresses = word_owner_addresses(snapshots, &facts, None, None).ok()?;
    let cells = addresses
        .iter()
        .skip(1)
        .take_while(|address| matches!(address.location, WordOwnerLocation::Cell { .. }))
        .count();
    // The table, its cells in story order, then only their paragraphs and runs.
    let cell_story = |address: &WordOwnerAddress| match &address.location {
        WordOwnerLocation::Cell { story, .. } => Some(story.owner_index()),
        _ => None,
    };
    let in_cells = |location: &oxoffice_docx::ContentLocation| {
        location.story().kind() == oxoffice_docx::StoryKind::TableCell
            && location.story().owner_index() < cells
    };
    let cell_parent =
        |parent: Option<usize>| parent.is_some_and(|parent| (1..=cells).contains(&parent));
    let shaped = matches!(addresses.first()?.location, WordOwnerLocation::Table(_))
        && addresses[1..=cells]
            .iter()
            .enumerate()
            .all(|(index, address)| {
                cell_story(address) == Some(index) && address.parent == Some(0)
            })
        && addresses[cells + 1..]
            .iter()
            .all(|address| match &address.location {
                WordOwnerLocation::Paragraph(location) => {
                    in_cells(location) && cell_parent(address.parent)
                }
                WordOwnerLocation::Run(location, _) => {
                    in_cells(location) && address.parent.is_some_and(|parent| parent > cells)
                }
                _ => false,
            });
    if !shaped {
        return None;
    }
    // Detached owner `index` takes this next-inventory index.
    let next = |index: usize| {
        if index <= cells {
            table_at + index
        } else {
            cells_at + index
        }
    };
    let moved = |location: &oxoffice_docx::ContentLocation| {
        let mut story = location.story().clone();
        story.set_owner_index(story.owner_index() + cells_before);
        oxoffice_docx::ContentLocation::new(
            story,
            location.item_kind(),
            location.index_path().to_vec(),
        )
    };
    let table = oxoffice_docx::ContentLocation::new(
        story.clone(),
        oxoffice_docx::StoryItemKind::Table,
        vec![first],
    );
    let mut entries = Vec::with_capacity(cells);
    let mut head = Vec::with_capacity(cells + 1);
    let mut tail = Vec::with_capacity(addresses.len() - cells - 1);
    for (index, address) in addresses.into_iter().enumerate() {
        let parent = address.parent.map(next);
        let location = match &address.location {
            WordOwnerLocation::Table(_) => WordOwnerLocation::Table(table.clone()),
            WordOwnerLocation::Cell {
                row, cell, story, ..
            } => {
                let mut story = story.clone();
                story.set_owner_index(story.owner_index() + cells_before);
                entries.push(((*row, *cell), Vec::new()));
                WordOwnerLocation::Cell {
                    table: table.clone(),
                    row: *row,
                    cell: *cell,
                    story,
                }
            }
            WordOwnerLocation::Paragraph(location) => {
                let location = moved(location);
                entries[address.parent? - 1].1.push(location.clone());
                WordOwnerLocation::Paragraph(location)
            }
            WordOwnerLocation::Run(location, path) => {
                WordOwnerLocation::Run(moved(location), path.clone())
            }
            _ => return None,
        };
        let address = if index == 0 {
            WordOwnerAddress {
                location,
                parent: None,
                body_slot: Some(slot),
                projection_ordinal: Some(ordinal),
                read_facts: address.read_facts,
            }
        } else {
            WordOwnerAddress {
                location,
                parent,
                ..address
            }
        };
        if index <= cells {
            head.push(address);
        } else {
            tail.push(address);
        }
    }
    let mut blocks = vec![(table_at, head)];
    if !tail.is_empty() {
        blocks.push((cells_at, tail));
    }
    let piece = piece_facts.and_then(|facts| table_owners(facts, cells));
    Some((blocks, cells, (table, entries), piece))
}

#[derive(Clone)]
pub(crate) enum WordPublication {
    Conservative,
    FormValues,
    /// Metadata and form publication that may keep comment and text-box
    /// owners when their physical part is proved unchanged.
    ModernFormValues,
    Signing,
    AppendBody,
    HeaderFooterAppend(usize),
    ParagraphText(oxoffice_docx::ContentLocation),
    ParagraphAppend(oxoffice_docx::ContentLocation),
    TableGrid(oxoffice_docx::ContentLocation),
    HyperlinkRemove(WordOwnerLocation),
    CellText(oxoffice_docx::ContentLocation, usize, usize),
    ElementXml(WordOwnerLocation),
    RunText(WordOwnerLocation),
}

fn form_owner_certificates(
    old: &[WordOwner],
    new: &[WordOwnerAddress],
    matched: &[Option<usize>],
    before: &FormSources,
    after: &FormSources,
    eligible_parts: &std::collections::HashSet<String>,
) -> std::collections::HashSet<(usize, usize)> {
    let mut old_routes = HashMap::<FormOwnerRoute, Vec<usize>>::new();
    let mut new_routes = HashMap::<FormOwnerRoute, Vec<usize>>::new();
    for (index, owner) in old.iter().enumerate() {
        if let Some(route) = form_owner_route(&owner.location) {
            old_routes.entry(route).or_default().push(index);
        }
    }
    for (index, owner) in new.iter().enumerate() {
        if let Some(route) = form_owner_route(&owner.location) {
            new_routes.entry(route).or_default().push(index);
        }
    }
    let mut proved = std::collections::HashSet::new();
    let mut ready = vec![false; new.len()];
    for target in 0..new.len() {
        let mut stack = vec![(target, false)];
        while let Some((index, parent_ready)) = stack.pop() {
            if ready[index] {
                continue;
            }
            if !parent_ready && let Some(parent) = new[index].parent {
                stack.push((index, true));
                if !ready[parent] {
                    stack.push((parent, false));
                }
                continue;
            }
            ready[index] = true;
            let Some(route) = form_owner_route(&new[index].location) else {
                continue;
            };
            let Some(indices) = old_routes.get(&route) else {
                continue;
            };
            let [previous] = indices.as_slice() else {
                continue;
            };
            if new_routes.get(&route).map(Vec::as_slice) != Some(&[index])
                || matched[index] != Some(*previous)
            {
                continue;
            }
            let source_proved = match &route {
                FormOwnerRoute::Cell(_, _, _, story) => {
                    before.cells.get(&route) == Some(&1)
                        && after.cells.get(&route) == Some(&1)
                        && before.cell_stories.get(story) == Some(&1)
                        && after.cell_stories.get(story) == Some(&1)
                }
                FormOwnerRoute::Paragraph(key) | FormOwnerRoute::Table(key)
                    if matches!(
                        key.0,
                        oxoffice_docx::StoryKind::Comment | oxoffice_docx::StoryKind::TextBox
                    ) =>
                {
                    eligible_parts.contains(&key.1)
                        && matches!(
                            (before.xml.get(&route), after.xml.get(&route)),
                            (Some(Some(a)), Some(Some(b))) if a == b
                        )
                }
                _ => {
                    matches!((before.xml.get(&route), after.xml.get(&route)), (Some(Some(a)), Some(Some(b))) if a == b)
                }
            };
            if !source_proved {
                continue;
            }
            let ancestors_proved = match (old[*previous].parent, new[index].parent) {
                (Some(a), Some(b)) => proved.contains(&(a, b)),
                (None, None) => !matches!(
                    &route,
                    FormOwnerRoute::Cell(..)
                        | FormOwnerRoute::Paragraph((oxoffice_docx::StoryKind::TableCell, ..))
                        | FormOwnerRoute::Table((oxoffice_docx::StoryKind::TableCell, ..))
                ),
                _ => false,
            };
            if ancestors_proved {
                proved.insert((*previous, index));
            }
        }
    }
    proved
}

// Signing changes package infrastructure. Preserve content authority only when
// the physical source and every required ancestor survived serialization unchanged.
fn signing_owner_certificates(
    old: &[WordOwner],
    new: &[WordOwnerAddress],
    matched: &[Option<usize>],
    before: &FormSources,
    after: &FormSources,
) -> std::collections::HashSet<(usize, usize)> {
    let mut proved = form_owner_certificates(
        old,
        new,
        matched,
        before,
        after,
        &std::collections::HashSet::new(),
    );
    let mut paragraphs = HashMap::<FormSourceKey, Vec<(usize, usize)>>::new();
    for &(previous, next) in &proved {
        if let WordOwnerLocation::Paragraph(location) = &new[next].location {
            paragraphs
                .entry(form_source_key(location))
                .or_default()
                .push((previous, next));
        }
    }
    for (next, address) in new.iter().enumerate() {
        let Some(previous) = matched[next] else {
            continue;
        };
        let survives = match (&old[previous].location, &address.location) {
            (WordOwnerLocation::Run(a, left), WordOwnerLocation::Run(b, right)) => {
                left == right
                    && form_source_key(a) == form_source_key(b)
                    && matches!((old[previous].parent, address.parent), (Some(a), Some(b)) if proved.contains(&(a, b)))
            }
            (WordOwnerLocation::Hyperlink(a, left), WordOwnerLocation::Hyperlink(b, right)) => {
                left == right
                    && form_source_key(a) == form_source_key(b)
                    && paragraphs
                        .get(&form_source_key(b))
                        .is_some_and(|pairs| pairs.len() == 1)
            }
            _ => false,
        };
        if survives {
            proved.insert((previous, next));
        }
    }
    proved
}

/// Comment and text-box certificates still need the ordinary owner proof.
/// A run survives only when that proof already covers its paragraph.
fn modern_form_owner_certificates(
    old: &[WordOwner],
    new: &[WordOwnerAddress],
    matched: &[Option<usize>],
    before: &FormSources,
    after: &FormSources,
    eligible_parts: &std::collections::HashSet<String>,
) -> std::collections::HashSet<(usize, usize)> {
    let mut proved = form_owner_certificates(old, new, matched, before, after, eligible_parts);
    for (next, address) in new.iter().enumerate() {
        let Some(previous) = matched[next] else {
            continue;
        };
        let survives = match (&old[previous].location, &address.location) {
            (WordOwnerLocation::Run(a, left), WordOwnerLocation::Run(b, right)) => {
                left == right
                    && form_source_key(a) == form_source_key(b)
                    && matches!(
                        (old[previous].parent, address.parent),
                        (Some(a), Some(b)) if proved.contains(&(a, b))
                    )
            }
            _ => false,
        };
        if survives {
            proved.insert((previous, next));
        }
    }
    proved
}

fn related_owner_part(location: &WordOwnerLocation) -> Option<String> {
    let content = match location {
        WordOwnerLocation::Paragraph(location) | WordOwnerLocation::Table(location) => location,
        _ => return None,
    };
    if !matches!(
        content.story().kind(),
        oxoffice_docx::StoryKind::Comment | oxoffice_docx::StoryKind::TextBox
    ) {
        return None;
    }
    Some(content.story().part_name().to_owned())
}

/// One inventory name, or none when case-insensitive lookup is missing or repeated.
fn unique_part_name<'a>(names: &'a [String], route: &str) -> Option<&'a str> {
    let mut found = names.iter().filter(|name| name.eq_ignore_ascii_case(route));
    match (found.next(), found.next()) {
        (Some(name), None) => Some(name.as_str()),
        _ => None,
    }
}

/// Unchanged comment and text-box parts, plus whether content snapshots match.
///
/// `part_xml` is the save-equivalent XML text. A UTF-8 BOM is stripped, so
/// this is not a ZIP byte proof. `part_names`, `stories` and `part_xml` each
/// stage the package. One part is proved once. That cost is whole-package
/// validation, not a constant-time claim. A helper error is not equality.
fn unchanged_related_owner_parts(
    old: &oxoffice_docx::Document,
    new: &oxoffice_docx::Document,
    old_owners: &[WordOwner],
    new_addresses: &[WordOwnerAddress],
) -> oxoffice_docx::Result<(std::collections::HashSet<String>, bool)> {
    let mut requested = Vec::new();
    for part in old_owners
        .iter()
        .filter_map(|owner| related_owner_part(&owner.location))
        .chain(
            new_addresses
                .iter()
                .filter_map(|address| related_owner_part(&address.location)),
        )
    {
        if !requested.iter().any(|seen: &String| seen == &part) {
            requested.push(part);
        }
    }
    let old_names = old.part_names()?;
    let new_names = new.part_names()?;
    let old_stories = old.stories()?;
    let new_stories = new.stories()?;
    let mut eligible = std::collections::HashSet::new();
    for part in &requested {
        let distinct = requested
            .iter()
            .filter(|candidate| candidate.eq_ignore_ascii_case(part))
            .count();
        if distinct != 1 {
            continue;
        }
        let Some(actual_old) = unique_part_name(&old_names, part) else {
            continue;
        };
        let Some(actual_new) = unique_part_name(&new_names, part) else {
            continue;
        };
        if actual_old != actual_new || actual_old != part {
            continue;
        }
        let old_part = old_stories
            .iter()
            .filter(|story| story.part_name() == actual_old)
            .cloned()
            .collect::<Vec<_>>();
        let new_part = new_stories
            .iter()
            .filter(|story| story.part_name() == actual_new)
            .cloned()
            .collect::<Vec<_>>();
        if old_part != new_part {
            continue;
        }
        match (old.part_xml(actual_old), new.part_xml(actual_new)) {
            (Ok(before), Ok(after)) if before == after => {
                eligible.insert(actual_old.to_owned());
            }
            (Ok(_), Ok(_)) => {}
            (Err(_), _) | (_, Err(_)) => {}
        }
    }
    let preserve =
        old_stories == new_stories && old.story_item_snapshots()? == new.story_item_snapshots()?;
    Ok((eligible, preserve))
}

fn same_inventory_route(
    old: &[WordOwner],
    mut before: usize,
    new: &[WordOwnerAddress],
    mut after: usize,
) -> bool {
    loop {
        let record = &old[before];
        let address = &new[after];
        let b = &address.location;
        let parent = address.parent;
        let equal = match (&record.location, b) {
            (WordOwnerLocation::Paragraph(a), WordOwnerLocation::Paragraph(b))
                if a.story().kind() == oxoffice_docx::StoryKind::Body
                    && b.story().kind() == oxoffice_docx::StoryKind::Body
                    && record.body_slot.is_some()
                    && address.body_slot.is_some() =>
            {
                record.body_slot == address.body_slot
                    && a.index_path().len() <= 2
                    && b.index_path().len() <= 2
                    && a.index_path().get(1..) == b.index_path().get(1..)
            }
            (WordOwnerLocation::Table(a), WordOwnerLocation::Table(b))
                if a.story().kind() == oxoffice_docx::StoryKind::Body
                    && b.story().kind() == oxoffice_docx::StoryKind::Body
                    && record.body_slot.is_some()
                    && address.body_slot.is_some() =>
            {
                record.body_slot == address.body_slot
            }
            (WordOwnerLocation::Paragraph(a), WordOwnerLocation::Paragraph(b))
            | (WordOwnerLocation::Table(a), WordOwnerLocation::Table(b))
                if record.parent.is_some_and(|index| {
                    matches!(old[index].location, WordOwnerLocation::Cell { .. })
                }) && parent.is_some_and(|index| {
                    matches!(new[index].location, WordOwnerLocation::Cell { .. })
                }) =>
            {
                a.item_kind() == b.item_kind()
                    && record.projection_ordinal == address.projection_ordinal
            }
            (WordOwnerLocation::Paragraph(a), WordOwnerLocation::Paragraph(b))
            | (WordOwnerLocation::Table(a), WordOwnerLocation::Table(b)) => {
                same_story_owner(a.story(), b.story())
                    && a.item_kind() == b.item_kind()
                    && record.projection_ordinal == address.projection_ordinal
            }
            (
                WordOwnerLocation::Cell {
                    row: a, cell: ac, ..
                },
                WordOwnerLocation::Cell {
                    row: b, cell: bc, ..
                },
            ) => a == b && ac == bc,
            (WordOwnerLocation::Run(_, a), WordOwnerLocation::Run(_, b)) => a == b,
            (a, b) => a.same_route(b),
        };
        if !equal {
            return false;
        }
        match (record.parent, parent) {
            (Some(a), Some(b)) => {
                before = a;
                after = b;
            }
            (None, None) => return true,
            _ => return false,
        }
    }
}

// The legacy relation is pairwise, not transitive. Index both slot and
// projection routes, partitioning the old operands before selecting a branch.
#[derive(Clone, PartialEq, Eq, Hash)]
enum WordRouteKey {
    Slot(oxoffice_docx::StoryItemKind, usize, Vec<usize>),
    Projection(
        oxoffice_docx::StoryKind,
        String,
        usize,
        oxoffice_docx::StoryItemKind,
        Option<usize>,
    ),
    CellProjection(oxoffice_docx::StoryItemKind, Option<usize>),
    Cell(usize, usize),
    Run(oxoffice_docx_oxml::text::AcceptedRunPath),
    Section(usize),
    Hyperlink(
        oxoffice_docx::StoryKind,
        String,
        usize,
        oxoffice_docx::StoryItemKind,
        Vec<usize>,
        usize,
    ),
}

fn word_route_key(
    address: &WordOwnerAddress,
    class: usize,
    parent_cell: bool,
) -> Option<WordRouteKey> {
    match &address.location {
        WordOwnerLocation::Paragraph(location) | WordOwnerLocation::Table(location) => {
            let body = location.story().kind() == oxoffice_docx::StoryKind::Body;
            if body
                && class & 1 != 0
                && let Some(slot) = address.body_slot
            {
                let suffix = if matches!(address.location, WordOwnerLocation::Paragraph(_)) {
                    if location.index_path().len() > 2 {
                        return None;
                    }
                    location.index_path().get(1..).unwrap_or_default().to_vec()
                } else {
                    Vec::new()
                };
                let kind = if matches!(address.location, WordOwnerLocation::Paragraph(_)) {
                    oxoffice_docx::StoryItemKind::Paragraph
                } else {
                    oxoffice_docx::StoryItemKind::Table
                };
                Some(WordRouteKey::Slot(kind, slot, suffix))
            } else if parent_cell && class & 2 != 0 {
                Some(WordRouteKey::CellProjection(
                    location.item_kind(),
                    address.projection_ordinal,
                ))
            } else {
                let story = location.story();
                Some(WordRouteKey::Projection(
                    story.kind(),
                    story.part_name().to_owned(),
                    story.owner_index(),
                    location.item_kind(),
                    address.projection_ordinal,
                ))
            }
        }
        WordOwnerLocation::Cell { row, cell, .. } => Some(WordRouteKey::Cell(*row, *cell)),
        WordOwnerLocation::Run(_, path) => Some(WordRouteKey::Run(path.clone())),
        WordOwnerLocation::Section(index) => Some(WordRouteKey::Section(*index)),
        WordOwnerLocation::Hyperlink(location, index) => {
            let story = location.story();
            Some(WordRouteKey::Hyperlink(
                story.kind(),
                story.part_name().to_owned(),
                story.owner_index(),
                location.item_kind(),
                location.index_path().to_vec(),
                *index,
            ))
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum PropertyOwnerKey {
    Table {
        story_kind: oxoffice_docx::StoryKind,
        part_name: String,
        path: Vec<usize>,
        cell: Option<(usize, usize)>,
    },
    Cell {
        story_kind: oxoffice_docx::StoryKind,
        part_name: String,
        path: Vec<usize>,
        row: usize,
        column: usize,
        parent_cell: Option<(usize, usize)>,
    },
    InCell {
        story_kind: oxoffice_docx::StoryKind,
        part_name: String,
        depth: usize,
        cell: (usize, usize),
        run: Option<oxoffice_docx_oxml::text::AcceptedRunPath>,
    },
    BodyItem {
        story_kind: oxoffice_docx::StoryKind,
        part_name: String,
        kind: oxoffice_docx::StoryItemKind,
        path: Vec<usize>,
    },
    BodyRun {
        story_kind: oxoffice_docx::StoryKind,
        part_name: String,
        path: Vec<usize>,
        run: oxoffice_docx_oxml::text::AcceptedRunPath,
    },
    Section(usize),
    Hyperlink {
        story_kind: oxoffice_docx::StoryKind,
        part_name: String,
        path: Vec<usize>,
        index: usize,
    },
}

fn containing_cell(
    addresses: &[WordOwnerAddress],
    mut parent: Option<usize>,
) -> Option<(usize, usize)> {
    while let Some(index) = parent {
        if let WordOwnerLocation::Cell { row, cell, .. } = &addresses[index].location {
            return Some((*row, *cell));
        }
        parent = addresses[index].parent;
    }
    None
}

fn story_part(location: &oxoffice_docx::ContentLocation) -> (oxoffice_docx::StoryKind, String) {
    let story = location.story();
    (story.kind(), story.part_name().to_owned())
}

fn property_owner_key(addresses: &[WordOwnerAddress], index: usize) -> PropertyOwnerKey {
    let address = &addresses[index];
    let cell = containing_cell(addresses, address.parent);
    match &address.location {
        WordOwnerLocation::Table(location) => {
            let (story_kind, part_name) = story_part(location);
            PropertyOwnerKey::Table {
                story_kind,
                part_name,
                path: location.index_path().to_vec(),
                cell,
            }
        }
        WordOwnerLocation::Cell {
            table,
            row,
            cell: column,
            ..
        } => {
            let (story_kind, part_name) = story_part(table);
            PropertyOwnerKey::Cell {
                story_kind,
                part_name,
                path: table.index_path().to_vec(),
                row: *row,
                column: *column,
                parent_cell: cell,
            }
        }
        WordOwnerLocation::Paragraph(location) => {
            let (story_kind, part_name) = story_part(location);
            if let Some(cell) = cell {
                PropertyOwnerKey::InCell {
                    story_kind,
                    part_name,
                    depth: location.index_path().len(),
                    cell,
                    run: None,
                }
            } else {
                PropertyOwnerKey::BodyItem {
                    story_kind,
                    part_name,
                    kind: location.item_kind(),
                    path: location.index_path().to_vec(),
                }
            }
        }
        WordOwnerLocation::Run(location, path) => {
            let (story_kind, part_name) = story_part(location);
            if let Some(cell) = cell {
                PropertyOwnerKey::InCell {
                    story_kind,
                    part_name,
                    depth: location.index_path().len(),
                    cell,
                    run: Some(path.clone()),
                }
            } else {
                PropertyOwnerKey::BodyRun {
                    story_kind,
                    part_name,
                    path: location.index_path().to_vec(),
                    run: path.clone(),
                }
            }
        }
        WordOwnerLocation::Section(index) => PropertyOwnerKey::Section(*index),
        WordOwnerLocation::Hyperlink(location, index) => {
            let (story_kind, part_name) = story_part(location);
            PropertyOwnerKey::Hyperlink {
                story_kind,
                part_name,
                path: location.index_path().to_vec(),
                index: *index,
            }
        }
    }
}

fn relocated_property_owners(
    old: &[WordOwner],
    new_addresses: &[WordOwnerAddress],
) -> Option<Vec<WordOwner>> {
    if old.len() != new_addresses.len() {
        return None;
    }
    let old_addresses = old
        .iter()
        .map(|owner| WordOwnerAddress {
            location: owner.location.clone(),
            parent: owner.parent,
            body_slot: owner.body_slot,
            projection_ordinal: owner.projection_ordinal,
            read_facts: owner.read_facts.clone(),
        })
        .collect::<Vec<_>>();
    let mut buckets: HashMap<PropertyOwnerKey, Vec<usize>> = HashMap::new();
    for index in 0..old_addresses.len() {
        buckets
            .entry(property_owner_key(&old_addresses, index))
            .or_default()
            .push(index);
    }
    let mut cursors: HashMap<PropertyOwnerKey, usize> = HashMap::new();
    let mut owners = Vec::with_capacity(new_addresses.len());
    for index in 0..new_addresses.len() {
        let key = property_owner_key(new_addresses, index);
        let bucket = buckets.get(&key)?;
        let cursor = cursors.entry(key).or_insert(0);
        if *cursor >= bucket.len() {
            return None;
        }
        let previous = &old[bucket[*cursor]];
        *cursor += 1;
        let address = &new_addresses[index];
        owners.push(WordOwner {
            location: address.location.clone(),
            parent: address.parent,
            body_slot: address.body_slot,
            projection_ordinal: address.projection_ordinal,
            read_facts: address.read_facts.clone(),
            owner: previous.owner.clone(),
            content: previous.content.clone(),
            properties: previous.properties.clone(),
            grid: previous.grid.clone(),
        });
    }
    if cursors.len() != buckets.len()
        || cursors
            .iter()
            .any(|(key, cursor)| buckets.get(key).map(Vec::len) != Some(*cursor))
    {
        return None;
    }
    Some(owners)
}

fn indexed_inventory_matches(
    old: &[WordOwner],
    new: &[WordOwnerAddress],
    publication: &WordPublication,
) -> Vec<Option<usize>> {
    let mut index = HashMap::<(Option<usize>, usize, WordRouteKey), Vec<usize>>::new();
    let mut links = HashMap::<WordRouteKey, Vec<usize>>::new();
    for (before, owner) in old.iter().enumerate() {
        let address = WordOwnerAddress {
            location: owner.location.clone(),
            parent: owner.parent,
            body_slot: owner.body_slot,
            projection_ordinal: owner.projection_ordinal,
            read_facts: owner.read_facts.clone(),
        };
        let parent_cell = owner
            .parent
            .is_some_and(|parent| matches!(old[parent].location, WordOwnerLocation::Cell { .. }));
        let body_slot = match &owner.location {
            WordOwnerLocation::Paragraph(location) | WordOwnerLocation::Table(location) => {
                location.story().kind() == oxoffice_docx::StoryKind::Body
                    && owner.body_slot.is_some()
            }
            _ => false,
        };
        let class = usize::from(body_slot) | (usize::from(parent_cell) << 1);
        // The candidate determines which pairwise branch applies. Supply each
        // branch's old key in its own namespace, without merging parent routes.
        let mut keys = Vec::new();
        for candidate_class in 0..4 {
            if let Some(key) = word_route_key(&address, candidate_class, candidate_class & 2 != 0)
                && !keys.contains(&key)
            {
                keys.push(key);
            }
        }
        for key in keys {
            index
                .entry((owner.parent, class, key.clone()))
                .or_default()
                .push(before);
            if matches!(owner.location, WordOwnerLocation::Hyperlink(..)) {
                links.entry(key).or_default().push(before);
            }
        }
    }
    let mut matches = vec![None::<Vec<usize>>; new.len()];
    for target in 0..new.len() {
        let mut stack = vec![(target, false)];
        while let Some((after, ready)) = stack.pop() {
            if matches[after].is_some() {
                continue;
            }
            if !ready && let Some(parent) = new[after].parent {
                stack.push((after, true));
                if matches[parent].is_none() {
                    stack.push((parent, false));
                }
                continue;
            }
            let address = &new[after];
            let mut found = Vec::new();
            // Removal's original override uses the source content route alone,
            // independently of the normal parent-chain matcher.
            let removed_link = if let (
                WordPublication::HyperlinkRemove(WordOwnerLocation::Hyperlink(
                    removed,
                    removed_index,
                )),
                WordOwnerLocation::Hyperlink(location, position),
            ) = (publication, &address.location)
            {
                if same_content_owner(removed, location) {
                    let previous = if position >= removed_index {
                        position.checked_add(1)
                    } else {
                        Some(*position)
                    };
                    if let Some(previous) = previous {
                        let shifted = WordOwnerAddress {
                            location: WordOwnerLocation::Hyperlink(location.clone(), previous),
                            parent: None,
                            body_slot: None,
                            projection_ordinal: None,
                            read_facts: WordReadFacts::None,
                        };
                        if let Some(key) = word_route_key(&shifted, 0, false) {
                            found.extend(links.get(&key).into_iter().flatten().copied());
                        }
                    }
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if !removed_link {
                let parents = address.parent.map_or_else(
                    || vec![None],
                    |parent| {
                        matches[parent]
                            .as_ref()
                            .unwrap()
                            .iter()
                            .copied()
                            .map(Some)
                            .collect()
                    },
                );
                let parent_cell = address.parent.is_some_and(|parent| {
                    matches!(new[parent].location, WordOwnerLocation::Cell { .. })
                });
                for parent in parents {
                    for class in 0..4 {
                        if let Some(key) = word_route_key(address, class, parent_cell)
                            && let Some(candidates) = index.get(&(parent, class, key))
                        {
                            for &before in candidates {
                                if same_inventory_route(old, before, new, after) {
                                    found.push(before);
                                }
                            }
                        }
                    }
                }
            }
            found.sort_unstable();
            found.dedup();
            matches[after] = Some(found);
        }
    }
    matches
        .into_iter()
        .map(|matched| matched.unwrap().into_iter().next())
        .collect()
}

impl WordOwner {
    fn states(&self) -> Vec<RevisionState> {
        self.scope_states().cloned().collect()
    }

    /// The owner's scope states, in [`WordOwnerLocation::scopes`] order.
    fn scope_states(&self) -> impl Iterator<Item = &RevisionState> {
        std::iter::once(&self.owner)
            .chain(&self.content)
            .chain(&self.properties)
            .chain(&self.grid)
    }
}

fn word_owners_with_states(
    locations: Vec<WordOwnerAddress>,
    states: &[RevisionState],
) -> Vec<WordOwner> {
    let mut result = Vec::with_capacity(locations.len());
    append_word_owners_with_states(&mut result, locations, states);
    result
}

fn append_word_owners_with_states(
    result: &mut Vec<WordOwner>,
    locations: Vec<WordOwnerAddress>,
    states: &[RevisionState],
) {
    let mut offset = 0;
    result.extend(locations.into_iter().map(|address| {
        let WordOwnerAddress {
            location,
            parent,
            body_slot,
            projection_ordinal,
            read_facts,
        } = address;
        let scopes = &states[offset..offset + location.scopes().len()];
        offset += scopes.len();
        let (content, properties, grid) = match &location {
            WordOwnerLocation::Paragraph(_) => {
                (Some(scopes[1].clone()), Some(scopes[2].clone()), None)
            }
            WordOwnerLocation::Table(_) => (None, None, Some(scopes[1].clone())),
            WordOwnerLocation::Hyperlink(..) => (None, None, None),
            _ => (Some(scopes[1].clone()), None, None),
        };
        WordOwner {
            location,
            parent,
            body_slot,
            projection_ordinal,
            read_facts,
            owner: scopes[0].clone(),
            content,
            properties,
            grid,
        }
    }));
}

fn validate_word_facts(
    inner: &oxoffice_docx::Document,
    py: Python<'_>,
) -> PyResult<(Vec<WordOwnerAddress>, WordIndexProjections, WordPieces)> {
    let (pieces, inventory) = WordPieces::default().assemble(inner);
    let (locations, projections) = match inventory {
        Some((locations, projections)) => {
            #[cfg(debug_assertions)]
            assert_word_pieces_rebuild(inner, &locations, &projections);
            (locations, Some(projections))
        }
        None => (
            word_owner_locations(inner, None).map_err(|error| docx_to_pyerr(py, error))?,
            None,
        ),
    };
    let projections = projections.unwrap_or_else(|| WordIndexProjections::of(inner));
    Ok((locations, projections, pieces))
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

impl PyDocument {
    fn imported_text<'py>(
        py: Python<'py>,
        text: &str,
        markdown: bool,
        template: Option<&Bound<'py, PyAny>>,
        images: &[(String, Py<PyBytes>, String)],
    ) -> PyResult<(Py<Self>, Bound<'py, PyTuple>)> {
        let resources = images
            .iter()
            .map(
                |(source, bytes, filename)| oxoffice_docx::HtmlImageResource {
                    source,
                    bytes: bytes.bind(py).as_bytes(),
                    filename,
                },
            )
            .collect::<Vec<_>>();
        let loaded;
        let borrowed;
        let template = match template {
            None => None,
            Some(value) if value.is_instance_of::<PyDocument>() => {
                borrowed = value.extract::<PyRef<'_, Self>>()?;
                Some(&borrowed.inner)
            }
            Some(value) => {
                let path = value.extract::<PathBuf>()?;
                loaded = oxoffice_docx::Document::open(path)
                    .map_err(|error| docx_to_pyerr(py, error))?;
                Some(&loaded)
            }
        };
        let result = py
            .detach(|| {
                if markdown {
                    oxoffice_docx::Document::from_markdown_with_options(text, template, &resources)
                } else {
                    oxoffice_docx::Document::from_html_with_options(text, template, &resources)
                }
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        let diagnostics = html_diagnostics(py, result.diagnostics)?;
        Ok((
            Py::new(py, Self::from_document(result.document, py)?)?,
            diagnostics,
        ))
    }

    fn section_child_scope(&self, index: usize) -> PyResult<RevisionState> {
        self.owners
            .iter()
            .find(
                |owner| matches!(owner.location,WordOwnerLocation::Section(value) if value==index),
            )
            .and_then(|owner| owner.content.clone())
            .ok_or_else(|| PyValueError::new_err("section furniture owner is absent"))
    }

    fn prepared_section_child_scope(
        &self,
        prepared: &PreparedWordRevision,
        index: usize,
    ) -> PyResult<RevisionState> {
        self.revisions
            .capture_prepared(&prepared.revision, smallvec![])
            .map_err(revision_to_pyerr)?;
        prepared
            .owners
            .iter()
            .find(
                |owner| matches!(owner.location,WordOwnerLocation::Section(value) if value==index),
            )
            .and_then(|owner| owner.content.clone())
            .ok_or_else(|| PyValueError::new_err("prepared section furniture owner is absent"))
    }

    pub(crate) fn prepared_furniture_scope(
        &self,
        prepared: &PreparedWordRevision,
        scope: Option<&RevisionState>,
    ) -> PyResult<Option<RevisionState>> {
        let Some(scope) = scope else {
            return Ok(None);
        };
        self.revisions
            .capture_prepared(&prepared.revision, smallvec![])
            .map_err(revision_to_pyerr)?;
        let index = self
            .owners
            .iter()
            .find_map(|owner| match owner.location {
                WordOwnerLocation::Section(index)
                    if owner
                        .content
                        .as_ref()
                        .is_some_and(|content| content.same_state(scope)) =>
                {
                    Some(index)
                }
                _ => None,
            })
            .ok_or_else(|| {
                PyValueError::new_err("furniture authority has no current section owner")
            })?;
        self.prepared_section_child_scope(prepared, index).map(Some)
    }

    fn indexed_owner(&self, state: &RevisionState) -> Option<(usize, &WordOwner)> {
        let index = *self.indexes.by_generation.get(&state.generation())?;
        let owner = self.owners.get(index)?;
        owner.owner.same_state(state).then_some((index, owner))
    }

    /// The route of the physical owner of a paragraph handle's location,
    /// from the inventory of `prepared` when given, or the current one.
    pub(crate) fn indexed_paragraph_location(
        &self,
        location: &ParagraphLocation,
        prepared: Option<&PreparedWordRevision>,
    ) -> Option<oxoffice_docx::ContentLocation> {
        let (owners, indexes) = prepared.map_or((&self.owners, &self.indexes), |prepared| {
            (&prepared.owners, &prepared.indexes)
        });
        let native = match location {
            ParagraphLocation::Body(index) => {
                indexes.body_paragraphs.as_ref()?.get(*index)?.as_ref()?
            }
            ParagraphLocation::Cell {
                table,
                row,
                cell,
                paragraph,
            } => indexes
                .cell_paragraphs
                .get(&(*table, *row, *cell))?
                .get(*paragraph)?,
            ParagraphLocation::Related(location) => location,
        };
        let index = indexes.find(owners, &WordOwnerLocation::Paragraph(native.clone()))?;
        let WordOwnerLocation::Paragraph(location) = &owners[index].location else {
            return None;
        };
        Some(location.clone())
    }

    /// The route of the physical owner of a paragraph handle's location in
    /// the current document, or in `candidate` as `prepared` publishes it:
    /// from that inventory, which holds the route the document resolves, or
    /// as the document resolves it.
    pub(crate) fn paragraph_route(
        &self,
        location: &ParagraphLocation,
        prepared: Option<(&PreparedWordRevision, &oxoffice_docx::Document)>,
    ) -> oxoffice_docx::Result<oxoffice_docx::ContentLocation> {
        let document = prepared.map_or(&self.inner, |(_, candidate)| candidate);
        let indexed =
            self.indexed_paragraph_location(location, prepared.map(|(prepared, _)| prepared));
        match indexed {
            Some(indexed) => {
                #[cfg(debug_assertions)]
                assert_indexed_route(&indexed, || location.native(document), "paragraph");
                Ok(indexed)
            }
            None => location.native(document),
        }
    }

    /// The route of the physical owner of a table handle's location in the
    /// current document: from the inventory, which holds the route the
    /// document resolves, or as the document resolves it.
    pub(crate) fn table_route(
        &self,
        index: usize,
        related: &Option<oxoffice_docx::ContentLocation>,
    ) -> oxoffice_docx::Result<oxoffice_docx::ContentLocation> {
        let native = || crate::table::TableLocation::new(index, related).native(&self.inner);
        let indexed = self.indexed_table_location(index, related, None);
        match indexed {
            Some(indexed) => {
                // The walk of a related table only reads its story again,
                // so the walk checks a body table alone.
                #[cfg(debug_assertions)]
                if related.is_none() {
                    assert_indexed_route(&indexed, native, "table");
                }
                Ok(indexed)
            }
            None => native(),
        }
    }

    /// The route and run path of a run handle's physical owner: from the
    /// inventory record of that owner, or as the document resolves it.
    pub(crate) fn run_route(
        &self,
        state: &RevisionState,
    ) -> PyResult<(
        oxoffice_docx::ContentLocation,
        oxoffice_docx_oxml::text::AcceptedRunPath,
    )> {
        if let Some((_, owner)) = self.indexed_owner(state)
            && let WordOwnerLocation::Run(location, path) = &owner.location
        {
            // The story walk of a recorded route only reads its story again,
            // so the check is that the paragraph there holds this run path.
            #[cfg(debug_assertions)]
            assert!(
                (|| {
                    let current =
                        ParagraphLocation::Related(location.clone()).native(&self.inner)?;
                    self.inner.read_story_paragraph(&current, |paragraph| {
                        (0..paragraph.run_count())
                            .any(|index| paragraph.run_path(index).as_ref() == Some(path))
                    })
                })()
                .expect("an indexed run route resolves in the document"),
                "indexed run route holds its run path"
            );
            return Ok((location.clone(), path.clone()));
        }
        self.current_run_owner(state)
    }

    /// The direct body slot of the paragraph that a body `location` and the
    /// run owner `state` both name, from the inventory. A typed edit made
    /// through `location` then changes that block and no other.
    pub(crate) fn typed_run_edit_slot(
        &self,
        location: &ParagraphLocation,
        state: &RevisionState,
    ) -> Option<usize> {
        if !matches!(location, ParagraphLocation::Body(_)) {
            return None;
        }
        let (_, run) = self.indexed_owner(state)?;
        let paragraph = self.owners.get(run.parent?)?;
        let WordOwnerLocation::Paragraph(route) = &paragraph.location else {
            return None;
        };
        let indexed = self.indexed_paragraph_location(location, None)?;
        (route.story().kind() == oxoffice_docx::StoryKind::Body
            && route.index_path().len() == 1
            && same_content_owner(route, &indexed))
        .then_some(paragraph.body_slot?)
    }

    pub(crate) fn indexed_table_location(
        &self,
        index: usize,
        related: &Option<oxoffice_docx::ContentLocation>,
        prepared: Option<&PreparedWordRevision>,
    ) -> Option<oxoffice_docx::ContentLocation> {
        let (owners, indexes) = prepared.map_or((&self.owners, &self.indexes), |prepared| {
            (&prepared.owners, &prepared.indexes)
        });
        let native = match related {
            Some(location) => location,
            None => indexes.body_tables.get(index)?.as_ref()?,
        };
        let index = indexes.find(owners, &WordOwnerLocation::Table(native.clone()))?;
        let WordOwnerLocation::Table(location) = &owners[index].location else {
            return None;
        };
        Some(location.clone())
    }

    pub(crate) fn indexed_table_owner_location(
        &self,
        state: &RevisionState,
    ) -> Option<oxoffice_docx::ContentLocation> {
        let (_, owner) = self.indexed_owner(state)?;
        let WordOwnerLocation::Table(location) = &owner.location else {
            return None;
        };
        Some(location.clone())
    }

    pub(crate) fn indexed_cell_parent_location(
        &self,
        state: &RevisionState,
    ) -> Option<oxoffice_docx::ContentLocation> {
        let (_, owner) = self.indexed_owner(state)?;
        let WordOwnerLocation::Cell { table, .. } = &owner.location else {
            return None;
        };
        Some(table.clone())
    }

    pub(crate) fn indexed_cell_location(
        &self,
        table: &oxoffice_docx::ContentLocation,
        row: usize,
        cell: usize,
    ) -> Option<WordOwnerLocation> {
        // Cell route equivalence intentionally ignores the story fingerprint.
        // Return the actual current inventory value, never the lookup placeholder.
        let key = WordOwnerLocation::Cell {
            table: table.clone(),
            row,
            cell,
            story: oxoffice_docx::StoryId::body(),
        };
        let index = self.indexes.find(&self.owners, &key)?;
        Some(self.owners[index].location.clone())
    }

    pub(crate) fn capture_current_view(
        &self,
        state: &RevisionState,
        segments: smallvec::SmallVec<[PathSeg; 5]>,
        view: WordView,
        furniture: Option<&RevisionState>,
    ) -> PyResult<ContentPath> {
        let (index, _) = self
            .indexed_owner(state)
            .ok_or_else(|| PyValueError::new_err("physical owner is no longer current"))?;
        word_owner_capture(
            &self.revisions,
            &self.owners,
            (
                self.revisions.current_document_scope(),
                self.removal_scope.as_ref().expect("active Word authority"),
            ),
            index,
            segments,
            view,
            furniture,
        )
        .map(|(path, _)| path)
    }

    /// The direct body slot of a current paragraph owner, from the inventory.
    pub(crate) fn direct_body_paragraph_slot(&self, state: &RevisionState) -> Option<usize> {
        let (_, owner) = self.indexed_owner(state)?;
        match &owner.location {
            WordOwnerLocation::Paragraph(location)
                if location.story().kind() == oxoffice_docx::StoryKind::Body
                    && location.index_path().len() == 1 =>
            {
                owner.body_slot
            }
            _ => None,
        }
    }

    /// The direct body slot of a current run owner's paragraph, and the run
    /// route in it, from the inventory.
    pub(crate) fn direct_body_run_slot(
        &self,
        state: &RevisionState,
    ) -> Option<(usize, oxoffice_docx_oxml::text::AcceptedRunPath)> {
        let (_, owner) = self.indexed_owner(state)?;
        let WordOwnerLocation::Run(location, path) = &owner.location else {
            return None;
        };
        let paragraph = self.owners.get(owner.parent?)?;
        match &paragraph.location {
            WordOwnerLocation::Paragraph(parent)
                if parent.story().kind() == oxoffice_docx::StoryKind::Body
                    && parent.index_path().len() == 1
                    && parent.index_path() == location.index_path() =>
            {
                Some((paragraph.body_slot?, path.clone()))
            }
            _ => None,
        }
    }

    /// The direct body slot of a current cell owner's table, and the cell
    /// coordinates, from the inventory.
    pub(crate) fn direct_body_cell_slot(
        &self,
        state: &RevisionState,
    ) -> Option<(usize, usize, usize)> {
        let (_, owner) = self.indexed_owner(state)?;
        let WordOwnerLocation::Cell {
            table, row, cell, ..
        } = &owner.location
        else {
            return None;
        };
        let parent = self.owners.get(owner.parent?)?;
        match &parent.location {
            WordOwnerLocation::Table(location)
                if location.story().kind() == oxoffice_docx::StoryKind::Body
                    && location.index_path().len() == 1
                    && location.index_path() == table.index_path() =>
            {
                Some((parent.body_slot?, *row, *cell))
            }
            _ => None,
        }
    }

    /// The direct body slot of a current table owner, from the inventory.
    pub(crate) fn direct_body_table_slot(&self, state: &RevisionState) -> Option<usize> {
        let (_, owner) = self.indexed_owner(state)?;
        match &owner.location {
            WordOwnerLocation::Table(location)
                if location.story().kind() == oxoffice_docx::StoryKind::Body
                    && location.index_path().len() == 1 =>
            {
                owner.body_slot
            }
            _ => None,
        }
    }

    pub(crate) fn cached_paragraph_text(&self, state: &RevisionState) -> Option<&str> {
        let (_, owner) = self.indexed_owner(state)?;
        match &owner.read_facts {
            WordReadFacts::Paragraph(text) => Some(text),
            _ => None,
        }
    }

    pub(crate) fn publish_run_content_candidate(
        &mut self,
        candidate: oxoffice_docx::Document,
        owner: Option<&RevisionState>,
        text: String,
    ) -> PyResult<()> {
        let paragraph = owner
            .map(|state| {
                let (_, run) = self.indexed_owner(state).ok_or_else(|| {
                    PyValueError::new_err("run physical owner is no longer current")
                })?;
                if !matches!(run.location, WordOwnerLocation::Run(..)) {
                    return Err(PyValueError::new_err("run owner has the wrong kind"));
                }
                run.parent
                    .filter(|parent| {
                        self.owners.get(*parent).is_some_and(|owner| {
                            matches!(owner.location, WordOwnerLocation::Paragraph(_))
                        })
                    })
                    .ok_or_else(|| PyValueError::new_err("run has no current paragraph owner"))
            })
            .transpose()?;
        // These publishers retain physical topology and all existing scopes.
        // Candidate text and parent authority are ready before either value moves.
        if let Some(paragraph) = paragraph {
            self.owners[paragraph].read_facts = WordReadFacts::Paragraph(text);
        }
        self.install_inner(candidate);
        Ok(())
    }

    pub(crate) fn cached_body_paragraph_count(&self) -> Option<usize> {
        Some(self.indexes.body_paragraphs.as_ref()?.len())
    }

    pub(crate) fn cached_body_table_count(&self) -> usize {
        self.indexes.body_tables.len()
    }

    pub(crate) fn cached_table_rows(&self, state: &RevisionState) -> Option<&[Vec<usize>]> {
        let (_, owner) = self.indexed_owner(state)?;
        match &owner.read_facts {
            WordReadFacts::Table(rows) => Some(rows),
            _ => None,
        }
    }

    pub(crate) fn cached_cell_paragraph_count(&self, state: &RevisionState) -> Option<usize> {
        let (_, owner) = self.indexed_owner(state)?;
        let WordOwnerLocation::Cell { row, cell, .. } = &owner.location else {
            return None;
        };
        let parent = self.owners.get(owner.parent?)?;
        let WordReadFacts::Table(rows) = &parent.read_facts else {
            return None;
        };
        rows.get(*row)?.get(*cell).copied()
    }

    pub(crate) fn cached_cell_paragraph(
        &self,
        state: &RevisionState,
        paragraph: usize,
    ) -> Option<oxoffice_docx::ContentLocation> {
        let (index, _) = self.indexed_owner(state)?;
        let items = self.indexes.paragraphs_by_parent.get(&index)?;
        // A source/model mismatch stays on the original native admission path.
        if items.len() != self.cached_cell_paragraph_count(state)? {
            return None;
        }
        let WordOwnerLocation::Paragraph(location) = &self.owners[*items.get(paragraph)?].location
        else {
            return None;
        };
        Some(location.clone())
    }

    pub(crate) fn cached_cell_tables(
        &self,
        state: &RevisionState,
    ) -> Option<Vec<oxoffice_docx::ContentLocation>> {
        let (index, owner) = self.indexed_owner(state)?;
        if !matches!(owner.location, WordOwnerLocation::Cell { .. }) {
            return None;
        }
        Some(
            self.indexes
                .tables_by_parent
                .get(&index)
                .into_iter()
                .flatten()
                .map(|index| {
                    let WordOwnerLocation::Table(location) = &self.owners[*index].location else {
                        unreachable!("table index contains tables");
                    };
                    location.clone()
                })
                .collect(),
        )
    }

    pub(crate) fn capture_owner(
        &self,
        location: &WordOwnerLocation,
        segments: smallvec::SmallVec<[PathSeg; 5]>,
        view: WordView,
        furniture_scope: Option<&RevisionState>,
    ) -> PyResult<(ContentPath, RevisionState)> {
        word_owner_capture(
            &self.revisions,
            &self.owners,
            (
                self.revisions.current_document_scope(),
                self.removal_scope.as_ref().expect("active Word authority"),
            ),
            self.indexes.find(&self.owners, location).ok_or_else(|| {
                PyValueError::new_err("element has no current checked physical owner")
            })?,
            segments,
            view,
            furniture_scope,
        )
    }

    pub(crate) fn capture_prepared_owner(
        &self,
        prepared: &PreparedWordRevision,
        location: &WordOwnerLocation,
        segments: smallvec::SmallVec<[PathSeg; 5]>,
        view: WordView,
        furniture_scope: Option<&RevisionState>,
    ) -> PyResult<(ContentPath, RevisionState)> {
        // Validate origin and expected serial before using any prepared state.
        self.revisions
            .capture_prepared(&prepared.revision, smallvec![])
            .map_err(revision_to_pyerr)?;
        let (mut path, owner) = word_owner_capture(
            &self.revisions,
            &prepared.owners,
            (
                prepared.revision.document_scope(),
                self.removal_scope.as_ref().expect("active Word authority"),
            ),
            prepared
                .indexes
                .find(&prepared.owners, location)
                .ok_or_else(|| {
                    PyValueError::new_err("element has no current checked physical owner")
                })?,
            segments,
            view,
            furniture_scope,
        )?;
        path.revision = prepared.revision.committed_serial();
        Ok((path, owner))
    }

    pub(crate) fn capture_snapshot(
        &self,
        segments: smallvec::SmallVec<[PathSeg; 5]>,
    ) -> PyResult<ContentPath> {
        self.revisions
            .capture_scopes(
                segments,
                &[
                    self.revisions.current_document_scope(),
                    self.snapshot_scope
                        .as_ref()
                        .expect("active Word authority")
                        .clone(),
                ],
            )
            .map_err(revision_to_pyerr)
    }

    pub(crate) fn capture_prepared_snapshot(
        &self,
        prepared: &PreparedWordRevision,
        segments: smallvec::SmallVec<[PathSeg; 5]>,
    ) -> PyResult<ContentPath> {
        self.revisions
            .capture_prepared(&prepared.revision, smallvec![])
            .map_err(revision_to_pyerr)?;
        let mut path = self
            .revisions
            .capture_scopes(
                segments,
                &[
                    prepared.revision.document_scope(),
                    prepared.snapshot_scope.clone(),
                ],
            )
            .map_err(revision_to_pyerr)?;
        path.revision = prepared.revision.committed_serial();
        Ok(path)
    }

    pub(crate) fn plain_paragraph_owner(&self, state: &RevisionState) -> Option<(usize, usize)> {
        let selected = *self.indexes.by_generation.get(&state.generation())?;
        let owner = self.owners.get(selected)?;
        if !owner.owner.same_state(state) {
            return None;
        }
        let slot = owner.body_slot?;
        (self.indexes.direct_body_paragraphs.get(&slot) == Some(&selected))
            .then_some((selected, slot))
    }

    fn prepare_tail_body_removal(
        &self,
        slot: usize,
    ) -> PyResult<Option<(std::ops::Range<usize>, PreparedRevision)>> {
        if slot.checked_add(1) != Some(self.inner.content_count()) {
            return Ok(None);
        }
        let Some(&selected) = self.indexes.direct_body_paragraphs.get(&slot) else {
            return Ok(None);
        };
        let mut end = selected + 1;
        while end < self.owners.len() && self.owners[end].parent == Some(selected) {
            end += 1;
        }
        for owner in &self.owners[end..] {
            let independent_run = matches!(owner.location, WordOwnerLocation::Run(..))
                && owner.parent.is_some_and(|parent| parent < selected);
            if !matches!(owner.location, WordOwnerLocation::Section(_)) && !independent_run {
                return Ok(None);
            }
        }
        let revision = self
            .revisions
            .prepare_scopes(
                "Document.remove_content",
                &[
                    self.removal_scope
                        .as_ref()
                        .expect("active Word authority")
                        .clone(),
                    self.snapshot_scope
                        .as_ref()
                        .expect("active Word authority")
                        .clone(),
                ],
                &[RevisionScope::Document, RevisionScope::Text],
            )
            .map_err(revision_to_pyerr)?;
        Ok(Some((selected..end, revision)))
    }

    fn commit_tail_body_removal(
        &mut self,
        slot: usize,
        range: std::ops::Range<usize>,
        revision: PreparedRevision,
    ) -> PyResult<()> {
        let removal_scope = revision.born(0).expect("removal epoch birth");
        let snapshot_scope = revision.born(1).expect("removal snapshot birth");
        self.revisions
            .commit_scopes(revision)
            .map_err(revision_to_pyerr)?;
        let start = range.start;
        let end = range.end;
        for (offset, owner) in self.owners.drain(range).enumerate() {
            self.indexes.by_generation.remove(&owner.owner.generation());
            let hash = owner.location.route_hash();
            let bucket = self
                .indexes
                .by_route
                .get_mut(&hash)
                .expect("removed owner route is indexed");
            bucket.retain(|index| *index != start + offset);
            if bucket.is_empty() {
                self.indexes.by_route.remove(&hash);
            }
            self.indexes.paragraphs_by_parent.remove(&(start + offset));
            self.indexes.tables_by_parent.remove(&(start + offset));
            self.indexes.cells_by_parent.remove(&(start + offset));
            self.indexes.runs_by_parent.remove(&(start + offset));
        }
        if let Some(paragraphs) = &mut self.indexes.body_paragraphs {
            paragraphs.pop();
        }
        // Suffix sections and locally born runs of earlier paragraphs have no
        // direct-body route or parent index to shift. Earlier owners stay put.
        for (offset, owner) in self.owners[start..].iter().enumerate() {
            for index in self
                .indexes
                .by_route
                .get_mut(&owner.location.route_hash())
                .expect("surviving suffix route is indexed")
            {
                if *index == end + offset {
                    *index = start + offset;
                }
            }
            *self
                .indexes
                .by_generation
                .get_mut(&owner.owner.generation())
                .expect("surviving suffix owner is indexed") = start + offset;
            if matches!(owner.location, WordOwnerLocation::Run(..))
                && let Some(parent) = owner.parent
                && let Some(runs) = self.indexes.runs_by_parent.get_mut(&parent)
                && let Some(index) = runs.iter_mut().find(|index| **index == end + offset)
            {
                *index = start + offset;
            }
        }
        self.indexes.direct_body_paragraphs.remove(&slot);
        self.removal_scope = Some(removal_scope);
        self.snapshot_scope = Some(snapshot_scope);
        self.pieces
            .as_mut()
            .expect("active pieces")
            .splice(&self.inner, slot, 1, None);
        Ok(())
    }

    pub(crate) fn current_paragraph_owner(
        &self,
        state: &RevisionState,
    ) -> PyResult<oxoffice_docx::ContentLocation> {
        self.owners
            .iter()
            .find_map(|owner| {
                if !owner.owner.same_state(state) {
                    return None;
                }
                match &owner.location {
                    WordOwnerLocation::Paragraph(location) => Some(location.clone()),
                    _ => None,
                }
            })
            .ok_or_else(|| PyValueError::new_err("paragraph physical owner is no longer current"))
            .and_then(|location| {
                ParagraphLocation::Related(location)
                    .native(&self.inner)
                    .map_err(|error| PyValueError::new_err(error.to_string()))
            })
    }

    pub(crate) fn current_run_owner(
        &self,
        state: &RevisionState,
    ) -> PyResult<(
        oxoffice_docx::ContentLocation,
        oxoffice_docx_oxml::text::AcceptedRunPath,
    )> {
        self.owners
            .iter()
            .find_map(|owner| {
                if !owner.owner.same_state(state) {
                    return None;
                }
                match &owner.location {
                    WordOwnerLocation::Run(location, path) => {
                        Some((location.clone(), path.clone()))
                    }
                    _ => None,
                }
            })
            .ok_or_else(|| PyValueError::new_err("run physical owner is no longer current"))
            .and_then(|(location, path)| {
                ParagraphLocation::Related(location)
                    .native(&self.inner)
                    .map(|location| (location, path))
                    .map_err(|error| PyValueError::new_err(error.to_string()))
            })
    }

    pub(crate) fn current_table_owner(
        &self,
        state: &RevisionState,
    ) -> PyResult<oxoffice_docx::ContentLocation> {
        self.owners
            .iter()
            .find_map(|owner| {
                if !owner.owner.same_state(state) {
                    return None;
                }
                match &owner.location {
                    WordOwnerLocation::Table(location) => Some(location.clone()),
                    _ => None,
                }
            })
            .ok_or_else(|| PyValueError::new_err("table physical owner is no longer current"))
            .and_then(|location| {
                crate::table::TableLocation::Related(location)
                    .native(&self.inner)
                    .map_err(|error| PyValueError::new_err(error.to_string()))
            })
    }

    pub(crate) fn current_cell_owner(
        &self,
        state: &RevisionState,
    ) -> PyResult<(oxoffice_docx::ContentLocation, usize, usize)> {
        self.owners
            .iter()
            .find_map(|owner| {
                if !owner.owner.same_state(state) {
                    return None;
                }
                match &owner.location {
                    WordOwnerLocation::Cell {
                        table, row, cell, ..
                    } => Some((table.clone(), *row, *cell)),
                    _ => None,
                }
            })
            .ok_or_else(|| PyValueError::new_err("cell physical owner is no longer current"))
            .and_then(|(table, row, cell)| {
                crate::table::TableLocation::Related(table)
                    .native(&self.inner)
                    .map(|table| (table, row, cell))
                    .map_err(|error| PyValueError::new_err(error.to_string()))
            })
    }

    pub(crate) fn prepare_plain_paragraph_xml(
        &mut self,
        selected: usize,
        paragraph: &oxoffice_docx_oxml::CT_P,
    ) -> PyResult<PreparedPlainParagraphXml> {
        let WordOwnerLocation::Paragraph(location) = &self.owners[selected].location else {
            return Err(PyValueError::new_err("local XML owner is not a paragraph"));
        };
        let mut runs = Vec::new();
        let mut addresses = Vec::new();
        for path in paragraph.accepted_run_paths() {
            let location = WordOwnerLocation::Run(location.clone(), path);
            if let Some(index) = self.indexes.find(&self.owners, &location) {
                runs.push(index);
            } else {
                addresses.push(WordOwnerAddress {
                    location,
                    parent: Some(selected),
                    body_slot: None,
                    projection_ordinal: None,
                    read_facts: WordReadFacts::None,
                });
            }
        }
        let fail =
            |_| pyo3::exceptions::PyMemoryError::new_err("Word local owner allocation failed");
        self.owners.try_reserve(addresses.len()).map_err(fail)?;
        self.indexes
            .by_generation
            .try_reserve(runs.len() + addresses.len())
            .map_err(fail)?;
        self.indexes
            .by_route
            .try_reserve(addresses.len())
            .map_err(fail)?;
        self.indexes.runs_by_parent.try_reserve(1).map_err(fail)?;
        self.indexes
            .runs_by_parent
            .entry(selected)
            .or_default()
            .try_reserve(addresses.len())
            .map_err(fail)?;
        let mut retired = Vec::new();
        if let Some(content) = &self.owners[selected].content {
            retired.push(content.clone());
        }
        if let Some(properties) = &self.owners[selected].properties {
            retired.push(properties.clone());
        }
        for &index in &runs {
            retired.extend(self.owners[index].states());
        }
        retired.push(
            self.snapshot_scope
                .as_ref()
                .expect("active Word authority")
                .clone(),
        );
        let mut scopes = retired.iter().map(RevisionState::scope).collect::<Vec<_>>();
        for address in &addresses {
            scopes.extend_from_slice(address.location.scopes());
        }
        let revision = self
            .revisions
            .prepare_local_scopes("Paragraph.replace_xml", &retired, &scopes)
            .map_err(revision_to_pyerr)?;
        let states = (0..scopes.len())
            .map(|index| revision.born(index).expect("prepared paragraph scope"))
            .collect::<Vec<_>>();
        let newborn = word_owners_with_states(addresses, &states[retired.len()..]);
        let mut route_buckets = HashMap::<u64, Vec<usize>>::new();
        for (offset, owner) in newborn.iter().enumerate() {
            let hash = owner.location.route_hash();
            route_buckets
                .entry(hash)
                .or_insert_with(|| {
                    self.indexes
                        .by_route
                        .get(&hash)
                        .cloned()
                        .unwrap_or_default()
                })
                .push(self.owners.len() + offset);
        }
        Ok(PreparedPlainParagraphXml {
            revision,
            states,
            runs,
            newborn,
            route_buckets,
        })
    }

    pub(crate) fn commit_plain_paragraph_xml(
        &mut self,
        selected: usize,
        prepared: PreparedPlainParagraphXml,
        text: String,
    ) -> PyResult<()> {
        let PreparedPlainParagraphXml {
            revision,
            states,
            runs,
            newborn,
            route_buckets,
        } = prepared;
        self.revisions
            .commit_scopes(revision)
            .map_err(revision_to_pyerr)?;
        let mut born = 0;
        if self.owners[selected].content.is_some() {
            self.owners[selected].content = Some(states[born].clone());
            born += 1;
        }
        if self.owners[selected].properties.is_some() {
            self.owners[selected].properties = Some(states[born].clone());
            born += 1;
        }
        for index in runs {
            let owner = &mut self.owners[index];
            self.indexes.by_generation.remove(&owner.owner.generation());
            owner.owner = states[born].clone();
            born += 1;
            if owner.content.is_some() {
                owner.content = Some(states[born].clone());
                born += 1;
            }
            if owner.properties.is_some() {
                owner.properties = Some(states[born].clone());
                born += 1;
            }
            if owner.grid.is_some() {
                owner.grid = Some(states[born].clone());
                born += 1;
            }
            self.indexes
                .by_generation
                .insert(owner.owner.generation(), index);
        }
        self.owners[selected].read_facts = WordReadFacts::Paragraph(text);
        self.snapshot_scope = Some(states[born].clone());
        for owner in newborn {
            self.indexes
                .by_generation
                .insert(owner.owner.generation(), self.owners.len());
            self.indexes
                .runs_by_parent
                .get_mut(&selected)
                .expect("reserved run bucket")
                .push(self.owners.len());
            self.owners.push(owner);
        }
        for (hash, bucket) in route_buckets {
            self.indexes.by_route.insert(hash, bucket);
        }
        Ok(())
    }

    pub(crate) fn prepare_word_revision(
        &mut self,
        py: Python<'_>,
        candidate: &oxoffice_docx::Document,
        operation: &'static str,
        publication: WordPublication,
    ) -> PyResult<PreparedWordRevision> {
        self.ensure_word_authority(py)?;
        let forms = matches!(
            publication,
            WordPublication::FormValues
                | WordPublication::ModernFormValues
                | WordPublication::Signing
        );
        let modern = matches!(publication, WordPublication::ModernFormValues);
        let mut old_sources = FormSources::default();
        let mut new_sources = FormSources::default();
        let (piece_cache, inventory) = if forms {
            (None, None)
        } else {
            let (pieces, inventory) = self
                .pieces
                .as_ref()
                .expect("active pieces")
                .assemble(candidate);
            (Some(pieces), inventory)
        };
        let (locations, projections) = match inventory {
            Some((locations, projections)) => {
                #[cfg(debug_assertions)]
                assert_word_pieces_rebuild(candidate, &locations, &projections);
                (locations, Some(projections))
            }
            None => (
                word_owner_locations(candidate, forms.then_some(&mut new_sources))
                    .map_err(|error| docx_to_pyerr(py, error))?,
                None,
            ),
        };
        if forms {
            word_owner_locations(&self.inner, Some(&mut old_sources))
                .map_err(|error| docx_to_pyerr(py, error))?;
        }
        let selected = match &publication {
            WordPublication::HeaderFooterAppend(index) => Some(WordOwnerLocation::Section(*index)),
            WordPublication::ParagraphText(location)
            | WordPublication::ParagraphAppend(location) => {
                Some(WordOwnerLocation::Paragraph(location.clone()))
            }
            WordPublication::TableGrid(location) => {
                Some(WordOwnerLocation::Table(location.clone()))
            }
            WordPublication::CellText(table, row, cell) => {
                self.owners.iter().find_map(|owner| match &owner.location {
                    WordOwnerLocation::Cell {
                        table: owned,
                        row: r,
                        cell: c,
                        ..
                    } if same_content_owner(owned, table) && r == row && c == cell => {
                        Some(owner.location.clone())
                    }
                    _ => None,
                })
            }
            WordPublication::ElementXml(location)
            | WordPublication::RunText(location)
            | WordPublication::HyperlinkRemove(location) => Some(location.clone()),
            _ => None,
        };
        let selected_index = selected.as_ref().and_then(|selected| {
            self.owners
                .iter()
                .position(|owner| owner.location.same_route(selected))
        });
        if !matches!(
            publication,
            WordPublication::Conservative
                | WordPublication::FormValues
                | WordPublication::ModernFormValues
                | WordPublication::Signing
                | WordPublication::AppendBody
        ) && selected_index.is_none()
        {
            return Err(PyValueError::new_err(
                "checked XML owner has no current physical authority",
            ));
        }
        // Conservative publication retires every old scope. Matching those
        // owners cannot certify any retention and only clones unused routes.
        let matches = if matches!(publication, WordPublication::Conservative) {
            vec![None; locations.len()]
        } else {
            indexed_inventory_matches(&self.owners, &locations, &publication)
        };
        let (eligible_parts, modern_snapshot) = if modern {
            unchanged_related_owner_parts(&self.inner, candidate, &self.owners, &locations)
                .map_err(|error| docx_to_pyerr(py, error))?
        } else {
            (std::collections::HashSet::new(), false)
        };
        let form_certificates = if matches!(publication, WordPublication::Signing) {
            signing_owner_certificates(
                &self.owners,
                &locations,
                &matches,
                &old_sources,
                &new_sources,
            )
        } else if modern {
            modern_form_owner_certificates(
                &self.owners,
                &locations,
                &matches,
                &old_sources,
                &new_sources,
                &eligible_parts,
            )
        } else if forms {
            form_owner_certificates(
                &self.owners,
                &locations,
                &matches,
                &old_sources,
                &new_sources,
                &std::collections::HashSet::new(),
            )
        } else {
            std::collections::HashSet::new()
        };
        let preserve_snapshot = modern_snapshot
            || matches!(publication, WordPublication::Signing)
                && self
                    .inner
                    .stories()
                    .map_err(|error| docx_to_pyerr(py, error))?
                    == candidate
                        .stories()
                        .map_err(|error| docx_to_pyerr(py, error))?
                && self
                    .inner
                    .story_item_snapshots()
                    .map_err(|error| docx_to_pyerr(py, error))?
                    == candidate
                        .story_item_snapshots()
                        .map_err(|error| docx_to_pyerr(py, error))?;
        let mut retained = Vec::new();
        let mut births = Vec::new();
        for (index, address) in locations.iter().enumerate() {
            let location = &address.location;
            let old = matches[index].map(|before| (before, &self.owners[before]));
            let mut states = Vec::new();
            let mut descendant = false;
            if let (Some((before, _)), Some(selected)) = (old, selected_index) {
                let mut parent = self.owners[before].parent;
                while let Some(index) = parent {
                    if index == selected {
                        descendant = true;
                        break;
                    }
                    parent = self.owners[index].parent;
                }
            }
            for (slot, scope) in location.scopes().iter().enumerate() {
                let route_proved = old.is_some_and(|(before, owner)| {
                    if matches!(publication, WordPublication::AppendBody) {
                        return true;
                    }
                    if Some(before) == selected_index {
                        // A text-box paragraph is rewritten through its physical
                        // part. Its owner scope retires with the edit, so the
                        // handle that made the change cannot be reused.
                        if matches!(
                            &self.owners[before].location,
                            WordOwnerLocation::Paragraph(location)
                                if location.story().kind() == oxoffice_docx::StoryKind::TextBox
                        ) {
                            return false;
                        }
                        return true;
                    }
                    let mut root = before;
                    while let Some(parent) = self.owners[root].parent {
                        root = parent;
                    }
                    match &self.owners[root].location {
                        WordOwnerLocation::Section(_) => true,
                        WordOwnerLocation::Hyperlink(..) => matches!(
                            publication,
                            WordPublication::ParagraphAppend(_)
                                | WordPublication::HyperlinkRemove(_)
                        ),
                        WordOwnerLocation::Paragraph(location)
                        | WordOwnerLocation::Table(location) => match location.story().kind() {
                            oxoffice_docx::StoryKind::Body => self.owners[root].body_slot.is_some(),
                            oxoffice_docx::StoryKind::TextBox
                            | oxoffice_docx::StoryKind::Comment => false,
                            oxoffice_docx::StoryKind::Header
                            | oxoffice_docx::StoryKind::Footer
                            | oxoffice_docx::StoryKind::Footnote
                            | oxoffice_docx::StoryKind::Endnote => true,
                            _ => false,
                        },
                        _ => {
                            let _ = owner;
                            false
                        }
                    }
                });
                let preserve = old
                    .is_some_and(|(before, _)| form_certificates.contains(&(before, index)))
                    || (route_proved
                        && !matches!(
                            publication,
                            WordPublication::Conservative
                                | WordPublication::FormValues
                                | WordPublication::ModernFormValues
                                | WordPublication::Signing
                        )
                        && !descendant
                        && !(matches!(publication, WordPublication::HyperlinkRemove(_))
                            && matches!(location, WordOwnerLocation::Paragraph(_))
                            && slot == 1)
                        && !(old.is_some_and(|(before, _)| Some(before) == selected_index)
                            && slot != 0
                            && !matches!(publication, WordPublication::RunText(_))
                            && (!matches!(
                                publication,
                                WordPublication::ParagraphText(_)
                                    | WordPublication::ParagraphAppend(_)
                            ) || slot == 1)));
                let state = if preserve {
                    old.map(|(_, owner)| owner.states()[slot].clone())
                } else {
                    None
                };
                if state.is_none() {
                    births.push(*scope);
                }
                states.push(state);
            }
            retained.push(states);
        }
        let surviving = retained
            .iter()
            .flatten()
            .filter_map(Option::as_ref)
            .map(RevisionState::generation)
            .collect::<std::collections::HashSet<_>>();
        let mut retired = self
            .owners
            .iter()
            .flat_map(WordOwner::states)
            .filter(|state| !surviving.contains(&state.generation()))
            .collect::<Vec<_>>();
        if !preserve_snapshot {
            retired.push(
                self.snapshot_scope
                    .as_ref()
                    .expect("active Word authority")
                    .clone(),
            );
            births.push(RevisionScope::Text);
        }
        // A run handle also carries the document epoch. Modern publication
        // keeps that epoch with the same local commit Signing uses. A proved
        // run then stays current. An unproved owner state still retires it.
        let revision = if matches!(publication, WordPublication::Conservative) {
            self.revisions.prepare_revision(
                Some(operation),
                &retired,
                &births,
                RevisionPreparation::Replacement,
            )
        } else if matches!(
            publication,
            WordPublication::ElementXml(_)
                | WordPublication::RunText(_)
                | WordPublication::Signing
                | WordPublication::ModernFormValues
        ) {
            self.revisions
                .prepare_local_scopes(operation, &retired, &births)
        } else {
            self.revisions.prepare_scopes(operation, &retired, &births)
        }
        .map_err(revision_to_pyerr)?;
        let mut born = 0;
        let mut states = Vec::new();
        for record in retained {
            for state in record {
                states.push(state.unwrap_or_else(|| {
                    let state = revision.born(born).expect("prepared owner birth");
                    born += 1;
                    state
                }));
            }
        }
        let snapshot_scope = if preserve_snapshot {
            self.snapshot_scope
                .as_ref()
                .expect("active Word authority")
                .clone()
        } else {
            revision.born(born).expect("prepared snapshot birth")
        };
        let owners = word_owners_with_states(locations, &states);
        let projections = projections.unwrap_or_else(|| WordIndexProjections::of(candidate));
        let indexes = WordOwnerIndexes::prepare(&owners, projections)?;
        Ok(PreparedWordRevision {
            revision,
            owners,
            indexes,
            snapshot_scope,
            pieces: piece_cache,
        })
    }

    pub(crate) fn commit_word_revision(&mut self, prepared: PreparedWordRevision) -> PyResult<()> {
        self.revisions
            .commit_scopes(prepared.revision)
            .map_err(revision_to_pyerr)?;
        self.owners = prepared.owners;
        self.indexes = prepared.indexes;
        self.snapshot_scope = Some(prepared.snapshot_scope);
        self.pending_pieces = Some(prepared.pieces.unwrap_or_default());
        Ok(())
    }

    pub(crate) fn install_inner(&mut self, candidate: oxoffice_docx::Document) {
        self.pieces = if self.snapshot_scope.is_some() {
            Some(self.pending_pieces.take().unwrap_or_default())
        } else {
            self.pending_pieces = None;
            None
        };
        self.inner = candidate;
    }

    pub(crate) fn publish_owner_candidate(
        &mut self,
        py: Python<'_>,
        candidate: oxoffice_docx::Document,
        operation: &'static str,
        publication: WordPublication,
    ) -> PyResult<()> {
        if self
            .inner
            .saves_as(&candidate)
            .map_err(|error| docx_to_pyerr(py, error))?
        {
            return Ok(());
        }
        let prepared = self.prepare_word_revision(py, &candidate, operation, publication)?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    /// Publish `candidate`, this document after one typed edit of the
    /// direct body paragraph at `slot`, unless the edit changed nothing. The
    /// answer is read from that block when it tells, and from the staged
    /// save comparison of [`Self::publish_owner_candidate`] when it does
    /// not.
    pub(crate) fn publish_block_candidate(
        &mut self,
        py: Python<'_>,
        candidate: oxoffice_docx::Document,
        operation: &'static str,
        publication: WordPublication,
        slot: usize,
    ) -> PyResult<()> {
        let Some(same) = self.direct_paragraph_saves_as(&candidate, slot) else {
            return self.publish_owner_candidate(py, candidate, operation, publication);
        };
        // The staged comparison saves the whole candidate and refuses when
        // content outside this block cannot be saved. It checks the block
        // answer whenever it gives one.
        #[cfg(debug_assertions)]
        if let Ok(saved) = self.inner.saves_as(&candidate) {
            assert_eq!(saved, same, "direct body paragraph save comparison");
        }
        if same {
            return Ok(());
        }
        let prepared = self.prepare_word_revision(py, &candidate, operation, publication)?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    /// Whether `candidate`, this document after one typed edit of the
    /// direct body paragraph at `slot`, saves as this document, read from
    /// that block alone. Equal paragraph models save the same bytes. Two
    /// plain paragraphs whose checked reader facts differ, the text and the
    /// run paths of [`oxoffice_docx::Document::direct_body_paragraph_facts`], save
    /// different bytes, as a plain paragraph holds nothing a save
    /// renumbers. `None` leaves the answer to the staged save: another
    /// paragraph shape, a body whose save is known to fail, text a save
    /// refuses, or facts that do not tell. The block answer does not save
    /// the rest of the document, so content elsewhere that no save accepts
    /// is reported by the save itself, as after `Paragraph.text`.
    fn direct_paragraph_saves_as(
        &self,
        candidate: &oxoffice_docx::Document,
        slot: usize,
    ) -> Option<bool> {
        if self.inner.unsavable_body_edit().is_some() {
            return None;
        }
        let (
            oxoffice_docx_oxml::BodyContent::Paragraph(current),
            oxoffice_docx_oxml::BodyContent::Paragraph(edited),
        ) = (
            self.inner.body_children().get(slot)?,
            candidate.body_children().get(slot)?,
        )
        else {
            return None;
        };
        if current == edited {
            return Some(true);
        }
        let before = self.inner.direct_body_paragraph_facts(current).ok()??;
        let after = candidate.direct_body_paragraph_facts(edited).ok()??;
        // A save refuses a character XML 1.0 cannot carry, with its place in
        // the part. The block reader does not, so the staged save answers.
        if !after
            .0
            .chars()
            .all(oxoffice_core::xml::is_xml_1_0_character)
        {
            return None;
        }
        (before != after).then_some(false)
    }

    pub(crate) fn local_cell_route(
        &self,
        state: &RevisionState,
    ) -> Option<(usize, usize, usize, usize)> {
        let (slot, row, cell) = self.direct_body_cell_slot(state)?;
        let (_, owner) = self.indexed_owner(state)?;
        let parent = self.owners.get(owner.parent?)?;
        Some((slot, parent.projection_ordinal?, row, cell))
    }

    pub(crate) fn try_local_cell_paragraph_text(
        &mut self,
        py: Python<'_>,
        location: &oxoffice_docx::ContentLocation,
        text: &str,
    ) -> PyResult<bool> {
        if location.index_path().len() != 1 {
            return Ok(false);
        }
        let Some(index) = self.indexes.find(
            &self.owners,
            &WordOwnerLocation::Paragraph(location.clone()),
        ) else {
            return Ok(false);
        };
        let Some(parent) = self.owners[index].parent else {
            return Ok(false);
        };
        let Some((slot, table, row, cell)) = self.local_cell_route(&self.owners[parent].owner)
        else {
            return Ok(false);
        };
        let Some(paragraphs) = self.indexes.cell_paragraphs.get(&(table, row, cell)) else {
            return Ok(false);
        };
        let Some(position) = paragraphs
            .iter()
            .position(|candidate| same_content_owner(candidate, location))
        else {
            return Ok(false);
        };
        let ordinal = paragraphs[..position]
            .iter()
            .filter(|location| location.index_path().len() == 1)
            .count();
        let Some(candidate) = self
            .inner
            .direct_body_table_cell_candidate(slot, row, cell, |cell| {
                cell.paragraph_mut(ordinal)
                    .ok_or_else(|| {
                        oxoffice_docx::Error::Other("paragraph index out of range".into())
                    })?
                    .set_text(text)
            })
            .map_err(|error| docx_to_pyerr(py, error))?
        else {
            return Ok(false);
        };
        self.apply_local_cell_edit(
            py,
            slot,
            table,
            row,
            cell,
            candidate,
            "Paragraph.text",
            Some(WordPublication::ParagraphText(location.clone())),
        )
    }

    /// Publish one prepared direct-body cell edit, touching only the owners of
    /// that cell. `candidate` is the edited cell alone in its table frame, as
    /// `direct_body_table_cell_candidate` prepares it, and the checked reader
    /// reads that frame for the facts of the cell. The other cells keep the
    /// facts the checked reader gave the aligned piece, as their models are
    /// the ones it read.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn apply_local_cell_edit(
        &mut self,
        py: Python<'_>,
        body_slot: usize,
        table_ordinal: usize,
        row: usize,
        cell: usize,
        candidate: oxoffice_docx_oxml::CT_Tbl,
        operation: &'static str,
        publication: Option<WordPublication>,
    ) -> PyResult<bool> {
        let Some(pieces) = &self.pieces else {
            return Ok(false);
        };
        if pieces.context.as_ref() != Some(&self.inner.body_item_context())
            || pieces.blocks.len() != self.inner.body_children().len()
        {
            return Ok(false);
        }
        let Some(piece) = pieces.blocks.get(body_slot) else {
            return Ok(false);
        };
        // The piece is spliced in place after the revision commit.
        if std::sync::Arc::strong_count(piece) != 1 || std::sync::Arc::weak_count(piece) != 0 {
            return Ok(false);
        }
        if !aligned_block(piece, &self.inner.body_children()[body_slot]) {
            return Ok(false);
        }
        let Some(before) = &piece.owners else {
            return Ok(false);
        };
        // Every physical cell must belong to this indexed table subtree, with
        // its standalone physical ordinal intact. Otherwise use the checked
        // publisher before preparing any revision or owner/index delta.
        if !before.local_table_membership
            || before.body_items != 1
            || !matches!(before.cell_paragraphs, Some((_, true)))
        {
            return Ok(false);
        }
        // A text-box count needs its own first-story base. Keep its checked fallback.
        if before.text_boxes != 0 || !before.facts.links.is_empty() {
            return Ok(false);
        }
        // Complete membership lists the cells in story order, so the cell's
        // story ordinal in the table alone is its position there.
        let Ok(local) = before
            .facts
            .cells
            .binary_search_by_key(&(row, cell), |(_, row, cell, _)| (*row, *cell))
        else {
            return Ok(false);
        };
        let Some(table_location) = self
            .indexes
            .body_tables
            .get(table_ordinal)
            .and_then(Option::as_ref)
            .cloned()
        else {
            return Ok(false);
        };
        let Some(table_index) = self.indexes.find(
            &self.owners,
            &WordOwnerLocation::Table(table_location.clone()),
        ) else {
            return Ok(false);
        };
        if self.owners[table_index].body_slot != Some(body_slot)
            || table_location.index_path().len() != 1
            || !matches!(&self.owners[table_index].read_facts, WordReadFacts::Table(rows)
                if rows.get(row).and_then(|cells| cells.get(cell)).is_some())
        {
            return Ok(false);
        }
        let Some(cell_indices) = self.indexes.cells_by_parent.get(&table_index) else {
            return Ok(false);
        };
        let Some(selected) = cell_indices.iter().copied().find(|index| {
            matches!(&self.owners[*index].location,
            WordOwnerLocation::Cell { row: r, cell: c, .. } if *r == row && *c == cell)
        }) else {
            return Ok(false);
        };
        let WordOwnerLocation::Cell {
            story: cell_story, ..
        } = &self.owners[selected].location
        else {
            return Ok(false);
        };
        if self
            .indexes
            .tables_by_parent
            .get(&selected)
            .is_some_and(|tables| !tables.is_empty())
        {
            return Ok(false);
        }
        let retire = publication.is_some();
        let paragraph_text = matches!(publication, Some(WordPublication::ParagraphText(_)));
        let text_selected = match &publication {
            None => selected,
            Some(WordPublication::CellText(table, r, c))
                if same_content_owner(table, &table_location) && *r == row && *c == cell =>
            {
                selected
            }
            Some(WordPublication::ParagraphText(location)) if location.index_path().len() == 1 => {
                let Some(index) = self.indexes.find(
                    &self.owners,
                    &WordOwnerLocation::Paragraph(location.clone()),
                ) else {
                    return Ok(false);
                };
                if self.owners[index].parent != Some(selected)
                    || !self
                        .indexes
                        .cell_paragraphs
                        .get(&(table_ordinal, row, cell))
                        .is_some_and(|paragraphs| {
                            paragraphs
                                .iter()
                                .any(|current| same_content_owner(current, location))
                        })
                {
                    return Ok(false);
                }
                index
            }
            _ => return Ok(false),
        };
        // The document's ordinal of the cell story, from the cell's own record.
        let stories = (cell_story.owner_index(), 0);
        let block = oxoffice_docx_oxml::BodyContent::Table(Box::new(candidate));
        let Some(after) =
            word_block_owners(&self.inner, &block).map_err(|error| docx_to_pyerr(py, error))?
        else {
            return Ok(false);
        };
        let Some((frame_routes, true)) = &after.cell_paragraphs else {
            return Ok(false);
        };
        let (Some(routes), Some((_, _, WordReadFacts::Table(frame_rows)))) =
            (frame_routes.get(&(0, 0, 0)), after.facts.items.first())
        else {
            return Ok(false);
        };
        // The frame holds this cell alone, with no nested table, text box or link.
        if !after.local_table_membership
            || after.facts.cells.len() != 1
            || after.body_items != 1
            || after.cell_stories != 1
            || after.text_boxes != 0
            || after.tables != 1
            || !after.facts.links.is_empty()
            || frame_rows.len() != 1
            || frame_rows[0].len() != 1
        {
            return Ok(false);
        }
        let paragraph_count = frame_rows[0][0];
        let item_base = table_location.index_path()[0];
        let body = table_location.story();
        let place = |location: &oxoffice_docx::ContentLocation| {
            placed_route(location, body, (item_base, stories))
        };
        let mut facts = after.facts.clone();
        for (location, slot, _) in &mut facts.items {
            *location = place(location);
            *slot = slot.map(|_| body_slot);
        }
        for (table, r, c, story) in &mut facts.cells {
            *table = place(table);
            (*r, *c) = (row, cell);
            *story = placed_story(story, stories);
        }
        for (location, _, _) in &mut facts.paragraphs {
            *location = place(location);
        }
        let mut addresses = word_piece_owner_addresses(facts, 0);
        // The frame's table owner is the table's own record. Keep the cell and
        // what it holds, with the cell first and parents among them.
        if !matches!(
            addresses.first().map(|address| &address.location),
            Some(WordOwnerLocation::Table(_))
        ) || !matches!(
            addresses.get(1),
            Some(WordOwnerAddress {
                location: WordOwnerLocation::Cell { .. },
                parent: Some(0),
                ..
            })
        ) || addresses[2..]
            .iter()
            .any(|address| !matches!(address.parent, Some(parent) if parent >= 1))
        {
            return Ok(false);
        }
        addresses.remove(0);
        for address in &mut addresses {
            address.parent = address.parent.and_then(|parent| parent.checked_sub(1));
        }
        // Walk indexed children of the cell, never the table's or the document's.
        let mut previous = Vec::new();
        let mut pending = vec![selected];
        while let Some(index) = pending.pop() {
            previous.push(index);
            for children in [
                self.indexes.cells_by_parent.get(&index),
                self.indexes.paragraphs_by_parent.get(&index),
                self.indexes.runs_by_parent.get(&index),
                self.indexes.tables_by_parent.get(&index),
            ]
            .into_iter()
            .flatten()
            {
                pending.extend(children);
            }
        }
        let local_positions = previous
            .iter()
            .enumerate()
            .map(|(local, global)| (*global, local))
            .collect::<HashMap<_, _>>();
        let mut old_addresses = Vec::with_capacity(previous.len());
        for index in &previous {
            let old = &self.owners[*index];
            old_addresses.push(WordOwnerAddress {
                location: old.location.clone(),
                parent: old
                    .parent
                    .and_then(|parent| local_positions.get(&parent).copied()),
                body_slot: old.body_slot,
                projection_ordinal: old.projection_ordinal,
                read_facts: old.read_facts.clone(),
            });
        }
        // Ordinal joins survive a first serialized tcPr shifting raw item indices.
        let key = |addresses: &[WordOwnerAddress], index: usize| {
            let address = &addresses[index];
            let paragraph = match address.location {
                WordOwnerLocation::Paragraph(_) => address.projection_ordinal,
                WordOwnerLocation::Run(..) => address
                    .parent
                    .and_then(|parent| addresses[parent].projection_ordinal),
                _ => None,
            };
            (property_owner_key(addresses, index), paragraph)
        };
        let mut buckets = HashMap::<_, Vec<usize>>::new();
        for index in 0..old_addresses.len() {
            buckets
                .entry(key(&old_addresses, index))
                .or_default()
                .push(index);
        }
        let mut cursors = HashMap::<_, usize>::new();
        let mut matched = Vec::with_capacity(addresses.len());
        let mut targets = Vec::with_capacity(addresses.len());
        let mut newborn = 0;
        for index in 0..addresses.len() {
            let key = key(&addresses, index);
            let cursor = cursors.entry(key.clone()).or_default();
            let old = buckets
                .get(&key)
                .and_then(|bucket| bucket.get(*cursor))
                .copied();
            *cursor += 1;
            matched.push(old.map(|local| previous[local]));
            targets.push(old.map(|local| previous[local]).unwrap_or_else(|| {
                let target = self.owners.len() + newborn;
                newborn += 1;
                target
            }));
        }
        if targets.first() != Some(&selected)
            || buckets
                .iter()
                .any(|(key, old)| cursors.get(key).copied().unwrap_or(0) < old.len())
        {
            return Ok(false);
        }
        if !retire && newborn != 0 {
            return Ok(false);
        }
        let mut memberships = Vec::new();
        let mut route_buckets = HashMap::<u64, Vec<usize>>::new();
        for index in &previous {
            let hash = self.owners[*index].location.route_hash();
            route_buckets.entry(hash).or_insert_with(|| {
                self.indexes
                    .by_route
                    .get(&hash)
                    .cloned()
                    .unwrap_or_default()
            });
        }
        for (index, address) in addresses.iter_mut().enumerate() {
            // The cell's parent is the table, outside the replaced owners.
            address.parent = Some(address.parent.map_or(table_index, |parent| targets[parent]));
            let hash = address.location.route_hash();
            route_buckets.entry(hash).or_insert_with(|| {
                self.indexes
                    .by_route
                    .get(&hash)
                    .cloned()
                    .unwrap_or_default()
            });
            memberships.push((hash, targets[index]));
        }
        for bucket in route_buckets.values_mut() {
            bucket.retain(|index| !local_positions.contains_key(index));
        }
        for (hash, index) in memberships {
            route_buckets
                .get_mut(&hash)
                .expect("prepared bucket")
                .push(index);
        }
        let mut parent_buckets = [
            HashMap::<usize, Vec<usize>>::new(),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        ];
        // The table keeps its list of cells, and the cell keeps its record.
        for (index, address) in addresses.iter().enumerate().skip(1) {
            if let Some(parent) = address.parent {
                let family = match address.location {
                    WordOwnerLocation::Cell { .. } => 0,
                    WordOwnerLocation::Paragraph(_) => 1,
                    WordOwnerLocation::Run(..) => 2,
                    WordOwnerLocation::Table(_) => 3,
                    _ => return Ok(false),
                };
                parent_buckets[family]
                    .entry(parent)
                    .or_default()
                    .push(targets[index]);
            }
        }
        let cell_routes = routes.iter().map(&place).collect::<Vec<_>>();
        let mut births = Vec::new();
        let mut retained = Vec::new();
        let mut retired = Vec::new();
        for (index, address) in addresses.iter().enumerate() {
            let old = matched[index].map(|index| &self.owners[index]);
            let changed = retire
                && matched[index].is_some_and(|index| {
                    let mut parent = self.owners[index].parent;
                    while let Some(index) = parent {
                        if index == text_selected {
                            return true;
                        }
                        parent = self.owners[index].parent;
                    }
                    false
                });
            let old_states = old.map(|owner| owner.states());
            let mut states = Vec::new();
            for (slot, scope) in address.location.scopes().iter().enumerate() {
                let replace = changed
                    || retire
                        && matched[index] == Some(text_selected)
                        && if paragraph_text { slot == 1 } else { slot != 0 };
                let state = if replace {
                    None
                } else {
                    old_states.as_ref().map(|states| states[slot].clone())
                };
                if state.is_none() {
                    births.push(*scope);
                    if let Some(states) = &old_states {
                        retired.push(states[slot].clone());
                    }
                }
                states.push(state);
            }
            retained.push(states);
        }
        if retire {
            retired.push(
                self.snapshot_scope
                    .as_ref()
                    .expect("active authority")
                    .clone(),
            );
            births.push(RevisionScope::Text);
        }
        // The piece facts of the cell, in the numbering of the table alone.
        let piece_place = |location: &oxoffice_docx::ContentLocation| {
            placed_route(location, location.story(), (0, (local, 0)))
        };
        let piece_paragraphs = after
            .facts
            .paragraphs
            .iter()
            .map(|(location, text, runs)| (piece_place(location), text.clone(), runs.clone()))
            .collect::<Vec<_>>();
        let piece_story = placed_story(&after.facts.cells[0].3, (local, 0));
        let piece_routes = routes.iter().map(piece_place).collect::<Vec<_>>();
        // The paragraphs are those of the cell stories, in story order.
        let ordinal = |location: &oxoffice_docx::ContentLocation| location.story().owner_index();
        let paragraphs = &before.facts.paragraphs;
        let paragraph_range = paragraphs
            .partition_point(|(location, _, _)| ordinal(location) < local)
            ..paragraphs.partition_point(|(location, _, _)| ordinal(location) <= local);
        let holds_field = after.holds_field;
        let oxoffice_docx_oxml::BodyContent::Table(frame) = block else {
            unreachable!()
        };
        let piece_cell = frame.rows[0].cells[0].clone();
        let fail = |_| {
            pyo3::exceptions::PyMemoryError::new_err("local cell publication allocation failed")
        };
        self.owners.try_reserve(newborn).map_err(fail)?;
        self.indexes
            .by_generation
            .try_reserve(newborn)
            .map_err(fail)?;
        self.indexes
            .by_route
            .try_reserve(route_buckets.len())
            .map_err(fail)?;
        self.indexes
            .paragraphs_by_parent
            .try_reserve(parent_buckets[1].len())
            .map_err(fail)?;
        self.indexes
            .runs_by_parent
            .try_reserve(parent_buckets[2].len())
            .map_err(fail)?;
        {
            let piece = std::sync::Arc::get_mut(
                &mut self.pieces.as_mut().expect("active pieces").blocks[body_slot],
            )
            .expect("a piece checked unique");
            let owners = piece.owners.as_mut().expect("aligned piece owners");
            owners
                .facts
                .paragraphs
                .try_reserve(piece_paragraphs.len())
                .map_err(fail)?;
        }
        let revision = if retire {
            Some(
                self.revisions
                    .prepare_scopes(operation, &retired, &births)
                    .map_err(revision_to_pyerr)?,
            )
        } else {
            None
        };
        let mut born = 0;
        let states = retained
            .into_iter()
            .flatten()
            .map(|state| {
                state.unwrap_or_else(|| {
                    let state = revision
                        .as_ref()
                        .expect("text births")
                        .born(born)
                        .expect("prepared owner birth");
                    born += 1;
                    state
                })
            })
            .collect::<Vec<_>>();
        let snapshot = revision
            .as_ref()
            .map(|revision| revision.born(born).expect("prepared snapshot"));
        let owners = word_owners_with_states(addresses, &states);
        if let Some(revision) = revision {
            self.revisions
                .commit_scopes(revision)
                .map_err(revision_to_pyerr)?;
        }
        // No recoverably fallible operation follows the revision commit.
        self.inner
            .commit_direct_body_table_cell(body_slot, row, cell, *frame);
        for index in &previous {
            self.indexes
                .by_generation
                .remove(&self.owners[*index].owner.generation());
        }
        for (owner, target) in owners.into_iter().zip(targets) {
            self.indexes
                .by_generation
                .insert(owner.owner.generation(), target);
            if target == self.owners.len() {
                self.owners.push(owner);
            } else {
                self.owners[target] = owner;
            }
        }
        if let WordReadFacts::Table(rows) = &mut self.owners[table_index].read_facts {
            rows[row][cell] = paragraph_count;
        }
        for (hash, bucket) in route_buckets {
            if bucket.is_empty() {
                self.indexes.by_route.remove(&hash);
            } else {
                self.indexes.by_route.insert(hash, bucket);
            }
        }
        for (map, prepared) in [
            &mut self.indexes.cells_by_parent,
            &mut self.indexes.paragraphs_by_parent,
            &mut self.indexes.runs_by_parent,
            &mut self.indexes.tables_by_parent,
        ]
        .into_iter()
        .zip(parent_buckets)
        {
            for index in &previous {
                map.remove(index);
            }
            map.extend(prepared);
        }
        self.indexes
            .cell_paragraphs
            .insert((table_ordinal, row, cell), cell_routes);
        if let Some(snapshot) = snapshot {
            self.snapshot_scope = Some(snapshot);
        }
        let piece = std::sync::Arc::get_mut(
            &mut self.pieces.as_mut().expect("active pieces").blocks[body_slot],
        )
        .expect("a piece checked unique");
        if let oxoffice_docx_oxml::BodyContent::Table(table) =
            std::sync::Arc::make_mut(&mut piece.block)
        {
            table.rows[row].cells[cell] = piece_cell;
        }
        let owners = piece.owners.as_mut().expect("aligned piece owners");
        // The inventory reads the table and body items of a piece, here the
        // table's own item. A full read also lists the paragraphs of the cell
        // stories, which a body splice leaves out. Keep no stale ones.
        owners.facts.items.truncate(1);
        owners
            .facts
            .paragraphs
            .splice(paragraph_range, piece_paragraphs);
        owners.facts.cells[local].3 = piece_story;
        if let Some((_, _, WordReadFacts::Table(rows))) = owners.facts.items.first_mut() {
            rows[row][cell] = paragraph_count;
        }
        if let Some((routes, _)) = &mut owners.cell_paragraphs {
            routes.insert((0, row, cell), piece_routes);
        }
        // A field elsewhere in the table may remain. The flag may stay set
        // until the next full read of the table, which a set flag asks for.
        owners.holds_field |= holds_field;
        #[cfg(debug_assertions)]
        self.assert_local_cell_publication(body_slot, table_index, table_ordinal);
        Ok(true)
    }

    /// Debug proof that a local cell publication leaves what a full read of
    /// the table gives: the piece facts the inventory reads, body story
    /// fingerprints aside, and the inventory records and cell paragraph
    /// routes of its owners.
    #[cfg(debug_assertions)]
    fn assert_local_cell_publication(
        &self,
        body_slot: usize,
        table_index: usize,
        table_ordinal: usize,
    ) {
        let fresh = word_block_owners(&self.inner, &self.inner.body_children()[body_slot])
            .expect("a published table reads")
            .expect("a published table has owner facts");
        let piece = self.pieces.as_ref().expect("active pieces").blocks[body_slot]
            .owners
            .as_ref()
            .expect("piece owners");
        let content = |location: &oxoffice_docx::ContentLocation| {
            if location.story().kind() == oxoffice_docx::StoryKind::Body {
                unfingerprinted_location(location)
            } else {
                location.clone()
            }
        };
        let facts = |facts: &WordOwnerFacts| {
            (
                facts
                    .items
                    .iter()
                    .filter(|(location, _, _)| {
                        location.story().kind() == oxoffice_docx::StoryKind::Body
                            || location.item_kind() == oxoffice_docx::StoryItemKind::Table
                    })
                    .map(|(location, slot, read)| (content(location), *slot, read.clone()))
                    .collect::<Vec<_>>(),
                facts
                    .cells
                    .iter()
                    .map(|(table, row, cell, story)| (content(table), *row, *cell, story.clone()))
                    .collect::<Vec<_>>(),
                facts.paragraphs.clone(),
                facts.links.clone(),
            )
        };
        assert_eq!(
            facts(&piece.facts),
            facts(&fresh.facts),
            "local cell piece facts"
        );
        assert_eq!(
            (
                &piece.relationships,
                piece.body_items,
                piece.cell_stories,
                piece.text_boxes,
                &piece.paragraphs,
                piece.tables,
                piece.local_table_membership,
                &piece.cell_paragraphs,
            ),
            (
                &fresh.relationships,
                fresh.body_items,
                fresh.cell_stories,
                fresh.text_boxes,
                &fresh.paragraphs,
                fresh.tables,
                fresh.local_table_membership,
                &fresh.cell_paragraphs,
            ),
            "local cell piece counts"
        );
        assert!(piece.holds_field || !fresh.holds_field);
        let WordOwnerLocation::Table(location) = &self.owners[table_index].location else {
            panic!("a local cell table owner")
        };
        let cells = &self.indexes.cells_by_parent[&table_index];
        let base = cells
            .iter()
            .filter_map(|index| match &self.owners[*index].location {
                WordOwnerLocation::Cell { story, .. } => Some(story.owner_index()),
                _ => None,
            })
            .min()
            .expect("table cells");
        let stories = (base, 0);
        let place = |route: &oxoffice_docx::ContentLocation| {
            placed_route(route, location.story(), (location.index_path()[0], stories))
        };
        let mut placed = fresh.facts.clone();
        for (route, slot, _) in &mut placed.items {
            *route = place(route);
            *slot = slot.map(|_| body_slot);
        }
        for (table, _, _, story) in &mut placed.cells {
            *table = place(table);
            *story = placed_story(story, stories);
        }
        for (route, _, _) in &mut placed.paragraphs {
            *route = place(route);
        }
        let expected = word_piece_owner_addresses(placed, 0);
        let record = |address: &WordOwnerAddress, parent: Option<&WordOwnerLocation>| {
            format!(
                "{:?}",
                (
                    owner_location_without_body(&address.location),
                    parent.map(owner_location_without_body),
                    address.body_slot,
                    address.projection_ordinal,
                    &address.read_facts,
                )
            )
        };
        let mut wanted = expected
            .iter()
            .map(|address| {
                let mut address = body_unfingerprinted_address(address);
                if address.parent.is_none() {
                    address.projection_ordinal = Some(table_ordinal);
                }
                record(
                    &address,
                    address.parent.map(|parent| &expected[parent].location),
                )
            })
            .collect::<Vec<_>>();
        let mut actual = Vec::new();
        let mut pending = vec![table_index];
        while let Some(index) = pending.pop() {
            let owner = &self.owners[index];
            let address = WordOwnerAddress {
                location: owner.location.clone(),
                parent: owner.parent,
                body_slot: owner.body_slot,
                projection_ordinal: owner.projection_ordinal,
                read_facts: owner.read_facts.clone(),
            };
            let parent = owner.parent.map(|parent| &self.owners[parent].location);
            actual.push(record(&address, parent));
            for children in [
                self.indexes.cells_by_parent.get(&index),
                self.indexes.paragraphs_by_parent.get(&index),
                self.indexes.runs_by_parent.get(&index),
                self.indexes.tables_by_parent.get(&index),
            ]
            .into_iter()
            .flatten()
            {
                pending.extend(children);
            }
        }
        wanted.sort();
        actual.sort();
        assert_eq!(actual, wanted, "local cell owner records");
        if let Some((routes, _)) = &fresh.cell_paragraphs {
            for ((_, row, cell), routes) in routes {
                assert_eq!(
                    self.indexes
                        .cell_paragraphs
                        .get(&(table_ordinal, *row, *cell)),
                    Some(&routes.iter().map(place).collect::<Vec<_>>()),
                    "local cell paragraph routes"
                );
            }
        }
    }

    /// Keep live handles when a property edit only moves existing physical
    /// addresses. A real insert, removal or replacement keeps the conservative
    /// publication.
    pub(crate) fn adopt_property_edit(
        &mut self,
        py: Python<'_>,
        candidate: oxoffice_docx::Document,
        operation: &'static str,
    ) -> PyResult<()> {
        let (pieces, inventory) = self
            .pieces
            .as_ref()
            .expect("active pieces")
            .assemble(&candidate);
        let (locations, projections) = match inventory {
            Some((locations, projections)) => {
                #[cfg(debug_assertions)]
                assert_word_pieces_rebuild(&candidate, &locations, &projections);
                (locations, Some(projections))
            }
            None => (
                word_owner_locations(&candidate, None).map_err(|error| docx_to_pyerr(py, error))?,
                None,
            ),
        };
        if let Some(owners) = relocated_property_owners(&self.owners, &locations) {
            let projections = projections.unwrap_or_else(|| WordIndexProjections::of(&candidate));
            let indexes = WordOwnerIndexes::prepare(&owners, projections)?;
            self.pending_pieces = Some(pieces);
            self.owners = owners;
            self.indexes = indexes;
            self.install_inner(candidate);
            return Ok(());
        }
        self.publish_owner_candidate(py, candidate, operation, WordPublication::Conservative)
    }

    fn remove_note_by_id(
        &mut self,
        py: Python<'_>,
        id: &Bound<'_, PyAny>,
        endnote: bool,
    ) -> PyResult<()> {
        if !id.is_instance_of::<PyInt>() || id.is_instance_of::<PyBool>() {
            return Err(PyTypeError::new_err("note id must be a non-bool integer"));
        }
        let id = id.extract::<i32>()?;
        let mut candidate = self.inner.clone_for_staging();
        if endnote {
            candidate.remove_endnote(id)
        } else {
            candidate.remove_footnote(id)
        }
        .map_err(|e| docx_to_pyerr(py, e))?;
        self.publish_api_candidate(py, candidate, "Document.remove_note")
    }

    fn edit_content_control(
        &mut self,
        py: Python<'_>,
        location: &PyContentLocation,
        operation: &'static str,
        edit: impl FnOnce(
            &mut oxoffice_docx::Document,
            &oxoffice_docx::ContentLocation,
        ) -> oxoffice_docx::Result<()>
        + Send,
    ) -> PyResult<()> {
        let location = location.native(py, self)?;
        let mut candidate = self.inner.clone_for_staging();
        py.detach(|| edit(&mut candidate, &location))
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, operation)
    }

    fn publish_api_candidate(
        &mut self,
        py: Python<'_>,
        candidate: oxoffice_docx::Document,
        operation: &'static str,
    ) -> PyResult<()> {
        if self
            .inner
            .saves_as(&candidate)
            .map_err(|e| docx_to_pyerr(py, e))?
        {
            return Ok(());
        }
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            operation,
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }
    fn publish_form_candidate(
        &mut self,
        py: Python<'_>,
        candidate: oxoffice_docx::Document,
        operation: &'static str,
        publication: WordPublication,
    ) -> PyResult<()> {
        if self
            .inner
            .saves_as(&candidate)
            .map_err(|e| docx_to_pyerr(py, e))?
        {
            return Ok(());
        }
        let prepared = self.prepare_word_revision(py, &candidate, operation, publication)?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }
    fn set_control_value(
        &mut self,
        py: Python<'_>,
        name: &str,
        value: &str,
        by_tag: bool,
    ) -> PyResult<usize> {
        let missing = if by_tag {
            self.inner.content_controls_by_tag(name).is_empty()
        } else {
            self.inner.content_controls_by_alias(name).is_empty()
        };
        if missing {
            return Err(PyKeyError::new_err(name.to_owned()));
        }
        let mut candidate = self.inner.clone_for_staging();
        let count = if by_tag {
            candidate.set_content_control_value_by_tag(name, value)
        } else {
            candidate.set_content_control_value_by_alias(name, value)
        }
        .map_err(|error| match error {
            oxoffice_docx::Error::Oxml(oxoffice_docx_oxml::OxmlError::InvalidValue(message)) => {
                PyValueError::new_err(message)
            }
            other => docx_to_pyerr(py, other),
        })?;
        self.publish_api_candidate(py, candidate, "Document.set_content_control_value")?;
        Ok(count)
    }

    fn create_note(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        reference: &Bound<'_, PyAny>,
        text: &str,
        endnote: bool,
    ) -> PyResult<Py<PyNote>> {
        let owner = slf.clone().unbind();
        let (location, run_index) = if let Ok(reference) = reference.cast::<PyParagraph>() {
            let paragraph = reference.borrow();
            if !paragraph.belongs_to(py, &owner) {
                return Err(PyValueError::new_err(
                    "note reference belongs to a different document",
                ));
            }
            let location = paragraph.validate(py)?;
            let document = slf.borrow();
            let run_index = location
                .read(&document.inner, |paragraph| paragraph.run_count())
                .map_err(|error| docx_to_pyerr(py, error))?;
            (
                location
                    .native(&document.inner)
                    .map_err(|error| docx_to_pyerr(py, error))?,
                run_index,
            )
        } else if let Ok(reference) = reference.cast::<crate::run::PyRun>() {
            let run = reference.borrow();
            if !run.belongs_to(py, &owner) {
                return Err(PyValueError::new_err(
                    "note reference belongs to a different document",
                ));
            }
            let (location, index) = run.validate(py)?;
            let document = slf.borrow();
            (
                location
                    .native(&document.inner)
                    .map_err(|error| docx_to_pyerr(py, error))?,
                index + 1,
            )
        } else {
            return Err(PyTypeError::new_err(
                "note reference must be a Paragraph or Run",
            ));
        };
        let mut document = slf.borrow_mut();
        let mut candidate = document.inner.clone_for_staging();
        let position = oxoffice_docx::StoryRunPosition {
            location,
            run_index,
        };
        let id = if endnote {
            candidate.create_endnote_at(&position, text)
        } else {
            candidate.create_footnote_at(&position, text)
        }
        .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.add_note",
            crate::document::WordPublication::ParagraphAppend(position.location.clone()),
        )?;
        let path = document
            .revisions
            .capture_prepared(&prepared.revision, smallvec![])
            .map_err(revision_to_pyerr)?;
        let result = Py::new(
            py,
            PyNote {
                document: owner,
                id,
                endnote,
                path,
            },
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(result)
    }

    fn from_document(inner: oxoffice_docx::Document, py: Python<'_>) -> PyResult<Self> {
        // Keep all eager fallible checks, without building dormant read facts.
        let (_, _, paragraphs, _) = inner
            .story_owner_snapshots()
            .map_err(|error| docx_to_pyerr(py, error))?;
        for (_, xml) in paragraphs {
            oxoffice_docx_oxml::CT_P::from_xml_fragment(&xml)
                .map_err(|error| docx_to_pyerr(py, error.into()))?;
        }
        Ok(Self {
            inner,
            revisions: RevisionCounter::new(),
            owners: Vec::new(),
            indexes: WordOwnerIndexes::prepare(
                &[],
                WordIndexProjections {
                    body_paragraphs: None,
                    cell_paragraphs: HashMap::new(),
                    table_slots: Vec::new(),
                },
            )?,
            snapshot_scope: None,
            removal_scope: None,
            pieces: None,
            pending_pieces: None,
        })
    }

    fn ensure_word_authority(&mut self, py: Python<'_>) -> PyResult<()> {
        if self.snapshot_scope.is_some() {
            return Ok(());
        }
        let (locations, projections, pieces) = validate_word_facts(&self.inner, py)?;
        let mut scopes = Vec::new();
        scopes
            .try_reserve(
                locations
                    .iter()
                    .map(|address| address.location.scopes().len())
                    .sum::<usize>()
                    + 2,
            )
            .map_err(|_| {
                pyo3::exceptions::PyMemoryError::new_err("Word scope allocation failed")
            })?;
        for address in &locations {
            scopes.extend_from_slice(address.location.scopes());
        }
        scopes.push(RevisionScope::Text);
        scopes.push(RevisionScope::Document);
        let mut owners = Vec::new();
        owners.try_reserve(locations.len()).map_err(|_| {
            pyo3::exceptions::PyMemoryError::new_err("Word owner allocation failed")
        })?;
        let mut indexes = WordOwnerIndexes::prepare_addresses(&locations, projections)?;
        // This is the last recoverably fallible step. All packet storage is reserved.
        let states = self
            .revisions
            .initialize_scopes(&scopes)
            .map_err(revision_to_pyerr)?;
        let removal_scope = states.last().expect("removal epoch allocated").clone();
        let snapshot_scope = states[states.len() - 2].clone();
        append_word_owners_with_states(&mut owners, locations, &states[..states.len() - 2]);
        for (index, owner) in owners.iter().enumerate() {
            indexes
                .by_generation
                .insert(owner.owner.generation(), index);
        }
        self.owners = owners;
        self.indexes = indexes;
        self.snapshot_scope = Some(snapshot_scope);
        self.removal_scope = Some(removal_scope);
        self.pieces = Some(pieces);
        Ok(())
    }

    /// The live native owner that a `Story` snapshot names.
    ///
    /// A snapshot carries no fingerprint, so it resolves by kind, part name
    /// and owner index against the document as it is now.
    fn native_story(&self, py: Python<'_>, story: &PyStory) -> PyResult<oxoffice_docx::StoryId> {
        self.validate_owned_story(py, story)?;
        self.inner
            .stories()
            .map_err(|error| docx_to_pyerr(py, error))?
            .into_iter()
            .find(|candidate| story_snapshot(candidate) == *story)
            .ok_or_else(|| {
                docx_to_pyerr(
                    py,
                    oxoffice_docx::Error::Other(format!(
                        "document has no {} story {} at owner index {}",
                        story.kind, story.part_name, story.owner_index
                    )),
                )
            })
    }

    /// The live story of a `Hyperlink` record and the position of its link.
    ///
    /// A captured snapshot resolves through its current checked physical owner
    /// state, independently of the source Story snapshot's global epoch.
    /// A constructed record resolves only when exactly one current link matches.
    fn native_hyperlink(
        &self,
        py: Python<'_>,
        hyperlink: &PyHyperlink,
    ) -> PyResult<(oxoffice_docx::StoryId, usize)> {
        if let Some((path, state)) = &hyperlink.owner {
            if !path.belongs_to(&self.revisions) {
                return Err(PyValueError::new_err(
                    "hyperlink belongs to a different document",
                ));
            }
            path.validate_scopes(
                &self.revisions,
                "hyperlink",
                "re-fetch it with document.hyperlinks.",
            )
            .map_err(|error| crate::stale_to_pyerr(py, error))?;
            let owner = self
                .owners
                .iter()
                .find(|owner| owner.owner.same_state(state))
                .ok_or_else(|| {
                    PyValueError::new_err("hyperlink physical owner is no longer current")
                })?;
            let WordOwnerLocation::Hyperlink(location, _) = &owner.location else {
                return Err(PyValueError::new_err(
                    "hyperlink physical owner has the wrong kind",
                ));
            };
            let story = self
                .inner
                .stories()
                .map_err(|error| docx_to_pyerr(py, error))?
                .into_iter()
                .find(|story| same_story_owner(story, location.story()))
                .ok_or_else(|| {
                    PyValueError::new_err("hyperlink story owner is no longer current")
                })?;
            let index = owner
                .projection_ordinal
                .ok_or_else(|| PyValueError::new_err("hyperlink source position is absent"))?;
            return Ok((story, index));
        }
        let story = self.native_story(py, &hyperlink.story)?;
        let links = self
            .inner
            .story_links(&story)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let matches =
            |(location, link): &(oxoffice_docx::ContentLocation, oxoffice_docx::LinkInfo)| {
                location.index_path() == hyperlink.index_path.as_slice()
                    && link.text == hyperlink.text
                    && link.url == hyperlink.url
                    && link.anchor == hyperlink.anchor
                    && link.rel_id == hyperlink.relationship_id
            };
        let index =
            match hyperlink.position {
                Some((index, layout)) => links
                    .get(index)
                    .filter(|entry| {
                        matches(entry)
                            && story_link_layout(links.iter().map(|(location, link)| {
                                (location.index_path(), link.text.as_str())
                            })) == layout
                    })
                    .map(|_| index),
                None => {
                    let mut found = links
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| matches(entry))
                        .map(|(index, _)| index);
                    match (found.next(), found.next()) {
                        (Some(index), None) => Some(index),
                        _ => None,
                    }
                }
            };
        index.map(|index| (story, index)).ok_or_else(|| {
            docx_to_pyerr(
                py,
                oxoffice_docx::Error::Other(
                    "the hyperlink does not match exactly one link of the document, re-fetch it with document.hyperlinks"
                        .to_owned(),
                ),
            )
        })
    }

    fn body_story(&self, py: Python<'_>) -> PyResult<oxoffice_docx::StoryId> {
        self.inner
            .stories()
            .map_err(|error| docx_to_pyerr(py, error))?
            .into_iter()
            .find(|story| story.kind() == oxoffice_docx::StoryKind::Body)
            .ok_or_else(|| {
                docx_to_pyerr(
                    py,
                    oxoffice_docx::Error::Other("document body story is missing".to_owned()),
                )
            })
    }

    fn native_location(
        &self,
        py: Python<'_>,
        item: &PyStoryItem,
    ) -> PyResult<oxoffice_docx::ContentLocation> {
        self.validate_owned_item(py, item)?;
        let story = self.native_story(py, &item.story)?;
        if item.binding.is_none()
            && item.story.binding.is_none()
            && item.revision != self.revisions.current()
        {
            return Err(crate::stale_to_pyerr(
                py,
                StaleElementError {
                    element_kind: "story item".to_owned(),
                    captured_revision: item.revision,
                    current_revision: self.revisions.current(),
                    recovery_hint: "Causal provenance is unavailable for this explicit numeric selector. Re-fetch it with document.story_items.".to_owned(),
                },
            ));
        }
        Ok(oxoffice_docx::ContentLocation::new(
            story,
            story_item_kind_from_name(&item.kind)?,
            item.index_path.clone(),
        ))
    }

    fn body_location(
        &self,
        py: Python<'_>,
        index: usize,
    ) -> PyResult<oxoffice_docx::ContentLocation> {
        self.body_locations(py, &[index])?
            .pop()
            .ok_or_else(|| PyIndexError::new_err("content index out of range"))
    }

    fn body_locations(
        &self,
        py: Python<'_>,
        indices: &[usize],
    ) -> PyResult<Vec<oxoffice_docx::ContentLocation>> {
        let content_count = self.inner.content_count();
        if indices.iter().any(|index| *index > content_count) {
            return Err(PyIndexError::new_err("content index out of range"));
        }
        let story = self.body_story(py)?;
        let mut locations = Vec::with_capacity(indices.len());
        for index in indices {
            if *index == content_count {
                locations.push(oxoffice_docx::ContentLocation::end(story.clone()));
                continue;
            }
            let location = self
                .inner
                .direct_body_item_location(*index)
                .map_err(|error| docx_to_pyerr(py, error))?
                .filter(|location| location.story() == &story)
                .ok_or_else(|| {
                    docx_to_pyerr(
                        py,
                        oxoffice_docx::Error::Other(format!(
                            "direct body content at index {index} has no checked location"
                        )),
                    )
                })?;
            locations.push(location);
        }
        Ok(locations)
    }

    fn comment_story_position(
        &self,
        py: Python<'_>,
        position: &oxoffice_docx::StoryRunPosition,
        items: &mut HashMap<oxoffice_docx::ContentLocation, oxoffice_docx::StoryItemSnapshot>,
        document: &std::sync::Arc<Py<PyDocument>>,
    ) -> PyResult<PyStoryRunPosition> {
        if !items.contains_key(&position.location) {
            let snapshot = self
                .inner
                .story_range_paragraph_snapshot(&position.location)
                .map_err(|error| docx_to_pyerr(py, error))?;
            items.insert(position.location.clone(), snapshot);
        }
        let item = &items[&position.location];
        Ok(PyStoryRunPosition {
            item: PyStoryItem {
                story: self.owned_story_snapshot(item.location().story())?,
                kind: story_item_kind_name(item.location().item_kind()).to_owned(),
                index_path: item.location().index_path().to_vec(),
                direct_body_index: item.direct_body_index(),
                text: item.text().map(str::to_owned),
                xml: item.xml().to_vec(),
                revision: self.revisions.current(),
                binding: Some(self.capture_snapshot(smallvec![])?),
                document: Some(document.clone()),
            },
            run_index: position.run_index,
        })
    }

    fn story_item_snapshot(
        &self,
        py: Python<'_>,
        location: &oxoffice_docx::ContentLocation,
        document: std::sync::Arc<Py<PyDocument>>,
    ) -> PyResult<PyStoryItem> {
        let item = self
            .inner
            .standalone_story_item_snapshots()
            .map_err(|error| docx_to_pyerr(py, error))?
            .into_iter()
            .find(|item| item.location() == location)
            .ok_or_else(|| PyIndexError::new_err("inserted story item was not found"))?;
        Ok(PyStoryItem {
            story: self.owned_story_snapshot(item.location().story())?,
            kind: story_item_kind_name(item.location().item_kind()).to_owned(),
            index_path: item.location().index_path().to_vec(),
            direct_body_index: item.direct_body_index(),
            text: item.text().map(str::to_owned),
            xml: item.xml().to_vec(),
            revision: self.revisions.current(),
            binding: Some(self.capture_snapshot(smallvec![])?),
            document: Some(document),
        })
    }

    /// Snapshot a checked body paragraph, including a paragraph inside a block control.
    fn paragraph_story_item(py: Python<'_>, paragraph: &PyParagraph) -> PyResult<PyStoryItem> {
        let ParagraphLocation::Body(paragraph_index) = paragraph.validate(py)? else {
            return Err(PyValueError::new_err(
                "StoryRunPosition does not accept a table cell paragraph handle",
            ));
        };
        let document = paragraph.document.borrow(py);
        let location = document
            .inner
            .paragraph_story_location(paragraph_index)
            .map_err(|error| docx_to_pyerr(py, error))?
            .ok_or_else(|| PyIndexError::new_err("paragraph index out of range"))?;
        let owner = std::sync::Arc::new(paragraph.document.clone_ref(py));
        let [_, _] = location.index_path() else {
            return document.story_item_snapshot(py, &location, owner);
        };
        let item = document
            .inner
            .story_range_paragraph_snapshot(&location)
            .map_err(|error| docx_to_pyerr(py, error))?;
        Ok(PyStoryItem {
            story: document.owned_story_snapshot(item.location().story())?,
            kind: story_item_kind_name(item.location().item_kind()).to_owned(),
            index_path: item.location().index_path().to_vec(),
            direct_body_index: item.direct_body_index(),
            text: item.text().map(str::to_owned),
            xml: item.xml().to_vec(),
            revision: document.revisions.current(),
            binding: Some(document.capture_snapshot(smallvec![])?),
            document: Some(owner),
        })
    }

    fn direct_content_index(
        slf: &Py<Self>,
        py: Python<'_>,
        content: &Bound<'_, PyAny>,
        argument: &str,
    ) -> PyResult<usize> {
        if let Ok(paragraph) = content.cast::<PyParagraph>() {
            let paragraph = paragraph.borrow();
            if !paragraph.belongs_to(py, slf) {
                return Err(PyValueError::new_err(
                    "content handle belongs to a different document",
                ));
            }
            let paragraph_index = match paragraph.validate(py)? {
                crate::paragraph::ParagraphLocation::Body(index) => index,
                crate::paragraph::ParagraphLocation::Cell { .. }
                | crate::paragraph::ParagraphLocation::Related(_) => {
                    return Err(PyValueError::new_err(
                        "content handle is not a direct body child",
                    ));
                }
            };
            return slf
                .borrow(py)
                .inner
                .content_index_of_paragraph(paragraph_index)
                .ok_or_else(|| PyValueError::new_err("content handle is not a direct body child"));
        }
        if let Ok(table) = content.cast::<PyTable>() {
            let table = table.borrow();
            if !table.belongs_to(py, slf) {
                return Err(PyValueError::new_err(
                    "content handle belongs to a different document",
                ));
            }
            return table.direct_body_index(py);
        }
        Err(PyTypeError::new_err(format!(
            "{argument} must be a Paragraph or Table handle"
        )))
    }

    /// Split a run of the direct body paragraph at `body_index`. The revision
    /// advances only when a continuation run is created.
    fn split_body_run(
        &mut self,
        py: Python<'_>,
        body_index: usize,
        run_index: usize,
        character_offset: usize,
    ) -> PyResult<usize> {
        let paragraph_index = self.inner.paragraph_index_of_content(body_index);
        let run_count = |document: &oxoffice_docx::Document| {
            paragraph_index
                .and_then(|index| document.paragraph(index))
                .map(|paragraph| paragraph.run_count())
        };
        let before = run_count(&self.inner);
        let mut candidate = self.inner.clone_for_staging();
        let boundary = candidate
            .split_run(body_index, run_index, character_offset)
            .map_err(|error| docx_to_pyerr(py, error))?;
        if run_count(&candidate) != before {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.split_run",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(boundary)
    }

    /// Run a native mutation that reports how many things it changed.
    ///
    /// The GIL is released while it runs, and live handles are staled only
    /// when the count is nonzero.
    fn counted_mutation<F>(
        &mut self,
        py: Python<'_>,
        operation: &'static str,
        mutation: F,
    ) -> PyResult<usize>
    where
        F: FnOnce(&mut oxoffice_docx::Document) -> oxoffice_docx::Result<usize> + Send,
    {
        let mut candidate = self.inner.clone_for_staging();
        let count = py
            .detach(|| mutation(&mut candidate))
            .map_err(|error| docx_to_pyerr(py, error))?;
        if count > 0 {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                operation,
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(count)
    }

    /// `check` refuses a pair whose text is left out of its replacement's
    /// reach, as `Document::try_replace_all_checked` does.
    fn dormant_replacements(
        &mut self,
        py: Python<'_>,
        pairs: &[(&str, &str, Option<usize>)],
        batch: bool,
        check: Option<oxoffice_docx::ReplaceOptions>,
    ) -> PyResult<Option<(oxoffice_docx::Document, Vec<usize>)>> {
        let candidate = self.inner.clone_for_staging();
        let changed = match check {
            None => py
                .detach(|| candidate.try_replace_all_expected_owned(pairs))
                .map_err(|error| docx_to_pyerr(py, error))?
                .map_err(|mismatch| crate::replacement_count_to_pyerr(py, &mismatch, batch))?,
            Some(options) => py
                .detach(|| candidate.try_replace_all_checked_owned(pairs, options))
                .map_err(|error| docx_to_pyerr(py, error))?
                .map_err(|refusal| crate::replacement_refusal_to_pyerr(py, refusal, batch))?,
        };
        if let Some((candidate, _)) = &changed {
            drop(validate_word_facts(candidate, py)?);
        }
        Ok(changed)
    }

    fn commit_dormant_replacement(
        &mut self,
        candidate: oxoffice_docx::Document,
        operation: &'static str,
    ) -> PyResult<()> {
        assert!(self.snapshot_scope.is_none());
        let prepared = self
            .revisions
            .prepare_scopes(operation, &[], &[])
            .map_err(revision_to_pyerr)?;
        self.revisions
            .commit_scopes(prepared)
            .map_err(revision_to_pyerr)?;
        self.install_inner(candidate);
        Ok(())
    }

    /// Run an ordered replacement batch that publishes only when every
    /// expected count holds, and stale live handles only when it replaced
    /// something.
    fn expected_replacements(
        &mut self,
        py: Python<'_>,
        pairs: &[(&str, &str, Option<usize>)],
        batch: bool,
        check: Option<oxoffice_docx::ReplaceOptions>,
    ) -> PyResult<(oxoffice_docx::Document, Vec<usize>)> {
        let mut candidate = self.inner.clone_for_staging();
        let counts = match check {
            None => py
                .detach(|| candidate.try_replace_all_expected(pairs))
                .map_err(|error| docx_to_pyerr(py, error))?
                .map_err(|mismatch| crate::replacement_count_to_pyerr(py, &mismatch, batch))?,
            Some(options) => py
                .detach(|| candidate.try_replace_all_checked(pairs, options))
                .map_err(|error| docx_to_pyerr(py, error))?
                .map_err(|refusal| crate::replacement_refusal_to_pyerr(py, refusal, batch))?,
        };
        Ok((candidate, counts))
    }

    // Both checked item and physical cell routes share singular count/error and
    // publication policy. The concrete closures are the two actual consumers.
    pub(crate) fn scoped_replacement<F>(
        &mut self,
        py: Python<'_>,
        operation: &'static str,
        mutation: F,
    ) -> PyResult<usize>
    where
        F: FnOnce(
                oxoffice_docx::Document,
            ) -> oxoffice_docx::Result<
                Result<
                    Option<(oxoffice_docx::Document, usize)>,
                    oxoffice_docx::ReplacementCountMismatch,
                >,
            > + Send,
    {
        let candidate = self.inner.clone_for_staging();
        let changed = py
            .detach(|| mutation(candidate))
            .map_err(|error| docx_to_pyerr(py, error))?
            .map_err(|mismatch| crate::replacement_count_to_pyerr(py, &mismatch, false))?;
        let Some((candidate, count)) = changed else {
            return Ok(0);
        };
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            operation,
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(count)
    }

    /// Check the names and the section of one header or footer variant.
    fn section_story_target(
        &self,
        section_index: usize,
        kind: &str,
        variant: &str,
    ) -> PyResult<(oxoffice_docx::HeaderFooterKind, oxoffice_docx::HdrFtrType)> {
        let kind = header_footer_kind_from_name(kind)?;
        let variant = header_footer_variant_from_name(variant)?;
        if section_index >= self.inner.section_count() {
            return Err(PyIndexError::new_err("section index out of range"));
        }
        Ok((kind, variant))
    }
}

/// The date `date` and `undated` give a new comment or reply: the current
/// time when neither is given. Both together are refused.
fn comment_date(date: Option<&str>, undated: bool) -> PyResult<oxoffice_docx::CommentDate<'_>> {
    match (date, undated) {
        (Some(_), true) => Err(PyValueError::new_err(
            "date and undated=True contradict each other: pass one of them",
        )),
        (Some(date), false) => Ok(oxoffice_docx::CommentDate::At(date)),
        (None, true) => Ok(oxoffice_docx::CommentDate::Undated),
        (None, false) => Ok(oxoffice_docx::CommentDate::Now),
    }
}

fn story_kind_name(kind: oxoffice_docx::StoryKind) -> &'static str {
    match kind {
        oxoffice_docx::StoryKind::Body => "body",
        oxoffice_docx::StoryKind::TableCell => "table_cell",
        oxoffice_docx::StoryKind::Header => "header",
        oxoffice_docx::StoryKind::Footer => "footer",
        oxoffice_docx::StoryKind::Footnote => "footnote",
        oxoffice_docx::StoryKind::Endnote => "endnote",
        oxoffice_docx::StoryKind::Comment => "comment",
        oxoffice_docx::StoryKind::TextBox => "text_box",
        _ => "unknown",
    }
}

fn story_snapshot(story: &oxoffice_docx::StoryId) -> PyStory {
    PyStory {
        binding: None,
        kind: story_kind_name(story.kind()).to_owned(),
        part_name: story.part_name().to_owned(),
        owner_index: story.owner_index(),
    }
}

fn story_item_kind_name(kind: oxoffice_docx::StoryItemKind) -> &'static str {
    match kind {
        oxoffice_docx::StoryItemKind::Paragraph => "paragraph",
        oxoffice_docx::StoryItemKind::Table => "table",
        oxoffice_docx::StoryItemKind::ContentControl => "content_control",
        oxoffice_docx::StoryItemKind::Field => "field",
        oxoffice_docx::StoryItemKind::Drawing => "drawing",
        oxoffice_docx::StoryItemKind::PreservedNode => "preserved_node",
        _ => "unknown",
    }
}

fn story_item_kind_from_name(name: &str) -> PyResult<oxoffice_docx::StoryItemKind> {
    match name {
        "paragraph" => Ok(oxoffice_docx::StoryItemKind::Paragraph),
        "table" => Ok(oxoffice_docx::StoryItemKind::Table),
        "content_control" => Ok(oxoffice_docx::StoryItemKind::ContentControl),
        "field" => Ok(oxoffice_docx::StoryItemKind::Field),
        "drawing" => Ok(oxoffice_docx::StoryItemKind::Drawing),
        "preserved_node" => Ok(oxoffice_docx::StoryItemKind::PreservedNode),
        _ => Err(PyValueError::new_err(format!(
            "unsupported story item kind {name:?}"
        ))),
    }
}

fn header_footer_kind_from_name(name: &str) -> PyResult<oxoffice_docx::HeaderFooterKind> {
    match name {
        "header" => Ok(oxoffice_docx::HeaderFooterKind::Header),
        "footer" => Ok(oxoffice_docx::HeaderFooterKind::Footer),
        _ => Err(PyValueError::new_err(format!(
            "kind must be header or footer, not {name:?}"
        ))),
    }
}

fn header_footer_variant_from_name(name: &str) -> PyResult<oxoffice_docx::HdrFtrType> {
    // `HdrFtrType::from_str` reads every unknown name as the default variant.
    match name {
        "default" => Ok(oxoffice_docx::HdrFtrType::Default),
        "first" => Ok(oxoffice_docx::HdrFtrType::First),
        "even" => Ok(oxoffice_docx::HdrFtrType::Even),
        _ => Err(PyValueError::new_err(format!(
            "variant must be default, first or even, not {name:?}"
        ))),
    }
}

/// Extract a direct body index, naming the accepted forms on a type error.
fn body_index_argument(value: &Bound<'_, PyAny>, message: &'static str) -> PyResult<usize> {
    value.extract::<usize>().map_err(|error| {
        if error.is_instance_of::<PyTypeError>(value.py()) {
            PyTypeError::new_err(message)
        } else {
            error
        }
    })
}

pub(crate) fn checked_section_length(
    value: Option<&Bound<'_, PyAny>>,
    name: &str,
) -> PyResult<Option<i64>> {
    value
        .map(|value| {
            if value.is_instance_of::<PyBool>() {
                return Err(PyTypeError::new_err(format!(
                    "{name} must be an integer EMU length, not bool"
                )));
            }
            value.extract::<i64>()
        })
        .transpose()
}

// Public update_section has an explicit-unit contract. Other consumers of
// checked_section_length, including live Section setters, keep their EMU API.
fn checked_update_section_length(
    py: Python<'_>,
    value: Option<&Bound<'_, PyAny>>,
    name: &str,
) -> PyResult<Option<i64>> {
    if let Some(value) = value
        && !value.is_instance_of::<pyo3::types::PyInt>()
    {
        return Err(PyTypeError::new_err(format!(
            "{name} requires Length or zero; use Twips or Emu"
        )));
    }
    let emu = checked_section_length(value, name)?;
    if let Some(value) = value
        && emu != Some(0)
        && !value.is_instance(&py.import("oxoffice.docx.shared")?.getattr("Length")?)?
    {
        return Err(PyValueError::new_err(format!(
            "update_section {name} nonzero geometry requires explicit Length; use Twips(1440) or Emu(value)"
        )));
    }
    Ok(emu)
}

fn section_snapshot(section: oxoffice_docx::SectionRef<'_>) -> PySection {
    let (page_width, page_height) = section.page_size().map_or((None, None), |(width, height)| {
        (Some(width.to_emu()), Some(height.to_emu()))
    });
    let (margin_top, margin_right, margin_bottom, margin_left) =
        section
            .margins()
            .map_or((None, None, None, None), |(top, right, bottom, left)| {
                (
                    Some(top.to_emu()),
                    Some(right.to_emu()),
                    Some(bottom.to_emu()),
                    Some(left.to_emu()),
                )
            });
    let (column_count, column_spacing) =
        section.columns().map_or((None, None), |(count, spacing)| {
            (Some(count), Some(spacing.to_emu()))
        });
    let (header_distance, footer_distance) = section
        .header_footer_distance()
        .map_or((None, None), |(header, footer)| {
            (Some(header.to_emu()), Some(footer.to_emu()))
        });
    PySection {
        ordinal: section.ordinal(),
        is_final: section.is_final(),
        orientation: section.orientation().map(|value| value.to_str().to_owned()),
        page_width,
        page_height,
        margin_top,
        margin_right,
        margin_bottom,
        margin_left,
        gutter: section.gutter().map(oxoffice_docx::Length::to_emu),
        column_count,
        column_spacing,
        page_number_start: section.page_number_start(),
        header_distance,
        footer_distance,
        different_first_page: section.different_first_page(),
        break_type: section.break_type().map(|value| value.to_str().to_owned()),
    }
}

/// A section length the caller gave, its explicit value, or its layout default.
fn given_or_current_length(
    given: Option<i64>,
    current: Option<oxoffice_docx::Twips>,
    default: oxoffice_docx::Twips,
) -> oxoffice_docx::Length {
    match (given, current) {
        (Some(emu), _) => oxoffice_docx::Length::emu(emu),
        (None, Some(twips)) => oxoffice_docx::Length::twips(twips.0),
        (None, None) => oxoffice_docx::Length::twips(default.0),
    }
}

fn style_snapshot(style: oxoffice_docx::style::Style<'_>) -> PyStyle {
    PyStyle {
        style_id: style.style_id().to_owned(),
        name: style.name().map(str::to_owned),
        based_on: style.based_on().map(str::to_owned),
        style_type: style.style_type().to_str().to_owned(),
        linked_style: style.linked_style().map(str::to_owned),
        next_style: style.next_style().map(str::to_owned),
        priority: style.priority(),
        auto_redefine: style.auto_redefine(),
        hidden: style.hidden(),
        semi_hidden: style.semi_hidden(),
        unhide_when_used: style.unhide_when_used(),
        quick_format: style.quick_format(),
        locked: style.locked(),
        is_default: style.is_default(),
    }
}

#[pymethods]
impl PyDocument {
    #[getter]
    fn default_font_name(&self) -> Option<String> {
        self.inner.default_font_name().map(str::to_owned)
    }

    #[setter]
    fn set_default_font_name(&mut self, py: Python<'_>, value: Option<&str>) -> PyResult<()> {
        self.inner
            .set_default_font_name_value(value)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    #[getter]
    fn default_font_size(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        self.inner
            .default_font_size()
            .map(|value| crate::length_object(py, value))
            .transpose()
    }

    #[setter]
    fn set_default_font_size(&mut self, py: Python<'_>, value: Option<i64>) -> PyResult<()> {
        let value = value.map(oxoffice_docx::Length::emu);
        if value.is_some_and(|value| {
            value.to_emu() < 0 || u32::try_from(value.to_emu() / 6350).is_err()
        }) {
            return Err(PyValueError::new_err(
                "Document.default_font_size must fit nonnegative half-points",
            ));
        }
        self.inner
            .set_default_font_size_value(value)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    #[new]
    #[pyo3(signature = (path = None))]
    fn new(path: Option<&Bound<'_, PyAny>>, py: Python<'_>) -> PyResult<Self> {
        let inner = match path {
            Some(path) => open_document_input(py, path)?,
            None => oxoffice_docx::Document::new(),
        };
        Self::from_document(inner, py)
    }

    #[staticmethod]
    fn open(path: &Bound<'_, PyAny>, py: Python<'_>) -> PyResult<Self> {
        Self::from_document(open_document_input(py, path)?, py)
    }

    #[staticmethod]
    fn from_bytes(bytes: &[u8], py: Python<'_>) -> PyResult<Self> {
        oxoffice_docx::Document::from_bytes(bytes)
            .map_err(|error| docx_to_pyerr(py, error))
            .and_then(|inner| Self::from_document(inner, py))
    }

    #[staticmethod]
    #[pyo3(signature = (html, *, images=Vec::new()))]
    fn from_html<'py>(
        py: Python<'py>,
        html: &str,
        images: Vec<(String, Py<PyBytes>, String)>,
    ) -> PyResult<(Py<Self>, Bound<'py, PyTuple>)> {
        Self::imported_text(py, html, false, None, &images)
    }

    #[staticmethod]
    #[pyo3(signature = (path, *, images=Vec::new()))]
    fn open_html<'py>(
        py: Python<'py>,
        path: PathBuf,
        images: Vec<(String, Py<PyBytes>, String)>,
    ) -> PyResult<(Py<Self>, Bound<'py, PyTuple>)> {
        Self::imported_text(py, &read_import_text(path)?, false, None, &images)
    }

    #[staticmethod]
    #[pyo3(signature = (markdown, *, template=None, images=Vec::new()))]
    fn from_markdown<'py>(
        py: Python<'py>,
        markdown: &str,
        template: Option<&Bound<'py, PyAny>>,
        images: Vec<(String, Py<PyBytes>, String)>,
    ) -> PyResult<(Py<Self>, Bound<'py, PyTuple>)> {
        Self::imported_text(py, markdown, true, template, &images)
    }

    #[staticmethod]
    #[pyo3(signature = (path, *, template=None, images=Vec::new()))]
    fn open_markdown<'py>(
        py: Python<'py>,
        path: PathBuf,
        template: Option<&Bound<'py, PyAny>>,
        images: Vec<(String, Py<PyBytes>, String)>,
    ) -> PyResult<(Py<Self>, Bound<'py, PyTuple>)> {
        Self::imported_text(py, &read_import_text(path)?, true, template, &images)
    }

    #[pyo3(signature = (html, *, after, images=Vec::new()))]
    fn insert_html<'py>(
        slf: Py<Self>,
        py: Python<'py>,
        html: &str,
        after: PyRef<'_, PyParagraph>,
        images: Vec<(String, Py<PyBytes>, String)>,
    ) -> PyResult<Bound<'py, PyTuple>> {
        if !after.belongs_to(py, &slf) {
            return Err(PyValueError::new_err(
                "after must be a live paragraph of this document",
            ));
        }
        let paragraph = after.validate(py)?;
        let mut document = slf.borrow_mut(py);
        let location = paragraph
            .native(&document.inner)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let snapshots = document
            .inner
            .story_item_snapshots()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let direct = snapshots
            .iter()
            .filter(|item| {
                item.location().story() == location.story()
                    && item.is_direct_child()
                    && (location.story().kind() != oxoffice_docx::StoryKind::Body
                        || item.direct_body_index().is_some())
            })
            .collect::<Vec<_>>();
        let index = direct
            .iter()
            .position(|item| item.location() == &location)
            .ok_or_else(|| {
                PyValueError::new_err("after must be a supported direct-child paragraph")
            })?;
        let destination = direct.get(index + 1).map_or_else(
            || oxoffice_docx::ContentLocation::end(location.story().clone()),
            |item| item.location().clone(),
        );
        let resources = images
            .iter()
            .map(
                |(source, bytes, filename)| oxoffice_docx::HtmlImageResource {
                    source,
                    bytes: bytes.bind(py).as_bytes(),
                    filename,
                },
            )
            .collect::<Vec<_>>();
        let mut candidate = document.inner.clone_for_staging();
        let result = py
            .detach(|| candidate.insert_html_fragment(&destination, html, &resources))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let diagnostics = html_diagnostics(py, result.diagnostics)?;
        document.publish_api_candidate(py, candidate, "Document.insert_html")?;
        Ok(diagnostics)
    }

    /// Sign current typed content with DER or one unencrypted PKCS#8/X.509 PEM block.
    fn sign(
        &mut self,
        private_key: &[u8],
        certificate: &[u8],
        py: Python<'_>,
    ) -> PyResult<Py<pyo3::types::PyDict>> {
        let mut candidate = self.inner.clone_for_staging();
        let report = py
            .detach(|| candidate.sign(private_key, certificate))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let result = oxoffice_py_support::signature_report_to_py(py, &report)?;
        let prepared =
            self.prepare_word_revision(py, &candidate, "Document.sign", WordPublication::Signing)?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(result)
    }

    /// Verify serialized current content, without publishing state or asserting chain trust.
    fn verify_signatures(&self, py: Python<'_>) -> PyResult<Py<PyTuple>> {
        let reports = py
            .detach(|| {
                let mut current = self.inner.clone_for_staging();
                let bytes = current.to_bytes()?;
                oxoffice_docx::Document::from_bytes(&bytes)?
                    .verify_signatures()
                    .map_err(oxoffice_docx::Error::from)
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        let reports = reports
            .iter()
            .map(|report| oxoffice_py_support::signature_report_to_py(py, report))
            .collect::<PyResult<Vec<_>>>()?;
        Ok(PyTuple::new(py, reports)?.unbind())
    }

    /// Save to a path or a binary stream. A document opened from a file
    /// refuses to replace that file unless `overwrite_source` is true, unlike
    /// python-docx.
    #[pyo3(signature = (path, *, overwrite_source = false))]
    fn save(
        &mut self,
        path: &Bound<'_, PyAny>,
        overwrite_source: bool,
        py: Python<'_>,
    ) -> PyResult<()> {
        if let Ok(path) = path.extract::<PathBuf>() {
            if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
            {
                return Err(PyValueError::new_err(
                    "use to_pdf() to export a PDF instead of save()",
                ));
            }
            let options = oxoffice_docx::SaveOptions { overwrite_source };
            return py
                .detach(|| self.inner.save_with_options(path, options))
                .map_err(|error| docx_to_pyerr(py, error));
        }
        let write = path.getattr("write").map_err(|_| PyTypeError::new_err(
            "save requires a filesystem path or binary writable stream; use stream.write(document.to_bytes())",
        ))?;
        let bytes = self
            .inner
            .to_bytes()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let written = write.call1((PyBytes::new(py, &bytes),))?;
        if written.extract::<usize>().ok() != Some(bytes.len()) {
            return Err(pyo3::exceptions::PyOSError::new_err(
                "binary stream write returned a short or invalid byte count",
            ));
        }
        Ok(())
    }

    #[pyo3(name = "to_bytes")]
    fn serialize<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        py.detach(|| self.inner.to_bytes())
            .map(|bytes| PyBytes::new(py, &bytes))
            .map_err(|error| docx_to_pyerr(py, error))
    }

    fn to_html(&self, py: Python<'_>) -> String {
        py.detach(|| self.inner.to_html())
    }
    fn to_markdown(&self, py: Python<'_>) -> String {
        py.detach(|| self.inner.to_markdown())
    }
    fn text(&self, py: Python<'_>) -> String {
        py.detach(|| self.inner.text_report().text)
    }
    fn text_report(&self, py: Python<'_>) -> PyTextReport {
        py.detach(|| self.inner.text_report()).into()
    }
    fn word_count(&self) -> usize {
        self.inner.word_count()
    }
    #[pyo3(signature = (*, include_spaces = true))]
    fn character_count(&self, include_spaces: bool) -> usize {
        self.inner.character_count(include_spaces)
    }
    fn validate(&self, py: Python<'_>) -> PyResult<PyValidationReport> {
        py.detach(|| self.inner.validate())
            .map(Into::into)
            .map_err(|error| docx_to_pyerr(py, error))
    }
    fn to_odt(&self, py: Python<'_>) -> PyResult<PyOdtExportResult> {
        let result = py
            .detach(|| self.inner.to_odt_bytes())
            .map_err(|error| docx_to_pyerr(py, error))?;
        Ok(PyOdtExportResult {
            bytes: result.bytes,
            diagnostics: result
                .diagnostics
                .into_iter()
                .map(|d| PyOdtDiagnostic {
                    path: d.path,
                    message: d.message,
                })
                .collect(),
        })
    }
    fn to_rtf(&self, py: Python<'_>) -> PyResult<PyRtfExportResult> {
        let result = py
            .detach(|| self.inner.to_rtf_bytes())
            .map_err(|error| docx_to_pyerr(py, error))?;
        Ok(PyRtfExportResult {
            bytes: result.bytes,
            diagnostics: result
                .diagnostics
                .into_iter()
                .map(|d| PyRtfDiagnostic {
                    offset: d.offset,
                    destination: d.destination,
                    message: d.message,
                })
                .collect(),
        })
    }
    fn to_epub(&self, py: Python<'_>) -> PyResult<PyEpubExportResult> {
        let result = py
            .detach(|| self.inner.to_epub_bytes())
            .map_err(|error| docx_to_pyerr(py, error))?;
        Ok(PyEpubExportResult {
            bytes: result.bytes,
            diagnostics: result
                .diagnostics
                .into_iter()
                .map(|d| PyEpubDiagnostic {
                    path: d.path,
                    message: d.message,
                })
                .collect(),
        })
    }
    #[pyo3(signature = (*, fonts = None, font_dir = None, font_dirs = None, system_fonts = false))]
    fn page_count<'py>(
        &self,
        py: Python<'py>,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
    ) -> PyResult<usize> {
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        let options = oxoffice_docx::RenderOptions::default().with_system_fonts(system_fonts);
        py.detach(|| match refs.as_deref() {
            None if system_fonts => self
                .inner
                .layout_with_options(options)
                .map(|layout| layout.layout.pages.len()),
            None => self
                .inner
                .layout_deterministic()
                .map(|layout| layout.layout.pages.len()),
            Some(fonts) if system_fonts => {
                // Feature-off gate for this nonempty system request.
                self.inner.layout_with_options(options)?;
                self.inner
                    .layout_with_additional_fonts_and_options(fonts, options)
                    .map(|layout| layout.layout.pages.len())
            }
            Some(fonts) => self
                .inner
                .layout_with_fonts_and_bundled_fallback_and_options(fonts, options)
                .map(|layout| layout.layout.pages.len()),
        })
        .map_err(|error| docx_to_pyerr(py, error))
    }
    #[pyo3(signature = (data, *, grammar_version=None))]
    fn render_template(
        &mut self,
        py: Python<'_>,
        data: &Bound<'_, PyAny>,
        grammar_version: Option<u32>,
    ) -> PyResult<usize> {
        if !data.is_instance_of::<PyDict>() {
            return Err(PyTypeError::new_err(
                "render_template data must be a dict with string keys",
            ));
        }
        let data = template_value(data, "data", &mut Vec::new())?;
        let before = self
            .inner
            .to_bytes()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let mut candidate = self.inner.clone_for_staging();
        let count = py
            .detach(|| match grammar_version {
                None => candidate.render_template(&data),
                Some(version) => candidate.render_template_versioned(&data, version),
            })
            .map_err(|error| template_render_pyerr(py, error))?;
        let after = candidate
            .to_bytes()
            .map_err(|error| docx_to_pyerr(py, error))?;
        if after != before {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.render_template",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(count)
    }

    fn validate_form<'py>(
        &self,
        py: Python<'py>,
        rules: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyTuple>> {
        let rules = copied_rules(py, rules)?;
        let issues = self
            .inner
            .validate_form(&rules)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let issues = issues
            .into_iter()
            .map(|issue| issue_to_py(py, issue))
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, issues)
    }

    fn fill_form(
        &mut self,
        py: Python<'_>,
        rules: &Bound<'_, PyAny>,
        assignments: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyTuple>> {
        let rules = copied_rules(py, rules)?;
        let assignments = copied_assignments(py, assignments)?;
        let mut candidate = self.inner.clone_for_staging();
        let counts = candidate
            .fill_form(&rules, &assignments)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let result = PyTuple::new(py, counts)?.unbind();
        self.publish_form_candidate(
            py,
            candidate,
            "Document.fill_form",
            WordPublication::ModernFormValues,
        )?;
        Ok(result)
    }

    fn reset_form(&mut self, py: Python<'_>, rules: &Bound<'_, PyAny>) -> PyResult<Py<PyTuple>> {
        let rules = copied_rules(py, rules)?;
        let mut candidate = self.inner.clone_for_staging();
        let counts = candidate
            .reset_form(&rules)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let result = PyTuple::new(py, counts)?.unbind();
        self.publish_form_candidate(
            py,
            candidate,
            "Document.reset_form",
            WordPublication::ModernFormValues,
        )?;
        Ok(result)
    }

    fn protect_form(
        &mut self,
        py: Python<'_>,
        rules: &Bound<'_, PyAny>,
        protection: &PyDocumentProtection,
    ) -> PyResult<()> {
        let rules = copied_rules(py, rules)?;
        let protection = protection.inner.clone();
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .protect_form(&rules, protection)
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_form_candidate(
            py,
            candidate,
            "Document.protect_form",
            WordPublication::ModernFormValues,
        )?;
        Ok(())
    }

    #[getter]
    fn document_protection(&self) -> Option<PyDocumentProtection> {
        self.inner
            .document_protection()
            .cloned()
            .map(|inner| PyDocumentProtection { inner })
    }

    fn set_document_protection(
        &mut self,
        py: Python<'_>,
        value: &PyDocumentProtection,
    ) -> PyResult<()> {
        let value = value.inner.clone();
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_document_protection(value)
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_form_candidate(
            py,
            candidate,
            "Document.set_document_protection",
            WordPublication::ModernFormValues,
        )?;
        Ok(())
    }

    fn remove_document_protection(
        &mut self,
        py: Python<'_>,
    ) -> PyResult<Option<Py<PyDocumentProtection>>> {
        // Py::new runs before publication. Allocation failure here is a source
        // ordering counterfactual, not an observed allocation RED.
        let snapshot = self
            .inner
            .document_protection()
            .cloned()
            .map(|inner| Py::new(py, PyDocumentProtection { inner }))
            .transpose()?;
        let mut candidate = self.inner.clone_for_staging();
        let removed = candidate
            .remove_document_protection()
            .map_err(|error| docx_to_pyerr(py, error))?;
        if removed.is_none() {
            return Ok(None);
        }
        self.publish_form_candidate(
            py,
            candidate,
            "Document.remove_document_protection",
            WordPublication::ModernFormValues,
        )?;
        Ok(snapshot)
    }

    #[pyo3(
        signature = (range, *, editor=None, group=None),
        text_signature = "($self, range, *, editor=None, group=None)"
    )]
    fn add_story_permission_range(
        &mut self,
        py: Python<'_>,
        range: &Bound<'_, PyAny>,
        editor: Option<String>,
        group: Option<String>,
    ) -> PyResult<Py<PyInt>> {
        let range = range
            .cast::<PyStoryRunRange>()
            .map_err(|_| PyTypeError::new_err("range must be a StoryRunRange"))?;
        let range = range.borrow();
        let start = oxoffice_docx::StoryRunPosition {
            location: self.native_location(py, &range.start.item)?,
            run_index: range.start.run_index,
        };
        let end = oxoffice_docx::StoryRunPosition {
            location: self.native_location(py, &range.end.item)?,
            run_index: range.end.run_index,
        };
        drop(range);
        let mut candidate = self.inner.clone_for_staging();
        let id = candidate
            .add_story_permission_range(
                editor.as_deref(),
                group.as_deref(),
                oxoffice_docx::StoryRunRange { start, end },
            )
            .map_err(|error| docx_to_pyerr(py, error))?;
        let id = id.into_pyobject(py)?.unbind();
        self.publish_api_candidate(py, candidate, "Document.add_story_permission_range")?;
        Ok(id)
    }

    fn bind_content_controls(
        &mut self,
        py: Python<'_>,
        values: &Bound<'_, PyDict>,
    ) -> PyResult<Py<PyInt>> {
        let values = values
            .iter()
            .map(|(key, value)| Ok((key.extract::<String>()?, value.extract::<String>()?)))
            .collect::<PyResult<HashMap<_, _>>>()?;
        let mut candidate = self.inner.clone_for_staging();
        let count = py
            .detach(|| candidate.bind_content_controls(&values))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let result = count.into_pyobject(py)?.unbind();
        self.publish_form_candidate(
            py,
            candidate,
            "Document.bind_content_controls",
            WordPublication::FormValues,
        )?;
        Ok(result)
    }

    fn legacy_form_fields<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let fields = self
            .inner
            .legacy_form_fields_with_defaults()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let fields = fields
            .into_iter()
            .map(|(field, default_value)| {
                Py::new(py, PyLegacyFormField::from_info(py, field, default_value)?)
            })
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, fields)
    }

    fn insert_legacy_form_field(
        &mut self,
        py: Python<'_>,
        position: &Bound<'_, PyAny>,
        definition: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let definition = definition.cast::<PyLegacyFormFieldDefinition>()?.borrow();
        let position = position.cast::<PyStoryRunPosition>()?.borrow();
        let field = definition.field();
        let mut candidate = self.inner.clone_for_staging();
        let location = self.native_location(py, &position.item)?;
        let native = oxoffice_docx::StoryRunPosition {
            location,
            run_index: position.run_index,
        };
        py.detach(|| candidate.insert_field(&native, field))
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.insert_legacy_form_field")?;
        Ok(())
    }

    #[pyo3(signature = (name, value, *, source_part=None, ordinal=None))]
    fn set_legacy_form_field_value(
        &mut self,
        py: Python<'_>,
        name: Option<&str>,
        value: &Bound<'_, PyAny>,
        source_part: Option<&str>,
        ordinal: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyLegacyFormField>> {
        let ordinal = ordinal
            .map(|value| {
                if !value.is_instance_of::<PyInt>() || value.is_instance_of::<PyBool>() {
                    return Err(PyTypeError::new_err(
                        "legacy field ordinal must be a nonnegative int, not bool",
                    ));
                }
                value.extract::<usize>()
            })
            .transpose()?;
        if source_part.is_some() != ordinal.is_some() {
            return Err(PyValueError::new_err(
                "source_part and ordinal must be supplied together",
            ));
        }
        if source_part.is_none() && name.is_none() {
            return Err(PyValueError::new_err(
                "name is required without source_part and ordinal",
            ));
        }
        let fields = self
            .inner
            .legacy_form_fields()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let mut matches = fields.into_iter().filter(|field| {
            if let (Some(part), Some(ordinal)) = (source_part, ordinal) {
                field.source_part == part
                    && field.ordinal == ordinal
                    && name.is_none_or(|name| field.name.as_deref() == Some(name))
            } else {
                field.name.as_deref() == name
            }
        });
        let field = matches
            .next()
            .ok_or_else(|| PyValueError::new_err("unknown or stale legacy form field identity"))?;
        if matches.next().is_some() {
            return Err(PyValueError::new_err(
                "ambiguous legacy form field name; supply source_part and ordinal",
            ));
        }
        let value = match field.kind {
            oxoffice_docx::LegacyFormFieldKind::TextInput => {
                oxoffice_docx::LegacyFormFieldValue::Text(value.extract::<String>()?)
            }
            oxoffice_docx::LegacyFormFieldKind::CheckBox => {
                if !value.is_instance_of::<PyBool>() {
                    return Err(PyTypeError::new_err("legacy checkbox requires bool"));
                }
                oxoffice_docx::LegacyFormFieldValue::Checked(value.extract::<bool>()?)
            }
            oxoffice_docx::LegacyFormFieldKind::DropDownList => {
                if !value.is_instance_of::<PyInt>() || value.is_instance_of::<PyBool>() {
                    return Err(PyTypeError::new_err(format!(
                        "legacy dropdown received {}. Use a nonnegative integer index, not bool, from document.legacy_form_fields() snapshot choices",
                        value.get_type().name()?,
                    )));
                }
                oxoffice_docx::LegacyFormFieldValue::SelectedIndex(value.extract::<usize>()?)
            }
        };
        let mut candidate = self.inner.clone_for_staging();
        let reopened = py
            .detach(|| {
                candidate.set_legacy_form_field_value(&field.source_part, field.ordinal, value)
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        let default_value = candidate
            .legacy_form_fields_with_defaults()
            .map_err(|error| docx_to_pyerr(py, error))?
            .into_iter()
            .find(|(info, _)| {
                info.source_part == reopened.source_part && info.ordinal == reopened.ordinal
            })
            .map(|(_, default_value)| default_value)
            .ok_or_else(|| PyValueError::new_err("legacy form identity did not survive reopen"))?;
        let prepared = Py::new(
            py,
            PyLegacyFormField::from_info(py, reopened, default_value)?,
        )?;
        self.publish_form_candidate(
            py,
            candidate,
            "Document.set_legacy_form_field_value",
            WordPublication::FormValues,
        )?;
        Ok(prepared)
    }

    fn content_controls<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner
                .content_controls()
                .into_iter()
                .map(PyContentControl::from),
        )
    }
    fn set_content_control_value_by_tag(
        &mut self,
        py: Python<'_>,
        tag: &str,
        value: &str,
    ) -> PyResult<usize> {
        self.set_control_value(py, tag, value, true)
    }
    fn fill_content_control_tags<'py>(
        &mut self,
        py: Python<'py>,
        pairs: Vec<(String, String)>,
    ) -> PyResult<Bound<'py, PyTuple>> {
        let pairs = pairs
            .iter()
            .map(|(tag, value)| (tag.as_str(), value.as_str()))
            .collect::<Vec<_>>();
        let mut candidate = self.inner.clone_for_staging();
        let counts = py
            .detach(|| candidate.fill_content_control_tags(&pairs))
            .map_err(|error| docx_to_pyerr(py, error))?;

        let result = PyTuple::new(py, counts)?;
        self.publish_api_candidate(py, candidate, "Document.fill_content_control_tags")?;

        Ok(result)
    }

    #[allow(clippy::too_many_arguments)] // Q21 keeps each public operand explicit.
    #[pyo3(signature = (destination, placement, kind, tag=None, alias=None, lock=None, appearance=None, placeholder=None))]
    fn insert_content_control(
        &mut self,
        py: Python<'_>,
        destination: PyRef<'_, PyContentLocation>,
        placement: &str,
        kind: &str,
        tag: Option<String>,
        alias: Option<String>,
        lock: Option<String>,
        appearance: Option<String>,
        placeholder: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let destination = destination.native(py, self)?;
        let spec = oxoffice_docx::ContentControlSpec {
            placement: content_control_placement_from_name(placement)?,
            kind: content_control_kind_from_name(kind)?,
            tag,
            alias,
            lock: lock
                .as_deref()
                .map(content_control_lock_from_name)
                .transpose()?,
            appearance: appearance
                .as_deref()
                .map(content_control_appearance_from_name)
                .transpose()?,
            placeholder: optional_python_bool(placeholder, "placeholder")?,
        };
        let mut candidate = self.inner.clone_for_staging();
        py.detach(|| candidate.insert_content_control(&destination, spec))
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.insert_content_control")
    }

    #[pyo3(signature = (location, tag=None))]
    fn set_content_control_tag(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
        tag: Option<String>,
    ) -> PyResult<()> {
        self.edit_content_control(
            py,
            &location,
            "Document.set_content_control_tag",
            |document, location| document.set_content_control_tag(location, tag.as_deref()),
        )
    }

    #[pyo3(signature = (location, alias=None))]
    fn set_content_control_alias(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
        alias: Option<String>,
    ) -> PyResult<()> {
        self.edit_content_control(
            py,
            &location,
            "Document.set_content_control_alias",
            |document, location| document.set_content_control_alias(location, alias.as_deref()),
        )
    }

    #[pyo3(signature = (location, lock=None))]
    fn set_content_control_lock(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
        lock: Option<String>,
    ) -> PyResult<()> {
        let lock = lock
            .as_deref()
            .map(content_control_lock_from_name)
            .transpose()?;
        self.edit_content_control(
            py,
            &location,
            "Document.set_content_control_lock",
            |document, location| document.set_content_control_lock(location, lock),
        )
    }

    #[pyo3(signature = (location, appearance=None))]
    fn set_content_control_appearance(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
        appearance: Option<String>,
    ) -> PyResult<()> {
        let appearance = appearance
            .as_deref()
            .map(content_control_appearance_from_name)
            .transpose()?;
        self.edit_content_control(
            py,
            &location,
            "Document.set_content_control_appearance",
            |document, location| document.set_content_control_appearance(location, appearance),
        )
    }

    #[pyo3(signature = (location, placeholder=None))]
    fn set_content_control_placeholder(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
        placeholder: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let placeholder = optional_python_bool(placeholder, "placeholder")?;
        self.edit_content_control(
            py,
            &location,
            "Document.set_content_control_placeholder",
            |document, location| document.set_content_control_placeholder(location, placeholder),
        )
    }

    fn remove_content_control(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
    ) -> PyResult<()> {
        self.edit_content_control(
            py,
            &location,
            "Document.remove_content_control",
            |document, location| document.remove_content_control(location),
        )
    }

    fn unwrap_content_control(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
    ) -> PyResult<()> {
        self.edit_content_control(
            py,
            &location,
            "Document.unwrap_content_control",
            |document, location| document.unwrap_content_control(location),
        )
    }

    fn move_content_control(
        &mut self,
        py: Python<'_>,
        source: PyRef<'_, PyContentLocation>,
        destination: PyRef<'_, PyContentLocation>,
    ) -> PyResult<()> {
        let source = source.native(py, self)?;
        let destination = destination.native(py, self)?;
        let mut candidate = self.inner.clone_for_staging();
        py.detach(|| candidate.move_content_control(&source, &destination))
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.move_content_control")
    }

    fn clone_content_control(
        &mut self,
        py: Python<'_>,
        source: PyRef<'_, PyContentLocation>,
        destination: PyRef<'_, PyContentLocation>,
    ) -> PyResult<()> {
        let source = source.native(py, self)?;
        let destination = destination.native(py, self)?;
        let mut candidate = self.inner.clone_for_staging();
        py.detach(|| candidate.clone_content_control(&source, &destination))
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.clone_content_control")
    }

    fn set_content_control_type(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
        payload: PyRef<'_, PyContentControlTypePayload>,
    ) -> PyResult<()> {
        let payload = payload.inner.clone();
        self.edit_content_control(
            py,
            &location,
            "Document.set_content_control_type",
            move |document, location| document.set_content_control_type(location, payload),
        )
    }

    fn repeat_content_control(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
    ) -> PyResult<()> {
        self.edit_content_control(
            py,
            &location,
            "Document.repeat_content_control",
            |document, location| document.repeat_content_control(location),
        )
    }

    #[pyo3(signature = (item_xml, schema_uris=Vec::new()), text_signature = "($self, item_xml, schema_uris=())")]
    fn add_custom_xml_store(
        &mut self,
        py: Python<'_>,
        item_xml: &[u8],
        schema_uris: Vec<String>,
    ) -> PyResult<String> {
        let mut candidate = self.inner.clone_for_staging();
        let item_id = py
            .detach(|| {
                let uris = schema_uris.iter().map(String::as_str).collect::<Vec<_>>();
                candidate.add_custom_xml_store(item_xml, &uris)
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.add_custom_xml_store")?;
        Ok(item_id)
    }

    #[pyo3(signature = (location, store_item_id, xpath, prefix_mappings=None))]
    fn set_content_control_binding(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
        store_item_id: String,
        xpath: String,
        prefix_mappings: Option<String>,
    ) -> PyResult<()> {
        self.edit_content_control(
            py,
            &location,
            "Document.set_content_control_binding",
            move |document, location| {
                document.set_content_control_binding(
                    location,
                    &store_item_id,
                    &xpath,
                    prefix_mappings.as_deref(),
                )
            },
        )
    }

    fn set_content_control_value_by_alias(
        &mut self,
        py: Python<'_>,
        alias: &str,
        value: &str,
    ) -> PyResult<usize> {
        self.set_control_value(py, alias, value, false)
    }
    #[pyo3(signature = (destination, fragment, *, policy=None))]
    fn import_fragment(
        &mut self,
        py: Python<'_>,
        destination: PyRef<'_, PyContentLocation>,
        fragment: PyRef<'_, PyDocumentFragment>,
        policy: Option<PyRef<'_, PyFragmentConflictPolicy>>,
    ) -> PyResult<()> {
        let destination = destination.native(py, self)?;
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .import_fragment(
                &destination,
                &fragment.inner,
                policy.map(|p| p.inner).unwrap_or_default(),
            )
            .map_err(|e| docx_to_pyerr(py, e))?;
        self.publish_api_candidate(py, candidate, "Document.import_fragment")
    }
    #[pyo3(signature = (other, *, at, policy=None))]
    fn insert_document(
        &mut self,
        py: Python<'_>,
        other: PyRef<'_, PyDocument>,
        at: &Bound<'_, PyAny>,
        policy: Option<PyRef<'_, PyFragmentConflictPolicy>>,
    ) -> PyResult<()> {
        if at.is_instance_of::<PyBool>() {
            return Err(PyTypeError::new_err(
                "at must be a physical body index, not bool",
            ));
        }
        let at = at.extract::<usize>()?;
        let destination = self.body_location(py, at)?;
        if other.inner.content_count() == 0 {
            return Ok(());
        }
        let start = other.body_location(py, 0)?;
        let end = oxoffice_docx::ContentLocation::end(other.body_story(py)?);
        let fragment =
            oxoffice_docx::DocumentFragment::from_range(&other.inner, &start, &end, false)
                .map_err(|e| docx_to_pyerr(py, e))?;
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .import_fragment(
                &destination,
                &fragment,
                oxoffice_docx::FragmentConflictPolicy::for_insert_document(policy.map(|p| p.inner)),
            )
            .map_err(|e| docx_to_pyerr(py, e))?;
        self.publish_api_candidate(py, candidate, "Document.insert_document")
    }
    #[getter]
    fn custom_properties(slf: Py<Self>) -> PyCustomProperties {
        PyCustomProperties { document: slf }
    }
    #[getter]
    fn app_properties(slf: Py<Self>) -> PyAppProperties {
        PyAppProperties { document: slf }
    }

    #[getter]
    fn core_properties(slf: Py<Self>, py: Python<'_>) -> PyResult<Py<PyCoreProperties>> {
        Py::new(py, PyCoreProperties { document: slf })
    }

    #[getter]
    fn update_fields_on_open(&self) -> Option<bool> {
        self.inner.update_fields_on_open()
    }

    #[setter]
    fn set_update_fields_on_open(&mut self, py: Python<'_>, value: Option<bool>) -> PyResult<()> {
        self.inner
            .set_update_fields_on_open(value)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    fn image_data<'py>(
        &self,
        py: Python<'py>,
        relationship_id: &str,
    ) -> Option<Bound<'py, PyBytes>> {
        self.inner
            .image_data(relationship_id)
            .map(|bytes| PyBytes::new(py, &bytes))
    }

    fn replace_image(
        &mut self,
        py: Python<'_>,
        relationship_id: &str,
        data: &[u8],
    ) -> PyResult<()> {
        // No content moves, so live handles stay valid.
        self.inner
            .replace_image(relationship_id, data)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    fn replace_image_for_story(
        &mut self,
        py: Python<'_>,
        story: PyRef<'_, PyStory>,
        relationship_id: &str,
        data: &[u8],
    ) -> PyResult<()> {
        let story = self.native_story(py, &story)?;
        self.inner
            .replace_image_for_story(&story, relationship_id, data)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    #[pyo3(signature = (paragraph, heading, text, *, tooltip=None))]
    fn add_heading_hyperlink(
        slf: Py<Self>,
        py: Python<'_>,
        paragraph: PyRef<'_, PyParagraph>,
        heading: PyRef<'_, PyParagraph>,
        text: &str,
        tooltip: Option<&str>,
    ) -> PyResult<Py<crate::run::PyRun>> {
        if !paragraph.belongs_to(py, &slf) || !heading.belongs_to(py, &slf) {
            return Err(PyValueError::new_err(
                "heading hyperlink paragraphs belong to a different document",
            ));
        }
        let source = paragraph.validate(py)?;
        let destination = heading.validate(py)?;
        let mut document = slf.borrow_mut(py);
        let source = source
            .native(&document.inner)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let destination = destination
            .native(&document.inner)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let mut candidate = document.inner.clone_for_staging();
        let position = candidate
            .add_heading_hyperlink_at(&source, &destination, text, tooltip)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let run_path = candidate
            .read_story_paragraph(&position.location, |p| p.run_path(position.run_index))
            .map_err(|error| docx_to_pyerr(py, error))?
            .ok_or_else(|| PyIndexError::new_err("heading link run was not found"))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.add_heading_hyperlink",
            crate::document::WordPublication::Conservative,
        )?;
        let path = document
            .revisions
            .capture_prepared(
                &prepared.revision,
                smallvec![
                    PathSeg::Para(*position.location.index_path().last().unwrap_or(&0)),
                    PathSeg::Run(position.run_index)
                ],
            )
            .map_err(revision_to_pyerr)?;
        let result = Py::new(
            py,
            crate::run::PyRun::new(slf.clone_ref(py), path, run_path, Some(position.location))
                .bind_prepared(py, &document, &prepared, &candidate, None)?,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(result)
    }

    /// Frozen snapshots distinguish physical drawings of the same media.
    fn pictures<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        self.ensure_word_authority(py)?;
        let values = self
            .inner
            .pictures()
            .map_err(|error| docx_to_pyerr(py, error))?
            .into_iter()
            .map(|inner| {
                Py::new(
                    py,
                    PyPicture {
                        inner,
                        binding: self.capture_snapshot(smallvec![])?,
                    },
                )
            })
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, values)
    }

    fn set_picture_size(
        &mut self,
        py: Python<'_>,
        picture: PyRef<'_, PyPicture>,
        width: &Bound<'_, PyAny>,
        height: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        picture
            .binding
            .validate_scopes(
                &self.revisions,
                "picture",
                "Re-fetch it with document.pictures().",
            )
            .map_err(|error| crate::stale_to_pyerr(py, error))?;
        if picture.binding.revision != self.revisions.current() {
            return Err(crate::stale_to_pyerr(
                py,
                StaleElementError {
                    element_kind: "picture".into(),
                    captured_revision: picture.binding.revision,
                    current_revision: self.revisions.current(),
                    recovery_hint: "Re-fetch it with document.pictures().".into(),
                },
            ));
        }
        let width = checked_section_length(Some(width), "picture width")?.unwrap();
        let height = checked_section_length(Some(height), "picture height")?.unwrap();
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_picture_size_at(
                &picture.inner,
                oxoffice_docx::Length::emu(width),
                oxoffice_docx::Length::emu(height),
            )
            .map_err(|error| docx_to_pyerr(py, error))?;
        if (width, height) == (picture.inner.width, picture.inner.height) {
            return Ok(());
        }
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.set_picture_size",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    fn set_picture_sizes_for_relationship(
        &mut self,
        py: Python<'_>,
        relationship_id: &str,
        width: i64,
        height: i64,
    ) -> PyResult<usize> {
        // No content moves, so live handles stay valid.
        py.detach(|| {
            self.inner.set_picture_size(
                relationship_id,
                oxoffice_docx::Length::emu(width),
                oxoffice_docx::Length::emu(height),
            )
        })
        .map_err(|error| docx_to_pyerr(py, error))
    }

    /// Split runs so that `span`, from `Paragraph.find_text`, covers whole
    /// runs, and return the indexes `(first, last)` of those runs, the last
    /// excluded, as `Paragraph.runs` counts them. The text does not change,
    /// and a tab, a break or another text-free item right before or right
    /// after the span stays out of those runs, so formatting them or setting
    /// their text leaves it alone.
    /// A paragraph of the body, a table cell, a header, a footer, a note or a
    /// comment is accepted. A change stales the document's earlier handles,
    /// the paragraphs of other spans included: find the next span after.
    fn isolate_span(
        slf: Py<Self>,
        py: Python<'_>,
        span: PyRef<'_, crate::paragraph::PyTextSpan>,
    ) -> PyResult<(usize, usize)> {
        let paragraph = span.paragraph.bind(py).borrow();
        if !paragraph.belongs_to(py, &slf) {
            return Err(PyValueError::new_err(
                "span belongs to a different document",
            ));
        }
        let location = paragraph.validate(py)?;
        let mut document = slf.borrow_mut(py);
        let run_count = |document: &oxoffice_docx::Document| {
            location.read(document, |paragraph| paragraph.run_count())
        };
        let before = run_count(&document.inner).map_err(|error| docx_to_pyerr(py, error))?;
        let mut candidate = document.inner.clone_for_staging();
        let runs = location
            .edit(&mut candidate, |paragraph| {
                paragraph.isolate_span(oxoffice_docx::TextSpan {
                    start: span.start,
                    end: span.end,
                })
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        if run_count(&candidate).map_err(|error| docx_to_pyerr(py, error))? != before {
            let prepared = document.prepare_word_revision(
                py,
                &candidate,
                "Document.isolate_span",
                crate::document::WordPublication::Conservative,
            )?;
            document.commit_word_revision(prepared)?;
            document.install_inner(candidate);
        }
        Ok((runs.start, runs.end))
    }

    fn split_run(
        slf: Py<Self>,
        py: Python<'_>,
        body_index: &Bound<'_, PyAny>,
        run_index: usize,
        character_offset: usize,
    ) -> PyResult<usize> {
        let Ok(paragraph) = body_index.cast::<PyParagraph>() else {
            // An integer is the direct body index, as in `RunPosition`.
            let body_index = body_index.extract::<usize>().map_err(|error| {
                if error.is_instance_of::<PyTypeError>(py) {
                    PyTypeError::new_err("body_index must be an int or a Paragraph handle")
                } else {
                    error
                }
            })?;
            return slf
                .borrow_mut(py)
                .split_body_run(py, body_index, run_index, character_offset);
        };
        let paragraph = paragraph.borrow();
        if !paragraph.belongs_to(py, &slf) {
            return Err(PyValueError::new_err(
                "paragraph handle belongs to a different document",
            ));
        }
        // A cell handle counts paragraphs inside cell content controls, which
        // `Cell::paragraph_mut` does not, so it could name another paragraph.
        let ParagraphLocation::Body(paragraph_index) = paragraph.validate(py)? else {
            return Err(PyValueError::new_err(
                "split_run does not accept a table cell paragraph handle",
            ));
        };
        let mut document = slf.borrow_mut(py);
        if let Some(body_index) = document.inner.content_index_of_paragraph(paragraph_index) {
            return document.split_body_run(py, body_index, run_index, character_offset);
        }
        // A paragraph inside a block content control has no direct body index.
        let run_count = |document: &oxoffice_docx::Document| {
            document
                .paragraph(paragraph_index)
                .map(|paragraph| paragraph.run_count())
        };
        let before = run_count(&document.inner);
        let mut candidate = document.inner.clone_for_staging();
        let boundary = candidate
            .paragraph_mut(paragraph_index)
            .ok_or_else(|| PyIndexError::new_err("paragraph index out of range"))?
            .split_run(run_index, character_offset)
            .map_err(|error| docx_to_pyerr(py, error))?;
        if run_count(&candidate) != before {
            let prepared = document.prepare_word_revision(
                py,
                &candidate,
                "Document.split_run",
                crate::document::WordPublication::Conservative,
            )?;
            document.commit_word_revision(prepared)?;
            document.install_inner(candidate);
        }
        Ok(boundary)
    }

    #[pyo3(signature = (*, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, revision_view = "accepted", revision_style = "plain"))]
    #[allow(clippy::too_many_arguments)]
    fn to_pdf<'py>(
        &self,
        py: Python<'py>,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        revision_view: &str,
        revision_style: &str,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let mut options = parse_render_options(revision_view, revision_style)?;
        options.use_system_fonts = system_fonts;
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        py.detach(|| match font_refs.as_deref() {
            None if system_fonts => self.inner.to_pdf_with_options(options),
            None => self.inner.to_pdf_deterministic_with_options(options),
            Some(fonts) if system_fonts => self
                .inner
                .to_pdf_with_diagnostics(fonts, options)
                .map(|(bytes, _)| bytes),
            Some(fonts) => {
                let layout = self
                    .inner
                    .layout_with_fonts_and_bundled_fallback_and_options(fonts, options)?;
                oxoffice_pdf::render_to_pdf_checked(&layout.layout)
                    .map_err(oxoffice_docx::Error::from)
            }
        })
        .map(|bytes| PyBytes::new(py, &bytes))
        .map_err(|error| docx_to_pyerr(py, error))
    }

    #[pyo3(signature = (*, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, revision_view = "accepted", revision_style = "plain"))]
    #[allow(clippy::too_many_arguments)]
    fn to_pdf_with_diagnostics<'py>(
        &self,
        py: Python<'py>,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        revision_view: &str,
        revision_style: &str,
    ) -> PyResult<(Bound<'py, PyBytes>, Bound<'py, PyTuple>)> {
        let mut options = parse_render_options(revision_view, revision_style)?;
        options.use_system_fonts = system_fonts;
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        let (payload, diagnostics) = py
            .detach(|| match font_refs.as_deref() {
                None => self.inner.to_pdf_with_diagnostics(&[], options),
                Some(fonts) if system_fonts => self.inner.to_pdf_with_diagnostics(fonts, options),
                Some(fonts) => {
                    let layout = self
                        .inner
                        .layout_with_fonts_and_bundled_fallback_and_options(fonts, options)?;
                    let bytes = oxoffice_pdf::render_to_pdf_checked(&layout.layout)
                        .map_err(oxoffice_docx::Error::from)?;
                    Ok((bytes, layout.layout.diagnostics.clone()))
                }
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        let messages = PyTuple::new(py, diagnostics.iter().map(|d| d.message.as_str()))?;
        Ok((PyBytes::new(py, &payload), messages))
    }
    #[pyo3(signature = (profile = "pdfa-2b"))]
    fn to_pdfa_deterministic<'py>(
        &self,
        py: Python<'py>,
        profile: &str,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let profile = match profile {
            "pdfa-2b" => oxoffice_docx::PdfConformance::PdfA2b,
            "pdfa-3b" => oxoffice_docx::PdfConformance::PdfA3b,
            _ => {
                return Err(PyValueError::new_err(format!(
                    "profile must be pdfa-2b or pdfa-3b, not {profile:?}"
                )));
            }
        };
        py.detach(|| self.inner.to_pdfa_deterministic(profile))
            .map(|bytes| PyBytes::new(py, &bytes))
            .map_err(|error| docx_to_pyerr(py, error))
    }

    #[pyo3(signature = (page_index, dpi = 150.0, *, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, revision_view = "accepted", revision_style = "plain"))]
    #[allow(clippy::too_many_arguments)]
    fn render_page_to_png<'py>(
        &self,
        py: Python<'py>,
        page_index: usize,
        dpi: f64,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        revision_view: &str,
        revision_style: &str,
    ) -> PyResult<Option<Bound<'py, PyBytes>>> {
        let mut options = parse_render_options(revision_view, revision_style)?;
        options.use_system_fonts = system_fonts;
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        py.detach(|| match font_refs.as_deref() {
            None if system_fonts => self
                .inner
                .render_page_to_png_with_options(page_index, dpi, options),
            None => self
                .inner
                .render_page_to_png_deterministic_with_options(page_index, dpi, options),
            Some(fonts) if system_fonts => self
                .inner
                .render_page_to_png_with_diagnostics(page_index, dpi, fonts, options)
                .map(|(bytes, _)| bytes),
            Some(fonts) => {
                let layout = self
                    .inner
                    .layout_with_fonts_and_bundled_fallback_and_options(fonts, options)?;
                Ok(oxoffice_pdf::render_page_to_png(
                    &layout.layout,
                    page_index,
                    dpi,
                ))
            }
        })
        .map(|bytes| bytes.map(|bytes| PyBytes::new(py, &bytes)))
        .map_err(|error| docx_to_pyerr(py, error))
    }

    #[pyo3(signature = (page_index, dpi = 150.0, *, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, revision_view = "accepted", revision_style = "plain"))]
    #[allow(clippy::too_many_arguments)]
    fn render_page_to_png_with_diagnostics<'py>(
        &self,
        py: Python<'py>,
        page_index: usize,
        dpi: f64,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        revision_view: &str,
        revision_style: &str,
    ) -> PyResult<(Option<Bound<'py, PyBytes>>, Bound<'py, PyTuple>)> {
        let mut options = parse_render_options(revision_view, revision_style)?;
        options.use_system_fonts = system_fonts;
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        let (payload, diagnostics) = py
            .detach(|| match font_refs.as_deref() {
                None => {
                    self.inner
                        .render_page_to_png_with_diagnostics(page_index, dpi, &[], options)
                }
                Some(fonts) if system_fonts => self
                    .inner
                    .render_page_to_png_with_diagnostics(page_index, dpi, fonts, options),
                Some(fonts) => {
                    let layout = self
                        .inner
                        .layout_with_fonts_and_bundled_fallback_and_options(fonts, options)?;
                    Ok((
                        oxoffice_pdf::render_page_to_png(&layout.layout, page_index, dpi),
                        layout.layout.diagnostics.clone(),
                    ))
                }
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        let messages = PyTuple::new(py, diagnostics.iter().map(|d| d.message.as_str()))?;
        Ok((payload.map(|bytes| PyBytes::new(py, &bytes)), messages))
    }
    #[pyo3(signature = (page_index, *, fonts = None, font_dir = None, font_dirs = None, system_fonts = false))]
    fn render_page_to_svg<'py>(
        &self,
        py: Python<'py>,
        page_index: usize,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
    ) -> PyResult<Option<PySvgRenderResult>> {
        let options = oxoffice_docx::RenderOptions::default().with_system_fonts(system_fonts);
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        let rendered = py
            .detach(|| match font_refs.as_deref() {
                None if system_fonts => self
                    .inner
                    .render_page_to_svg_with_options(page_index, options),
                None => self.inner.render_page_to_svg_deterministic(page_index),
                Some(fonts) if system_fonts => {
                    // Feature-off gate for this nonempty system request.
                    self.inner.layout_with_options(options)?;
                    self.inner
                        .render_page_to_svg_with_fonts_and_options(page_index, fonts, options)
                }
                Some(fonts) => {
                    let layout = self
                        .inner
                        .layout_with_fonts_and_bundled_fallback_and_options(fonts, options)?;
                    Ok(oxoffice_docx::render_page(&layout.layout, page_index))
                }
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        Ok(rendered.map(|result| PySvgRenderResult {
            svg: result.svg,
            diagnostics: result
                .diagnostics
                .into_iter()
                .map(|diagnostic| PySvgDiagnostic {
                    path: diagnostic.path,
                    message: diagnostic.message,
                })
                .collect(),
        }))
    }

    #[pyo3(signature = (dpi = 150.0, *, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, revision_view = "accepted", revision_style = "plain"))]
    #[allow(clippy::too_many_arguments)] // Explicit public rendering keyword operands.
    fn render_all_pages<'py>(
        &self,
        py: Python<'py>,
        dpi: f64,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        revision_view: &str,
        revision_style: &str,
    ) -> PyResult<Bound<'py, PyList>> {
        let mut options = parse_render_options(revision_view, revision_style)?;
        options.use_system_fonts = system_fonts;
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        let pages = py
            .detach(|| match font_refs.as_deref() {
                None if system_fonts => self.inner.render_all_pages_with_options(dpi, options),
                None => {
                    let layout = self.inner.layout_deterministic_with_options(options)?;
                    Ok(oxoffice_pdf::render_all_pages(&layout.layout, dpi))
                }
                Some(fonts) if system_fonts => self
                    .inner
                    .render_all_pages_with_diagnostics(dpi, fonts, options)
                    .map(|(pages, _)| pages),
                Some(fonts) => {
                    let layout = self
                        .inner
                        .layout_with_fonts_and_bundled_fallback_and_options(fonts, options)?;
                    Ok(oxoffice_pdf::render_all_pages(&layout.layout, dpi))
                }
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        PyList::new(py, pages.iter().map(|page| PyBytes::new(py, page)))
    }

    #[pyo3(signature = (dpi = 150.0, *, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, revision_view = "accepted", revision_style = "plain"))]
    #[allow(clippy::too_many_arguments)] // Explicit public rendering keyword operands.
    fn render_all_pages_with_diagnostics<'py>(
        &self,
        py: Python<'py>,
        dpi: f64,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        revision_view: &str,
        revision_style: &str,
    ) -> PyResult<(Bound<'py, PyList>, Bound<'py, PyTuple>)> {
        let mut options = parse_render_options(revision_view, revision_style)?;
        options.use_system_fonts = system_fonts;
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        let (payload, diagnostics) = py
            .detach(|| match font_refs.as_deref() {
                None => self
                    .inner
                    .render_all_pages_with_diagnostics(dpi, &[], options),
                Some(fonts) if system_fonts => self
                    .inner
                    .render_all_pages_with_diagnostics(dpi, fonts, options),
                Some(fonts) => {
                    let layout = self
                        .inner
                        .layout_with_fonts_and_bundled_fallback_and_options(fonts, options)?;
                    Ok((
                        oxoffice_pdf::render_all_pages(&layout.layout, dpi),
                        layout.layout.diagnostics.clone(),
                    ))
                }
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        let messages = PyTuple::new(py, diagnostics.iter().map(|d| d.message.as_str()))?;
        Ok((
            PyList::new(py, payload.iter().map(|bytes| PyBytes::new(py, bytes)))?,
            messages,
        ))
    }
    #[pyo3(signature = (*, dpi = 150.0, format = "png", quality = 90, transparent = false, pages = None, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, revision_view = "accepted", revision_style = "plain"))]
    #[allow(clippy::too_many_arguments)]
    fn render_pages<'py>(
        &self,
        py: Python<'py>,
        dpi: f64,
        format: &str,
        quality: u8,
        transparent: bool,
        pages: Option<Vec<usize>>,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        revision_view: &str,
        revision_style: &str,
    ) -> PyResult<Py<PyAny>> {
        // The twin selects page indices from the exact layout it paints.
        // Reuse that path so caller fonts cannot leave stale page bounds.
        self.render_pages_with_diagnostics(
            py,
            dpi,
            format,
            quality,
            transparent,
            pages,
            fonts,
            font_dir,
            font_dirs,
            system_fonts,
            revision_view,
            revision_style,
        )
        .map(|(payload, _)| payload)
    }

    #[pyo3(signature = (*, dpi = 150.0, format = "png", quality = 90, transparent = false, pages = None, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, revision_view = "accepted", revision_style = "plain"))]
    #[allow(clippy::too_many_arguments)]
    fn render_pages_with_diagnostics<'py>(
        &self,
        py: Python<'py>,
        dpi: f64,
        format: &str,
        quality: u8,
        transparent: bool,
        pages: Option<Vec<usize>>,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        revision_view: &str,
        revision_style: &str,
    ) -> PyResult<(Py<PyAny>, Bound<'py, PyTuple>)> {
        let mut options = parse_render_options(revision_view, revision_style)?;
        options.use_system_fonts = system_fonts;
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        let (rendered, diagnostics) = py
            .detach(|| {
                let format = parse_raster_format(format, quality, transparent)?;
                let raster = oxoffice_docx::RasterOptions { dpi, format };
                match font_refs.as_deref() {
                    None => self.inner.render_pages_with_diagnostics(
                        pages.as_deref(),
                        raster,
                        &[],
                        options,
                    ),
                    Some(fonts) if system_fonts => self.inner.render_pages_with_diagnostics(
                        pages.as_deref(),
                        raster,
                        fonts,
                        options,
                    ),
                    Some(fonts) => {
                        let layout = self
                            .inner
                            .layout_with_fonts_and_bundled_fallback_and_options(fonts, options)?;
                        let all_pages;
                        let selected = match pages.as_deref() {
                            Some(selected) => selected,
                            None => {
                                all_pages = (0..layout.layout.pages.len()).collect::<Vec<_>>();
                                &all_pages
                            }
                        };
                        let output = oxoffice_pdf::render_pages(&layout.layout, selected, raster)
                            .map_err(oxoffice_docx::Error::Raster)?;
                        Ok((output, layout.layout.diagnostics.clone()))
                    }
                }
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        let payload = match rendered {
            oxoffice_docx::RasterOutput::SeparatePages(pages) => {
                PyList::new(py, pages.iter().map(|bytes| PyBytes::new(py, bytes)))?
                    .into_any()
                    .unbind()
            }
            oxoffice_docx::RasterOutput::MultiPageTiff(bytes) => {
                PyBytes::new(py, &bytes).into_any().unbind()
            }
        };
        let messages = PyTuple::new(py, diagnostics.iter().map(|d| d.message.as_str()))?;
        Ok((payload, messages))
    }
    #[pyo3(signature = (
        edited,
        author,
        timestamp,
        *,
        granularity = "run",
        ignore_formatting = false,
        ignore_whitespace = false,
        ignore_fields = false,
        ignore_comments = false,
        ignored_stories = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn compare<'py>(
        &mut self,
        edited: &PyDocument,
        author: &str,
        timestamp: &str,
        py: Python<'py>,
        granularity: &str,
        ignore_formatting: bool,
        ignore_whitespace: bool,
        ignore_fields: bool,
        ignore_comments: bool,
        ignored_stories: Option<Vec<String>>,
    ) -> PyResult<Bound<'py, PyTuple>> {
        let mut candidate = self.inner.clone_for_staging();
        let (diagnostics, changed) = py
            .detach(|| {
                let options = oxoffice_docx::ComparisonOptions {
                    granularity: parse_comparison_granularity(granularity)?,
                    ignore_formatting,
                    ignore_whitespace,
                    ignore_fields,
                    ignore_comments,
                    ignored_stories: ignored_stories
                        .iter()
                        .flatten()
                        .map(|name| parse_comparison_story(name))
                        .collect::<oxoffice_docx::Result<_>>()?,
                };
                let before = candidate.to_bytes()?;
                let diagnostics =
                    candidate.compare_with_options(&edited.inner, author, timestamp, &options)?;
                let changed = candidate.to_bytes()? != before;
                Ok::<_, oxoffice_docx::Error>((diagnostics, changed))
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        let result = PyTuple::new(
            py,
            diagnostics.into_iter().map(|item| PyComparisonDiagnostic {
                location: item.location,
                message: item.message,
            }),
        )?;
        if changed {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.compare",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(result)
    }

    #[getter]
    fn comments<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyTuple>> {
        let py = slf.py();
        let owner = std::sync::Arc::new(slf.clone().unbind());
        let mut this = slf.borrow_mut();
        this.ensure_word_authority(py)?;
        let mut anchors = this
            .inner
            .comment_anchor_snapshot_results()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let mut items = this
            .inner
            .standalone_story_item_snapshots()
            .map_err(|error| docx_to_pyerr(py, error))?
            .into_iter()
            .map(|item| (item.location().clone(), item))
            .collect::<HashMap<_, _>>();
        let records = this
            .inner
            .comments()
            .into_iter()
            .map(|comment| {
                let snapshot = anchors.remove(&comment.id()).ok_or_else(|| {
                    PyValueError::new_err(
                        "comment snapshot is missing its checked ownership record",
                    )
                })?;
                // A comment whose anchor cannot be read or placed keeps its
                // record. The reason is that comment's issue.
                let (anchor, anchor_text, anchor_issue) = match snapshot
                    .map_err(|error| error.to_string())
                    .and_then(|(range, text)| {
                        range
                            .map(|range| -> PyResult<PyStoryRunRange> {
                                Ok(PyStoryRunRange {
                                    start: this.comment_story_position(
                                        py,
                                        &range.start,
                                        &mut items,
                                        &owner,
                                    )?,
                                    end: this.comment_story_position(
                                        py, &range.end, &mut items, &owner,
                                    )?,
                                })
                            })
                            .transpose()
                            .map(|anchor| (anchor, text))
                            .map_err(|error| error.to_string())
                    }) {
                    Ok((anchor, text)) => (anchor, text, None),
                    Err(issue) => (None, None, Some(issue)),
                };
                Ok(PyComment {
                    id: comment.id(),
                    author: comment.author().map(str::to_owned),
                    initials: comment.initials().map(str::to_owned),
                    date: comment.date().map(str::to_owned),
                    text: comment.text(),
                    parent_id: comment.parent_id(),
                    resolved: comment.resolved(),
                    anchor_text,
                    anchor,
                    anchor_issue,
                })
            })
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, records)
    }

    #[getter]
    fn sections<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let sections = self
            .inner
            .sections()
            .map(section_snapshot)
            .collect::<Vec<_>>();
        PyTuple::new(py, sections)
    }

    #[pyo3(signature = (
        index,
        *,
        orientation = None,
        page_width = None,
        page_height = None,
        margin_top = None,
        margin_right = None,
        margin_bottom = None,
        margin_left = None,
        gutter = None,
        column_count = None,
        column_spacing = None,
        page_number_start = None,
        header_distance = None,
        footer_distance = None,
        different_first_page = None,
        break_type = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn update_section(
        &mut self,
        py: Python<'_>,
        index: usize,
        orientation: Option<&str>,
        page_width: Option<Bound<'_, PyAny>>,
        page_height: Option<Bound<'_, PyAny>>,
        margin_top: Option<Bound<'_, PyAny>>,
        margin_right: Option<Bound<'_, PyAny>>,
        margin_bottom: Option<Bound<'_, PyAny>>,
        margin_left: Option<Bound<'_, PyAny>>,
        gutter: Option<Bound<'_, PyAny>>,
        column_count: Option<u32>,
        column_spacing: Option<Bound<'_, PyAny>>,
        page_number_start: Option<u32>,
        header_distance: Option<Bound<'_, PyAny>>,
        footer_distance: Option<Bound<'_, PyAny>>,
        different_first_page: Option<bool>,
        break_type: Option<&str>,
    ) -> PyResult<PySection> {
        let page_width = checked_update_section_length(py, page_width.as_ref(), "page_width")?;
        let page_height = checked_update_section_length(py, page_height.as_ref(), "page_height")?;
        let margin_top = checked_update_section_length(py, margin_top.as_ref(), "margin_top")?;
        let margin_right =
            checked_update_section_length(py, margin_right.as_ref(), "margin_right")?;
        let margin_bottom =
            checked_update_section_length(py, margin_bottom.as_ref(), "margin_bottom")?;
        let margin_left = checked_update_section_length(py, margin_left.as_ref(), "margin_left")?;
        let gutter = checked_update_section_length(py, gutter.as_ref(), "gutter")?;
        let column_spacing =
            checked_update_section_length(py, column_spacing.as_ref(), "column_spacing")?;
        let header_distance =
            checked_update_section_length(py, header_distance.as_ref(), "header_distance")?;
        let footer_distance =
            checked_update_section_length(py, footer_distance.as_ref(), "footer_distance")?;
        let orientation = orientation
            .map(|value| {
                ST_PageOrientation::from_str(value)
                    .map_err(|_| PyValueError::new_err("orientation must be portrait or landscape"))
            })
            .transpose()?;
        let break_type = break_type
            .map(|value| {
                ST_SectionType::from_str(value).map_err(|_| {
                    PyValueError::new_err(
                        "break type must be nextPage, continuous, evenPage, oddPage or nextColumn",
                    )
                })
            })
            .transpose()?;
        let mut section = self
            .inner
            .section_mut(index)
            .ok_or_else(|| PyIndexError::new_err("section index out of range"))?;

        let current = section.properties();
        let defaults = oxoffice_docx_oxml::CT_SectPr::default_letter();
        let page_size = if page_width.is_some() || page_height.is_some() {
            Some((
                given_or_current_length(
                    page_width,
                    current.page_width,
                    defaults.page_width.unwrap(),
                ),
                given_or_current_length(
                    page_height,
                    current.page_height,
                    defaults.page_height.unwrap(),
                ),
            ))
        } else {
            None
        };
        let margins = if [margin_top, margin_right, margin_bottom, margin_left]
            .iter()
            .any(Option::is_some)
        {
            Some((
                given_or_current_length(
                    margin_top,
                    current.margin_top,
                    defaults.margin_top.unwrap(),
                ),
                given_or_current_length(
                    margin_right,
                    current.margin_right,
                    defaults.margin_right.unwrap(),
                ),
                given_or_current_length(
                    margin_bottom,
                    current.margin_bottom,
                    defaults.margin_bottom.unwrap(),
                ),
                given_or_current_length(
                    margin_left,
                    current.margin_left,
                    defaults.margin_left.unwrap(),
                ),
            ))
        } else {
            None
        };
        let columns = if column_count.is_some() || column_spacing.is_some() {
            let current = section.columns();
            if current.is_none()
                && section.properties().columns.is_some()
                && (column_count.is_none() || column_spacing.is_none())
            {
                return Err(PyValueError::new_err(
                    "unequal-width columns require both column_count and column_spacing",
                ));
            }
            let count = column_count
                .or(current.map(|(count, _)| count))
                .unwrap_or(1);
            let spacing = given_or_current_length(
                column_spacing,
                current.map(|(_, spacing)| spacing.as_twips()),
                oxoffice_docx::Twips(720),
            );
            Some((count, spacing))
        } else {
            None
        };
        let distances = if header_distance.is_some() || footer_distance.is_some() {
            Some((
                given_or_current_length(
                    header_distance,
                    current.header_distance,
                    defaults.header_distance.unwrap(),
                ),
                given_or_current_length(
                    footer_distance,
                    current.footer_distance,
                    defaults.footer_distance.unwrap(),
                ),
            ))
        } else {
            None
        };

        // The native setters check one value at a time, so restore the
        // section if a later value is rejected after an earlier one applied.
        let saved = section.properties().clone();
        let applied = (|| -> oxoffice_docx::Result<()> {
            if let Some((width, height)) = page_size {
                section.set_page_size(width, height)?;
            }
            if let Some(orientation) = orientation {
                section.set_orientation(orientation);
            }
            if let Some((top, right, bottom, left)) = margins {
                section.set_margins(top, right, bottom, left)?;
            }
            if let Some(gutter) = gutter {
                section.set_gutter(oxoffice_docx::Length::emu(gutter))?;
            }
            if let Some((count, spacing)) = columns {
                section.set_columns(count, spacing)?;
            }
            if let Some(start) = page_number_start {
                section.set_page_number_start(start)?;
            }
            if let Some((header, footer)) = distances {
                section.set_header_footer_distance(header, footer)?;
            }
            if let Some(enabled) = different_first_page {
                section.set_different_first_page(enabled);
            }
            if let Some(break_type) = break_type {
                section.set_break_type(break_type);
            }
            Ok(())
        })();
        if let Err(error) = applied {
            *section.properties_mut() = saved;
            return Err(docx_to_pyerr(py, error));
        }
        Ok(self
            .inner
            .sections()
            .nth(index)
            .map(section_snapshot)
            .expect("the updated section exists"))
    }

    #[pyo3(signature = (reference, text = ""))]
    fn add_footnote(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        reference: &Bound<'_, PyAny>,
        text: &str,
    ) -> PyResult<Py<PyNote>> {
        Self::create_note(slf, py, reference, text, false)
    }
    #[pyo3(signature = (reference, text = ""))]
    fn add_endnote(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        reference: &Bound<'_, PyAny>,
        text: &str,
    ) -> PyResult<Py<PyNote>> {
        Self::create_note(slf, py, reference, text, true)
    }

    #[pyo3(signature = (family, *, format="decimal", start=None, restart="continuous", placement=None))]
    fn set_note_policy(
        &mut self,
        py: Python<'_>,
        family: &str,
        format: &str,
        start: Option<&Bound<'_, PyAny>>,
        restart: &str,
        placement: Option<&str>,
    ) -> PyResult<()> {
        let (family, policy) = checked_note_policy(family, format, start, restart, placement)?;
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_note_policy(family, policy)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.set_note_policy",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    fn set_page_color(&mut self, py: Python<'_>, color: &str) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_page_color(color)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.set_page_color",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    /// Author the existing native VML compatibility watermark.
    fn set_text_watermark(&mut self, py: Python<'_>, text: &str) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_text_watermark(text)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.set_text_watermark",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    /// Author the existing native VML image watermark, with checked EMU dimensions.
    fn set_image_watermark(
        &mut self,
        py: Python<'_>,
        data: &[u8],
        filename: &str,
        width: &Bound<'_, PyAny>,
        height: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let width = checked_section_length(Some(width), "watermark width")?
            .ok_or_else(|| PyTypeError::new_err("watermark width is required"))?;
        let height = checked_section_length(Some(height), "watermark height")?
            .ok_or_else(|| PyTypeError::new_err("watermark height is required"))?;
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_image_watermark(
                data,
                filename,
                oxoffice_docx::Length::emu(width),
                oxoffice_docx::Length::emu(height),
            )
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.set_image_watermark",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    fn section(slf: &Bound<'_, Self>, index: usize) -> PyResult<PySectionHandle> {
        let mut document = slf.borrow_mut();
        if index >= document.inner.section_count() {
            return Err(PyIndexError::new_err("section index out of range"));
        }
        document.ensure_word_authority(slf.py())?;
        let (path, owner) = document.capture_owner(
            &WordOwnerLocation::Section(index),
            smallvec![],
            WordView::Owner,
            None,
        )?;
        Ok(PySectionHandle {
            document: slf.clone().unbind(),
            index,
            path,
            owner,
        })
    }
    #[getter]
    fn settings(slf: &Bound<'_, Self>) -> PySettings {
        PySettings {
            document: slf.clone().unbind(),
        }
    }

    fn insert_section(&mut self, py: Python<'_>, index: usize) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        if index > candidate.section_count() {
            return Err(PyIndexError::new_err("section index out of range"));
        }
        candidate
            .insert_section(index)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.insert_section",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    fn remove_section(&mut self, py: Python<'_>, index: usize) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        if index >= candidate.section_count() {
            return Err(PyIndexError::new_err("section index out of range"));
        }
        candidate
            .remove_section(index)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.remove_section",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    #[getter]
    fn styles<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let snapshots = PyTuple::new(py, self.inner.styles().into_iter().map(style_snapshot))?;
        py.import("oxoffice.docx")?
            .getattr("_StyleSnapshots")?
            .call1((snapshots,))?
            .cast_into::<PyTuple>()
            .map_err(Into::into)
    }

    // Create a style as python-docx's `styles.add_style` does, deriving the
    // ID from the name unless one is given, with the formatting given as
    // keywords. Styles change no content, so handles stay valid.
    #[pyo3(signature = (
        name,
        style_type = "paragraph",
        *,
        style_id = None,
        based_on = None,
        next_style = None,
        font_name = None,
        font_size = None,
        bold = None,
        italic = None,
        color = None,
        space_before = None,
        space_after = None,
        left_indent = None,
        right_indent = None,
        first_line_indent = None,
        alignment = None,
        line_spacing = None,
        underline = None,
        keep_with_next = None,
        keep_together = None,
        page_break_before = None,
        borders = None,
        shading = None,
        tab_stops = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn add_style(
        &mut self,
        py: Python<'_>,
        name: &str,
        style_type: &str,
        style_id: Option<&str>,
        based_on: Option<&str>,
        next_style: Option<&str>,
        font_name: Option<String>,
        font_size: Option<i64>,
        bold: Option<bool>,
        italic: Option<bool>,
        color: Option<&Bound<'_, PyAny>>,
        space_before: Option<i64>,
        space_after: Option<i64>,
        left_indent: Option<i64>,
        right_indent: Option<i64>,
        first_line_indent: Option<i64>,
        alignment: Option<i32>,
        line_spacing: Option<&Bound<'_, PyAny>>,
        underline: Option<&Bound<'_, PyAny>>,
        keep_with_next: Option<bool>,
        keep_together: Option<bool>,
        page_break_before: Option<bool>,
        borders: Option<&Bound<'_, PyAny>>,
        shading: Option<&Bound<'_, PyAny>>,
        tab_stops: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyStyle> {
        let color = color
            .filter(|value| !value.is_none())
            .map(|value| oxoffice_py_support::rgb_channels(value, "Document.add_style(color)"))
            .transpose()?;
        type NewStyle = fn(&str, &str) -> oxoffice_docx::StyleBuilder;
        let (native_type, new_style): (oxoffice_docx::StyleType, NewStyle) = match style_type {
            "paragraph" => (
                oxoffice_docx::StyleType::Paragraph,
                oxoffice_docx::StyleBuilder::paragraph,
            ),
            "character" => (
                oxoffice_docx::StyleType::Character,
                oxoffice_docx::StyleBuilder::character,
            ),
            "table" => (
                oxoffice_docx::StyleType::Table,
                oxoffice_docx::StyleBuilder::table,
            ),
            _ => {
                return Err(PyValueError::new_err(
                    "style_type must be 'paragraph', 'character' or 'table'",
                ));
            }
        };
        let style_id =
            style_id.map_or_else(|| style_id_from_name(&self.inner, name), str::to_owned);
        if name.trim().is_empty() || style_id.trim().is_empty() {
            return Err(PyValueError::new_err(
                "a style name and style ID cannot be blank",
            ));
        }
        if self.inner.style(&style_id).is_some() {
            return Err(PyValueError::new_err(format!(
                "a style with the ID '{style_id}' already exists"
            )));
        }
        let lowered = name.to_lowercase();
        if self.inner.styles().iter().any(|style| {
            style
                .name()
                .is_some_and(|name| name.to_lowercase() == lowered)
        }) {
            return Err(PyValueError::new_err(format!(
                "a style named '{name}' already exists"
            )));
        }
        let formatting = StyleFormatting {
            font_name,
            font_size,
            bold,
            italic,
            color,
            space_before,
            space_after,
            left_indent,
            right_indent,
            first_line_indent,
            alignment,
            line_spacing: line_spacing
                .filter(|v| !v.is_none())
                .map(|v| Some(v.clone().unbind())),
            underline: underline
                .filter(|v| !v.is_none())
                .map(|v| v.clone().unbind()),
            keep_with_next,
            keep_together,
            page_break_before,
            borders: borders.filter(|v| !v.is_none()).map(|v| v.clone().unbind()),
            shading: shading.filter(|v| !v.is_none()).map(|v| v.clone().unbind()),
            tab_stops: tab_stops
                .filter(|v| !v.is_none())
                .map(|v| v.clone().unbind()),
        };
        let mut builder = new_style(&style_id, name);
        if let Some(parent) = based_on {
            builder = builder.based_on(&style_id_of_type(&self.inner, parent, native_type)?);
        }
        if let Some(next) = next_style {
            if native_type != oxoffice_docx::StyleType::Paragraph {
                return Err(PyValueError::new_err(
                    "only a paragraph style has a next style",
                ));
            }
            builder = builder.next_style(&style_id_of_type(
                &self.inner,
                next,
                oxoffice_docx::StyleType::Paragraph,
            )?);
        }
        if let Some(properties) = formatting.paragraph_properties(py, None)? {
            if native_type == oxoffice_docx::StyleType::Character {
                return Err(PyValueError::new_err(
                    "a character style takes no paragraph formatting",
                ));
            }
            builder = builder.replace_paragraph_properties(properties);
        }
        if let Some(properties) = formatting.run_properties(py, None)? {
            builder = builder.run_properties(properties);
            if let Some(name) = &formatting.font_name {
                builder = builder.font(name);
            }
            if let Some((r, g, b)) = formatting.color {
                builder = builder.color(&format!("{r:02X}{g:02X}{b:02X}"));
            }
        }
        self.inner
            .add_style(builder)
            .map_err(|error| docx_to_pyerr(py, error))?;
        Ok(style_snapshot(
            self.inner
                .style(&style_id)
                .expect("the style was just added"),
        ))
    }

    // Update only the supplied formatting on a defined style. The native
    // setter stages and validates the complete style graph before publishing.
    #[pyo3(signature = (
        style,
        *,
        based_on = None,
        next_style = None,
        font_name = None,
        font_size = None,
        bold = None,
        italic = None,
        color = None,
        space_before = None,
        space_after = None,
        left_indent = None,
        right_indent = None,
        first_line_indent = None,
        alignment = None,
        line_spacing = StyleLineSpacingArg::Omitted,
        underline = None,
        keep_with_next = None,
        keep_together = None,
        page_break_before = None,
        borders = None,
        shading = None,
        tab_stops = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn set_style(
        &mut self,
        py: Python<'_>,
        style: &str,
        based_on: Option<&str>,
        next_style: Option<&str>,
        font_name: Option<String>,
        font_size: Option<i64>,
        bold: Option<bool>,
        italic: Option<bool>,
        color: Option<&Bound<'_, PyAny>>,
        space_before: Option<i64>,
        space_after: Option<i64>,
        left_indent: Option<i64>,
        right_indent: Option<i64>,
        first_line_indent: Option<i64>,
        alignment: Option<i32>,
        line_spacing: StyleLineSpacingArg,
        underline: Option<&Bound<'_, PyAny>>,
        keep_with_next: Option<bool>,
        keep_together: Option<bool>,
        page_break_before: Option<bool>,
        borders: Option<&Bound<'_, PyAny>>,
        shading: Option<&Bound<'_, PyAny>>,
        tab_stops: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyStyle> {
        let color = color
            .filter(|value| !value.is_none())
            .map(|value| oxoffice_py_support::rgb_channels(value, "Document.set_style(color)"))
            .transpose()?;
        let (style_type, style_id) = defined_style(&self.inner, style)?;
        let existing = self.inner.style(&style_id).expect("the style was found");
        let name = existing.name().unwrap_or(&style_id).to_owned();
        let mut builder = match style_type {
            oxoffice_docx::StyleType::Paragraph => {
                oxoffice_docx::StyleBuilder::paragraph(&style_id, &name)
            }
            oxoffice_docx::StyleType::Character => {
                oxoffice_docx::StyleBuilder::character(&style_id, &name)
            }
            oxoffice_docx::StyleType::Table => oxoffice_docx::StyleBuilder::table(&style_id, &name),
            _ => return Err(PyValueError::new_err("unsupported style type")),
        };
        if let Some(parent) = based_on {
            builder = builder.based_on(&style_id_of_type(&self.inner, parent, style_type)?);
        }
        if let Some(next) = next_style {
            if style_type != oxoffice_docx::StyleType::Paragraph {
                return Err(PyValueError::new_err(
                    "only a paragraph style has a next style",
                ));
            }
            builder = builder.next_style(&style_id_of_type(
                &self.inner,
                next,
                oxoffice_docx::StyleType::Paragraph,
            )?);
        }
        let formatting = StyleFormatting {
            font_name,
            font_size,
            bold,
            italic,
            color,
            space_before,
            space_after,
            left_indent,
            right_indent,
            first_line_indent,
            alignment,
            line_spacing: match line_spacing {
                StyleLineSpacingArg::Omitted => None,
                StyleLineSpacingArg::Given(value) => Some(value),
            },
            underline: underline
                .filter(|v| !v.is_none())
                .map(|v| v.clone().unbind()),
            keep_with_next,
            keep_together,
            page_break_before,
            borders: borders.filter(|v| !v.is_none()).map(|v| v.clone().unbind()),
            shading: shading.filter(|v| !v.is_none()).map(|v| v.clone().unbind()),
            tab_stops: tab_stops
                .filter(|v| !v.is_none())
                .map(|v| v.clone().unbind()),
        };
        if let Some(properties) =
            formatting.paragraph_properties(py, existing.paragraph_properties())?
        {
            if style_type == oxoffice_docx::StyleType::Character {
                return Err(PyValueError::new_err(
                    "a character style takes no paragraph formatting",
                ));
            }
            builder = builder.replace_paragraph_properties(properties);
        }
        if let Some(properties) = formatting.run_properties(py, existing.run_properties())? {
            builder = builder.run_properties(properties);
            if let Some(name) = &formatting.font_name {
                builder = builder.font(name);
            }
            if let Some((r, g, b)) = formatting.color {
                builder = builder.color(&format!("{r:02X}{g:02X}{b:02X}"));
            }
        }
        self.inner
            .set_style(builder)
            .map_err(|error| docx_to_pyerr(py, error))?;
        Ok(style_snapshot(
            self.inner
                .style(&style_id)
                .expect("the style was just updated"),
        ))
    }

    // Return false when no style has the ID or name. The native call refuses
    // a style that content, another style or a numbering level still uses.
    fn remove_style(&mut self, py: Python<'_>, style: &str) -> PyResult<bool> {
        let Ok((_, style_id)) = defined_style(&self.inner, style) else {
            return Ok(false);
        };
        self.inner
            .remove_style(&style_id)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    fn set_default_style(&mut self, py: Python<'_>, style: &str) -> PyResult<()> {
        let (style_type, style_id) = defined_style(&self.inner, style)?;
        self.inner
            .set_default_style(style_type, &style_id)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    #[getter]
    fn default_paragraph_format(&self, py: Python<'_>) -> PyResult<PyDefaultParagraphFormat> {
        default_paragraph_format_snapshot(py, self.inner.default_paragraph_properties())
    }

    #[pyo3(signature = (**kwargs))]
    fn set_default_paragraph_format(
        &mut self,
        py: Python<'_>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        let Some(kwargs) = kwargs else {
            return Ok(());
        };
        for (key, _) in kwargs.iter() {
            let key = key.extract::<String>()?;
            if !matches!(
                key.as_str(),
                "space_before" | "space_after" | "line_spacing"
            ) {
                return Err(PyTypeError::new_err(format!(
                    "set_default_paragraph_format() got an unexpected keyword argument '{key}'"
                )));
            }
        }
        let mut next = self
            .inner
            .default_paragraph_properties()
            .cloned()
            .unwrap_or_default();
        for name in ["space_before", "space_after", "line_spacing"] {
            let Some(value) = kwargs.get_item(name)? else {
                continue;
            };
            match name {
                "space_before" => next.space_before = length_keyword(py, &value, name)?,
                "space_after" => next.space_after = length_keyword(py, &value, name)?,
                "line_spacing" => {
                    let line = line_spacing_keyword(py, &value)?;
                    // A missing rule already means auto, so the same multiple
                    // writes back without adding one.
                    let kept = line.is_some_and(|(twips, rule)| {
                        rule == "auto"
                            && next.line_rule.is_none()
                            && next.line_spacing == Some(twips)
                    });
                    next.line_spacing = line.map(|(twips, _)| twips);
                    if !kept {
                        next.line_rule = line.map(|(_, rule)| rule.to_owned());
                    }
                }
                _ => unreachable!("keyword was checked above"),
            }
        }
        let current = self.inner.default_paragraph_properties().cloned();
        if current.as_ref() == Some(&next)
            || (current.is_none() && next == oxoffice_docx::CT_PPr::default())
        {
            return Ok(());
        }
        self.inner
            .set_default_paragraph_properties(Some(next))
            .map_err(|error| docx_to_pyerr(py, error))
    }

    fn part_names<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner
                .part_names()
                .map_err(|error| docx_to_pyerr(py, error))?,
        )
    }

    fn part_xml(&self, py: Python<'_>, name: &str) -> PyResult<String> {
        self.inner
            .part_xml(name)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    fn add_numbering_definition(
        &mut self,
        py: Python<'_>,
        levels: Vec<PyRef<'_, PyListLevel>>,
    ) -> PyResult<u32> {
        let levels = levels
            .iter()
            .map(|level| level.native())
            .collect::<Vec<_>>();
        self.inner
            .add_numbering_definition(&levels)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    #[pyo3(signature = (definition_id, *, start = None, level = 0))]
    fn add_numbering_instance(
        &mut self,
        py: Python<'_>,
        definition_id: u32,
        start: Option<i64>,
        level: i64,
    ) -> PyResult<u32> {
        if !(0..=8).contains(&level) {
            return Err(PyValueError::new_err("level must be between 0 and 8"));
        }
        if start.is_none() && level != 0 {
            return Err(PyValueError::new_err("nonzero level requires start"));
        }
        let overrides = start
            .map(|start| {
                let start = u32::try_from(start)
                    .map_err(|_| PyValueError::new_err("start must be between 0 and 4294967295"))?;
                Ok::<_, PyErr>(
                    oxoffice_docx::NumberingLevelOverride::new(level as u32).start(start),
                )
            })
            .transpose()?
            .into_iter()
            .collect::<Vec<_>>();
        self.inner
            .add_numbering_instance(definition_id, &overrides)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    #[getter]
    fn charts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let values = self
            .inner
            .charts()
            .map_err(|e| docx_to_pyerr(py, e))?
            .into_iter()
            .map(|(chart_type, data)| Py::new(py, PyChart { chart_type, data }))
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, values)
    }

    #[pyo3(signature = (paragraph, *, start=Python::attach(|py| PyInt::new(py, 1).into_any().unbind())), text_signature = "($self, paragraph, *, start=1)")]
    fn restart_numbering(
        slf: Py<Self>,
        py: Python<'_>,
        paragraph: PyRef<'_, PyParagraph>,
        start: Py<PyAny>,
    ) -> PyResult<u32> {
        let start = start.bind(py);
        if !start.is_instance_of::<PyInt>() || start.is_instance_of::<PyBool>() {
            return Err(PyTypeError::new_err("start must be a non-bool integer"));
        }
        let start = start.extract::<u32>()?;
        if !paragraph.belongs_to(py, &slf) {
            return Err(PyValueError::new_err(
                "paragraph belongs to a different document",
            ));
        }
        let ParagraphLocation::Body(index) = paragraph.validate(py)? else {
            return Err(PyValueError::new_err(
                "numbering restart requires a direct body paragraph",
            ));
        };
        let mut document = slf.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        let id = candidate
            .restart_numbering(index, start)
            .map_err(|e| docx_to_pyerr(py, e))?;
        document.publish_api_candidate(py, candidate, "Document.restart_numbering")?;
        Ok(id)
    }

    fn remove_footnote(&mut self, py: Python<'_>, id: &Bound<'_, PyAny>) -> PyResult<()> {
        self.remove_note_by_id(py, id, false)
    }

    fn remove_endnote(&mut self, py: Python<'_>, id: &Bound<'_, PyAny>) -> PyResult<()> {
        self.remove_note_by_id(py, id, true)
    }

    /// Effective legend properties of the live direct-body chart paragraph.
    fn chart_legend(
        slf: Py<Self>,
        py: Python<'_>,
        paragraph: PyRef<'_, PyParagraph>,
    ) -> PyResult<Py<PyDict>> {
        if !paragraph.belongs_to(py, &slf) {
            return Err(PyValueError::new_err(
                "paragraph belongs to a different document",
            ));
        }
        let ParagraphLocation::Body(index) = paragraph.validate(py)? else {
            return Err(PyValueError::new_err(
                "chart legend requires a direct body paragraph",
            ));
        };
        let settings = slf
            .borrow(py)
            .inner
            .chart_legend(index)
            .map_err(|e| docx_to_pyerr(py, e))?;
        let result = PyDict::new(py);
        result.set_item("visible", settings.visible)?;
        result.set_item("position", settings.position.as_str())?;
        result.set_item("overlay", settings.overlay)?;
        Ok(result.unbind())
    }

    /// None leaves a property unchanged. Hidden legends discard their settings.
    #[pyo3(signature=(paragraph, *, visible=None, position=None, overlay=None))]
    fn set_chart_legend(
        slf: Py<Self>,
        py: Python<'_>,
        paragraph: PyRef<'_, PyParagraph>,
        visible: Option<&Bound<'_, PyAny>>,
        position: Option<&str>,
        overlay: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let checked_bool = |value: Option<&Bound<'_, PyAny>>| -> PyResult<Option<bool>> {
            value
                .map(|v| {
                    if !v.is_instance_of::<PyBool>() {
                        return Err(PyTypeError::new_err(
                            "legend visibility and overlay require bool",
                        ));
                    }
                    v.extract()
                })
                .transpose()
        };
        let visible = checked_bool(visible)?;
        let overlay = checked_bool(overlay)?;
        let position = position
            .map(oxoffice_docx::LegendPosition::parse)
            .transpose()
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        if !paragraph.belongs_to(py, &slf) {
            return Err(PyValueError::new_err(
                "paragraph belongs to a different document",
            ));
        }
        let ParagraphLocation::Body(index) = paragraph.validate(py)? else {
            return Err(PyValueError::new_err(
                "chart legend requires a direct body paragraph",
            ));
        };
        let mut document = slf.borrow_mut(py);
        let location = document
            .paragraph_route(&ParagraphLocation::Body(index), None)
            .map_err(|e| docx_to_pyerr(py, e))?;
        let mut candidate = document.inner.clone_for_staging();
        candidate
            .set_chart_legend(index, visible, position, overlay)
            .map_err(|e| docx_to_pyerr(py, e))?;
        document.publish_owner_candidate(
            py,
            candidate,
            "Document.set_chart_legend",
            WordPublication::ElementXml(WordOwnerLocation::Paragraph(location)),
        )
    }

    #[pyo3(signature = (chart_type, chart_data, width, height, *, after=None))]
    fn add_chart(
        slf: Py<Self>,
        py: Python<'_>,
        chart_type: &Bound<'_, PyAny>,
        chart_data: &Bound<'_, PyAny>,
        width: &Bound<'_, PyAny>,
        height: &Bound<'_, PyAny>,
        after: Option<PyRef<'_, PyParagraph>>,
    ) -> PyResult<Py<PyParagraph>> {
        let (kind, data) = checked_chart_inputs(chart_type, chart_data)?;
        let width = oxoffice_docx::Length::emu(
            checked_section_length(Some(width), "chart width")?.unwrap(),
        );
        let height = oxoffice_docx::Length::emu(
            checked_section_length(Some(height), "chart height")?.unwrap(),
        );
        let after = after
            .map(|paragraph| {
                if !paragraph.belongs_to(py, &slf) {
                    return Err(PyValueError::new_err(
                        "paragraph belongs to a different document",
                    ));
                }
                let ParagraphLocation::Body(index) = paragraph.validate(py)? else {
                    return Err(PyValueError::new_err(
                        "chart insertion requires a direct body paragraph",
                    ));
                };
                Ok(index)
            })
            .transpose()?;
        let mut document = slf.borrow_mut(py);
        let index = after.map_or(document.inner.paragraph_count(), |index| index + 1);
        let mut candidate = document.inner.clone_for_staging();
        match after {
            Some(after) => candidate.add_chart_after(after, kind, width, height, &data),
            None => candidate.add_chart(kind, width, height, &data),
        }
        .map(drop)
        .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.add_chart",
            if after.is_none() {
                WordPublication::AppendBody
            } else {
                WordPublication::Conservative
            },
        )?;
        let path = document
            .revisions
            .capture_prepared(
                &prepared.revision,
                smallvec![PathSeg::Body(0), PathSeg::Para(index)],
            )
            .map_err(revision_to_pyerr)?;
        let result = Py::new(
            py,
            PyParagraph::new(slf.clone_ref(py), path)
                .bind_prepared(py, &document, &prepared, &candidate, None)?,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(result)
    }

    #[pyo3(signature = (text, level = 0))]
    fn add_bullet_list_item(
        slf: Py<Self>,
        py: Python<'_>,
        text: &str,
        level: i64,
    ) -> PyResult<Py<PyParagraph>> {
        if !(0..=8).contains(&level) {
            return Err(PyValueError::new_err("level must be between 0 and 8"));
        }
        let mut document = slf.borrow_mut(py);
        let index = document.inner.paragraph_count();
        let mut candidate = document.inner.clone_for_staging();
        candidate
            .try_add_bullet_list_item(text, level as u32)
            .map(|_| ())
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.add_bullet_list_item",
            crate::document::WordPublication::Conservative,
        )?;
        let path = document
            .revisions
            .capture_prepared(
                &prepared.revision,
                smallvec![PathSeg::Body(0), PathSeg::Para(index)],
            )
            .map_err(oxoffice_py_support::revision_to_pyerr)?;
        let result = Py::new(
            py,
            PyParagraph::new(slf.clone_ref(py), path)
                .bind_prepared(py, &document, &prepared, &candidate, None)?,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(result)
    }

    #[pyo3(signature = (text, level = 0, *, restart = false))]
    fn add_numbered_list_item(
        slf: Py<Self>,
        py: Python<'_>,
        text: &str,
        level: i64,
        restart: bool,
    ) -> PyResult<Py<PyParagraph>> {
        if !(0..=8).contains(&level) {
            return Err(PyValueError::new_err("level must be between 0 and 8"));
        }
        let mut document = slf.borrow_mut(py);
        let index = document.inner.paragraph_count();
        let mut candidate = document.inner.clone_for_staging();
        candidate
            .try_add_numbered_list_item_with_restart(text, level as u32, restart)
            .map(|_| ())
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.add_numbered_list_item",
            crate::document::WordPublication::Conservative,
        )?;
        let path = document
            .revisions
            .capture_prepared(
                &prepared.revision,
                smallvec![PathSeg::Body(0), PathSeg::Para(index)],
            )
            .map_err(oxoffice_py_support::revision_to_pyerr)?;
        let result = Py::new(
            py,
            PyParagraph::new(slf.clone_ref(py), path)
                .bind_prepared(py, &document, &prepared, &candidate, None)?,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(result)
    }

    fn link_style_to_numbering(
        &mut self,
        py: Python<'_>,
        style: &str,
        num_id: u32,
        level: u32,
    ) -> PyResult<()> {
        let style_id = style_id_of_type(&self.inner, style, oxoffice_docx::StyleType::Paragraph)?;
        self.inner
            .link_style_to_numbering(&style_id, num_id, level)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    #[getter]
    fn stories<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        self.ensure_word_authority(py)?;
        let stories = self
            .inner
            .stories()
            .map_err(|error| docx_to_pyerr(py, error))?;
        PyTuple::new(
            py,
            stories
                .iter()
                .map(|story| self.owned_story_snapshot(story))
                .collect::<PyResult<Vec<_>>>()?,
        )
    }

    #[getter]
    fn story_items<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyTuple>> {
        let py = slf.py();
        let owner = std::sync::Arc::new(slf.clone().unbind());
        let mut this = slf.borrow_mut();
        this.ensure_word_authority(py)?;
        let items = this
            .inner
            .standalone_story_item_snapshots()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let snapshots = items
            .into_iter()
            .map(|item| {
                Ok(PyStoryItem {
                    story: this.owned_story_snapshot(item.location().story())?,
                    kind: story_item_kind_name(item.location().item_kind()).to_owned(),
                    index_path: item.location().index_path().to_vec(),
                    direct_body_index: item.direct_body_index(),
                    text: item.text().map(str::to_owned),
                    xml: item.xml().to_vec(),
                    revision: this.revisions.current(),
                    binding: Some(this.capture_snapshot(smallvec![])?),
                    document: Some(owner.clone()),
                })
            })
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, snapshots)
    }

    #[getter]
    fn story_paragraphs<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        self.ensure_word_authority(py)?;
        let paragraphs = py
            .detach(|| self.inner.story_paragraphs())
            .map_err(|error| docx_to_pyerr(py, error))?;
        let snapshots = paragraphs
            .into_iter()
            .map(|paragraph| {
                Ok(PyStoryParagraph {
                    kind: story_kind_name(paragraph.kind).to_owned(),
                    story: self.owned_story_snapshot(paragraph.location.story())?,
                    index_path: paragraph.location.index_path().to_vec(),
                    style: paragraph.style,
                    text: paragraph.text,
                })
            })
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, snapshots)
    }

    fn count_text<'py>(&self, py: Python<'py>, text: &str) -> PyResult<Bound<'py, PyDict>> {
        if text.is_empty() {
            return Err(PyValueError::new_err("count_text needs a text to count"));
        }
        let counts = py
            .detach(|| self.inner.count_text(text))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let by_kind = PyDict::new(py);
        for (kind, count) in counts {
            by_kind.set_item(story_kind_name(kind), count)?;
        }
        Ok(by_kind)
    }

    #[getter]
    fn header_footer_variants<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        self.ensure_word_authority(py)?;
        let mut snapshots = Vec::new();
        for section_index in 0..self.inner.section_count() {
            for (kind, kind_name) in [
                (oxoffice_docx::HeaderFooterKind::Header, "header"),
                (oxoffice_docx::HeaderFooterKind::Footer, "footer"),
            ] {
                for variant in [
                    oxoffice_docx::HdrFtrType::Default,
                    oxoffice_docx::HdrFtrType::First,
                    oxoffice_docx::HdrFtrType::Even,
                ] {
                    let resolved = self
                        .inner
                        .section_story(section_index, kind, variant)
                        .map_err(|error| docx_to_pyerr(py, error))?;
                    snapshots.push(PyHeaderFooterVariant {
                        section_index,
                        kind: kind_name.to_owned(),
                        variant: variant.to_str().to_owned(),
                        story: resolved
                            .as_ref()
                            .map(|value| self.owned_story_snapshot(value.story()))
                            .transpose()?,
                        source_section: resolved
                            .as_ref()
                            .map(oxoffice_docx::SectionStory::source_section),
                        inherited: resolved.is_some_and(|value| value.is_inherited()),
                    });
                }
            }
        }
        PyTuple::new(py, snapshots)
    }

    // The section story operations publish a reopened package, so each one
    // advances the revision once, even when the variant already had a story.
    fn create_section_story(
        &mut self,
        py: Python<'_>,
        section_index: usize,
        kind: &str,
        variant: &str,
    ) -> PyResult<Py<PyStory>> {
        let (kind, variant) = self.section_story_target(section_index, kind, variant)?;
        let mut candidate = self.inner.clone_for_staging();
        let story = py
            .detach(|| candidate.create_section_story(section_index, kind, variant))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.create_section_story",
            crate::document::WordPublication::Conservative,
        )?;
        let mut snapshot = story_snapshot(&story);
        snapshot.binding = Some(self.capture_prepared_snapshot(&prepared, smallvec![])?);
        let result = Py::new(py, snapshot)?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(result)
    }

    fn link_section_story(
        &mut self,
        py: Python<'_>,
        section_index: usize,
        kind: &str,
        variant: &str,
        story: PyRef<'_, PyStory>,
    ) -> PyResult<Py<PyStory>> {
        let (kind, variant) = self.section_story_target(section_index, kind, variant)?;
        let story = self.native_story(py, &story)?;
        let mut candidate = self.inner.clone_for_staging();
        let linked = py
            .detach(|| candidate.link_section_story(section_index, kind, variant, &story))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.link_section_story",
            crate::document::WordPublication::Conservative,
        )?;
        let mut snapshot = story_snapshot(&linked);
        snapshot.binding = Some(self.capture_prepared_snapshot(&prepared, smallvec![])?);
        let result = Py::new(py, snapshot)?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(result)
    }

    fn unlink_section_story(
        &mut self,
        py: Python<'_>,
        section_index: usize,
        kind: &str,
        variant: &str,
    ) -> PyResult<Py<PyStory>> {
        let (kind, variant) = self.section_story_target(section_index, kind, variant)?;
        let mut candidate = self.inner.clone_for_staging();
        let story = py
            .detach(|| candidate.unlink_section_story(section_index, kind, variant))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.unlink_section_story",
            crate::document::WordPublication::Conservative,
        )?;
        let mut snapshot = story_snapshot(&story);
        snapshot.binding = Some(self.capture_prepared_snapshot(&prepared, smallvec![])?);
        let result = Py::new(py, snapshot)?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(result)
    }

    #[getter]
    fn hyperlinks<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        self.ensure_word_authority(py)?;
        let links = self
            .inner
            .story_link_snapshots()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let mut story_links = HashMap::<_, Vec<_>>::new();
        for (location, link) in &links {
            story_links
                .entry(location.story().clone())
                .or_default()
                .push((location.index_path(), link.text.as_str()));
        }
        let mut layouts = story_links
            .into_iter()
            .map(|(story, links)| (story, (0usize, story_link_layout(links))))
            .collect::<HashMap<_, _>>();
        let snapshots = links
            .into_iter()
            .map(|(location, link)| {
                let (next, layout) = layouts
                    .get_mut(location.story())
                    .expect("every snapshot story has a layout");
                let position = Some((*next, *layout));
                let owner = self
                    .owners
                    .iter()
                    .find(|owner| {
                        matches!(&owner.location, WordOwnerLocation::Hyperlink(current, _)
                        if same_content_owner(current, &location))
                            && owner.projection_ordinal == Some(*next)
                    })
                    .ok_or_else(|| {
                        PyValueError::new_err("hyperlink has no checked physical source owner")
                    })?;
                let binding = self
                    .revisions
                    .capture_scopes(smallvec![], std::slice::from_ref(&owner.owner))
                    .map_err(revision_to_pyerr)?;
                let owner = Some((binding, owner.owner.clone()));
                *next += 1;
                Ok(PyHyperlink {
                    story: self.owned_story_snapshot(location.story())?,
                    index_path: location.index_path().to_vec(),
                    text: link.text,
                    url: link.url,
                    anchor: link.anchor,
                    relationship_id: link.rel_id,
                    position,
                    owner,
                })
            })
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, snapshots)
    }

    fn set_hyperlink_url(
        &mut self,
        py: Python<'_>,
        hyperlink: PyRef<'_, PyHyperlink>,
        url: &str,
    ) -> PyResult<()> {
        let (story, link_index) = self.native_hyperlink(py, &hyperlink)?;
        // No content moves, so live handles stay valid.
        py.detach(|| self.inner.set_hyperlink_url(&story, link_index, url))
            .map_err(|error| docx_to_pyerr(py, error))
    }

    fn remove_hyperlink(
        &mut self,
        py: Python<'_>,
        hyperlink: PyRef<'_, PyHyperlink>,
    ) -> PyResult<()> {
        let (story, link_index) = self.native_hyperlink(py, &hyperlink)?;
        let mut candidate = self.inner.clone_for_staging();
        py.detach(|| candidate.remove_hyperlink(&story, link_index))
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.ensure_word_authority(py)?;
        let selected = self
            .owners
            .iter()
            .find_map(|owner| match &owner.location {
                WordOwnerLocation::Hyperlink(location, _)
                    if same_story_owner(location.story(), &story)
                        && owner.projection_ordinal == Some(link_index) =>
                {
                    Some(owner.location.clone())
                }
                _ => None,
            })
            .ok_or_else(|| {
                PyValueError::new_err("hyperlink has no checked physical source owner")
            })?;
        self.publish_owner_candidate(
            py,
            candidate,
            "Document.remove_hyperlink",
            WordPublication::HyperlinkRemove(selected),
        )
    }

    fn set_header(&mut self, py: Python<'_>, text: &str) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        py.detach(|| candidate.try_set_header(text))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.set_header",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    fn set_footer(&mut self, py: Python<'_>, text: &str) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        py.detach(|| candidate.try_set_footer(text))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.set_footer",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    fn set_story_text(
        &mut self,
        py: Python<'_>,
        item: PyRef<'_, PyStoryItem>,
        text: &str,
    ) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        let location = self.native_location(py, &item)?;
        py.detach(|| candidate.set_story_text(&location, text))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.set_story_text",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    fn add_hyperlink_to_story(
        &mut self,
        py: Python<'_>,
        story: PyRef<'_, PyStory>,
        text: &str,
        url: &str,
    ) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        let story = self.native_story(py, &story)?;
        py.detach(|| candidate.add_hyperlink_to_story(&story, text, url))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.add_hyperlink_to_story",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // Preserve explicit checked public picture operands.
    #[pyo3(signature = (data, filename=None, width=None, height=None, *, after=None, options=None))]
    fn add_picture(
        &mut self,
        py: Python<'_>,
        data: &Bound<'_, PyAny>,
        filename: Option<&str>,
        width: Option<&Bound<'_, PyAny>>,
        height: Option<&Bound<'_, PyAny>>,
        after: Option<&Bound<'_, PyAny>>,
        options: Option<PyRef<'_, PyPictureOptions>>,
    ) -> PyResult<Py<PyPicture>> {
        let (data, filename) = picture_input(data, filename)?;
        let width = checked_section_length(width, "picture width")?.map(oxoffice_docx::Length::emu);
        let height =
            checked_section_length(height, "picture height")?.map(oxoffice_docx::Length::emu);
        if width.is_some() != height.is_some() {
            return Err(crate::docx_to_pyerr(
                py,
                oxoffice_docx::Error::Other(
                    "use add_picture(..., width=..., height=...) with both dimensions; omit both for the native image size".to_owned(),
                ),
            ));
        }
        let after = after
            .map(|value| -> PyResult<oxoffice_docx::ContentLocation> {
                if let Ok(item) = value.extract::<PyRef<'_, PyStoryItem>>() {
                    return self.native_location(py, &item);
                }
                if let Ok(picture) = value.extract::<PyRef<'_, PyPicture>>() {
                    picture
                        .binding
                        .validate_scopes(
                            &self.revisions,
                            "picture",
                            "Re-fetch it with document.pictures().",
                        )
                        .map_err(|error| crate::stale_to_pyerr(py, error))?;
                    let current = self
                        .inner
                        .pictures()
                        .map_err(|error| docx_to_pyerr(py, error))?;
                    if !current.contains(&picture.inner) {
                        return Err(PyValueError::new_err("picture placement source changed"));
                    }
                    return Ok(picture.inner.location.clone());
                }
                Err(PyTypeError::new_err("after must be a StoryItem or Picture"))
            })
            .transpose()?;
        let story = match &after {
            Some(location) => location.story().clone(),
            None => self.body_story(py)?,
        };
        let candidate = self.inner.clone_for_staging();
        let (candidate, location, pictures) = candidate
            .insert_picture_to_story_with_options_and_inventory_owned(
                &story,
                after.as_ref(),
                &data,
                &filename,
                width,
                height,
                options.map(|options| options.inner.clone()),
            )
            .map_err(|error| docx_to_pyerr(py, error))?;
        let picture = pictures
            .into_iter()
            .find(|picture| picture.location == location)
            .ok_or_else(|| PyIndexError::new_err("inserted picture was not inventoried"))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.add_picture",
            crate::document::WordPublication::Conservative,
        )?;
        let result = Py::new(
            py,
            PyPicture {
                inner: picture,
                binding: self.capture_prepared_snapshot(&prepared, smallvec![])?,
            },
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(result)
    }

    #[pyo3(signature = (range, *, author, text, initials = None, date = None, undated = false))]
    #[allow(clippy::too_many_arguments)]
    fn add_comment(
        &mut self,
        range: &Bound<'_, PyAny>,
        author: &str,
        text: &str,
        initials: Option<&str>,
        date: Option<&str>,
        undated: bool,
        py: Python<'_>,
    ) -> PyResult<i32> {
        let date = comment_date(date, undated)?;
        let mut candidate = self.inner.clone_for_staging();
        let id = if let Ok(range) = range.cast::<PyRunRange>() {
            candidate.add_comment_with_date((*range.borrow()).into(), author, initials, text, date)
        } else if let Ok(range) = range.cast::<PyStoryRunRange>() {
            let range = range.borrow();
            let start = oxoffice_docx::StoryRunPosition {
                location: self.native_location(py, &range.start.item)?,
                run_index: range.start.run_index,
            };
            let end = oxoffice_docx::StoryRunPosition {
                location: self.native_location(py, &range.end.item)?,
                run_index: range.end.run_index,
            };
            candidate.add_story_comment_with_date(
                oxoffice_docx::StoryRunRange { start, end },
                author,
                initials,
                text,
                date,
            )
        } else {
            return Err(PyTypeError::new_err(
                "range must be a RunRange or StoryRunRange",
            ));
        }
        .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.add_comment",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(id)
    }

    /// Comment on the `occurrence`-th match of `anchor`, counted from zero,
    /// in the main story, or among the matches in table cells only with
    /// `tables_only`. Matching is case-sensitive, non-overlapping and within
    /// one paragraph. A match whose range would also show other text, such as
    /// a field result, raises.
    #[pyo3(signature = (anchor, *, author, text, occurrence = 0, tables_only = false, initials = None, date = None, undated = false))]
    #[allow(clippy::too_many_arguments)]
    fn add_comment_on_text(
        &mut self,
        anchor: &str,
        author: &str,
        text: &str,
        occurrence: usize,
        tables_only: bool,
        initials: Option<&str>,
        date: Option<&str>,
        undated: bool,
        py: Python<'_>,
    ) -> PyResult<i32> {
        let date = comment_date(date, undated)?;
        let mut candidate = self.inner.clone_for_staging();
        let id = candidate
            .add_comment_on_text(
                anchor,
                author,
                text,
                oxoffice_docx::TextCommentOptions {
                    occurrence,
                    tables_only,
                    initials,
                    date,
                },
            )
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.add_comment_on_text",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(id)
    }

    /// Move a root comment and its replies to a checked current story range
    /// atomically.
    fn move_comment(
        &mut self,
        id: i32,
        range: PyRef<'_, PyStoryRunRange>,
        py: Python<'_>,
    ) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        let start = oxoffice_docx::StoryRunPosition {
            location: self.native_location(py, &range.start.item)?,
            run_index: range.start.run_index,
        };
        let end = oxoffice_docx::StoryRunPosition {
            location: self.native_location(py, &range.end.item)?,
            run_index: range.end.run_index,
        };
        py.detach(|| candidate.move_comment(id, oxoffice_docx::StoryRunRange { start, end }))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.move_comment",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    /// Move a root comment and its replies onto the selected literal
    /// main-story occurrence.
    #[pyo3(signature = (id, anchor, *, occurrence = 0))]
    fn move_comment_to_text(
        &mut self,
        id: i32,
        anchor: &str,
        occurrence: usize,
        py: Python<'_>,
    ) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        py.detach(|| candidate.move_comment_to_text(id, anchor, occurrence))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.move_comment_to_text",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    #[pyo3(signature = (parent_id, *, author, text, date = None, initials = None, undated = false), text_signature = "($self, parent_id, *, author, text, date=None, initials=None, undated=False)")]
    #[allow(clippy::too_many_arguments)]
    fn reply_to(
        &mut self,
        parent_id: i32,
        author: &str,
        text: &str,
        date: Option<&str>,
        initials: Option<&str>,
        undated: bool,
        py: Python<'_>,
    ) -> PyResult<i32> {
        let date = comment_date(date, undated)?;
        let mut candidate = self.inner.clone_for_staging();
        let id = candidate
            .reply_to_with_initials(parent_id, author, initials, text, date)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.reply_to",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(id)
    }

    #[pyo3(signature = (id, *, resolved = true))]
    fn resolve_comment(&mut self, id: i32, resolved: bool, py: Python<'_>) -> PyResult<bool> {
        let mut candidate = self.inner.clone_for_staging();
        let updated = candidate
            .resolve_comment(id, resolved)
            .map_err(|error| docx_to_pyerr(py, error))?;
        if updated {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.resolve_comment",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(updated)
    }

    fn remove_comment(&mut self, id: i32, py: Python<'_>) -> PyResult<bool> {
        let mut candidate = self.inner.clone_for_staging();
        let removed = candidate
            .remove_comment(id)
            .map_err(|error| docx_to_pyerr(py, error))?;
        if removed {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.remove_comment",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(removed)
    }

    #[getter]
    fn bookmarks<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner
                .bookmarks()
                .into_iter()
                .map(|bookmark| PyBookmark {
                    id: bookmark.id(),
                    name: bookmark.name().map(str::to_owned),
                    range: bookmark.range().map(PyRunRange::from),
                    direct_range: bookmark.direct_range().map(PyRunRange::from),
                    text: bookmark.text().to_owned(),
                    issue: bookmark.issue().map(str::to_owned),
                }),
        )
    }

    fn add_bookmark(
        &mut self,
        py: Python<'_>,
        name: &str,
        range: PyRef<'_, PyRunRange>,
    ) -> PyResult<i32> {
        // Bookmark markers sit between runs, so no content moves and live
        // handles stay valid.
        self.inner
            .add_bookmark(name, (*range).into())
            .map_err(|error| docx_to_pyerr(py, error))
    }

    #[pyo3(signature = (*, fonts = None, font_dir = None, font_dirs = None, system_fonts = false))]
    fn layout<'py>(
        &self,
        py: Python<'py>,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
    ) -> PyResult<Bound<'py, PyTuple>> {
        let options = oxoffice_docx::RenderOptions::default().with_system_fonts(system_fonts);
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        let fragments = py
            .detach(|| {
                let layout = match font_refs.as_deref() {
                    None if system_fonts => self.inner.layout_with_options(options)?,
                    None => self.inner.layout_deterministic()?,
                    Some(fonts) if system_fonts => {
                        // Feature-off gate for this nonempty system request.
                        self.inner.layout_with_options(options)?;
                        std::sync::Arc::new(
                            self.inner
                                .layout_with_additional_fonts_and_options(fonts, options)?,
                        )
                    }
                    Some(fonts) => std::sync::Arc::new(
                        self.inner
                            .layout_with_fonts_and_bundled_fallback_and_options(fonts, options)?,
                    ),
                };
                let mut fragments = Vec::new();
                for body_index in 0..self.inner.content_count() {
                    let Some(body_fragments) = layout.body_layout_fragments(body_index) else {
                        continue;
                    };
                    fragments.extend(body_fragments.iter().map(|fragment| PyLayoutFragment {
                        body_index,
                        physical_page: fragment.physical_page,
                        displayed_page: fragment.displayed_page,
                        bounds: PyBoundingBox {
                            x: fragment.x,
                            y: fragment.y,
                            width: fragment.width,
                            height: fragment.height,
                        },
                    }));
                }
                Ok::<_, oxoffice_docx::Error>(fragments)
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        PyTuple::new(py, fragments)
    }

    #[pyo3(signature = (page_index, *, fonts = None, font_dir = None, font_dirs = None, system_fonts = false))]
    fn layout_page<'py>(
        &self,
        page_index: usize,
        py: Python<'py>,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
    ) -> PyResult<Option<PyLayoutPage>> {
        let options = oxoffice_docx::RenderOptions::default().with_system_fonts(system_fonts);
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        py.detach(|| {
            let layout = match font_refs.as_deref() {
                None if system_fonts => self.inner.layout_with_options(options)?,
                None => self.inner.layout_deterministic()?,
                Some(fonts) if system_fonts => {
                    // Feature-off gate for this nonempty system request.
                    self.inner.layout_with_options(options)?;
                    std::sync::Arc::new(
                        self.inner
                            .layout_with_additional_fonts_and_options(fonts, options)?,
                    )
                }
                Some(fonts) => std::sync::Arc::new(
                    self.inner
                        .layout_with_fonts_and_bundled_fallback_and_options(fonts, options)?,
                ),
            };
            Ok(layout
                .layout
                .pages
                .get(page_index)
                .map(|page| PyLayoutPage {
                    page_number: page.page_number,
                    displayed_page_number: page.displayed_page_number,
                    width: page.width,
                    height: page.height,
                }))
        })
        .map_err(|error| docx_to_pyerr(py, error))
    }

    fn rebuild_toc(&mut self, py: Python<'_>) -> PyResult<Py<PyTocRebuildReport>> {
        let mut candidate = self.inner.clone_for_staging();
        let (report, changed) = py
            .detach(|| {
                let before = candidate.to_bytes()?;
                let report = candidate.rebuild_toc()?;
                let changed = candidate.to_bytes()? != before;
                Ok::<_, oxoffice_docx::Error>((report, changed))
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        let result = Py::new(
            py,
            PyTocRebuildReport {
                entry_count: report.entry_count,
                bookmark_count: report.bookmark_count,
                diagnostics: report.diagnostics,
            },
        )?;
        if changed {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.rebuild_toc",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(result)
    }

    #[getter(revisions)]
    fn revision_snapshots<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        self.ensure_word_authority(py)?;
        let revisions = self
            .inner
            .story_revisions()
            .map_err(|error| docx_to_pyerr(py, error))?;
        PyTuple::new(
            py,
            revisions
                .iter()
                .map(|revision| {
                    Ok(PyRevision {
                        id: revision.id(),
                        author: revision.author().to_owned(),
                        timestamp: revision.timestamp().map(str::to_owned),
                        kind: revision_kind_name(revision.kind()).to_owned(),
                        story: Some(self.owned_story_snapshot(revision.story())?),
                        text: revision.text().to_owned(),
                    })
                })
                .collect::<PyResult<Vec<_>>>()?,
        )
    }

    fn accept_all(&mut self, py: Python<'_>) -> PyResult<usize> {
        self.counted_mutation(
            py,
            "Document.accept_all",
            oxoffice_docx::Document::accept_all,
        )
    }

    fn reject_all(&mut self, py: Python<'_>) -> PyResult<usize> {
        self.counted_mutation(
            py,
            "Document.reject_all",
            oxoffice_docx::Document::reject_all,
        )
    }

    fn accept_revisions_by_author(&mut self, py: Python<'_>, author: &str) -> PyResult<usize> {
        self.counted_mutation(py, "Document.accept_revisions_by_author", |document| {
            document.accept_revisions_by_author(author)
        })
    }

    fn reject_revisions_by_author(&mut self, py: Python<'_>, author: &str) -> PyResult<usize> {
        self.counted_mutation(py, "Document.reject_revisions_by_author", |document| {
            document.reject_revisions_by_author(author)
        })
    }

    #[pyo3(signature = (*, start, end))]
    fn accept_revisions_in_date_range(
        &mut self,
        py: Python<'_>,
        start: &str,
        end: &str,
    ) -> PyResult<usize> {
        self.counted_mutation(py, "Document.accept_revisions_in_date_range", |document| {
            document.accept_revisions_in_date_range(start, end)
        })
    }

    #[pyo3(signature = (*, start, end))]
    fn reject_revisions_in_date_range(
        &mut self,
        py: Python<'_>,
        start: &str,
        end: &str,
    ) -> PyResult<usize> {
        self.counted_mutation(py, "Document.reject_revisions_in_date_range", |document| {
            document.reject_revisions_in_date_range(start, end)
        })
    }

    fn accept_revision_id(&mut self, py: Python<'_>, id: i32) -> PyResult<usize> {
        self.counted_mutation(py, "Document.accept_revision_id", |document| {
            document.accept_revision_id(id)
        })
    }

    fn reject_revision_id(&mut self, py: Python<'_>, id: i32) -> PyResult<usize> {
        self.counted_mutation(py, "Document.reject_revision_id", |document| {
            document.reject_revision_id(id)
        })
    }

    /// Replace literal text only inside a checked paragraph, table or control item.
    #[pyo3(signature = (item, old, new, *, expect = None))]
    fn replace_text_at(
        &mut self,
        py: Python<'_>,
        item: &PyStoryItem,
        old: &str,
        new: &str,
        expect: Option<usize>,
    ) -> PyResult<usize> {
        // Constructed/numeric/unknown/nested-control selectors retain the
        // original resolver and its exact validation/error order.
        let story_kind = match item.story.kind.as_str() {
            "body" => Some(oxoffice_docx::StoryKind::Body),
            "table_cell" => Some(oxoffice_docx::StoryKind::TableCell),
            "header" => Some(oxoffice_docx::StoryKind::Header),
            "footer" => Some(oxoffice_docx::StoryKind::Footer),
            "footnote" => Some(oxoffice_docx::StoryKind::Footnote),
            "endnote" => Some(oxoffice_docx::StoryKind::Endnote),
            "comment" => Some(oxoffice_docx::StoryKind::Comment),
            "text_box" => Some(oxoffice_docx::StoryKind::TextBox),
            _ => None,
        };
        let item_kind = match item.kind.as_str() {
            "paragraph" => Some(oxoffice_docx::StoryItemKind::Paragraph),
            "table" => Some(oxoffice_docx::StoryItemKind::Table),
            "content_control" => Some(oxoffice_docx::StoryItemKind::ContentControl),
            _ => None,
        };
        if item.binding.is_some()
            && item.index_path.len() == 1
            && let (Some(story_kind), Some(item_kind)) = (story_kind, item_kind)
        {
            self.validate_owned_item(py, item)?;
            let part_name = item.story.part_name.clone();
            let owner_index = item.story.owner_index;
            let index_path = item.index_path.clone();
            return self.scoped_replacement(py, "Document.replace_text_at", |document| {
                document.try_replace_text_at_selector_owned(
                    story_kind,
                    &part_name,
                    owner_index,
                    item_kind,
                    &index_path,
                    old,
                    new,
                    expect,
                )
            });
        }
        let location = self.native_location(py, item)?;
        self.scoped_replacement(py, "Document.replace_text_at", |document| {
            document.try_replace_text_at_owned(&location, old, new, expect)
        })
    }

    #[pyo3(signature = (placeholder, replacement, *, expect = None))]
    fn try_replace_text<'py>(
        &mut self,
        py: Python<'py>,
        placeholder: &str,
        replacement: &str,
        expect: Option<usize>,
    ) -> PyResult<Bound<'py, PyInt>> {
        if self.snapshot_scope.is_none() {
            if expect.is_none() && replacement.chars().any(|character| {
                !matches!(character as u32, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
            }) {
                // The native singular API owns this exact upfront error and ordering.
                oxoffice_docx::Document::new().try_replace_text(placeholder, replacement)
                    .map_err(|error| docx_to_pyerr(py, error))?;
            }
            let Some((candidate, counts)) =
                self.dormant_replacements(py, &[(placeholder, replacement, expect)], false, None)?
            else {
                return Ok(PyInt::new(py, 0));
            };
            let result = PyInt::new(py, counts[0]);
            self.commit_dormant_replacement(candidate, "Document.try_replace_text")?;
            return Ok(result);
        }
        if expect.is_none() {
            return self
                .counted_mutation(py, "Document.try_replace_text", |document| {
                    document.try_replace_text(placeholder, replacement)
                })
                .map(|count| PyInt::new(py, count));
        }
        let (candidate, counts) =
            self.expected_replacements(py, &[(placeholder, replacement, expect)], false, None)?;
        if counts[0] > 0 {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.try_replace_text",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(PyInt::new(py, counts[0]))
    }

    #[pyo3(signature = (pairs, *, allow_unreached = false))]
    fn replace_all<'py>(
        &mut self,
        py: Python<'py>,
        pairs: &Bound<'py, PyAny>,
        allow_unreached: bool,
    ) -> PyResult<Bound<'py, PyTuple>> {
        let check = Some(oxoffice_docx::ReplaceOptions { allow_unreached });
        let mut owned = Vec::new();
        for (index, pair) in pairs.try_iter()?.enumerate() {
            let pair = pair?;
            let parsed = match pair.extract::<(String, String)>() {
                Ok((placeholder, replacement)) => (placeholder, replacement, None),
                Err(_) => pair
                    .extract::<(String, String, Option<usize>)>()
                    .map_err(|_| {
                        PyTypeError::new_err(format!(
                            "pair {index} must be an (old, new) or (old, new, expected) tuple, \
                         where expected is a nonnegative int or None"
                        ))
                    })?,
            };
            owned.push(parsed);
        }
        let pairs = owned
            .iter()
            .map(|(placeholder, replacement, expected)| {
                (placeholder.as_str(), replacement.as_str(), *expected)
            })
            .collect::<Vec<_>>();
        if self.snapshot_scope.is_none() {
            let changed = self.dormant_replacements(py, &pairs, true, check)?;
            let (candidate, counts) = match changed {
                Some((candidate, counts)) => (Some(candidate), counts),
                None => (None, vec![0; pairs.len()]),
            };
            let result = PyTuple::new(py, counts)?;
            if let Some(candidate) = candidate {
                self.commit_dormant_replacement(candidate, "Document.replace_all")?;
            }
            return Ok(result);
        }
        let (candidate, counts) = self.expected_replacements(py, &pairs, true, check)?;
        let changed = counts.iter().any(|count| *count > 0);
        let result = PyTuple::new(py, counts)?;
        if changed {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.replace_all",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(result)
    }

    fn replace_all_regex_expected<'py>(
        &mut self,
        py: Python<'py>,
        pairs: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyTuple>> {
        let mut owned = Vec::new();
        for (index, pair) in pairs.try_iter()?.enumerate() {
            let pair = pair?;
            let parsed = match pair.extract::<(String, String)>() {
                Ok((placeholder, replacement)) => (placeholder, replacement, None),
                Err(_) => pair
                    .extract::<(String, String, Option<usize>)>()
                    .map_err(|_| {
                        PyTypeError::new_err(format!(
                            "pair {index} must be an (old, new) or (old, new, expected) tuple, \
                         where expected is a nonnegative int or None"
                        ))
                    })?,
            };
            owned.push(parsed);
        }
        let pairs = owned
            .iter()
            .map(|(placeholder, replacement, expected)| {
                (placeholder.as_str(), replacement.as_str(), *expected)
            })
            .collect::<Vec<_>>();
        let (candidate, counts) = {
            let mut candidate = self.inner.clone_for_staging();
            let counts = py
                .detach(|| candidate.try_replace_all_regex_expected(&pairs))
                .map_err(|error| docx_to_pyerr(py, error))?
                .map_err(|mismatch| crate::replacement_count_to_pyerr(py, &mismatch, true))?;
            (candidate, counts)
        };

        let result = PyTuple::new(py, counts)?;
        self.publish_api_candidate(py, candidate, "Document.replace_all_regex_expected")?;

        Ok(result)
    }

    fn replace_all_regex(
        &mut self,
        py: Python<'_>,
        patterns: Vec<(String, String)>,
    ) -> PyResult<usize> {
        self.counted_mutation(py, "Document.replace_all_regex", |document| {
            document.replace_all_regex(&patterns)
        })
    }

    #[pyo3(signature = (
        *,
        now = None,
        file_name = None,
        file_path = None,
        merge_fields = None,
        included_text = None,
        merge_record_number = None,
        merge_sequence_number = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn update_fields(
        &mut self,
        py: Python<'_>,
        now: Option<&Bound<'_, PyAny>>,
        file_name: Option<String>,
        file_path: Option<String>,
        merge_fields: Option<BTreeMap<String, String>>,
        included_text: Option<BTreeMap<String, String>>,
        merge_record_number: Option<u32>,
        merge_sequence_number: Option<u32>,
    ) -> PyResult<usize> {
        // Read field by field: the abi3 build has no datetime accessors, and
        // the wall-clock values are used as given.
        let now = now
            .map(|now| {
                Ok::<_, PyErr>(oxoffice_docx::FieldDateTime {
                    year: now.getattr("year")?.extract()?,
                    month: now.getattr("month")?.extract()?,
                    day: now.getattr("day")?.extract()?,
                    hour: now.getattr("hour")?.extract()?,
                    minute: now.getattr("minute")?.extract()?,
                    second: now.getattr("second")?.extract()?,
                })
            })
            .transpose()?;
        let context = oxoffice_docx::FieldEvaluationContext {
            now,
            file_name,
            file_path,
            merge_fields: merge_fields.unwrap_or_default(),
            included_text: included_text.unwrap_or_default(),
            merge_record_number,
            merge_sequence_number,
        };
        self.counted_mutation(py, "Document.update_fields", |document| {
            document.update_fields(&context)
        })
    }

    #[pyo3(signature = (
        *,
        now = None,
        file_name = None,
        file_path = None,
        merge_fields = None,
        included_text = None,
        merge_record_number = None,
        merge_sequence_number = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn update_all_fields(
        &mut self,
        py: Python<'_>,
        now: Option<&Bound<'_, PyAny>>,
        file_name: Option<String>,
        file_path: Option<String>,
        merge_fields: Option<BTreeMap<String, String>>,
        included_text: Option<BTreeMap<String, String>>,
        merge_record_number: Option<u32>,
        merge_sequence_number: Option<u32>,
    ) -> PyResult<Py<PyFieldsUpdateReport>> {
        // Read field by field: the abi3 build has no datetime accessors, and
        // the wall-clock values are used as given.
        let now = now
            .map(|now| {
                Ok::<_, PyErr>(oxoffice_docx::FieldDateTime {
                    year: now.getattr("year")?.extract()?,
                    month: now.getattr("month")?.extract()?,
                    day: now.getattr("day")?.extract()?,
                    hour: now.getattr("hour")?.extract()?,
                    minute: now.getattr("minute")?.extract()?,
                    second: now.getattr("second")?.extract()?,
                })
            })
            .transpose()?;
        let context = oxoffice_docx::FieldEvaluationContext {
            now,
            file_name,
            file_path,
            merge_fields: merge_fields.unwrap_or_default(),
            included_text: included_text.unwrap_or_default(),
            merge_record_number,
            merge_sequence_number,
        };
        let mut candidate = self.inner.clone_for_staging();
        let report = py
            .detach(|| candidate.update_all_fields(&context))
            .map_err(|error| docx_to_pyerr(py, error))?;

        // Every returned object, including nested outcomes and tuples, exists
        // before publishing. Returning this owned Py object needs no allocation.
        let json_loads = py.import("json")?.getattr("loads")?;
        let evaluations = report
            .evaluations
            .iter()
            .map(|evaluation| {
                let outcome = json_loads
                    .call1((evaluation.to_json_value()["outcome"].to_string(),))?
                    .cast_into::<PyDict>()?
                    .unbind();
                Py::new(
                    py,
                    PyFieldEvaluation {
                        field_index: evaluation.field_index,
                        instruction: evaluation.instruction.clone(),
                        cached_result: evaluation.cached_result.clone(),
                        outcome,
                    },
                )
            })
            .collect::<PyResult<Vec<_>>>()?;
        let evaluations = PyTuple::new(py, evaluations)?.unbind();
        let diagnostics = PyTuple::new(py, &report.diagnostics)?.unbind();
        let layout = Py::new(
            py,
            PyLayoutBackedFieldUpdateReport {
                page_fields: report.layout.page_fields,
                num_pages_fields: report.layout.num_pages_fields,
                page_reference_fields: report.layout.page_reference_fields,
                section_fields: report.layout.section_fields,
                section_pages_fields: report.layout.section_pages_fields,
                diagnostics: report.layout.diagnostics,
            },
        )?;
        let result = Py::new(
            py,
            PyFieldsUpdateReport {
                ordinary_updated: report.ordinary_updated,
                layout,
                evaluations,
                diagnostics,
                total_updated: report.total_updated,
                saved_cache_scope: report.saved_cache_scope.to_owned(),
            },
        )?;
        self.publish_api_candidate(py, candidate, "Document.update_all_fields")?;

        Ok(result)
    }

    fn update_page_fields(&mut self, py: Python<'_>) -> PyResult<usize> {
        let mut candidate = self.inner.clone_for_staging();
        let updated = py
            .detach(|| candidate.update_page_fields())
            .map_err(|error| docx_to_pyerr(py, error))?;
        if updated != 0 {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.update_page_fields",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(updated)
    }

    fn update_layout_backed_fields(
        &mut self,
        py: Python<'_>,
    ) -> PyResult<Py<PyLayoutBackedFieldUpdateReport>> {
        let mut candidate = self.inner.clone_for_staging();
        let report = py
            .detach(|| candidate.update_layout_backed_fields())
            .map_err(|error| docx_to_pyerr(py, error))?;
        let changed = report.updated_count() != 0;
        let result = Py::new(
            py,
            PyLayoutBackedFieldUpdateReport {
                page_fields: report.page_fields,
                num_pages_fields: report.num_pages_fields,
                page_reference_fields: report.page_reference_fields,
                section_fields: report.section_fields,
                section_pages_fields: report.section_pages_fields,
                diagnostics: report.diagnostics,
            },
        )?;
        if changed {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.update_layout_backed_fields",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(result)
    }

    #[pyo3(signature = (index, max_level = 3))]
    fn insert_toc(&mut self, py: Python<'_>, index: usize, max_level: u32) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        if index > candidate.content_count() {
            return Err(PyIndexError::new_err("content index out of range"));
        }
        if !(1..=9).contains(&max_level) {
            return Err(PyValueError::new_err("max_level must be between 1 and 9"));
        }
        candidate
            .insert_toc(index, max_level)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.insert_toc",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    #[getter]
    fn paragraphs(slf: Py<Self>, py: Python<'_>) -> PyResult<Py<PyParagraphCollection>> {
        slf.borrow_mut(py).ensure_word_authority(py)?;
        Py::new(py, PyParagraphCollection::new(slf))
    }

    #[getter]
    fn tables(slf: Py<Self>, py: Python<'_>) -> PyResult<Py<PyTableCollection>> {
        slf.borrow_mut(py).ensure_word_authority(py)?;
        Py::new(py, PyTableCollection::new(slf))
    }

    #[pyo3(signature = (text = "", style = None))]
    fn add_paragraph(
        slf: Py<Self>,
        py: Python<'_>,
        text: &str,
        style: Option<&str>,
    ) -> PyResult<Py<PyParagraph>> {
        Self::append_paragraph_candidate(slf, py, text, style, false)
    }

    #[pyo3(signature = (text = "", level = 1))]
    fn add_heading(
        slf: Py<Self>,
        py: Python<'_>,
        text: &str,
        #[pyo3(from_py_with = checked_heading_level)] level: i64,
    ) -> PyResult<Py<PyParagraph>> {
        if !(0..=9).contains(&level) {
            return Err(PyValueError::new_err(
                "heading level must be from 0 through 9",
            ));
        }
        let style = if level == 0 {
            "Title".to_owned()
        } else {
            format!("Heading{level}")
        };
        Self::append_paragraph_candidate(slf, py, text, Some(&style), false)
    }

    fn add_page_break(slf: Py<Self>, py: Python<'_>) -> PyResult<Py<PyParagraph>> {
        Self::append_paragraph_candidate(slf, py, "", None, true)
    }

    #[pyo3(signature = (start_type = None))]
    fn add_section(&self, start_type: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        let _ = start_type;
        Err(pyo3::exceptions::PyNotImplementedError::new_err(
            "use document.insert_section(len(document.sections)) and document.update_section(index, break_type=...); existing native insertion copies geometry only",
        ))
    }

    #[pyo3(signature = (rows, cols, style = None))]
    fn add_table(
        slf: Py<Self>,
        py: Python<'_>,
        rows: usize,
        cols: usize,
        style: Option<&str>,
    ) -> PyResult<Py<PyTable>> {
        let mut document = slf.borrow_mut(py);
        let index = document.inner.table_count();
        document.refuse_unsavable_body_edit(py)?;
        // A style may add its definition, so only a plain table splices.
        if style.is_none() {
            let appended = oxoffice_docx::Document::detached_table(rows, cols);
            let slot = document.inner.content_count();
            if let Ok((xml, snapshots)) = document.inner.detached_table_owner_snapshots(&appended)
                && let Some(plan) = document.plan_body_splice(
                    py,
                    BodySplice {
                        slot,
                        removed: 0,
                        inserted: Some(SplicedBodyItem::Table(xml, snapshots)),
                    },
                )?
            {
                // Every fallible step precedes the in-place native append.
                let publication = crate::document::WordPublication::AppendBody;
                let prepared =
                    document.prepare_body_splice(plan, "Document.add_table", &publication)?;
                let path = document
                    .revisions
                    .capture_prepared(&prepared.revision, smallvec![PathSeg::Body(index)])
                    .map_err(revision_to_pyerr)?;
                let result = Py::new(
                    py,
                    PyTable::new(slf.clone_ref(py), path)
                        .bind_prepared_splice(&document, &prepared)?,
                )?;
                let savable = document.inner.unsavable_body_edit().is_none();
                document.inner.add_table(rows, cols);
                debug_assert_eq!(prepared.plan.table, index);
                document.commit_body_splice(prepared, &publication)?;
                document
                    .inner
                    .keep_body_savable_after_typed_insert(slot, savable);
                return Ok(result);
            }
        }
        let mut candidate = document.inner.clone_for_staging();
        let style = style
            .map(|value| crate::table::checked_table_style(py, &mut candidate, value))
            .transpose()?;
        let mut table = candidate.add_table(rows, cols);
        if let Some(style) = style {
            table.set_style(&style);
        }
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.add_table",
            crate::document::WordPublication::AppendBody,
        )?;
        let path = document
            .revisions
            .capture_prepared(&prepared.revision, smallvec![PathSeg::Body(index)])
            .map_err(revision_to_pyerr)?;
        let result = Py::new(
            py,
            PyTable::new(slf.clone_ref(py), path)
                .bind_prepared(py, &document, &prepared, &candidate, None)?,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(result)
    }

    fn remove_content(&mut self, py: Python<'_>, index: usize) -> PyResult<bool> {
        if self.inner.plain_direct_paragraph(index)
            && let Some((range, revision)) = self.prepare_tail_body_removal(index)?
        {
            let removed = py
                .detach(|| self.inner.try_remove_content(index))
                .map_err(|error| docx_to_pyerr(py, error))?;
            if removed {
                self.commit_tail_body_removal(index, range, revision)?;
            }
            return Ok(removed);
        }
        if let Some(plan) = self.plan_body_splice(
            py,
            BodySplice {
                slot: index,
                removed: 1,
                inserted: None,
            },
        )? {
            let publication = crate::document::WordPublication::Conservative;
            let prepared =
                self.prepare_body_splice(plan, "Document.remove_content", &publication)?;
            let removed = py
                .detach(|| self.inner.try_remove_content(index))
                .map_err(|error| docx_to_pyerr(py, error))?;
            if removed {
                self.commit_body_splice(prepared, &publication)?;
            }
            return Ok(removed);
        }
        let mut candidate = self.inner.clone_for_staging();
        let removed = py
            .detach(|| candidate.try_remove_content(index))
            .map_err(|error| docx_to_pyerr(py, error))?;
        if removed {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.remove_content",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(removed)
    }

    fn remove_content_range(
        &mut self,
        py: Python<'_>,
        start: usize,
        end: usize,
    ) -> PyResult<usize> {
        let len = self.inner.content_count();
        if start > end || end > len {
            return Err(docx_to_pyerr(
                py,
                oxoffice_docx::Error::Other("content range is reversed or out of range".to_owned()),
            ));
        }
        if start == end {
            return Ok(0);
        }
        if let Some(plan) = self.plan_body_splice(
            py,
            BodySplice {
                slot: start,
                removed: end - start,
                inserted: None,
            },
        )? {
            let publication = crate::document::WordPublication::Conservative;
            let prepared =
                self.prepare_body_splice(plan, "Document.remove_content_range", &publication)?;
            let removed = py
                .detach(|| self.inner.remove_content_range(start, end))
                .map_err(|error| docx_to_pyerr(py, error))?;
            self.commit_body_splice(prepared, &publication)?;
            return Ok(removed);
        }
        let mut candidate = self.inner.clone_for_staging();
        let removed = py
            .detach(|| candidate.remove_content_range(start, end))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.remove_content_range",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(removed)
    }

    fn find_content_index(
        slf: Py<Self>,
        py: Python<'_>,
        content: &Bound<'_, PyAny>,
    ) -> PyResult<usize> {
        if let Ok(text) = content.extract::<String>() {
            return slf
                .borrow(py)
                .inner
                .find_content_index(&text)
                .ok_or_else(|| PyValueError::new_err("text was not found in body content"));
        }
        Self::direct_content_index(&slf, py, content, "content")
    }

    fn find_content_indices<'py>(
        &self,
        py: Python<'py>,
        text: &str,
    ) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, self.inner.find_content_indices(text))
    }

    #[pyo3(signature = (index, text, style = None))]
    pub(crate) fn insert_paragraph(
        slf: Py<Self>,
        py: Python<'_>,
        index: usize,
        text: &str,
        style: Option<&str>,
    ) -> PyResult<Py<PyParagraph>> {
        let result = {
            let mut document = slf.borrow_mut(py);
            if index > document.inner.content_count() {
                return Err(PyIndexError::new_err("content index out of range"));
            }
            let style = style
                .map(|value| {
                    style_id_of_type(&document.inner, value, oxoffice_docx::StyleType::Paragraph)
                })
                .transpose()?;
            document.refuse_non_xml_paragraph_text(py, text)?;
            document.refuse_unsavable_body_edit(py)?;
            // The same edit builds the paragraph read for the inventory and
            // the paragraph placed.
            let edit = |paragraph: &mut oxoffice_docx::Paragraph<'_>| {
                if let Some(style) = &style {
                    paragraph.set_style(style);
                }
            };
            let mut inserted = oxoffice_docx::Document::detached_text_paragraph(text);
            edit(&mut oxoffice_docx::Paragraph::detached(&mut inserted));
            let splice = document.inner.direct_body_paragraph_facts(&inserted);
            if let Ok(Some(facts)) = splice
                && let Some(plan) = document.plan_body_splice(
                    py,
                    BodySplice {
                        slot: index,
                        removed: 0,
                        inserted: Some(SplicedBodyItem::Paragraph(facts.0, facts.1)),
                    },
                )?
            {
                // Every fallible step precedes the in-place native insertion.
                let publication = crate::document::WordPublication::Conservative;
                let prepared = document.prepare_body_splice(
                    plan,
                    "Document.insert_paragraph",
                    &publication,
                )?;
                let path = document
                    .revisions
                    .capture_prepared(
                        &prepared.revision,
                        smallvec![PathSeg::Body(0), PathSeg::Para(prepared.plan.paragraph)],
                    )
                    .map_err(revision_to_pyerr)?;
                let result = Py::new(
                    py,
                    PyParagraph::new(slf.clone_ref(py), path)
                        .bind_prepared_splice(&document, &prepared)?,
                )?;
                let savable = document.inner.unsavable_body_edit().is_none();
                edit(&mut document.inner.insert_paragraph(index, text));
                debug_assert_eq!(
                    document.inner.paragraph_index_of_content(index),
                    Some(prepared.plan.paragraph)
                );
                document.commit_body_splice(prepared, &publication)?;
                document
                    .inner
                    .keep_body_savable_after_typed_insert(index, savable);
                return Ok(result);
            }
            let mut candidate = document.inner.clone_for_staging();
            edit(&mut candidate.insert_paragraph(index, text));
            let paragraph = candidate
                .paragraph_index_of_content(index)
                .expect("an inserted body paragraph has a paragraph index");
            let prepared = document.prepare_word_revision(
                py,
                &candidate,
                "Document.insert_paragraph",
                crate::document::WordPublication::Conservative,
            )?;
            let path = document
                .revisions
                .capture_prepared(
                    &prepared.revision,
                    smallvec![PathSeg::Body(0), PathSeg::Para(paragraph)],
                )
                .map_err(revision_to_pyerr)?;
            let result = Py::new(
                py,
                PyParagraph::new(slf.clone_ref(py), path)
                    .bind_prepared(py, &document, &prepared, &candidate, None)?,
            )?;
            document.commit_word_revision(prepared)?;
            document.install_inner(candidate);
            result
        };
        Ok(result)
    }

    #[pyo3(signature = (index, rows, cols))]
    fn insert_table(
        slf: Py<Self>,
        py: Python<'_>,
        index: usize,
        rows: usize,
        cols: usize,
    ) -> PyResult<Py<PyTable>> {
        let result = {
            let mut document = slf.borrow_mut(py);
            if index > document.inner.content_count() {
                return Err(PyIndexError::new_err("content index out of range"));
            }
            document.refuse_unsavable_body_edit(py)?;
            let inserted = oxoffice_docx::Document::detached_table(rows, cols);
            if let Ok((xml, snapshots)) = document.inner.detached_table_owner_snapshots(&inserted)
                && let Some(plan) = document.plan_body_splice(
                    py,
                    BodySplice {
                        slot: index,
                        removed: 0,
                        inserted: Some(SplicedBodyItem::Table(xml, snapshots)),
                    },
                )?
            {
                // Every fallible step precedes the in-place native insertion.
                let publication = crate::document::WordPublication::Conservative;
                let prepared =
                    document.prepare_body_splice(plan, "Document.insert_table", &publication)?;
                let path = document
                    .revisions
                    .capture_prepared(
                        &prepared.revision,
                        smallvec![PathSeg::Body(prepared.plan.table)],
                    )
                    .map_err(revision_to_pyerr)?;
                let result = Py::new(
                    py,
                    PyTable::new(slf.clone_ref(py), path)
                        .bind_prepared_splice(&document, &prepared)?,
                )?;
                let savable = document.inner.unsavable_body_edit().is_none();
                document.inner.insert_table(index, rows, cols);
                debug_assert_eq!(
                    document.inner.content_index_of_table(prepared.plan.table),
                    Some(index)
                );
                document.commit_body_splice(prepared, &publication)?;
                document
                    .inner
                    .keep_body_savable_after_typed_insert(index, savable);
                return Ok(result);
            }
            let mut candidate = document.inner.clone_for_staging();
            candidate.insert_table(index, rows, cols);
            // Preserve the direct body table ordinal while capturing its physical owner.
            let table = (0..candidate.table_count())
                .find(|table| candidate.content_index_of_table(*table) == Some(index))
                .expect("an inserted body table has a table index");
            let prepared = document.prepare_word_revision(
                py,
                &candidate,
                "Document.insert_table",
                crate::document::WordPublication::Conservative,
            )?;
            let path = document
                .revisions
                .capture_prepared(&prepared.revision, smallvec![PathSeg::Body(table)])
                .map_err(revision_to_pyerr)?;
            let result = Py::new(
                py,
                PyTable::new(slf.clone_ref(py), path)
                    .bind_prepared(py, &document, &prepared, &candidate, None)?,
            )?;
            document.commit_word_revision(prepared)?;
            document.install_inner(candidate);
            result
        };
        Ok(result)
    }

    fn pop_content(
        slf: Py<Self>,
        py: Python<'_>,
        index: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyContentFragment>> {
        let location = if let Ok(item) = index.cast::<PyStoryItem>() {
            slf.borrow(py).native_location(py, &item.borrow())?
        } else {
            let index = body_index_argument(index, "index must be an int or a StoryItem")?;
            let location = slf.borrow(py).body_location(py, index)?;
            if index == slf.borrow(py).inner.content_count() {
                return Err(PyIndexError::new_err("content index out of range"));
            }
            location
        };
        let mut document = slf.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        let fragment = candidate
            .remove_content_at(&location)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.pop_content",
            crate::document::WordPublication::Conservative,
        )?;
        let result = Py::new(py, PyContentFragment { inner: fragment })?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(result)
    }

    fn insert_content(
        slf: Py<Self>,
        py: Python<'_>,
        destination: &Bound<'_, PyAny>,
        fragment: PyRef<'_, PyContentFragment>,
    ) -> PyResult<()> {
        // A story item names the boundary before it, and a story its end.
        let location = if let Ok(item) = destination.cast::<PyStoryItem>() {
            slf.borrow(py).native_location(py, &item.borrow())?
        } else if let Ok(story) = destination.cast::<PyStory>() {
            oxoffice_docx::ContentLocation::end(slf.borrow(py).native_story(py, &story.borrow())?)
        } else {
            let destination = body_index_argument(
                destination,
                "destination must be an int, a StoryItem or a Story",
            )?;
            slf.borrow(py).body_location(py, destination)?
        };
        let fragment = fragment.inner.clone();
        let mut document = slf.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        candidate
            .insert_content(&location, fragment)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.insert_content",
            crate::document::WordPublication::Conservative,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(())
    }

    fn clone_content(
        slf: Py<Self>,
        py: Python<'_>,
        source: &Bound<'_, PyAny>,
        destination: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let source_index = Self::direct_content_index(&slf, py, source, "source")?;
        let destination = destination
            .extract::<usize>()
            .map_err(|_| PyTypeError::new_err("destination must be a direct body index integer"))?;
        let (source, destination) = {
            let document = slf.borrow(py);
            let mut locations = document.body_locations(py, &[source_index, destination])?;
            let destination = locations.pop().expect("two requested body locations");
            let source = locations.pop().expect("two requested body locations");
            (source, destination)
        };
        let mut document = slf.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        candidate
            .clone_content(&source, &destination)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.clone_content",
            crate::document::WordPublication::Conservative,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(())
    }

    fn move_content(
        slf: Py<Self>,
        py: Python<'_>,
        source: &Bound<'_, PyAny>,
        destination: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let source_index = Self::direct_content_index(&slf, py, source, "source")?;
        let destination = destination
            .extract::<usize>()
            .map_err(|_| PyTypeError::new_err("destination must be a direct body index integer"))?;
        let (source, destination) = {
            let document = slf.borrow(py);
            let mut locations = document.body_locations(py, &[source_index, destination])?;
            let destination = locations.pop().expect("two requested body locations");
            let source = locations.pop().expect("two requested body locations");
            (source, destination)
        };
        let mut document = slf.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        candidate
            .move_content(&source, &destination)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.move_content",
            crate::document::WordPublication::Conservative,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(())
    }

    #[getter]
    fn fonts(&self, py: Python<'_>) -> PyResult<Py<PyTuple>> {
        let fonts = self.inner.try_fonts().map_err(|e| docx_to_pyerr(py, e))?;
        let mut snapshots = Vec::new();
        snapshots.try_reserve_exact(fonts.len()).map_err(|_| {
            pyo3::exceptions::PyMemoryError::new_err("font listing allocation failed")
        })?;
        for font in fonts {
            snapshots.push(PyFontDefinition::snapshot(py, font)?);
        }
        Ok(PyTuple::new(py, snapshots)?.unbind())
    }
    fn set_font(&mut self, py: Python<'_>, font: PyRef<'_, PyFontDefinition>) -> PyResult<()> {
        if !font.embedded_fonts.bind(py).is_empty() {
            return Err(PyValueError::new_err(
                "set_font accepts descriptive records only, use embed_font",
            ));
        }
        let native = oxoffice_docx::FontDefinition {
            name: font.name.clone(),
            alternate_name: font.alternate_name.clone(),
            family: font.family.clone(),
            pitch: font.pitch.clone(),
            embedded_fonts: Vec::new(),
        };
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_font(native)
            .map_err(|e| docx_to_pyerr(py, e))?;
        self.publish_api_candidate(py, candidate, "Document.set_font")
    }
    fn remove_font(
        &mut self,
        py: Python<'_>,
        name: &str,
    ) -> PyResult<Option<Py<PyFontDefinition>>> {
        let mut candidate = self.inner.clone_for_staging();
        let removed = candidate
            .remove_font(name)
            .map_err(|e| docx_to_pyerr(py, e))?
            .map(|font| PyFontDefinition::snapshot(py, font))
            .transpose()?;
        self.publish_api_candidate(py, candidate, "Document.remove_font")?;
        Ok(removed)
    }
    #[pyo3(signature = (name, kind="regular"))]
    fn remove_embedded_font(
        &mut self,
        py: Python<'_>,
        name: &str,
        kind: &str,
    ) -> PyResult<Option<Py<PyEmbeddedFont>>> {
        let kind = checked_font_kind(kind)?;
        let mut candidate = self.inner.clone_for_staging();
        let removed = candidate
            .remove_embedded_font(name, kind)
            .map_err(|e| docx_to_pyerr(py, e))?
            .map(|font| PyEmbeddedFont::snapshot(py, font))
            .transpose()?;
        self.publish_api_candidate(py, candidate, "Document.remove_embedded_font")?;
        Ok(removed)
    }
    #[pyo3(signature = (name, font, kind="regular", *, license, font_key=None, subsetted=false))]
    #[allow(clippy::too_many_arguments)] // Explicit operands match the native font authoring contract.
    fn embed_font(
        &mut self,
        py: Python<'_>,
        name: &str,
        font: &Bound<'_, PyAny>,
        kind: &str,
        license: PyRef<'_, PyFontEmbeddingLicense>,
        font_key: Option<String>,
        subsetted: bool,
    ) -> PyResult<()> {
        let kind = checked_font_kind(kind)?;
        let license = checked_font_license(&license)?;
        let data = checked_font_input(font)?;
        let key = match font_key {
            Some(key) => key,
            None => oxoffice_docx::derive_font_key(&data).map_err(|e| docx_to_pyerr(py, e))?,
        };
        let font =
            oxoffice_docx::EmbeddedFont::new(kind, data, key, license).with_subsetted(subsetted);
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .embed_font(name, font)
            .map_err(|e| docx_to_pyerr(py, e))?;
        self.publish_api_candidate(py, candidate, "Document.embed_font")
    }

    #[getter]
    fn embed_true_type_fonts(&self, py: Python<'_>) -> PyResult<Option<bool>> {
        self.inner
            .embed_true_type_fonts()
            .map_err(|e| docx_to_pyerr(py, e))
    }
    #[setter]
    fn set_embed_true_type_fonts(&mut self, py: Python<'_>, value: Option<bool>) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_embed_true_type_fonts(value)
            .map_err(|e| docx_to_pyerr(py, e))?;
        self.publish_api_candidate(py, candidate, "Document.set_embed_true_type_fonts")
    }
    #[getter]
    fn embed_system_fonts(&self, py: Python<'_>) -> PyResult<Option<bool>> {
        self.inner
            .embed_system_fonts()
            .map_err(|e| docx_to_pyerr(py, e))
    }
    #[setter]
    fn set_embed_system_fonts(&mut self, py: Python<'_>, value: Option<bool>) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_embed_system_fonts(value)
            .map_err(|e| docx_to_pyerr(py, e))?;
        self.publish_api_candidate(py, candidate, "Document.set_embed_system_fonts")
    }
    #[getter]
    fn save_subset_fonts(&self, py: Python<'_>) -> PyResult<Option<bool>> {
        self.inner
            .save_subset_fonts()
            .map_err(|e| docx_to_pyerr(py, e))
    }
    #[setter]
    fn set_save_subset_fonts(&mut self, py: Python<'_>, value: Option<bool>) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_save_subset_fonts(value)
            .map_err(|e| docx_to_pyerr(py, e))?;
        self.publish_api_candidate(py, candidate, "Document.set_save_subset_fonts")
    }

    fn set_mail_merge_package(
        &mut self,
        py: Python<'_>,
        options: &Bound<'_, PyMailMergePackageOptions>,
    ) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_mail_merge_package(options.borrow().inner.clone())
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.set_mail_merge_package")
    }

    fn mail_merge_package(&self, py: Python<'_>) -> PyResult<Option<PyMailMergePackageOptions>> {
        Ok(self
            .inner
            .mail_merge_package()
            .map_err(|error| docx_to_pyerr(py, error))?
            .map(|inner| PyMailMergePackageOptions { inner }))
    }

    fn remove_mail_merge_package(&mut self, py: Python<'_>) -> PyResult<bool> {
        let mut candidate = self.inner.clone_for_staging();
        let removed = candidate
            .remove_mail_merge_package()
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.remove_mail_merge_package")?;
        Ok(removed)
    }

    #[pyo3(
        signature = (*, policy = "offline", external_records = None),
        text_signature = "($self, *, policy='offline', external_records=None)"
    )]
    fn prepare_mail_merge_data(
        &self,
        py: Python<'_>,
        policy: &str,
        external_records: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyMailMergePreparedData> {
        let native_policy = match policy {
            "offline" => oxoffice_docx::MailMergeSourcePolicy::Offline,
            "caller_supplied" => oxoffice_docx::MailMergeSourcePolicy::CallerSupplied,
            _ => {
                return Err(PyValueError::new_err(
                    "policy must be 'offline' or 'caller_supplied'",
                ));
            }
        };
        let prepared = if let Some(external_records) = external_records {
            require_carrier_sequence(external_records, "external_records")?;
            let probe = self
                .inner
                .prepare_mail_merge_data(native_policy, Some(&[]))
                .map_err(|error| docx_to_pyerr(py, error))?;
            if !probe.diagnostics.is_empty() {
                probe
            } else {
                let rows = copied_external_rows(py, external_records)?;
                self.inner
                    .prepare_mail_merge_data(native_policy, Some(&rows))
                    .map_err(|error| docx_to_pyerr(py, error))?
            }
        } else {
            self.inner
                .prepare_mail_merge_data(native_policy, None)
                .map_err(|error| docx_to_pyerr(py, error))?
        };
        Ok(PyMailMergePreparedData { inner: prepared })
    }
}

fn parse_render_options(
    revision_view: &str,
    revision_style: &str,
) -> PyResult<oxoffice_docx::RenderOptions> {
    let revision_view = match revision_view {
        "accepted" => oxoffice_docx::RevisionView::Accepted,
        "tracked" => oxoffice_docx::RevisionView::Tracked,
        other => {
            return Err(PyValueError::new_err(format!(
                "unknown revision view {other:?}, expected accepted or tracked"
            )));
        }
    };
    let revision_style = match revision_style {
        "plain" => oxoffice_docx::RevisionStyle::Plain,
        "legal" => oxoffice_docx::RevisionStyle::Legal,
        other => {
            return Err(PyValueError::new_err(format!(
                "unknown revision style {other:?}, expected plain or legal"
            )));
        }
    };
    if revision_style != oxoffice_docx::RevisionStyle::Plain
        && revision_view != oxoffice_docx::RevisionView::Tracked
    {
        return Err(PyValueError::new_err(
            "revision_style applies only with revision_view=\"tracked\"",
        ));
    }
    Ok(oxoffice_docx::RenderOptions::default()
        .with_revision_view(revision_view)
        .with_revision_style(revision_style))
}

fn parse_raster_format(
    format: &str,
    quality: u8,
    transparent: bool,
) -> oxoffice_docx::Result<oxoffice_docx::RasterFormat> {
    match format {
        "png" => Ok(oxoffice_docx::RasterFormat::Png {
            transparent_background: transparent,
        }),
        "jpg" | "jpeg" => Ok(oxoffice_docx::RasterFormat::Jpeg { quality }),
        "tif" | "tiff" => Ok(oxoffice_docx::RasterFormat::Tiff),
        other => Err(oxoffice_docx::Error::Other(format!(
            "unknown raster format {other:?}, expected png, jpeg, or tiff"
        ))),
    }
}

fn parse_comparison_granularity(
    name: &str,
) -> oxoffice_docx::Result<oxoffice_docx::ComparisonGranularity> {
    match name {
        "run" => Ok(oxoffice_docx::ComparisonGranularity::Run),
        "word" => Ok(oxoffice_docx::ComparisonGranularity::Word),
        "character" => Ok(oxoffice_docx::ComparisonGranularity::Character),
        other => Err(oxoffice_docx::Error::Other(format!(
            "unknown comparison granularity {other:?}, expected run, word, or character"
        ))),
    }
}

/// Story names follow `Story.kind`, so `body` selects the main story.
fn parse_comparison_story(name: &str) -> oxoffice_docx::Result<oxoffice_docx::ComparisonStoryKind> {
    match name {
        "body" => Ok(oxoffice_docx::ComparisonStoryKind::Main),
        "header" => Ok(oxoffice_docx::ComparisonStoryKind::Header),
        "footer" => Ok(oxoffice_docx::ComparisonStoryKind::Footer),
        "comment" => Ok(oxoffice_docx::ComparisonStoryKind::Comment),
        "text_box" => Ok(oxoffice_docx::ComparisonStoryKind::TextBox),
        "footnote" => Ok(oxoffice_docx::ComparisonStoryKind::Footnote),
        "endnote" => Ok(oxoffice_docx::ComparisonStoryKind::Endnote),
        other => Err(oxoffice_docx::Error::Other(format!(
            "unknown comparison story {other:?}, expected body, header, footer, comment, \
             text_box, footnote, or endnote"
        ))),
    }
}

// Native snapshots bind causal state without changing their visible value equality.

impl PyDocument {
    fn owned_story_snapshot(&self, story: &oxoffice_docx::StoryId) -> PyResult<PyStory> {
        let mut snapshot = story_snapshot(story);
        snapshot.binding = Some(self.capture_snapshot(smallvec![])?);
        Ok(snapshot)
    }

    fn validate_owned_story(&self, py: Python<'_>, story: &PyStory) -> PyResult<()> {
        let Some(path) = &story.binding else {
            return Ok(());
        };
        if !path.belongs_to(&self.revisions) {
            return Err(PyValueError::new_err(
                "story belongs to a different document",
            ));
        }
        path.validate_scopes(
            &self.revisions,
            "story",
            "Re-fetch it with document.stories.",
        )
        .map_err(|error| crate::stale_to_pyerr(py, error))
    }

    fn validate_owned_item(&self, py: Python<'_>, item: &PyStoryItem) -> PyResult<()> {
        if let Some(path) = &item.binding {
            if !path.belongs_to(&self.revisions) {
                return Err(PyValueError::new_err(
                    "story item belongs to a different document",
                ));
            }
            path.validate_scopes(
                &self.revisions,
                "story item",
                "Re-fetch it with document.story_items.",
            )
            .map_err(|error| crate::stale_to_pyerr(py, error))?;
        }
        self.validate_owned_story(py, &item.story)
    }
}

impl PartialEq for PyStory {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.part_name == other.part_name
            && self.owner_index == other.owner_index
    }
}
impl Eq for PyStory {}
impl PartialEq for PyStoryItem {
    fn eq(&self, other: &Self) -> bool {
        self.story == other.story
            && self.kind == other.kind
            && self.index_path == other.index_path
            && self.direct_body_index == other.direct_body_index
            && self.text == other.text
            && self.xml == other.xml
            && self.revision == other.revision
    }
}
impl Eq for PyStoryItem {}

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

impl PyDocument {
    fn refuse_unsavable_body_edit(&self, py: Python<'_>) -> PyResult<()> {
        match self.inner.unsavable_body_edit() {
            Some(error) => Err(docx_to_pyerr(
                py,
                oxoffice_docx::Error::Other(error.to_owned()),
            )),
            None => Ok(()),
        }
    }

    /// XML 1.0 text before the namespace refusal. Index stays a Python
    /// `IndexError` at the insert callers. A namespace `Other` is left for
    /// [`Self::refuse_unsavable_body_edit`] so the diagnostic stays one path.
    fn refuse_non_xml_paragraph_text(&self, py: Python<'_>, text: &str) -> PyResult<()> {
        match self.inner.preflight_checked_paragraph(None, text) {
            Err(oxoffice_docx::Error::Oxml(error)) => {
                Err(docx_to_pyerr(py, oxoffice_docx::Error::Oxml(error)))
            }
            Err(oxoffice_docx::Error::Other(_)) => Ok(()),
            Err(error) => Err(docx_to_pyerr(py, error)),
            Ok(()) => Ok(()),
        }
    }

    fn append_paragraph_candidate(
        slf: Py<Self>,
        py: Python<'_>,
        text: &str,
        style: Option<&str>,
        page_break: bool,
    ) -> PyResult<Py<PyParagraph>> {
        let mut document = slf.borrow_mut(py);
        let index = document.inner.paragraph_count();
        let style = style
            .map(|value| {
                style_id_of_type(&document.inner, value, oxoffice_docx::StyleType::Paragraph)
            })
            .transpose()?;
        document.refuse_non_xml_paragraph_text(py, text)?;
        document.refuse_unsavable_body_edit(py)?;
        // The same edit builds the paragraph read for the inventory and the
        // paragraph placed.
        let edit = |paragraph: &mut oxoffice_docx::Paragraph<'_>| {
            if let Some(style) = &style {
                paragraph.set_style(style);
            }
            if page_break {
                paragraph
                    .add_run("")
                    .add_break(oxoffice_docx::BreakKind::Page);
            }
        };
        let mut appended = oxoffice_docx::Document::detached_text_paragraph(text);
        edit(&mut oxoffice_docx::Paragraph::detached(&mut appended));
        let slot = document.inner.content_count();
        let splice = document.inner.direct_body_paragraph_facts(&appended);
        if let Ok(Some(facts)) = splice
            && let Some(plan) = document.plan_body_splice(
                py,
                BodySplice {
                    slot,
                    removed: 0,
                    inserted: Some(SplicedBodyItem::Paragraph(facts.0, facts.1)),
                },
            )?
        {
            // Every fallible step precedes the in-place native append.
            let publication = crate::document::WordPublication::AppendBody;
            let prepared =
                document.prepare_body_splice(plan, "Document.add_paragraph", &publication)?;
            let path = document
                .revisions
                .capture_prepared(
                    &prepared.revision,
                    smallvec![PathSeg::Body(0), PathSeg::Para(index)],
                )
                .map_err(revision_to_pyerr)?;
            let result = Py::new(
                py,
                PyParagraph::new(slf.clone_ref(py), path)
                    .bind_prepared_splice(&document, &prepared)?,
            )?;
            let savable = document.inner.unsavable_body_edit().is_none();
            edit(&mut document.inner.add_paragraph(text));
            debug_assert_eq!(prepared.plan.paragraph, index);
            document.commit_body_splice(prepared, &publication)?;
            document
                .inner
                .keep_body_savable_after_typed_insert(slot, savable);
            return Ok(result);
        }
        let mut candidate = document.inner.clone_for_staging();
        edit(&mut candidate.add_paragraph(text));
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.add_paragraph",
            crate::document::WordPublication::AppendBody,
        )?;
        let path = document
            .revisions
            .capture_prepared(
                &prepared.revision,
                smallvec![PathSeg::Body(0), PathSeg::Para(index)],
            )
            .map_err(revision_to_pyerr)?;
        let result = Py::new(
            py,
            PyParagraph::new(slf.clone_ref(py), path)
                .bind_prepared(py, &document, &prepared, &candidate, None)?,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(result)
    }
}
