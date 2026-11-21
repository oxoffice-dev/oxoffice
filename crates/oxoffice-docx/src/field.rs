//! Pure evaluation of Word fields against an explicit document context.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use oxoffice_core::Length;
use oxoffice_core::custom_properties::CustomPropertyValue;
use oxoffice_core::xml::{XmlLexicalError, validate_strict_xml_1_0};
use oxoffice_docx_oxml::comments::CT_Comments;
use oxoffice_docx_oxml::content_control::{CT_Sdt, SdtContent};
use oxoffice_docx_oxml::document::{BodyContent, CT_Body, CT_Document, CT_SectPr};
use oxoffice_docx_oxml::drawing::{CT_Drawing, CT_Inline};
use oxoffice_docx_oxml::footnotes::CT_Footnotes;
use oxoffice_docx_oxml::header_footer::CT_HdrFtr;
use oxoffice_docx_oxml::namespace::{R_NS, W_NS, matches_local_name};
use oxoffice_docx_oxml::numbering::ST_LvlSuffix;
use oxoffice_docx_oxml::properties::{CT_PPr, CT_RPr};
use oxoffice_docx_oxml::revision::{CT_Revision, RevisionContent, RevisionKind};
use oxoffice_docx_oxml::shared::{ST_SectionType, ST_TabJc};
use oxoffice_docx_oxml::styles::{CT_Styles, StyleType};
use oxoffice_docx_oxml::table::{CT_Row, CT_Tbl, CT_Tc, CellContent};
use oxoffice_docx_oxml::text::CommentRangeMarker;
use oxoffice_docx_oxml::text::{
    CT_P, CT_R, CT_Text, FieldArgument, FieldInstruction, RunContent, hyperlink_revision_index,
};
use oxoffice_opc::OpcPackage;
use oxoffice_opc::relationship::rel_types;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, NamespaceResolver, ResolveResult};
use quick_xml::reader::NsReader;

pub use oxoffice_docx_oxml::text::{
    Field, LegacyCheckBoxSize, LegacyFormFieldDefinition, LegacyFormFieldKind,
    LegacyFormFieldOptions, LegacyFormFieldValue, LegacyFormInfoText, LegacyTextInputType,
};

use crate::document::{
    DocumentIdentifiers, FragmentConflictPolicy, StoryKind, prepare_physical_story_projection,
    uniquify_drawing_ids_in_xml,
};
use crate::{Document, Error, Result, style};

mod bibliography;
mod formula;
mod generated;
mod insert;
mod legacy_form;

use bibliography::bibliography_story_text_width;
pub(crate) use bibliography::{
    bibliography_citation_encounter_tags, bibliography_reference_instructions,
    patch_bibliography_instruction_options, update_bibliography_caches,
};
use formula::{
    FormulaParser, MAX_FORMULA_BYTES, apply_formats, argument_text, compare_if,
    custom_property_text, format_date_time, format_formula_value, has_switch, lexical_file_name,
    normalized_name, parse_barcode, parse_custom_styles, parse_level_range, parse_toc_level,
    switch_text, text_argument, unsupported_switch, valid_date_time, validate_instruction_shape,
};
pub use generated::{
    AuthorityEntry, GeneratedTablesReport, IndexEntry, IndexOptions, TableOfAuthoritiesOptions,
    TableOfFiguresOptions,
};
use generated::{
    DynamicOwnerPolicy, SimpleGeneratedOwnerContext, generated_normalize_simple_tables,
    generated_simple_span, generated_story_inventory, generated_table_opcode,
};
pub use insert::{
    CaptionOptions, CaptionTarget, CrossReferenceNumber, CrossReferenceOptions, SequenceOptions,
};
use insert::{caption_bookmark_names, field_option_switch, validate_reference_name};
pub use legacy_form::LegacyFormFieldInfo;
pub(crate) use legacy_form::{LegacyFormAnchor, word_accepted_form_container};
use legacy_form::{
    append_legacy_form_infos_in_paragraph, append_nested_textbox_groups, field_story_parts,
    find_bound_field_source, word_txbx_paragraph_spans,
};

mod fragment;
mod mail_merge;
mod merge_package;

