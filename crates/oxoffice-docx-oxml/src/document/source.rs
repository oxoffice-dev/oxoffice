//! Admission and publication of Word sources: source maps, the model resource guard, capture namespace planning, constructor qualification, scoped operations and the admission entry points.

use super::*;

mod admission;
mod capture_namespaces;
mod capture_plan;
mod constructors;
mod control_cursor;
mod field_input;
mod fragments;
mod input;
mod model_guard;
mod operation;
mod operation_models;
mod operation_templates;
mod qualification;
mod returned;
mod scoped_source;
mod source_map;

pub(crate) use admission::{
    admit_paragraph_result_field_input, admit_raw_word_source, admit_reader_source,
    admit_revision_field_input, admit_word_field_input, admit_word_input_with_constructors,
    admit_word_reader, admit_word_source, admit_word_xml,
};
use admission::{admit_word_input, admit_word_input_classified, record_word_source_failure};
use capture_namespaces::{
    CaptureDeclarations, CaptureNamespaceState, capture_index_push,
    capture_non_namespace_attribute, capture_revision_opens_runs, charge_original_capture_binding,
    classify_original_capture, constructor_attribute_prefixes, constructor_namespace_name,
    drawing_metadata_after_capture, finish_original_capture_owner, original_capture_binding,
    original_capture_event_prefixes, original_capture_root_attributes, original_capture_word_name,
    reduce_inherited_bindings, root_attribute_plan_failed, simple_field_header_projection_stable,
};
use capture_plan::{
    admit_original_capture_records, inherited_capture_declarations, plan_capture_with_context,
};
use constructors::ConstructorKind;
pub(crate) use constructors::{PartReaderKind, WordSourceReader, reader_input_origin};
pub use fragments::close_content_fragment_namespaces;
pub(crate) use input::WordXmlInput;
pub(crate) use model_guard::{
    WordOwner, admit_word_model, admitted_word_child, word_model_children_maxima,
    word_model_maxima, word_owner_cost,
};
use model_guard::{captured_namespace_is, decoded_namespace_is, restore_namespace_depths};
pub(crate) use qualification::control_has_property_measurement_failure_in;
use qualification::{original_comment_reference_has_payload, qualify_constructor_failures};
use returned::returned_body_owner;
pub(crate) use returned::{returned_block_inputs, template_paragraph_clone_pairs};
pub(crate) use scoped_source::{TemplateWordRead, template_scalar_bytes};
pub(crate) use source_map::WordPublicationTape;

/// Admission keeps resource refusals distinct from ordinary raw-input failure.
/// Public parsers retain their existing OxmlError representation.
#[derive(Debug, thiserror::Error)]
enum WordSourceFailure {
    #[error("{0}")]
    Input(#[from] OxmlError),
    #[error("{0}")]
    Resource(OxmlError),
}
impl WordSourceFailure {
    fn resource(message: String) -> Self {
        Self::Resource(OxmlError::InvalidValue(message))
    }
}
impl From<WordSourceFailure> for OxmlError {
    fn from(failure: WordSourceFailure) -> Self {
        match failure {
            WordSourceFailure::Input(error) | WordSourceFailure::Resource(error) => error,
        }
    }
}
type WordSourceResult<T> = std::result::Result<T, WordSourceFailure>;

/// Correspondence to one live operation input. Inserted bytes have no original
/// range. Repeated pieces keep repeated output positions and the same origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WordSourceMap {
    length: usize,
    pieces: Vec<WordSourcePiece>,
    word_events: Vec<crate::text::TemplateWordEvent>,
    publication: Vec<WordPublicationPiece>,
    publication_windows: Vec<WordCaptureWindow>,
    publication_owned: Vec<std::ops::Range<usize>>,
    ordinary_owned: Vec<std::ops::Range<usize>>,
}

