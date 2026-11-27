//! Text content elements: `CT_P` (paragraph), `CT_R` (run), `CT_Text`.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oxoffice_core::xml_reader::{NsReader, Reader};
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::{Writer, XmlVersion};

use crate::content_control::{CT_Sdt, SdtContent, SdtOwner};
use crate::drawing::CT_Drawing;
use crate::error::{OxmlError, Result};
use crate::math::OfficeMath;
use crate::namespace::{R_NS, matches_local_name};
use crate::numbering::{namespace_bindings, word_prefixes_at};
use crate::properties::{CT_PPr, CT_RPr, is_word_attribute, is_word_element};
use crate::raw_xml::{capture_element, capture_empty};
use crate::revision::{CT_Revision, RevisionKind};
use crate::ruby::CT_Ruby;
use crate::table::{CT_Row, CT_Tbl, CT_Tc, CellContent};

mod bindings;
mod emission;
mod owner_check;
mod run;
mod work;

use bindings::{
    GeneratedWordPrefix, MC_NS, ROOT_ATTRIBUTES_ELEMENT, ROOT_BINDINGS, W14_NS, WP_NS,
    affected_source_dynamic_work, external_binding_indices_used_by_raw, namespace_declaration,
    needs_generated_word_binding, published_field_root_plan, published_field_roots_in,
    push_root_attribute_record_in, qualified_name_prefix, raw_with_root_bindings_in,
    retain_foreign_word_alias_declaration, write_retained_roots_final_transport,
};
pub(crate) use bindings::{
    ROOT_ATTRIBUTES_POSITION, ROOT_MC_BINDING, ROOT_R_BINDING, ROOT_WP_BINDING, RootBindingScope,
    XmlWriteRole, bindings_outside_retained_root, capture_root_attribute_record,
    emit_retained_roots, emit_retained_roots_in, emit_retained_roots_mapped_in,
    emitted_ancestor_scope, emitted_binding_scope, emitted_bindings, emitted_root_binding_scope,
    external_bindings_used_by_raw, field_scope_bindings, generated_subtree_end,
    generated_word_name, generated_word_start, generated_word_start_in, plan_w14_on_part_root,
    push_root_attribute_record, raw_namespace_declarations, raw_root_binding_scope,
    raw_with_external_bindings, raw_with_external_bindings_and_map, raw_with_external_bindings_in,
    restore_generated_word_binding, retained_binding_plan, retained_binding_work,
    retained_owner_work, retained_raw_roots, retained_root_plan, retained_source_envelope,
    root_attribute_value_prefixes, root_binding_scope, with_publication_bindings,
    write_raw_with_word_override_in,
};
pub use bindings::{declare_w14_on_part_root, is_root_attribute_record};
use emission::{FieldXmlRecord, field_source_prefix};
pub(crate) use owner_check::{discard_checked_paragraph, validate_word_owner_fragment};
pub use run::{
    BreakClear, BreakType, CT_R, CommentRangeMarker, ComplexFieldHyperlink, HyperlinkSpan,
    RunContent, ST_PTabAlignment, ST_PTabLeader, ST_PTabRelativeTo, SpecialCharacter,
    hyperlink_revision_index,
};
use run::{
    ParsedHyperlinkAttributes, RAW_COMMENT_REFERENCE_FLAG, retained_comment_reference_id,
    run_xml_root, write_run_raw_source,
};
pub(crate) use run::{
    RunChildAction, RunChildPlan, SpecialRunValue, comment_reference_has_payload_attributes,
    decode_run_text_span, field_run, hyperlink_revision_slot, parse_run_raw,
    parse_run_raw_with_input, parse_run_raw_with_observation, parse_typed_special_run_child,
    started_reference_has_payload,
};
use work::{
    FieldOriginEntry, MappedFieldRange, OriginalFieldFacts, ParsedFieldCache, StoredSourceOrigin,
    TemplateFieldRegistry, TemplateParagraphTextLease, TemplateScalarBorrow, TemplateScalarOrigin,
    TemplateTextCloneLease, WordPayloadRange, WordPayloadTransport, admit_initial_field_capture,
    append_escaped_source, checked_field_scope_bytes, escaped_source_len, escaped_source_piece,
    field_argument_work, field_switch_work, initial_field_work_base, initial_field_work_bytes,
    legacy_field_work_bytes, template_word_scalar_input, template_word_text_input,
};
pub(crate) use work::{
    FinalWordBlock, ParsedWordRunTexts, ParsedWordText, TemplateParagraphCloneLease,
    TemplateWordEvent, TemplateWordOrigin, WordParagraphReadFacts, WordParagraphTextFacts,
    escaped_source, final_word_source, ordered_normalization_children, write_final_word_block,
    write_prepared_validation_xml,
};

mod field;
mod field_complex;
mod field_instruction;
mod field_legacy_form;
mod field_rewrite;
mod field_simple;
mod field_write;
mod written;

#[doc(hidden)]
pub use written::WrittenVisitor;

pub use field::FieldInstruction;
use field::{FIELD_CACHE_VALIDATION_SCOPE, cached_run_display};
use field_complex::{
    FieldSpanPart, field_break_type, field_span_parts, has_typed_boundary_inside,
    project_complex_fields,
};
pub(crate) use field_complex::{
    WordBlockComponent, WordBlockSourceMaps, WordExtraSourceMaps, WordParagraphReadOutput,
    inline_field_paragraph_from_facts, preflight_inline_field_copies, pure_complex_marker_run,
};
use field_instruction::{
    InstructionOperand, InstructionPart, classify_instruction_tokens, lex_field_text,
    parse_field_instruction, parse_instruction_syntax, switch_is_known_flag, switch_takes_argument,
};
pub use field_instruction::{
    NESTED_FIELD_STAND_IN, bibliography_option_switch_edits, bookmark_reference_span,
    bookmark_reference_spans, hyperlink_has_address, story_field_instruction_has_name,
};
use field_legacy_form::{
    InlineFieldParagraph, NestedEffectiveSegment, PhysicalRunSources, complex_outer_result_present,
    field_publication_bytes, legacy_form_author_parts, legacy_form_canonical_display,
    legacy_text_value_only_change, lenient_legacy_form_name, parse_legacy_form_data,
    rewrite_legacy_form_source, validate_legacy_form_authoring, write_legacy_form_ff_data,
    write_missing_legacy_result_region,
};
use field_rewrite::{
    CachedFieldSource, FieldSourcePiece, cached_field_source, cached_runs_need_typed_projection,
    field_attribute_source, field_source_has_unmodeled_semantic_attributes, field_source_lock,
    is_field_word_attribute, lexical_attribute_value_range, nested_instruction_source_edits,
    replace_typed_field_cache, scan_complex_source, source_has_note_reference_name,
    update_complex_field_source, update_complex_instruction_source, update_nested_field_sources,
    update_simple_field_source, usable_qualified_word_prefix, validate_cached_comment_ranges,
    write_field_lock_update, write_typed_cached_runs,
};
pub use field_simple::story_simple_field_is_typed;
pub(crate) use field_simple::{
    SimpleFieldHeader, SimpleOwnershipAction, ordered_paragraph_xml_in,
    paragraph_content_mapped_in, parse_simple_field, retain_ordered_field_source_identities,
    retain_ordered_field_source_identities_in, simple_ownership_action,
    source_has_ordered_field_owner,
};
use field_simple::{
    contiguous_ordered_result_runs, ordered_paragraph_mapped_xml_in, parse_empty_simple_field,
    parse_ordered_field_result, parse_simple_field_with_summary, replace_ordered_result_runs,
    replace_ordered_result_text,
};
use field_write::{
    FieldResultPart, FieldResultParts, canonical_instruction_text, capture_standalone_event,
    collect_story_complex_field_sources, instruction_contains_nested, instruction_for_write,
    instruction_source_identity_eq, instruction_structure_eq, is_xml_whitespace,
    needs_space_preservation, source_contains_own_bookmark, write_field, write_field_raw_output,
    write_field_result_content_inner, write_field_result_content_with_binding,
    write_field_result_run, write_field_text_with_source, write_field_with_result_properties,
    write_word_run_start,
};
pub(crate) use field_write::{write_mixed_field_run, write_published_run_field, write_run_field};

mod block_bookmarks;
pub use block_bookmarks::BlockBookmarks;
mod ref_cycles;
pub use ref_cycles::{
    bookmark_name_key, bookmarks_depending_on, field_ref_targets, same_bookmark_name,
};
mod paragraph_projection;
mod paragraph_read;
mod paragraph_write;

use paragraph_projection::{
    accepted_control_bookmark_projection, accepted_revision_bookmark_projection,
    append_nested_bookmark_projection, current_paragraph_bookmark_projection,
    element_prefix_has_binding, is_content_revision_element, is_element_in_namespace,
    parse_hyperlink_attributes, parse_hyperlink_children, prefixes_with_assumed_word_owner,
    sdt_content_bookmark_starts,
};
use paragraph_write::{safe_hyperlink_name, write_hyperlink_record};

static NEXT_FIELD_SOURCE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
pub(crate) struct GeneratedWordName<'a> {
    prefix: GeneratedWordPrefix<'a>,
    declaration: bool,
}

/// Primitive root attribute facts. Namespace URIs remain borrowed from the
/// caller's binding owner until actual record emission.
pub(crate) struct RootAttributePlan {
    attributes: Vec<(String, String)>,
    used_prefixes: Vec<Vec<u8>>,
}

/// `CT_Text` — The text content of a run, with optional xml:space="preserve".
#[derive(Debug, Clone, PartialEq)]
pub struct CT_Text {
    pub text: String,
    pub preserve_space: bool,
}

impl CT_Text {
    pub fn new(text: &str) -> Self {
        CT_Text {
            text: text.to_string(),
            preserve_space: text.starts_with(' ') || text.ends_with(' '),
        }
    }

    /// Keep the first `at` characters and return the rest.
    ///
    /// Either part preserves space when the original did or when the split
    /// leaves a space at one of its ends.
    fn split_off_at(&mut self, at: usize) -> CT_Text {
        let byte = self
            .text
            .char_indices()
            .nth(at)
            .map_or(self.text.len(), |(byte, _)| byte);
        let rest = self.text.split_off(byte);
        let inherited = self.preserve_space;
        let preserve = |text: &str| inherited || text.starts_with(' ') || text.ends_with(' ');
        let rest = CT_Text {
            preserve_space: preserve(&rest),
            text: rest,
        };
        self.preserve_space = preserve(&self.text);
        rest
    }
}

/// A parsed Word field with its stored result and update marker.
#[derive(Debug)]
pub struct Field {
    pub instruction: FieldInstruction,
    pub cached_result: String,
    pub dirty: Option<bool>,
    locked: Option<bool>,
    /// Typed legacy form metadata retained below the begin `w:fldChar`.
    pub legacy_form: Option<LegacyFormFieldData>,
    legacy_form_parse_error: bool,
    /// Explicit reset default. Absent metadata stays `None`.
    legacy_reset_default: Option<LegacyFormFieldValue>,
    /// Borrowed text fallback for an absent result region.
    legacy_text_default: Option<String>,
    /// A physical result region is present, or a setter published one.
    legacy_result_published: bool,
    /// Last parent cache derived from children or from the imported source.
    /// A later direct parent assignment diverges from this baseline.
    derived_parent_cache: String,
    /// Authoring facts written on a new begin `w:ffData` only.
    legacy_author: Option<LegacyFormAuthor>,
    nested_order: Vec<NestedFieldPosition>,
    cached_fields: Vec<(std::ops::Range<usize>, Field)>,
    simple_cached_runs: Option<Vec<CT_R>>,
    typed_cached_runs: Option<Vec<CT_R>>,
    /// Checked complete source sizes for immutable authored baseline and current
    /// typed replacement. Baseline runs can remain shared after replacement.
    checked_payload_source_bytes: (usize, usize),
    typed_cached_comment_ranges: Vec<CommentRangeMarker>,
    parsed_cached_comment_ranges: Vec<CommentRangeMarker>,
    parsed_cached_runs: Option<Vec<CT_R>>,
    ordered_result: Option<OrderedFieldResult>,
    source: Arc<FieldSource>,
    // Inert movable producer identity. All interval facts belong to the
    // operation. Ordinary models and parsers have no receipt.
    template_word_receipt: Option<(std::sync::Weak<()>, usize)>,
    /// The physical run this field shares with text outside it, if any.
    span: Option<Arc<FieldRunSpan>>,
}

// Every owned Field edge that can repeat without consuming a resource depth
// is scheduled here. Immutable source/span and ordered paragraph owners retain
// their existing Arc sharing. Other run payloads retain their existing Clone.
enum FieldCloneStep<'a> {
    Enter(&'a Field, Option<&'a Field>),
    Finish(&'a Field, Option<&'a Field>, usize),
}

impl Clone for Field {
    fn clone(&self) -> Self {
        // No receipt registry is entered by an ordinary clone, so this path
        // has no fallible producer operation beyond the existing allocations.
        self.clone_fields(None, None)
            .expect("ordinary Field clone has no receipt producer")
    }
}

impl Field {
    fn clone_fields(
        &self,
        frozen: Option<&Field>,
        mut work: Option<&mut WordOperationWork>,
    ) -> Result<Self> {
        let mut pending = vec![FieldCloneStep::Enter(self, frozen)];
        let mut completed = Vec::new();
        while let Some(step) = pending.pop() {
            match step {
                FieldCloneStep::Enter(field, frozen) => {
                    let children = field.owned_clone_children();
                    let frozen_children = frozen.map(Field::owned_clone_children);
                    let same_slots = frozen_children
                        .as_ref()
                        .is_some_and(|frozen| frozen.len() == children.len());
                    pending.push(FieldCloneStep::Finish(field, frozen, completed.len()));
                    pending.extend(
                        children
                            .into_iter()
                            .enumerate()
                            .rev()
                            .map(|(index, child)| {
                                FieldCloneStep::Enter(
                                    child,
                                    same_slots.then(|| {
                                        frozen_children.as_ref().expect("checked clone slots")
                                            [index]
                                    }),
                                )
                            }),
                    );
                }
                FieldCloneStep::Finish(field, frozen, start) => {
                    let mut children = completed.split_off(start).into_iter();
                    let mut cloned = field.clone_with_children(&mut children);
                    debug_assert!(children.next().is_none(), "unconsumed field clone slot");
                    if let Some(work) = work.as_deref_mut() {
                        work.template_mint_field(field, frozen, &mut cloned)?;
                    }
                    completed.push(cloned);
                }
            }
        }
        Ok(completed.pop().expect("root field clone"))
    }

    pub(crate) fn prepare_template_in(
        &self,
        frozen: Option<&Field>,
        normalize: bool,
        work: &mut WordOperationWork,
    ) -> Result<Self> {
        fn normalize_field(field: &mut Field, work: &mut WordOperationWork) -> Result<()> {
            for argument in field.instruction.arguments.iter_mut().chain(
                field
                    .instruction
                    .switches
                    .iter_mut()
                    .filter_map(|switch| switch.argument.as_mut()),
            ) {
                for child in argument.nested_fields_mut() {
                    normalize_field(child, work)?;
                }
            }
            let count = field.cached_fields_in_source_order().len();
            for index in 0..count {
                let child = field.cached_field_mut(index).ok_or_else(|| {
                    OxmlError::InvalidValue(
                        "current field preparation lost its actual cached slot".into(),
                    )
                })?;
                normalize_field(child, work)?;
            }
            field.normalize_cached_result_owner_in(work)
        }
        let mut current = self.clone_fields(frozen, Some(work))?;
        if normalize {
            normalize_field(&mut current, work)?;
        }
        Ok(current)
    }
}

impl Field {
    /// Whether a piece this field holds answers: `raw` is asked of its
    /// stored source and of a pending ordered result, `run` of every result
    /// run it keeps (a new field's, a simple field's, the typed and parsed
    /// ones), and nested result fields are asked the same. A walk that must
    /// not miss what a field result shows (an `INCLUDEPICTURE` result) asks
    /// here.
    pub(crate) fn any_held_piece(
        &self,
        raw: &dyn Fn(&[u8]) -> bool,
        run: &dyn Fn(&CT_R) -> bool,
    ) -> bool {
        let source = match self.source.as_ref() {
            FieldSource::Parsed { raw_xml, .. } => raw(raw_xml),
            FieldSource::New { cached_runs, .. } => cached_runs.iter().flatten().any(run),
        };
        source
            || self
                .ordered_result
                .as_ref()
                .is_some_and(|ordered| raw(&ordered.normalized_xml))
            || [
                &self.simple_cached_runs,
                &self.typed_cached_runs,
                &self.parsed_cached_runs,
            ]
            .into_iter()
            .flatten()
            .flatten()
            .any(run)
            || self
                .cached_fields
                .iter()
                .any(|(_, nested)| nested.any_held_piece(raw, run))
    }
}

impl Field {
    /// The direct formatting of the run that opens this field, read from its
    /// source. A field that stores no result, such as a legacy checkbox
    /// saved by Word, draws with it.
    pub fn begin_run_properties(&self) -> Option<CT_RPr> {
        let FieldSource::Parsed {
            raw_xml,
            word_prefixes,
            ..
        } = self.source.as_ref()
        else {
            return None;
        };
        // Entries starting with NUL carry namespace bindings, the rest are the
        // prefixes bound to the main namespace.
        let mut prefixes: Vec<String> = word_prefixes
            .iter()
            .filter(|entry| !entry.starts_with('\0'))
            .cloned()
            .collect();
        if prefixes.is_empty() {
            prefixes.push("w".to_owned());
        }
        let mut reader = Reader::from_reader(raw_xml.as_slice());
        let mut buf = Vec::new();
        let mut in_run = false;
        loop {
            match reader.read_event_into(&mut buf).ok()? {
                Event::Start(start) if !in_run => {
                    in_run = matches_local_name(start.name().as_ref(), b"r");
                }
                Event::Start(start) if matches_local_name(start.name().as_ref(), b"rPr") => {
                    return CT_RPr::from_xml_with_prefixes(&mut reader, &prefixes).ok();
                }
                // The run's first child is not its properties: it has none.
                Event::Start(_) | Event::Empty(_) | Event::End(_) if in_run => return None,
                Event::Eof => return None,
                _ => {}
            }
            buf.clear();
        }
    }

    fn owned_clone_children(&self) -> Vec<&Field> {
        let mut children = Vec::new();
        for argument in self.instruction.arguments.iter().chain(
            self.instruction
                .switches
                .iter()
                .filter_map(|switch| switch.argument.as_ref()),
        ) {
            children.extend(argument.nested_fields());
        }
        children.extend(self.cached_fields.iter().map(|(_, child)| child));
        for runs in [
            &self.simple_cached_runs,
            &self.typed_cached_runs,
            &self.parsed_cached_runs,
        ]
        .into_iter()
        .flatten()
        {
            for run in runs {
                for content in &run.content {
                    if let RunContent::Field(child) = content {
                        children.push(child);
                    }
                }
            }
        }
        children
    }

    fn clone_with_children(&self, children: &mut std::vec::IntoIter<Field>) -> Self {
        let instruction = FieldInstruction {
            raw: self.instruction.raw.clone(),
            name: self.instruction.name.clone(),
            arguments: self
                .instruction
                .arguments
                .iter()
                .map(|argument| clone_field_argument(argument, children))
                .collect(),
            switches: self
                .instruction
                .switches
                .iter()
                .map(|switch| FieldSwitch {
                    name: switch.name.clone(),
                    argument: switch
                        .argument
                        .as_ref()
                        .map(|argument| clone_field_argument(argument, children)),
                })
                .collect(),
        };
        let cached_fields = self
            .cached_fields
            .iter()
            .map(|(range, _)| {
                (
                    range.clone(),
                    children.next().expect("cached field clone slot"),
                )
            })
            .collect();
        let simple_cached_runs = self
            .simple_cached_runs
            .as_ref()
            .map(|runs| clone_field_runs(runs, children));
        let typed_cached_runs = self
            .typed_cached_runs
            .as_ref()
            .map(|runs| clone_field_runs(runs, children));
        let parsed_cached_runs = self
            .parsed_cached_runs
            .as_ref()
            .map(|runs| clone_field_runs(runs, children));
        Self {
            instruction,
            cached_result: self.cached_result.clone(),
            dirty: self.dirty,
            locked: self.locked,
            legacy_form: self.legacy_form.clone(),
            legacy_form_parse_error: self.legacy_form_parse_error,
            legacy_reset_default: self.legacy_reset_default.clone(),
            legacy_text_default: self.legacy_text_default.clone(),
            legacy_result_published: self.legacy_result_published,
            derived_parent_cache: self.derived_parent_cache.clone(),
            legacy_author: self.legacy_author.clone(),
            nested_order: self.nested_order.clone(),
            cached_fields,
            simple_cached_runs,
            typed_cached_runs,
            checked_payload_source_bytes: self.checked_payload_source_bytes,
            typed_cached_comment_ranges: self.typed_cached_comment_ranges.clone(),
            parsed_cached_comment_ranges: self.parsed_cached_comment_ranges.clone(),
            parsed_cached_runs,
            ordered_result: self.ordered_result.clone(),
            source: Arc::clone(&self.source),
            template_word_receipt: self.template_word_receipt.clone(),
            span: self.span.as_ref().map(Arc::clone),
        }
    }
}

fn clone_field_argument(
    argument: &FieldArgument,
    children: &mut std::vec::IntoIter<Field>,
) -> FieldArgument {
    match argument {
        FieldArgument::Text(text) => FieldArgument::Text(text.clone()),
        FieldArgument::Nested(_) => FieldArgument::Nested(Box::new(
            children.next().expect("instruction field clone slot"),
        )),
        FieldArgument::Joined(pieces) => FieldArgument::Joined(
            pieces
                .iter()
                .map(|piece| clone_field_argument(piece, children))
                .collect(),
        ),
    }
}

fn clone_field_runs(runs: &[CT_R], children: &mut std::vec::IntoIter<Field>) -> Vec<CT_R> {
    runs.iter()
        .map(|run| CT_R {
            properties: run.properties.clone(),
            content: run
                .content
                .iter()
                .map(|content| match content {
                    RunContent::Field(_) => {
                        RunContent::Field(children.next().expect("run field clone slot"))
                    }
                    content => content.clone(),
                })
                .collect(),
            extra_xml: run.extra_xml.clone(),
            extra_xml_positions: run.extra_xml_positions.clone(),
            alt_drawings: run.alt_drawings.clone(),
        })
        .collect()
}

/// One current ordered stored owner. The outer Field retains its original locator.
#[derive(Debug, Clone)]
struct OrderedFieldResult {
    paragraph: Arc<CT_P>,
    source_range: std::ops::Range<usize>,
    normalized_xml: Arc<Vec<u8>>,
    normalized_cache: String,
    pending: bool,
    pending_payload: bool,
    pending_payload_source_bytes: usize,
}

/// The actual normalization phase selected before its candidate Field clone.
/// Child normalization and source emission still use their concrete producers.
struct OrderedNormalizationPlan<'a> {
    work: usize,
    field_changes: bool,
    cache_override: bool,
    requested: &'a str,
    source_context: Vec<(&'a str, &'a str)>,
}

struct ParsedFieldMetadata {
    legacy_form: Option<LegacyFormFieldData>,
    reset_default: Option<LegacyFormFieldValue>,
    text_default: Option<String>,
    parse_error: bool,
    result_published: bool,
    form: FieldForm,
}

/// The unchanged local formula split into actual input roles. These sizes do
/// not prove physical provenance or grant independent operation credit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NormalizationInputWork {
    retained_source: usize,
    scope: usize,
    current: usize,
    checked_payload: [usize; 2],
    stored_result: usize,
    pending_payload: usize,
}

#[derive(Default)]
pub(crate) struct FieldOriginRegistry {
    source_resource_refused: bool,
    source_resource_message: Option<String>,
    entries: std::collections::HashMap<usize, FieldOriginEntry>,
    sweep: std::collections::VecDeque<usize>,
    stored: std::collections::HashMap<usize, StoredSourceOrigin>,
    stored_sweep: std::collections::VecDeque<usize>,
    original_facts: std::collections::HashMap<OriginalFieldKey, Arc<OriginalFieldFacts>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct OriginalFieldKey {
    physical: Vec<(usize, usize, usize)>,
    // None denotes the actual simple-field opening. Some indexes the canonical
    // complex scanner's raw child slot inside this proved physical run.
    marker: Option<usize>,
}

/// One checked model ingress. Derived models never register another baseline.
/// This is the model-source branch of the operation context, not original OPC
/// correspondence. Original parser admission must supply its mapped branch.
pub(crate) struct WordOperationWork {
    constructor_allowance: usize,
    constructor_actions: usize,
    field_output_metric: bool,
    resource_refused: bool,
    current_inputs: HashSet<usize>,
    template_current_inputs: HashSet<usize>,
    retained_inputs: HashSet<usize>,
    stored_inputs: HashSet<usize>,
    original_current_inputs: HashSet<OriginalFieldKey>,
    original_source_ranges: std::collections::BTreeMap<(usize, usize), usize>,
    pub(crate) field_origins: Arc<std::sync::Mutex<FieldOriginRegistry>>,
    normalized_output: Arc<std::sync::Mutex<NormalizedOutputWork>>,
    template_word_capture: Option<Arc<std::sync::Mutex<TemplateWordCapture>>>,
    template_field_registry: Option<TemplateFieldRegistry>,
}

pub(crate) struct TemplateWordCapture {
    note_drawing_bindings: bool,
    text_origins: Arc<std::collections::HashMap<usize, (TemplateWordOrigin, usize)>>,
    produced_texts: std::collections::HashMap<usize, TemplateScalarOrigin>,
    returned_raw: std::collections::BTreeMap<usize, (usize, crate::document::WordSourceMap)>,
    raw_origins: Arc<std::collections::BTreeMap<usize, (usize, usize)>>,
    output: Arc<std::sync::Mutex<NormalizedOutputWork>>,
    fields: Arc<std::collections::HashMap<usize, TemplateScalarOrigin>>,
    receipts: Arc<std::sync::Mutex<std::collections::HashMap<usize, TemplateScalarOrigin>>>,
    scalar: Option<TemplateScalarBorrow>,
}

/// A selected operand plus the exact operation metadata anchor. This carries
/// no allowance and is only consumed at a real Word Text payload producer.
pub(crate) struct TemplateWordTextInput {
    origin: TemplateScalarOrigin,
    output: Arc<std::sync::Mutex<NormalizedOutputWork>>,
}

struct TemplateProducedText {
    run: usize,
    content: usize,
    origin: TemplateScalarOrigin,
}

struct NormalizedOutputWork {
    allowance: usize,
    emitted: usize,
    refused: bool,
    retained_revision: Option<RetainedRevisionRange>,
    new_field: bool,
    mapped_transport: Option<MappedFieldRange>,
    prepaid_xml: Option<RetainedRevisionRange>,
    // Only concrete final block/preservation publishers install this live role.
    // This is not an input identity or a Publication-wide exemption.
    final_validation_writer: Option<usize>,
    word_payload: Option<WordPayloadRange>,
    publication_field: bool,
}

pub(crate) struct PrepaidXmlTransport<'a> {
    _produced: &'a [u8],
    previous: Option<RetainedRevisionRange>,
    work: Arc<std::sync::Mutex<NormalizedOutputWork>>,
}