use fragment::{
    AttributeNamespace, BodyIdentityState, W14_NS, add_identity_attribute_edit,
    attribute_source_span, body_identity_values, discover_fragment_part_closure,
    import_fragment_content_with_state, namespace_matches, patch_body_identity_attributes,
    patch_fragment_note_ids, remap_body_identities, resolved_element_attribute,
};
pub(crate) use fragment::{
    attribute_value_span, copy_fragment_part, fragment_note_references,
    freshen_content_fragment_identities, import_document_fragment_content, patch_bookmark_ids,
    patch_bookmark_source, patch_relationship_ids, patch_relationship_source,
    relationship_ids_in_xml,
};
pub use mail_merge::{
    MailMergeControl, MailMergeData, MailMergeFieldMapping, MailMergeFormatContext,
    MailMergeFormattedText, MailMergeImage, MailMergePackageDiagnostic,
    MailMergePackageDiagnosticKind, MailMergePackageOptions, MailMergePackageSource,
    MailMergePreparedData, MailMergeRecipient, MailMergeRecord, MailMergeSourcePolicy,
    MailMergeValue,
};
use mail_merge::{merge_referenced_header_footer_parts, resolve_mergefield_text};
use merge_package::{
    MERGE_DATA_NS, MERGE_STORE_DOC_VAR, MergeRegion, RecipientClass, classify_recipients,
    clear_mail_merge_markup, decode_merge_table, decode_recipients, embedded_mappings_are_faithful,
    encode_merge_table, identity_mappings, inspect_merge_xml, install_merge_source_graph,
    install_recipient_part, mail_merge_has_modeled_value, merge_configuration_recognized,
    merge_diagnostic, merge_settings_need_evaluation, merge_store_has_other_owner,
    merge_uri_scheme_is_file_or_https, recipient_part_census, release_owned_merge_store,
    release_owned_recipient, require_authored_merge_settings, require_merge_store_schema,
    resolve_settings_source_target, validate_merge_mappings,
};
pub use merge_package::{MailMergeTableBudget, validate_mail_merge_external_uri};

mod evaluate;
mod ref_copy;
mod scan;
mod story_sources;
mod toc;
mod update;

use evaluate::BookmarkValue;
use ref_copy::{
    append_ref_copy_paragraph, collect_ref_cached_note_references, prepare_ref_comment_copy,
    prepare_ref_note_copy, publish_ref_comment_copy, ref_bookmark_runs,
    ref_comment_occupied_identity_values, ref_comment_reference_ids, ref_comment_replacement_ids,
    ref_note_reference_counts,
};
pub(crate) use ref_copy::{
    patch_ref_comment_identity_attributes, preflight_current_field, prepared_current_field,
};
pub(crate) use scan::xml_fragment_with_namespaces;
use scan::{
    DynamicHyperlinkPlan, TypedBlockOwner, accepted_bookmark_parent, accepted_simple_field_parent,
    classify_typed_block_owner, collect_simple_toc_diagnostics, dynamic_namespace_bindings,
    invalidate_overdeep_revision_owner, mark_typed_sdt_content, parse_dynamic_toc_field,
    parse_dynamic_toc_fields, scan_dynamic_table_spans, typed_inline_owner,
    validate_dynamic_content_control,
};
pub(crate) use story_sources::find_typed_field_source;
use story_sources::{
    LegacyBlockOwner, PackageStoryKind, collect_legacy_form_anchors, legacy_story_paragraphs,
    paragraph_field_source_replacements, parsed_physical_field_paragraph,
    patch_legacy_story_field_sources, same_field_inventory,
    staged_paragraph_field_source_replacements, typed_field_ancestors,
    validate_story_document_declarations_and_doctype,
};
use toc::{
    AcceptedParagraphOwner, TocRawOrder, accepted_paragraph_owners, accepted_toc_runs,
    append_toc_wrapper_closures, dynamic_toc_replacement, reopen_staged_document,
    toc_paragraph_insertions, toc_section_text_width, toc_source_position_is_owned,
    xml_escape_attribute, xml_escape_text,
};
use update::{
    apply_updates_to_paragraph, collect_preorder_fields, referenced_header_footer_parts,
    relationship_parts, section_page_formats,
};

/// A deterministic civil date and time supplied to field evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldDateTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// Explicit values that are not stored in the document package.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldEvaluationContext {
    pub now: Option<FieldDateTime>,
    pub file_name: Option<String>,
    pub file_path: Option<String>,
    pub merge_fields: BTreeMap<String, String>,
    pub included_text: BTreeMap<String, String>,
    /// One-based source record number for mail-merge control fields.
    pub merge_record_number: Option<u32>,
    /// One-based output sequence number for mail-merge control fields.
    pub merge_sequence_number: Option<u32>,
}

/// The result of evaluating one field in document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldEvaluation {
    pub field_index: usize,
    pub instruction: String,
    pub cached_result: String,
    pub outcome: FieldOutcome,
}