/// One observation from an actual retained property reader. The map carries
/// only its real capture images, not a guessed enclosing ordinary subtree.
#[derive(Clone, Debug)]
pub(crate) struct WordOrdinaryFact {
    map: std::sync::Arc<WordSourceMap>,
    selected: std::ops::Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WordSourcePiece {
    output: std::ops::Range<usize>,
    original: Option<WordOriginalRange>,
    word: Option<std::ops::Range<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WordOriginalRange {
    part: usize,
    range: std::ops::Range<usize>,
}

/// Positive, partial publication inventory. Empty fields are NOT proof of
/// fieldlessness: unvisited wrapper and fallback consumers can remain pending.
#[derive(Clone, Debug, Default)]
pub(crate) struct WordPublicationCoverage {
    fields: Vec<std::ops::Range<usize>>,
    ordinary: Vec<std::ops::Range<usize>>,
    // Necessary delivery evidence only. Even Some(true) says nothing about
    // unvisited raw consumers or complete retained physical inventory.
    pub(crate) returned_fields_joined: Option<bool>,
}

impl WordPublicationCoverage {
    fn copied_field_bytes(&self, length: usize) -> Result<usize> {
        self.fields.iter().try_fold(0usize, |total, range| {
            if range.start > range.end || range.end > length {
                return Err(OxmlError::InvalidValue(
                    "copied Field publication range is invalid".into(),
                ));
            }
            total.checked_add(range.len()).ok_or_else(|| {
                OxmlError::InvalidValue("copied Field publication work overflow".into())
            })
        })
    }

    pub(crate) fn covers_ranges(&self, ranges: &[std::ops::Range<usize>]) -> bool {
        !ranges.is_empty()
            && ranges.iter().all(|range| {
                if range.is_empty() {
                    return false;
                }
                let index = self
                    .fields
                    .partition_point(|field| field.end <= range.start);
                self.fields
                    .get(index)
                    .is_some_and(|field| field.start <= range.start && range.end <= field.end)
            })
    }

    pub(crate) fn add_ranges(&mut self, ranges: Vec<std::ops::Range<usize>>) {
        self.fields.extend(ranges);
        Self::merge_ranges(&mut self.fields);
        self.remove_field_overlap();
    }

    pub(super) fn add_ordinary_ranges(&mut self, ranges: Vec<std::ops::Range<usize>>) {
        self.ordinary.extend(ranges);
        Self::merge_ranges(&mut self.ordinary);
        self.remove_field_overlap();
    }

    fn merge_ranges(ranges: &mut Vec<std::ops::Range<usize>>) {
        ranges.sort_by_key(|range| range.start);
        let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
        for range in std::mem::take(ranges) {
            if let Some(last) = merged.last_mut()
                && range.start <= last.end
            {
                last.end = last.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
        *ranges = merged;
    }

    // A later positive Field consumer supersedes earlier ordinary metadata.
    // Unknown gaps are never filled by either kind of positive fact.
    fn remove_field_overlap(&mut self) {
        if self.ordinary.is_empty() || self.fields.is_empty() {
            return;
        }
        let mut ordinary = Vec::new();
        let mut first = 0;
        for range in std::mem::take(&mut self.ordinary) {
            while first < self.fields.len() && self.fields[first].end <= range.start {
                first += 1;
            }
            let mut at = range.start;
            for field in self.fields[first..]
                .iter()
                .take_while(|field| field.start < range.end)
            {
                if at < field.start {
                    ordinary.push(at..field.start.min(range.end));
                }
                at = at.max(field.end);
                if at >= range.end {
                    break;
                }
            }
            if at < range.end {
                ordinary.push(at..range.end);
            }
        }
        self.ordinary = ordinary;
    }
}

#[derive(Clone, Debug)]
pub(crate) struct WordPublicationPiece {
    output: std::ops::Range<usize>,
    current: std::ops::Range<usize>,
    collector: std::sync::Weak<std::sync::Mutex<WordPublicationCoverage>>,
}

/// An event-preserving, trim(false) capture's contiguous retained subwindow.
/// Unlike a copied piece its two lengths can differ. Ownership still requires
/// actual scanner-selected boundaries with monotonic copied CURRENT images.
#[derive(Clone, Debug)]
pub(crate) struct WordCaptureWindow {
    output: std::ops::Range<usize>,
    current: std::ops::Range<usize>,
    collector: std::sync::Weak<std::sync::Mutex<WordPublicationCoverage>>,
}

struct WordNamespaceBinding {
    uri: Vec<u8>,
    depth: usize,
}

struct WordAdmissionFrame {
    admitted: bool,
    word: bool,
    // An admitted legacy unbound run route, not a namespace URI binding.
    legacy_run_prefix: Option<Vec<u8>>,
    mc: bool,
    vml: bool,
    owner: Option<SdtOwner>,
    anchor_depth: Option<usize>,
    namespace_undo: Vec<(Vec<u8>, Option<WordNamespaceBinding>)>,
    mc_capture_depth: Option<usize>,
    qualified_drawing_depth: Option<usize>,
    control: bool,
    ordered_field_charged: bool,
    cost: usize,
    table_depth: usize,
    local: std::borrow::Cow<'static, [u8]>,
    paragraph_reader_root: bool,
    ordered_revision_root: bool,
    fact_start: usize,
    copy_fact: bool,
    copy_depth: usize,
    copy_scope_bytes: usize,
}

struct OriginalCaptureScope {
    declarations: Vec<u32>,
    /// This element and its descendants are retained as one verbatim raw
    /// record, so modeled descendants are part of the capture source.
    raw: bool,
    /// This element is inside a captured WP inline or anchor raw record. The
    /// record closes against its ancestors, so only run-frame and drawing-scope
    /// bindings are copied into it.
    typed_drawing: bool,
    /// Inline drawing raw never constructs embedded Word runs.
    inline_drawing: bool,
    /// Inner lexical markup is decoded as a text value, never parsed as models.
    text_span: bool,
    /// This element is a direct `w:drawing` whose children the drawing parser
    /// either captures as one raw inline or anchor or discards.
    drawing_root: bool,
    /// The drawing parser discards this subtree with `read_to_end_into`, so no
    /// capture record is created for it.
    dropped: bool,
    /// This element is one physical inline or anchor raw stored by the drawing
    /// parser. Each such root gets its own injected declaration set.
    raw_copy: bool,
    /// Direct `w:r` children are constructed by this element's parser.
    constructs_runs: bool,
    /// Direct `w:hyperlink` children are constructed by this element's parser.
    constructs_hyperlinks: bool,
    /// Actual and synthetic wrapper paragraphs parse revisions without a
    /// required author.
    paragraph: bool,
    /// Ruby rejected the annotation, so its runs are not constructed.
    ruby_opaque: bool,
    /// A rejected fallible child retains bytes without constructing typed
    /// descendants. Its enclosing raw-capture action remains independent.
    unconstructed: bool,
    /// This direct run child owns its independently namespace-closed raw source.
    comment_root: bool,
}

struct OriginalCaptureOwner {
    depth: usize,
    /// Run and hyperlink parents have different direct-child grammars.
    run: bool,
    /// Bindings stored once on the shared run-frame record.
    shared: Vec<u32>,
    /// Run-root declarations stored again on the lexical local record.
    local: Vec<u32>,
    /// Declaration sets injected into each physical drawing raw.
    raw_copies: Vec<Vec<u32>>,
    open_raw: Vec<Vec<u32>>,
    /// The one direct comment-reference carrier currently being inspected.
    open_comment: Option<(usize, Vec<u32>)>,
}

struct OriginalCaptureClass {
    drawing_root: bool,
    typed_root: bool,
    inside_typed: bool,
    dropped: bool,
    raw: bool,
    owner: bool,
}

/// Global dispatch and only its original wrapper declaration updates. Item
/// readers keep lexical scopes. Wrapper End never restores global reader scope.
#[derive(Debug)]
struct PartReaderEvent {
    start: usize,
    kind: ConstructorKind,
    updates: Vec<u32>,
}

struct ConstructorFrame<'a> {
    kind: ConstructorKind,
    start: usize,
    failed: bool,
    content_seen: bool,
    properties_seen: bool,
    control_occurrence: Option<usize>,
    /// Decoded at the source occurrence, applied only at the actual late boundary.
    root_attribute_failed: bool,
    /// Success is proved only for shared primitive actions and exact cursor modes.
    known: bool,
    /// First actual Option::None boundary, distinct from a propagated Err.
    ruby_none_at: Option<usize>,
    /// Reached header only. Decode its scalar after this Started child closes.
    ruby_property_opening: Option<(WordXmlInput<'a>, usize)>,
    revision_mode: Option<crate::revision::RevisionBodyMode>,
    revision_stopped: bool,
    lexical_revision: bool,
    revision_height: usize,
    drawing_stopped: bool,
    drawing_has_id: bool,
    drawing_metadata_opening: Option<(WordXmlInput<'a>, usize)>,
    /// Nearest active containing constructor, excluding this frame itself.
    drawing_owner: Option<usize>,
    source_depth: usize,
    /// A reached typed body needs its own resource admission before late facts.
    drawing_prefix_unknown: bool,
}

#[derive(Debug)]
struct SimpleSelectorOccurrence {
    opening: std::ops::Range<usize>,
    whole: std::ops::Range<usize>,
    ordered: bool,
    result: std::ops::Range<usize>,
}

struct DirectSimpleSelectorFold {
    opening: std::ops::Range<usize>,
    result_start: usize,
    ordered: bool,
    result_seen: bool,
    traversed: usize,
    consumed: Option<(usize, bool)>,
    reusable: bool,
}

#[derive(Debug)]
pub(crate) struct ConstructorFailures {
    raw_subtrees: Vec<RawConstructorSpan>,
    root_raw: bool,
    controls: Vec<ControlOccurrence>,
    discarded: Vec<std::ops::Range<usize>>,
    simple_headers: Vec<SimpleHeaderOccurrence>,
    part_events: Vec<PartReaderEvent>,
    paragraph_result_root: Option<usize>,
    ordered_revision_root: Option<usize>,
    selector_paragraph_root: Option<std::ops::Range<usize>>,
    simple_selectors: Vec<SimpleSelectorOccurrence>,
}

#[derive(Debug)]
struct SimpleHeaderOccurrence {
    /// Identity in the actual admission input before explicit insertion mapping.
    original_opening: std::ops::Range<usize>,
    original_part: usize,
    opening: std::ops::Range<usize>,
    header: crate::text::SimpleFieldHeader,
}

#[derive(Debug)]
struct RawConstructorSpan {
    span: std::ops::Range<usize>,
    /// Controls replace their complete owner with a raw capture. Other fallible
    /// children retain their parent's capture policy and stop typed children.
    control: bool,
}

/// An original source occurrence, never a derived-byte or equal-text identity.
#[derive(Debug)]
struct ControlOccurrence {
    owner: SdtOwner,
    original_part: usize,
    source_span: std::ops::Range<usize>,
    after: usize,
    raw: bool,
}

pub(crate) struct WordFieldInput {
    initial_delivery: bool,
    map: std::sync::Arc<WordSourceMap>,
    registry: std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>,
}

/// A disjoint preorder range reserved by the actual parent constructor.
/// Root namespace injection and capture retain element occurrence order.
pub(crate) struct ControlSourceCursor<'a> {
    occurrences: &'a [ControlOccurrence],
    next: usize,
    end: usize,
    projected_aliases: bool,
    initial_field_delivery: bool,
    simple_headers: &'a [SimpleHeaderOccurrence],
    simple_selectors: &'a [SimpleSelectorOccurrence],
    selector_paragraph_root: Option<&'a std::ops::Range<usize>>,
    selector_requested: bool,
    selector_ready: bool,
    input: Option<&'a [u8]>,
    source_map: std::sync::Arc<WordSourceMap>,
    field_origins: Option<std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>>,
}

#[derive(Debug)]
pub(crate) struct AdmittedWordSource {
    work: (usize, usize),
    constructors: std::sync::Arc<ConstructorFailures>,
    projected_aliases: bool,
    source_map: std::sync::Arc<WordSourceMap>,
}

struct FragmentNamespaceClosure {
    insertion: usize,
    bindings: Vec<u32>,
    bytes: usize,
    original_w_binding: bool,
}

#[derive(Default)]
pub(super) struct ReturnedWordRead {
    comments: Option<crate::comments::CommentsReadFacts>,
    pub(super) body: Vec<(usize, crate::text::WordBlockSourceMaps)>,
    notes: Vec<Vec<(usize, crate::text::WordBlockSourceMaps)>>,
}

/// An operation-produced source. Its bytes and correspondence move together.
/// There is deliberately no public constructor or Clone implementation.
#[doc(hidden)]
pub struct ScopedWordSource {
    read_facts: std::sync::Mutex<Option<ReturnedWordRead>>,
    initial_delivery: bool,
    bytes: Vec<u8>,
    publication: std::sync::Arc<std::sync::Mutex<WordPublicationCoverage>>,
    source_map: WordSourceMap,
    // Word-only physical correspondence never grants Field source/current work.
    word_source_map: WordSourceMap,
    word_events: Vec<crate::text::TemplateWordEvent>,
    constructors: Option<std::sync::Arc<ConstructorFailures>>,
    operation: std::sync::Arc<()>,
}

// Carrier assembly uses this one byte/map recipe.
// The caller chooses its actual admission boundary before materialization.
struct SourceAssembly<'s> {
    chunks: Vec<&'s [u8]>,
    coverage: WordPublicationCoverage,
    source_map: WordSourceMap,
    word_source_map: WordSourceMap,
    word_events: Vec<crate::text::TemplateWordEvent>,
    length: usize,
}

/// Template copies have their own input and demand accounting. Field work
/// cannot spend this allowance, including the per-operand fixed component.
#[derive(Default)]
pub(crate) struct TemplateCopyWork {
    allowance: usize,
    copied: usize,
    inputs: std::collections::HashSet<usize>,
    word_inputs: std::collections::HashMap<
        crate::text::TemplateWordOrigin,
        std::collections::BTreeMap<usize, usize>,
    >,
    refused: bool,
}

/// One scoped edit's reached original sources and cumulative materializations.
/// Field/current-value aggregation remains separate from capture and insertion.
#[doc(hidden)]
pub struct ScopedWordOperation<'a> {
    original_parts: Vec<&'a [u8]>,
    part_names: std::collections::HashMap<&'a str, usize>,
    current_paragraphs: std::collections::HashMap<usize, &'a CT_P>,
    current_word_sources: Vec<&'a [u8]>,
    current_word_source_keys: std::collections::HashSet<(usize, usize)>,
    current_word_views: std::collections::BTreeMap<usize, usize>,
    current_texts: std::collections::HashMap<usize, &'a crate::text::CT_Text>,
    current_fields: std::collections::HashMap<usize, &'a crate::text::Field>,
    field_work: crate::text::WordOperationWork,
    template_datums: std::collections::HashMap<usize, &'a serde_json::Value>,
    template_copy_work: TemplateCopyWork,
    field_origins: std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>,
    identity: std::sync::Arc<()>,
    declarations: CaptureDeclarations,
    ranges: std::collections::BTreeMap<(usize, usize), usize>,
    source_bytes: usize,
    attributes: std::collections::HashMap<(usize, usize, usize), usize>,
    attribute_bytes: usize,
    capture_namespaces: std::collections::HashSet<u32>,
    capture_scope_bytes: usize,
    insertion_namespaces: std::collections::HashSet<u32>,
    insertion_scope_bytes: usize,
    capture_needed: usize,
    insertion_needed: usize,
}

/// Exact capture-only metric. Physical attribute occurrences and pooled
/// namespace facts stay separate so a scoped operation can register origins.
struct OriginalCaptureWork {
    needed: usize,
    attributes: Vec<(std::ops::Range<usize>, usize)>,
    namespaces: Vec<u32>,
}