#[derive(Clone, Copy)]
struct RetainedRevisionRange {
    next: usize,
    end: usize,
}

/// Actual normalized XML bytes, distinct from unchanged captured transport.
pub(crate) struct NormalizedWordOutput {
    bytes: Vec<u8>,
    source_map: crate::document::WordSourceMap,
    local_limit: Option<usize>,
    field_only: bool,
    new_source_record: bool,
    final_writer: Option<usize>,
    word_capture: Option<Arc<std::sync::Mutex<TemplateWordCapture>>>,
    work: Arc<std::sync::Mutex<NormalizedOutputWork>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ParsedFieldSummary {
    affine: (usize, usize),
    maxima: (usize, usize),
}

/// Output work supplied only by an actual parent Field producer.
/// The metadata-only Arc does not own a model or source buffer.
pub(crate) struct FieldXmlEmission {
    enabled: bool,
    output: Arc<std::sync::Mutex<NormalizedOutputWork>>,
    word_capture: Option<Arc<std::sync::Mutex<TemplateWordCapture>>>,
}

// Actual sparse edits of one unchanged-cache source. The input bounds the
// number of reached End/DocType edits. Unchanged event gaps stay borrowed.
struct FieldXmlSourceRecord<'a> {
    original: &'a [u8],
    edits: Vec<FieldSourceEdit<'a>>,
}

/// One actual owned event buffer, reserved incrementally before each growth.
/// Ruby, controls and retained RPr attributes share this concrete producer.
pub(crate) struct FieldXmlStart<'a, 'name> {
    start: BytesStart<'name>,
    emission: Option<&'a FieldXmlEmission>,
    prepaid: bool,
    reserved: usize,
}

/// The model runs a physical field span was split into on read.
///
/// A run such as `Page {PAGE} of the report` holds text outside the field.
/// The reader turns it into sibling runs so every consumer sees that text,
/// and the writer puts the original bytes back while the runs are untouched.
#[derive(Debug, Clone)]
struct FieldRunSpan {
    /// The span's runs in order, `None` where a field of the span sits.
    runs: Vec<Option<CT_R>>,
    /// Where this field sits among `runs`.
    position: usize,
    /// Which field of the span this is, counted from the first.
    field_index: usize,
}

/// The bounded legacy form kind projected from `w:ffData`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyFormFieldKind {
    TextInput,
    CheckBox,
    DropDownList,
}

/// The current value stored by one legacy form field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyFormFieldValue {
    Text(String),
    Checked(bool),
    SelectedIndex(usize),
}

/// Supported metadata from one structurally valid `w:ffData` subtree.
///
/// Non-exhaustive: more `w:ffData` facts may join it without breaking
/// callers, who read it and never build it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LegacyFormFieldData {
    pub name: Option<String>,
    pub enabled: bool,
    pub calculate_on_exit: bool,
    pub kind: LegacyFormFieldKind,
    pub value: LegacyFormFieldValue,
    pub choices: Vec<String>,
    pub max_length: Option<usize>,
    /// A checkbox's drawn size, `None` for the other kinds.
    pub checkbox_size: Option<LegacyCheckBoxSize>,
    /// `w:helpText`, what Word shows on F1.
    pub help_text: Option<LegacyFormInfoText>,
    /// A text input's kind, `None` for regular text, stated or not, and for
    /// the other kinds.
    pub text_input_type: Option<LegacyTextInputType>,
}

/// Help or status text stored on a legacy form field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyFormInfoText {
    /// Literal help or status text. An empty string is stored.
    Text(String),
    /// An AutoText entry name.
    AutoText(String),
}

/// The six stored text-input kinds. This crate does not evaluate them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyTextInputType {
    Regular,
    Number,
    Date,
    CurrentTime,
    CurrentDate,
    Calculated,
}

impl LegacyTextInputType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Regular => "regular",
            Self::Number => "number",
            Self::Date => "date",
            Self::CurrentTime => "currentTime",
            Self::CurrentDate => "currentDate",
            Self::Calculated => "calculated",
        }
    }
}

/// Checkbox size for a newly authored legacy field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyCheckBoxSize {
    /// `w:sizeAuto`.
    Auto,
    /// Exact half-points from 0 through `u64::MAX`.
    HalfPoints(u64),
}

/// Shared legacy form options. Omitted help and status stay absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyFormFieldOptions {
    pub name: Option<String>,
    pub enabled: bool,
    pub calculate_on_exit: bool,
    pub help_text: Option<LegacyFormInfoText>,
    pub status_text: Option<LegacyFormInfoText>,
}

impl Default for LegacyFormFieldOptions {
    fn default() -> Self {
        Self {
            name: None,
            enabled: true,
            calculate_on_exit: false,
            help_text: None,
            status_text: None,
        }
    }
}

/// One complete legacy form definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyFormFieldDefinition {
    TextInput {
        input_type: LegacyTextInputType,
        default: Option<String>,
        value: String,
        max_length: Option<usize>,
        format: Option<String>,
    },
    CheckBox {
        size: LegacyCheckBoxSize,
        default: bool,
        checked: bool,
    },
    DropDownList {
        choices: Vec<String>,
        default_index: usize,
        selected_index: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LegacyFormAuthor {
    help_text: Option<LegacyFormInfoText>,
    status_text: Option<LegacyFormInfoText>,
    text_type: Option<LegacyTextInputType>,
    text_default: Option<String>,
    format: Option<String>,
    checkbox_size: Option<LegacyCheckBoxSize>,
    checkbox_default: Option<bool>,
    list_default: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NestedFieldPosition {
    Argument(usize),
    Switch(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldArgument {
    Text(String),
    Nested(Box<Field>),
    /// One operand made of several pieces in source order, each `Text` or
    /// `Nested`, at least one of them `Nested`: `"Mr { MERGEFIELD Name }"`
    /// or `{ MERGEFIELD Qty }*2`. Word reads it as the pieces' values joined.
    Joined(Vec<FieldArgument>),
}

impl FieldArgument {
    /// The fields nested in this operand, in source order.
    #[doc(hidden)]
    pub fn nested_fields(&self) -> Vec<&Field> {
        match self {
            Self::Text(_) => Vec::new(),
            Self::Nested(field) => vec![field.as_ref()],
            Self::Joined(pieces) => pieces.iter().flat_map(Self::nested_fields).collect(),
        }
    }

    /// The fields nested in this operand, in source order.
    #[doc(hidden)]
    pub fn nested_fields_mut(&mut self) -> Vec<&mut Field> {
        match self {
            Self::Text(_) => Vec::new(),
            Self::Nested(field) => vec![field.as_mut()],
            Self::Joined(pieces) => pieces
                .iter_mut()
                .flat_map(Self::nested_fields_mut)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldSwitch {
    pub name: String,
    pub argument: Option<FieldArgument>,
}

#[derive(Debug, Clone)]
enum FieldSource {
    New {
        original_instruction: FieldInstruction,
        form: FieldForm,
        cached_runs: Option<Vec<CT_R>>,
        original_cached_result: String,
        cached_segments: Vec<CachedDisplaySegment>,
    },
    Parsed {
        source_id: u64,
        owner_id: u64,
        form: FieldForm,
        raw_xml: Vec<u8>,
        publication_owned: Option<Vec<std::ops::Range<usize>>>,
        original_instruction: FieldInstruction,
        original_cached_result: String,
        cached_segments: Vec<CachedDisplaySegment>,
        original_dirty: Option<bool>,
        original_locked: Option<bool>,
        original_legacy_form: Option<LegacyFormFieldData>,
        original_result_published: bool,
        word_prefixes: Vec<String>,
    },
}

#[derive(Debug, Clone)]
struct CachedDisplaySegment {
    run_index: usize,
    text: String,
    properties: Option<CT_RPr>,
}

/// The representation used to serialize a Word field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldForm {
    /// One `w:fldSimple` element.
    Simple,
    /// Ordered begin, instruction, separate, result and end runs.
    Complex,
}

/// Read projection of a bookmark marker retained at a run boundary.
#[derive(Clone)]
pub struct BookmarkMarker {
    start: bool,
    id: Option<i32>,
    name: Option<String>,
    run_index: usize,
    raw_before: usize,
    projected_run_index: usize,
    tracked_run_index: usize,
    word_prefixes: Vec<String>,
    has_child_content: bool,
}

struct RunRawContext<'a, 'b> {
    foreign_word_namespace: Option<&'a str>,
    bindings: &'a [(String, String)],
    localized: &'a [(String, String)],
    source_context: Option<&'a [(&'a str, &'a str)]>,
    source_budget: Option<&'b mut usize>,
    emission: Option<&'a FieldXmlEmission>,
}

struct ParsedHyperlinkChildren {
    ordinary_runs: Vec<Option<crate::document::WordOrdinaryFact>>,
    runs: Vec<CT_R>,
    run_sources: Vec<Option<Vec<u8>>>,
    revisions: Vec<(usize, CT_Revision)>,
    extra_xml: Vec<(usize, usize, Vec<u8>)>,
    bookmark_markers: Vec<BookmarkMarker>,
    projected_run_count: usize,
    tracked_run_count: usize,
    run_maps: Vec<Option<Arc<crate::document::WordSourceMap>>>,
    text_facts: WordParagraphTextFacts,
    extra_maps: Vec<Option<Arc<crate::document::WordSourceMap>>>,
}

#[derive(Default)]
struct AcceptedBookmarkProjection {
    markers: Vec<BookmarkMarker>,
    projected_run_count: usize,
    tracked_run_count: usize,
}

struct WordProjectionSourceMaps {
    text_facts_enabled: bool,
    collect_properties: bool,
    ordinary_runs: Vec<Option<crate::document::WordOrdinaryFact>>,
    ordinary_controls: Vec<Vec<crate::document::WordOrdinaryFact>>,
    retained_control_slots: Option<Vec<usize>>,
    retained_revision_slots: Option<Vec<usize>>,
    texts: WordParagraphTextFacts,
    runs: Vec<Option<Arc<crate::document::WordSourceMap>>>,
    extras: Vec<Option<Arc<crate::document::WordSourceMap>>>,
    comments: Vec<Option<Arc<crate::document::WordSourceMap>>>,
    controls: Vec<Option<Arc<crate::document::WordSourceMap>>>,
    hyperlink_sources: Vec<Option<Arc<crate::document::WordSourceMap>>>,
    hyperlink_extras: Vec<Vec<Option<Arc<crate::document::WordSourceMap>>>>,
    registry: Arc<std::sync::Mutex<FieldOriginRegistry>>,
    initial_delivery: bool,
}

struct ComplexFieldProjection<'a> {
    source_maps: Option<&'a mut WordProjectionSourceMaps>,
    inline_complete_only: bool,
    legacy_consumed_owner: bool,
    run_coverage: Option<&'a mut Vec<std::ops::Range<usize>>>,
    runs: &'a mut Vec<CT_R>,
    run_sources: &'a mut Vec<Option<Vec<u8>>>,
    simple_field_summaries: &'a [(usize, ParsedFieldSummary)],
    extra_xml: &'a mut Vec<(usize, Vec<u8>)>,
    comment_ranges: &'a mut Vec<CommentRangeMarker>,
    comment_sources: &'a [(CommentRangeMarker, Vec<u8>)],
    control_sources: &'a [(usize, usize, usize, Vec<u8>)],
    hyperlink_sources: &'a [(usize, usize, Vec<u8>)],
    run_projection_ends: &'a [(usize, usize, usize)],
    bookmark_markers: &'a mut Vec<BookmarkMarker>,
    content_controls: &'a mut Vec<(usize, usize, usize, CT_Sdt)>,
    revisions: &'a mut Vec<(usize, usize, CT_Revision)>,
    hyperlinks: &'a mut Vec<HyperlinkSpan>,
    rubies: &'a mut Vec<CT_Ruby>,
    word_prefixes: &'a [String],
}

/// Where a run split puts the text-free content at its cut: tabs, breaks,
/// fields, drawings and raw children, which have no literal width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SplitEdge {
    /// With the text after the cut. A cut at either end of the run splits
    /// nothing.
    Plain,
    /// The cut starts a span: the text-free content before the span's first
    /// character stays in the first run, even at the start of the run.
    SpanStart,
    /// The cut ends a span: the text-free content after the span's last
    /// character goes to the second run, even at the end of the run.
    SpanEnd,
}

/// Where [`CT_R::split_point`] cuts a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunCut {
    /// Inside the text item at `index`, after `within` of its characters.
    Inside { index: usize, within: usize },
    /// Before the content item at `index`. The first run keeps the raw
    /// children positioned up to `head_raw` content items.
    Between { index: usize, head_raw: usize },
}

/// Why [`CT_P::split_run`] could not split a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RunSplitError {
    #[error("run index {run_index} is out of range for a paragraph with {run_count} runs")]
    RunOutOfRange { run_index: usize, run_count: usize },
    #[error("split offset {offset} exceeds a run's literal text length of {len} characters")]
    OffsetOutOfRange { offset: usize, len: usize },
    #[error("bookmark markers could not be projected after the split")]
    BookmarkProjection,
}

/// The range kind that [`CT_P::anchor_accepted_range`] writes.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeAnchor<'a> {
    /// Comment range markers, with the reference run right after the end.
    Comment(i32),
    /// Bookmark markers.
    Bookmark { id: i32, name: &'a str },
    /// Permission range with one selected editor or editor group.
    Permission {
        id: i32,
        editor: Option<&'a str>,
        group: Option<&'a str>,
    },
    /// Spelling or grammar proofing range, which has no numeric identifier.
    Proofing { kind: &'a str },
}

/// Why [`CT_P::anchor_accepted_range`] could not place a range exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RangeAnchorError {
    #[error("run boundary {boundary} exceeds the paragraph run count {run_count}")]
    OutOfRange { boundary: usize, run_count: usize },
    #[error("run range {start}..{end} ends before it starts")]
    Reversed { start: usize, end: usize },
    #[error("run range {start}..{end} crosses the edge of an inline content control")]
    CrossesControl { start: usize, end: usize },
    #[error(
        "run boundary {boundary} falls inside an inline content control, where a range that continues into another paragraph cannot start or end"
    )]
    InsideControl { boundary: usize },
    #[error("run boundary {boundary} falls inside a tracked insertion or move")]
    InsideRevision { boundary: usize },
    #[error("run boundary {boundary} sits next to a tracked change inside a hyperlink")]
    HyperlinkRevision { boundary: usize },
    #[error("range markers could not be written into the paragraph")]
    Write,
}

/// A run with nothing but the reference to comment `id`.
fn comment_reference_run(id: i32) -> CT_R {
    CT_R {
        properties: None,
        content: vec![RunContent::CommentReference { id, raw_before: 0 }],
        extra_xml: Vec::new(),
        extra_xml_positions: Vec::new(),
        alt_drawings: Vec::new(),
    }
}

/// One physical place for a range marker.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MarkerSite {
    /// A position among the paragraph-level children at a direct run boundary.
    Paragraph { boundary: usize, position: usize },
    /// A `w:sdtContent` child index in the inline control that `controls`
    /// reaches: a paragraph control index, then nested content indexes.
    Control { controls: Vec<usize>, index: usize },
}

/// A direct run boundary and a position among its paragraph-level children.
type ChildPosition = (usize, usize);

/// One paragraph-level child at a direct run boundary, in serialization order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundaryItem {
    Control(usize),
    Marker(usize),
    Raw(usize),
}

/// `CT_P` — A paragraph element containing runs and properties.
#[derive(Debug, Clone, PartialEq)]
#[allow(non_snake_case)]
pub struct CT_P {
    pub properties: Option<CT_PPr>,
    pub runs: Vec<CT_R>,
    /// Hyperlink spans referencing ranges of runs.
    pub hyperlinks: Vec<HyperlinkSpan>,
    /// Typed comment range boundaries at run insertion points.
    pub comment_ranges: Vec<CommentRangeMarker>,
    /// Typed projections of preserved bookmark markers.
    pub bookmark_markers: Vec<BookmarkMarker>,
    /// Unknown child elements captured as raw XML with their insertion position (run index).
    pub extra_xml: Vec<(usize, Vec<u8>)>,
    /// Typed run controls at
    /// `(run index, raw children before, comment markers before, control)`.
    pub content_controls: Vec<(usize, usize, usize, CT_Sdt)>,
    /// Read projections of revision wrappers retained at paragraph or hyperlink boundaries.
    pub revisions: Vec<(usize, usize, CT_Revision)>,
    /// Typed OfficeMath projections keyed by `(run boundary, raw child slot)`.
    pub equations: Vec<(usize, usize, OfficeMath)>,
    /// Ruby phonetic guides over half-open spans of [`Self::runs`].
    ///
    /// The base runs are ordinary paragraph runs, so text extraction, search
    /// and redaction see them without knowing about ruby. The phonetic runs
    /// stay inside the annotation, which is what keeps them out of every
    /// text projection.
    pub rubies: Vec<CT_Ruby>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AcceptedOwnerOrder {
    BeforeRaw,
    Raw(usize),
    AfterRaw,
}

/// One recursive step to a visible accepted-view run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AcceptedRunPathSegment {
    /// A direct run index in the current paragraph or content control.
    Run(usize),
    /// A content-control index in the current paragraph or content control.
    ContentControl(usize),
    /// A revision index in the current paragraph or content control.
    Revision(usize),
}

/// A checked recursive address for one visible accepted-view run.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AcceptedRunPath {
    pub(crate) segments: Vec<AcceptedRunPathSegment>,
}

impl AcceptedRunPath {
    /// Return the recursive source steps for this run.
    pub fn segments(&self) -> &[AcceptedRunPathSegment] {
        &self.segments
    }
}

/// A content control or a revision wrapper at a run boundary of a paragraph.
#[derive(Clone, Copy)]
pub(crate) enum BoundaryOwner {
    /// An index into [`CT_P::content_controls`].
    ContentControl(usize),
    /// An index into [`CT_P::revisions`].
    Revision(usize),
    /// An index into [`CT_P::extra_xml`] of a smart tag or an inline custom
    /// XML element, see [`run_wrapper_paragraph`].
    Wrapper(usize),
}

/// The content controls, revision wrappers, smart tags and inline custom
/// XML elements at run `boundary` of `paragraph`, in document order.
pub(crate) fn boundary_owners(paragraph: &CT_P, boundary: usize) -> Vec<BoundaryOwner> {
    ordered_boundary_owners(paragraph, boundary)
        .into_iter()
        .map(|(_, owner)| owner)
        .collect()
}

/// [`boundary_owners`] with the place each takes among the raw children of
/// the boundary.
fn ordered_boundary_owners(
    paragraph: &CT_P,
    boundary: usize,
) -> Vec<(AcceptedOwnerOrder, BoundaryOwner)> {
    let mut owners = paragraph
        .content_controls
        .iter()
        .enumerate()
        .filter(|(_, (at, _, _, _))| *at == boundary)
        .map(|(index, (_, raw_before, _, _))| {
            (
                AcceptedOwnerOrder::Raw(*raw_before),
                BoundaryOwner::ContentControl(index),
            )
        })
        .chain(
            paragraph
                .revisions
                .iter()
                .enumerate()
                .filter(|(_, (at, _, _))| *at == boundary)
                .map(|(index, (_, slot, _))| {
                    let order = if let Some(hyperlink_index) = hyperlink_revision_index(*slot) {
                        if let Some(raw_before) = paragraph
                            .hyperlinks
                            .get(hyperlink_index)
                            .and_then(|hyperlink| hyperlink.preserved_raw_before)
                        {
                            AcceptedOwnerOrder::Raw(raw_before)
                        } else if paragraph
                            .hyperlinks
                            .get(hyperlink_index)
                            .is_some_and(|hyperlink| boundary == hyperlink.run_end)
                        {
                            AcceptedOwnerOrder::BeforeRaw
                        } else {
                            AcceptedOwnerOrder::AfterRaw
                        }
                    } else {
                        AcceptedOwnerOrder::Raw(*slot)
                    };
                    (order, BoundaryOwner::Revision(index))
                }),
        )
        .chain(
            paragraph
                .extra_xml
                .iter()
                .enumerate()
                .filter(|(_, (at, _))| *at == boundary)
                .enumerate()
                .filter(|(_, (_, (_, raw)))| is_run_wrapper(raw))
                .map(|(slot, (index, _))| {
                    (AcceptedOwnerOrder::Raw(slot), BoundaryOwner::Wrapper(index))
                }),
        )
        .collect::<Vec<_>>();
    // A control goes before the raw child of the same slot.
    owners
        .sort_by_key(|(order, owner)| (*order, !matches!(owner, BoundaryOwner::ContentControl(_))));
    owners
}

/// Whether `raw` is a smart tag or an inline custom XML element with
/// content, read with the conventional `w` prefix and those it declares.
fn is_run_wrapper(raw: &[u8]) -> bool {
    let mut reader = Reader::from_reader(raw);
    let mut buffer = Vec::new();
    let Ok(Event::Start(start)) = reader.read_event_into(&mut buffer) else {
        return false;
    };
    let inherited = ["w".to_owned()];
    let Ok(prefixes) = word_prefixes_at(&start, &inherited) else {
        return false;
    };
    is_word_element(start.name().as_ref(), b"smartTag", &prefixes)
        || is_word_element(start.name().as_ref(), b"customXml", &prefixes)
}

/// The content of a smart tag, an inline custom XML element or a simple
/// field, parsed as a paragraph from its preserved source `raw` in the scope
/// of the Word prefixes `inherited`. Its runs are the text a reader sees
/// inside the wrapper. None for any other element, and for a wrapper whose
/// content [`with_run_wrapper_content`] could not write back, because `w`
/// does not name WordprocessingML there.
pub(crate) fn run_wrapper_paragraph(raw: &[u8], inherited: &[String]) -> Option<CT_P> {
    run_wrapper_paragraph_in(raw, inherited, None, None, None, None)
        .ok()
        .flatten()
}

fn wrapper_resource_failure(
    mapped: Option<&crate::document::WordFieldInput>,
    work: Option<&WordOperationWork>,
) -> Option<OxmlError> {
    mapped
        .and_then(crate::document::WordFieldInput::resource_failure)
        .or_else(|| {
            work.filter(|work| work.resource_refused()).map(|_| {
                OxmlError::InvalidValue(
                    "normalized result exceeds its preflight source work".into(),
                )
            })
        })
}

/// The same actual wrapper reader, with optional producer-owned correspondence.
/// The synthetic paragraph is ordinary transport, not normalization demand.
pub(crate) fn run_wrapper_paragraph_in(
    raw: &[u8],
    inherited: &[String],
    mapped: Option<&crate::document::WordFieldInput>,
    work: Option<&mut WordOperationWork>,
    extra_maps: Option<&mut WordExtraSourceMaps>,
    word_facts: Option<&mut Option<WordParagraphReadFacts>>,
) -> Result<Option<CT_P>> {
    if let Some(error) = wrapper_resource_failure(mapped, work.as_deref()) {
        return Err(error);
    }
    let mut produced_maps = Vec::new();
    let mut produced_word = None;
    let collect_properties =
        mapped.is_some_and(|input| input.projection_parts().0.has_publication());
    let mut produced_properties = Vec::new();
    let parsed = (|| -> Result<Option<CT_P>> {
        let mut reader = Reader::from_reader(raw);
        let mut buffer = Vec::new();
        let Event::Start(start) = reader.read_event_into(&mut buffer)? else {
            return Ok(None);
        };
        let prefixes = word_prefixes_at(&start, inherited)?;
        if !["smartTag", "customXml", "fldSimple"]
            .iter()
            .any(|local| is_word_element(start.name().as_ref(), local.as_bytes(), &prefixes))
        {
            return Ok(None);
        }
        let Some((content_start, content_end)) = run_wrapper_content_bounds(raw) else {
            return Ok(None);
        };
        let length = (content_end - content_start)
            .checked_add(11)
            .ok_or_else(|| {
                let error = OxmlError::InvalidValue("wrapper paragraph byte overflow".into());
                if let Some(mapped) = mapped {
                    mapped
                        .registry()
                        .lock()
                        .expect("field origin registry poisoned")
                        .record_source_resource_failure(&error);
                }
                error
            })?;
        let wrapped = mapped
            .map(|input| input.slice(content_start..content_end)?.with_shell(5, 6))
            .transpose()?;
        let mut paragraph_xml = Vec::with_capacity(length);
        paragraph_xml.extend_from_slice(b"<w:p>");
        paragraph_xml.extend_from_slice(&raw[content_start..content_end]);
        paragraph_xml.extend_from_slice(b"</w:p>");
        let mut reader = Reader::from_reader(paragraph_xml.as_slice());
        let mut buffer = Vec::new();
        let Event::Start(root) = reader.read_event_into(&mut buffer)? else {
            return Ok(None);
        };
        let mut cursor = wrapped
            .as_ref()
            .map(|input| input.cursor(&paragraph_xml))
            .transpose()?;
        let mut texts = Vec::new();
        let mut nested = WordBlockSourceMaps::default();
        let paragraph = CT_P::from_xml_with_read_output(
            &mut reader,
            &prefixes,
            Some(&root),
            cursor.as_mut(),
            WordParagraphReadOutput {
                extra_maps: extra_maps.as_ref().map(|_| &mut produced_maps),
                text_facts: word_facts.as_ref().map(|_| &mut texts),
                nested_maps: word_facts.as_ref().map(|_| &mut nested),
                ordinary: collect_properties.then_some(&mut produced_properties),
                ..Default::default()
            },
        )?;
        if word_facts.is_some()
            && let Some(wrapped) = &wrapped
        {
            produced_word = Some(WordParagraphReadFacts {
                source_map: Arc::new(wrapped.projection_parts().0.clone()),
                texts,
                nested,
            });
        }
        Ok(Some(paragraph))
    })();
    if let Some(error) = wrapper_resource_failure(mapped, work.as_deref()) {
        return Err(error);
    }
    match parsed {
        Ok(Some(paragraph)) => {
            if let Some(mapped) = mapped {
                mapped
                    .registry()
                    .lock()
                    .expect("field origin registry poisoned")
                    .commit_publication_owner(
                        crate::document::WordOwner::Paragraph(&paragraph),
                        mapped.projection_parts().0,
                    );
            }
            crate::document::WordOrdinaryFact::publish_all(produced_properties);
            if let Some(output) = extra_maps {
                *output = produced_maps;
            }
            if let Some(output) = word_facts {
                *output = produced_word;
            }
            Ok(Some(paragraph))
        }
        Ok(None) | Err(_) => Ok(None),
    }
}

/// The source `raw` of a wrapper read by run_wrapper_paragraph, with its
/// content written by the same substantive Source paragraph emitter.
pub(crate) fn with_run_wrapper_content(
    raw: &[u8],
    paragraph: &CT_P,
    source_context: Option<&[(&str, &str)]>,
) -> Result<Vec<u8>> {
    with_run_wrapper_content_in(raw, paragraph, source_context, None, None, None, None)
        .map(|(bytes, _)| bytes)
}