impl FieldEvaluation {
    /// Complete ordered evaluation facts shared by CLI and Python reports.
    #[must_use]
    pub fn to_json_value(&self) -> serde_json::Value {
        use serde_json::json;
        let outcome = match &self.outcome {
            FieldOutcome::Resolved(text) => json!({"kind":"resolved","text":text}),
            FieldOutcome::DeferredPagination => json!({"kind":"deferred_pagination"}),
            FieldOutcome::KeepStored { diagnostic } => {
                json!({"kind":"keep_stored","diagnostic":diagnostic})
            }
            FieldOutcome::TableOfContents(request) => {
                let entries = match &request.entries {
                    TocEntrySelection::None => json!({"kind":"none"}),
                    TocEntrySelection::All => json!({"kind":"all"}),
                    TocEntrySelection::Identifier(value) => {
                        json!({"kind":"identifier","value":value})
                    }
                };
                json!({"kind":"table_of_contents","request":{
                    "heading_levels":request.heading_levels,"custom_styles":request.custom_styles,
                    "entries":entries,"sequence_identifier":request.sequence_identifier,
                    "bookmark":request.bookmark,"hyperlink":request.hyperlink,
                    "use_outline_levels":request.use_outline_levels,
                    "omit_page_number_levels":request.omit_page_number_levels,
                    "page_number_separator":request.page_number_separator,"entry_page_separator":request.entry_page_separator}})
            }
            FieldOutcome::TableOfContentsEntry(request) => {
                json!({"kind":"table_of_contents_entry","request":{
                "entry":request.entry,"level":request.level,"table_identifier":request.table_identifier,
                "omit_page_number":request.omit_page_number}})
            }
            FieldOutcome::MailMergeControl(control) => {
                let control = match control {
                    MailMergeControl::NextRecord { record_number } => {
                        json!({"kind":"next_record","record_number":record_number})
                    }
                    MailMergeControl::NextRecordIf {
                        condition,
                        record_number,
                    } => {
                        json!({"kind":"next_record_if","condition":condition,"record_number":record_number})
                    }
                    MailMergeControl::SkipRecordIf {
                        condition,
                        record_number,
                    } => {
                        json!({"kind":"skip_record_if","condition":condition,"record_number":record_number})
                    }
                    MailMergeControl::RecordNumber(value) => {
                        json!({"kind":"record_number","value":value})
                    }
                    MailMergeControl::SequenceNumber(value) => {
                        json!({"kind":"sequence_number","value":value})
                    }
                };
                json!({"kind":"mail_merge_control","control":control})
            }
            FieldOutcome::Barcode(request) => json!({"kind":"barcode","request":{
                "value":request.value,"kind":format!("{:?}",request.kind),"height":request.height,
                "scale":request.scale,"error_correction":request.error_correction,
                "point_of_sale_style":request.point_of_sale_style.map(|value| format!("{value:?}")),
                "case_style":request.case_style.map(|value| format!("{value:?}")),
                "fix_check_digit":request.fix_check_digit,"rotation":request.rotation,
                "foreground_color":request.foreground_color,"background_color":request.background_color,
                "display_text":request.display_text,"add_start_stop":request.add_start_stop}}),
        };
        json!({"field_index":self.field_index,"instruction":self.instruction,
            "cached_result":self.cached_result,"outcome":outcome})
    }
}

/// Result of one atomic ordinary and layout-backed stored-cache update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldsUpdateReport {
    pub ordinary_updated: usize,
    pub layout: LayoutBackedFieldUpdateReport,
    pub evaluations: Vec<FieldEvaluation>,
    pub diagnostics: Vec<String>,
    pub total_updated: usize,
    /// Supported typed and placed caches only. Header/footer PAGE and NUMPAGES stay stored.
    pub saved_cache_scope: &'static str,
}

/// A pure field-evaluation decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldOutcome {
    Resolved(String),
    DeferredPagination,
    TableOfContents(TocField),
    TableOfContentsEntry(TcField),
    MailMergeControl(MailMergeControl),
    Barcode(BarcodeField),
    KeepStored { diagnostic: String },
}

/// A validated table-of-contents rebuild request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocField {
    pub heading_levels: Option<(u8, u8)>,
    pub custom_styles: Vec<(String, u8)>,
    pub entries: TocEntrySelection,
    pub sequence_identifier: Option<String>,
    pub bookmark: Option<String>,
    pub hyperlink: bool,
    pub use_outline_levels: bool,
    pub omit_page_number_levels: Option<(u8, u8)>,
    pub page_number_separator: Option<String>,
    pub entry_page_separator: Option<String>,
}