pub(crate) fn with_run_wrapper_content_in(
    raw: &[u8],
    paragraph: &CT_P,
    source_context: Option<&[(&str, &str)]>,
    mapped: Option<&crate::document::WordFieldInput>,
    mut work: Option<&mut WordOperationWork>,
    emission: Option<&FieldXmlEmission>,
    content_maps: Option<&WordExtraSourceMaps>,
) -> Result<(Vec<u8>, crate::document::WordSourceMap)> {
    let _source_scope = root_binding_scope(0);
    if let Some(error) = wrapper_resource_failure(mapped, work.as_deref()) {
        return Err(error);
    }
    let (content_start, content_end) = run_wrapper_content_bounds(raw)
        .ok_or_else(|| OxmlError::MissingElement("wrapper content".into()))?;
    let (mut content, content_map) = if let Some(work) = work.as_deref_mut() {
        paragraph_content_mapped_in(
            paragraph,
            source_context,
            work,
            None,
            emission,
            content_maps,
        )?
    } else {
        let mut writer = Writer::new(Vec::new());
        paragraph.write_xml_content(&mut writer, XmlWriteRole::Source, source_context)?;
        let content = writer.into_inner();
        let map = crate::document::WordSourceMap::generated(content.len());
        (content, map)
    };
    if let Some(error) = wrapper_resource_failure(mapped, work.as_deref()) {
        return Err(error);
    }
    let length = raw
        .len()
        .checked_sub(content_end - content_start)
        .and_then(|length| length.checked_add(content.len()))
        .ok_or_else(|| OxmlError::InvalidValue("wrapper content byte overflow".into()))?;
    let raw_map = mapped.map_or_else(
        || crate::document::WordSourceMap::generated(raw.len()),
        |input| input.projection_parts().0.clone(),
    );
    let map = crate::document::WordSourceMap::concat(&[
        raw_map.slice(0..content_start)?,
        content_map,
        raw_map.slice(content_end..raw.len())?,
    ])?;
    // The substantive paragraph writer already materialized this content.
    // Reuse that same buffer instead of copying it into a second whole record.
    // Only the actual inserted wrapper bytes are new producer demand. Retain
    // the completed content validation and prospective-length checks above.
    if emission.is_some()
        && let Some(work) = work
    {
        work.reserve_inner_xml(raw.len() - (content_end - content_start))?;
    }
    content.splice(0..0, raw[..content_start].iter().copied());
    content.extend_from_slice(&raw[content_end..]);
    debug_assert_eq!(content.len(), length);
    Ok((content, map))
}

/// The byte range of the content of the element `raw`, between the end of
/// its start tag and the start of its end tag.
fn run_wrapper_content_bounds(raw: &[u8]) -> Option<(usize, usize)> {
    let mut reader = Reader::from_reader(raw);
    let mut buffer = Vec::new();
    let Ok(Event::Start(_)) = reader.read_event_into(&mut buffer) else {
        return None;
    };
    let content_start = reader.buffer_position() as usize;
    let content_end = raw.iter().rposition(|byte| *byte == b'<')?;
    (content_start <= content_end).then_some((content_start, content_end))
}

/// The preserved source of a run that holds only an unchanged simple field,
/// with the Word prefixes it was read with.
pub(crate) fn simple_field_source(run: &CT_R) -> Option<(&[u8], &[String])> {
    let [RunContent::Field(field)] = run.content.as_slice() else {
        return None;
    };
    match field.source.as_ref() {
        FieldSource::Parsed {
            form: FieldForm::Simple,
            raw_xml,
            word_prefixes,
            ..
        } if field.is_unchanged() => Some((raw_xml, word_prefixes)),
        _ => None,
    }
}

/// Read the simple field of `run` again from the source `raw`, which
/// [`with_run_wrapper_content`] wrote. Returns false, and changes nothing,
/// when `run` holds no unchanged simple field or `raw` is not one.
pub(crate) fn set_simple_field_source(run: &mut CT_R, raw: &[u8]) -> Result<bool> {
    set_simple_field_source_in(run, raw, None, None)
}

pub(crate) fn set_simple_field_source_in(
    run: &mut CT_R,
    raw: &[u8],
    mapped: Option<&crate::document::WordFieldInput>,
    work: Option<&mut WordOperationWork>,
) -> Result<bool> {
    if let Some(error) = wrapper_resource_failure(mapped, work.as_deref()) {
        return Err(error);
    }
    let Some((_, prefixes)) = simple_field_source(run) else {
        return Ok(false);
    };
    let parsed = parse_simple_field(raw, prefixes, mapped);
    if let Some(error) = wrapper_resource_failure(mapped, work.as_deref()) {
        return Err(error);
    }
    let Some(field) = parsed? else {
        return Ok(false);
    };
    if let Some(mapped) = mapped {
        let [RunContent::Field(previous)] = run.content.as_slice() else {
            return Ok(false);
        };
        let mut registry = mapped
            .registry()
            .lock()
            .expect("field origin registry poisoned");
        let facts = registry.original_for(previous);
        registry.register_forwarded(&field, mapped.projection_parts().0.clone(), facts)?;
    }
    run.content = vec![RunContent::Field(field)];
    Ok(true)
}

/// Append the text of the accepted view of `paragraph` to `output`, see
/// [`CT_P::accepted_text`].
fn append_accepted_text(paragraph: &CT_P, output: &mut String) {
    append_accepted_text_keeping(paragraph, output, &mut |_| true);
}

/// The text a reader sees in `paragraph`, with the display of each field
/// `keep` refuses left out. `keep` sees every field a reader sees, simple
/// fields inside insertions and wrappers included, outermost first; the
/// fields nested in one are part of its display.
fn append_accepted_text_keeping(
    paragraph: &CT_P,
    output: &mut String,
    keep: &mut dyn FnMut(&Field) -> bool,
) {
    for boundary in 0..=paragraph.runs.len() {
        for owner in boundary_owners(paragraph, boundary) {
            let mut runs = Vec::new();
            match owner {
                BoundaryOwner::ContentControl(index) => {
                    append_accepted_control_runs(&paragraph.content_controls[index].3, &mut runs);
                }
                BoundaryOwner::Revision(index) => {
                    // The whole content of an insertion, simple fields and
                    // other wrappers included.
                    if let Some(content) = accepted_revision_content(&paragraph.revisions[index].2)
                    {
                        append_accepted_text_keeping(content, output, keep);
                    }
                }
                BoundaryOwner::Wrapper(index) => {
                    if let Some(content) =
                        run_wrapper_paragraph(&paragraph.extra_xml[index].1, &["w".to_owned()])
                    {
                        append_accepted_text_keeping(&content, output, keep);
                    }
                }
            }
            for run in runs {
                append_current_run_text(run, output, keep);
            }
        }
        if let Some(run) = paragraph.runs.get(boundary) {
            append_current_run_text(run, output, keep);
        }
    }
}

fn append_current_run_text(run: &CT_R, output: &mut String, keep: &mut dyn FnMut(&Field) -> bool) {
    if let [RunContent::Field(field)] = run.content.as_slice()
        && field.form() == FieldForm::Simple
        && (field.ordered_result.is_some() || simple_field_source(run).is_none())
    {
        if keep(field) {
            output.push_str(&field.reader_display());
        }
    } else if let Some((raw, prefixes)) = simple_field_source(run) {
        // Its source is read again as a wrapper of its result runs: the field
        // itself is the run's.
        if let [RunContent::Field(field)] = run.content.as_slice()
            && !keep(field)
        {
            return;
        }
        if let Some(paragraph) = run_wrapper_paragraph(raw, prefixes) {
            append_accepted_text_keeping(&paragraph, output, keep);
        }
    } else {
        // A field added to a run beside other content stays in that run until
        // a save writes it out on its own, as a `w:fldSimple`, or as a complex
        // field when its instruction nests one: its result is read in place,
        // as the saved file reads it.
        for content in &run.content {
            match content {
                RunContent::Field(field) => {
                    if keep(field) {
                        output.push_str(&field.reader_display());
                    }
                }
                other => output.push_str(CT_R::content_text(other)),
            }
        }
    }
}

struct CommentTextProjection {
    text: String,
    open: bool,
    start: Option<usize>,
    end: Option<usize>,
    start_offset: Option<usize>,
    end_offset: Option<usize>,
    markers: Vec<std::ops::Range<usize>>,
}

fn comment_projection_paragraph(
    xml: &[u8],
    runs: &[std::ops::Range<usize>],
    fields: &[std::ops::Range<usize>],
    window: std::ops::Range<usize>,
) -> Result<CT_P> {
    let kept = |span: &std::ops::Range<usize>| window.start <= span.start && span.end <= window.end;
    let mut removed = runs
        .iter()
        .filter(|span| !kept(span))
        .cloned()
        .collect::<Vec<_>>();
    removed.extend(
        fields
            .iter()
            .filter(|field| {
                !kept(field)
                    && !runs
                        .iter()
                        .any(|run| field.start <= run.start && run.end <= field.end && kept(run))
            })
            .cloned(),
    );
    removed.sort_by_key(|span| (span.start, std::cmp::Reverse(span.end)));
    let mut outer = Vec::<std::ops::Range<usize>>::new();
    for span in removed {
        if outer
            .last()
            .is_some_and(|previous| span.end <= previous.end)
        {
            continue;
        }
        outer.push(span);
    }
    let mut projected = xml.to_vec();
    for span in outer.into_iter().rev() {
        projected.drain(span);
    }
    CT_P::from_xml_fragment(&projected)
}

fn project_comment_text(xml: &[u8], id: i32, incoming: bool) -> Result<CommentTextProjection> {
    let invalid = |reason: &str| OxmlError::InvalidValue(format!("comment {id}: {reason}"));
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut stack = Vec::<(Vec<u8>, bool, usize, Option<usize>)>::new();
    let mut runs = Vec::new();
    let mut fields = Vec::new();
    let mut markers = Vec::<(bool, std::ops::Range<usize>)>::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader.read_resolved_event_into(&mut buffer)?;
        let word = matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == crate::namespace::W_NS.as_bytes()
            || std::str::from_utf8(uri).ok().and_then(|uri| quick_xml::escape::unescape(uri).ok()).is_some_and(|uri| uri == crate::namespace::W_NS));
        drop(namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let local = element.local_name().as_ref().to_vec();
                let mut marker = None;
                if word && matches!(local.as_slice(), b"commentRangeStart" | b"commentRangeEnd") {
                    let mut selected_id = None;
                    for attribute in element.attributes() {
                        let attribute = attribute?;
                        let (namespace, name) = reader.resolver().resolve_attribute(attribute.key);
                        if name.as_ref() == b"id"
                            && matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == crate::namespace::W_NS.as_bytes())
                        {
                            if selected_id.is_some() {
                                return Err(invalid("duplicate identity attribute"));
                            }
                            selected_id = Some(
                                attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        reader.decoder(),
                                    )?
                                    .parse::<i32>()
                                    .map_err(|_| invalid("invalid marker id"))?,
                            );
                        }
                    }
                    if selected_id == Some(id)
                        && !stack
                            .iter()
                            .any(|(name, word, _, _)| *word && name == b"txbxContent")
                    {
                        if stack.iter().any(|(name, word, _, _)| *word && name == b"r") {
                            return Err(invalid(
                                "existing accepted run axis cannot faithfully represent a mid-run source boundary",
                            ));
                        }
                        if stack.iter().any(|(name, word, _, _)| {
                            !*word
                                || !matches!(
                                    name.as_slice(),
                                    b"p" | b"sdt"
                                        | b"sdtContent"
                                        | b"hyperlink"
                                        | b"ins"
                                        // An edge inside tracked deleted or
                                        // moved-away text reads at the next
                                        // position of the accepted view, as
                                        // Google Docs writes a comment on a
                                        // tracked change.
                                        | b"del"
                                        | b"moveTo"
                                        | b"moveFrom"
                                        | b"smartTag"
                                        | b"customXml"
                                        | b"fldSimple"
                                )
                        }) {
                            return Err(invalid(
                                "selected source has an unsupported opaque owner outside the accepted paragraph view",
                            ));
                        }
                        markers.push((local == b"commentRangeStart", before..after));
                        marker = Some(markers.len() - 1);
                    }
                }
                if matches!(event, Event::Start(_)) {
                    stack.push((local, word, before, marker));
                } else if word && local == b"r" {
                    runs.push(before..after);
                } else if word && local == b"fldSimple" {
                    fields.push(before..after);
                }
            }
            Event::End(_) => {
                if let Some((name, word, start, marker)) = stack.pop() {
                    if let Some(index) = marker {
                        if !xml[markers[index].1.end..before]
                            .iter()
                            .all(u8::is_ascii_whitespace)
                        {
                            return Err(invalid(
                                "selected range marker has unsupported child content",
                            ));
                        }
                        markers[index].1.end = after;
                    }
                    if word && name == b"r" {
                        runs.push(start..after);
                    }
                    if word && name == b"fldSimple" {
                        fields.push(start..after);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let mut open = incoming;
    let mut start_span = None;
    let mut end_span = None;
    for (start, span) in &markers {
        if *start {
            if open || start_span.is_some() {
                return Err(invalid("duplicate or reversed selected start"));
            }
            start_span = Some(span.clone());
            open = true;
        } else {
            if !open || end_span.is_some() {
                return Err(invalid("unmatched or reversed selected end"));
            }
            end_span = Some(span.clone());
            open = false;
        }
    }
    let prefix = |at: usize| -> Result<(usize, usize)> {
        let p = comment_projection_paragraph(xml, &runs, &fields, 0..at)?;
        Ok((p.accepted_run_paths().len(), p.accepted_text().len()))
    };
    let start = start_span
        .as_ref()
        .map(|span| prefix(span.start))
        .transpose()?;
    let end = end_span
        .as_ref()
        .map(|span| prefix(span.start))
        .transpose()?;
    let text = if incoming || start_span.is_some() {
        comment_projection_paragraph(
            xml,
            &runs,
            &fields,
            start_span.as_ref().map_or(0, |span| span.end)
                ..end_span.as_ref().map_or(xml.len(), |span| span.start),
        )?
        .accepted_text()
    } else {
        String::new()
    };
    Ok(CommentTextProjection {
        text,
        open,
        start: start.map(|value| value.0),
        end: end.map(|value| value.0),
        start_offset: start.map(|value| value.1),
        end_offset: end.map(|value| value.1),
        markers: markers.into_iter().map(|(_, span)| span).collect(),
    })
}

fn accepted_paragraph_run_paths(paragraph: &CT_P) -> Vec<AcceptedRunPath> {
    let mut output = Vec::new();
    let mut prefix = Vec::new();
    append_accepted_paragraph_run_paths(paragraph, &mut prefix, &mut output);
    output
}

pub(crate) fn append_accepted_paragraph_run_paths(
    paragraph: &CT_P,
    prefix: &mut Vec<AcceptedRunPathSegment>,
    output: &mut Vec<AcceptedRunPath>,
) {
    for boundary in 0..=paragraph.runs.len() {
        for owner in boundary_owners(paragraph, boundary) {
            match owner {
                BoundaryOwner::ContentControl(index) => {
                    prefix.push(AcceptedRunPathSegment::ContentControl(index));
                    paragraph.content_controls[index]
                        .3
                        .append_accepted_run_paths(prefix, output);
                }
                BoundaryOwner::Revision(index) => {
                    prefix.push(AcceptedRunPathSegment::Revision(index));
                    paragraph.revisions[index]
                        .2
                        .append_accepted_run_paths(prefix, output);
                }
                BoundaryOwner::Wrapper(_) => continue,
            }
            prefix.pop();
        }
        if paragraph.runs.get(boundary).is_some() {
            prefix.push(AcceptedRunPathSegment::Run(boundary));
            output.push(AcceptedRunPath {
                segments: prefix.clone(),
            });
            prefix.pop();
        }
    }
}

fn accepted_paragraph_runs(paragraph: &CT_P) -> Vec<&CT_R> {
    let mut output = Vec::new();
    for boundary in 0..=paragraph.runs.len() {
        for owner in boundary_owners(paragraph, boundary) {
            match owner {
                BoundaryOwner::ContentControl(index) => {
                    append_accepted_control_runs(&paragraph.content_controls[index].3, &mut output);
                }
                BoundaryOwner::Revision(index) => {
                    append_accepted_revision_runs(&paragraph.revisions[index].2, &mut output);
                }
                BoundaryOwner::Wrapper(_) => {}
            }
        }
        if let Some(run) = paragraph.runs.get(boundary) {
            output.push(run);
        }
    }
    output
}

fn append_accepted_control_runs<'a>(control: &'a CT_Sdt, output: &mut Vec<&'a CT_R>) {
    for boundary in 0..=control.content.len() {
        for (_, revision) in control.revisions().iter().filter(|(at, _)| *at == boundary) {
            append_accepted_revision_runs(revision, output);
        }
        let Some(content) = control.content.get(boundary) else {
            continue;
        };
        match content {
            SdtContent::Run(run) => output.push(run),
            SdtContent::ContentControl(control) => append_accepted_control_runs(control, output),
            SdtContent::Paragraph(paragraph) => output.extend(accepted_paragraph_runs(paragraph)),
            SdtContent::Table(table) => append_accepted_table_runs(table, output),
            SdtContent::Row(row) => append_accepted_row_runs(row, output),
            SdtContent::Cell(cell) => append_accepted_cell_runs(cell, output),
            SdtContent::RawXml(_) => {}
        }
    }
}

fn append_accepted_table_runs<'a>(table: &'a CT_Tbl, output: &mut Vec<&'a CT_R>) {
    for boundary in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == boundary)
        {
            append_accepted_control_runs(control, output);
        }
        if let Some(row) = table.rows.get(boundary) {
            append_accepted_row_runs(row, output);
        }
    }
}

fn append_accepted_row_runs<'a>(row: &'a CT_Row, output: &mut Vec<&'a CT_R>) {
    for boundary in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == boundary)
        {
            append_accepted_control_runs(control, output);
        }
        if let Some(cell) = row.cells.get(boundary) {
            append_accepted_cell_runs(cell, output);
        }
    }
}

fn append_accepted_cell_runs<'a>(cell: &'a CT_Tc, output: &mut Vec<&'a CT_R>) {
    for content in &cell.content {
        match content {
            CellContent::Paragraph(paragraph) => output.extend(accepted_paragraph_runs(paragraph)),
            CellContent::Table(table) => append_accepted_table_runs(table, output),
            CellContent::ContentControl(control) => append_accepted_control_runs(control, output),
        }
    }
}

/// The runs of a tracked walk, and where each content control starts among
/// them when the caller asks for it.
#[derive(Default)]
struct TrackedRuns<'a> {
    runs: Vec<&'a CT_R>,
    starts: Option<Vec<(&'a CT_Sdt, usize)>>,
}

fn tracked_paragraph_runs(paragraph: &CT_P) -> Vec<&CT_R> {
    let mut output = TrackedRuns::default();
    walk_tracked_paragraph(paragraph, &mut output);
    output.runs
}

fn walk_tracked_paragraph<'a>(paragraph: &'a CT_P, output: &mut TrackedRuns<'a>) {
    for boundary in 0..=paragraph.runs.len() {
        for owner in boundary_owners(paragraph, boundary) {
            match owner {
                BoundaryOwner::ContentControl(index) => {
                    walk_tracked_control(&paragraph.content_controls[index].3, output);
                }
                BoundaryOwner::Revision(index) => {
                    walk_tracked_revision(&paragraph.revisions[index].2, output);
                }
                BoundaryOwner::Wrapper(_) => {}
            }
        }
        if let Some(run) = paragraph.runs.get(boundary) {
            output.runs.push(run);
        }
    }
}

fn walk_tracked_control<'a>(control: &'a CT_Sdt, output: &mut TrackedRuns<'a>) {
    if let Some(starts) = output.starts.as_mut() {
        starts.push((control, output.runs.len()));
    }
    for boundary in 0..=control.content.len() {
        for (_, revision) in control.revisions().iter().filter(|(at, _)| *at == boundary) {
            walk_tracked_revision(revision, output);
        }
        let Some(content) = control.content.get(boundary) else {
            continue;
        };
        match content {
            SdtContent::Run(run) => output.runs.push(run),
            SdtContent::ContentControl(control) => walk_tracked_control(control, output),
            SdtContent::Paragraph(paragraph) => walk_tracked_paragraph(paragraph, output),
            SdtContent::Table(table) => walk_tracked_table(table, output),
            SdtContent::Row(row) => walk_tracked_row(row, output),
            SdtContent::Cell(cell) => walk_tracked_cell(cell, output),
            SdtContent::RawXml(_) => {}
        }
    }
}

fn walk_tracked_revision<'a>(revision: &'a CT_Revision, output: &mut TrackedRuns<'a>) {
    if let Some(paragraph) = revision.content_paragraph() {
        walk_tracked_paragraph(paragraph, output);
        return;
    }
    let crate::revision::RevisionContent::Runs(runs) = revision.content() else {
        return;
    };
    for boundary in 0..=runs.len() {
        for (_, nested) in revision
            .nested_revisions()
            .iter()
            .filter(|(at, _)| *at == boundary)
        {
            walk_tracked_revision(nested, output);
        }
        if let Some(run) = runs.get(boundary) {
            output.runs.push(run);
        }
    }
}

fn append_tracked_revision_runs<'a>(revision: &'a CT_Revision, output: &mut Vec<&'a CT_R>) {
    let mut walk = TrackedRuns::default();
    walk_tracked_revision(revision, &mut walk);
    output.append(&mut walk.runs);
}

fn walk_tracked_table<'a>(table: &'a CT_Tbl, output: &mut TrackedRuns<'a>) {
    for boundary in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == boundary)
        {
            walk_tracked_control(control, output);
        }
        if let Some(row) = table.rows.get(boundary) {
            walk_tracked_row(row, output);
        }
    }
}

fn walk_tracked_row<'a>(row: &'a CT_Row, output: &mut TrackedRuns<'a>) {
    for boundary in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == boundary)
        {
            walk_tracked_control(control, output);
        }
        if let Some(cell) = row.cells.get(boundary) {
            walk_tracked_cell(cell, output);
        }
    }
}

fn walk_tracked_cell<'a>(cell: &'a CT_Tc, output: &mut TrackedRuns<'a>) {
    for content in &cell.content {
        match content {
            CellContent::Paragraph(paragraph) => walk_tracked_paragraph(paragraph, output),
            CellContent::Table(table) => walk_tracked_table(table, output),
            CellContent::ContentControl(control) => walk_tracked_control(control, output),
        }
    }
}

/// Append the accepted view of `paragraph` with its simple-field and
/// wrapper boundaries (Markdown keeps them apart from the typed runs),
/// return its equations and push the bookmarks that start in it on
/// `bookmarks`, all shifted to their place in `runs`.
fn append_accepted_inline_paragraph(
    paragraph: &CT_P,
    runs: &mut Vec<CT_R>,
    boundaries: &mut Vec<usize>,
    bookmarks: &mut Vec<(usize, String)>,
) -> Vec<(usize, usize, OfficeMath)> {
    let projection = paragraph.accepted_view_projection(true);
    let offset = runs.len();
    bookmarks.extend(shifted_bookmarks(projection.bookmark_starts, offset));
    let inner = projection.field_boundaries;
    let view = projection.paragraph.into_owned();
    let equations = shifted_equations(&view, offset).collect();
    if !view.runs.is_empty() {
        boundaries.push(offset);
        boundaries.extend(inner.into_iter().map(|at| offset + at));
        boundaries.push(offset + view.runs.len());
        runs.extend(view.runs);
    }
    equations
}

/// Append the accepted view of an inline content control: its runs, with
/// their simple-field and wrapper boundaries, and the equations and bookmarks
/// that start in it, all counted on the runs appended, so that an export
/// writes each at its place among them.
fn append_accepted_inline_control(
    control: &CT_Sdt,
    runs: &mut Vec<CT_R>,
    boundaries: &mut Vec<usize>,
    equations: &mut Vec<(usize, usize, OfficeMath)>,
    bookmarks: &mut Vec<(usize, String)>,
) {
    let start = runs.len();
    let boundary_start = boundaries.len();
    boundaries.push(start);
    let mut bindings = None;
    for boundary in 0..=control.content.len() {
        for (_, revision) in control.revisions().iter().filter(|(at, _)| *at == boundary) {
            if let Some(paragraph) = accepted_revision_content(revision) {
                equations.extend(append_accepted_inline_paragraph(
                    paragraph, runs, boundaries, bookmarks,
                ));
            }
        }
        let Some(content) = control.content.get(boundary) else {
            continue;
        };
        bookmarks.extend(shifted_bookmarks(
            sdt_content_bookmark_starts(content, control.word_prefixes()),
            runs.len(),
        ));
        match content {
            SdtContent::Run(run) => runs.push(run.clone()),
            SdtContent::ContentControl(nested) => {
                append_accepted_inline_control(nested, runs, boundaries, equations, bookmarks)
            }
            SdtContent::Paragraph(paragraph) => {
                equations.extend(append_accepted_inline_paragraph(
                    paragraph, runs, boundaries, bookmarks,
                ));
            }
            // Block carriers are emitted separately by Markdown. Retain the
            // accepted typed projection if encountered in a synthetic inline model.
            SdtContent::Table(table) => {
                let mut owned = Vec::new();
                append_accepted_table_runs(table, &mut owned);
                boundaries.push(runs.len());
                runs.extend(owned.into_iter().cloned());
                boundaries.push(runs.len());
            }
            SdtContent::Row(row) => {
                let mut owned = Vec::new();
                append_accepted_row_runs(row, &mut owned);
                boundaries.push(runs.len());
                runs.extend(owned.into_iter().cloned());
                boundaries.push(runs.len());
            }
            SdtContent::Cell(cell) => {
                let mut owned = Vec::new();
                append_accepted_cell_runs(cell, &mut owned);
                boundaries.push(runs.len());
                runs.extend(owned.into_iter().cloned());
                boundaries.push(runs.len());
            }
            // An equation in an inline control is read as a paragraph's.
            SdtContent::RawXml(raw) => {
                let bindings = bindings.get_or_insert_with(|| {
                    crate::numbering::namespace_bindings(control.word_prefixes())
                });
                if let Ok(Some(equation)) = OfficeMath::from_raw(raw, bindings) {
                    equations.push((runs.len(), 0, equation));
                }
            }
        }
    }
    if runs.len() == start {
        boundaries.truncate(boundary_start);
    } else {
        boundaries.push(runs.len());
    }
}

/// The content of an insertion or a move destination, which the accepted
/// view keeps, simple fields included; a deletion or a move source has none.
fn accepted_revision_content(revision: &CT_Revision) -> Option<&CT_P> {
    revision.accepted_content()
}

fn append_accepted_revision_runs<'a>(revision: &'a CT_Revision, output: &mut Vec<&'a CT_R>) {
    if let Some(paragraph) = accepted_revision_content(revision) {
        output.extend(accepted_paragraph_runs(paragraph));
    }
}

/// The bookmarks of a nested accepted view, at `offset` in the outer one.
fn shifted_bookmarks(
    bookmarks: Vec<(usize, String)>,
    offset: usize,
) -> impl Iterator<Item = (usize, String)> {
    bookmarks
        .into_iter()
        .map(move |(at, name)| (offset + at, name))
}

/// The first start of each bookmark name. The paragraph's markers also list
/// those of its content controls and tracked insertions, at the owner's
/// place: the owner, read before them, gives the start its exact place.
fn first_of_each_name(starts: Vec<(usize, String)>) -> Vec<(usize, String)> {
    let mut seen = HashSet::new();
    starts
        .into_iter()
        .filter(|(_, name)| seen.insert(name.clone()))
        .collect()
}

/// A paragraph's accepted view, as [`CT_P::accepted_view_with_bookmarks`]
/// gives it.
#[doc(hidden)]
pub struct AcceptedView<'a> {
    /// The view: plain runs, hyperlink spans and equations.
    pub paragraph: Cow<'a, CT_P>,
    /// The run boundaries of its simple fields and wrappers, when asked.
    pub field_boundaries: Vec<usize>,
    /// The bookmarks that start in it, by run boundary in the view.
    pub bookmark_starts: Vec<(usize, String)>,
}

/// One thing at a run boundary of the accepted view, in document order.
enum ViewStep<'a> {
    Owner(BoundaryOwner),
    Equation(usize, &'a OfficeMath),
    BookmarkStart(&'a str),
}

/// The equations of a nested accepted view, at `offset` in the outer one.
fn shifted_equations(
    view: &CT_P,
    offset: usize,
) -> impl Iterator<Item = (usize, usize, OfficeMath)> + '_ {
    view.equations
        .iter()
        .map(move |(at, slot, equation)| (offset + at, *slot, equation.clone()))
}

/// The scope a smart tag or inline custom XML element is read in when its
/// own ancestors are not at hand: `w`, and `m` for the equations it holds,
/// which Word declares on the document root.
fn wrapper_scope() -> [String; 2] {
    [
        "w".to_owned(),
        crate::numbering::namespace_binding("m", crate::namespace::M_NS),
    ]
}

#[allow(non_snake_case)]
impl CT_P {
    pub fn new() -> Self {
        CT_P {
            properties: None,
            runs: Vec::new(),
            hyperlinks: Vec::new(),
            comment_ranges: Vec::new(),
            bookmark_markers: Vec::new(),
            extra_xml: Vec::new(),
            content_controls: Vec::new(),
            revisions: Vec::new(),
            equations: Vec::new(),
            rubies: Vec::new(),
        }
    }

    /// Report whether one raw paragraph carrier retains root attributes.
    #[doc(hidden)]
    pub fn raw_is_root_attributes(position: usize, raw: &[u8]) -> bool {
        position == ROOT_ATTRIBUTES_POSITION && is_root_attribute_record(raw)
    }

    pub(crate) fn from_empty_root(root: &BytesStart<'_>, word_prefixes: &[String]) -> Result<Self> {
        let mut paragraph = Self::new();
        if let Some(record) = capture_root_attribute_record(root, word_prefixes)? {
            paragraph.extra_xml.push((ROOT_ATTRIBUTES_POSITION, record));
        }
        Ok(paragraph)
    }

    /// Get the combined raw text of [`Self::runs`], each read by `CT_R::text`.
    ///
    /// It leaves out simple field results and tracked insertions.
    /// [`Self::accepted_text`] reads the text a reader of the document sees.
    pub fn text(&self) -> String {
        self.runs().iter().map(|run| run.text()).collect()
    }

    /// Return valid complex `HYPERLINK` fields projected into synthetic runs:
    /// those with an address, a bookmark (`\l`), or both.
    pub fn complex_field_hyperlinks(&self) -> Vec<ComplexFieldHyperlink> {
        self.runs
            .iter()
            .enumerate()
            .filter_map(|(run_index, run)| {
                let [RunContent::Field(field)] = run.content.as_slice() else {
                    return None;
                };
                if !field.is_parsed_complex()
                    || field.dirty == Some(true)
                    || field.cached_result.is_empty()
                    || field.instruction.name != "HYPERLINK"
                {
                    return None;
                }
                let target = field.instruction.arguments.iter().find_map(|argument| {
                    let FieldArgument::Text(target) = argument else {
                        return None;
                    };
                    (!target.is_empty()).then(|| target.clone())
                });
                let anchor = field.instruction.switches.iter().find_map(|switch| {
                    let Some(FieldArgument::Text(anchor)) = &switch.argument else {
                        return None;
                    };
                    (switch.name == "l" && !anchor.is_empty()).then(|| anchor.clone())
                });
                (target.is_some() || anchor.is_some()).then_some(ComplexFieldHyperlink {
                    run_start: run_index,
                    run_end: run_index + 1,
                    target,
                    anchor,
                })
            })
            .collect()
    }

    /// Return direct and content-control-wrapped runs in document order.
    pub fn runs(&self) -> Vec<&CT_R> {
        let mut runs = Vec::new();
        self.collect_runs(&mut runs);
        runs
    }

    /// Borrow typed source runs in physical order, including revision owners.
    /// Deleted runs reserve identity but selection remains the caller's job.
    /// Opaque wrapper XML does not become a typed source.
    #[doc(hidden)]
    pub fn source_runs(&self) -> Vec<&CT_R> {
        tracked_paragraph_runs(self)
    }

    /// Where each content control starts among [`Self::source_runs`]: the
    /// number of source runs before it, nested controls included, in the
    /// order the walk meets them. A control with no run starts where the
    /// next run would sit, so it still has a place.
    #[doc(hidden)]
    pub fn source_control_starts(&self) -> Vec<(&CT_Sdt, usize)> {
        let mut output = TrackedRuns {
            runs: Vec::new(),
            starts: Some(Vec::new()),
        };
        walk_tracked_paragraph(self, &mut output);
        output.starts.unwrap_or_default()
    }

    /// Return the text of the accepted view, as `Paragraph::text` reads it:
    /// the text of [`Self::accepted_bookmark_runs`], and that of the runs
    /// inside smart tags, inline custom XML elements and simple fields, in
    /// document order, nested ones included. The runs of a wrapper inside a
    /// content control, a revision or a hyperlink are not read. A legacy
    /// drop-down that stores no result reads its selected entry, as Word
    /// shows it ([`Field::reader_display`]).
    #[doc(hidden)]
    pub fn accepted_text(&self) -> String {
        let mut text = String::new();
        append_accepted_text(self, &mut text);
        text
    }

    /// Return the text of hyperlink `index` as a reader sees it: its runs
    /// with their field results, and the runs of the tracked insertions inside
    /// it, in document order. Tracked deletions are left out.
    #[doc(hidden)]
    pub fn accepted_hyperlink_text(&self, index: usize) -> String {
        let mut text = String::new();
        let Some(hyperlink) = self.hyperlinks.get(index) else {
            return text;
        };
        let start = hyperlink.run_start.min(self.runs.len());
        let end = hyperlink.run_end.clamp(start, self.runs.len());
        // This hyperlink's revisions, taken once rather than at each boundary.
        let revisions = self
            .revisions
            .iter()
            .filter(|(_, slot, _)| hyperlink_revision_index(*slot) == Some(index))
            .collect::<Vec<_>>();
        for boundary in start..=end {
            for (_, _, revision) in revisions.iter().filter(|(at, _, _)| *at == boundary) {
                if let Some(content) = accepted_revision_content(revision) {
                    append_accepted_text(content, &mut text);
                }
            }
            if boundary < end {
                append_current_run_text(&self.runs[boundary], &mut text, &mut |_| true);
            }
        }
        text
    }

    /// Project one selected comment span through the existing accepted text and run axis.
    /// Returns display, continuation state and optional start/end run boundaries.
    /// Original source is untouched. Unrepresentable source endpoints are errors.
    #[doc(hidden)]
    pub fn accepted_comment_projection(
        &self,
        id: i32,
        open_at_start: bool,
    ) -> Result<(String, bool, Option<usize>, Option<usize>)> {
        if self.comment_ranges.iter().any(|marker| match marker {
            CommentRangeMarker::Start {
                id: marker_id,
                has_child_content,
                ..
            }
            | CommentRangeMarker::End {
                id: marker_id,
                has_child_content,
                ..
            } => *marker_id == id && *has_child_content,
        }) {
            return Err(OxmlError::InvalidValue(format!(
                "comment {id}: selected range marker has unsupported child content"
            )));
        }
        let mut raw = Vec::new();
        self.to_xml(&mut Writer::new(&mut raw))?;
        Self::accepted_comment_source_projection(&raw, id, open_at_start)
    }

    /// Project a comment from its namespace-closed original paragraph source.
    /// Retain its exact inherited bindings through the private fidelity probe.
    /// The accepted text, endpoint axis and unsupported-source checks are the
    /// same as [`Self::accepted_comment_projection`].
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // Concrete text, open state and two accepted endpoints.
    pub fn accepted_comment_source_projection(
        xml: &[u8],
        id: i32,
        open_at_start: bool,
    ) -> Result<(String, bool, Option<usize>, Option<usize>)> {
        let raw = raw_with_external_bindings(
            xml,
            &[("w".to_owned(), crate::namespace::W_NS.to_owned())],
        )?;
        let mut root_reader = Reader::from_reader(raw.as_slice());
        let bindings = loop {
            match root_reader.read_event()? {
                Event::Start(element) | Event::Empty(element) => {
                    break raw_namespace_declarations(&element)?;
                }
                Event::Eof => {
                    return Err(OxmlError::MissingElement("comment paragraph root".into()));
                }
                _ => {}
            }
        };
        let selected = project_comment_text(&raw, id, open_at_start)?;
        if !selected.markers.is_empty() {
            // Remove only this selected pair before asking the existing checked
            // writer to place the same endpoints. No temporary identity is allocated.
            let mut unmarked = raw;
            for span in selected.markers.iter().rev() {
                unmarked.drain(span.clone());
            }
            let unfaithful = |detail: String| {
                OxmlError::InvalidValue(format!(
                    "comment {id}: existing accepted run axis cannot faithfully represent selected source{detail}"
                ))
            };
            // Place these edges with the checked writer and read them back.
            let place = |start: Option<usize>,
                         end: Option<usize>,
                         incoming: bool|
             -> Result<
                std::result::Result<CommentTextProjection, RangeAnchorError>,
            > {
                let mut probe = CT_P::from_xml_fragment(&unmarked)?;
                if let Err(error) =
                    probe.anchor_accepted_range(start, end, RangeAnchor::Comment(id))
                {
                    return Ok(Err(error));
                }
                let mut raw = Vec::new();
                probe.to_xml(&mut Writer::new(&mut raw))?;
                let raw = raw_with_external_bindings(&raw, &bindings)?;
                let raw = raw_with_external_bindings(
                    &raw,
                    &[("w".to_owned(), crate::namespace::W_NS.to_owned())],
                )?;
                Ok(Ok(project_comment_text(&raw, id, incoming)?))
            };
            match place(selected.start, selected.end, open_at_start)? {
                Ok(placed) => {
                    if selected.text != placed.text
                        || selected.start_offset != placed.start_offset
                        || selected.end_offset != placed.end_offset
                    {
                        return Err(unfaithful(String::new()));
                    }
                }
                // An edge the writer cannot place inside a tracked insertion
                // or move (one inside a control, or a wrapper it cannot write
                // back) is already exact when read: the accepted view holds
                // the inserted text. The other edge is still checked on its
                // own.
                Err(RangeAnchorError::InsideRevision { .. }) => {
                    // Each edge alone, as a collapsed range in this paragraph.
                    for (edge, is_start) in [(selected.start, true), (selected.end, false)] {
                        let Some(edge) = edge else {
                            continue;
                        };
                        match place(Some(edge), Some(edge), false)? {
                            Ok(placed) => {
                                let (read, wanted) = if is_start {
                                    (placed.start_offset, selected.start_offset)
                                } else {
                                    (placed.end_offset, selected.end_offset)
                                };
                                if read != wanted {
                                    return Err(unfaithful(String::new()));
                                }
                            }
                            Err(RangeAnchorError::InsideRevision { .. }) => {}
                            Err(error) => return Err(unfaithful(format!(": {error}"))),
                        }
                    }
                }
                Err(error) => return Err(unfaithful(format!(": {error}"))),
            }
        }
        Ok((selected.text, selected.open, selected.start, selected.end))
    }