/// Outcome produced by one atomic table-of-contents rebuild.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TocRebuildReport {
    pub entry_count: usize,
    pub bookmark_count: usize,
    pub diagnostics: Vec<String>,
}

/// Results of refreshing caches whose values require deterministic layout.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayoutBackedFieldUpdateReport {
    pub page_fields: usize,
    pub num_pages_fields: usize,
    pub page_reference_fields: usize,
    pub section_fields: usize,
    pub section_pages_fields: usize,
    pub diagnostics: Vec<String>,
}

impl LayoutBackedFieldUpdateReport {
    /// Number of field caches written by the operation.
    #[must_use]
    pub fn updated_count(&self) -> usize {
        self.page_fields
            + self.num_pages_fields
            + self.page_reference_fields
            + self.section_fields
            + self.section_pages_fields
    }

    /// Number of layout diagnostics returned by the operation.
    #[must_use]
    pub fn diagnostic_count(&self) -> usize {
        self.diagnostics.len()
    }
}

impl TocRebuildReport {
    /// Number of retained fields that produced diagnostics.
    #[must_use]
    pub fn diagnostic_count(&self) -> usize {
        self.diagnostics.len()
    }
}

/// Which TC entries contribute to a table of contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TocEntrySelection {
    None,
    All,
    Identifier(String),
}

/// A validated table-of-contents entry request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcField {
    pub entry: String,
    pub level: u8,
    pub table_identifier: Option<String>,
    pub omit_page_number: bool,
}

/// A validated generated-barcode request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarcodeField {
    pub value: String,
    pub kind: BarcodeKind,
    pub height: Option<u32>,
    pub scale: Option<u16>,
    pub error_correction: Option<u8>,
    pub point_of_sale_style: Option<BarcodePointOfSaleStyle>,
    pub case_style: Option<BarcodeCaseStyle>,
    pub fix_check_digit: bool,
    pub rotation: Option<u8>,
    pub foreground_color: Option<u32>,
    pub background_color: Option<u32>,
    pub display_text: bool,
    pub add_start_stop: bool,
}

/// A barcode type accepted by Word's barcode field grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarcodeKind {
    Upca,
    Upce,
    Jan13,
    Jan8,
    Ean13,
    Ean8,
    Case,
    Itf14,
    Nw7,
    Code39,
    Code128,
    JpPost,
    Qr,
}

/// A point-of-sale style accepted by `\p`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarcodePointOfSaleStyle {
    Standard,
    SupplementalTwoDigit,
    SupplementalFiveDigit,
    Case,
}

/// An ITF14 case style accepted by `\c`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarcodeCaseStyle {
    Standard,
    Extended,
    Add,
}