    /// Return the accepted view as a paragraph of plain runs, for the
    /// exporters: the runs whose text [`Self::accepted_text`] reads, in
    /// document order. Those of content controls, tracked insertions and
    /// moves in, smart tags, inline custom XML elements and simple fields
    /// are included, deleted and moved-away runs are left out, and the
    /// hyperlink spans cover the same runs as in the source, and the
    /// equations of the paragraph, of its insertions and of its inline
    /// controls stay at their run boundary. Content
    /// controls, revisions, wrappers and markers are not kept. Borrowed when
    /// the paragraph holds none of them, since its runs are then the view.
    #[doc(hidden)]
    pub fn accepted_view(&self) -> Cow<'_, CT_P> {
        self.accepted_view_projection(false).paragraph
    }

    /// Read accepted content while retaining physical simple-field boundaries.
    /// Markdown uses these boundaries to keep independent formatting spans.
    #[doc(hidden)]
    pub fn accepted_view_with_field_owners(&self) -> (Cow<'_, CT_P>, Vec<usize>) {
        let view = self.accepted_view_projection(true);
        (view.paragraph, view.field_boundaries)
    }

    /// [`Self::accepted_view`] (or, with `preserve_field_owners`,
    /// [`Self::accepted_view_with_field_owners`]) with the bookmarks that
    /// start in it, each at its run boundary in the view: those of the
    /// paragraph, its content controls, tracked insertions, smart tags,
    /// custom XML elements and simple fields. The exports write their
    /// anchors there.
    #[doc(hidden)]
    pub fn accepted_view_with_bookmarks(&self, preserve_field_owners: bool) -> AcceptedView<'_> {
        self.accepted_view_projection(preserve_field_owners)
    }

    fn accepted_view_projection(&self, preserve_field_owners: bool) -> AcceptedView<'_> {
        if self.content_controls.is_empty()
            && self.revisions.is_empty()
            && !self.extra_xml.iter().any(|(_, raw)| is_run_wrapper(raw))
            && !self
                .runs
                .iter()
                .any(|run| simple_field_source(run).is_some())
        {
            // The runs are the view, so a marker's run is its boundary.
            let bookmark_starts = self
                .bookmark_markers
                .iter()
                .filter(|marker| marker.is_start())
                .filter_map(|marker| Some((marker.run_index(), marker.name()?.to_owned())))
                .collect();
            return AcceptedView {
                paragraph: Cow::Borrowed(self),
                field_boundaries: Vec::new(),
                bookmark_starts,
            };
        }
        let mut runs = Vec::new();
        let mut inline_boundaries = Vec::new();
        // The index into `self.hyperlinks` of the hyperlink each run is in.
        let mut links = Vec::new();
        // Equations stay at their place among the projected runs.
        let mut equations = Vec::new();
        let mut bookmark_starts = Vec::new();
        for boundary in 0..=self.runs.len() {
            // An equation or a bookmark marker is the raw child of its slot,
            // after the owners that go before that child.
            let mut steps = ordered_boundary_owners(self, boundary)
                .into_iter()
                .map(|(order, owner)| {
                    let rank = usize::from(!matches!(owner, BoundaryOwner::ContentControl(_)));
                    ((order, rank), ViewStep::Owner(owner))
                })
                .chain(
                    self.equations
                        .iter()
                        .filter(|(at, _, _)| *at == boundary)
                        .map(|(_, slot, equation)| {
                            (
                                (AcceptedOwnerOrder::Raw(*slot), 2),
                                ViewStep::Equation(*slot, equation),
                            )
                        }),
                )
                .chain(
                    self.bookmark_markers
                        .iter()
                        .filter(|marker| marker.run_index() == boundary && marker.is_start())
                        .filter_map(|marker| {
                            Some((
                                (AcceptedOwnerOrder::Raw(marker.raw_before()), 1),
                                ViewStep::BookmarkStart(marker.name()?),
                            ))
                        }),
                )
                .collect::<Vec<_>>();
            steps.sort_by_key(|(order, _)| *order);
            for (_, step) in steps {
                let owner = match step {
                    ViewStep::Equation(slot, equation) => {
                        equations.push((runs.len(), slot, equation.clone()));
                        continue;
                    }
                    ViewStep::BookmarkStart(name) => {
                        bookmark_starts.push((runs.len(), name.to_owned()));
                        continue;
                    }
                    ViewStep::Owner(owner) => owner,
                };
                match owner {
                    BoundaryOwner::ContentControl(index) => {
                        let offset = runs.len();
                        let mut boundaries = Vec::new();
                        append_accepted_inline_control(
                            &self.content_controls[index].3,
                            &mut runs,
                            if preserve_field_owners {
                                &mut inline_boundaries
                            } else {
                                &mut boundaries
                            },
                            &mut equations,
                            &mut bookmark_starts,
                        );
                        links.extend(std::iter::repeat_n(None, runs.len() - offset));
                    }
                    BoundaryOwner::Revision(index) => {
                        let (_, slot, revision) = &self.revisions[index];
                        let offset = runs.len();
                        // The runs and equations of an insertion's own accepted
                        // view, wrappers and simple fields read as everywhere.
                        if let Some(content) = accepted_revision_content(revision) {
                            if preserve_field_owners {
                                equations.extend(append_accepted_inline_paragraph(
                                    content,
                                    &mut runs,
                                    &mut inline_boundaries,
                                    &mut bookmark_starts,
                                ));
                            } else {
                                let projection = content.accepted_view_projection(false);
                                bookmark_starts
                                    .extend(shifted_bookmarks(projection.bookmark_starts, offset));
                                let view = projection.paragraph.into_owned();
                                equations.extend(shifted_equations(&view, offset));
                                runs.extend(view.runs);
                            }
                        }
                        links.extend(std::iter::repeat_n(
                            hyperlink_revision_index(*slot),
                            runs.len() - offset,
                        ));
                    }
                    BoundaryOwner::Wrapper(index) => {
                        if let Some(content) =
                            run_wrapper_paragraph(&self.extra_xml[index].1, &wrapper_scope())
                        {
                            let projection =
                                content.accepted_view_projection(preserve_field_owners);
                            let offset = runs.len();
                            bookmark_starts
                                .extend(shifted_bookmarks(projection.bookmark_starts, offset));
                            let boundaries = projection.field_boundaries;
                            let view = projection.paragraph.into_owned();
                            if preserve_field_owners && !view.runs.is_empty() {
                                inline_boundaries.push(offset);
                                inline_boundaries
                                    .extend(boundaries.into_iter().map(|at| offset + at));
                                inline_boundaries.push(offset + view.runs.len());
                            }
                            links.extend(std::iter::repeat_n(None, view.runs.len()));
                            equations.extend(shifted_equations(&view, offset));
                            runs.extend(view.runs);
                        }
                    }
                }
            }
            if let Some(run) = self.runs.get(boundary) {
                let link = self.hyperlinks.iter().position(|hyperlink| {
                    hyperlink.run_start <= boundary && boundary < hyperlink.run_end
                });
                // A simple field reads as the runs of its result, as in
                // `Self::accepted_text`, so a revision inside it is resolved.
                let field = simple_field_source(run)
                    .and_then(|(raw, prefixes)| run_wrapper_paragraph(raw, prefixes));
                match field {
                    Some(content) => {
                        let projection = content.accepted_view_projection(preserve_field_owners);
                        let offset = runs.len();
                        bookmark_starts
                            .extend(shifted_bookmarks(projection.bookmark_starts, offset));
                        let boundaries = projection.field_boundaries;
                        let view = projection.paragraph.into_owned();
                        if preserve_field_owners && !view.runs.is_empty() {
                            inline_boundaries.push(offset);
                            inline_boundaries.extend(boundaries.into_iter().map(|at| offset + at));
                            inline_boundaries.push(offset + view.runs.len());
                        }
                        links.extend(std::iter::repeat_n(link, view.runs.len()));
                        equations.extend(shifted_equations(&view, offset));
                        runs.extend(view.runs);
                    }
                    None => {
                        runs.push(run.clone());
                        links.push(link);
                    }
                }
            }
        }
        let mut hyperlinks = Vec::new();
        let mut start = 0;
        while start < runs.len() {
            let mut end = start + 1;
            while end < runs.len() && links[end] == links[start] {
                end += 1;
            }
            if let Some(link) = links[start] {
                hyperlinks.push(HyperlinkSpan {
                    run_start: start,
                    run_end: end,
                    ..self.hyperlinks[link].clone()
                });
            }
            start = end;
        }
        AcceptedView {
            paragraph: Cow::Owned(CT_P {
                properties: self.properties.clone(),
                runs,
                hyperlinks,
                equations,
                ..CT_P::new()
            }),
            field_boundaries: inline_boundaries,
            bookmark_starts: first_of_each_name(bookmark_starts),
        }
    }

    /// Return the content of a preserved smart tag or inline custom XML
    /// element, which [`Self::accepted_view`] reads in place. None for any
    /// other raw paragraph child.
    #[doc(hidden)]
    pub fn raw_run_wrapper_content(raw: &[u8]) -> Option<CT_P> {
        if !is_run_wrapper(raw) {
            return None;
        }
        run_wrapper_paragraph(raw, &wrapper_scope())
    }

    /// Current accepted/tracked indexes for the retained physical marker occurrences.
    /// Original source slots and malformed marker evidence are unchanged.
    #[doc(hidden)]
    pub fn current_bookmark_projection(&self) -> Vec<BookmarkMarker> {
        if self.bookmark_markers.is_empty() {
            return Vec::new();
        }
        current_paragraph_bookmark_projection(self).markers
    }

    /// Return accepted-view runs in the same order as bookmark projections.
    #[doc(hidden)]
    pub fn accepted_bookmark_runs(&self) -> Vec<&CT_R> {
        accepted_paragraph_runs(self)
    }

    /// Return recursive addresses for every accepted-view run.
    #[doc(hidden)]
    pub fn accepted_run_paths(&self) -> Vec<AcceptedRunPath> {
        accepted_paragraph_run_paths(self)
    }

    /// Resolve one recursive accepted-view run address.
    #[doc(hidden)]
    pub fn accepted_run(&self, path: &AcceptedRunPath) -> Option<&CT_R> {
        self.accepted_run_segments(path.segments())
    }

    pub(crate) fn accepted_run_segments(&self, path: &[AcceptedRunPathSegment]) -> Option<&CT_R> {
        let (first, rest) = path.split_first()?;
        match *first {
            AcceptedRunPathSegment::Run(index) if rest.is_empty() => self.runs.get(index),
            AcceptedRunPathSegment::ContentControl(index) => self
                .content_controls
                .get(index)?
                .3
                .accepted_run_segments(rest),
            AcceptedRunPathSegment::Revision(index) => {
                self.revisions.get(index)?.2.accepted_run_segments(rest)
            }
            AcceptedRunPathSegment::Run(_) => None,
        }
    }

    pub(crate) fn accepted_run_segments_mut(
        &mut self,
        path: &[AcceptedRunPathSegment],
    ) -> Option<&mut CT_R> {
        let (first, rest) = path.split_first()?;
        match *first {
            AcceptedRunPathSegment::Run(index) if rest.is_empty() => self.runs.get_mut(index),
            AcceptedRunPathSegment::ContentControl(index) => self
                .content_controls
                .get_mut(index)?
                .3
                .accepted_run_segments_mut(rest),
            AcceptedRunPathSegment::Revision(index) => self
                .revisions
                .get_mut(index)?
                .2
                .accepted_run_segments_mut(rest),
            AcceptedRunPathSegment::Run(_) => None,
        }
    }

    pub(crate) fn has_changed_field_owners(&self) -> bool {
        self.runs.iter().any(|run| {
            run.content
                .iter()
                .any(|content| matches!(content, RunContent::Field(field) if !field.is_unchanged()))
        }) || self
            .content_controls
            .iter()
            .any(|(_, _, _, control)| control.has_changed_field_owners())
            || self.revisions.iter().any(|(_, _, revision)| {
                revision
                    .content_paragraph()
                    .is_some_and(CT_P::has_changed_field_owners)
            })
    }

    pub(crate) fn normalize_field_owners_in(&mut self, work: &mut WordOperationWork) -> Result<()> {
        self.validate_resource_limits()?;
        for run in &mut self.runs {
            for content in &mut run.content {
                if let RunContent::Field(field) = content {
                    field.normalize_cached_result_owner_in(work)?;
                }
            }
        }
        for (_, _, _, control) in &mut self.content_controls {
            control.normalize_field_owners_in(work)?;
        }
        for (_, _, revision) in &mut self.revisions {
            revision.normalize_pending_fields_in(work)?;
        }
        Ok(())
    }

    /// Replace one actual slot without retiring its containing revision/control.
    pub(crate) fn append_ordered_comment_ranges(
        &self,
        accepted: &mut usize,
        output: &mut Vec<CommentRangeMarker>,
    ) {
        for boundary in 0..=self.runs.len() {
            for item in self.boundary_items(boundary) {
                match item {
                    BoundaryItem::Control(index) => self.content_controls[index]
                        .3
                        .append_ordered_comment_ranges(accepted, output),
                    BoundaryItem::Marker(index) => {
                        let mut marker = self.comment_ranges[index].clone();
                        match &mut marker {
                            CommentRangeMarker::Start {
                                run_index,
                                raw_before,
                                ..
                            }
                            | CommentRangeMarker::End {
                                run_index,
                                raw_before,
                                ..
                            } => {
                                *run_index = *accepted;
                                *raw_before = 0;
                            }
                        }
                        output.push(marker);
                    }
                    BoundaryItem::Raw(index) => {
                        let slot = self.extra_xml[..index]
                            .iter()
                            .filter(|(at, _)| *at == boundary)
                            .count();
                        if let Some((_, _, revision)) = self
                            .revisions
                            .iter()
                            .find(|(at, raw, _)| *at == boundary && *raw == slot)
                            && matches!(
                                revision.kind(),
                                crate::revision::RevisionKind::Insertion
                                    | crate::revision::RevisionKind::MoveTo
                            )
                            && let Some(paragraph) = revision.content_paragraph()
                        {
                            paragraph.append_ordered_comment_ranges(accepted, output);
                        }
                    }
                }
            }
            if boundary < self.runs.len() {
                *accepted += 1;
            }
        }
    }

    pub(crate) fn insert_ordered_result_comment_ranges(
        &mut self,
        path: &[AcceptedRunPathSegment],
        ranges: &[CommentRangeMarker],
    ) -> Result<bool> {
        let Some((first, rest)) = path.split_first() else {
            return Ok(false);
        };
        match *first {
            AcceptedRunPathSegment::Run(base) if rest.is_empty() => {
                let mut boundaries = Vec::<(usize, Vec<BoundaryItem>)>::new();
                let mut inserted_at_boundary = 0;
                for marker in ranges {
                    let boundary = base.checked_add(marker.run_index()).ok_or_else(|| {
                        OxmlError::InvalidValue("annotation result boundary overflow".into())
                    })?;
                    if boundary > self.runs.len() {
                        return Ok(false);
                    }
                    if boundaries.last().is_none_or(|(at, _)| *at != boundary) {
                        boundaries.push((boundary, self.boundary_items(boundary)));
                        inserted_at_boundary = 0;
                    }
                    let mut current = marker.clone();
                    match &mut current {
                        CommentRangeMarker::Start { run_index, .. }
                        | CommentRangeMarker::End { run_index, .. } => *run_index = boundary,
                    }
                    let marker_index = self.comment_ranges.len();
                    self.comment_ranges.push(current);
                    let items = &mut boundaries.last_mut().expect("current boundary").1;
                    // At the first anchor the retained preceding events stay
                    // before the new payload. At later requested boundaries the
                    // new markers precede retained residual/source events.
                    if boundary == base {
                        items.push(BoundaryItem::Marker(marker_index));
                    } else {
                        items.insert(inserted_at_boundary, BoundaryItem::Marker(marker_index));
                    }
                    inserted_at_boundary += 1;
                }
                self.place_boundary_items(&boundaries);
                Ok(true)
            }
            AcceptedRunPathSegment::ContentControl(index) => {
                match self.content_controls.get_mut(index) {
                    Some((_, _, _, control)) => {
                        control.insert_ordered_result_comment_ranges(rest, ranges)
                    }
                    None => Ok(false),
                }
            }
            AcceptedRunPathSegment::Revision(index) => match self
                .revisions
                .get_mut(index)
                .and_then(|(_, _, revision)| revision.ordered_paragraph_mut())
            {
                Some(paragraph) => paragraph.insert_ordered_result_comment_ranges(rest, ranges),
                None => Ok(false),
            },
            AcceptedRunPathSegment::Run(_) => Ok(false),
        }
    }

    pub(crate) fn replace_ordered_result_slot(
        &mut self,
        path: &[AcceptedRunPathSegment],
        replacement: Vec<CT_R>,
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<bool> {
        let Some((first, rest)) = path.split_first() else {
            return Ok(false);
        };
        match *first {
            AcceptedRunPathSegment::Run(index) if rest.is_empty() => {
                if index >= self.runs.len() {
                    return Ok(false);
                }
                if replacement.len() == 1 {
                    return self.replace_accepted_run_segments(
                        path,
                        replacement.into_iter().next().expect("one replacement run"),
                    );
                }
                let mut replacement = replacement;
                // A zero logical payload must retain the last physical base of
                // an excluded Ruby owner at its original anchor.
                if replacement.is_empty()
                    && self
                        .rubies
                        .iter()
                        .any(|ruby| ruby.base_start == index && ruby.base_end == index + 1)
                {
                    let mut residual = self.runs[index].clone();
                    residual.replace_content(Vec::new());
                    return self.replace_accepted_run_segments(path, residual);
                }
                let count = replacement.len();
                let new_len = self
                    .runs
                    .len()
                    .checked_sub(1)
                    .and_then(|len| len.checked_add(count))
                    .ok_or_else(|| OxmlError::InvalidValue("ordered run count overflow".into()))?;
                let shifted = |at: usize| -> Result<usize> {
                    let mapped = if at > index {
                        at.checked_sub(1).and_then(|at| at.checked_add(count))
                    } else {
                        Some(at)
                    };
                    mapped.filter(|at| *at <= new_len).ok_or_else(|| {
                        OxmlError::InvalidValue("ordered owner boundary overflow".into())
                    })
                };
                for ruby in &self.rubies {
                    if shifted(ruby.base_start)? >= shifted(ruby.base_end)? {
                        return Err(OxmlError::InvalidValue(
                            "ordered replacement collapsed Ruby base".into(),
                        ));
                    }
                }
                for hyperlink in &self.hyperlinks {
                    shifted(hyperlink.run_start)?;
                    shifted(hyperlink.run_end)?;
                    for (at, _, _) in &hyperlink.extra_xml {
                        shifted(hyperlink.run_start.checked_add(*at).ok_or_else(|| {
                            OxmlError::InvalidValue("ordered hyperlink boundary overflow".into())
                        })?)?;
                    }
                }
                let hyperlinks = self.hyperlinks.clone();
                let mut rubies = std::mem::take(&mut self.rubies);
                for ruby in &mut rubies {
                    ruby.base_start = shifted(ruby.base_start)?;
                    ruby.base_end = shifted(ruby.base_end)?;
                }
                let revision_slots = self
                    .revisions
                    .iter()
                    .map(|(_, slot, _)| *slot)
                    .collect::<Vec<_>>();
                for (offset, run) in replacement.drain(..).enumerate() {
                    if !self.insert_run_after_boundary_items(index + offset, usize::MAX, run) {
                        return Ok(false);
                    }
                }
                let mut removed = vec![false; self.runs.len()];
                removed[index + count] = true;
                self.remove_runs_core(&removed, source_context)?;
                self.rubies = rubies;
                // Every boundary was checked against the prospective length above.
                let shifted = |at: usize| if at > index { at - 1 + count } else { at };
                self.hyperlinks = hyperlinks
                    .into_iter()
                    .map(|mut hyperlink| {
                        let old_start = hyperlink.run_start;
                        hyperlink.run_start = shifted(hyperlink.run_start);
                        hyperlink.run_end = shifted(hyperlink.run_end);
                        for (at, _, _) in &mut hyperlink.extra_xml {
                            *at = shifted(old_start + *at) - hyperlink.run_start;
                        }
                        hyperlink
                    })
                    .collect();
                if revision_slots.len() != self.revisions.len() {
                    return Err(OxmlError::InvalidValue(
                        "ordered replacement changed revision slot cardinality".into(),
                    ));
                }
                for ((_, slot, _), original) in self.revisions.iter_mut().zip(revision_slots) {
                    if hyperlink_revision_index(original).is_some() {
                        *slot = original;
                    }
                }
                Ok(true)
            }
            AcceptedRunPathSegment::ContentControl(index) => self
                .content_controls
                .get_mut(index)
                .map(|(_, _, _, control)| {
                    control.replace_ordered_result_slot(rest, replacement, source_context)
                })
                .unwrap_or(Ok(false)),
            AcceptedRunPathSegment::Revision(index) => {
                let Some((_, _, revision)) = self.revisions.get_mut(index) else {
                    return Ok(false);
                };
                revision.replace_ordered_result_slot(rest, replacement)
            }
            AcceptedRunPathSegment::Run(_) => Ok(false),
        }
    }

    /// Replace one accepted-view run while retaining its recursive owner.
    #[doc(hidden)]
    pub fn replace_accepted_run(
        &mut self,
        path: &AcceptedRunPath,
        replacement: CT_R,
    ) -> Result<bool> {
        self.replace_accepted_run_segments(path.segments(), replacement)
    }

    pub(crate) fn replace_accepted_run_segments(
        &mut self,
        path: &[AcceptedRunPathSegment],
        replacement: CT_R,
    ) -> Result<bool> {
        self.replace_accepted_run_segments_in(path, replacement, None, None, false)
    }

    pub(crate) fn replace_accepted_run_segments_in(
        &mut self,
        path: &[AcceptedRunPathSegment],
        replacement: CT_R,
        work: Option<&mut WordOperationWork>,
        emission: Option<&FieldXmlEmission>,
        prepared_fields: bool,
    ) -> Result<bool> {
        let Some((first, rest)) = path.split_first() else {
            return Ok(false);
        };
        match *first {
            AcceptedRunPathSegment::Run(index) if rest.is_empty() => {
                let Some(run) = self.runs.get_mut(index) else {
                    return Ok(false);
                };
                *run = replacement;
                Ok(true)
            }
            AcceptedRunPathSegment::ContentControl(index) => {
                let Some((_, _, _, control)) = self.content_controls.get_mut(index) else {
                    return Ok(false);
                };
                if work.is_some() {
                    control.replace_accepted_run_segments_in(
                        rest,
                        replacement,
                        work,
                        emission,
                        prepared_fields,
                    )
                } else {
                    control.replace_accepted_run_segments(rest, replacement)
                }
            }
            AcceptedRunPathSegment::Revision(index) => {
                let Some((_, _, revision)) = self.revisions.get_mut(index) else {
                    return Ok(false);
                };
                if work.is_some() {
                    revision.replace_accepted_run_segments_in(
                        rest,
                        replacement,
                        work,
                        emission,
                        prepared_fields,
                    )
                } else {
                    revision.replace_accepted_run_segments(rest, replacement)
                }
            }
            AcceptedRunPathSegment::Run(_) => Ok(false),
        }
    }

    /// Split one accepted-view run and return the accepted boundary.
    #[doc(hidden)]
    pub fn split_accepted_run(
        &mut self,
        path: &AcceptedRunPath,
        accepted_index: usize,
        offset: usize,
    ) -> Result<usize> {
        self.split_accepted_run_in(path, accepted_index, offset, None, None)
    }

    pub(crate) fn split_accepted_run_in(
        &mut self,
        path: &AcceptedRunPath,
        accepted_index: usize,
        offset: usize,
        work: Option<&mut WordOperationWork>,
        emission: Option<&FieldXmlEmission>,
    ) -> Result<usize> {
        self.split_accepted_run_at(
            path,
            accepted_index,
            offset,
            work,
            emission,
            SplitEdge::Plain,
        )
    }

    /// [`Self::split_accepted_run_in`], where `edge` says where the text-free
    /// content at the cut goes.
    pub(crate) fn split_accepted_run_at(
        &mut self,
        path: &AcceptedRunPath,
        accepted_index: usize,
        offset: usize,
        work: Option<&mut WordOperationWork>,
        emission: Option<&FieldXmlEmission>,
        edge: SplitEdge,
    ) -> Result<usize> {
        let run = self
            .accepted_run(path)
            .ok_or_else(|| OxmlError::InvalidValue("accepted run path is stale".to_owned()))?;
        let literal_len = run.literal_len();
        if offset > literal_len {
            return Err(OxmlError::InvalidValue(format!(
                "split offset {offset} exceeds a run's literal text length of {literal_len} characters"
            )));
        }
        if run.split_point(offset, edge).is_none() {
            return Ok(if offset == 0 {
                accepted_index
            } else {
                accepted_index + 1
            });
        }
        // Field XML emission only applies to a recorded operation.
        let emission = emission.filter(|_| work.is_some());
        if self.split_accepted_run_segments_in(path.segments(), offset, work, emission, edge)? {
            Ok(accepted_index + 1)
        } else {
            Err(OxmlError::InvalidValue(
                "accepted run path is stale".to_owned(),
            ))
        }
    }

    /// Return the literal text of the accepted-view runs, the text that
    /// split offsets count. Tabs, breaks and other non-text content have no
    /// width.
    #[doc(hidden)]
    pub fn accepted_literal_text(&self) -> String {
        accepted_paragraph_runs(self)
            .into_iter()
            .flat_map(|run| run.content.iter().map(CT_R::literal_text))
            .collect()
    }

    /// Return the text of every `t` element, in any namespace, between the
    /// range markers of comment `id`, or `None` when this paragraph does not
    /// hold both markers.
    ///
    /// Unlike [`Self::accepted_literal_text`], this includes the text of
    /// preserved children that the accepted view leaves out, such as a
    /// `w:fldSimple` result, so it is the text the commented range shows.
    #[doc(hidden)]
    pub fn comment_range_text(&self, id: i32) -> Option<String> {
        let mut xml = Vec::new();
        self.to_xml(&mut Writer::new(&mut xml)).ok()?;
        let is_marker = |element: &BytesStart<'_>, local: &[u8]| {
            matches_local_name(element.name().as_ref(), local)
                && element
                    .attributes()
                    .flatten()
                    .find(|attribute| matches_local_name(attribute.key.as_ref(), b"id"))
                    .and_then(|attribute| {
                        oxoffice_core::xml::attribute_text(&attribute)
                            .ok()?
                            .parse()
                            .ok()
                    })
                    == Some(id)
        };
        let mut reader = Reader::from_reader(xml.as_slice());
        let mut buffer = Vec::new();
        let mut text = None::<String>;
        loop {
            match reader.read_event_into(&mut buffer).ok()? {
                Event::Start(element) | Event::Empty(element)
                    if is_marker(&element, b"commentRangeStart") =>
                {
                    text = Some(String::new());
                }
                Event::Start(element) | Event::Empty(element)
                    if is_marker(&element, b"commentRangeEnd") =>
                {
                    return text;
                }
                Event::Start(element) if matches_local_name(element.name().as_ref(), b"t") => {
                    let content = crate::xml_text::read_element_text(&mut reader, element.name());
                    if let Some(text) = text.as_mut() {
                        text.push_str(&content);
                    }
                }
                Event::Eof => return None,
                _ => {}
            }
            buffer.clear();
        }
    }

    /// Read the literal range safeguard from its namespace-qualified source.
    /// Only qualified Word markers, identity attributes and `t` elements count.
    /// Tabs and breaks have zero width, while field-result text remains visible.
    /// This does not change the richer accepted anchor projection.
    #[doc(hidden)]
    pub fn comment_range_text_from_source(xml: &[u8], id: i32) -> Result<Option<String>> {
        // Validate the complete source separately. Text extraction below skips
        // `t` subtrees and ends at the selected marker, neither of which may
        // hide a missing namespace declaration elsewhere in the candidate.
        let mut validation = NsReader::from_reader(xml);
        loop {
            let (namespace, event) = validation.read_resolved_event()?;
            if let ResolveResult::Unknown(prefix) = namespace {
                return Err(OxmlError::InvalidValue(format!(
                    "comment literal source has unresolved element namespace prefix {:?}",
                    String::from_utf8_lossy(&prefix)
                )));
            }
            match event {
                Event::Start(element) | Event::Empty(element) => {
                    for attribute in element.attributes() {
                        let attribute = attribute?;
                        attribute.decoded_and_normalized_value(
                            XmlVersion::Implicit1_0,
                            validation.decoder(),
                        )?;
                        if !namespace_declaration(attribute.key.as_ref())
                            && let (ResolveResult::Unknown(prefix), _) =
                                validation.resolver().resolve_attribute(attribute.key)
                        {
                            return Err(OxmlError::InvalidValue(format!(
                                "comment literal source has unresolved attribute namespace prefix {:?}",
                                String::from_utf8_lossy(&prefix)
                            )));
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        let mut reader = NsReader::from_reader(xml);
        let mut text = None::<String>;
        loop {
            let (namespace, event) = reader.read_resolved_event()?;
            let word = matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == crate::namespace::W_NS.as_bytes()
            || std::str::from_utf8(uri).ok().and_then(|uri| quick_xml::escape::unescape(uri).ok()).is_some_and(|uri| uri == crate::namespace::W_NS));
            drop(namespace);
            match event {
                Event::Start(element) | Event::Empty(element)
                    if word
                        && matches!(
                            element.local_name().as_ref(),
                            b"commentRangeStart" | b"commentRangeEnd"
                        ) =>
                {
                    let mut marker_id = None;
                    for attribute in element.attributes() {
                        let attribute = attribute?;
                        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                        if local.as_ref() == b"id"
                            && matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == crate::namespace::W_NS.as_bytes())
                        {
                            marker_id = Some(
                                attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        reader.decoder(),
                                    )?
                                    .parse::<i32>()
                                    .map_err(|error| OxmlError::InvalidValue(error.to_string()))?,
                            );
                        }
                    }
                    if marker_id == Some(id) {
                        if element.local_name().as_ref() == b"commentRangeEnd" {
                            return Ok(text);
                        }
                        text = Some(String::new());
                    }
                }
                Event::Start(element) if word && element.local_name().as_ref() == b"t" => {
                    let encoded = reader.read_text(element.name())?;
                    if let Some(text) = text.as_mut() {
                        text.push_str(&crate::xml_text::decode_escaped(&encoded));
                    }
                }
                Event::Eof => return Ok(None),
                _ => {}
            }
        }
    }

    /// Split accepted-view runs so that the non-empty literal text span
    /// `[start, end)`, in Unicode scalar values of
    /// [`Self::accepted_literal_text`], covers whole runs, and return the run
    /// boundaries around it.
    #[doc(hidden)]
    pub fn split_accepted_literal_span(
        &mut self,
        start: usize,
        end: usize,
    ) -> Result<(usize, usize)> {
        let lengths = accepted_paragraph_runs(self)
            .into_iter()
            .map(CT_R::literal_len)
            .collect::<Vec<_>>();
        // The run holding one literal character and its offset in that run.
        let locate = |character: usize| {
            let mut run_start = 0;
            lengths.iter().enumerate().find_map(|(index, len)| {
                let found = (character < run_start + len).then_some((index, character - run_start));
                run_start += len;
                found
            })
        };
        let outside = || {
            OxmlError::InvalidValue(format!(
                "literal span {start}..{end} is not inside the paragraph text"
            ))
        };
        let ((start_run, start_offset), (end_run, end_offset)) = match end.checked_sub(1) {
            Some(last) if start < end => (
                locate(start).ok_or_else(outside)?,
                locate(last).ok_or_else(outside)?,
            ),
            _ => return Err(outside()),
        };
        // The end goes first, so the start run keeps its index. Text-free
        // content at either edge stays outside the span, even at the edge of
        // a run.
        let path = self.accepted_run_paths()[end_run].clone();
        let end_boundary = self.split_accepted_run_at(
            &path,
            end_run,
            end_offset + 1,
            None,
            None,
            SplitEdge::SpanEnd,
        )?;
        let path = self.accepted_run_paths()[start_run].clone();
        let start_boundary = self.split_accepted_run_at(
            &path,
            start_run,
            start_offset,
            None,
            None,
            SplitEdge::SpanStart,
        )?;
        // A split of the start run adds one run before the end boundary.
        Ok((start_boundary, end_boundary + start_boundary - start_run))
    }

    pub(crate) fn split_accepted_run_segments_in(
        &mut self,
        path: &[AcceptedRunPathSegment],
        offset: usize,
        work: Option<&mut WordOperationWork>,
        emission: Option<&FieldXmlEmission>,
        edge: SplitEdge,
    ) -> Result<bool> {
        let Some((first, rest)) = path.split_first() else {
            return Ok(false);
        };
        match *first {
            AcceptedRunPathSegment::Run(index) if rest.is_empty() => self
                .split_run_with(index, offset, edge)
                .map(|_| true)
                .map_err(|error| OxmlError::InvalidValue(error.to_string())),
            AcceptedRunPathSegment::ContentControl(index) => {
                let Some((_, _, _, control)) = self.content_controls.get_mut(index) else {
                    return Ok(false);
                };
                control.split_accepted_run_segments_in(rest, offset, work, emission, edge)
            }
            AcceptedRunPathSegment::Revision(index) => {
                let Some((_, _, revision)) = self.revisions.get_mut(index) else {
                    return Ok(false);
                };
                revision.split_accepted_run_segments_in(rest, offset, work, emission, edge)
            }
            AcceptedRunPathSegment::Run(_) => Ok(false),
        }
    }

    /// Remove one accepted-view run.
    ///
    /// The comment, bookmark and permission markers and the other preserved
    /// children around the run stay where they are. A run inside a
    /// hyperlink, an inline content control or a tracked insertion is removed
    /// inside it. A hyperlink or a tracked insertion left with nothing in it
    /// goes too, while a content control stays with its properties, as Word
    /// keeps an emptied control to show its placeholder. A run holding a
    /// comment, footnote or endnote reference, part of a complex field whose
    /// other parts are in other runs, or part of a tracked move destination
    /// is refused, since the note, the field or the move would lose its
    /// balance. On
    /// error the paragraph is unchanged.
    #[doc(hidden)]
    pub fn remove_accepted_run(&mut self, path: &AcceptedRunPath) -> Result<()> {
        let stale = || OxmlError::InvalidValue("accepted run path is stale".to_owned());
        let run = self.accepted_run(path).ok_or_else(stale)?;
        if let Some(id) = run.content.iter().find_map(|content| match content {
            RunContent::CommentReference { id, .. } => Some(*id),
            _ => None,
        }) {
            return Err(OxmlError::InvalidValue(format!(
                "it holds the reference of comment {id}, which removing the comment removes"
            )));
        }
        if let Some((kind, id)) = run.content.iter().find_map(|content| match content {
            RunContent::FootnoteRef { id, .. } => Some(("footnote", *id)),
            RunContent::EndnoteRef { id, .. } => Some(("endnote", *id)),
            _ => None,
        }) {
            return Err(OxmlError::InvalidValue(format!(
                "it holds the reference of {kind} {id}, which would be left without one"
            )));
        }
        if run
            .extra_xml
            .iter()
            .any(|raw| raw_is_complex_field_part(raw))
        {
            return Err(OxmlError::InvalidValue(
                "it holds part of a complex field whose other parts are in other runs".to_owned(),
            ));
        }
        let mut paragraph = self.clone();
        if !paragraph.remove_accepted_run_segments(path.segments())? {
            return Err(stale());
        }
        // Accepted run indexes after the run moved down by one.
        let _ = paragraph.refresh_bookmark_projection();
        *self = paragraph;
        Ok(())
    }

    /// Remove the accepted-view run at `path`, and a tracked insertion left
    /// with nothing in it. Returns false for a stale path.
    pub(crate) fn remove_accepted_run_segments(
        &mut self,
        path: &[AcceptedRunPathSegment],
    ) -> Result<bool> {
        let Some((first, rest)) = path.split_first() else {
            return Ok(false);
        };
        match *first {
            AcceptedRunPathSegment::Run(index) if rest.is_empty() => {
                if index >= self.runs.len() {
                    return Ok(false);
                }
                let mut removed = vec![false; self.runs.len()];
                removed[index] = true;
                self.remove_runs(&removed)?;
                Ok(true)
            }
            AcceptedRunPathSegment::ContentControl(index) => {
                let Some((_, _, _, control)) = self.content_controls.get_mut(index) else {
                    return Ok(false);
                };
                control.remove_accepted_run_segments(rest)
            }
            AcceptedRunPathSegment::Revision(index) => {
                let Some((_, _, revision)) = self.revisions.get_mut(index) else {
                    return Ok(false);
                };
                match revision.remove_accepted_run_segments(rest)? {
                    None => Ok(false),
                    Some(emptied) => {
                        if emptied {
                            self.remove_revision(index);
                        }
                        Ok(true)
                    }
                }
            }
            AcceptedRunPathSegment::Run(_) => Ok(false),
        }
    }

    /// Whether the paragraph has no child at all, attributes aside.
    pub(crate) fn has_no_children(&self) -> bool {
        self.runs.is_empty()
            && self.hyperlinks.is_empty()
            && self.comment_ranges.is_empty()
            && self.bookmark_markers.is_empty()
            && self.content_controls.is_empty()
            && self.revisions.is_empty()
            && self.equations.is_empty()
            && self.rubies.is_empty()
            && self
                .extra_xml
                .iter()
                .all(|(position, raw)| Self::raw_is_root_attributes(*position, raw))
    }

    /// Remove the preserved children at these indices of
    /// [`Self::extra_xml`], with the revision written in place of one, and
    /// move every projection that counts the children before them. Returns
    /// how many went.
    #[doc(hidden)]
    pub fn remove_raw_children(&mut self, positions: &[usize]) -> usize {
        let mut positions = positions.to_vec();
        positions.sort_unstable();
        positions.dedup();
        let mut removed = 0;
        for position in positions.into_iter().rev() {
            let Some((boundary, _)) = self.extra_xml.get(position) else {
                continue;
            };
            let boundary = *boundary;
            let slot = self.extra_xml[..position]
                .iter()
                .filter(|(at, _)| *at == boundary)
                .count();
            match self.revisions.iter().position(|(at, other, _)| {
                *at == boundary && *other == slot && hyperlink_revision_index(*other).is_none()
            }) {
                Some(index) => self.remove_revision(index),
                None => self.remove_boundary_raw(boundary, slot),
            }
            removed += 1;
        }
        if removed > 0 {
            // A bookmark the removed children held goes from the projection.
            let _ = self.refresh_bookmark_projection();
        }
        removed
    }

    /// Remove the tracked insertions, deletions and moves inside the
    /// paragraph's hyperlinks, with everything they hold, and a hyperlink left
    /// with nothing in it, then the paragraph mark's own insertion, deletion
    /// or move: the mark stays, untracked. Those among the paragraph's own
    /// children go with [`Self::remove_raw_children`]. The changes of
    /// properties stay. Returns how many went.
    #[doc(hidden)]
    pub fn remove_content_revisions(&mut self) -> usize {
        let mut removed = 0;
        for index in (0..self.revisions.len()).rev() {
            if self.revisions[index].2.kind().changes_content() {
                self.remove_revision(index);
                removed += 1;
            }
        }
        if let Some(mark) = self
            .properties
            .as_mut()
            .and_then(|properties| properties.rpr.as_mut())
        {
            let before = mark.revision_markers.len();
            mark.revision_markers
                .retain(|marker| !marker.kind().changes_content());
            removed += before - mark.revision_markers.len();
            for index in (0..mark.revision_xml.len()).rev() {
                if crate::revision::raw_changes_content(&mark.revision_xml[index]) {
                    if mark.revision_xml_positions.len() == mark.revision_xml.len() {
                        mark.revision_xml_positions.remove(index);
                    }
                    mark.revision_xml.remove(index);
                    removed += 1;
                }
            }
        }
        if removed > 0 {
            let _ = self.refresh_bookmark_projection();
        }
        removed
    }

    /// Remove the revision wrapper at `index` with the placeholder that holds
    /// its place among the paragraph children, or its place inside a
    /// hyperlink, and the hyperlink when nothing else is left in it.
    fn remove_revision(&mut self, index: usize) {
        let (boundary, slot, _) = self.revisions.remove(index);
        let Some(hyperlink_index) = hyperlink_revision_index(slot) else {
            self.remove_boundary_raw(boundary, slot);
            return;
        };
        // The children of a hyperlink count the revisions written before them.
        let position = self.revisions[..index]
            .iter()
            .filter(|(at, other, _)| *at == boundary && *other == slot)
            .count();
        let Some(hyperlink) = self.hyperlinks.get_mut(hyperlink_index) else {
            return;
        };
        let relative = boundary.saturating_sub(hyperlink.run_start);
        for (at, revisions_before, _) in &mut hyperlink.extra_xml {
            if *at == relative && *revisions_before > position {
                *revisions_before -= 1;
            }
        }
        let emptied = hyperlink.run_start == hyperlink.run_end
            && hyperlink.extra_xml.is_empty()
            && !self.revisions.iter().any(|(_, other, _)| *other == slot);
        if !emptied {
            return;
        }
        let hyperlink = self.hyperlinks.remove(hyperlink_index);
        for (_, other, _) in &mut self.revisions {
            if let Some(later) = hyperlink_revision_index(*other)
                && later > hyperlink_index
            {
                *other = hyperlink_revision_slot(later - 1);
            }
        }
        if let Some(raw_before) = hyperlink.preserved_raw_before {
            self.remove_boundary_raw(hyperlink.run_start, raw_before);
        }
    }

    /// Remove the preserved child at `slot` among those at run `boundary`,
    /// and move every projection that counts the children before it. The
    /// children on both sides of it then share one slot, where those that
    /// came before it stay first.
    fn remove_boundary_raw(&mut self, boundary: usize, slot: usize) {
        let Some(position) = self
            .extra_xml
            .iter()
            .enumerate()
            .filter(|(_, (at, _))| *at == boundary)
            .nth(slot)
            .map(|(position, _)| position)
        else {
            return;
        };
        self.extra_xml.remove(position);
        self.comment_ranges
            .sort_by_key(|marker| (marker.run_index(), marker.raw_before()));
        let merged_markers = self
            .comment_ranges
            .iter()
            .filter(|marker| marker.run_index() == boundary && marker.raw_before() == slot)
            .count();
        let shift = |before: &mut usize| {
            if *before > slot {
                *before -= 1;
            }
        };
        for marker in &mut self.comment_ranges {
            match marker {
                CommentRangeMarker::Start {
                    run_index,
                    raw_before,
                    ..
                }
                | CommentRangeMarker::End {
                    run_index,
                    raw_before,
                    ..
                } if *run_index == boundary => shift(raw_before),
                _ => {}
            }
        }
        for (at, raw_before, markers_before, _) in &mut self.content_controls {
            if *at == boundary {
                if *raw_before == slot + 1 {
                    *markers_before += merged_markers;
                }
                shift(raw_before);
            }
        }
        for marker in &mut self.bookmark_markers {
            if marker.run_index == boundary {
                shift(&mut marker.raw_before);
            }
        }
        for (at, other, _) in &mut self.revisions {
            if *at == boundary && hyperlink_revision_index(*other).is_none() {
                shift(other);
            }
        }
        for (at, other, _) in &mut self.equations {
            if *at == boundary {
                shift(other);
            }
        }
        for hyperlink in &mut self.hyperlinks {
            if hyperlink.run_start == boundary
                && let Some(raw_before) = hyperlink.preserved_raw_before.as_mut()
            {
                shift(raw_before);
            }
        }
    }

    /// Add a run with the given text.
    pub fn add_run(&mut self, text: &str) -> &mut CT_R {
        self.runs.push(CT_R::new(text));
        self.runs.last_mut().unwrap()
    }

    /// Insert a direct run while keeping every paragraph boundary projection aligned.
    #[doc(hidden)]
    pub fn insert_unwrapped_run(&mut self, run_index: usize, run: CT_R) -> bool {
        if run_index > self.runs.len() {
            return false;
        }
        self.shift_run_boundaries_from(run_index);

        let mut hyperlinks = Vec::with_capacity(self.hyperlinks.len() + 1);
        let mut hyperlink_map = Vec::with_capacity(self.hyperlinks.len());
        for mut hyperlink in self.hyperlinks.drain(..) {
            if hyperlink.run_start >= run_index {
                hyperlink.run_start += 1;
                hyperlink.run_end += 1;
                hyperlink_map.push((hyperlinks.len(), None, false));
                hyperlinks.push(hyperlink);
            } else if hyperlink.run_end > run_index {
                let split_at = run_index - hyperlink.run_start;
                let suffix = HyperlinkSpan {
                    rel_id: hyperlink.rel_id.clone(),
                    anchor: hyperlink.anchor.clone(),
                    tooltip: hyperlink.tooltip.clone(),
                    doc_location: hyperlink.doc_location.clone(),
                    run_start: run_index + 1,
                    run_end: hyperlink.run_end + 1,
                    extra_attributes: hyperlink.extra_attributes.clone(),
                    extra_xml: hyperlink
                        .extra_xml
                        .iter()
                        .filter(|(boundary, _, _)| *boundary >= split_at)
                        .map(|(boundary, before, raw)| (boundary - split_at, *before, raw.clone()))
                        .collect(),
                    preserved_raw_before: None,
                };
                hyperlink
                    .extra_xml
                    .retain(|(boundary, _, _)| *boundary < split_at);
                hyperlink.run_end = run_index;
                let prefix_index = hyperlinks.len();
                hyperlinks.push(hyperlink);
                let suffix_index = hyperlinks.len();
                hyperlinks.push(suffix);
                hyperlink_map.push((prefix_index, Some(suffix_index), false));
            } else {
                hyperlink_map.push((hyperlinks.len(), None, hyperlink.run_end == run_index));
                hyperlinks.push(hyperlink);
            }
        }
        self.hyperlinks = hyperlinks;
        for (at, slot, _) in &mut self.revisions {
            let Some(old_index) = hyperlink_revision_index(*slot) else {
                continue;
            };
            let Some((prefix, suffix, ended_at_insertion)) = hyperlink_map.get(old_index) else {
                continue;
            };
            if *ended_at_insertion && *at == run_index + 1 {
                *at = run_index;
            }
            let new_index = suffix.filter(|_| *at > run_index).unwrap_or(*prefix);
            *slot = hyperlink_revision_slot(new_index);
        }
        self.shift_ruby_spans_for_insert(run_index);
        self.runs.insert(run_index, run);
        self.refresh_bookmark_projection()
    }

    /// Split the direct run at `run_index` at a Unicode scalar offset of its
    /// literal text.
    ///
    /// The run keeps the text before `offset`. A new direct run at
    /// `run_index + 1` receives the rest with cloned properties, inside the
    /// same hyperlink. Zero selects the boundary before the run and the literal
    /// text length selects the boundary after it without changing the paragraph.
    /// Tabs, breaks, fields, drawings, references, and raw children are
    /// zero-width for this operation.
    /// Text-free content and raw children right after the split point go to
    /// the second run with the text that follows, and boundary markers after
    /// the original run stay after the second.
    /// Only [`RunSplitError::BookmarkProjection`] leaves the paragraph changed.
    pub fn split_run(
        &mut self,
        run_index: usize,
        offset: usize,
    ) -> std::result::Result<usize, RunSplitError> {
        self.split_run_with(run_index, offset, SplitEdge::Plain)
    }

    /// [`Self::split_run`], where `edge` says where the text-free content at
    /// the cut goes.
    pub(crate) fn split_run_with(
        &mut self,
        run_index: usize,
        offset: usize,
        edge: SplitEdge,
    ) -> std::result::Result<usize, RunSplitError> {
        let run_count = self.runs.len();
        let run = self
            .runs
            .get(run_index)
            .ok_or(RunSplitError::RunOutOfRange {
                run_index,
                run_count,
            })?;
        let literal_len = run.literal_len();
        if offset > literal_len {
            return Err(RunSplitError::OffsetOutOfRange {
                offset,
                len: literal_len,
            });
        }
        if run.split_point(offset, edge).is_none() {
            return Ok(if offset == 0 {
                run_index
            } else {
                run_index + 1
            });
        }
        let tail = self.runs[run_index].split_off_at(offset, edge)?;
        let inserted = run_index + 1;
        self.shift_run_boundaries_from(inserted);
        for hyperlink in &mut self.hyperlinks {
            if hyperlink.run_start >= inserted {
                hyperlink.run_start += 1;
                hyperlink.run_end += 1;
            } else if hyperlink.run_end >= inserted {
                // The hyperlink contains the split run, so it grows by one run.
                let boundary_after_run = inserted - hyperlink.run_start;
                for (boundary, _, _) in &mut hyperlink.extra_xml {
                    if *boundary >= boundary_after_run {
                        *boundary += 1;
                    }
                }
                hyperlink.run_end += 1;
            }
        }
        self.shift_ruby_spans_for_insert(inserted);
        self.runs.insert(inserted, tail);
        if self.refresh_bookmark_projection() {
            Ok(inserted)
        } else {
            Err(RunSplitError::BookmarkProjection)
        }
    }

    /// Move every run-boundary projection at or after `run_index` one run later.
    ///
    /// Hyperlink spans are left to the caller, which decides whether the new
    /// run joins or splits them.
    /// Move every ruby span across one run inserted at `inserted`.
    ///
    /// A span that starts at or after the insertion point moves whole, and a
    /// span that already contains the point grows by one run, which is the
    /// rule `w:hyperlink` spans follow beside this call.
    fn shift_ruby_spans_for_insert(&mut self, inserted: usize) {
        for ruby in &mut self.rubies {
            if ruby.base_start >= inserted {
                ruby.base_start += 1;
                ruby.base_end += 1;
            } else if ruby.base_end >= inserted {
                ruby.base_end += 1;
            }
        }
    }

    fn shift_run_boundaries_from(&mut self, run_index: usize) {
        for marker in &mut self.comment_ranges {
            match marker {
                CommentRangeMarker::Start { run_index: at, .. }
                | CommentRangeMarker::End { run_index: at, .. }
                    if *at >= run_index =>
                {
                    *at += 1;
                }
                CommentRangeMarker::Start { .. } | CommentRangeMarker::End { .. } => {}
            }
        }
        for marker in &mut self.bookmark_markers {
            if marker.run_index >= run_index {
                marker.run_index += 1;
            }
        }
        for (at, _, _, _) in &mut self.content_controls {
            if *at >= run_index {
                *at += 1;
            }
        }
        for (at, _, _) in &mut self.revisions {
            if *at >= run_index {
                *at += 1;
            }
        }
        for (position, _) in &mut self.extra_xml {
            if *position != ROOT_ATTRIBUTES_POSITION && *position >= run_index {
                *position += 1;
            }
        }
        for (position, _, _) in &mut self.equations {
            if *position >= run_index {
                *position += 1;
            }
        }
    }

    /// Remove selected comment anchors and remap every collapsed run boundary.
    ///
    /// The whole removal is staged. If the run removal refuses its source
    /// reservation, the paragraph keeps its prior state instead of losing its
    /// markers and references with nothing installed in their place.
    #[doc(hidden)]
    pub fn remove_comment_anchors(&mut self, ids: &[i32]) {
        let mut candidate = self.clone();
        if candidate.remove_comment_anchors_core(ids).is_err() {
            return;
        }
        *self = candidate;
    }

    fn remove_comment_anchors_core(&mut self, ids: &[i32]) -> Result<()> {
        for (run_index, raw_before, markers_before, _) in &mut self.content_controls {
            let preceding_removed =
                self.comment_ranges
                    .iter()
                    .filter(|marker| {
                        marker.run_index() == *run_index && marker.raw_before() == *raw_before
                    })
                    .take(*markers_before)
                    .filter(|marker| match marker {
                        CommentRangeMarker::Start { id, .. }
                        | CommentRangeMarker::End { id, .. } => ids.contains(id),
                    })
                    .count();
            *markers_before = markers_before.saturating_sub(preceding_removed);
        }
        self.comment_ranges.retain(|marker| match marker {
            CommentRangeMarker::Start { id, .. } | CommentRangeMarker::End { id, .. } => {
                !ids.contains(id)
            }
        });

        let mut removed = Vec::with_capacity(self.runs.len());
        for run in &mut self.runs {
            let removed_reference = run.remove_comment_references(ids);
            removed.push(
                removed_reference
                    && run.content.is_empty()
                    && run.extra_xml.is_empty()
                    && run.alt_drawings.is_empty(),
            );
        }
        if removed.iter().all(|remove| !remove) {
            return Ok(());
        }
        self.remove_runs(&removed)
    }

    /// Remove the direct runs flagged in `removed` and move every run-boundary
    /// projection onto the boundary that remains. Raw children, comment
    /// markers, bookmarks and controls of the boundaries that collapse into
    /// one keep their order, and a hyperlink or a ruby annotation left
    /// without runs is dropped. A hyperlink that still holds a marker or a
    /// tracked change stays where it was, after the children at its start.
    ///
    /// The paragraph is staged, so a refused source reservation leaves it
    /// exactly as it was rather than half-removed with an unfilled
    /// placeholder.
    pub(crate) fn remove_runs(&mut self, removed: &[bool]) -> Result<()> {
        let mut candidate = self.clone();
        candidate.remove_runs_core(removed, None)?;
        *self = candidate;
        Ok(())
    }

    /// Retained original source bytes the paragraph still owns, including run,
    /// hyperlink, revision and paragraph raw records. Read-only.
    pub(crate) fn retained_paragraph_source_bytes(&self) -> Result<usize> {
        let mut total = 0usize;
        let mut add = |bytes: usize| -> Result<()> {
            total = total.checked_add(bytes).ok_or_else(|| {
                OxmlError::InvalidValue("paragraph retained source byte overflow".into())
            })?;
            Ok(())
        };
        for (_, raw) in &self.extra_xml {
            add(raw.len())?;
        }
        for run in &self.runs {
            for raw in &run.extra_xml {
                add(raw.len())?;
            }
        }
        for hyperlink in &self.hyperlinks {
            for (_, _, raw) in &hyperlink.extra_xml {
                add(raw.len())?;
            }
        }
        for (_, _, revision) in &self.revisions {
            add(revision.retained_source_len())?;
        }
        Ok(total)
    }

    /// The unchanged private affected-source envelope for one collapsed-root
    /// mutation. It is derived from the paragraph's retained original source,
    /// the actual source scope and current typed facts, never from the
    /// replacement bytes the mutation would construct.
    fn collapsed_hyperlink_source_allowance(
        &self,
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<usize> {
        let source_bytes = self.retained_paragraph_source_bytes()?;
        let mut scope_bytes = 0usize;
        for (prefix, namespace) in source_context.unwrap_or(&[]) {
            scope_bytes = scope_bytes
                .checked_add(prefix.len())
                .and_then(|bytes| bytes.checked_add(namespace.len()))
                .ok_or_else(|| {
                    OxmlError::InvalidValue("collapsed hyperlink scope byte overflow".into())
                })?;
        }
        let mut current_bytes = 0usize;
        for run in &self.runs {
            current_bytes = current_bytes
                .checked_add(run.content.len())
                .ok_or_else(|| {
                    OxmlError::InvalidValue("collapsed hyperlink current byte overflow".into())
                })?;
            for content in &run.content {
                if let RunContent::Text(text) = content {
                    current_bytes =
                        current_bytes.checked_add(text.text.len()).ok_or_else(|| {
                            OxmlError::InvalidValue(
                                "collapsed hyperlink current byte overflow".into(),
                            )
                        })?;
                }
            }
        }
        for hyperlink in &self.hyperlinks {
            // A generated root and its matching end are bounded current facts,
            // so a replacement never derives its allowance from its expansion.
            current_bytes = current_bytes.checked_add(64).ok_or_else(|| {
                OxmlError::InvalidValue("collapsed hyperlink current byte overflow".into())
            })?;
            for fact in [
                hyperlink.rel_id.as_deref(),
                hyperlink.anchor.as_deref(),
                hyperlink.tooltip.as_deref(),
                hyperlink.doc_location.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                current_bytes = current_bytes.checked_add(fact.len()).ok_or_else(|| {
                    OxmlError::InvalidValue("collapsed hyperlink current byte overflow".into())
                })?;
            }
            for (name, value) in &hyperlink.extra_attributes {
                current_bytes = current_bytes
                    .checked_add(name.len())
                    .and_then(|bytes| bytes.checked_add(value.len()))
                    .ok_or_else(|| {
                        OxmlError::InvalidValue("collapsed hyperlink current byte overflow".into())
                    })?;
            }
        }
        affected_source_dynamic_work(scope_bytes.checked_add(current_bytes).ok_or_else(|| {
            OxmlError::InvalidValue("collapsed hyperlink envelope overflow".into())
        })?)
        .and_then(|work| work.checked_add(source_bytes))
        .ok_or_else(|| OxmlError::InvalidValue("collapsed hyperlink envelope overflow".into()))
    }

    /// One checked TOTAL estimate for every collapsed hyperlink replacement,
    /// computed before the first expanded construction. Generated root, end,
    /// declarations and typed attributes are counted with the retained raw
    /// children so the shared reservation can never under-count a child.
    fn collapsed_hyperlink_source_total(
        &self,
        collapsed: &[usize],
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<usize> {
        let mut total = 0usize;
        for index in collapsed {
            let hyperlink = &self.hyperlinks[*index];
            let word_prefix = safe_hyperlink_name(
                hyperlink,
                "w",
                "rdocxW",
                crate::namespace::W_NS,
                source_context,
                XmlWriteRole::Source,
            );
            let relationship_prefix = safe_hyperlink_name(
                hyperlink,
                "r",
                "rdocxR",
                R_NS,
                source_context,
                XmlWriteRole::Source,
            );
            let name_len = word_prefix.prefix_len() + 1 + "hyperlink".len();
            let mut bytes = 0usize;
            let mut add = |value: usize| -> Result<()> {
                bytes = bytes.checked_add(value).ok_or_else(|| {
                    OxmlError::InvalidValue("collapsed hyperlink growth overflow".into())
                })?;
                Ok(())
            };
            // `<word:hyperlink ...>` and `</word:hyperlink>`, including delimiters.
            add(2 + name_len)?;
            add(2 + name_len + 1)?;
            if word_prefix.declaration {
                add(1
                    + 6
                    + word_prefix.prefix_len()
                    + 2
                    + escaped_source_len(crate::namespace::W_NS)?
                    + 1)?;
            }
            if hyperlink.rel_id.is_some() && relationship_prefix.declaration {
                add(1 + 6 + relationship_prefix.prefix_len() + 2 + escaped_source_len(R_NS)? + 1)?;
            }
            if let Some(rel_id) = &hyperlink.rel_id {
                add(1
                    + relationship_prefix.prefix_len()
                    + 1
                    + 2
                    + 2
                    + escaped_source_len(rel_id)?
                    + 1)?;
            }
            if let Some(anchor) = &hyperlink.anchor {
                add(1 + word_prefix.prefix_len() + 1 + 6 + 2 + escaped_source_len(anchor)? + 1)?;
            }
            if let Some(tooltip) = &hyperlink.tooltip {
                add(1 + word_prefix.prefix_len() + 1 + 7 + 2 + escaped_source_len(tooltip)? + 1)?;
            }
            if let Some(doc_location) = &hyperlink.doc_location {
                add(1
                    + word_prefix.prefix_len()
                    + 1
                    + 11
                    + 2
                    + escaped_source_len(doc_location)?
                    + 1)?;
            }
            for (name, value) in &hyperlink.extra_attributes {
                add(name.len() + 2 + escaped_source_len(value)? + 1 + 1)?;
            }
            for (_, _, raw) in &hyperlink.extra_xml {
                add(raw.len())?;
            }
            // A revision-backed collapsed hyperlink emits every revision that
            // collapses onto its surviving boundary. Count those retained raw
            // bytes before the first replacement is constructed.
            for (at, slot, revision) in &self.revisions {
                if hyperlink_revision_index(*slot) == Some(*index)
                    && *at >= hyperlink.run_start
                    && *at <= hyperlink.run_end
                {
                    add(revision.retained_source_len())?;
                }
            }
            total = total.checked_add(bytes).ok_or_else(|| {
                OxmlError::InvalidValue("collapsed hyperlink total overflow".into())
            })?;
        }
        Ok(total)
    }

    pub(crate) fn remove_runs_core(
        &mut self,
        removed: &[bool],
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<()> {
        self.remove_runs_core_in(removed, source_context, None, None, None)
    }

    pub(crate) fn remove_runs_core_in(
        &mut self,
        removed: &[bool],
        source_context: Option<&[(&str, &str)]>,
        mut extra_maps: Option<&mut WordExtraSourceMaps>,
        mut work: Option<&mut WordOperationWork>,
        emission: Option<&FieldXmlEmission>,
    ) -> Result<()> {
        if extra_maps
            .as_ref()
            .is_some_and(|maps| maps.len() != self.extra_xml.len())
        {
            return Err(OxmlError::InvalidValue(
                "paragraph extra source map alignment changed".into(),
            ));
        }
        let removed_run_addresses = self
            .runs
            .iter()
            .zip(removed)
            .filter_map(|(run, remove)| remove.then_some(std::ptr::from_ref(run)))
            .collect::<Vec<_>>();
        let removed_projected_indices = accepted_paragraph_runs(self)
            .into_iter()
            .enumerate()
            .filter_map(|(index, run)| {
                removed_run_addresses
                    .contains(&std::ptr::from_ref(run))
                    .then_some(index)
            })
            .collect::<Vec<_>>();

        let old_run_count = self.runs.len();
        // A hyperlink keeping no run is written in place of a raw child at its
        // boundary. Its placeholder goes after the children at its start, so
        // the children at its end stay after it.
        let collapsed = (0..self.hyperlinks.len())
            .filter(|&index| {
                let hyperlink = &self.hyperlinks[index];
                hyperlink.preserved_raw_before.is_none()
                    && hyperlink.run_start < hyperlink.run_end
                    && hyperlink.run_end <= old_run_count
                    && removed[hyperlink.run_start..hyperlink.run_end]
                        .iter()
                        .all(|remove| *remove)
                    && (!hyperlink.extra_xml.is_empty()
                        || self
                            .revisions
                            .iter()
                            .any(|(_, slot, _)| hyperlink_revision_index(*slot) == Some(index)))
            })
            .collect::<Vec<_>>();
        // Admission precedes every mutation and every expanded replacement.
        // The collapsed-root growth and its retained raw children share one
        // operation-local reservation, so a later child cannot renew it.
        let mut collapsed_reservation = 0usize;
        if !collapsed.is_empty() {
            let needed = self.collapsed_hyperlink_source_total(&collapsed, source_context)?;
            let allowance = self.collapsed_hyperlink_source_allowance(source_context)?;
            if needed > allowance {
                return Err(OxmlError::InvalidValue(
                    "collapsed hyperlink source growth exceeds admitted source work".into(),
                ));
            }
            collapsed_reservation = needed;
        }
        for index in &collapsed {
            let start = self.hyperlinks[*index].run_start;
            let slot = self
                .extra_xml
                .iter()
                .filter(|(position, _)| *position == start)
                .count();
            self.extra_xml.push((start, Vec::new()));
            if let Some(maps) = extra_maps.as_deref_mut() {
                maps.push(None);
            }
            self.hyperlinks[*index].preserved_raw_before = Some(slot);
        }
        let boundary_map = (0..=old_run_count)
            .map(|boundary| {
                boundary
                    - removed
                        .iter()
                        .take(boundary)
                        .filter(|remove| **remove)
                        .count()
            })
            .collect::<Vec<_>>();
        let mut raw_counts = vec![0usize; old_run_count + 1];
        for (position, raw) in &self.extra_xml {
            if Self::raw_is_root_attributes(*position, raw) {
                continue;
            }
            raw_counts[(*position).min(old_run_count)] += 1;
        }
        let mut raw_prefixes = vec![0usize; old_run_count + 1];
        for boundary in 1..=old_run_count {
            if boundary_map[boundary] == boundary_map[boundary - 1] {
                raw_prefixes[boundary] = raw_prefixes[boundary - 1] + raw_counts[boundary - 1];
            }
        }

        if let Some(maps) = extra_maps {
            let mut paired = std::mem::take(&mut self.extra_xml)
                .into_iter()
                .zip(std::mem::take(maps))
                .collect::<Vec<_>>();
            paired.sort_by_key(|((position, _), _)| *position);
            (self.extra_xml, *maps) = paired.into_iter().unzip();
        } else {
            self.extra_xml.sort_by_key(|(position, _)| *position);
        }
        self.comment_ranges
            .sort_by_key(|marker| (marker.run_index(), marker.raw_before()));
        self.bookmark_markers.sort_by_key(|marker| marker.run_index);
        self.content_controls
            .sort_by_key(|(at, raw_before, markers_before, _)| (*at, *raw_before, *markers_before));

        for control_index in 0..self.content_controls.len() {
            let (run_index, raw_before, markers_before) = {
                let control = &self.content_controls[control_index];
                (control.0, control.1, control.2)
            };
            let old_boundary = run_index.min(old_run_count);
            let old_raw_slot = raw_before.min(raw_counts[old_boundary]);
            let new_boundary = boundary_map[old_boundary];
            let new_raw_slot = raw_prefixes[old_boundary] + old_raw_slot;
            let preceding_markers = self
                .comment_ranges
                .iter()
                .filter(|marker| {
                    let marker_boundary = marker.run_index().min(old_run_count);
                    marker_boundary < old_boundary
                        && boundary_map[marker_boundary] == new_boundary
                        && raw_prefixes[marker_boundary]
                            + marker.raw_before().min(raw_counts[marker_boundary])
                            == new_raw_slot
                })
                .count();
            let local_marker_count = self
                .comment_ranges
                .iter()
                .filter(|marker| {
                    marker.run_index() == old_boundary
                        && marker.raw_before().min(raw_counts[old_boundary]) == old_raw_slot
                })
                .count();
            let control = &mut self.content_controls[control_index];
            control.0 = new_boundary;
            control.1 = new_raw_slot;
            control.2 = preceding_markers + markers_before.min(local_marker_count);
        }
        for marker in &mut self.comment_ranges {
            match marker {
                CommentRangeMarker::Start {
                    run_index,
                    raw_before,
                    ..
                }
                | CommentRangeMarker::End {
                    run_index,
                    raw_before,
                    ..
                } => {
                    let old_boundary = (*run_index).min(old_run_count);
                    *run_index = boundary_map[old_boundary];
                    *raw_before =
                        raw_prefixes[old_boundary] + (*raw_before).min(raw_counts[old_boundary]);
                }
            }
        }
        for marker in &mut self.bookmark_markers {
            let old_boundary = marker.run_index.min(old_run_count);
            marker.run_index = boundary_map[old_boundary];
            marker.raw_before =
                raw_prefixes[old_boundary] + marker.raw_before.min(raw_counts[old_boundary]);
            marker.projected_run_index -= removed_projected_indices
                .iter()
                .filter(|index| **index < marker.projected_run_index)
                .count();
        }
        let hyperlink_revision_counts = self
            .hyperlinks
            .iter()
            .enumerate()
            .map(|(hyperlink_index, hyperlink)| {
                let start = hyperlink.run_start.min(old_run_count);
                let end = hyperlink.run_end.min(old_run_count);
                (start..=end)
                    .map(|boundary| {
                        self.revisions
                            .iter()
                            .filter(|(at, slot, _)| {
                                *at == boundary
                                    && hyperlink_revision_index(*slot) == Some(hyperlink_index)
                            })
                            .count()
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        for (run_index, raw_before, _) in &mut self.revisions {
            let old_boundary = (*run_index).min(old_run_count);
            *run_index = boundary_map[old_boundary];
            if hyperlink_revision_index(*raw_before).is_none() {
                *raw_before =
                    raw_prefixes[old_boundary] + (*raw_before).min(raw_counts[old_boundary]);
            }
        }
        for (position, raw) in &mut self.extra_xml {
            if Self::raw_is_root_attributes(*position, raw) {
                continue;
            }
            *position = boundary_map[(*position).min(old_run_count)];
        }
        for (position, raw_before, _) in &mut self.equations {
            let old_boundary = (*position).min(old_run_count);
            *position = boundary_map[old_boundary];
            *raw_before = raw_prefixes[old_boundary] + (*raw_before).min(raw_counts[old_boundary]);
        }
        for ruby in &mut self.rubies {
            ruby.base_start = boundary_map[ruby.base_start.min(old_run_count)];
            ruby.base_end = boundary_map[ruby.base_end.min(old_run_count)];
        }
        // An annotation over no base run is not written, so drop it.
        self.rubies.retain(|ruby| ruby.base_start < ruby.base_end);
        let old_hyperlinks = std::mem::take(&mut self.hyperlinks);
        let mut hyperlink_map = vec![None; old_hyperlinks.len()];
        for (old_index, mut hyperlink) in old_hyperlinks.into_iter().enumerate() {
            let old_start = hyperlink.run_start.min(old_run_count);
            let old_end = hyperlink.run_end.min(old_run_count);
            if let Some(raw_before) = hyperlink.preserved_raw_before {
                hyperlink.preserved_raw_before =
                    Some(raw_prefixes[old_start] + raw_before.min(raw_counts[old_start]));
            }
            let revision_counts = &hyperlink_revision_counts[old_index];
            let new_start = boundary_map[old_start];
            let new_end = boundary_map[old_end];
            for (boundary, revisions_before, _) in &mut hyperlink.extra_xml {
                let old_relative = (*boundary).min(old_end - old_start);
                let old_boundary = old_start + old_relative;
                let new_boundary = boundary_map[old_boundary];
                let collapsed_before = (0..old_relative)
                    .filter(|relative| boundary_map[old_start + relative] == new_boundary)
                    .map(|relative| revision_counts[relative])
                    .sum::<usize>();
                *boundary = new_boundary.saturating_sub(new_start);
                *revisions_before =
                    collapsed_before + (*revisions_before).min(revision_counts[old_relative]);
            }
            let owns_revision = self
                .revisions
                .iter()
                .any(|(_, slot, _)| hyperlink_revision_index(*slot) == Some(old_index));
            hyperlink.run_start = new_start;
            hyperlink.run_end = new_end;
            if new_start < new_end || owns_revision || !hyperlink.extra_xml.is_empty() {
                hyperlink_map[old_index] = Some(self.hyperlinks.len());
                self.hyperlinks.push(hyperlink);
            }
        }
        for (_, slot, _) in &mut self.revisions {
            let Some(old_index) = hyperlink_revision_index(*slot) else {
                continue;
            };
            if let Some(new_index) = hyperlink_map.get(old_index).copied().flatten() {
                *slot = hyperlink_revision_slot(new_index);
            }
        }
        self.runs = self
            .runs
            .drain(..)
            .zip(removed)
            .filter_map(|(run, remove)| (!*remove).then_some(run))
            .collect();
        for old_index in collapsed {
            let Some(index) = hyperlink_map[old_index] else {
                continue;
            };
            let hyperlink = &self.hyperlinks[index];
            let (boundary, slot) = (
                hyperlink.run_start,
                hyperlink.preserved_raw_before.unwrap_or_default(),
            );
            let record_context = ParagraphBoundary {
                role: XmlWriteRole::Source,
                emission,
                source_context,
                extra_xml: &self.extra_xml,
                extra_maps: None,
                content_controls: &self.content_controls,
                markers: &self.comment_ranges,
                hyperlinks: &self.hyperlinks,
                revisions: &self.revisions,
                equations: &self.equations,
            };
            let result = if let Some(emission) = emission {
                // A new complete record is distinct from an outer retained or
                // prepaid transport. Inner actual event prepayments may apply.
                let (output, _outer) = emission.new_source_record_output();
                let mut writer = Writer::new(output);
                write_hyperlink_record(
                    &mut writer,
                    &record_context,
                    index,
                    boundary,
                    work.as_deref_mut(),
                )
                .map(|()| writer.into_inner().into_mapped_parts().0)
            } else {
                let mut raw = Vec::new();
                write_hyperlink_record(
                    &mut Writer::new(&mut raw),
                    &record_context,
                    index,
                    boundary,
                    work.as_deref_mut(),
                )
                .map(|()| raw)
            };
            let (raw, succeeded) = match result {
                Ok(raw) => (raw, true),
                Err(error)
                    if work
                        .as_deref()
                        .is_some_and(WordOperationWork::resource_refused)
                        || emission.is_some_and(|emission| {
                            emission
                                .output
                                .lock()
                                .expect("normalization ledger poisoned")
                                .refused
                        }) =>
                {
                    return Err(match work.as_deref() {
                        Some(work) => work.writer_error(error),
                        None => error,
                    });
                }
                Err(error) if source_context.is_some() => return Err(error),
                Err(_) => (Vec::new(), false),
            };
            if succeeded {
                if raw.len() > collapsed_reservation {
                    return Err(OxmlError::InvalidValue(
                        "collapsed hyperlink replacement exceeds its reservation".into(),
                    ));
                }
                collapsed_reservation -= raw.len();
            }
            if succeeded
                && let Some((_, placeholder)) = self
                    .extra_xml
                    .iter_mut()
                    .filter(|(position, _)| *position == boundary)
                    .nth(slot)
            {
                *placeholder = raw;
            }
        }
        let _ = self.refresh_bookmark_projection();
        Ok(())
    }

    /// Insert a canonical bookmark start marker at a direct-run boundary.
    pub fn insert_bookmark_start(&mut self, run_index: usize, id: i32, name: &str) -> bool {
        if run_index > self.runs.len() {
            return false;
        }
        let mut value = itoa::Buffer::new();
        let mut element = BytesStart::new("w:bookmarkStart");
        element.push_attribute(("w:id", value.format(id)));
        element.push_attribute(("w:name", name));
        let mut raw = Vec::new();
        if Writer::new(&mut raw)
            .write_event(Event::Empty(element))
            .is_err()
        {
            return false;
        }
        self.extra_xml.push((run_index, raw));
        let projected = self.refresh_bookmark_projection();
        if !projected {
            self.extra_xml.pop();
        }
        projected
    }

    /// Insert a canonical bookmark end marker at a direct-run boundary.
    pub fn insert_bookmark_end(&mut self, run_index: usize, id: i32) -> bool {
        if run_index > self.runs.len() {
            return false;
        }
        let mut value = itoa::Buffer::new();
        let mut element = BytesStart::new("w:bookmarkEnd");
        element.push_attribute(("w:id", value.format(id)));
        let mut raw = Vec::new();
        if Writer::new(&mut raw)
            .write_event(Event::Empty(element))
            .is_err()
        {
            return false;
        }
        self.extra_xml.push((run_index, raw));
        let projected = self.refresh_bookmark_projection();
        if !projected {
            self.extra_xml.pop();
        }
        projected
    }

    /// Write range markers at accepted-view run boundaries, the run index
    /// space of [`Self::accepted_run_paths`].
    ///
    /// The markers go inside `w:sdtContent` when the range starts or ends
    /// between two runs of an inline content control, and around the whole
    /// control when the range covers it. A missing side means the range
    /// continues into another paragraph, so that side must not fall inside a
    /// control. An edge between two runs of a tracked insertion or move
    /// destination goes inside its wrapper. A comment reference run follows
    /// the comment end marker, or the wrapper when only the end is inside
    /// one. A range that cannot be written exactly is refused and the
    /// paragraph is unchanged.
    #[doc(hidden)]
    pub fn anchor_accepted_range(
        &mut self,
        start: Option<usize>,
        end: Option<usize>,
        anchor: RangeAnchor<'_>,
    ) -> std::result::Result<(), RangeAnchorError> {
        self.anchor_range_with_reference(start, end, anchor, true)
    }

    /// [`Self::anchor_accepted_range`], with or without the comment reference
    /// run after the end marker.
    fn anchor_range_with_reference(
        &mut self,
        start: Option<usize>,
        end: Option<usize>,
        anchor: RangeAnchor<'_>,
        reference: bool,
    ) -> std::result::Result<(), RangeAnchorError> {
        if let Some(anchored) = self.anchor_inside_revisions(start, end, anchor, reference) {
            return anchored;
        }
        let (start_site, end_site) = self.accepted_range_sites(start, end)?;
        let mut paragraph = self.clone();
        // The end goes first. A start site never follows it, so the end and
        // its reference run leave the start site where it was.
        if let Some(site) = end_site {
            paragraph.insert_range_marker(site, anchor, false, reference)?;
        }
        if let Some(site) = start_site {
            paragraph.insert_range_marker(site, anchor, true, reference)?;
        }
        if !paragraph.refresh_bookmark_projection() {
            return Err(RangeAnchorError::Write);
        }
        *self = paragraph;
        Ok(())
    }

    /// Write range markers when an edge falls between two runs of one tracked
    /// insertion or move destination, as Word does: inside the wrapper, which
    /// keeps its identifier, author and date, with a comment reference run
    /// after the end marker. Each edge is placed on its own, so a range may
    /// start inside one insertion and end outside it. When only the end is
    /// inside, the reference run follows the wrapper instead: rejecting the
    /// insertion then keeps the comment mark of a range that survives.
    /// `None` when no edge falls inside a revision.
    fn anchor_inside_revisions(
        &mut self,
        start: Option<usize>,
        end: Option<usize>,
        anchor: RangeAnchor<'_>,
        reference: bool,
    ) -> Option<std::result::Result<(), RangeAnchorError>> {
        let paths = self.accepted_run_paths();
        let run_count = paths.len();
        // The revision holding the runs on both sides of a boundary, and the
        // boundary among the accepted runs of that revision.
        let inside = |boundary: usize| {
            let left = paths.get(boundary.checked_sub(1)?)?.segments()[0];
            let right = paths.get(boundary)?.segments()[0];
            match (left, right) {
                (AcceptedRunPathSegment::Revision(a), AcceptedRunPathSegment::Revision(b))
                    if a == b =>
                {
                    let first = paths.iter().position(|path| path.segments()[0] == left)?;
                    Some((a, first))
                }
                _ => None,
            }
        };
        let (start_inside, end_inside) = (start.and_then(inside), end.and_then(inside));
        if start_inside.is_none() && end_inside.is_none() {
            return None;
        }
        Some(
            (|| {
                for boundary in [start, end].into_iter().flatten() {
                    if boundary > run_count {
                        return Err(RangeAnchorError::OutOfRange {
                            boundary,
                            run_count,
                        });
                    }
                }
                if let (Some(start), Some(end)) = (start, end)
                    && start > end
                {
                    return Err(RangeAnchorError::Reversed { start, end });
                }
                let mut paragraph = self.clone();
                // The edge outside the insertion is placed alone, as if the
                // range continued into another paragraph; within this one, a
                // control it falls in is crossed by the range.
                let crossing = |error| match (error, start, end) {
                    (RangeAnchorError::InsideControl { .. }, Some(start), Some(end)) => {
                        RangeAnchorError::CrossesControl { start, end }
                    }
                    (error, _, _) => error,
                };
                match (start, end, start_inside, end_inside) {
                    (Some(start), Some(end), Some((revision, first)), Some((other, _)))
                        if revision == other =>
                    {
                        paragraph.anchor_in_revision(
                            revision,
                            first,
                            Some(start - first),
                            Some(end - first),
                            anchor,
                            reference,
                        )?;
                    }
                    _ => {
                        // The end goes first: its reference run comes after the
                        // start, which keeps its boundary.
                        if let Some(end) = end {
                            match end_inside {
                                Some((revision, first)) => {
                                    // Just after the wrapper, which the paths of
                                    // this unchanged paragraph give. A wrapper in
                                    // a hyperlink with runs of its own may have
                                    // no such place: the mark then stays inside.
                                    let after = match anchor {
                                        RangeAnchor::Comment(id) if reference => {
                                            match self.marker_site(&paths, &[], end, false) {
                                                Ok(MarkerSite::Paragraph {
                                                    boundary,
                                                    position,
                                                }) => Some((id, boundary, position)),
                                                _ => None,
                                            }
                                        }
                                        _ => None,
                                    };
                                    paragraph.anchor_in_revision(
                                        revision,
                                        first,
                                        None,
                                        Some(end - first),
                                        anchor,
                                        reference && after.is_none(),
                                    )?;
                                    if let Some((id, boundary, position)) = after
                                        && !paragraph.insert_run_after_boundary_items(
                                            boundary,
                                            position,
                                            comment_reference_run(id),
                                        )
                                    {
                                        return Err(RangeAnchorError::Write);
                                    }
                                }
                                None => paragraph
                                    .anchor_range_with_reference(None, Some(end), anchor, reference)
                                    .map_err(crossing)?,
                            }
                        }
                        if let Some(start) = start {
                            match start_inside {
                                Some((revision, first)) => paragraph.anchor_in_revision(
                                    revision,
                                    first,
                                    Some(start - first),
                                    None,
                                    anchor,
                                    reference,
                                )?,
                                None => paragraph
                                    .anchor_range_with_reference(
                                        Some(start),
                                        None,
                                        anchor,
                                        reference,
                                    )
                                    .map_err(crossing)?,
                            }
                        }
                    }
                }
                if !paragraph.refresh_bookmark_projection() {
                    return Err(RangeAnchorError::Write);
                }
                Ok(paragraph)
            })()
            .map(|paragraph| *self = paragraph),
        )
    }

    /// Anchor a range among the accepted runs of revision `revision`, whose
    /// first run is accepted run `first` of this paragraph.
    fn anchor_in_revision(
        &mut self,
        revision: usize,
        first: usize,
        start: Option<usize>,
        end: Option<usize>,
        anchor: RangeAnchor<'_>,
        reference: bool,
    ) -> std::result::Result<(), RangeAnchorError> {
        let (_, _, wrapper) = self
            .revisions
            .get_mut(revision)
            .ok_or(RangeAnchorError::Write)?;
        // A wrapper whose content cannot be written back keeps the refusal.
        let refused = RangeAnchorError::InsideRevision {
            boundary: start.or(end).unwrap_or_default() + first,
        };
        let mut anchored = Ok(());
        let edited = wrapper
            .edit_accepted_paragraph(|paragraph| {
                anchored = paragraph.anchor_range_with_reference(start, end, anchor, reference);
                Ok(anchored.is_ok())
            })
            .map_err(|_| refused)?;
        // Report the boundaries of this paragraph, not of the revision.
        anchored.map_err(|error| match error {
            RangeAnchorError::OutOfRange {
                boundary,
                run_count,
            } => RangeAnchorError::OutOfRange {
                boundary: boundary + first,
                run_count: run_count + first,
            },
            RangeAnchorError::Reversed { start, end } => RangeAnchorError::Reversed {
                start: start + first,
                end: end + first,
            },
            RangeAnchorError::CrossesControl { start, end } => RangeAnchorError::CrossesControl {
                start: start + first,
                end: end + first,
            },
            RangeAnchorError::InsideControl { boundary } => RangeAnchorError::InsideControl {
                boundary: boundary + first,
            },
            RangeAnchorError::InsideRevision { boundary } => RangeAnchorError::InsideRevision {
                boundary: boundary + first,
            },
            RangeAnchorError::HyperlinkRevision { boundary } => {
                RangeAnchorError::HyperlinkRevision {
                    boundary: boundary + first,
                }
            }
            RangeAnchorError::Write => RangeAnchorError::Write,
        })?;
        if !edited {
            return Err(refused);
        }
        Ok(())
    }

    /// Give the reference run of comment `id` that
    /// [`Self::anchor_accepted_range`] wrote, a run with nothing but the
    /// reference, the character style `style_id`, as Word styles a comment
    /// mark. Returns whether such a run was found.
    #[doc(hidden)]
    pub fn style_comment_reference_run(&mut self, id: i32, style_id: &str) -> bool {
        // A run inside a tracked insertion is written back through its
        // wrapper, or the style would not reach the saved XML.
        for (_, _, revision) in &mut self.revisions {
            let mut styled = false;
            let edited = revision.edit_accepted_paragraph(|paragraph| {
                styled = paragraph.style_comment_reference_run(id, style_id);
                Ok(styled)
            });
            if styled {
                return edited.is_ok_and(|edited| edited);
            }
        }
        for path in self.accepted_run_paths() {
            let Some(run) = self.accepted_run_segments_mut(path.segments()) else {
                continue;
            };
            if run.properties.is_none()
                && run.extra_xml.is_empty()
                && matches!(
                    run.content.as_slice(),
                    [RunContent::CommentReference { id: found, .. }] if *found == id
                )
            {
                run.properties = Some(CT_RPr {
                    style_id: Some(style_id.to_owned()),
                    ..Default::default()
                });
                return true;
            }
        }
        false
    }

    /// Insert one run at a checked accepted-view boundary atomically.
    #[doc(hidden)]
    pub fn insert_accepted_run(
        &mut self,
        boundary: usize,
        run: CT_R,
    ) -> std::result::Result<(), RangeAnchorError> {
        let (site, _) = self.accepted_range_sites(Some(boundary), Some(boundary))?;
        let mut paragraph = self.clone();
        let inserted = match site.ok_or(RangeAnchorError::Write)? {
            MarkerSite::Control { controls, index } => paragraph
                .control_at_mut(&controls)
                .is_some_and(|control| control.insert_content(index, vec![SdtContent::Run(run)])),
            MarkerSite::Paragraph { boundary, position } => {
                paragraph.insert_run_after_boundary_items(boundary, position, run)
            }
        };
        if !inserted || !paragraph.refresh_bookmark_projection() {
            return Err(RangeAnchorError::Write);
        }
        *self = paragraph;
        Ok(())
    }

    /// Resolve where the start and end markers of a range go.
    fn accepted_range_sites(
        &self,
        start: Option<usize>,
        end: Option<usize>,
    ) -> std::result::Result<(Option<MarkerSite>, Option<MarkerSite>), RangeAnchorError> {
        let paths = self.accepted_run_paths();
        let run_count = paths.len();
        for boundary in [start, end].into_iter().flatten() {
            if boundary > run_count {
                return Err(RangeAnchorError::OutOfRange {
                    boundary,
                    run_count,
                });
            }
        }
        // The recursive owner of a run is its path without the final run step.
        let owner = |index: usize| {
            let segments = paths[index].segments();
            &segments[..segments.len() - 1]
        };
        // The deepest owner holding the runs on both sides of a boundary.
        let shared = |boundary: usize| match boundary.checked_sub(1) {
            Some(left) if boundary < run_count => {
                &owner(left)[..common_prefix_len(owner(left), owner(boundary))]
            }
            _ => &[] as &[AcceptedRunPathSegment],
        };
        let (owner_path, blamed) = match (start, end) {
            (Some(start), Some(end)) if start > end => {
                return Err(RangeAnchorError::Reversed { start, end });
            }
            (Some(start), Some(end)) if start == end => (shared(start), start),
            (Some(start), Some(end)) => {
                // The outermost owner that holds the first and the last run
                // of the range and reaches both boundaries.
                let (start_depth, end_depth) = (shared(start).len(), shared(end).len());
                let depth = start_depth.max(end_depth);
                let first = owner(start);
                if depth > common_prefix_len(first, owner(end - 1)) {
                    return Err(RangeAnchorError::CrossesControl { start, end });
                }
                let blamed = if start_depth >= end_depth { start } else { end };
                (&first[..depth], blamed)
            }
            (Some(boundary), None) | (None, Some(boundary)) => {
                let shared = shared(boundary);
                if shared
                    .iter()
                    .any(|segment| matches!(segment, AcceptedRunPathSegment::Revision(_)))
                {
                    return Err(RangeAnchorError::InsideRevision { boundary });
                } else if !shared.is_empty() {
                    return Err(RangeAnchorError::InsideControl { boundary });
                }
                (shared, boundary)
            }
            (None, None) => return Ok((None, None)),
        };
        let mut controls = Vec::with_capacity(owner_path.len());
        for segment in owner_path {
            match *segment {
                AcceptedRunPathSegment::ContentControl(index) => controls.push(index),
                AcceptedRunPathSegment::Revision(_) | AcceptedRunPathSegment::Run(_) => {
                    return Err(RangeAnchorError::InsideRevision { boundary: blamed });
                }
            }
        }

        let end_site = end
            .map(|boundary| self.marker_site(&paths, &controls, boundary, false))
            .transpose()?;
        let start_site = match (start, end) {
            (Some(start), Some(end)) if start == end => end_site.clone(),
            _ => start
                .map(|boundary| self.marker_site(&paths, &controls, boundary, true))
                .transpose()?,
        };
        Ok((start_site, end_site))
    }

    /// Place a marker just before the run after `boundary` for a start, or
    /// just after the run before it for an end, inside the owner `controls`.
    fn marker_site(
        &self,
        paths: &[AcceptedRunPath],
        controls: &[usize],
        boundary: usize,
        start: bool,
    ) -> std::result::Result<MarkerSite, RangeAnchorError> {
        if !controls.is_empty() {
            let path = if start {
                paths.get(boundary)
            } else {
                boundary.checked_sub(1).map(|index| &paths[index])
            }
            .ok_or(RangeAnchorError::Write)?;
            let control = self.control_at(controls).ok_or(RangeAnchorError::Write)?;
            let index = match path.segments()[controls.len()] {
                AcceptedRunPathSegment::Run(index)
                | AcceptedRunPathSegment::ContentControl(index) => index,
                AcceptedRunPathSegment::Revision(index) => {
                    control
                        .revisions()
                        .get(index)
                        .ok_or(RangeAnchorError::Write)?
                        .0
                }
            };
            return Ok(MarkerSite::Control {
                controls: controls.to_vec(),
                index: index + usize::from(!start),
            });
        }
        // A paragraph-level marker must fall between the top-level owners of
        // both neighbouring runs, which a revision inside a hyperlink can
        // prevent.
        let before_right = paths
            .get(boundary)
            .map(|path| self.paragraph_owner_positions(path).0);
        let after_left = boundary
            .checked_sub(1)
            .map(|index| self.paragraph_owner_positions(&paths[index]).1);
        let (Some(before_right), Some(after_left)) = (
            before_right.unwrap_or(Some((
                self.runs.len(),
                self.boundary_items(self.runs.len()).len(),
            ))),
            after_left.unwrap_or(Some((0, 0))),
        ) else {
            return Err(RangeAnchorError::HyperlinkRevision { boundary });
        };
        let (boundary, position) = if start { before_right } else { after_left };
        Ok(MarkerSite::Paragraph { boundary, position })
    }

    /// Positions just before and just after the paragraph-level owner of one
    /// accepted run, or `None` where no paragraph-level child can go.
    fn paragraph_owner_positions(
        &self,
        path: &AcceptedRunPath,
    ) -> (Option<ChildPosition>, Option<ChildPosition>) {
        let around = |boundary: usize, item: BoundaryItem| {
            let items = self.boundary_items(boundary);
            match items.iter().position(|candidate| *candidate == item) {
                Some(position) => (Some((boundary, position)), Some((boundary, position + 1))),
                None => (None, None),
            }
        };
        let raw_at = |boundary: usize, slot: usize| {
            self.extra_xml
                .iter()
                .enumerate()
                .filter(|(_, (position, _))| *position == boundary)
                .nth(slot)
                .map(|(index, _)| BoundaryItem::Raw(index))
        };
        match path.segments()[0] {
            AcceptedRunPathSegment::Run(index) => (
                Some((index, self.boundary_items(index).len())),
                Some((index + 1, 0)),
            ),
            AcceptedRunPathSegment::ContentControl(index) => self
                .content_controls
                .get(index)
                .map_or((None, None), |(boundary, _, _, _)| {
                    around(*boundary, BoundaryItem::Control(index))
                }),
            AcceptedRunPathSegment::Revision(index) => {
                let Some((boundary, slot, _)) = self.revisions.get(index) else {
                    return (None, None);
                };
                let boundary = *boundary;
                let Some(hyperlink) = hyperlink_revision_index(*slot) else {
                    return raw_at(boundary, *slot)
                        .map_or((None, None), |item| around(boundary, item));
                };
                match self.hyperlinks.get(hyperlink) {
                    Some(hyperlink) if hyperlink.preserved_raw_before.is_some() => {
                        raw_at(boundary, hyperlink.preserved_raw_before.unwrap_or_default())
                            .map_or((None, None), |item| around(boundary, item))
                    }
                    // A closing hyperlink writes its revision before the
                    // paragraph children at its end boundary, and an open
                    // one writes it after them.
                    Some(hyperlink)
                        if hyperlink.run_start < hyperlink.run_end
                            && boundary == hyperlink.run_end =>
                    {
                        (None, Some((boundary, 0)))
                    }
                    Some(hyperlink) if hyperlink.run_start < hyperlink.run_end => {
                        (Some((boundary, self.boundary_items(boundary).len())), None)
                    }
                    _ => (None, None),
                }
            }
        }
    }

    /// Reach the inline control that `controls` names.
    fn control_at(&self, controls: &[usize]) -> Option<&CT_Sdt> {
        let (first, rest) = controls.split_first()?;
        let mut control = &self.content_controls.get(*first)?.3;
        for index in rest {
            let SdtContent::ContentControl(nested) = control.content.get(*index)? else {
                return None;
            };
            control = nested;
        }
        Some(control)
    }

    fn control_at_mut(&mut self, controls: &[usize]) -> Option<&mut CT_Sdt> {
        let (first, rest) = controls.split_first()?;
        let mut control = &mut self.content_controls.get_mut(*first)?.3;
        for index in rest {
            let SdtContent::ContentControl(nested) = control.content.get_mut(*index)? else {
                return None;
            };
            control = nested;
        }
        Some(control)
    }

    fn insert_range_marker(
        &mut self,
        site: MarkerSite,
        anchor: RangeAnchor<'_>,
        start: bool,
        with_reference: bool,
    ) -> std::result::Result<(), RangeAnchorError> {
        let marker_xml = range_marker_xml(anchor, start).ok_or(RangeAnchorError::Write)?;
        let reference = match anchor {
            RangeAnchor::Comment(id) if !start && with_reference => Some(comment_reference_run(id)),
            RangeAnchor::Comment(_) if !start => None,
            RangeAnchor::Comment(_)
            | RangeAnchor::Bookmark { .. }
            | RangeAnchor::Permission { .. }
            | RangeAnchor::Proofing { .. } => None,
        };
        let inserted = match site {
            MarkerSite::Control { controls, index } => {
                let mut children = vec![SdtContent::RawXml(marker_xml)];
                children.extend(reference.map(SdtContent::Run));
                self.control_at_mut(&controls)
                    .is_some_and(|control| control.insert_content(index, children))
            }
            MarkerSite::Paragraph { boundary, position } => {
                let mut items = self.boundary_items(boundary);
                let item = match anchor {
                    RangeAnchor::Comment(id) => {
                        self.comment_ranges.push(if start {
                            CommentRangeMarker::Start {
                                id,
                                run_index: boundary,
                                raw_before: 0,
                                has_child_content: false,
                            }
                        } else {
                            CommentRangeMarker::End {
                                id,
                                run_index: boundary,
                                raw_before: 0,
                                has_child_content: false,
                            }
                        });
                        BoundaryItem::Marker(self.comment_ranges.len() - 1)
                    }
                    RangeAnchor::Bookmark { .. }
                    | RangeAnchor::Permission { .. }
                    | RangeAnchor::Proofing { .. } => {
                        self.extra_xml.push((boundary, marker_xml));
                        BoundaryItem::Raw(self.extra_xml.len() - 1)
                    }
                };
                items.insert(position.min(items.len()), item);
                self.place_boundary_items(&[(boundary, items)]);
                match reference {
                    Some(run) => self.insert_run_after_boundary_items(boundary, position + 1, run),
                    None => true,
                }
            }
        };
        if inserted {
            Ok(())
        } else {
            Err(RangeAnchorError::Write)
        }
    }

    /// List the paragraph-level children at one direct run boundary in the
    /// order that serialization writes them.
    fn boundary_items(&self, boundary: usize) -> Vec<BoundaryItem> {
        let raws = self
            .extra_xml
            .iter()
            .enumerate()
            .filter(|(_, (position, _))| *position == boundary)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let mut items = Vec::new();
        for raw_slot in 0..=raws.len() {
            let markers = self
                .comment_ranges
                .iter()
                .enumerate()
                .filter(|(_, marker)| {
                    marker.run_index() == boundary
                        && marker.raw_before().min(raws.len()) == raw_slot
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            for marker_slot in 0..=markers.len() {
                items.extend(
                    self.content_controls
                        .iter()
                        .enumerate()
                        .filter(|(_, (at, raw_before, markers_before, _))| {
                            *at == boundary
                                && (*raw_before).min(raws.len()) == raw_slot
                                && (*markers_before).min(markers.len()) == marker_slot
                        })
                        .map(|(index, _)| BoundaryItem::Control(index)),
                );
                if let Some(marker) = markers.get(marker_slot) {
                    items.push(BoundaryItem::Marker(*marker));
                }
            }
            if let Some(raw) = raws.get(raw_slot) {
                items.push(BoundaryItem::Raw(*raw));
            }
        }
        items
    }

    /// Give each listed direct run boundary exactly its listed children in
    /// order, and move every projection that names a moved raw child.
    fn place_boundary_items(&mut self, boundaries: &[(usize, Vec<BoundaryItem>)]) {
        let mut raw_moves = Vec::new();
        let mut placed_raws = Vec::new();
        let mut placed_markers = Vec::new();
        for (boundary, items) in boundaries {
            let (mut raws, mut markers) = (0, 0);
            for item in items {
                match *item {
                    BoundaryItem::Control(index) => {
                        let control = &mut self.content_controls[index];
                        (control.0, control.1, control.2) = (*boundary, raws, markers);
                    }
                    BoundaryItem::Marker(index) => {
                        match &mut self.comment_ranges[index] {
                            CommentRangeMarker::Start {
                                run_index,
                                raw_before,
                                ..
                            }
                            | CommentRangeMarker::End {
                                run_index,
                                raw_before,
                                ..
                            } => (*run_index, *raw_before) = (*boundary, raws),
                        }
                        markers += 1;
                        placed_markers.push(index);
                    }
                    BoundaryItem::Raw(index) => {
                        let old_boundary = self.extra_xml[index].0;
                        let old_slot = self.extra_xml[..index]
                            .iter()
                            .filter(|(position, _)| *position == old_boundary)
                            .count();
                        raw_moves.push(((old_boundary, old_slot), (*boundary, raws)));
                        placed_raws.push(index);
                        raws += 1;
                        markers = 0;
                    }
                }
            }
        }
        // Serialization reads the raw children and markers of one boundary in
        // vector order, so the placed ones move to the end in list order.
        let mut extra_xml = std::mem::take(&mut self.extra_xml)
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>();
        let placed = placed_raws
            .iter()
            .zip(&raw_moves)
            .filter_map(|(index, (_, (boundary, _)))| {
                extra_xml[*index].take().map(|(_, raw)| (*boundary, raw))
            })
            .collect::<Vec<_>>();
        self.extra_xml = extra_xml.into_iter().flatten().chain(placed).collect();
        let mut comment_ranges = std::mem::take(&mut self.comment_ranges)
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>();
        let placed = placed_markers
            .iter()
            .filter_map(|index| comment_ranges[*index].take())
            .collect::<Vec<_>>();
        self.comment_ranges = comment_ranges.into_iter().flatten().chain(placed).collect();

        let moved = |at: usize, slot: usize| {
            raw_moves
                .iter()
                .find(|(old, _)| *old == (at, slot))
                .map(|(_, new)| *new)
        };
        let mut moved_hyperlinks = Vec::new();
        for (hyperlink_index, hyperlink) in self.hyperlinks.iter_mut().enumerate() {
            if hyperlink.run_start == hyperlink.run_end
                && let Some(slot) = hyperlink.preserved_raw_before
                && let Some((boundary, slot)) = moved(hyperlink.run_start, slot)
            {
                moved_hyperlinks.push((hyperlink_index, hyperlink.run_start, boundary));
                (hyperlink.run_start, hyperlink.run_end) = (boundary, boundary);
                hyperlink.preserved_raw_before = Some(slot);
            }
        }
        for (at, slot, _) in &mut self.revisions {
            if let Some(hyperlink) = hyperlink_revision_index(*slot) {
                if let Some((_, _, boundary)) = moved_hyperlinks
                    .iter()
                    .find(|(index, old, _)| *index == hyperlink && *old == *at)
                {
                    *at = *boundary;
                }
            } else if let Some(new) = moved(*at, *slot) {
                (*at, *slot) = new;
            }
        }
        for (at, slot, _) in &mut self.equations {
            if let Some(new) = moved(*at, *slot) {
                (*at, *slot) = new;
            }
        }
    }

    /// Insert a direct run at `boundary` after the first `kept` children
    /// there, so the remaining children follow the new run.
    fn insert_run_after_boundary_items(&mut self, boundary: usize, kept: usize, run: CT_R) -> bool {
        let items = self.boundary_items(boundary);
        // The insertion moves every child of the boundary after the new run.
        if !self.insert_unwrapped_run(boundary, run) {
            return false;
        }
        let (before, after) = items.split_at(kept.min(items.len()));
        if !before.is_empty() {
            self.place_boundary_items(&[
                (boundary, before.to_vec()),
                (boundary + 1, after.to_vec()),
            ]);
        }
        true
    }

    /// Remap facade-authored bookmark marker ids without reserializing any
    /// unrelated preserved child XML.
    #[doc(hidden)]
    pub fn remap_authored_bookmark_ids(
        &mut self,
        remap: &std::collections::HashMap<i32, i32>,
    ) -> bool {
        for (_, raw) in &mut self.extra_xml {
            remap_authored_bookmark_marker(raw, remap);
        }
        // Markers anchored inside an inline control are its content children.
        for (_, _, _, control) in &mut self.content_controls {
            control.remap_authored_bookmark_ids(remap);
        }
        self.refresh_bookmark_projection()
    }

    fn refresh_bookmark_projection(&mut self) -> bool {
        let mut word_prefixes = vec![
            "w".to_owned(),
            format!("\0r\0{R_NS}"),
            format!("\0mc\0{}", crate::namespace::MC_NS),
        ];
        // Inline controls keep the source prefix of their preserved runs,
        // which the isolated paragraph no longer declares.
        for prefix in self
            .bookmark_markers
            .iter()
            .flat_map(|marker| marker.word_prefixes.iter())
            .chain(
                self.content_controls
                    .iter()
                    .flat_map(|(_, _, _, control)| control.word_prefixes()),
            )
        {
            if !word_prefixes.contains(prefix) {
                word_prefixes.push(prefix.clone());
            }
        }
        let mut raw = Vec::new();
        if self.to_xml(&mut Writer::new(&mut raw)).is_err() {
            return false;
        }
        let mut reader = Reader::from_reader(raw.as_slice());
        reader.config_mut().trim_text(false);
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Start(start)) if matches_local_name(start.name().as_ref(), b"p") => {
                    let Ok(parsed) = CT_P::from_xml_with_prefixes(&mut reader, &word_prefixes)
                    else {
                        return false;
                    };
                    self.bookmark_markers = parsed.bookmark_markers;
                    return true;
                }
                Ok(Event::Eof) | Err(_) => return false,
                Ok(_) => {}
            }
            buffer.clear();
        }
    }

    /// Validate borrowed owners before cloning or serializing this paragraph.
    #[doc(hidden)]
    pub fn validate_resource_limits(&self) -> Result<()> {
        crate::document::admit_word_model(crate::document::WordOwner::Paragraph(self))
    }

    /// Each untouched field span's source bytes, with the bytes that write its
    /// split runs as separate physical runs.
    #[doc(hidden)]
    pub fn detached_field_spans(&self) -> Result<Vec<(&[u8], Vec<u8>)>> {
        let mut spans = Vec::new();
        let mut run_idx = 0;
        while run_idx < self.runs.len() {
            let Some((end, raw, true, _)) = self.field_span_at(run_idx) else {
                run_idx += 1;
                continue;
            };
            spans.push((raw, self.detached_field_span(run_idx..end)?));
            run_idx = end;
        }
        Ok(spans)
    }

    /// Materialized source runs outside the fields of one intact physical span.
    ///
    /// The positive modeled coverage equals the vector length. Field entries
    /// are `None`. Outside entries keep their actual run shell and properties,
    /// rather than serializing the typed projection. These operation-local
    /// fragments inherit the containing paragraph's source namespace scope.
    /// An absent, changed or unproved span returns `None`.
    #[doc(hidden)]
    pub fn field_source_run_fragments_at(
        &self,
        run_idx: usize,
    ) -> Result<Option<(usize, PhysicalRunSources)>> {
        let Some((end, raw, _, _)) = self.field_span_at(run_idx) else {
            return Ok(None);
        };
        let runs = &self.runs[run_idx..end];
        let Some(field) = runs.iter().find_map(|run| match run.content.as_slice() {
            [RunContent::Field(field)] => Some(field),
            _ => None,
        }) else {
            return Ok(None);
        };
        let FieldSource::Parsed { word_prefixes, .. } = field.source.as_ref() else {
            return Ok(None);
        };
        let Some(parts) = field_span_parts(raw, word_prefixes, None, None)? else {
            return Ok(None);
        };
        if parts.len() != runs.len() {
            return Ok(None);
        }
        let mut fragments = Vec::with_capacity(parts.len());
        for (part, run) in parts.into_iter().zip(runs) {
            match part {
                FieldSpanPart::Outside(original, ranges) if original == *run => {
                    fragments.push(Some(FieldSpanPart::materialize_source(raw, &ranges)?));
                }
                FieldSpanPart::Field(_)
                    if matches!(run.content.as_slice(), [RunContent::Field(_)]) =>
                {
                    fragments.push(None);
                }
                _ => return Ok(None),
            }
        }
        Ok(Some((end - run_idx, fragments)))
    }

    /// Append the field source replacements that start at run `run_idx`, and
    /// return the number of runs they cover.
    ///
    /// A field read from a run it shared with text is replaced together with
    /// that text, as one physical span, so every field of the span can change.
    #[doc(hidden)]
    pub fn field_source_replacements_at(
        &self,
        run_idx: usize,
        output: &mut Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Result<usize> {
        let mut work = WordOperationWork::model_fields(self.writer_field_roots())?;
        self.field_source_replacements_at_in(run_idx, output, &mut work)
    }

    pub(crate) fn field_source_replacements_at_in(
        &self,
        run_idx: usize,
        output: &mut Vec<(Vec<u8>, Vec<u8>)>,
        work: &mut WordOperationWork,
    ) -> Result<usize> {
        if let Some((end, raw, untouched, _)) = self.field_span_at(run_idx) {
            let replacement = if untouched {
                raw.to_vec()
            } else {
                self.detached_field_span_in(run_idx..end, work)?
            };
            output.push((raw.to_vec(), replacement));
            return Ok(end - run_idx);
        }
        for content in self.runs.get(run_idx).map_or(&[][..], |run| &run.content) {
            let RunContent::Field(field) = content else {
                continue;
            };
            let (source, replacement) = field.source_replacement_in(work)?.ok_or_else(|| {
                OxmlError::MissingElement("the source of a parsed field".to_owned())
            })?;
            output.push((source.to_vec(), replacement));
        }
        Ok(1)
    }

    /// Write the runs of one field span as separate physical runs.
    fn detached_field_span(&self, runs: std::ops::Range<usize>) -> Result<Vec<u8>> {
        let mut work = WordOperationWork::model_fields(self.writer_field_roots())?;
        self.detached_field_span_in(runs, &mut work)
    }

    fn detached_field_span_in(
        &self,
        runs: std::ops::Range<usize>,
        work: &mut WordOperationWork,
    ) -> Result<Vec<u8>> {
        let _source_scope = root_binding_scope(0);
        let owner = self
            .field_span_at(runs.start)
            .map(|(_, _, _, field)| field)
            .ok_or_else(|| OxmlError::MissingElement("detached span owner".into()))?;
        let context = match owner.source.as_ref() {
            FieldSource::Parsed { word_prefixes, .. } => field_scope_bindings(word_prefixes),
            FieldSource::New { .. } => Vec::new(),
        };
        let mut writer = Writer::new(Vec::new());
        for run in &self.runs[runs] {
            match run.content.as_slice() {
                [RunContent::Field(field)] => write_run_field(
                    &mut writer,
                    field,
                    None,
                    run.properties.as_ref(),
                    None,
                    work,
                )?,
                _ => run.write_xml_role(
                    &mut writer,
                    None,
                    XmlWriteRole::Source,
                    Some(&context),
                    Some(&work.field_xml_emission()),
                    work.template_word_capture(),
                )?,
            }
        }
        Ok(writer.into_inner())
    }

    /// The field span whose runs start at `run_idx`, while they are still the
    /// runs the reader split it into: its end, source bytes, and whether its
    /// fields are unchanged too.
    ///
    /// The reader splits a run shared by a field and text into sibling runs.
    /// While those runs, their fields and the boundaries between them are as
    /// read, the span is written back as its original bytes.
    fn field_span_at(&self, run_idx: usize) -> Option<(usize, &[u8], bool, &Field)> {
        for lead in 0..=1 {
            let [RunContent::Field(field)] = self.runs.get(run_idx + lead)?.content.as_slice()
            else {
                continue;
            };
            let Some(span) = field.span.as_deref() else {
                continue;
            };
            let FieldSource::Parsed {
                raw_xml, owner_id, ..
            } = field.source.as_ref()
            else {
                continue;
            };
            if span.position != lead || span.field_index != 0 {
                continue;
            }
            let end = run_idx + span.runs.len();
            let runs = self.runs.get(run_idx..end)?;
            let in_place = runs.iter().zip(&span.runs).all(|(run, shape)| match shape {
                Some(shape) => run == shape,
                None => matches!(
                    run.content.as_slice(),
                    [RunContent::Field(candidate)]
                        if candidate.source_owner_id() == Some(*owner_id)
                ),
            });
            let last = end - 1;
            let inside = |at: usize| at > run_idx && at <= last;
            let bounded = has_typed_boundary_inside(
                run_idx,
                last,
                &self.comment_ranges,
                &self.bookmark_markers,
                &self.content_controls,
                &self.revisions,
                &self.hyperlinks,
            ) || self.extra_xml.iter().any(|(at, _)| inside(*at))
                || self.equations.iter().any(|(at, _, _)| inside(*at))
                || self
                    .rubies
                    .iter()
                    .any(|ruby| ruby.base_start < end && ruby.base_end > run_idx)
                || self.hyperlinks.iter().any(|hyperlink| {
                    hyperlink
                        .extra_xml
                        .iter()
                        .any(|(boundary, _, _)| inside(hyperlink.run_start + *boundary))
                });
            if !in_place || bounded {
                return None;
            }
            let unchanged = runs.iter().all(|run| match run.content.as_slice() {
                [RunContent::Field(field)] => field.is_unchanged(),
                _ => true,
            });
            return Some((end, raw_xml.as_slice(), unchanged, field));
        }
        None
    }

    pub(crate) fn collect_runs<'a>(&'a self, runs: &mut Vec<&'a CT_R>) {
        for index in 0..=self.runs.len() {
            for (_, _, _, sdt) in self
                .content_controls
                .iter()
                .filter(|(at, _, _, _)| *at == index)
            {
                sdt.collect_runs(runs);
            }
            if let Some(run) = self.runs.get(index) {
                runs.push(run);
            }
        }
    }

    pub(crate) fn collect_controls<'a>(&'a self, controls: &mut Vec<&'a CT_Sdt>) {
        for (_, _, _, sdt) in &self.content_controls {
            controls.push(sdt);
            sdt.collect_controls(SdtOwner::Inline, controls);
        }
    }
}

struct FieldSourceEdit<'a> {
    source_bytes: usize,
    range: std::ops::Range<usize>,
    replacement: Vec<FieldSourcePiece<'a>>,
    replacement_bytes: usize,
    projected: usize,
}

fn push_comment_marker(
    markers: &mut Vec<CommentRangeMarker>,
    extra_xml: &[(usize, Vec<u8>)],
    run_index: usize,
    id: i32,
    start: bool,
    has_child_content: bool,
) {
    let raw_before = raw_xml_count_at(extra_xml, run_index);
    let marker = if start {
        CommentRangeMarker::Start {
            id,
            run_index,
            raw_before,
            has_child_content,
        }
    } else {
        CommentRangeMarker::End {
            id,
            run_index,
            raw_before,
            has_child_content,
        }
    };
    markers.push(marker);
}

fn raw_element_has_child_content(raw: &[u8]) -> bool {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(_)) => {
                if depth > 0 {
                    return true;
                }
                depth += 1;
            }
            Ok(Event::Empty(_)) if depth > 0 => return true,
            Ok(Event::Text(text))
                if depth > 0 && !text.as_ref().iter().all(u8::is_ascii_whitespace) =>
            {
                return true;
            }
            Ok(Event::CData(text))
                if depth > 0 && !text.as_ref().iter().all(u8::is_ascii_whitespace) =>
            {
                return true;
            }
            Ok(Event::GeneralRef(reference)) if depth > 0 => match reference.resolve_char_ref() {
                Ok(Some(character)) if !character.is_ascii_whitespace() => return true,
                Ok(Some(_)) => {}
                Ok(None) => return true,
                Err(_) => return false,
            },
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::Eof) | Err(_) => return false,
            _ => {}
        }
        buffer.clear();
    }
}

fn raw_xml_count_at(extra_xml: &[(usize, Vec<u8>)], run_index: usize) -> usize {
    extra_xml
        .iter()
        .filter(|(position, raw)| *position == run_index && !is_xml_whitespace(raw))
        .count()
}

fn common_prefix_len(left: &[AcceptedRunPathSegment], right: &[AcceptedRunPathSegment]) -> usize {
    left.iter()
        .zip(right)
        .take_while(|(left, right)| left == right)
        .count()
}

/// Serialize one canonical comment or bookmark range marker.
fn range_marker_xml(anchor: RangeAnchor<'_>, start: bool) -> Option<Vec<u8>> {
    let tag = match anchor {
        RangeAnchor::Comment(_) if start => "w:commentRangeStart",
        RangeAnchor::Comment(_) => "w:commentRangeEnd",
        RangeAnchor::Bookmark { .. } if start => "w:bookmarkStart",
        RangeAnchor::Bookmark { .. } => "w:bookmarkEnd",
        RangeAnchor::Permission { .. } if start => "w:permStart",
        RangeAnchor::Permission { .. } => "w:permEnd",
        RangeAnchor::Proofing { .. } => "w:proofErr",
    };
    let mut value = itoa::Buffer::new();
    let mut element = BytesStart::new(tag);
    match anchor {
        RangeAnchor::Comment(id)
        | RangeAnchor::Bookmark { id, .. }
        | RangeAnchor::Permission { id, .. } => {
            element.push_attribute(("w:id", value.format(id)));
        }
        RangeAnchor::Proofing { kind } => {
            let value = match (kind, start) {
                ("spell", true) => "spellStart",
                ("spell", false) => "spellEnd",
                ("gram", true) => "gramStart",
                ("gram", false) => "gramEnd",
                _ => return None,
            };
            element.push_attribute(("w:type", value));
        }
    }
    if let RangeAnchor::Bookmark { name, .. } = anchor
        && start
    {
        element.push_attribute(("w:name", name));
    }
    if let RangeAnchor::Permission { editor, group, .. } = anchor
        && start
    {
        if let Some(editor) = editor {
            element.push_attribute(("w:ed", editor));
        }
        if let Some(group) = group {
            element.push_attribute(("w:edGrp", group));
        }
    }
    let mut raw = Vec::new();
    Writer::new(&mut raw)
        .write_event(Event::Empty(element))
        .ok()?;
    Some(raw)
}

/// Rewrite the id of one facade-authored bookmark marker through `remap`,
/// leaving any other raw child unchanged.
pub(crate) fn remap_authored_bookmark_marker(
    raw: &mut Vec<u8>,
    remap: &std::collections::HashMap<i32, i32>,
) {
    let Ok(text) = std::str::from_utf8(raw) else {
        return;
    };
    if !(text.starts_with("<w:bookmarkStart ") || text.starts_with("<w:bookmarkEnd ")) {
        return;
    }
    let Some(attribute) = text.find("w:id=\"") else {
        return;
    };
    let value_start = attribute + "w:id=\"".len();
    let Some(value_end) = text[value_start..].find('"').map(|end| value_start + end) else {
        return;
    };
    let Ok(old) = text[value_start..value_end].parse::<i32>() else {
        return;
    };
    let Some(updated) = remap.get(&old) else {
        return;
    };
    let mut replaced = Vec::with_capacity(raw.len());
    replaced.extend_from_slice(&raw[..value_start]);
    replaced.extend_from_slice(updated.to_string().as_bytes());
    replaced.extend_from_slice(&raw[value_end..]);
    *raw = replaced;
}

/// Whether a preserved run child is a field character or field code, which
/// a run keeps as raw XML when the rest of its complex field lies in other
/// runs.
fn raw_is_complex_field_part(raw: &[u8]) -> bool {
    let mut reader = Reader::from_reader(raw);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element) | Event::Empty(element)) => {
                return [b"fldChar".as_slice(), b"instrText", b"delInstrText"]
                    .iter()
                    .any(|local| matches_local_name(element.name().as_ref(), local));
            }
            Ok(Event::Eof) | Err(_) => return false,
            _ => {}
        }
        buffer.clear();
    }
}

/// Return the id of a preserved comment range marker, or `None` for any
/// other raw child.
pub(crate) fn raw_comment_marker_id(raw: &[u8], word_prefixes: &[String]) -> Option<i32> {
    match raw_comment_range_marker(raw, word_prefixes, 0)? {
        CommentRangeMarker::Start { id, .. } | CommentRangeMarker::End { id, .. } => Some(id),
    }
}

pub(crate) fn raw_comment_range_marker(
    raw: &[u8],
    word_prefixes: &[String],
    boundary: usize,
) -> Option<CommentRangeMarker> {
    let mut reader = Reader::from_reader(raw);
    let mut buffer = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buffer).ok()?;
        let has_child_content = matches!(event, Event::Start(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let prefixes = word_prefixes_at(&element, word_prefixes).ok()?;
                let name = element.name();
                if !is_word_element(name.as_ref(), b"commentRangeStart", &prefixes)
                    && !is_word_element(name.as_ref(), b"commentRangeEnd", &prefixes)
                {
                    return None;
                }
                let id = required_word_i32_attribute(&element, b"id", &prefixes).ok()?;
                return Some(
                    if is_word_element(name.as_ref(), b"commentRangeStart", &prefixes) {
                        CommentRangeMarker::Start {
                            id,
                            run_index: boundary,
                            raw_before: 0,
                            has_child_content,
                        }
                    } else {
                        CommentRangeMarker::End {
                            id,
                            run_index: boundary,
                            raw_before: 0,
                            has_child_content,
                        }
                    },
                );
            }
            Event::Eof => return None,
            _ => {}
        }
        buffer.clear();
    }
}