#[cfg(test)]
thread_local! {
    static NAVIGATION_OWNER_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Document {
    pub(crate) fn navigation_exclusions_for_input(
        &self,
        input: &oxoffice_docx_layout::LayoutInput,
    ) -> Result<Option<Arc<oxoffice_docx_layout::WordSequenceSnapshot>>> {
        let xml = input.document.to_xml()?;
        let mut spans = scan_dynamic_table_spans(&xml, DynamicOwnerPolicy::Bibliography)?;
        spans.retain(|span| {
            Field::new(&span.instruction, "")
                .effective_instruction()
                .name
                != "CITATION"
        });
        if spans.is_empty() {
            return Ok(None);
        }
        let mut snapshot = oxoffice_docx_layout::engine::evaluate_sequence_fields(input)?;
        let mut paths = Vec::new();
        for index in 1.. {
            let Some(node) = oxoffice_docx_layout::SourceNodeId::new(index) else {
                break;
            };
            let Some(path) = snapshot.source_node(node) else {
                break;
            };
            if path.story == oxoffice_docx_layout::WordStory::Document {
                paths.push(path.clone());
            }
        }
        let mut paragraphs = Vec::new();
        collect_body_paragraphs(&input.document.body, &mut paragraphs, None);
        if paragraphs.len() != paths.len() {
            return Err(Error::Other(
                "navigation generated-range physical source inventory disagrees".into(),
            ));
        }
        let mut exclusions = Vec::new();
        // Scanner spans are disjoint and physically ordered. Advance the paragraph
        // window once, then search end positions only inside that window. A single
        // paragraph may contain many owners, so scanning its whole window per run
        // would still amplify legal input quadratically.
        let mut span_start = 0;
        let mut span_end = 0;
        for (paragraph_index, (paragraph, path)) in paragraphs.iter().zip(paths).enumerate() {
            while span_start < spans.len() {
                #[cfg(test)]
                NAVIGATION_OWNER_VISITS.set(NAVIGATION_OWNER_VISITS.get() + 1);
                if spans[span_start].end_paragraph >= paragraph_index {
                    break;
                }
                span_start += 1;
            }
            span_end = span_end.max(span_start);
            while span_end < spans.len() {
                #[cfg(test)]
                NAVIGATION_OWNER_VISITS.set(NAVIGATION_OWNER_VISITS.get() + 1);
                if spans[span_end].begin_paragraph > paragraph_index {
                    break;
                }
                span_end += 1;
            }
            let relevant = &spans[span_start..span_end];
            if relevant.is_empty() {
                continue;
            }
            let excluded = accepted_toc_runs(paragraph)
                .iter()
                .enumerate()
                .filter_map(|(index, run)| {
                    let position = TocOwnedPosition {
                        paragraph: paragraph_index,
                        run: TocRunPosition {
                            run_boundary: run.run_boundary,
                            raw_order: run.raw_order,
                            nested_order: run.nested_order,
                        },
                    };
                    let candidate = relevant.partition_point(|span| {
                        #[cfg(test)]
                        NAVIGATION_OWNER_VISITS.set(NAVIGATION_OWNER_VISITS.get() + 1);
                        TocOwnedPosition {
                            paragraph: span.end_paragraph,
                            run: span.result_end_position,
                        } <= position
                    });
                    relevant.get(candidate).and_then(|span| {
                        toc_source_position_is_owned(std::slice::from_ref(span), position)
                            .then_some(index)
                    })
                })
                .collect();
            exclusions.push((path, excluded));
        }
        snapshot.set_navigation_exclusions(input, exclusions)?;
        Ok(Some(Arc::new(snapshot)))
    }
}

struct ImportedFragmentContent {
    typed: Vec<BodyContent>,
    xml: Vec<u8>,
    source: Option<Arc<oxoffice_docx_oxml::document::ScopedWordSource>>,
}

struct FragmentNoteCopy {
    kind: StoryKind,
    destination_part: String,
    xml: Vec<u8>,
    source: Option<Arc<oxoffice_docx_oxml::document::ScopedWordSource>>,
    replace_id: Option<i32>,
}

#[derive(Debug, Clone)]
struct DynamicTocSpan {
    simple_field: Option<Field>,
    instruction: String,
    field_start: usize,
    field_end: usize,
    begin_paragraph: usize,
    end_paragraph: usize,
    begin_run_start: usize,
    instruction_paragraph_start: usize,
    result_start: usize,
    result_end: usize,
    result_start_position: TocRunPosition,
    result_end_position: TocRunPosition,
    end_run_end: usize,
    start_paragraph_name: String,
    start_paragraph_namespaces: BTreeMap<String, String>,
    separator_wrapper_names: Vec<String>,
    instruction_runs: Vec<DynamicInstructionRun>,
    end_paragraph_start: usize,
    end_paragraph_content_start: usize,
    end_wrapper_prefixes: Vec<(usize, usize)>,
}

#[derive(Debug, Clone)]
struct DynamicInstructionRun {
    start: usize,
    end: usize,
    inherited_namespaces: BTreeMap<String, String>,
}

#[derive(Debug)]
struct DynamicXmlElement {
    local_name: Vec<u8>,
    qualified_name: String,
    typed_block_owner: Option<TypedBlockOwner>,
    is_word: bool,
    is_typed_paragraph: bool,
    is_typed_inline_owner: bool,
    revision_depth: usize,
    sdt_content_seen: bool,
    namespace_bindings: BTreeMap<String, String>,
    inherited_namespaces: BTreeMap<String, String>,
    run_position: Option<TocRunPosition>,
    hyperlink_plan: Option<DynamicHyperlinkPlan>,
    start: usize,
    start_tag_end: usize,
    paragraph: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct TocRunPosition {
    run_boundary: usize,
    raw_order: TocRawOrder,
    nested_order: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct TocOwnedPosition {
    paragraph: usize,
    run: TocRunPosition,
}

#[derive(Clone, Copy)]
struct AcceptedTocRun<'a> {
    run: &'a CT_R,
    run_boundary: usize,
    raw_order: TocRawOrder,
    nested_order: usize,
}

#[derive(Debug)]
struct TocParagraphInsertion {
    start: usize,
    end: usize,
    name: String,
    namespaces: BTreeMap<String, String>,
    has_non_namespace_attributes: bool,
    start_tag_end: usize,
    content_start: usize,
    content_end: usize,
}

fn byte_match_offsets(input: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut output = Vec::new();
    let mut start = 0usize;
    while let Some(relative) = find_bytes(&input[start..], needle) {
        let found = start + relative;
        output.push(found);
        start = found + needle.len();
    }
    output
}

fn non_body_story_parts(document: &Document) -> Result<Vec<(String, Vec<u8>)>> {
    let mut parts = merge_referenced_header_footer_parts(document)?
        .into_iter()
        .chain(
            relationship_parts(document, rel_types::FOOTNOTES)
                .into_iter()
                .map(|(name, xml)| (format!("footnotes:{name}"), xml)),
        )
        .chain(
            relationship_parts(document, rel_types::ENDNOTES)
                .into_iter()
                .map(|(name, xml)| (format!("endnotes:{name}"), xml)),
        )
        .collect::<Vec<_>>();
    parts.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(parts)
}

struct ComplexInstruction {
    text: String,
    collecting: bool,
}

#[derive(Default)]
struct BodyIdentityValues {
    bookmark_ids: Vec<String>,
    bookmark_events: Vec<(String, bool)>,
    comment_ids: Vec<String>,
    comment_events: Vec<(String, u8)>,
    content_control_ids: Vec<String>,
    drawing_ids: Vec<String>,
    non_visual_drawing_ids: Vec<String>,
    bookmark_names: Vec<String>,
    reference_names: Vec<String>,
}

#[derive(Default)]
struct BodyIdentityRemap {
    bookmark_ids: BTreeMap<String, String>,
    comment_ids: BTreeMap<String, String>,
    content_control_ids: BTreeMap<String, String>,
    drawing_ids: BTreeMap<String, String>,
    non_visual_drawing_ids: BTreeMap<String, String>,
    bookmark_names: BTreeMap<String, String>,
    /// Remove `w14:paraId` and `w14:textId` from paragraphs and table rows.
    /// A copy must not share them with its source, and Word assigns new ones
    /// to an element that has none.
    drop_paragraph_identities: bool,
}

fn empty_section_properties() -> CT_SectPr {
    CT_SectPr {
        page_width: None,
        page_height: None,
        orientation: None,
        margin_top: None,
        margin_right: None,
        margin_bottom: None,
        margin_left: None,
        gutter: None,
        header_distance: None,
        footer_distance: None,
        section_type: None,
        columns: None,
        page_number: None,
        footnote_pr: None,
        endnote_pr: None,
        paper_source: None,
        page_borders: None,
        line_numbers: None,
        vertical_alignment: None,
        text_direction: None,
        doc_grid: None,
        title_pg: None,
        header_refs: Vec::new(),
        footer_refs: Vec::new(),
        extra_xml: Vec::new(),
        extra_xml_positions: Vec::new(),
        change: None,
    }
}

struct TextBoxCachePatch {
    location: crate::ContentLocation,
    replacements: Vec<(Vec<u8>, Vec<u8>)>,
    updated: usize,
}

struct RefCopyField {
    field: Field,
    locked: bool,
    generated: bool,
    suppress_copied_references: bool,
}

struct RefBookmarkCache {
    runs: Vec<CT_R>,
    comment_ranges: Vec<CommentRangeMarker>,
}

struct CachedFieldUpdate {
    cached_result: String,
    dirty: bool,
    typed_runs: Option<Vec<CT_R>>,
    comment_ranges: Vec<CommentRangeMarker>,
}

fn valid_xml_character(value: char) -> bool {
    matches!(value, '\u{0009}' | '\u{000A}' | '\u{000D}')
        || ('\u{0020}'..='\u{D7FF}').contains(&value)
        || ('\u{E000}'..='\u{FFFD}').contains(&value)
        || ('\u{10000}'..='\u{10FFFF}').contains(&value)
}

#[derive(Debug, Clone, Copy)]
struct MailMergeStoryState {
    record_number: Option<u32>,
    sequence_number: Option<u32>,
}

struct Evaluator<'a> {
    document: &'a Document,
    context: &'a FieldEvaluationContext,
    bookmarks: BTreeMap<String, BookmarkValue>,
    // Related-story bookmarks whose two markers are in two owners. A REF to
    // one refuses by name instead of reading as missing.
    cross_owner_bookmarks: HashSet<String>,
    numbering_layout: Option<Arc<oxoffice_docx_layout::WordLayoutResult>>,
    numbering_nodes:
        HashMap<oxoffice_docx_layout::WordSourcePath, oxoffice_docx_layout::SourceNodeId>,
    navigation_styles: HashMap<String, Vec<oxoffice_docx_layout::SourceNodeId>>,
    navigation_style_pages:
        HashMap<String, BTreeMap<usize, Vec<oxoffice_docx_layout::SourceNodeId>>>,
    navigation_field_pages: HashMap<oxoffice_layout::FieldSource, BTreeSet<usize>>,
    results: Vec<FieldEvaluation>,
    visible_results: Vec<bool>,
    general_field_contexts: Vec<bool>,
    general_context: bool,
    mail_merge_stories: BTreeMap<String, MailMergeStoryState>,
    nested_outcomes: Vec<BTreeMap<usize, FieldOutcome>>,
    missing_merge_fields_as_empty: bool,
    current_field_source: Option<oxoffice_layout::FieldSource>,
    sequence_snapshot: Option<Arc<oxoffice_docx_layout::WordSequenceSnapshot>>,
    sequence_nodes: Vec<Option<oxoffice_docx_layout::SourceNodeId>>,
    field_sources: HashMap<usize, oxoffice_layout::FieldSource>,
    ref_note_counts: Option<HashMap<(StoryKind, i32), usize>>,
    locked_fields: HashSet<usize>,
}

struct LegacyStoryParagraph {
    path: Vec<(&'static str, usize)>,
    start: usize,
    end: usize,
    general_context: bool,
    original: CT_P,
    paragraph: CT_P,
}

struct FieldSourceEdit {
    start: usize,
    end: usize,
    replacement: Vec<u8>,
}

fn namespace_is_word(namespace: &ResolveResult<'_>) -> bool {
    matches!(namespace, ResolveResult::Bound(Namespace(uri)) if *uri == W_NS.as_bytes())
}

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        None
    } else {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }
}