fn attribute_in_namespace(name: &[u8], local: &[u8], namespace: &str, scope: &[String]) -> bool {
    let Some(separator) = name.iter().position(|byte| *byte == b':') else {
        return false;
    };
    if name.get(separator + 1..) != Some(local) {
        return false;
    }
    let prefix = &name[..separator];
    let bound_namespace = scope.iter().find_map(|binding| {
        binding
            .strip_prefix('\0')
            .and_then(|binding| binding.split_once('\0'))
            .filter(|(candidate, _)| candidate.as_bytes() == prefix)
            .map(|(_, value)| value)
    });
    match bound_namespace {
        Some(value) => value == namespace,
        None if namespace == crate::namespace::W_NS => {
            scope.iter().any(|candidate| candidate.as_bytes() == prefix)
        }
        None => false,
    }
}

struct ParagraphBoundary<'a> {
    role: XmlWriteRole,
    emission: Option<&'a FieldXmlEmission>,
    source_context: Option<&'a [(&'a str, &'a str)]>,
    extra_xml: &'a [(usize, Vec<u8>)],
    extra_maps: Option<&'a WordExtraSourceMaps>,
    content_controls: &'a [(usize, usize, usize, CT_Sdt)],
    markers: &'a [CommentRangeMarker],
    hyperlinks: &'a [HyperlinkSpan],
    revisions: &'a [(usize, usize, CT_Revision)],
    equations: &'a [(usize, usize, OfficeMath)],
}

pub(crate) fn required_word_i32_attribute(
    element: &BytesStart<'_>,
    local: &[u8],
    word_prefixes: &[String],
) -> Result<i32> {
    for attribute in element.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        let Some(separator) = key.iter().position(|byte| *byte == b':') else {
            continue;
        };
        if key.get(separator + 1..) == Some(local)
            && word_prefixes
                .iter()
                .any(|prefix| prefix.as_bytes() == &key[..separator])
        {
            return Ok(attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?
                .parse()?);
        }
    }
    Err(OxmlError::MissingElement(format!(
        "{} attribute",
        String::from_utf8_lossy(local)
    )))
}

pub(crate) fn optional_word_attribute(
    element: &BytesStart<'_>,
    local: &[u8],
    word_prefixes: &[String],
) -> Option<String> {
    for attribute in element.attributes().flatten() {
        let key = attribute.key.as_ref();
        let Some(separator) = key.iter().position(|byte| *byte == b':') else {
            continue;
        };
        if key.get(separator + 1..) == Some(local)
            && word_prefixes
                .iter()
                .any(|prefix| prefix.as_bytes() == &key[..separator])
        {
            return oxoffice_core::xml::attribute_text(&attribute)
                .ok()
                .map(std::borrow::Cow::into_owned);
        }
    }
    None
}

struct InstructionToken {
    argument: InstructionOperand,
    quoted: bool,
    source_span: Option<std::ops::Range<usize>>,
    well_formed: bool,
}

/// Shared lexer/classifier output, independent of recursively owned fields.
#[derive(Debug, Clone)]
struct InstructionSyntax {
    raw: String,
    name: String,
    arguments: Vec<InstructionOperand>,
    switches: Vec<(String, Option<InstructionOperand>)>,
    nested_order: Vec<NestedFieldPosition>,
    /// Scope-independent scalar work, with overflow deferred until admission.
    instruction_work: Option<usize>,
}

impl Default for CT_P {
    fn default() -> Self {
        Self::new()
    }
}

/// Source attribute ownership of modeled Word content leaves.
pub(crate) fn source_modeled_attributes(
    parent: &[u8],
    local: &[u8],
) -> Option<&'static [&'static [u8]]> {
    Some(match (parent, local) {
        (
            b"r",
            b"t"
            | b"delText"
            | b"instrText"
            | b"delInstrText"
            | b"tab"
            | b"noBreakHyphen"
            | b"softHyphen"
            | b"lastRenderedPageBreak",
        ) => &[],
        (b"r", b"br") => &[b"type", b"clear"],
        (b"r", b"footnoteReference" | b"endnoteReference") => &[b"id", b"customMarkFollows"],
        (b"r", b"commentReference") => &[b"id"],
        (b"r", b"fldChar") => &[b"fldCharType", b"fldLock", b"dirty"],
        (b"p" | b"hyperlink" | b"sdtContent", b"commentRangeStart" | b"commentRangeEnd") => {
            &[b"id"]
        }
        (b"p" | b"hyperlink" | b"sdtContent", b"bookmarkStart") => &[b"id", b"name"],
        (b"p" | b"hyperlink" | b"sdtContent", b"bookmarkEnd") => &[b"id"],
        (b"p", b"hyperlink") => &[b"anchor", b"tooltip", b"docLocation"],
        (b"p" | b"hyperlink" | b"sdtContent" | b"r", b"fldSimple") => {
            &[b"instr", b"fldLock", b"dirty"]
        }
        _ => return None,
    })
}

/// Values count as modeled only when the source reader actually retains their meaning.
pub(crate) fn source_value_is_modeled(
    parent: &[u8],
    local: &[u8],
    key: &[u8],
    value: &str,
) -> bool {
    if parent == b"r" && local == b"br" && key == b"type" {
        return matches!(value, "textWrapping" | "page" | "column");
    }
    if parent == b"r" && local == b"br" && key == b"clear" {
        return matches!(value, "none" | "left" | "right" | "all");
    }
    if parent == b"r" && matches!(local, b"footnoteReference" | b"endnoteReference") {
        if key == b"id" {
            return value.parse::<i32>().is_ok();
        }
        if key == b"customMarkFollows" {
            // The reader takes every on/off spelling; an off one is absent.
            return crate::shared::ST_OnOff::parse(value).is_some();
        }
    }
    if matches!(local, b"fldSimple" | b"fldChar") {
        if matches!(key, b"dirty" | b"fldLock") {
            return crate::shared::ST_OnOff::parse(value).is_some();
        }
        if key == b"fldCharType" {
            return matches!(value, "begin" | "separate" | "end");
        }
    }
    true
}