fn collect_body_paragraphs<'a>(
    body: &'a CT_Body,
    output: &mut Vec<&'a CT_P>,
    mut paths: Option<&mut Vec<Vec<(&'static str, usize)>>>,
) {
    let mut path = Vec::new();
    for (index, content) in body
        .content
        .iter()
        .filter(|content| !matches!(content, BodyContent::RawXml(_)))
        .enumerate()
    {
        let kind = match content {
            BodyContent::Paragraph(_) => "p",
            BodyContent::Table(_) => "tbl",
            BodyContent::ContentControl(_) => "sdt",
            BodyContent::RawXml(_) => continue,
        };
        if paths.is_some() {
            path.push((kind, index));
        }
        match content {
            BodyContent::Paragraph(paragraph) => {
                record_field_paragraph(paragraph, output, &path, &mut paths)
            }
            BodyContent::Table(table) => {
                collect_table_paragraphs(table, output, &mut path, &mut paths)
            }
            BodyContent::ContentControl(control) => collect_control_paragraphs(
                control,
                LegacyBlockOwner::Root,
                output,
                &mut path,
                &mut paths,
            ),
            BodyContent::RawXml(_) => {}
        }
        if paths.is_some() {
            path.pop();
        }
    }
}

fn record_field_paragraph<'a>(
    paragraph: &'a CT_P,
    output: &mut Vec<&'a CT_P>,
    path: &[(&'static str, usize)],
    paths: &mut Option<&mut Vec<Vec<(&'static str, usize)>>>,
) {
    output.push(paragraph);
    if let Some(paths) = paths {
        paths.push(path.to_vec());
    }
}

fn collect_table_paragraphs<'a>(
    table: &'a CT_Tbl,
    output: &mut Vec<&'a CT_P>,
    path: &mut Vec<(&'static str, usize)>,
    paths: &mut Option<&mut Vec<Vec<(&'static str, usize)>>>,
) {
    let mut index = 0usize;
    for boundary in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(position, _, _)| *position == boundary)
        {
            if paths.is_some() {
                path.push(("sdt", index));
            }
            collect_control_paragraphs(control, LegacyBlockOwner::Table, output, path, paths);
            if paths.is_some() {
                path.pop();
            }
            index = index.checked_add(1).expect("admitted modeled owner count");
        }
        if let Some(row) = table.rows.get(boundary) {
            if paths.is_some() {
                path.push(("tr", index));
            }
            collect_row_paragraphs(row, output, path, paths);
            if paths.is_some() {
                path.pop();
            }
            index = index.checked_add(1).expect("admitted modeled owner count");
        }
    }
}

fn collect_row_paragraphs<'a>(
    row: &'a CT_Row,
    output: &mut Vec<&'a CT_P>,
    path: &mut Vec<(&'static str, usize)>,
    paths: &mut Option<&mut Vec<Vec<(&'static str, usize)>>>,
) {
    let mut index = 0usize;
    for boundary in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter()
            .filter(|(position, _, _)| *position == boundary)
        {
            if paths.is_some() {
                path.push(("sdt", index));
            }
            collect_control_paragraphs(control, LegacyBlockOwner::Row, output, path, paths);
            if paths.is_some() {
                path.pop();
            }
            index = index.checked_add(1).expect("admitted modeled owner count");
        }
        if let Some(cell) = row.cells.get(boundary) {
            if paths.is_some() {
                path.push(("tc", index));
            }
            collect_cell_paragraphs(cell, output, path, paths);
            if paths.is_some() {
                path.pop();
            }
            index = index.checked_add(1).expect("admitted modeled owner count");
        }
    }
}

fn collect_cell_paragraphs<'a>(
    cell: &'a CT_Tc,
    output: &mut Vec<&'a CT_P>,
    path: &mut Vec<(&'static str, usize)>,
    paths: &mut Option<&mut Vec<Vec<(&'static str, usize)>>>,
) {
    for (index, content) in cell.content.iter().enumerate() {
        let kind = match content {
            CellContent::Paragraph(_) => "p",
            CellContent::Table(_) => "tbl",
            CellContent::ContentControl(_) => "sdt",
        };
        if paths.is_some() {
            path.push((kind, index));
        }
        match content {
            CellContent::Paragraph(paragraph) => {
                record_field_paragraph(paragraph, output, path, paths)
            }
            CellContent::Table(table) => collect_table_paragraphs(table, output, path, paths),
            CellContent::ContentControl(control) => {
                collect_control_paragraphs(control, LegacyBlockOwner::Cell, output, path, paths)
            }
        }
        if paths.is_some() {
            path.pop();
        }
    }
}

fn collect_control_paragraphs<'a>(
    control: &'a CT_Sdt,
    owner: LegacyBlockOwner,
    output: &mut Vec<&'a CT_P>,
    path: &mut Vec<(&'static str, usize)>,
    paths: &mut Option<&mut Vec<Vec<(&'static str, usize)>>>,
) {
    let mut index = 0usize;
    for content in &control.content {
        let kind = match (owner, content) {
            (LegacyBlockOwner::Root | LegacyBlockOwner::Cell, SdtContent::Paragraph(_)) => "p",
            (LegacyBlockOwner::Root | LegacyBlockOwner::Cell, SdtContent::Table(_)) => "tbl",
            (LegacyBlockOwner::Table, SdtContent::Row(_)) => "tr",
            (LegacyBlockOwner::Row, SdtContent::Cell(_)) => "tc",
            (_, SdtContent::ContentControl(_)) => "sdt",
            _ => continue,
        };
        if paths.is_some() {
            path.push((kind, index));
        }
        match content {
            SdtContent::Paragraph(paragraph) => {
                record_field_paragraph(paragraph, output, path, paths)
            }
            SdtContent::Table(table) => collect_table_paragraphs(table, output, path, paths),
            SdtContent::Row(row) => collect_row_paragraphs(row, output, path, paths),
            SdtContent::Cell(cell) => collect_cell_paragraphs(cell, output, path, paths),
            SdtContent::ContentControl(control) => {
                collect_control_paragraphs(control, owner, output, path, paths)
            }
            _ => unreachable!("accepted control role"),
        }
        if paths.is_some() {
            path.pop();
        }
        index = index.checked_add(1).expect("admitted modeled owner count");
    }
}

fn keep(diagnostic: &str) -> FieldOutcome {
    FieldOutcome::KeepStored {
        diagnostic: format!("{diagnostic}, stored display retained"),
    }
}

struct MergePackageView {
    options: MailMergePackageOptions,
    sources_differ: bool,
    recipient_blocked: bool,
    unsupported_projection: bool,
}

#[derive(Default)]
struct ParsedMergeGraph {
    data_source: Option<String>,
    odso_source: Option<String>,
    header_id: Option<String>,
    mappings: Vec<MailMergeFieldMapping>,
    recipient_id: Option<String>,
    recipient_graph: bool,
    unsafe_field_map: bool,
    unsupported_field_map: bool,
    unsafe_source: bool,
    recognized_field_map: bool,
    modeled_member: bool,
    owned_docvar: bool,
}

struct MergeInspection {
    graph: ParsedMergeGraph,
    region: Option<MergeRegion>,
}

#[derive(Clone)]
struct GeneratedStorySource {
    story: crate::StoryId,
    range: std::ops::Range<usize>,
    wrapper_len: usize,
    xml: Vec<u8>,
    body: CT_Body,
    spans: Vec<DynamicTocSpan>,
}
