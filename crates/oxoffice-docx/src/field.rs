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

impl Document {
    /// Evaluate every typed field without mutating stored results.
    pub fn evaluate_fields(
        &self,
        context: &FieldEvaluationContext,
    ) -> Result<Vec<FieldEvaluation>> {
        let mut source = self.clone_for_staging();
        source.flush_dirty_related_story_models()?;
        prepare_physical_story_projection(&mut source.document.body, &mut [])?;
        let (evaluations, visible) = source.evaluate_fields_with_policy(context, false)?;
        Ok(evaluations
            .into_iter()
            .zip(visible)
            .filter_map(|(evaluation, visible)| visible.then_some(evaluation))
            .enumerate()
            .map(|(index, mut evaluation)| {
                evaluation.field_index = index;
                evaluation
            })
            .collect())
    }

    fn evaluate_fields_with_policy(
        &self,
        context: &FieldEvaluationContext,
        missing_merge_fields_as_empty: bool,
    ) -> Result<(Vec<FieldEvaluation>, Vec<bool>)> {
        let source = self;
        let mut evaluator = if missing_merge_fields_as_empty {
            Evaluator::for_mail_merge(source, context)
        } else {
            Evaluator::new(source, context)
        };
        let input = source.build_layout_input()?;
        let snapshot = oxoffice_docx_layout::engine::evaluate_sequence_fields(&input)
            .map_err(|error| Error::Other(format!("sequence source evaluation failed: {error}")))?;
        let mut nodes_by_story = HashMap::<
            oxoffice_docx_layout::WordStory,
            Vec<oxoffice_docx_layout::SourceNodeId>,
        >::new();
        for index in 1u32.. {
            let Some(node) = oxoffice_docx_layout::SourceNodeId::new(index) else {
                break;
            };
            let Some(path) = snapshot.source_node(node) else {
                break;
            };
            nodes_by_story
                .entry(path.story.clone())
                .or_default()
                .push(node);
        }
        evaluator.sequence_snapshot = Some(Arc::new(snapshot));
        let mut main = Vec::new();
        collect_body_paragraphs(&source.document.body, &mut main, None);
        evaluator.sequence_nodes = nodes_by_story
            .get(&oxoffice_docx_layout::WordStory::Document)
            .map(|nodes| nodes.iter().copied().map(Some).collect())
            .unwrap_or_default();
        evaluator.refresh_main_bookmark_sequence_text(&main)?;
        evaluator.evaluate_story("main", &main)?;

        let general_parts = [true, false]
            .into_iter()
            .flat_map(|header| referenced_header_footer_parts(source, header, false))
            .map(|(part, _)| part)
            .collect::<HashSet<_>>();
        for (part_name, kind, xml) in field_story_parts(source, true) {
            let records = legacy_story_paragraphs(&xml, kind)?;
            evaluator.general_field_contexts = records
                .iter()
                .map(|record| {
                    record.general_context
                        && (!matches!(kind, PackageStoryKind::Header | PackageStoryKind::Footer)
                            || general_parts.contains(&part_name))
                })
                .collect();
            let paragraphs = records
                .iter()
                .map(|entry| &entry.paragraph)
                .collect::<Vec<_>>();
            let mut owners = nodes_by_story
                .iter()
                .filter(|(story, _)| {
                    input.story_part_names.get(*story) == Some(&part_name)
                        && matches!(
                            (story, kind),
                            (
                                oxoffice_docx_layout::WordStory::Header { .. },
                                PackageStoryKind::Header
                            ) | (
                                oxoffice_docx_layout::WordStory::Footer { .. },
                                PackageStoryKind::Footer
                            ) | (
                                oxoffice_docx_layout::WordStory::Footnote { .. },
                                PackageStoryKind::Footnotes
                            ) | (
                                oxoffice_docx_layout::WordStory::Endnote { .. },
                                PackageStoryKind::Endnotes
                            )
                        )
                })
                .collect::<Vec<_>>();
            owners.sort_by_key(|(_, nodes)| nodes.first().map(|id| id.get()));
            evaluator.sequence_nodes =
                if matches!(kind, PackageStoryKind::Header | PackageStoryKind::Footer) {
                    owners
                        .first()
                        .map(|(_, nodes)| nodes.iter().copied().map(Some).collect())
                        .unwrap_or_default()
                } else {
                    // Each note owner is physically distinct, unlike relationship aliases to furniture.
                    owners
                        .into_iter()
                        .flat_map(|(_, nodes)| nodes.iter().copied().map(Some))
                        .collect()
                };
            let story = match kind {
                PackageStoryKind::Header => format!("header:{part_name}"),
                PackageStoryKind::Footer => format!("footer:{part_name}"),
                PackageStoryKind::Footnotes => "footnotes".into(),
                PackageStoryKind::Endnotes => "endnotes".into(),
                PackageStoryKind::Comments => "comments".into(),
                PackageStoryKind::Document => "document".into(),
            };
            if matches!(kind, PackageStoryKind::Comments) {
                evaluator.sequence_nodes = vec![None; paragraphs.len()];
            }
            evaluator.evaluate_story(&story, &paragraphs)?;
        }

        for source in source.text_box_cache_paragraphs()? {
            let paragraph = parsed_physical_field_paragraph(&source.xml)?;
            let mut fields = Vec::new();
            let mut root_indices = HashSet::new();
            for run in paragraph.source_runs() {
                for content in &run.content {
                    if let RunContent::Field(field) = content {
                        root_indices.insert(fields.len());
                        let mut pending = vec![(field, false, false)];
                        while let Some((field, inherited_lock, cached)) = pending.pop() {
                            let locked = inherited_lock || field.locked() == Some(true);
                            fields.push((field, locked, cached));
                            pending.extend(
                                field
                                    .cached_fields_in_source_order()
                                    .into_iter()
                                    .rev()
                                    .map(|child| (child, locked, true)),
                            );
                            pending.extend(
                                field
                                    .nested_fields_in_source_order()
                                    .into_iter()
                                    .rev()
                                    .map(|child| (child, locked, cached)),
                            );
                        }
                    }
                }
            }
            if fields.is_empty() {
                continue;
            }
            let snapshot = evaluator
                .sequence_snapshot
                .as_ref()
                .expect("sequence inventory");
            let mut matching = Vec::new();
            for (story, nodes) in &nodes_by_story {
                if !matches!(story, oxoffice_docx_layout::WordStory::TextBox { part_name, .. } if part_name == source.location.story().part_name())
                    || snapshot.text_box_owner_index(story) != Some(source.owner_index)
                {
                    continue;
                }
                let Some(node) = nodes.get(source.paragraph_index).copied() else {
                    continue;
                };
                let mut enclosing_qualified = false;
                let qualified =
                    fields
                        .iter()
                        .enumerate()
                        .all(|(index, (field, locked, cached))| {
                            let root = root_indices.contains(&index);
                            if root {
                                enclosing_qualified = false;
                            }
                            let source = oxoffice_layout::FieldSource {
                                node,
                                index: index as u32,
                            };
                            if snapshot.source_field_context(source)
                                != Some((
                                    field.effective_instruction_text().as_str(),
                                    *locked,
                                    *cached,
                                ))
                            {
                                return false;
                            }
                            let model_raw = field
                                .source_replacement()
                                .ok()
                                .flatten()
                                .map(|(raw, _)| raw);
                            let source_raw = snapshot.source_field_xml(source);
                            match (model_raw, source_raw) {
                                (Some(model), Some(source)) if model == source => {
                                    if root {
                                        enclosing_qualified = true;
                                    }
                                    true
                                }
                                (None, None) if !root && enclosing_qualified => true,
                                _ => false,
                            }
                        });
                if qualified
                    && snapshot
                        .source_field_context(oxoffice_layout::FieldSource {
                            node,
                            index: fields.len() as u32,
                        })
                        .is_none()
                {
                    matching.push(node);
                }
            }
            evaluator.sequence_nodes = vec![(matching.len() == 1).then(|| matching[0])];
            evaluator.general_field_contexts = vec![false];
            evaluator.evaluate_story("textbox", &[&paragraph])?;
        }

        Ok((evaluator.results, evaluator.visible_results))
    }

    /// Evaluate and materialize every typed field cache in document order.
    pub fn update_fields(&mut self, context: &FieldEvaluationContext) -> Result<usize> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_dirty_related_story_models()?;
        let updated = candidate.update_fields_with_policy(context, false)?;
        if updated != 0 {
            self.commit_staged_mutation(candidate.prepare_and_reopen_staged()?);
        }
        Ok(updated)
    }

    /// Refresh ordinary and deterministic layout caches in one outer transaction.
    /// Omitted time remains unresolved. Ordinary retention diagnostics precede layout diagnostics.
    pub fn update_all_fields(
        &mut self,
        context: &FieldEvaluationContext,
    ) -> Result<FieldsUpdateReport> {
        let mut candidate = self.clone_for_staging();
        let evaluations = candidate.evaluate_fields(context)?;
        let ordinary = candidate.update_fields(context)?;
        let layout = candidate.update_layout_backed_fields()?;
        let mut diagnostics = evaluations
            .iter()
            .filter_map(|field| match &field.outcome {
                FieldOutcome::KeepStored { diagnostic } => Some(diagnostic.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        diagnostics.extend(layout.diagnostics.iter().cloned());
        let total_updated = ordinary
            .checked_add(layout.updated_count())
            .ok_or_else(|| Error::Other("field update count overflow".to_owned()))?;
        if total_updated != 0 {
            self.commit_staged_mutation(candidate.prepare_and_reopen_staged()?);
        }
        Ok(FieldsUpdateReport {
            ordinary_updated: ordinary,
            layout,
            evaluations,
            diagnostics,
            total_updated,
            saved_cache_scope: "supported-typed-and-placed-caches-with-dynamic-header-footer-page-caches-retained",
        })
    }

    /// Write layout-backed field caches and return the number changed.
    ///
    /// PAGE takes its placed story owner's displayed number. Word preserves
    /// dynamic PAGE and NUMPAGES caches in headers and footers on save, so
    /// those caches remain unchanged. PAGEREF takes its resolved
    /// bookmark. Written fields are marked clean. Every other field keeps its
    /// cache, as does a layout-backed field that layout does not place, that
    /// uses an unsupported switch, or whose section page number format layout
    /// does not reproduce. `update_fields` still defers these kinds. Use
    /// [`Self::update_layout_backed_fields`] for per-kind counts and layout
    /// diagnostics.
    pub fn update_page_fields(&mut self) -> Result<usize> {
        Ok(self.update_layout_backed_fields()?.updated_count())
    }

    /// Write page, section, total and resolved bookmark page caches from one layout.
    ///
    /// Each field uses its immutable placement and owning section. Header and
    /// footer PAGE and NUMPAGES caches retain Word's saved-cache behavior. Fields
    /// with an unsupported switch or unresolved target retain their caches.
    /// Successful writes are marked clean and committed atomically.
    pub fn update_layout_backed_fields(&mut self) -> Result<LayoutBackedFieldUpdateReport> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_dirty_related_story_models()?;
        prepare_physical_story_projection(&mut candidate.document.body, &mut [])?;
        let layout = candidate.layout_deterministic()?;
        let (updates, mut report) = candidate.page_field_updates(&layout)?;
        let textbox_updates = candidate.text_box_page_field_updates(&layout, &mut report)?;
        report.diagnostics.extend(
            layout
                .layout
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<_>>(),
        );
        let mut updated = candidate.apply_cached_field_updates(&updates, false)?;
        for patch in textbox_updates {
            candidate.patch_story_paragraph_field_sources(&patch.location, &patch.replacements)?;
            updated += patch.updated;
        }
        debug_assert_eq!(updated, report.updated_count());
        if updated != 0 {
            self.commit_staged_mutation(candidate);
        }
        Ok(report)
    }

    /// Stage one optional cache update per field, in update traversal order,
    /// for every PAGE, NUMPAGES, and PAGEREF field that `layout` places.
    fn page_field_updates(
        &self,
        layout: &oxoffice_docx_layout::WordLayoutResult,
    ) -> Result<(
        Vec<Option<CachedFieldUpdate>>,
        LayoutBackedFieldUpdateReport,
    )> {
        use oxoffice_docx_layout::{SourceNodeId, WordSourcePath, WordStory};

        let placed = placed_page_fields(layout);
        let section_formats = section_page_formats(&self.document);
        let mut updates = Vec::new();
        let mut report = LayoutBackedFieldUpdateReport::default();

        let mut main = Vec::new();
        collect_body_paragraphs(&self.document.body, &mut main, None);
        // Layout registers main-story paragraphs in this same flattened order.
        let main_paths = (1u32..)
            .map_while(|index| SourceNodeId::new(index).and_then(|id| layout.source_node(id)))
            .filter(|path| path.story == WordStory::Document)
            .map(|path| vec![path.clone()])
            .collect::<Vec<_>>();
        if main_paths.len() != main.len() {
            return Err(Error::Other(format!(
                "layout identified {} of {} main story paragraphs",
                main_paths.len(),
                main.len()
            )));
        }
        push_page_field_updates(
            &main,
            &main_paths,
            &placed,
            layout,
            &section_formats,
            &mut updates,
            &mut report,
        );

        let input = self.build_layout_input()?;
        for (part_name, kind, xml) in field_story_parts(self, false) {
            let story = legacy_story_paragraphs(&xml, kind)?;
            let paragraphs = story
                .iter()
                .map(|entry| &entry.paragraph)
                .collect::<Vec<_>>();
            let mut physical_paths = BTreeMap::<String, Vec<WordSourcePath>>::new();
            for path in (1u32..)
                .map_while(|index| SourceNodeId::new(index).and_then(|id| layout.source_node(id)))
            {
                let matches_kind = matches!(
                    (&path.story, kind),
                    (WordStory::Header { .. }, PackageStoryKind::Header)
                        | (WordStory::Footer { .. }, PackageStoryKind::Footer)
                        | (WordStory::Footnote { .. }, PackageStoryKind::Footnotes)
                        | (WordStory::Endnote { .. }, PackageStoryKind::Endnotes)
                );
                if matches_kind && input.story_part_names.get(&path.story) == Some(&part_name) {
                    // Header relationships sharing one physical part describe
                    // alternative placements of the same paragraph inventory.
                    let key = match &path.story {
                        WordStory::Header { relationship_id }
                        | WordStory::Footer { relationship_id } => relationship_id.clone(),
                        _ => String::new(),
                    };
                    physical_paths.entry(key).or_default().push(path.clone());
                }
            }
            let paths = (0..paragraphs.len())
                .map(|index| {
                    physical_paths
                        .values()
                        .filter(|paths| paths.len() == paragraphs.len())
                        .filter_map(|paths| paths.get(index).cloned())
                        .collect()
                })
                .collect::<Vec<_>>();
            push_page_field_updates(
                &paragraphs,
                &paths,
                &placed,
                layout,
                &section_formats,
                &mut updates,
                &mut report,
            );
        }

        Ok((updates, report))
    }

    fn text_box_page_field_updates(
        &self,
        layout: &oxoffice_docx_layout::WordLayoutResult,
        report: &mut LayoutBackedFieldUpdateReport,
    ) -> Result<Vec<TextBoxCachePatch>> {
        use oxoffice_docx_layout::{SourceNodeId, WordSourcePath, WordStory};
        let mut nodes = Vec::new();
        for index in 1u32.. {
            let Some(node) = SourceNodeId::new(index) else {
                break;
            };
            let Some(path) = layout.source_node(node) else {
                break;
            };
            if matches!(path.story, WordStory::TextBox { .. }) {
                nodes.push((node, path));
            }
        }
        let placed = placed_page_fields(layout);
        let section_formats = section_page_formats(&self.document);
        let mut result = Vec::new();
        for source in self.text_box_cache_paragraphs()? {
            let crate::document::TextBoxCacheParagraph {
                location,
                xml,
                owner_index,
                paragraph_index,
            } = source;
            let mut paragraph = parsed_physical_field_paragraph(&xml)?;
            let mut fields = Vec::new();
            for field in paragraph
                .source_runs()
                .into_iter()
                .flat_map(|run| &run.content)
                .filter_map(|content| match content {
                    RunContent::Field(field) => Some(field),
                    _ => None,
                })
            {
                collect_preorder_fields(field, false, &mut fields);
            }
            if fields.is_empty() {
                continue;
            }
            let raw = fields
                .iter()
                .map(|(field, _)| {
                    field
                        .source_replacement()
                        .ok()
                        .flatten()
                        .map(|(source, _)| source.to_vec())
                })
                .collect::<Vec<_>>();
            let matching = nodes.iter().filter(|(node, path)| {
                matches!(&path.story, WordStory::TextBox { part_name, .. } if part_name == location.story().part_name())
                    && layout.text_box_owner_index(&path.story) == Some(owner_index)
                    && layout.text_box_paragraph_index(*node) == Some(paragraph_index)
                    && raw.iter().enumerate().all(|(index, raw)| raw.as_deref().is_some_and(|raw| layout.source_field_xml(oxoffice_layout::FieldSource { node: *node, index: index as u32 }) == Some(raw)))
                    && layout.source_field_xml(oxoffice_layout::FieldSource { node: *node, index: raw.len() as u32 }).is_none()
            }).collect::<Vec<_>>();
            let paths = if matching.len() == 1 {
                vec![matching[0].1.clone()]
            } else {
                Vec::<WordSourcePath>::new()
            };
            let mut updates = Vec::new();
            push_page_field_updates(
                &[&paragraph],
                &[paths],
                &placed,
                layout,
                &section_formats,
                &mut updates,
                report,
            );
            let count = updates.iter().flatten().count();
            if count == 0 {
                continue;
            }
            let original = paragraph.clone();
            let mut consumed = 0;
            apply_updates_to_paragraph(&mut paragraph, &updates, &mut consumed)?;
            if consumed != updates.len() {
                return Err(Error::Other(
                    "text-box field traversal lost source identity".to_owned(),
                ));
            }
            let replacements = paragraph_field_source_replacements(&original, &paragraph)?;
            if replacements.is_empty() {
                return Err(Error::Other(
                    "text-box cache edit has no producer source span".to_owned(),
                ));
            }
            result.push(TextBoxCachePatch {
                location,
                replacements,
                updated: count,
            });
        }
        Ok(result)
    }

    /// Rebuild every supported table of contents already present in the document.
    ///
    /// The operation stages bookmarks, cached entries, and deterministic page
    /// targets on an independent document. Any malformed or ambiguous source
    /// leaves the receiver unchanged.
    ///
    /// The cached entry paragraphs are replaced, so comment and bookmark
    /// markers placed on them are dropped with them. A comment anchored only
    /// there stays in the comments part without an anchor.
    pub fn rebuild_toc(&mut self) -> Result<TocRebuildReport> {
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_package()?;
        let document_xml = candidate
            .package
            .get_part(&candidate.doc_part_name)
            .ok_or(Error::NoDocumentPart)?
            .to_vec();
        let toc_spans = scan_dynamic_table_spans(&document_xml, DynamicOwnerPolicy::Toc)?;
        let mut diagnostics = collect_simple_toc_diagnostics(&document_xml)?;
        if toc_spans.is_empty() {
            return Ok(TocRebuildReport {
                diagnostics: diagnostics
                    .into_iter()
                    .map(|(_, diagnostic)| diagnostic)
                    .collect(),
                ..Default::default()
            });
        }

        let (toc_fields, mut dynamic_diagnostics) =
            parse_dynamic_toc_fields(&candidate, &document_xml, &toc_spans)?;
        diagnostics.append(&mut dynamic_diagnostics);
        diagnostics.sort_by_key(|(offset, _)| *offset);
        let mut diagnostics = diagnostics
            .into_iter()
            .map(|(_, diagnostic)| diagnostic)
            .collect::<Vec<_>>();
        if toc_fields.iter().all(Option::is_none) {
            return Ok(TocRebuildReport {
                diagnostics,
                ..Default::default()
            });
        }
        // Layout and dynamic-instruction parsing prepare their own clones.
        // Source discovery below consumes this actual staged body instead.
        crate::document::prepare_semantic_body_projection(&mut candidate.document.body.content)?;
        let bookmark_state = inspect_toc_bookmarks(&candidate.document.body, &document_xml)?;
        let numbering_layout = candidate.layout_deterministic()?;
        let sources = discover_toc_sources(
            &candidate,
            &toc_spans,
            &toc_fields,
            &bookmark_state,
            &numbering_layout,
        )?;
        diagnostics.extend(toc_duplicate_style_diagnostics(&candidate.styles));
        diagnostics.extend(toc_default_style_diagnostics(&candidate.styles));
        let (toc_entry_styles, retained_style_defects) =
            ensure_toc_entry_styles(&mut candidate, &sources)?;
        let entry_style_ids = toc_entry_style_ids(&candidate.styles, &toc_entry_styles);
        diagnostics.extend(retained_style_defects);
        candidate.flush_to_package()?;
        let rebuilt_toc_spans = toc_spans
            .iter()
            .zip(&toc_fields)
            .filter_map(|(span, field)| field.is_some().then_some(span.clone()))
            .collect::<Vec<_>>();
        let bookmark_repairs =
            toc_crossing_bookmark_repairs(&bookmark_state, &toc_spans, &toc_fields);
        let authored_bookmark_names = candidate.identifiers.authored_bookmark_names().clone();
        let mut allocator = TocBookmarkAllocator::new(bookmark_state, &authored_bookmark_names);
        let mut bookmark_by_paragraph = BTreeMap::<usize, TocBookmark>::new();
        let mut bookmark_name_remap = BTreeMap::<String, String>::new();
        for source in sources.iter().flatten() {
            if !source.needs_bookmark {
                continue;
            }
            if bookmark_by_paragraph.contains_key(&source.paragraph_index) {
                continue;
            }
            let is_partial_boundary_source = rebuilt_toc_spans.iter().any(|span| {
                source.paragraph_index == span.begin_paragraph
                    || source.paragraph_index == span.end_paragraph
            });
            if !is_partial_boundary_source
                && let Some(existing) = allocator.whole_paragraph_name(source.paragraph_index)
            {
                let authored = candidate.identifiers.is_authored_bookmark(existing.0);
                let name = if authored {
                    let name = allocator.allocate_name()?;
                    if name != existing.1 {
                        bookmark_name_remap.insert(existing.1, name.clone());
                    }
                    name
                } else {
                    existing.1
                };
                bookmark_by_paragraph.insert(
                    source.paragraph_index,
                    TocBookmark {
                        id: existing.0,
                        name,
                        insert: false,
                        authored,
                    },
                );
            } else {
                let allocated = allocator.allocate(&mut candidate.identifiers)?;
                bookmark_by_paragraph.insert(
                    source.paragraph_index,
                    TocBookmark {
                        id: allocated.0,
                        name: allocated.1,
                        insert: true,
                        authored: true,
                    },
                );
            }
        }
        if !bookmark_name_remap.is_empty() {
            let remap = BodyIdentityRemap {
                bookmark_names: bookmark_name_remap,
                ..Default::default()
            };
            let renamed_xml = patch_body_identity_attributes(&document_xml, &remap)?;
            CT_Document::from_xml(&renamed_xml)?;
            candidate
                .package
                .set_part(&candidate.doc_part_name, renamed_xml);
            let mut renamed = reopen_staged_document(candidate)?;
            restore_toc_bookmark_provenance(&mut renamed, &bookmark_by_paragraph);
            let report = renamed.rebuild_toc()?;
            self.commit_staged_mutation(renamed);
            return Ok(report);
        }
        let bookmark_count = bookmark_by_paragraph
            .values()
            .filter(|bookmark| bookmark.insert)
            .count();
        let provisional_xml = insert_toc_bookmarks_xml(
            &document_xml,
            &bookmark_by_paragraph,
            &bookmark_repairs,
            &toc_spans,
            &rebuilt_toc_spans,
        )?;
        CT_Document::from_xml(&provisional_xml)?;
        candidate
            .package
            .set_part(&candidate.doc_part_name, provisional_xml.clone());
        candidate = reopen_staged_document(candidate)?;
        let provisional_spans =
            scan_dynamic_table_spans(&provisional_xml, DynamicOwnerPolicy::Toc)?;
        if provisional_spans.len() != toc_fields.len() {
            return Err(Error::Other(
                "table of contents ownership changed while staging bookmarks".to_owned(),
            ));
        }
        let end_bookmark_starts = provisional_spans
            .iter()
            .enumerate()
            .map(|(toc_index, span)| {
                end_boundary_bookmark_starts(
                    toc_index,
                    span,
                    &provisional_spans,
                    &toc_fields,
                    &bookmark_by_paragraph,
                    &bookmark_repairs,
                )
            })
            .collect::<Vec<_>>();
        let provisional_paragraphs = toc_paragraph_insertions(&provisional_xml)?;
        let mut placeholders = Vec::new();
        let mut edits = Vec::new();
        let mut entry_count = 0usize;
        for (toc_index, ((span, toc), toc_sources)) in provisional_spans
            .iter()
            .zip(&toc_fields)
            .zip(&sources)
            .enumerate()
        {
            let Some(toc) = toc else {
                continue;
            };
            let generated = render_toc_entries(
                toc_index,
                toc,
                toc_sources,
                &toc_entry_styles,
                toc_section_text_width(&candidate.document.body, span.begin_paragraph),
                &bookmark_by_paragraph,
                &provisional_xml,
                &mut placeholders,
            )?;
            entry_count += toc_sources.len();
            if let Some(mut merged) = entry_boundary_toc_edits(
                &provisional_xml,
                span,
                &provisional_paragraphs,
                &generated,
                &entry_style_ids,
                &end_bookmark_starts[toc_index],
            )? {
                edits.append(&mut merged);
                continue;
            }
            if span.begin_paragraph == span.end_paragraph {
                return Err(Error::Other(
                    "table of contents result must span paragraph boundaries".to_owned(),
                ));
            }
            edits.push(FieldSourceEdit {
                start: span.result_start,
                end: span.result_end,
                replacement: dynamic_toc_replacement(
                    &provisional_xml,
                    span,
                    &generated,
                    &end_bookmark_starts[toc_index],
                )?,
            });
        }
        let mut staged_xml = provisional_xml;
        for edit in edits.into_iter().rev() {
            staged_xml.splice(edit.start..edit.end, edit.replacement);
        }
        CT_Document::from_xml(&staged_xml)?;
        candidate
            .package
            .set_part(&candidate.doc_part_name, staged_xml);
        let mut provisional = reopen_staged_document(candidate)?;

        let page_values = deterministic_toc_page_values(&provisional)?;
        let current_xml = provisional
            .package
            .get_part(&provisional.doc_part_name)
            .ok_or(Error::NoDocumentPart)?;
        let mut final_xml = current_xml.to_vec();
        let final_spans = scan_dynamic_table_spans(&final_xml, DynamicOwnerPolicy::Toc)?;
        if final_spans.len() != toc_fields.len() {
            return Err(Error::Other(
                "table of contents ownership changed during page substitution".to_owned(),
            ));
        }
        let mut page_edits = Vec::with_capacity(placeholders.len());
        for placeholder in &placeholders {
            let target = page_values.get(&placeholder.bookmark).ok_or_else(|| {
                Error::Other(format!(
                    "table of contents page target {} was not resolved",
                    placeholder.bookmark
                ))
            })?;
            let span = &final_spans[placeholder.toc_index];
            let owned = &final_xml[span.result_start..span.result_end];
            let matches = byte_match_offsets(owned, placeholder.token.as_bytes());
            if matches.len() != 1 {
                return Err(Error::Other(format!(
                    "table of contents page placeholder {} was not uniquely owned",
                    placeholder.token
                )));
            }
            let start = span.result_start + matches[0];
            page_edits.push(FieldSourceEdit {
                start,
                end: start + placeholder.token.len(),
                replacement: target.as_bytes().to_vec(),
            });
        }
        page_edits.sort_by_key(|edit| edit.start);
        for edit in page_edits.into_iter().rev() {
            final_xml.splice(edit.start..edit.end, edit.replacement);
        }
        let final_spans = scan_dynamic_table_spans(&final_xml, DynamicOwnerPolicy::Toc)?;
        final_xml =
            relocate_end_boundary_bookmark_starts(final_xml, &final_spans, &end_bookmark_starts)?;
        CT_Document::from_xml(&final_xml)?;
        provisional
            .package
            .set_part(&provisional.doc_part_name, final_xml);
        let mut completed = reopen_staged_document(provisional)?;
        restore_toc_bookmark_provenance(&mut completed, &bookmark_by_paragraph);
        completed.invalidate_layout();
        self.commit_staged_mutation(completed);

        Ok(TocRebuildReport {
            entry_count,
            bookmark_count,
            diagnostics,
        })
    }

    fn update_fields_with_policy(
        &mut self,
        context: &FieldEvaluationContext,
        missing_merge_fields_as_empty: bool,
    ) -> Result<usize> {
        prepare_physical_story_projection(&mut self.document.body, &mut [])?;
        let (evaluations, visible) =
            self.evaluate_fields_with_policy(context, missing_merge_fields_as_empty)?;
        let original = self.clone_for_staging();
        let mut fields = Vec::new();
        let mut body = Vec::new();
        collect_body_paragraphs(&original.document.body, &mut body, None);
        for paragraph in body {
            append_ref_copy_paragraph(paragraph, false, &mut fields);
        }
        for (_, kind, xml) in field_story_parts(&original, true) {
            for record in legacy_story_paragraphs(&xml, kind)? {
                append_ref_copy_paragraph(&record.paragraph, true, &mut fields);
            }
        }
        for source in original.text_box_cache_paragraphs()? {
            append_ref_copy_paragraph(
                &parsed_physical_field_paragraph(&source.xml)?,
                true,
                &mut fields,
            );
        }
        if fields.len() != evaluations.len() {
            return Err(Error::Other(
                "REF copy inventory disagrees with physical field evaluations".into(),
            ));
        }
        let mut updates = evaluations
            .iter()
            .map(|evaluation| match &evaluation.outcome {
                FieldOutcome::Resolved(value) => CachedFieldUpdate {
                    typed_runs: None,
                    comment_ranges: Vec::new(),
                    cached_result: value.clone(),
                    dirty: false,
                },
                FieldOutcome::DeferredPagination
                | FieldOutcome::TableOfContents(_)
                | FieldOutcome::TableOfContentsEntry(_)
                | FieldOutcome::MailMergeControl(_)
                | FieldOutcome::Barcode(_)
                | FieldOutcome::KeepStored { .. } => CachedFieldUpdate {
                    typed_runs: None,
                    comment_ranges: Vec::new(),
                    cached_result: evaluation.cached_result.clone(),
                    dirty: true,
                },
            })
            .map(Some)
            .collect::<Vec<_>>();
        for ((source, update), visible) in fields.iter().zip(&mut updates).zip(visible) {
            if source.locked || !visible {
                *update = None;
            }
        }
        let source_projection = if fields.iter().any(|source| {
            let instruction = source.field.effective_instruction();
            instruction.name == "REF"
                && has_switch(&instruction, "f")
                && !source.locked
                && !source.generated
        }) {
            let sequence_updates = fields
                .iter()
                .zip(&evaluations)
                .map(|(source, evaluation)| {
                    if source.locked
                        || source.generated
                        || source.field.effective_instruction().name != "SEQ"
                    {
                        return None;
                    }
                    if let FieldOutcome::Resolved(value) = &evaluation.outcome {
                        Some(CachedFieldUpdate {
                            cached_result: value.clone(),
                            dirty: false,
                            typed_runs: None,
                            comment_ranges: Vec::new(),
                        })
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            if sequence_updates.iter().any(Option::is_some) {
                let mut projection = original.clone_for_staging();
                projection.apply_cached_physical_field_updates(&sequence_updates)?;
                let canonical = projection.main_part_with_body_spans()?.0;
                projection.document =
                    crate::document::DocumentModel::unproved(CT_Document::from_xml(&canonical)?);
                Some(projection)
            } else {
                None
            }
        } else {
            None
        };
        let ref_source = source_projection.as_ref().unwrap_or(&original);
        let mut note_copies = Vec::new();
        let mut comment_copies = Vec::new();
        let mut occupied_comments = original
            .comments
            .as_ref()
            .map(|comments| {
                comments
                    .comments
                    .iter()
                    .map(|comment| comment.id)
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        let mut occupied_comment_identities = None;
        let mut occupied_notes = HashMap::new();
        let note_reference_counts = if fields.iter().any(|source| {
            let instruction = source.field.effective_instruction();
            !source.suppress_copied_references
                && !source.locked
                && !source.generated
                && instruction.name == "REF"
                && has_switch(&instruction, "f")
        }) {
            ref_note_reference_counts(&original.package)?
        } else {
            HashMap::new()
        };
        for (source, update) in fields.iter().zip(&mut updates) {
            let instruction = source.field.effective_instruction();
            if instruction.name != "REF" || !has_switch(&instruction, "f") {
                continue;
            }
            if source.locked || source.generated {
                *update = None;
                continue;
            }
            if update.as_ref().is_some_and(|update| update.dirty) {
                *update = None;
                continue;
            }
            let Some(update) = update else {
                continue;
            };
            let target = text_argument(&instruction, 0)
                .ok_or_else(|| Error::Other("REF f target is absent".into()))?;
            let Some(cache) = ref_bookmark_runs(ref_source, target)? else {
                continue;
            };
            let comment_replacements = if source.suppress_copied_references {
                Vec::new()
            } else {
                ref_comment_replacement_ids(&original, &source.field, &cache)?
            };
            let mut comment_cursor = 0;
            let mut runs = cache.runs;
            let mut comment_ranges = if source.suppress_copied_references {
                Vec::new()
            } else {
                cache.comment_ranges
            };
            let mut old_references = Vec::new();
            collect_ref_cached_note_references(&source.field, &mut old_references)?;
            for run in &mut runs {
                if source.suppress_copied_references {
                    run.content.retain(|content| {
                        !matches!(
                            content,
                            RunContent::FootnoteRef { .. }
                                | RunContent::EndnoteRef { .. }
                                | RunContent::CommentReference { .. }
                        )
                    });
                } else {
                    for content in &mut run.content {
                        if let RunContent::CommentReference { id, .. } = content {
                            if occupied_comment_identities.is_none() {
                                occupied_comment_identities =
                                    Some(ref_comment_occupied_identity_values(&original.package)?);
                            }
                            let (new_id, copy) = prepare_ref_comment_copy(
                                self,
                                ref_source,
                                *id,
                                comment_replacements[comment_cursor],
                                &mut occupied_comments,
                                occupied_comment_identities.as_mut().unwrap(),
                            )?;
                            comment_cursor += 1;
                            for marker in &mut comment_ranges {
                                let marker_id = match marker {
                                    CommentRangeMarker::Start { id, .. }
                                    | CommentRangeMarker::End { id, .. } => id,
                                };
                                if *marker_id == *id {
                                    *marker_id = new_id;
                                }
                            }
                            *id = new_id;
                            comment_copies.push(copy);
                            continue;
                        }
                        let kind = match content {
                            RunContent::FootnoteRef { .. } => StoryKind::Footnote,
                            RunContent::EndnoteRef { .. } => StoryKind::Endnote,
                            _ => continue,
                        };
                        let id = match content {
                            RunContent::FootnoteRef { id, .. }
                            | RunContent::EndnoteRef { id, .. } => id,
                            _ => unreachable!("note kind"),
                        };
                        let replacement = old_references
                            .iter()
                            .position(|(family, _)| *family == kind)
                            .map(|index| old_references.remove(index).1)
                            .filter(|old| {
                                *old != *id && note_reference_counts.get(&(kind, *old)) == Some(&1)
                            });
                        let (new_id, copy) = prepare_ref_note_copy(
                            self,
                            ref_source,
                            kind,
                            *id,
                            replacement,
                            &mut occupied_notes,
                        )?;
                        *id = new_id;
                        note_copies.push(copy);
                    }
                }
            }
            let mut candidate = source.field.clone();
            candidate.set_cached_runs_with_comment_ranges(runs.clone(), comment_ranges.clone())?;
            update.cached_result = candidate.cached_result;
            update.typed_runs = Some(runs);
            update.comment_ranges = comment_ranges;
        }
        let updated = self.apply_cached_physical_field_updates(&updates)?;
        for copy in note_copies {
            self.publish_fragment_note_staged(
                copy.kind,
                &copy.destination_part,
                copy.xml,
                copy.replace_id,
            )?;
        }
        for copy in comment_copies {
            publish_ref_comment_copy(self, copy)?;
        }
        Ok(updated)
    }

    fn apply_cached_physical_field_updates(
        &mut self,
        updates: &[Option<CachedFieldUpdate>],
    ) -> Result<usize> {
        let boxes = self.text_box_cache_paragraphs()?;
        let mut paragraphs = Vec::new();
        let mut box_count = 0;
        for source in boxes {
            let paragraph = parsed_physical_field_paragraph(&source.xml)?;
            let mut fields = Vec::new();
            for run in accepted_toc_runs(&paragraph) {
                for content in &run.run.content {
                    if let RunContent::Field(field) = content {
                        collect_preorder_fields(field, false, &mut fields);
                    }
                }
            }
            if fields.is_empty() {
                continue;
            }
            let count = fields.len();
            box_count += count;
            paragraphs.push((source.location, paragraph, count));
        }
        let split = updates.len().checked_sub(box_count).ok_or_else(|| {
            Error::Other("text-box update inventory exceeds field evaluations".into())
        })?;
        let mut cursor = split;
        let mut patches = Vec::new();
        for (location, mut paragraph, count) in paragraphs {
            let end = cursor + count;
            if !updates[cursor..end].iter().any(Option::is_some) {
                cursor = end;
                continue;
            }
            let original = paragraph.clone();
            let mut consumed = 0;
            apply_updates_to_paragraph(&mut paragraph, &updates[cursor..end], &mut consumed)?;
            if consumed != count {
                return Err(Error::Other(
                    "text-box update field inventory is inconsistent".into(),
                ));
            }
            let replacements = paragraph_field_source_replacements(&original, &paragraph)?;
            if replacements.is_empty() {
                return Err(Error::Other(
                    "text-box update has no preserved field source".into(),
                ));
            }
            patches.push(TextBoxCachePatch {
                location,
                replacements,
                updated: updates[cursor..end].iter().flatten().count(),
            });
            cursor = end;
        }
        if cursor != updates.len() {
            return Err(Error::Other(
                "text-box update left unconsumed evaluations".into(),
            ));
        }
        let mut updated = self.apply_cached_field_updates(&updates[..split], true)?;
        for patch in patches {
            self.patch_story_paragraph_field_sources(&patch.location, &patch.replacements)?;
            updated += patch.updated;
        }
        Ok(updated)
    }

    fn apply_cached_field_updates(
        &mut self,
        updates: &[Option<CachedFieldUpdate>],
        include_comments: bool,
    ) -> Result<usize> {
        if updates
            .iter()
            .flatten()
            .any(|update| !update.cached_result.chars().all(valid_xml_character))
        {
            return Err(Error::Other(
                "field result contains a character forbidden by XML 1.0".to_owned(),
            ));
        }
        let updated = updates.iter().flatten().count();
        if updated == 0 {
            return Ok(0);
        }
        let package_before = self.package.clone();
        let mut update_index = 0;
        let main_source = self.updated_main_field_source(updates, &mut update_index)?;
        let mut staged = Vec::new();
        for (part_name, kind, xml) in field_story_parts(self, include_comments) {
            let mut paragraphs = legacy_story_paragraphs(&xml, kind)?;
            let first = update_index;
            for entry in &mut paragraphs {
                apply_updates_to_paragraph(&mut entry.paragraph, updates, &mut update_index)?;
            }
            if updates[first..update_index].iter().any(Option::is_some) {
                let xml = patch_legacy_story_field_sources(&xml, &paragraphs, true)?;
                match kind {
                    PackageStoryKind::Header | PackageStoryKind::Footer => {
                        CT_HdrFtr::from_xml(&xml)?;
                    }
                    PackageStoryKind::Footnotes | PackageStoryKind::Endnotes => {
                        CT_Footnotes::from_xml(&xml)?;
                    }
                    PackageStoryKind::Comments => {
                        CT_Comments::from_xml(&xml)?;
                    }
                    PackageStoryKind::Document => {
                        CT_Document::from_xml(&xml)?;
                    }
                }
                staged.push((part_name, kind, xml));
            }
        }
        if update_index != updates.len() {
            return Err(Error::Other(format!(
                "page field traversal consumed {update_index} of {} staged evaluations",
                updates.len()
            )));
        }
        if let Some(xml) = main_source {
            let part = self.doc_part_name.clone();
            crate::document::set_story_source_xml(self, &part, xml)?;
        }
        for (part_name, kind, xml) in staged {
            if matches!(kind, PackageStoryKind::Footnotes) {
                self.footnotes = CT_Footnotes::from_xml(&xml)?;
                self.footnotes_dirty = false;
            }
            if matches!(kind, PackageStoryKind::Comments) {
                self.comments = Some(CT_Comments::from_xml(&xml)?);
                self.comments_dirty = false;
            }
            self.package.set_part(&part_name, xml);
        }
        self.package_signatures_invalidated |= self
            .retained_package_signature_would_be_invalidated()?
            || crate::embedded::synchronized_package_mutation_invalidates_signature(
                &package_before,
                &self.package,
            );
        self.invalidate_layout();
        Ok(updated)
    }

    // Map the one immutable current writer into the guarded restored source.
    // Paths select owners, same-context emission proves each occurrence, and
    // only those independently parsed source owners receive cache updates.
    fn updated_main_field_source(
        &self,
        updates: &[Option<CachedFieldUpdate>],
        update_index: &mut usize,
    ) -> Result<Option<Vec<u8>>> {
        self.validate_model_resource_limits()?;
        let (canonical, _, roots) = self.main_part_with_body_spans()?;
        let source = self.namespace_restored_main_source()?;
        let (boundaries, _) =
            crate::document::comment_namespace_source_boundaries(&canonical, &source)?.ok_or_else(
                || Error::Other("field-cache restored source changed physical events".into()),
            )?;
        let mut inverse = BTreeMap::new();
        for (before, after) in boundaries {
            if inverse
                .insert(after, before)
                .is_some_and(|prior| prior != before)
            {
                return Err(Error::Other(
                    "field-cache source has ambiguous event boundaries".into(),
                ));
            }
        }
        if roots.len() != self.document.body.content.len() {
            return Err(Error::Other(
                "field-cache direct body inventory changed".into(),
            ));
        }
        for (item, range) in self.document.body.content.iter().zip(&roots) {
            let emitted = self.document.body_item_xml(item)?;
            crate::document::note_transaction_buffer("field_cache_root_emission", emitted.len());
            if canonical.get(range.clone()) != Some(emitted.as_slice()) {
                return Err(Error::Other(
                    "field-cache direct body emitter differs from current source".into(),
                ));
            }
        }
        let root_indices = self
            .document
            .body
            .content
            .iter()
            .enumerate()
            .filter_map(|(index, item)| (!matches!(item, BodyContent::RawXml(_))).then_some(index))
            .collect::<Vec<_>>();
        let mut paragraphs = Vec::new();
        let mut paths = Vec::new();
        collect_body_paragraphs(&self.document.body, &mut paragraphs, Some(&mut paths));
        let parsed = legacy_story_paragraphs(&source, PackageStoryKind::Document)?;
        if parsed.len() != paragraphs.len() || paths.len() != paragraphs.len() {
            return Err(Error::Other(
                "field-cache modeled/source paragraph inventory differs".into(),
            ));
        }
        let mut source_paragraphs = BTreeMap::new();
        for paragraph in parsed {
            if source_paragraphs
                .insert(paragraph.path.clone(), paragraph)
                .is_some()
            {
                return Err(Error::Other(
                    "field-cache duplicate physical paragraph path".into(),
                ));
            }
        }
        let canonical_starts = source_paragraphs
            .values()
            .map(|paragraph| {
                inverse.get(&paragraph.start).copied().ok_or_else(|| {
                    Error::Other("field-cache paragraph start lacks a source boundary".into())
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let (ancestor_frames, ancestor_paths) =
            crate::document::story_namespace_ancestors_at(&canonical, &canonical_starts)?;
        let mut edits = Vec::<FieldSourceEdit>::new();
        for (paragraph, path) in paragraphs.into_iter().zip(paths) {
            let mut physical = source_paragraphs.remove(&path).ok_or_else(|| {
                Error::Other("field-cache paragraph has no matching container path".into())
            })?;
            let first = *inverse.get(&physical.start).ok_or_else(|| {
                Error::Other("field-cache paragraph start lacks a source boundary".into())
            })?;
            let last = *inverse.get(&physical.end).ok_or_else(|| {
                Error::Other("field-cache paragraph end lacks a source boundary".into())
            })?;
            let root = path
                .first()
                .and_then(|(_, index)| root_indices.get(*index))
                .and_then(|index| roots.get(*index))
                .ok_or_else(|| {
                    Error::Other("field-cache paragraph lacks a direct body certificate".into())
                })?;
            if first > last || first < root.start || last > root.end {
                return Err(Error::Other(
                    "field-cache paragraph escapes its certified body owner".into(),
                ));
            }
            paragraph.validate_resource_limits()?;
            let ancestor_indices = ancestor_paths.get(&first).ok_or_else(|| {
                Error::Other("field-cache paragraph lacks writer-time ancestors".into())
            })?;
            let ancestors = ancestor_indices
                .iter()
                .map(|index| ancestor_frames[*index].as_slice())
                .collect::<Vec<_>>();
            let emitted = self.document.body_item_xml_with_ancestor_declarations(
                &BodyContent::Paragraph(Box::new(paragraph.clone())),
                &ancestors,
            )?;
            crate::document::note_transaction_buffer(
                "field_cache_paragraph_emission",
                emitted.len(),
            );
            if canonical.get(first..last) != Some(emitted.as_slice()) {
                return Err(Error::Other(
                    "field-cache paragraph emitter differs at its proved physical path".into(),
                ));
            }
            if !same_field_inventory(paragraph, &physical.original) {
                return Err(Error::Other(
                    "field-cache evaluation and physical field inventories differ".into(),
                ));
            }
            let before = *update_index;
            apply_updates_to_paragraph(&mut physical.paragraph, updates, update_index)?;
            if !updates
                .get(before..*update_index)
                .ok_or_else(|| Error::Other("field-cache update cursor escaped inventory".into()))?
                .iter()
                .any(Option::is_some)
            {
                continue;
            }
            let pairs =
                paragraph_field_source_replacements(&physical.original, &physical.paragraph)?;
            let mut cursor = 0;
            for (original, replacement) in pairs {
                if original.is_empty() {
                    return Err(Error::Other("empty field source identity".into()));
                }
                let relative = find_typed_field_source(
                    &source,
                    physical.start,
                    physical.end,
                    &original,
                    cursor,
                )?
                .ok_or_else(|| {
                    Error::Other("field cache source left its proved paragraph owner".into())
                })?;
                cursor = relative
                    .checked_add(original.len())
                    .ok_or_else(|| Error::Other("field-cache source bounds overflow".into()))?;
                if original == replacement {
                    continue;
                }
                let start = physical
                    .start
                    .checked_add(relative)
                    .ok_or_else(|| Error::Other("field-cache source bounds overflow".into()))?;
                let end = start
                    .checked_add(original.len())
                    .filter(|end| *end <= physical.end)
                    .ok_or_else(|| {
                        Error::Other("field-cache source escaped paragraph bounds".into())
                    })?;
                edits.push(FieldSourceEdit {
                    start,
                    end,
                    replacement,
                });
            }
        }
        if !source_paragraphs.is_empty() {
            return Err(Error::Other(
                "field-cache physical paragraph inventory not consumed".into(),
            ));
        }
        if edits.is_empty() {
            return Ok(None);
        }
        edits.sort_by_key(|edit| edit.start);
        let mut cursor = 0;
        let mut length = source.len();
        for edit in &edits {
            if edit.start < cursor || edit.start > edit.end || edit.end > source.len() {
                return Err(Error::Other(
                    "field-cache source edits overlap or escape source".into(),
                ));
            }
            length = length
                .checked_sub(edit.end - edit.start)
                .and_then(|length| length.checked_add(edit.replacement.len()))
                .ok_or_else(|| Error::Other("field-cache output length overflow".into()))?;
            cursor = edit.end;
        }
        let mut output = Vec::with_capacity(length);
        cursor = 0;
        for edit in edits {
            output.extend_from_slice(&source[cursor..edit.start]);
            output.extend_from_slice(&edit.replacement);
            cursor = edit.end;
        }
        output.extend_from_slice(&source[cursor..]);
        debug_assert_eq!(output.len(), length);
        crate::document::note_transaction_output("field_cache_source_patch", output.len());
        Ok(Some(output))
    }

    /// Update typed field caches, then save the package to a file path.
    pub fn save_with_field_updates<P: AsRef<Path>>(
        &mut self,
        path: P,
        context: &FieldEvaluationContext,
    ) -> Result<()> {
        self.update_fields(context)?;
        self.save(path)
    }

    /// Update typed field caches, then save the package to bytes.
    pub fn to_bytes_with_field_updates(
        &mut self,
        context: &FieldEvaluationContext,
    ) -> Result<Vec<u8>> {
        self.update_fields(context)?;
        self.to_bytes()
    }
}

fn restore_toc_bookmark_provenance(
    document: &mut Document,
    bookmarks: &BTreeMap<usize, TocBookmark>,
) {
    for bookmark in bookmarks.values().filter(|bookmark| bookmark.authored) {
        document
            .identifiers
            .restore_authored_bookmark(bookmark.id, &bookmark.name);
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

#[derive(Debug)]
struct DynamicFieldScan {
    instruction: String,
    field_start: usize,
    begin_paragraph: usize,
    begin_paragraph_start: usize,
    begin_run_start: usize,
    separator_paragraph: Option<usize>,
    separator_run_start: Option<usize>,
    result_start: Option<usize>,
    result_start_position: Option<TocRunPosition>,
    start_paragraph_name: Option<String>,
    start_paragraph_namespaces: BTreeMap<String, String>,
    separator_wrapper_names: Vec<String>,
    instruction_runs: Vec<DynamicInstructionRun>,
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

#[derive(Debug)]
struct DynamicHyperlinkPlan {
    revision_orders: Vec<TocRawOrder>,
    next_revision: usize,
    preserved_raw: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypedBlockOwner {
    Document,
    Body,
    Table,
    Row,
    Cell,
    ContentControl(LegacyBlockOwner),
    Content(LegacyBlockOwner),
    Paragraph,
}

const MAX_DYNAMIC_REVISION_NESTING_DEPTH: usize = 32;

fn dynamic_namespace_bindings(
    element: &BytesStart<'_>,
    elements: &[DynamicXmlElement],
) -> Result<(BTreeMap<String, String>, BTreeMap<String, String>)> {
    let inherited = elements
        .last()
        .map_or_else(BTreeMap::new, |parent| parent.namespace_bindings.clone());
    let mut bindings = inherited.clone();
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("invalid XML namespace attribute: {error}")))?;
        let key = attribute.key.as_ref();
        let prefix = if key == b"xmlns" {
            Some(String::new())
        } else {
            key.strip_prefix(b"xmlns:")
                .map(|prefix| String::from_utf8_lossy(prefix).into_owned())
        };
        if let Some(prefix) = prefix {
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .map_err(|error| Error::Other(format!("invalid XML namespace attribute: {error}")))?
                .into_owned();
            if value.is_empty() {
                bindings.remove(&prefix);
            } else {
                bindings.insert(prefix, value);
            }
        }
    }
    Ok((bindings, inherited))
}

#[derive(Clone, Copy)]
enum DynamicContentControlOwner {
    Block(LegacyBlockOwner),
    Inline,
}

fn dynamic_element_end(xml: &[u8], start: usize) -> Result<usize> {
    let mut reader = quick_xml::Reader::from_reader(&xml[start..]);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    loop {
        match reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!("invalid table of contents XML element: {error}"))
        })? {
            Event::Start(_) => depth += 1,
            Event::Empty(_) if depth == 0 => {
                return Ok(start + reader.buffer_position() as usize);
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    Error::Other("table of contents XML element has an unmatched end".to_owned())
                })?;
                if depth == 0 {
                    return Ok(start + reader.buffer_position() as usize);
                }
            }
            Event::Eof => {
                return Err(Error::Other(
                    "table of contents XML element is unclosed".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
}

pub(crate) fn xml_fragment_with_namespaces(
    raw: &[u8],
    bindings: &BTreeMap<String, String>,
    description: &str,
) -> Result<Vec<u8>> {
    let mut reader = quick_xml::Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let (insertion, local_namespaces) = match reader
        .read_event_into(&mut buffer)
        .map_err(|error| Error::Other(format!("invalid {description}: {error}")))?
    {
        Event::Start(start) | Event::Empty(start) => {
            let mut local_namespaces = HashSet::new();
            for attribute in start.attributes() {
                let attribute = attribute
                    .map_err(|error| Error::Other(format!("invalid {description}: {error}")))?;
                let key = attribute.key.as_ref();
                if key == b"xmlns" {
                    local_namespaces.insert(String::new());
                } else if let Some(prefix) = key.strip_prefix(b"xmlns:") {
                    local_namespaces.insert(String::from_utf8_lossy(prefix).into_owned());
                }
            }
            let tag_end = reader.buffer_position() as usize;
            let insertion = if tag_end >= 2 && raw[tag_end - 2] == b'/' {
                tag_end - 2
            } else {
                tag_end - 1
            };
            (insertion, local_namespaces)
        }
        _ => return Err(Error::Other(format!("{description} has no start tag"))),
    };
    let mut output = Vec::with_capacity(raw.len() + bindings.len() * 32);
    output.extend_from_slice(&raw[..insertion]);
    for (prefix, namespace) in bindings {
        if prefix == "xml" || local_namespaces.contains(prefix) {
            continue;
        }
        if prefix.is_empty() {
            output.extend_from_slice(b" xmlns=\"");
        } else {
            output.extend_from_slice(b" xmlns:");
            output.extend_from_slice(prefix.as_bytes());
            output.extend_from_slice(b"=\"");
        }
        output.extend_from_slice(xml_escape_attribute(namespace).as_bytes());
        output.push(b'"');
    }
    output.extend_from_slice(&raw[insertion..]);
    Ok(output)
}

fn dynamic_content_control_is_typed(
    xml: &[u8],
    start: usize,
    bindings: &BTreeMap<String, String>,
    owner: DynamicContentControlOwner,
) -> Result<bool> {
    let end = dynamic_element_end(xml, start)?;
    let control = xml_fragment_with_namespaces(
        &xml[start..end],
        bindings,
        "table of contents content control",
    )?;
    let mut candidate = Vec::with_capacity(control.len() + 256);
    candidate.extend_from_slice(format!(r#"<w:document xmlns:w="{W_NS}"><w:body>"#).as_bytes());
    let (prefix, suffix): (&[u8], &[u8]) = match owner {
        DynamicContentControlOwner::Block(LegacyBlockOwner::Root) => (b"", b""),
        DynamicContentControlOwner::Block(LegacyBlockOwner::Table) => {
            (b"<w:tbl>", b"<w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>")
        }
        DynamicContentControlOwner::Block(LegacyBlockOwner::Row) => {
            (b"<w:tbl><w:tr>", b"<w:tc><w:p/></w:tc></w:tr></w:tbl>")
        }
        DynamicContentControlOwner::Block(LegacyBlockOwner::Cell) => {
            (b"<w:tbl><w:tr><w:tc>", b"<w:p/></w:tc></w:tr></w:tbl>")
        }
        DynamicContentControlOwner::Inline => (b"<w:p>", b"</w:p>"),
    };
    candidate.extend_from_slice(prefix);
    candidate.extend_from_slice(&control);
    candidate.extend_from_slice(suffix);
    candidate.extend_from_slice(b"</w:body></w:document>");
    let Ok(document) = CT_Document::from_xml(&candidate) else {
        return Ok(false);
    };
    let accepted = match owner {
        DynamicContentControlOwner::Block(LegacyBlockOwner::Root) => matches!(
            document.body.content.first(),
            Some(BodyContent::ContentControl(_))
        ),
        DynamicContentControlOwner::Block(LegacyBlockOwner::Table) => document
            .body
            .tables()
            .next()
            .is_some_and(|table| table.content_controls.len() == 1),
        DynamicContentControlOwner::Block(LegacyBlockOwner::Row) => document
            .body
            .tables()
            .next()
            .and_then(|table| table.rows.first())
            .is_some_and(|row| row.content_controls.len() == 1),
        DynamicContentControlOwner::Block(LegacyBlockOwner::Cell) => document
            .body
            .tables()
            .next()
            .and_then(|table| table.rows.first())
            .and_then(|row| row.cells.first())
            .is_some_and(|cell| {
                matches!(cell.content.first(), Some(CellContent::ContentControl(_)))
            }),
        DynamicContentControlOwner::Inline => document
            .body
            .paragraphs()
            .next()
            .is_some_and(|paragraph| paragraph.content_controls.len() == 1),
    };
    Ok(accepted)
}

fn validate_dynamic_content_control(
    xml: &[u8],
    start: usize,
    word: bool,
    local: &[u8],
    bindings: &BTreeMap<String, String>,
    typed_block_owner: &mut Option<TypedBlockOwner>,
    is_typed_inline_owner: &mut bool,
) -> Result<()> {
    if !word || local != b"sdt" {
        return Ok(());
    }
    let owner = match *typed_block_owner {
        Some(TypedBlockOwner::ContentControl(owner)) => {
            Some(DynamicContentControlOwner::Block(owner))
        }
        _ if *is_typed_inline_owner => Some(DynamicContentControlOwner::Inline),
        _ => None,
    };
    if let Some(owner) = owner
        && !dynamic_content_control_is_typed(xml, start, bindings, owner)?
    {
        *typed_block_owner = None;
        *is_typed_inline_owner = false;
    }
    Ok(())
}

fn is_dynamic_revision_element(local: &[u8]) -> bool {
    matches!(
        local,
        b"ins"
            | b"del"
            | b"moveFrom"
            | b"moveTo"
            | b"rPrChange"
            | b"pPrChange"
            | b"tblPrChange"
            | b"sectPrChange"
    )
}

fn invalidate_overdeep_revision_owner(
    word: bool,
    local: &[u8],
    elements: &mut [DynamicXmlElement],
) -> Option<usize> {
    if !word
        || !is_dynamic_revision_element(local)
        || elements
            .iter()
            .filter(|element| element.is_word && is_dynamic_revision_element(&element.local_name))
            .count()
            < MAX_DYNAMIC_REVISION_NESTING_DEPTH
    {
        return None;
    }
    let owner = elements.iter().position(|element| {
        element.is_typed_inline_owner && matches!(element.local_name.as_slice(), b"ins" | b"moveTo")
    })?;
    let start = elements[owner].start;
    for element in &mut elements[owner..] {
        element.is_typed_inline_owner = false;
    }
    Some(start)
}

fn mark_typed_sdt_content(
    elements: &mut [DynamicXmlElement],
    local: &[u8],
    typed_block_owner: Option<TypedBlockOwner>,
    is_typed_inline_owner: bool,
) {
    if local == b"sdtContent"
        && (matches!(typed_block_owner, Some(TypedBlockOwner::Content(_))) || is_typed_inline_owner)
        && let Some(parent) = elements.last_mut()
    {
        parent.sdt_content_seen = true;
    }
}

fn scan_dynamic_table_spans(
    xml: &[u8],
    generated: DynamicOwnerPolicy,
) -> Result<Vec<DynamicTocSpan>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut elements = Vec::<DynamicXmlElement>::new();
    let mut fields = Vec::<DynamicFieldScan>::new();
    let mut spans = Vec::<DynamicTocSpan>::new();
    let mut paragraph_count = 0usize;
    let mut paragraph_run_boundaries = Vec::<usize>::new();
    let mut paragraph_nested_run_orders = Vec::<HashMap<(usize, TocRawOrder), usize>>::new();
    let mut paragraph_raw_before = Vec::<usize>::new();
    let mut instruction_depth = None;

    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid table of contents XML: {error}")))?;
        let word = namespace_is_word(&namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let local = local_name(element.name().as_ref()).to_vec();
                if let Some(start) = invalidate_overdeep_revision_owner(word, &local, &mut elements)
                {
                    fields.retain(|field| field.field_start < start);
                    spans.retain(|span| span.field_start < start);
                    instruction_depth = None;
                }
                let (namespace_bindings, inherited_namespaces) =
                    dynamic_namespace_bindings(&element, &elements)?;
                let mut typed_block_owner = classify_typed_block_owner(word, &local, &elements);
                let is_typed_paragraph = typed_block_owner == Some(TypedBlockOwner::Paragraph);
                let paragraph = if is_typed_paragraph {
                    let index = paragraph_count;
                    paragraph_count += 1;
                    paragraph_run_boundaries.push(0);
                    paragraph_nested_run_orders.push(HashMap::new());
                    paragraph_raw_before.push(0);
                    Some(index)
                } else {
                    elements.last().and_then(|element| element.paragraph)
                };
                let mut is_typed_inline_owner = typed_inline_owner(
                    &element,
                    reader.resolver(),
                    word,
                    &local,
                    &elements,
                    paragraph,
                )?;
                validate_dynamic_content_control(
                    xml,
                    before,
                    word,
                    &local,
                    &namespace_bindings,
                    &mut typed_block_owner,
                    &mut is_typed_inline_owner,
                )?;
                let revision_depth = if is_typed_inline_owner {
                    elements.last().map_or(0, |parent| parent.revision_depth)
                        + usize::from(matches!(local.as_slice(), b"ins" | b"moveTo"))
                } else {
                    0
                };
                let modeled_simple_field = if word && local == b"fldSimple" {
                    resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"instr",
                        AttributeNamespace::Word,
                    )?
                    .is_some_and(|(_, instruction)| {
                        !Field::new(&instruction, "").instruction.name.is_empty()
                    })
                } else {
                    false
                };
                let run_position = dynamic_toc_run_position(
                    word,
                    &local,
                    is_typed_inline_owner,
                    modeled_simple_field,
                    &mut elements,
                    paragraph,
                    &mut paragraph_run_boundaries,
                    &mut paragraph_nested_run_orders,
                    &mut paragraph_raw_before,
                );
                let direct_paragraph_child = direct_typed_paragraph_parent(&elements, paragraph);
                let hyperlink_plan = if word && local == b"hyperlink" && direct_paragraph_child {
                    Some(dynamic_hyperlink_plan(
                        xml,
                        before,
                        &namespace_bindings,
                        paragraph
                            .and_then(|index| paragraph_raw_before.get(index).copied())
                            .ok_or_else(|| {
                                Error::Other(
                                    "table of contents hyperlink has no paragraph position"
                                        .to_owned(),
                                )
                            })?,
                    )?)
                } else {
                    None
                };
                advance_direct_paragraph_raw_child(
                    word,
                    &local,
                    is_typed_inline_owner,
                    modeled_simple_field,
                    direct_paragraph_child,
                    paragraph,
                    hyperlink_plan.as_ref(),
                    &mut paragraph_raw_before,
                );
                if word && local == b"fldChar" && direct_word_run_parent(&elements, paragraph) {
                    update_dynamic_field_stack(
                        &element,
                        reader.resolver(),
                        before,
                        paragraph,
                        &elements,
                        &mut fields,
                        &mut spans,
                        generated,
                    )?;
                }
                if word
                    && local == b"instrText"
                    && !fields.is_empty()
                    && direct_word_run_parent(&elements, paragraph)
                {
                    instruction_depth = Some(elements.len());
                }
                mark_typed_sdt_content(
                    &mut elements,
                    &local,
                    typed_block_owner,
                    is_typed_inline_owner,
                );
                elements.push(DynamicXmlElement {
                    local_name: local,
                    qualified_name: String::from_utf8_lossy(element.name().as_ref()).into_owned(),
                    typed_block_owner,
                    is_word: word,
                    is_typed_paragraph,
                    is_typed_inline_owner,
                    revision_depth,
                    sdt_content_seen: false,
                    namespace_bindings,
                    inherited_namespaces,
                    run_position,
                    hyperlink_plan,
                    start: before,
                    start_tag_end: after,
                    paragraph,
                });
            }
            Event::Empty(element) => {
                let local = local_name(element.name().as_ref()).to_vec();
                // Empty content is a retained raw slot, not the typed Start slot.
                let word = word && &local[..] != b"sdtContent";
                if let Some(start) = invalidate_overdeep_revision_owner(word, &local, &mut elements)
                {
                    fields.retain(|field| field.field_start < start);
                    spans.retain(|span| span.field_start < start);
                    instruction_depth = None;
                }
                let typed_block_owner = classify_typed_block_owner(word, &local, &elements);
                let paragraph = if typed_block_owner == Some(TypedBlockOwner::Paragraph) {
                    let index = paragraph_count;
                    paragraph_count += 1;
                    paragraph_run_boundaries.push(0);
                    paragraph_nested_run_orders.push(HashMap::new());
                    paragraph_raw_before.push(0);
                    Some(index)
                } else {
                    elements.last().and_then(|element| element.paragraph)
                };
                let is_typed_inline_owner = typed_inline_owner(
                    &element,
                    reader.resolver(),
                    word,
                    &local,
                    &elements,
                    paragraph,
                )?;
                let (namespace_bindings, inherited_namespaces) =
                    dynamic_namespace_bindings(&element, &elements)?;
                let modeled_simple_field = word
                    && local == b"fldSimple"
                    && resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"instr",
                        AttributeNamespace::Word,
                    )?
                    .is_some_and(|(_, instruction)| {
                        !Field::new(&instruction, "").instruction.name.is_empty()
                    });
                let run_position = dynamic_toc_run_position(
                    word,
                    &local,
                    is_typed_inline_owner,
                    modeled_simple_field,
                    &mut elements,
                    paragraph,
                    &mut paragraph_run_boundaries,
                    &mut paragraph_nested_run_orders,
                    &mut paragraph_raw_before,
                );
                let direct_paragraph_child = direct_typed_paragraph_parent(&elements, paragraph);
                advance_direct_paragraph_raw_child(
                    word,
                    &local,
                    is_typed_inline_owner,
                    modeled_simple_field,
                    direct_paragraph_child,
                    paragraph,
                    None,
                    &mut paragraph_raw_before,
                );
                mark_typed_sdt_content(
                    &mut elements,
                    &local,
                    typed_block_owner,
                    is_typed_inline_owner,
                );
                if generated != DynamicOwnerPolicy::Toc && modeled_simple_field {
                    let closed = DynamicXmlElement {
                        local_name: local.clone(),
                        qualified_name: String::from_utf8_lossy(element.name().as_ref())
                            .into_owned(),
                        typed_block_owner,
                        is_word: word,
                        is_typed_paragraph: false,
                        is_typed_inline_owner,
                        revision_depth: 0,
                        sdt_content_seen: false,
                        namespace_bindings,
                        inherited_namespaces,
                        run_position,
                        hyperlink_plan: None,
                        start: before,
                        start_tag_end: after,
                        paragraph,
                    };
                    if let Some(span) =
                        generated_simple_span(xml, &closed, &elements, after, after, generated)?
                    {
                        spans.push(span);
                    }
                }
                if word
                    && matches_local_name(element.name().as_ref(), b"fldChar")
                    && direct_word_run_parent(&elements, paragraph)
                {
                    update_dynamic_field_stack(
                        &element,
                        reader.resolver(),
                        before,
                        paragraph,
                        &elements,
                        &mut fields,
                        &mut spans,
                        generated,
                    )?;
                }
            }
            Event::Text(text) if instruction_depth.is_some() => {
                if let Some(field) = fields.last_mut()
                    && field.separator_paragraph.is_none()
                {
                    let decoded = text.decode().map_err(|error| {
                        Error::Other(format!("invalid table of contents instruction: {error}"))
                    })?;
                    let unescaped = quick_xml::escape::unescape(&decoded).map_err(|error| {
                        Error::Other(format!("invalid table of contents instruction: {error}"))
                    })?;
                    field.instruction.push_str(&unescaped);
                }
            }
            Event::GeneralRef(_)
                if generated == DynamicOwnerPolicy::Bibliography && instruction_depth.is_some() =>
            {
                if let Some(field) = fields.last_mut()
                    && field.separator_paragraph.is_none()
                {
                    let reference = std::str::from_utf8(&xml[before..after]).map_err(|error| {
                        Error::Other(format!(
                            "invalid bibliography instruction reference: {error}"
                        ))
                    })?;
                    let decoded = quick_xml::escape::unescape(reference).map_err(|error| {
                        Error::Other(format!(
                            "invalid bibliography instruction reference: {error}"
                        ))
                    })?;
                    field.instruction.push_str(&decoded);
                }
            }
            Event::CData(text) if instruction_depth.is_some() => {
                if let Some(field) = fields.last_mut()
                    && field.separator_paragraph.is_none()
                {
                    field.instruction.push_str(&text.decode().map_err(|error| {
                        Error::Other(format!("invalid table of contents instruction: {error}"))
                    })?);
                }
            }
            Event::Comment(_) | Event::PI(_) => {
                if let Some(paragraph) = elements.last().and_then(|element| {
                    element
                        .is_typed_paragraph
                        .then_some(element.paragraph)
                        .flatten()
                }) && let Some(raw_before) = paragraph_raw_before.get_mut(paragraph)
                {
                    *raw_before += 1;
                }
            }
            Event::End(element) => {
                let Some(closed) = elements.pop() else {
                    return Err(Error::Other(
                        "table of contents XML has an unmatched end element".to_owned(),
                    ));
                };
                if generated != DynamicOwnerPolicy::Toc
                    && let Some(span) =
                        generated_simple_span(xml, &closed, &elements, before, after, generated)?
                {
                    spans.push(span);
                }
                if closed
                    .hyperlink_plan
                    .as_ref()
                    .is_some_and(|plan| plan.preserved_raw)
                    && let Some(paragraph) = closed.paragraph
                    && let Some(raw_before) = paragraph_raw_before.get_mut(paragraph)
                {
                    *raw_before += 1;
                }
                if word && matches_local_name(element.name().as_ref(), b"instrText") {
                    instruction_depth = None;
                }
                if word && matches_local_name(element.name().as_ref(), b"r") {
                    for field in &mut fields {
                        if closed.is_typed_inline_owner && field.result_start.is_none() {
                            field.instruction_runs.push(DynamicInstructionRun {
                                start: closed.start,
                                end: after,
                                inherited_namespaces: closed.inherited_namespaces.clone(),
                            });
                        }
                        if field.separator_run_start == Some(closed.start)
                            && field.result_start.is_none()
                        {
                            field.result_start = Some(after);
                        }
                    }
                    if let Some(span) = spans
                        .iter_mut()
                        .rev()
                        .find(|span| span.result_end == closed.start && span.end_run_end == 0)
                    {
                        span.end_run_end = after;
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !elements.is_empty() {
        return Err(Error::Other(
            "table of contents XML has an unclosed element".to_owned(),
        ));
    }
    if fields.iter().any(|field| {
        generated_table_opcode(
            &Field::new(&field.instruction, "").instruction.name,
            generated,
        )
    }) {
        return Err(Error::Other(
            "table of contents field is missing its end marker".to_owned(),
        ));
    }
    spans.sort_by_key(|span| span.field_start);
    if spans
        .windows(2)
        .any(|pair| pair[1].field_start < pair[0].field_end)
    {
        return Err(Error::Other(
            "nested table of contents fields have ambiguous ownership".to_owned(),
        ));
    }
    let paragraphs = toc_paragraph_insertions(xml)?;
    for span in &mut spans {
        span.end_paragraph_content_start = paragraphs
            .get(span.end_paragraph)
            .map(|paragraph| paragraph.content_start)
            .ok_or_else(|| {
                Error::Other(
                    "table of contents end paragraph was not found in package XML".to_owned(),
                )
            })?;
    }
    Ok(spans)
}

fn collect_simple_toc_diagnostics(xml: &[u8]) -> Result<Vec<(usize, String)>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut elements = Vec::<DynamicXmlElement>::new();
    let mut simple_toc_starts = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid table of contents XML: {error}")))?;
        let word = namespace_is_word(&namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let local = local_name(element.name().as_ref()).to_vec();
                if let Some(start) = invalidate_overdeep_revision_owner(word, &local, &mut elements)
                {
                    simple_toc_starts.retain(|field_start| *field_start < start);
                }
                let (namespace_bindings, inherited_namespaces) =
                    dynamic_namespace_bindings(&element, &elements)?;
                let mut typed_block_owner = classify_typed_block_owner(word, &local, &elements);
                let is_typed_paragraph = typed_block_owner == Some(TypedBlockOwner::Paragraph);
                let paragraph = if is_typed_paragraph {
                    Some(0)
                } else {
                    elements.last().and_then(|element| element.paragraph)
                };
                let mut is_typed_inline_owner = typed_inline_owner(
                    &element,
                    reader.resolver(),
                    word,
                    &local,
                    &elements,
                    paragraph,
                )?;
                validate_dynamic_content_control(
                    xml,
                    before,
                    word,
                    &local,
                    &namespace_bindings,
                    &mut typed_block_owner,
                    &mut is_typed_inline_owner,
                )?;
                let revision_depth = if is_typed_inline_owner {
                    elements.last().map_or(0, |parent| parent.revision_depth)
                        + usize::from(matches!(local.as_slice(), b"ins" | b"moveTo"))
                } else {
                    0
                };
                if word
                    && local == b"fldSimple"
                    && paragraph.is_some()
                    && accepted_simple_toc_parent(&elements, paragraph)
                    && resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"instr",
                        AttributeNamespace::Word,
                    )?
                    .is_some_and(|(_, instruction)| {
                        Field::new(&instruction, "").instruction.name == "TOC"
                    })
                {
                    simple_toc_starts.push(before);
                }
                mark_typed_sdt_content(
                    &mut elements,
                    &local,
                    typed_block_owner,
                    is_typed_inline_owner,
                );
                elements.push(DynamicXmlElement {
                    local_name: local,
                    qualified_name: String::from_utf8_lossy(element.name().as_ref()).into_owned(),
                    typed_block_owner,
                    is_word: word,
                    is_typed_paragraph,
                    is_typed_inline_owner,
                    revision_depth,
                    sdt_content_seen: false,
                    namespace_bindings,
                    inherited_namespaces,
                    run_position: None,
                    hyperlink_plan: None,
                    start: before,
                    start_tag_end: after,
                    paragraph,
                });
            }
            Event::Empty(element) => {
                let local = local_name(element.name().as_ref()).to_vec();
                // Empty content is a retained raw slot, not the typed Start slot.
                let word = word && &local[..] != b"sdtContent";
                if let Some(start) = invalidate_overdeep_revision_owner(word, &local, &mut elements)
                {
                    simple_toc_starts.retain(|field_start| *field_start < start);
                }
                let paragraph = elements.last().and_then(|element| element.paragraph);
                let typed_block_owner = classify_typed_block_owner(word, &local, &elements);
                let is_typed_inline_owner = typed_inline_owner(
                    &element,
                    reader.resolver(),
                    word,
                    &local,
                    &elements,
                    paragraph,
                )?;
                mark_typed_sdt_content(
                    &mut elements,
                    &local,
                    typed_block_owner,
                    is_typed_inline_owner,
                );
                if word
                    && matches_local_name(element.name().as_ref(), b"fldSimple")
                    && elements
                        .last()
                        .and_then(|element| element.paragraph)
                        .is_some()
                    && accepted_simple_toc_parent(
                        &elements,
                        elements.last().and_then(|element| element.paragraph),
                    )
                    && resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"instr",
                        AttributeNamespace::Word,
                    )?
                    .is_some_and(|(_, instruction)| {
                        Field::new(&instruction, "").instruction.name == "TOC"
                    })
                {
                    simple_toc_starts.push(before);
                }
            }
            Event::End(_) => {
                elements.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(simple_toc_starts
        .into_iter()
        .map(|offset| {
            (
                offset,
                "simple table of contents fields are not rebuilt, stored display retained"
                    .to_owned(),
            )
        })
        .collect())
}

fn classify_typed_block_owner(
    word: bool,
    local: &[u8],
    elements: &[DynamicXmlElement],
) -> Option<TypedBlockOwner> {
    if !word {
        return None;
    }
    let parent = elements
        .last()
        .and_then(|element| element.typed_block_owner);
    let content_owner = match parent {
        Some(TypedBlockOwner::Body | TypedBlockOwner::Content(LegacyBlockOwner::Root)) => {
            Some(LegacyBlockOwner::Root)
        }
        Some(TypedBlockOwner::Table | TypedBlockOwner::Content(LegacyBlockOwner::Table)) => {
            Some(LegacyBlockOwner::Table)
        }
        Some(TypedBlockOwner::Row | TypedBlockOwner::Content(LegacyBlockOwner::Row)) => {
            Some(LegacyBlockOwner::Row)
        }
        Some(TypedBlockOwner::Cell | TypedBlockOwner::Content(LegacyBlockOwner::Cell)) => {
            Some(LegacyBlockOwner::Cell)
        }
        _ => None,
    };
    match local {
        b"document" if elements.is_empty() => Some(TypedBlockOwner::Document),
        b"body" if parent == Some(TypedBlockOwner::Document) => Some(TypedBlockOwner::Body),
        b"p" if matches!(
            content_owner,
            Some(LegacyBlockOwner::Root | LegacyBlockOwner::Cell)
        ) =>
        {
            Some(TypedBlockOwner::Paragraph)
        }
        b"tbl"
            if matches!(
                content_owner,
                Some(LegacyBlockOwner::Root | LegacyBlockOwner::Cell)
            ) =>
        {
            Some(TypedBlockOwner::Table)
        }
        b"tr" if content_owner == Some(LegacyBlockOwner::Table) => Some(TypedBlockOwner::Row),
        b"tc" if content_owner == Some(LegacyBlockOwner::Row) => Some(TypedBlockOwner::Cell),
        b"sdt" if content_owner.is_some() => {
            Some(TypedBlockOwner::ContentControl(content_owner.unwrap()))
        }
        b"sdtContent"
            if matches!(parent, Some(TypedBlockOwner::ContentControl(_)))
                && elements
                    .last()
                    .is_some_and(|control| !control.sdt_content_seen) =>
        {
            let Some(TypedBlockOwner::ContentControl(owner)) = parent else {
                unreachable!();
            };
            Some(TypedBlockOwner::Content(owner))
        }
        _ => None,
    }
}

fn direct_word_run_parent(elements: &[DynamicXmlElement], paragraph: Option<usize>) -> bool {
    paragraph.is_some()
        && elements.last().is_some_and(|parent| {
            parent.is_typed_inline_owner
                && parent.local_name == b"r"
                && parent.paragraph == paragraph
        })
}

fn dynamic_toc_run_position(
    word: bool,
    local: &[u8],
    is_typed_inline_owner: bool,
    modeled_simple_field: bool,
    elements: &mut [DynamicXmlElement],
    paragraph: Option<usize>,
    next_boundaries: &mut [usize],
    nested_run_orders: &mut [HashMap<(usize, TocRawOrder), usize>],
    paragraph_raw_before: &mut [usize],
) -> Option<TocRunPosition> {
    let paragraph = paragraph?;
    let next = next_boundaries.get_mut(paragraph)?;
    let inherited = elements.last().and_then(|parent| parent.run_position);
    if word && local == b"r" && is_typed_inline_owner {
        let nested = elements.iter().rev().take_while(|element| {
            !element.is_typed_paragraph || element.paragraph != Some(paragraph)
        });
        if nested.into_iter().any(|element| {
            matches!(
                element.local_name.as_slice(),
                b"sdt" | b"sdtContent" | b"ins" | b"moveTo"
            )
        }) {
            let outer = inherited.unwrap_or(TocRunPosition {
                run_boundary: *next,
                raw_order: TocRawOrder::Raw(0),
                nested_order: 0,
            });
            return nested_leaf_position(paragraph, outer, nested_run_orders);
        }
        let boundary = *next;
        let position = nested_leaf_position(
            paragraph,
            TocRunPosition {
                run_boundary: boundary,
                raw_order: TocRawOrder::AfterRaw,
                nested_order: 0,
            },
            nested_run_orders,
        );
        *next += 1;
        *paragraph_raw_before.get_mut(paragraph)? = 0;
        return position;
    }
    if word
        && local == b"fldSimple"
        && modeled_simple_field
        && accepted_simple_field_parent(elements, paragraph)
    {
        if elements.last().is_some_and(|parent| {
            parent.is_typed_inline_owner
                && matches!(parent.local_name.as_slice(), b"ins" | b"moveTo")
        }) {
            let outer = inherited.unwrap_or(TocRunPosition {
                run_boundary: *next,
                raw_order: TocRawOrder::Raw(0),
                nested_order: 0,
            });
            return nested_leaf_position(paragraph, outer, nested_run_orders);
        }
        let boundary = *next;
        let position = nested_leaf_position(
            paragraph,
            TocRunPosition {
                run_boundary: boundary,
                raw_order: TocRawOrder::AfterRaw,
                nested_order: 0,
            },
            nested_run_orders,
        );
        *next += 1;
        *paragraph_raw_before.get_mut(paragraph)? = 0;
        return position;
    }
    if is_typed_inline_owner && local == b"hyperlink" {
        return inherited;
    }
    if is_typed_inline_owner && matches!(local, b"sdt" | b"sdtContent" | b"ins" | b"moveTo") {
        if let Some(inherited) = inherited {
            return Some(inherited);
        }
        let raw_order = if elements
            .last()
            .is_some_and(|parent| parent.is_typed_inline_owner && parent.local_name == b"hyperlink")
        {
            let parent = elements.last_mut()?;
            let plan = parent.hyperlink_plan.as_mut()?;
            let order = *plan.revision_orders.get(plan.next_revision)?;
            plan.next_revision += 1;
            order
        } else {
            TocRawOrder::Raw(*paragraph_raw_before.get(paragraph)?)
        };
        return Some(TocRunPosition {
            run_boundary: *next,
            raw_order,
            nested_order: 0,
        });
    }
    None
}

fn advance_direct_paragraph_raw_child(
    word: bool,
    local: &[u8],
    is_typed_inline_owner: bool,
    modeled_simple_field: bool,
    direct_paragraph_child: bool,
    paragraph: Option<usize>,
    hyperlink_plan: Option<&DynamicHyperlinkPlan>,
    paragraph_raw_before: &mut [usize],
) {
    if !direct_paragraph_child {
        return;
    }
    let modeled_without_raw = word
        && (local == b"pPr"
            || local == b"r" && is_typed_inline_owner
            || local == b"fldSimple" && modeled_simple_field
            || local == b"sdt" && is_typed_inline_owner
            || local == b"hyperlink");
    if modeled_without_raw
        || hyperlink_plan.is_some_and(|plan| plan.preserved_raw)
        || local == b"hyperlink"
    {
        return;
    }
    if let Some(raw_before) = paragraph.and_then(|index| paragraph_raw_before.get_mut(index)) {
        *raw_before += 1;
    }
}

fn dynamic_hyperlink_plan(
    xml: &[u8],
    start: usize,
    bindings: &BTreeMap<String, String>,
    raw_before: usize,
) -> Result<DynamicHyperlinkPlan> {
    let raw = dynamic_element_slice(xml, start)?;
    let mut scoped = Vec::new();
    scoped.extend_from_slice(b"<rdocx-scope");
    for (prefix, namespace) in bindings {
        if prefix == "xml" {
            continue;
        }
        if prefix.is_empty() {
            scoped.extend_from_slice(b" xmlns=\"");
        } else {
            scoped.extend_from_slice(b" xmlns:");
            scoped.extend_from_slice(prefix.as_bytes());
            scoped.extend_from_slice(b"=\"");
        }
        scoped.extend_from_slice(xml_escape_attribute(namespace).as_bytes());
        scoped.push(b'"');
    }
    scoped.push(b'>');
    scoped.extend_from_slice(raw);
    scoped.extend_from_slice(b"</rdocx-scope>");

    let mut reader = NsReader::from_reader(scoped.as_slice());
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut direct_runs = 0usize;
    let mut revision_boundaries = Vec::new();
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid hyperlink XML: {error}")))?;
        let word = namespace_is_word(&namespace);
        match event {
            Event::Start(element) => {
                if depth == 2 && word && local_name(element.name().as_ref()) == b"r" {
                    direct_runs += 1;
                } else if depth == 2
                    && word
                    && matches!(local_name(element.name().as_ref()), b"ins" | b"moveTo")
                {
                    let valid_id = resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"id",
                        AttributeNamespace::Word,
                    )?
                    .is_some_and(|(_, id)| id.parse::<i32>().is_ok());
                    let has_author = resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"author",
                        AttributeNamespace::Word,
                    )?
                    .is_some();
                    if valid_id && has_author {
                        revision_boundaries.push(direct_runs);
                    }
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            Event::Eof => {
                return Err(Error::Other(
                    "table of contents hyperlink is not balanced".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
    let preserved_raw = direct_runs == 0;
    let revision_orders = revision_boundaries
        .into_iter()
        .map(|boundary| {
            if preserved_raw {
                TocRawOrder::Raw(raw_before)
            } else if boundary == direct_runs {
                TocRawOrder::BeforeRaw
            } else {
                TocRawOrder::AfterRaw
            }
        })
        .collect();
    Ok(DynamicHyperlinkPlan {
        revision_orders,
        next_revision: 0,
        preserved_raw,
    })
}

fn dynamic_element_slice(xml: &[u8], start: usize) -> Result<&[u8]> {
    let tail = xml.get(start..).ok_or_else(|| {
        Error::Other("table of contents element offset is outside the document".to_owned())
    })?;
    let mut reader = quick_xml::Reader::from_reader(tail);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid table of contents XML: {error}")))?
        {
            Event::Start(_) => depth += 1,
            Event::Empty(_) if depth == 0 => {
                return Ok(&tail[..reader.buffer_position() as usize]);
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok(&tail[..reader.buffer_position() as usize]);
                }
            }
            Event::Eof => {
                return Err(Error::Other(
                    "table of contents element is not balanced".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn nested_leaf_position(
    paragraph: usize,
    outer: TocRunPosition,
    nested_run_orders: &mut [HashMap<(usize, TocRawOrder), usize>],
) -> Option<TocRunPosition> {
    let next = nested_run_orders
        .get_mut(paragraph)?
        .entry((outer.run_boundary, outer.raw_order))
        .or_default();
    let nested_order = *next;
    *next += 1;
    Some(TocRunPosition {
        nested_order,
        ..outer
    })
}

fn accepted_simple_field_parent(elements: &[DynamicXmlElement], paragraph: usize) -> bool {
    direct_typed_paragraph_parent(elements, Some(paragraph))
        || elements.last().is_some_and(|parent| {
            parent.is_typed_inline_owner
                && matches!(parent.local_name.as_slice(), b"ins" | b"moveTo")
                && parent.paragraph == Some(paragraph)
        })
}

fn typed_inline_owner(
    element: &BytesStart<'_>,
    resolver: &NamespaceResolver,
    word: bool,
    local: &[u8],
    elements: &[DynamicXmlElement],
    paragraph: Option<usize>,
) -> Result<bool> {
    if !word || paragraph.is_none() {
        return Ok(false);
    }
    let Some(parent) = elements.last() else {
        return Ok(false);
    };
    let parent_is_paragraph = parent.is_typed_paragraph && parent.paragraph == paragraph;
    let parent_is_inline = parent.is_typed_inline_owner && parent.paragraph == paragraph;
    let parent_local = parent.local_name.as_slice();
    let valid = match local {
        b"r" => {
            parent_is_paragraph
                || (parent_is_inline
                    && matches!(
                        parent_local,
                        b"hyperlink" | b"sdtContent" | b"ins" | b"moveTo"
                    ))
        }
        b"hyperlink" => {
            parent_is_paragraph || (parent_is_inline && matches!(parent_local, b"ins" | b"moveTo"))
        }
        b"sdt" => {
            parent_is_paragraph
                || (parent_is_inline && matches!(parent_local, b"sdtContent" | b"ins" | b"moveTo"))
        }
        b"sdtContent" => parent_is_inline && parent_local == b"sdt" && !parent.sdt_content_seen,
        b"ins" | b"moveTo" => {
            let valid_parent = parent_is_paragraph
                || (parent_is_inline
                    && matches!(
                        parent_local,
                        b"hyperlink" | b"sdtContent" | b"ins" | b"moveTo"
                    ));
            let valid_id =
                resolved_element_attribute(element, resolver, b"id", AttributeNamespace::Word)?
                    .is_some_and(|(_, id)| id.parse::<i32>().is_ok());
            let has_author =
                resolved_element_attribute(element, resolver, b"author", AttributeNamespace::Word)?
                    .is_some();
            valid_parent
                && parent.revision_depth < MAX_DYNAMIC_REVISION_NESTING_DEPTH
                && valid_id
                && has_author
        }
        _ => false,
    };
    Ok(valid)
}

fn direct_typed_paragraph_parent(elements: &[DynamicXmlElement], paragraph: Option<usize>) -> bool {
    let Some(paragraph) = paragraph else {
        return false;
    };
    elements
        .last()
        .is_some_and(|parent| parent.is_typed_paragraph && parent.paragraph == Some(paragraph))
}

fn accepted_bookmark_parent(elements: &[DynamicXmlElement], paragraph: Option<usize>) -> bool {
    direct_typed_paragraph_parent(elements, paragraph)
        || paragraph.is_some()
            && elements.last().is_some_and(|parent| {
                parent.is_typed_inline_owner
                    && matches!(
                        parent.local_name.as_slice(),
                        b"hyperlink" | b"sdtContent" | b"ins" | b"moveTo"
                    )
                    && parent.paragraph == paragraph
            })
}

fn accepted_simple_toc_parent(elements: &[DynamicXmlElement], paragraph: Option<usize>) -> bool {
    direct_typed_paragraph_parent(elements, paragraph)
        || paragraph.is_some()
            && elements.last().is_some_and(|parent| {
                parent.is_typed_inline_owner
                    && matches!(parent.local_name.as_slice(), b"ins" | b"moveTo")
                    && parent.paragraph == paragraph
            })
}

fn update_dynamic_field_stack(
    element: &BytesStart<'_>,
    resolver: &NamespaceResolver,
    event_start: usize,
    paragraph: Option<usize>,
    elements: &[DynamicXmlElement],
    fields: &mut Vec<DynamicFieldScan>,
    spans: &mut Vec<DynamicTocSpan>,
    generated: DynamicOwnerPolicy,
) -> Result<()> {
    let Some(paragraph) = paragraph else {
        return Ok(());
    };
    let kind =
        resolved_element_attribute(element, resolver, b"fldCharType", AttributeNamespace::Word)?
            .map(|(_, value)| value);
    match kind.as_deref() {
        Some("begin") => fields.push(DynamicFieldScan {
            instruction: String::new(),
            field_start: event_start,
            begin_paragraph: paragraph,
            begin_paragraph_start: elements
                .iter()
                .rev()
                .find(|element| element.is_typed_paragraph && element.paragraph == Some(paragraph))
                .map(|element| element.start)
                .ok_or_else(|| {
                    Error::Other(
                        "table of contents begin marker is not contained by a paragraph".to_owned(),
                    )
                })?,
            begin_run_start: elements
                .iter()
                .rev()
                .find(|element| {
                    element.is_word
                        && element.local_name == b"r"
                        && element.paragraph == Some(paragraph)
                })
                .map(|element| element.start)
                .ok_or_else(|| {
                    Error::Other(
                        "table of contents begin marker is not contained by a run".to_owned(),
                    )
                })?,
            separator_paragraph: None,
            separator_run_start: None,
            result_start: None,
            result_start_position: None,
            start_paragraph_name: None,
            start_paragraph_namespaces: BTreeMap::new(),
            separator_wrapper_names: Vec::new(),
            instruction_runs: Vec::new(),
        }),
        Some("separate") => {
            let Some(field) = fields.last_mut() else {
                return Ok(());
            };
            if field.separator_paragraph.is_some() {
                return Err(Error::Other(
                    "table of contents field has more than one separator".to_owned(),
                ));
            }
            let run = elements
                .iter()
                .rev()
                .find(|element| {
                    element.is_word
                        && element.local_name == b"r"
                        && element.paragraph == Some(paragraph)
                })
                .ok_or_else(|| {
                    Error::Other("table of contents separator is not contained by a run".to_owned())
                })?;
            let para = elements
                .iter()
                .rev()
                .find(|element| element.is_typed_paragraph && element.paragraph == Some(paragraph))
                .ok_or_else(|| {
                    Error::Other(
                        "table of contents separator is not contained by a paragraph".to_owned(),
                    )
                })?;
            field.separator_paragraph = Some(paragraph);
            field.separator_run_start = Some(run.start);
            field.result_start_position = run.run_position.map(|position| {
                if position.raw_order == TocRawOrder::AfterRaw {
                    TocRunPosition {
                        run_boundary: position.run_boundary + 1,
                        raw_order: TocRawOrder::BeforeRaw,
                        nested_order: 0,
                    }
                } else {
                    TocRunPosition {
                        nested_order: position.nested_order + 1,
                        ..position
                    }
                }
            });
            field.start_paragraph_name = Some(para.qualified_name.clone());
            field.start_paragraph_namespaces = para.inherited_namespaces.clone();
            let paragraph_position = elements
                .iter()
                .position(|element| std::ptr::eq(element, para))
                .expect("paragraph was borrowed from the element stack");
            let run_position = elements
                .iter()
                .rposition(|element| std::ptr::eq(element, run))
                .expect("run was borrowed from the element stack");
            field.separator_wrapper_names = elements[paragraph_position + 1..run_position]
                .iter()
                .filter(|element| element.is_typed_inline_owner)
                .map(|element| element.qualified_name.clone())
                .collect();
        }
        Some("end") => {
            let Some(field) = fields.pop() else {
                return Ok(());
            };
            if !generated_table_opcode(
                &Field::new(&field.instruction, "").instruction.name,
                generated,
            ) {
                return Ok(());
            }
            let result_start = field.result_start.ok_or_else(|| {
                Error::Other("table of contents field is missing its separator".to_owned())
            })?;
            let separator_paragraph = field.separator_paragraph.ok_or_else(|| {
                Error::Other("table of contents field is missing its separator".to_owned())
            })?;
            if separator_paragraph != field.begin_paragraph {
                return Err(Error::Other(
                    "table of contents instruction crosses a paragraph boundary".to_owned(),
                ));
            }
            let end_run = elements
                .iter()
                .rev()
                .find(|element| {
                    element.is_word
                        && element.local_name == b"r"
                        && element.paragraph == Some(paragraph)
                })
                .ok_or_else(|| {
                    Error::Other(
                        "table of contents end marker is not contained by a run".to_owned(),
                    )
                })?;
            let end_para = elements
                .iter()
                .rev()
                .find(|element| element.is_typed_paragraph && element.paragraph == Some(paragraph))
                .ok_or_else(|| {
                    Error::Other(
                        "table of contents end marker is not contained by a paragraph".to_owned(),
                    )
                })?;
            if event_start < result_start || end_run.start < result_start {
                return Err(Error::Other(
                    "table of contents result boundaries are reversed".to_owned(),
                ));
            }
            let paragraph_position = elements
                .iter()
                .position(|element| std::ptr::eq(element, end_para))
                .expect("end paragraph was borrowed from the element stack");
            let run_position = elements
                .iter()
                .rposition(|element| std::ptr::eq(element, end_run))
                .expect("end run was borrowed from the element stack");
            let wrapper_chain = &elements[paragraph_position + 1..run_position];
            let mut end_wrapper_prefixes = Vec::new();
            let mut wrapper_index = 0usize;
            while wrapper_index < wrapper_chain.len() {
                let wrapper = &wrapper_chain[wrapper_index];
                if !wrapper.is_typed_inline_owner {
                    wrapper_index += 1;
                    continue;
                }
                if wrapper.local_name == b"sdt"
                    && let Some(content) = wrapper_chain.get(wrapper_index + 1)
                    && content.is_typed_inline_owner
                    && content.local_name == b"sdtContent"
                {
                    end_wrapper_prefixes.push((wrapper.start, content.start_tag_end));
                    wrapper_index += 2;
                } else {
                    end_wrapper_prefixes.push((wrapper.start, wrapper.start_tag_end));
                    wrapper_index += 1;
                }
            }
            spans.push(DynamicTocSpan {
                simple_field: None,
                instruction: field.instruction,
                field_start: field.field_start,
                field_end: event_start,
                begin_paragraph: field.begin_paragraph,
                end_paragraph: paragraph,
                begin_run_start: field.begin_run_start,
                instruction_paragraph_start: field.begin_paragraph_start,
                result_start,
                result_end: end_run.start,
                result_start_position: field.result_start_position.ok_or_else(|| {
                    Error::Other(
                        "table of contents separator run position was not retained".to_owned(),
                    )
                })?,
                result_end_position: end_run.run_position.ok_or_else(|| {
                    Error::Other("table of contents end run position was not retained".to_owned())
                })?,
                end_run_end: 0,
                start_paragraph_name: field.start_paragraph_name.ok_or_else(|| {
                    Error::Other("table of contents field is missing its separator".to_owned())
                })?,
                start_paragraph_namespaces: field.start_paragraph_namespaces,
                separator_wrapper_names: field.separator_wrapper_names,
                instruction_runs: field.instruction_runs,
                end_paragraph_start: end_para.start,
                end_paragraph_content_start: 0,
                end_wrapper_prefixes,
            });
        }
        _ => {}
    }
    Ok(())
}

type ParsedDynamicTocFields = (Vec<Option<TocField>>, Vec<(usize, String)>);

fn parse_dynamic_toc_fields(
    document: &Document,
    xml: &[u8],
    spans: &[DynamicTocSpan],
) -> Result<ParsedDynamicTocFields> {
    let mut projected = document.clone_for_staging();
    prepare_physical_story_projection(&mut projected.document.body, &mut [])?;
    let document = &projected;
    let mut paragraphs = Vec::new();
    collect_body_paragraphs(&document.document.body, &mut paragraphs, None);
    let context = FieldEvaluationContext::default();
    let mut output = Vec::with_capacity(spans.len());
    let mut diagnostics = Vec::new();
    for span in spans {
        let field = parse_dynamic_toc_field(xml, span)?;
        let mut evaluator = Evaluator::new(document, &context);
        evaluator.ensure_numbering_layout_for_field(&field)?;
        match evaluator.evaluate_field(&field, "main", &paragraphs, span.begin_paragraph, None) {
            FieldOutcome::TableOfContents(toc) => output.push(Some(toc)),
            FieldOutcome::KeepStored { diagnostic } => {
                diagnostics.push((span.field_start, diagnostic));
                output.push(None);
            }
            _ => {
                return Err(Error::Other(
                    "table of contents instruction was not recognized".to_owned(),
                ));
            }
        }
    }
    Ok((output, diagnostics))
}

fn parse_dynamic_toc_field(xml: &[u8], span: &DynamicTocSpan) -> Result<Field> {
    if let Some(field) = &span.simple_field {
        return Ok(field.clone());
    }
    let prefix = span
        .start_paragraph_name
        .split_once(':')
        .map_or("w", |(prefix, _)| prefix);
    // The instruction paragraph is cut out of its part, so its start tag gets
    // the declarations it inherits there. Every run in it then resolves the
    // prefixes it resolved when the document was read.
    let mut source = Vec::new();
    append_with_inherited_namespaces(
        &mut source,
        &xml[span.instruction_paragraph_start..span.result_start],
        &span.start_paragraph_namespaces,
    )?;
    source.extend_from_slice(
        format!("<{prefix}:r><{prefix}:fldChar {prefix}:fldCharType=\"end\"/></{prefix}:r>")
            .as_bytes(),
    );
    append_toc_wrapper_closures(&mut source, &span.separator_wrapper_names);
    source.extend_from_slice(b"</");
    source.extend_from_slice(span.start_paragraph_name.as_bytes());
    source.push(b'>');
    let parse_paragraph = |source: &[u8]| -> Result<CT_P> {
        let mut reader = quick_xml::Reader::from_reader(source);
        reader.config_mut().trim_text(false);
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer).map_err(|error| {
                Error::Other(format!("invalid table of contents field: {error}"))
            })? {
                Event::Start(element) if matches_local_name(element.name().as_ref(), b"p") => {
                    return Ok(CT_P::from_xml(&mut reader)?);
                }
                Event::Eof => {
                    return Err(Error::Other(
                        "table of contents instruction paragraph was not found".to_owned(),
                    ));
                }
                _ => {}
            }
            buffer.clear();
        }
    };
    CT_P::from_xml_fragment(&source)?;

    let mut projected = format!("<w:p xmlns:w=\"{W_NS}\">").into_bytes();
    for run in &span.instruction_runs {
        append_with_inherited_namespaces(
            &mut projected,
            &xml[run.start..run.end],
            &run.inherited_namespaces,
        )?;
    }
    projected.extend_from_slice(b"<w:r><w:fldChar w:fldCharType=\"end\"/></w:r></w:p>");
    let paragraph = parse_paragraph(&projected)?;
    accepted_toc_runs(&paragraph)
        .into_iter()
        .flat_map(|run| &run.run.content)
        .find_map(|content| match content {
            RunContent::Field(field)
                if field.effective_instruction().name
                    == Field::new(&span.instruction, "").instruction.name =>
            {
                Some(field.clone())
            }
            _ => None,
        })
        .ok_or_else(|| {
            Error::Other(format!(
                "table of contents instruction could not be parsed: {}",
                span.instruction.trim()
            ))
        })
}

fn append_with_inherited_namespaces(
    output: &mut Vec<u8>,
    raw: &[u8],
    inherited_namespaces: &BTreeMap<String, String>,
) -> Result<()> {
    let mut reader = quick_xml::Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let (insertion, local_namespaces) =
        match reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!(
                "invalid table of contents instruction XML: {error}"
            ))
        })? {
            Event::Start(start) | Event::Empty(start) => {
                let mut local_namespaces = HashSet::new();
                for attribute in start.attributes() {
                    let attribute = attribute.map_err(|error| {
                        Error::Other(format!(
                            "invalid table of contents instruction XML: {error}"
                        ))
                    })?;
                    let key = attribute.key.as_ref();
                    if key == b"xmlns" {
                        local_namespaces.insert(String::new());
                    } else if let Some(prefix) = key.strip_prefix(b"xmlns:") {
                        local_namespaces.insert(String::from_utf8_lossy(prefix).into_owned());
                    }
                }
                let tag_end = reader.buffer_position() as usize;
                let insertion = if tag_end >= 2 && raw[tag_end - 2] == b'/' {
                    tag_end - 2
                } else {
                    tag_end - 1
                };
                (insertion, local_namespaces)
            }
            _ => {
                return Err(Error::Other(
                    "table of contents instruction XML has no start tag".to_owned(),
                ));
            }
        };
    output.extend_from_slice(&raw[..insertion]);
    for (prefix, namespace) in inherited_namespaces {
        if prefix == "xml" || local_namespaces.contains(prefix) {
            continue;
        }
        if prefix.is_empty() {
            output.extend_from_slice(b" xmlns=\"");
        } else {
            output.extend_from_slice(b" xmlns:");
            output.extend_from_slice(prefix.as_bytes());
            output.extend_from_slice(b"=\"");
        }
        output.extend_from_slice(xml_escape_attribute(namespace).as_bytes());
        output.push(b'"');
    }
    output.extend_from_slice(&raw[insertion..]);
    Ok(())
}

#[derive(Debug, Clone)]
struct TocBookmark {
    id: i32,
    name: String,
    insert: bool,
    authored: bool,
}

#[derive(Debug)]
struct TocBookmarkState {
    max_id: i32,
    names: HashSet<String>,
    named_ranges: HashMap<String, (TocDocumentPosition, TocDocumentPosition)>,
    whole_paragraphs: HashMap<usize, (i32, String)>,
    ranges: Vec<TocBookmarkRange>,
}

#[derive(Debug, Clone)]
struct TocBookmarkRange {
    id: i32,
    name: String,
    start_offset: usize,
    end_offset: usize,
}

#[derive(Debug)]
struct TocBookmarkRepair {
    id: i32,
    name: String,
    toc_index: usize,
    replace_start: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum TocRawOrder {
    BeforeRaw,
    Raw(usize),
    AfterRaw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct TocRunPosition {
    run_boundary: usize,
    raw_order: TocRawOrder,
    nested_order: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct TocDocumentPosition {
    paragraph: usize,
    accepted_run_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct TocOwnedPosition {
    paragraph: usize,
    run: TocRunPosition,
}

fn inspect_toc_bookmarks(body: &CT_Body, xml: &[u8]) -> Result<TocBookmarkState> {
    let whole_bookmarks = whole_paragraph_bookmark_ids(xml)?;
    let marker_offsets = toc_bookmark_marker_offsets(xml)?;
    let mut paragraphs = Vec::new();
    collect_body_paragraphs(body, &mut paragraphs, None);
    let mut starts = HashMap::<i32, (String, usize, usize)>::new();
    let mut start_order = Vec::new();
    let mut ends = HashMap::<i32, (usize, usize)>::new();
    let mut names = HashSet::new();
    let mut max_id = 0i32;
    for (paragraph_index, paragraph) in paragraphs.iter().enumerate() {
        for marker in paragraph.current_bookmark_projection() {
            let Some(id) = marker.id() else {
                return Err(Error::Other(
                    "table of contents source has a bookmark without an ID".to_owned(),
                ));
            };
            max_id = max_id.max(id);
            if marker.is_start() {
                let name = marker.name().ok_or_else(|| {
                    Error::Other(
                        "table of contents source has a bookmark without a name".to_owned(),
                    )
                })?;
                if !names.insert(name.to_owned()) || starts.contains_key(&id) {
                    return Err(Error::Other(
                        "table of contents source has ambiguous duplicate bookmarks".to_owned(),
                    ));
                }
                starts.insert(
                    id,
                    (
                        name.to_owned(),
                        paragraph_index,
                        marker.projected_run_index(),
                    ),
                );
                start_order.push(id);
            } else if ends
                .insert(id, (paragraph_index, marker.projected_run_index()))
                .is_some()
            {
                return Err(Error::Other(
                    "table of contents source has ambiguous duplicate bookmarks".to_owned(),
                ));
            }
        }
    }
    if starts.len() != ends.len() || starts.keys().any(|id| !ends.contains_key(id)) {
        return Err(Error::Other(
            "table of contents source has an unmatched bookmark".to_owned(),
        ));
    }
    let mut named_ranges = HashMap::new();
    let mut whole_paragraphs = HashMap::new();
    let mut ranges = Vec::new();
    for id in start_order {
        let (name, start_paragraph, start_run) = starts
            .remove(&id)
            .expect("bookmark start order contains each validated start once");
        let (end_paragraph, end_run) = ends[&id];
        let start_offset = marker_offsets.get(&(id, true)).copied().ok_or_else(|| {
            Error::Other("table of contents bookmark start offset was not retained".to_owned())
        })?;
        let end_offset = marker_offsets.get(&(id, false)).copied().ok_or_else(|| {
            Error::Other("table of contents bookmark end offset was not retained".to_owned())
        })?;
        if start_offset > end_offset {
            return Err(Error::Other(
                "table of contents source has a reversed bookmark".to_owned(),
            ));
        }
        named_ranges.insert(
            name.clone(),
            (
                TocDocumentPosition {
                    paragraph: start_paragraph,
                    accepted_run_index: start_run,
                },
                TocDocumentPosition {
                    paragraph: end_paragraph,
                    accepted_run_index: end_run,
                },
            ),
        );
        ranges.push(TocBookmarkRange {
            id,
            name: name.clone(),
            start_offset,
            end_offset,
        });
        if start_paragraph == end_paragraph && whole_bookmarks.contains(&id) {
            whole_paragraphs
                .entry(start_paragraph)
                .or_insert((id, name));
        }
    }
    Ok(TocBookmarkState {
        max_id,
        names,
        named_ranges,
        whole_paragraphs,
        ranges,
    })
}

fn toc_crossing_bookmark_repairs(
    state: &TocBookmarkState,
    spans: &[DynamicTocSpan],
    fields: &[Option<TocField>],
) -> Vec<TocBookmarkRepair> {
    state
        .ranges
        .iter()
        .filter_map(|range| {
            let containing_span = |offset: usize| {
                spans
                    .iter()
                    .zip(fields)
                    .enumerate()
                    .find(|(_, (span, field))| {
                        field.is_some() && offset >= span.result_start && offset < span.result_end
                    })
                    .map(|(index, _)| index)
            };
            let start_span = containing_span(range.start_offset);
            let end_span = containing_span(range.end_offset);
            match (start_span, end_span) {
                (Some(toc_index), None) => Some(TocBookmarkRepair {
                    id: range.id,
                    name: range.name.clone(),
                    toc_index,
                    replace_start: true,
                }),
                (None, Some(toc_index)) => Some(TocBookmarkRepair {
                    id: range.id,
                    name: range.name.clone(),
                    toc_index,
                    replace_start: false,
                }),
                _ => None,
            }
        })
        .collect()
}

#[derive(Clone, Copy)]
enum TocParagraphToken {
    Content,
    BookmarkStart(i32),
    BookmarkEnd(i32),
}

fn toc_bookmark_marker_offsets(xml: &[u8]) -> Result<HashMap<(i32, bool), usize>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut elements = Vec::<DynamicXmlElement>::new();
    let mut offsets = HashMap::new();
    let mut paragraph_count = 0usize;
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid table of contents XML: {error}")))?;
        let word = namespace_is_word(&namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let local = local_name(element.name().as_ref()).to_vec();
                invalidate_overdeep_revision_owner(word, &local, &mut elements);
                let (namespace_bindings, inherited_namespaces) =
                    dynamic_namespace_bindings(&element, &elements)?;
                let mut typed_block_owner = classify_typed_block_owner(word, &local, &elements);
                let is_typed_paragraph = typed_block_owner == Some(TypedBlockOwner::Paragraph);
                let paragraph = if is_typed_paragraph {
                    let paragraph = paragraph_count;
                    paragraph_count += 1;
                    Some(paragraph)
                } else {
                    elements.last().and_then(|element| element.paragraph)
                };
                let mut is_typed_inline_owner = typed_inline_owner(
                    &element,
                    reader.resolver(),
                    word,
                    &local,
                    &elements,
                    paragraph,
                )?;
                validate_dynamic_content_control(
                    xml,
                    before,
                    word,
                    &local,
                    &namespace_bindings,
                    &mut typed_block_owner,
                    &mut is_typed_inline_owner,
                )?;
                if word
                    && matches!(local.as_slice(), b"bookmarkStart" | b"bookmarkEnd")
                    && accepted_bookmark_parent(&elements, paragraph)
                    && let Some((_, id)) = resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"id",
                        AttributeNamespace::Word,
                    )?
                    && let Ok(id) = id.parse::<i32>()
                    && offsets
                        .insert((id, local == b"bookmarkStart"), before)
                        .is_some()
                {
                    return Err(Error::Other(
                        "table of contents source has ambiguous duplicate bookmarks".to_owned(),
                    ));
                }
                let revision_depth = if is_typed_inline_owner {
                    elements.last().map_or(0, |parent| {
                        parent.revision_depth
                            + usize::from(matches!(local.as_slice(), b"ins" | b"moveTo"))
                    })
                } else {
                    0
                };
                mark_typed_sdt_content(
                    &mut elements,
                    &local,
                    typed_block_owner,
                    is_typed_inline_owner,
                );
                elements.push(DynamicXmlElement {
                    local_name: local,
                    qualified_name: String::from_utf8_lossy(element.name().as_ref()).into_owned(),
                    typed_block_owner,
                    is_word: word,
                    is_typed_paragraph,
                    is_typed_inline_owner,
                    revision_depth,
                    sdt_content_seen: false,
                    namespace_bindings,
                    inherited_namespaces,
                    run_position: None,
                    hyperlink_plan: None,
                    start: before,
                    start_tag_end: after,
                    paragraph,
                });
            }
            Event::Empty(element) => {
                let local = local_name(element.name().as_ref()).to_vec();
                // Empty content is a retained raw slot, not the typed Start slot.
                let word = word && &local[..] != b"sdtContent";
                invalidate_overdeep_revision_owner(word, &local, &mut elements);
                let paragraph = elements.last().and_then(|element| element.paragraph);
                let typed_block_owner = classify_typed_block_owner(word, &local, &elements);
                let is_typed_inline_owner = typed_inline_owner(
                    &element,
                    reader.resolver(),
                    word,
                    &local,
                    &elements,
                    paragraph,
                )?;
                if word
                    && matches!(local.as_slice(), b"bookmarkStart" | b"bookmarkEnd")
                    && accepted_bookmark_parent(&elements, paragraph)
                    && let Some((_, id)) = resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"id",
                        AttributeNamespace::Word,
                    )?
                    && let Ok(id) = id.parse::<i32>()
                    && offsets
                        .insert((id, local == b"bookmarkStart"), before)
                        .is_some()
                {
                    return Err(Error::Other(
                        "table of contents source has ambiguous duplicate bookmarks".to_owned(),
                    ));
                }
                mark_typed_sdt_content(
                    &mut elements,
                    &local,
                    typed_block_owner,
                    is_typed_inline_owner,
                );
            }
            Event::End(_) => {
                elements.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(offsets)
}

fn whole_paragraph_bookmark_ids(xml: &[u8]) -> Result<HashSet<i32>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut elements = Vec::<DynamicXmlElement>::new();
    let mut paragraphs = Vec::<Vec<TocParagraphToken>>::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid table of contents XML: {error}")))?;
        let word = namespace_is_word(&namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let local = local_name(element.name().as_ref()).to_vec();
                let (namespace_bindings, inherited_namespaces) =
                    dynamic_namespace_bindings(&element, &elements)?;
                let mut typed_block_owner = classify_typed_block_owner(word, &local, &elements);
                let mut is_typed_inline_owner = false;
                validate_dynamic_content_control(
                    xml,
                    before,
                    word,
                    &local,
                    &namespace_bindings,
                    &mut typed_block_owner,
                    &mut is_typed_inline_owner,
                )?;
                let is_typed_paragraph = typed_block_owner == Some(TypedBlockOwner::Paragraph);
                let paragraph = if is_typed_paragraph {
                    paragraphs.push(Vec::new());
                    Some(paragraphs.len() - 1)
                } else {
                    elements.last().and_then(|element| element.paragraph)
                };
                if let Some(index) = paragraph
                    && elements
                        .last()
                        .is_some_and(|parent| parent.is_typed_paragraph)
                    && local != b"pPr"
                {
                    paragraphs[index].push(paragraph_token(
                        &element,
                        reader.resolver(),
                        word,
                        &local,
                    )?);
                }
                mark_typed_sdt_content(&mut elements, &local, typed_block_owner, false);
                elements.push(DynamicXmlElement {
                    local_name: local,
                    qualified_name: String::from_utf8_lossy(element.name().as_ref()).into_owned(),
                    typed_block_owner,
                    is_word: word,
                    is_typed_paragraph,
                    is_typed_inline_owner: false,
                    revision_depth: 0,
                    sdt_content_seen: false,
                    namespace_bindings,
                    inherited_namespaces,
                    run_position: None,
                    hyperlink_plan: None,
                    start: before,
                    start_tag_end: after,
                    paragraph,
                });
            }
            Event::Empty(element) => {
                let name = element.name();
                let local = local_name(name.as_ref());
                // Empty content is a retained raw slot, not the typed Start slot.
                let word = word && local != b"sdtContent";
                let typed_block_owner = classify_typed_block_owner(word, local, &elements);
                mark_typed_sdt_content(&mut elements, local, typed_block_owner, false);
                if let Some(parent) = elements.last()
                    && parent.is_typed_paragraph
                    && local != b"pPr"
                    && let Some(index) = parent.paragraph
                {
                    paragraphs[index].push(paragraph_token(
                        &element,
                        reader.resolver(),
                        word,
                        local,
                    )?);
                }
            }
            Event::End(_) => {
                elements.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    let mut whole = HashSet::new();
    for tokens in paragraphs {
        let content = tokens
            .iter()
            .enumerate()
            .filter_map(|(index, token)| {
                matches!(token, TocParagraphToken::Content).then_some(index)
            })
            .collect::<Vec<_>>();
        let mut starts = HashMap::new();
        let mut ends = HashMap::new();
        for (index, token) in tokens.iter().enumerate() {
            match token {
                TocParagraphToken::BookmarkStart(id) => {
                    starts.insert(*id, index);
                }
                TocParagraphToken::BookmarkEnd(id) => {
                    ends.insert(*id, index);
                }
                TocParagraphToken::Content => {}
            }
        }
        for (id, start) in starts {
            let Some(end) = ends.get(&id).copied() else {
                continue;
            };
            if content
                .iter()
                .all(|position| *position > start && *position < end)
            {
                whole.insert(id);
            }
        }
    }
    Ok(whole)
}

fn paragraph_token(
    element: &BytesStart<'_>,
    resolver: &NamespaceResolver,
    word: bool,
    local: &[u8],
) -> Result<TocParagraphToken> {
    if word && matches!(local, b"bookmarkStart" | b"bookmarkEnd") {
        let id = resolved_element_attribute(element, resolver, b"id", AttributeNamespace::Word)?
            .and_then(|(_, id)| id.parse::<i32>().ok());
        if let Some(id) = id {
            return Ok(if local == b"bookmarkStart" {
                TocParagraphToken::BookmarkStart(id)
            } else {
                TocParagraphToken::BookmarkEnd(id)
            });
        }
    }
    Ok(TocParagraphToken::Content)
}

struct TocBookmarkAllocator {
    next_id: Option<i32>,
    next_name: Option<u32>,
    names: HashSet<String>,
    whole_paragraphs: HashMap<usize, (i32, String)>,
}

impl TocBookmarkAllocator {
    fn new(state: TocBookmarkState, authored_names: &HashSet<String>) -> Self {
        let mut names = state.names;
        names.retain(|name| !authored_names.contains(name));
        Self {
            next_id: state.max_id.checked_add(1),
            next_name: Some(1),
            names,
            whole_paragraphs: state.whole_paragraphs,
        }
    }

    fn whole_paragraph_name(&self, paragraph: usize) -> Option<(i32, String)> {
        self.whole_paragraphs.get(&paragraph).cloned()
    }

    fn allocate(&mut self, identifiers: &mut DocumentIdentifiers) -> Result<(i32, String)> {
        let preferred = self.next_id.ok_or_else(|| {
            Error::Other("table of contents exhausted the bookmark ID range".to_owned())
        })?;
        let id = identifiers.reserve_preferred_bookmark_id(preferred)?;
        let name = self.allocate_name()?;
        self.next_id = id.checked_add(1);
        Ok((id, name))
    }

    fn allocate_name(&mut self) -> Result<String> {
        loop {
            let suffix = self.next_name.ok_or_else(|| {
                Error::Other("table of contents exhausted the bookmark name range".to_owned())
            })?;
            let name = format!("_Toc{suffix}");
            self.next_name = suffix.checked_add(1);
            if self.names.insert(name.clone()) {
                return Ok(name);
            }
        }
    }
}

#[derive(Debug, Clone)]
struct TocSource {
    paragraph_index: usize,
    level: u8,
    title: String,
    omit_page_number: bool,
    needs_bookmark: bool,
    sequence_prefix: Option<String>,
    numbering_prefix: Option<TocNumberingPrefix>,
}

#[derive(Debug, Clone)]
struct TocNumberingPrefix {
    marker: String,
    suffix: ST_LvlSuffix,
}

#[derive(Debug, Clone)]
struct TocEntryStyle {
    style_id: String,
    has_right_tab: bool,
}

fn discover_toc_sources(
    document: &Document,
    spans: &[DynamicTocSpan],
    fields: &[Option<TocField>],
    bookmarks: &TocBookmarkState,
    numbering_layout: &Arc<oxoffice_docx_layout::WordLayoutResult>,
) -> Result<Vec<Vec<TocSource>>> {
    let mut paragraphs = Vec::new();
    collect_body_paragraphs(&document.document.body, &mut paragraphs, None);
    let context = FieldEvaluationContext::default();
    let sequence_snapshot = Arc::new(
        oxoffice_docx_layout::engine::evaluate_sequence_fields(&document.build_layout_input()?)
            .map_err(|error| {
                Error::Other(format!("TOC sequence source evaluation failed: {error}"))
            })?,
    );
    let mut sequence_nodes = Vec::new();
    for index in 1u32.. {
        let Some(node) = oxoffice_docx_layout::SourceNodeId::new(index) else {
            break;
        };
        let Some(path) = sequence_snapshot.source_node(node) else {
            break;
        };
        if matches!(path.story, oxoffice_docx_layout::WordStory::Document) {
            sequence_nodes.push(node);
        }
    }
    if sequence_nodes.len() != paragraphs.len() {
        return Err(Error::Other(
            "TOC sequence paragraph source inventory is incomplete".into(),
        ));
    }
    let mut all_sources = Vec::with_capacity(fields.len());
    for toc in fields {
        let Some(toc) = toc else {
            all_sources.push(Vec::new());
            continue;
        };
        let bookmark_range = toc
            .bookmark
            .as_ref()
            .map(|name| {
                bookmarks.named_ranges.get(name).copied().ok_or_else(|| {
                    Error::Other(format!(
                        "table of contents source bookmark {name} was not found"
                    ))
                })
            })
            .transpose()?;
        let mut sources = Vec::new();
        let mut evaluator = Evaluator::new(document, &context);
        evaluator.bind_numbering_layout(Arc::clone(numbering_layout));
        evaluator.sequence_snapshot = Some(Arc::clone(&sequence_snapshot));
        let mut sequence_value = None;
        for (paragraph_index, paragraph) in paragraphs.iter().enumerate() {
            let paragraph_fully_owned = spans.iter().any(|span| {
                paragraph_index > span.begin_paragraph && paragraph_index < span.end_paragraph
            });
            if paragraph_fully_owned {
                continue;
            }
            if evaluator
                .numbering_source(sequence_nodes[paragraph_index])
                .and_then(|node| numbering_layout.paragraph_omitted_by_table(node))
                == Some(true)
            {
                continue;
            }
            evaluator.register_paragraph_fields(paragraph, Some(sequence_nodes[paragraph_index]));
            let accepted_runs = accepted_toc_runs(paragraph);
            for (accepted_run_index, run) in accepted_runs.iter().enumerate() {
                let position = TocDocumentPosition {
                    paragraph: paragraph_index,
                    accepted_run_index,
                };
                let in_bookmark =
                    bookmark_range.is_none_or(|(start, end)| position >= start && position < end);
                for content in &run.run.content {
                    let RunContent::Field(field) = content else {
                        continue;
                    };
                    let owned_position = TocOwnedPosition {
                        paragraph: paragraph_index,
                        run: TocRunPosition {
                            run_boundary: run.run_boundary,
                            raw_order: run.raw_order,
                            nested_order: run.nested_order,
                        },
                    };
                    if toc_source_position_is_owned(spans, owned_position) {
                        continue;
                    }
                    let outcome =
                        evaluator.evaluate_field(field, "main", &paragraphs, paragraph_index, None);
                    if field.instruction.name == "SEQ"
                        && let Some(identifier) = field.instruction.arguments.first()
                        && field_argument_text(identifier).is_some_and(|name| {
                            toc.sequence_identifier
                                .as_deref()
                                .is_some_and(|selected| selected.eq_ignore_ascii_case(name))
                        })
                        && let FieldOutcome::Resolved(ref value) = outcome
                    {
                        sequence_value = Some(value.clone());
                    }
                    if !in_bookmark {
                        continue;
                    }
                    let FieldOutcome::TableOfContentsEntry(tc) = outcome else {
                        continue;
                    };
                    if toc_accepts_tc(toc, &tc) {
                        let omit_page_number = tc.omit_page_number
                            || toc
                                .omit_page_number_levels
                                .is_some_and(|(start, end)| (start..=end).contains(&tc.level));
                        sources.push(TocSource {
                            paragraph_index,
                            level: tc.level,
                            title: tc.entry,
                            omit_page_number,
                            needs_bookmark: toc.hyperlink || !omit_page_number,
                            sequence_prefix: sequence_value.clone(),
                            numbering_prefix: toc_numbering_prefix(
                                numbering_layout,
                                paragraph_index,
                            ),
                        });
                    }
                }
            }
            let paragraph_in_bookmark = bookmark_range.is_none_or(|(start, end)| {
                accepted_runs
                    .iter()
                    .enumerate()
                    .any(|(accepted_run_index, run)| {
                        let position = TocDocumentPosition {
                            paragraph: paragraph_index,
                            accepted_run_index,
                        };
                        let owned_position = TocOwnedPosition {
                            paragraph: paragraph_index,
                            run: TocRunPosition {
                                run_boundary: run.run_boundary,
                                raw_order: run.raw_order,
                                nested_order: run.nested_order,
                            },
                        };
                        !toc_source_position_is_owned(spans, owned_position)
                            && position >= start
                            && position < end
                    })
            });
            if !paragraph_in_bookmark {
                continue;
            }
            let effective_heading = evaluator
                .numbering_source(sequence_nodes[paragraph_index])
                .and_then(|node| numbering_layout.paragraph_navigation_source(node))
                .map(|(_, level, ..)| level);
            let Some(level) = toc_paragraph_level(document, toc, paragraph, effective_heading)?
            else {
                continue;
            };
            let title = accepted_runs
                .iter()
                .filter(|run| {
                    !toc_source_position_is_owned(
                        spans,
                        TocOwnedPosition {
                            paragraph: paragraph_index,
                            run: TocRunPosition {
                                run_boundary: run.run_boundary,
                                raw_order: run.raw_order,
                                nested_order: run.nested_order,
                            },
                        },
                    )
                })
                .map(|run| run.run.text())
                .collect::<String>();
            if title.is_empty() {
                continue;
            }
            let omit_page_number = toc
                .omit_page_number_levels
                .is_some_and(|(start, end)| (start..=end).contains(&level));
            sources.push(TocSource {
                paragraph_index,
                level,
                title,
                omit_page_number,
                needs_bookmark: toc.hyperlink || !omit_page_number,
                sequence_prefix: sequence_value.clone(),
                numbering_prefix: toc_numbering_prefix(numbering_layout, paragraph_index),
            });
        }
        sources.sort_by_key(|source| source.paragraph_index);
        all_sources.push(sources);
    }
    Ok(all_sources)
}

fn toc_numbering_prefix(
    layout: &oxoffice_docx_layout::WordLayoutResult,
    paragraph_index: usize,
) -> Option<TocNumberingPrefix> {
    let numbering = layout.document_paragraph_numbering(paragraph_index)?;
    if numbering.marker_text.is_empty() {
        return None;
    }
    Some(TocNumberingPrefix {
        marker: numbering.marker_text.clone(),
        suffix: numbering.suffix,
    })
}

fn ensure_toc_entry_styles(
    document: &mut Document,
    sources: &[Vec<TocSource>],
) -> Result<(BTreeMap<u8, TocEntryStyle>, Vec<String>)> {
    let source_styles = toc_style_view(&document.styles);
    let mut levels = sources
        .iter()
        .flatten()
        .map(|source| source.level)
        .collect::<Vec<_>>();
    levels.sort_unstable();
    levels.dedup();
    let mut resolved = BTreeMap::new();
    for level in levels {
        let built_in_name = format!("toc {level}");
        let canonical_id = format!("TOC{level}");
        let style_id = document
            .styles
            .styles
            .iter()
            .find(|style| {
                // Entries reference their style by identifier, so a producer
                // style without one cannot be the entry style.
                style.style_type == StyleType::Paragraph
                    && !style.style_id.is_empty()
                    && style
                        .name
                        .as_deref()
                        .is_some_and(|name| name.trim().eq_ignore_ascii_case(&built_in_name))
            })
            .map(|style| style.style_id.clone())
            .or_else(|| {
                document
                    .styles
                    .get_by_id(&canonical_id)
                    .map(|style| style.style_id.clone())
            })
            .unwrap_or_else(|| {
                let (style, _, _) =
                    style::StyleBuilder::paragraph(&canonical_id, &format!("TOC {level}")).build();
                std::sync::Arc::make_mut(&mut document.styles)
                    .styles
                    .push(style);
                canonical_id
            });
        let effective = style::resolve_paragraph_properties(Some(&style_id), &document.styles);
        let has_right_tab = effective
            .tabs
            .as_ref()
            .is_some_and(|tabs| tabs.tabs.iter().any(|tab| tab.val == ST_TabJc::Right));
        resolved.insert(
            level,
            TocEntryStyle {
                style_id,
                has_right_tab,
            },
        );
    }
    // Producer defects the read surface accepts stay in the package and are
    // reported. Only a defect the staged entry styles introduce rejects.
    let retained =
        style::validate_style_graph_change(&source_styles, &toc_style_view(&document.styles))?
            .into_iter()
            .map(|defect| format!("{defect}, retained while rebuilding TOC"))
            .collect();
    Ok((resolved, retained))
}

/// The style graph a TOC rebuild resolves. It keeps the first definition of
/// each style ID and, for each style type, the default that
/// `CT_Styles::get_default` gives layout. The package keeps every definition.
fn toc_style_view(styles: &CT_Styles) -> CT_Styles {
    let mut view = styles.clone();
    let mut seen = HashSet::new();
    view.styles
        .retain(|style| seen.insert(style.style_id.clone()));
    for style in &mut view.styles {
        style.is_default = styles
            .get_default(style.style_type)
            .is_some_and(|default| default.style_id == style.style_id);
    }
    view
}

fn toc_default_style_diagnostics(styles: &CT_Styles) -> Vec<String> {
    [
        StyleType::Paragraph,
        StyleType::Character,
        StyleType::Table,
        StyleType::Numbering,
    ]
    .into_iter()
    .filter_map(|style_type| {
        let first = styles.get_default(style_type)?;
        styles
            .styles
            .iter()
            .any(|style| {
                style.style_type == style_type
                    && style.is_default
                    && style.style_id != first.style_id
            })
            .then(|| {
                format!(
                    "multiple default {} styles used first default '{}' while rebuilding TOC",
                    style_type.to_str(),
                    first.style_id
                )
            })
    })
    .collect()
}

fn toc_duplicate_style_diagnostics(styles: &CT_Styles) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut reported = HashSet::new();
    styles
        .styles
        .iter()
        .filter_map(|style| {
            if seen.insert(style.style_id.as_str()) || !reported.insert(style.style_id.as_str()) {
                return None;
            }
            Some(format!(
                "duplicate style ID '{}' used first definition while rebuilding TOC",
                style.style_id
            ))
        })
        .collect()
}

fn toc_section_text_width(body: &CT_Body, paragraph_index: usize) -> i32 {
    const DEFAULT_PAGE_WIDTH: i32 = 12_240;
    const DEFAULT_MARGIN: i32 = 1_440;
    const DEFAULT_TEXT_WIDTH: i32 = DEFAULT_PAGE_WIDTH - 2 * DEFAULT_MARGIN;

    let mut paragraphs = Vec::new();
    collect_body_paragraphs(body, &mut paragraphs, None);
    let section = paragraphs
        .iter()
        .skip(paragraph_index)
        .find_map(|paragraph| {
            paragraph
                .properties
                .as_ref()
                .and_then(|properties| properties.sect_pr.as_ref())
        })
        .or(body.sect_pr.as_ref());
    let Some(section) = section else {
        return DEFAULT_TEXT_WIDTH;
    };
    let page_width = section
        .page_width
        .map(|width| width.0)
        .unwrap_or(DEFAULT_PAGE_WIDTH);
    let left = section
        .margin_left
        .map(|margin| margin.0)
        .unwrap_or(DEFAULT_MARGIN);
    let right = section
        .margin_right
        .map(|margin| margin.0)
        .unwrap_or(DEFAULT_MARGIN);
    page_width
        .checked_sub(left)
        .and_then(|width| width.checked_sub(right))
        .filter(|width| *width > 0)
        .unwrap_or(DEFAULT_TEXT_WIDTH)
}

fn toc_source_position_is_owned(spans: &[DynamicTocSpan], position: TocOwnedPosition) -> bool {
    spans.iter().any(|span| {
        #[cfg(test)]
        NAVIGATION_OWNER_VISITS.set(NAVIGATION_OWNER_VISITS.get() + 1);
        if span.begin_paragraph == span.end_paragraph && position.paragraph == span.begin_paragraph
        {
            return position.run >= span.result_start_position
                && position.run < span.result_end_position;
        }
        if position.paragraph > span.begin_paragraph && position.paragraph < span.end_paragraph {
            return true;
        }
        if position.paragraph == span.begin_paragraph {
            return position.run >= span.result_start_position;
        }
        if position.paragraph == span.end_paragraph {
            return position.run < span.result_end_position;
        }
        false
    })
}

#[derive(Clone, Copy)]
struct AcceptedTocRun<'a> {
    run: &'a CT_R,
    run_boundary: usize,
    raw_order: TocRawOrder,
    nested_order: usize,
}

fn accepted_toc_runs(paragraph: &CT_P) -> Vec<AcceptedTocRun<'_>> {
    let mut runs = Vec::new();
    append_accepted_paragraph_runs(paragraph, None, &mut runs);
    let mut nested_run_orders = HashMap::<(usize, TocRawOrder), usize>::new();
    for run in &mut runs {
        let next = nested_run_orders
            .entry((run.run_boundary, run.raw_order))
            .or_default();
        run.nested_order = *next;
        *next += 1;
    }
    runs
}

fn accepted_paragraph_owners(
    paragraph: &CT_P,
    boundary: usize,
) -> Vec<(TocRawOrder, AcceptedParagraphOwner<'_>)> {
    let mut owners = paragraph
        .content_controls
        .iter()
        .filter(|(at, _, _, _)| *at == boundary)
        .map(|(_, raw_before, _, control)| {
            (
                TocRawOrder::Raw(*raw_before),
                0u8,
                AcceptedParagraphOwner::Control(control),
            )
        })
        .chain(
            paragraph
                .revisions
                .iter()
                .filter(|(at, _, _)| *at == boundary)
                .map(|(_, slot, revision)| {
                    (
                        accepted_revision_raw_order(paragraph, boundary, *slot),
                        1u8,
                        AcceptedParagraphOwner::Revision(revision),
                    )
                }),
        )
        .collect::<Vec<_>>();
    owners.sort_by_key(|(raw_order, kind, _)| (*raw_order, *kind));
    owners
        .into_iter()
        .map(|(order, _, owner)| (order, owner))
        .collect()
}

fn append_accepted_paragraph_runs<'a>(
    paragraph: &'a CT_P,
    inherited_position: Option<(usize, TocRawOrder)>,
    runs: &mut Vec<AcceptedTocRun<'a>>,
) {
    for boundary in 0..=paragraph.runs.len() {
        for (raw_order, owner) in accepted_paragraph_owners(paragraph, boundary) {
            let position = inherited_position.unwrap_or((boundary, raw_order));
            match owner {
                AcceptedParagraphOwner::Control(control) => {
                    append_accepted_control_runs(control, position, runs);
                }
                AcceptedParagraphOwner::Revision(revision) => {
                    append_accepted_revision_runs(revision, position, runs);
                }
            }
        }
        if let Some(run) = paragraph.runs.get(boundary) {
            let (run_boundary, raw_order) =
                inherited_position.unwrap_or((boundary, TocRawOrder::AfterRaw));
            runs.push(AcceptedTocRun {
                run,
                run_boundary,
                raw_order,
                nested_order: 0,
            });
        }
    }
}

fn accepted_revision_raw_order(paragraph: &CT_P, boundary: usize, slot: usize) -> TocRawOrder {
    let Some(index) = hyperlink_revision_index(slot) else {
        return TocRawOrder::Raw(slot);
    };
    if let Some(raw_before) = paragraph
        .hyperlinks
        .get(index)
        .and_then(|hyperlink| hyperlink.preserved_raw_before)
    {
        TocRawOrder::Raw(raw_before)
    } else if paragraph
        .hyperlinks
        .get(index)
        .is_some_and(|hyperlink| boundary == hyperlink.run_end)
    {
        TocRawOrder::BeforeRaw
    } else {
        TocRawOrder::AfterRaw
    }
}

enum AcceptedParagraphOwner<'a> {
    Control(&'a CT_Sdt),
    Revision(&'a CT_Revision),
}

fn append_accepted_control_runs<'a>(
    control: &'a CT_Sdt,
    position: (usize, TocRawOrder),
    runs: &mut Vec<AcceptedTocRun<'a>>,
) {
    for boundary in 0..=control.content.len() {
        for (_, revision) in control.revisions().iter().filter(|(at, _)| *at == boundary) {
            append_accepted_revision_runs(revision, position, runs);
        }
        if let Some(content) = control.content.get(boundary) {
            match content {
                SdtContent::Run(run) => runs.push(AcceptedTocRun {
                    run,
                    run_boundary: position.0,
                    raw_order: position.1,
                    nested_order: 0,
                }),
                SdtContent::ContentControl(control) => {
                    append_accepted_control_runs(control, position, runs)
                }
                SdtContent::Paragraph(paragraph) => {
                    append_accepted_paragraph_runs(paragraph, Some(position), runs)
                }
                SdtContent::Table(table) => append_accepted_table_runs(table, position, runs),
                SdtContent::Row(row) => append_accepted_row_runs(row, position, runs),
                SdtContent::Cell(cell) => append_accepted_cell_runs(cell, position, runs),
                SdtContent::RawXml(_) => {}
            }
        }
    }
}

fn append_accepted_revision_runs<'a>(
    revision: &'a CT_Revision,
    position: (usize, TocRawOrder),
    runs: &mut Vec<AcceptedTocRun<'a>>,
) {
    match revision.kind() {
        RevisionKind::Insertion | RevisionKind::MoveTo => {
            if let Some(paragraph) = revision.content_paragraph() {
                append_accepted_paragraph_runs(paragraph, Some(position), runs);
                return;
            }
            let RevisionContent::Runs(direct) = revision.content() else {
                return;
            };
            for boundary in 0..=direct.len() {
                for (_, nested) in revision
                    .nested_revisions()
                    .iter()
                    .filter(|(at, _)| *at == boundary)
                {
                    append_accepted_revision_runs(nested, position, runs);
                }
                if let Some(run) = direct.get(boundary) {
                    runs.push(AcceptedTocRun {
                        run,
                        run_boundary: position.0,
                        raw_order: position.1,
                        nested_order: 0,
                    });
                }
            }
        }
        RevisionKind::Deletion
        | RevisionKind::MoveFrom
        | RevisionKind::RunPropertyChange
        | RevisionKind::ParagraphPropertyChange
        | RevisionKind::TablePropertyChange
        | RevisionKind::SectionPropertyChange => {}
    }
}

fn append_accepted_table_runs<'a>(
    table: &'a CT_Tbl,
    position: (usize, TocRawOrder),
    runs: &mut Vec<AcceptedTocRun<'a>>,
) {
    for boundary in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == boundary)
        {
            append_accepted_control_runs(control, position, runs);
        }
        if let Some(row) = table.rows.get(boundary) {
            append_accepted_row_runs(row, position, runs);
        }
    }
}

fn append_accepted_row_runs<'a>(
    row: &'a CT_Row,
    position: (usize, TocRawOrder),
    runs: &mut Vec<AcceptedTocRun<'a>>,
) {
    for boundary in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == boundary)
        {
            append_accepted_control_runs(control, position, runs);
        }
        if let Some(cell) = row.cells.get(boundary) {
            append_accepted_cell_runs(cell, position, runs);
        }
    }
}

fn append_accepted_cell_runs<'a>(
    cell: &'a CT_Tc,
    position: (usize, TocRawOrder),
    runs: &mut Vec<AcceptedTocRun<'a>>,
) {
    for content in &cell.content {
        match content {
            CellContent::Paragraph(paragraph) => {
                append_accepted_paragraph_runs(paragraph, Some(position), runs)
            }
            CellContent::Table(table) => append_accepted_table_runs(table, position, runs),
            CellContent::ContentControl(control) => {
                append_accepted_control_runs(control, position, runs)
            }
        }
    }
}

fn field_argument_text(argument: &FieldArgument) -> Option<&str> {
    match argument {
        FieldArgument::Text(value) => Some(value),
        FieldArgument::Nested(_) => None,
    }
}

fn toc_accepts_tc(toc: &TocField, tc: &TcField) -> bool {
    match &toc.entries {
        TocEntrySelection::None => false,
        TocEntrySelection::All => true,
        TocEntrySelection::Identifier(identifier) => tc
            .table_identifier
            .as_deref()
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(identifier)),
    }
}

fn toc_paragraph_level(
    document: &Document,
    toc: &TocField,
    paragraph: &CT_P,
    effective_heading: Option<Option<u32>>,
) -> Result<Option<u8>> {
    let style_id = paragraph
        .properties
        .as_ref()
        .and_then(|properties| properties.style_id.as_deref());
    if let Some(style_id) = style_id {
        for (name, level) in &toc.custom_styles {
            let matches_id = style_id.eq_ignore_ascii_case(name);
            let matches_name = document
                .styles
                .get_by_id(style_id)
                .and_then(|style| style.name.as_deref())
                .is_some_and(|style_name| style_name.eq_ignore_ascii_case(name));
            if matches_id || matches_name {
                return Ok(Some(*level));
            }
        }
    }
    let outline = if toc.use_outline_levels {
        match effective_heading {
            Some(Some(level)) => u8::try_from(level)
                .ok()
                .filter(|level| (1..=9).contains(level)),
            Some(None) => return Ok(None),
            None => {
                return Err(Error::Other(
                    "TOC outline source has no qualified current layout facts".into(),
                ));
            }
        }
    } else {
        None
    };
    if let Some(style_id) = style_id
        && let Some((start, end)) = toc.heading_levels
        && let Some(level) = heading_style_level(document, style_id)
        && (start..=end).contains(&level)
    {
        return Ok(Some(level));
    }
    // Apply an explicit heading range to outline-derived entries as well.
    Ok(outline.filter(|level| {
        toc.heading_levels
            .is_none_or(|(start, end)| (start..=end).contains(level))
    }))
}

fn heading_style_level(document: &Document, style_id: &str) -> Option<u8> {
    let candidates = std::iter::once(style_id).chain(
        document
            .styles
            .get_by_id(style_id)
            .and_then(|style| style.name.as_deref()),
    );
    for candidate in candidates {
        let normalized = candidate
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        if let Some(suffix) = normalized.strip_prefix("heading")
            && let Ok(level) = suffix.parse::<u8>()
            && (1..=9).contains(&level)
        {
            return Some(level);
        }
    }
    None
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

fn insert_toc_bookmarks_xml(
    xml: &[u8],
    bookmarks: &BTreeMap<usize, TocBookmark>,
    repairs: &[TocBookmarkRepair],
    all_toc_spans: &[DynamicTocSpan],
    toc_spans: &[DynamicTocSpan],
) -> Result<Vec<u8>> {
    let paragraphs = toc_paragraph_insertions(xml)?;
    if bookmarks.keys().any(|index| *index >= paragraphs.len()) {
        return Err(Error::Other(
            "table of contents source paragraph was not found in package XML".to_owned(),
        ));
    }
    let mut insertions = BTreeMap::<usize, Vec<u8>>::new();
    for (paragraph_index, paragraph) in paragraphs.iter().enumerate() {
        let bookmark = bookmarks
            .get(&paragraph_index)
            .filter(|bookmark| bookmark.insert);
        if bookmark.is_none() {
            continue;
        }
        let end_boundary = toc_spans
            .iter()
            .any(|span| span.end_paragraph == paragraph_index);
        let fragment_start = toc_spans
            .iter()
            .filter(|span| span.end_paragraph == paragraph_index)
            .map(|span| span.end_run_end)
            .max()
            .unwrap_or(paragraph.content_start);
        let content_end = toc_spans
            .iter()
            .filter(|span| span.begin_paragraph == paragraph_index)
            .map(|span| span.begin_run_start)
            .min()
            .unwrap_or(paragraph.content_end);
        if fragment_start > content_end {
            return Err(Error::Other(
                "table of contents source bookmark has no unowned paragraph range".to_owned(),
            ));
        }
        if !end_boundary && let Some(bookmark) = bookmark {
            let start_insertion = insertions.entry(fragment_start).or_default();
            start_insertion.extend_from_slice(
                format!(
                    "<w:bookmarkStart w:id=\"{}\" w:name=\"{}\" xmlns:w=\"{W_NS}\"/>",
                    bookmark.id,
                    xml_escape_attribute(&bookmark.name)
                )
                .as_bytes(),
            );
        }

        let end_insertion = insertions.entry(content_end).or_default();
        if let Some(bookmark) = bookmark {
            end_insertion.extend_from_slice(
                format!(
                    "<w:bookmarkEnd w:id=\"{}\" xmlns:w=\"{W_NS}\"/>",
                    bookmark.id
                )
                .as_bytes(),
            );
        }
    }
    let mut repair_ends = BTreeMap::<usize, Vec<i32>>::new();
    for repair in repairs.iter().filter(|repair| !repair.replace_start) {
        let span = all_toc_spans.get(repair.toc_index).ok_or_else(|| {
            Error::Other("table of contents bookmark repair owner was not found".to_owned())
        })?;
        repair_ends
            .entry(span.begin_run_start)
            .or_default()
            .push(repair.id);
    }
    for (position, ids) in repair_ends {
        let insertion = insertions.entry(position).or_default();
        for id in ids.into_iter().rev() {
            insertion.extend_from_slice(
                format!("<w:bookmarkEnd w:id=\"{id}\" xmlns:w=\"{W_NS}\"/>").as_bytes(),
            );
        }
    }
    let mut output = xml.to_vec();
    for (position, replacement) in insertions.into_iter().rev() {
        output.splice(position..position, replacement);
    }
    Ok(output)
}

fn toc_paragraph_insertions(xml: &[u8]) -> Result<Vec<TocParagraphInsertion>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut elements = Vec::<DynamicXmlElement>::new();
    let mut paragraphs = Vec::<TocParagraphInsertion>::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid table of contents XML: {error}")))?;
        let word = namespace_is_word(&namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let local = local_name(element.name().as_ref()).to_vec();
                let (namespace_bindings, inherited_namespaces) =
                    dynamic_namespace_bindings(&element, &elements)?;
                let mut typed_block_owner = classify_typed_block_owner(word, &local, &elements);
                let mut is_typed_inline_owner = false;
                validate_dynamic_content_control(
                    xml,
                    before,
                    word,
                    &local,
                    &namespace_bindings,
                    &mut typed_block_owner,
                    &mut is_typed_inline_owner,
                )?;
                let is_typed_paragraph = typed_block_owner == Some(TypedBlockOwner::Paragraph);
                let paragraph = if is_typed_paragraph {
                    let index = paragraphs.len();
                    paragraphs.push(TocParagraphInsertion {
                        start: before,
                        end: after,
                        name: String::from_utf8_lossy(element.name().as_ref()).into_owned(),
                        namespaces: namespace_bindings.clone(),
                        has_non_namespace_attributes: element.attributes().any(|attribute| {
                            attribute.is_ok_and(|attribute| {
                                attribute.key.as_ref() != b"xmlns"
                                    && !attribute.key.as_ref().starts_with(b"xmlns:")
                            })
                        }),
                        start_tag_end: after,
                        content_start: after,
                        content_end: after,
                    });
                    Some(index)
                } else {
                    elements.last().and_then(|element| element.paragraph)
                };
                mark_typed_sdt_content(&mut elements, &local, typed_block_owner, false);
                elements.push(DynamicXmlElement {
                    local_name: local,
                    qualified_name: String::from_utf8_lossy(element.name().as_ref()).into_owned(),
                    typed_block_owner,
                    is_word: word,
                    is_typed_paragraph,
                    is_typed_inline_owner: false,
                    revision_depth: 0,
                    sdt_content_seen: false,
                    namespace_bindings,
                    inherited_namespaces,
                    run_position: None,
                    hyperlink_plan: None,
                    start: before,
                    start_tag_end: after,
                    paragraph,
                });
            }
            Event::Empty(element) => {
                let name = element.name();
                let local = local_name(name.as_ref());
                // Empty content is a retained raw slot, not the typed Start slot.
                let word = word && local != b"sdtContent";
                let typed_block_owner = classify_typed_block_owner(word, local, &elements);
                if typed_block_owner == Some(TypedBlockOwner::Paragraph) {
                    paragraphs.push(TocParagraphInsertion {
                        start: before,
                        end: after,
                        name: String::from_utf8_lossy(element.name().as_ref()).into_owned(),
                        namespaces: BTreeMap::new(),
                        has_non_namespace_attributes: false,
                        start_tag_end: before,
                        content_start: before,
                        content_end: before,
                    });
                }
                mark_typed_sdt_content(&mut elements, local, typed_block_owner, false);
                if word
                    && local == b"pPr"
                    && let Some(parent) = elements.last()
                    && parent.is_typed_paragraph
                    && let Some(paragraph) = parent.paragraph
                {
                    paragraphs[paragraph].content_start = after;
                }
            }
            Event::End(element) => {
                let Some(closed) = elements.pop() else {
                    return Err(Error::Other(
                        "table of contents XML has an unmatched end element".to_owned(),
                    ));
                };
                if word
                    && matches_local_name(element.name().as_ref(), b"pPr")
                    && let Some(paragraph) = closed.paragraph
                    && elements
                        .last()
                        .is_some_and(|parent| parent.is_word && parent.local_name == b"p")
                {
                    paragraphs[paragraph].content_start = after;
                }
                if word
                    && matches_local_name(element.name().as_ref(), b"p")
                    && let Some(paragraph) = closed.paragraph
                {
                    paragraphs[paragraph].content_end = before;
                    paragraphs[paragraph].end = after;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(paragraphs)
}

#[derive(Debug)]
struct TocPagePlaceholder {
    toc_index: usize,
    token: String,
    bookmark: String,
}

fn render_toc_entries(
    toc_index: usize,
    toc: &TocField,
    sources: &[TocSource],
    entry_styles: &BTreeMap<u8, TocEntryStyle>,
    fallback_right_tab: i32,
    bookmarks: &BTreeMap<usize, TocBookmark>,
    source_xml: &[u8],
    placeholders: &mut Vec<TocPagePlaceholder>,
) -> Result<Vec<u8>> {
    let mut output = String::new();
    for source in sources {
        let bookmark = bookmarks.get(&source.paragraph_index);
        if source.needs_bookmark && bookmark.is_none() {
            return Err(Error::Other(
                "table of contents source bookmark was not allocated".to_owned(),
            ));
        }
        let entry_style = entry_styles.get(&source.level).ok_or_else(|| {
            Error::Other(format!(
                "table of contents level {} has no entry style",
                source.level
            ))
        })?;
        output.push_str("<w:p><w:pPr><w:pStyle w:val=\"");
        output.push_str(&xml_escape_attribute(&entry_style.style_id));
        output.push_str("\"/>");
        // Word gives the entry of a heading numbered with a tab its own left
        // stop after the number, so the title does not go to the page number
        // stop.
        let number_stop = match source.numbering_prefix.as_ref() {
            Some(prefix) if prefix.suffix == ST_LvlSuffix::Tab => {
                Some(toc_number_tab_stop(&prefix.marker, source.level))
            }
            _ => None,
        };
        let fallback_right = !source.omit_page_number
            && toc.page_number_separator.is_none()
            && !entry_style.has_right_tab;
        if number_stop.is_some() || fallback_right {
            output.push_str("<w:tabs>");
            if let Some(stop) = number_stop {
                output.push_str("<w:tab w:val=\"left\" w:pos=\"");
                output.push_str(&stop.to_string());
                output.push_str("\"/>");
            }
            if fallback_right {
                output.push_str("<w:tab w:val=\"right\" w:leader=\"dot\" w:pos=\"");
                output.push_str(&fallback_right_tab.to_string());
                output.push_str("\"/>");
            }
            output.push_str("</w:tabs>");
        }
        output.push_str("</w:pPr>");
        if toc.hyperlink {
            output.push_str("<w:hyperlink w:anchor=\"");
            output.push_str(&xml_escape_attribute(
                &bookmark.expect("required above").name,
            ));
            output.push_str("\">");
        }
        if let Some(prefix) = source.numbering_prefix.as_ref() {
            output.push_str("<w:r><w:t>");
            output.push_str(&xml_escape_text(&prefix.marker));
            output.push_str("</w:t></w:r>");
            match prefix.suffix {
                ST_LvlSuffix::Tab => output.push_str("<w:r><w:tab/></w:r>"),
                ST_LvlSuffix::Space => {
                    output.push_str("<w:r><w:t xml:space=\"preserve\"> </w:t></w:r>")
                }
                ST_LvlSuffix::Nothing => {}
            }
        }
        output.push_str("<w:r><w:t>");
        output.push_str(&xml_escape_text(&source.title));
        output.push_str("</w:t></w:r>");
        if toc.hyperlink {
            output.push_str("</w:hyperlink>");
        }
        if !source.omit_page_number {
            if let Some(separator) = toc.page_number_separator.as_deref() {
                output.push_str("<w:r><w:t xml:space=\"preserve\">");
                output.push_str(&xml_escape_text(separator));
                output.push_str("</w:t></w:r>");
            } else {
                output.push_str("<w:r><w:tab/></w:r>");
            }
            if let Some(sequence) = source.sequence_prefix.as_deref() {
                output.push_str("<w:r><w:t>");
                output.push_str(&xml_escape_text(sequence));
                output.push_str("</w:t></w:r><w:r><w:t>");
                output.push_str(&xml_escape_text(
                    toc.entry_page_separator.as_deref().unwrap_or("-"),
                ));
                output.push_str("</w:t></w:r>");
            }
            let bookmark = &bookmark.expect("required above").name;
            let token = collision_safe_toc_page_token(source_xml, output.as_bytes(), placeholders);
            output.push_str("<w:fldSimple w:instr=\" PAGEREF ");
            output.push_str(&xml_escape_attribute(bookmark));
            output.push_str(" \\h \" w:dirty=\"0\"><w:r><w:t>");
            output.push_str(&token);
            output.push_str("</w:t></w:r></w:fldSimple>");
            placeholders.push(TocPagePlaceholder {
                toc_index,
                token,
                bookmark: bookmark.clone(),
            });
        }
        output.push_str("</w:p>");
    }
    Ok(output.into_bytes())
}

/// Advance widths of Aptos, Word's default font, for the printable ASCII
/// characters from space to `~`, in its 2048 units per em.
///
/// Word measures a TOC entry's number in this font, so these values are
/// facts about Word's output rather than about the document. They are the
/// font's own advances: Word writes 2509 twips for "WWWWWWWWWW1", which is
/// exactly their sum at 12 points plus 240.
const APTOS_ADVANCES: [u16; 95] = [
    416, 600, 757, 1100, 1094, 1692, 1317, 431, 600, 600, 936, 1094, 585, 697, 585, 695, 1094,
    1094, 1094, 1094, 1094, 1094, 1094, 1094, 1094, 1094, 585, 585, 1094, 1094, 1094, 1027, 1832,
    1207, 1237, 1418, 1405, 1139, 1074, 1451, 1448, 533, 678, 1164, 1023, 1618, 1446, 1500, 1182,
    1500, 1241, 1159, 980, 1395, 1199, 1827, 1132, 1105, 1056, 603, 695, 603, 1094, 942, 1130,
    1088, 1149, 1075, 1148, 1079, 617, 992, 1129, 489, 489, 997, 533, 1747, 1129, 1130, 1149, 1148,
    685, 995, 662, 1144, 926, 1476, 906, 926, 897, 603, 553, 603, 1094,
];

/// The left stop Word writes after the number of a TOC entry, in twips.
///
/// Word measures the number in its own default font, 12 pt Aptos in current
/// releases, whatever the document's fonts, and puts the stop on the first
/// 12 pt boundary at least 12 pt past the number, counting 12 pt of indent
/// for each level below the first. A character outside the table counts as
/// one em.
fn toc_number_tab_stop(marker: &str, level: u8) -> i32 {
    const STEP: u64 = 240;
    let units = marker.chars().fold(0u64, |sum, ch| {
        let advance = u32::from(ch)
            .checked_sub(0x20)
            .and_then(|index| APTOS_ADVANCES.get(index as usize))
            .map_or(2048, |advance| u32::from(*advance));
        sum.saturating_add(u64::from(advance))
    });
    // The indent is `level - 1` steps and the stop is one step past the
    // number, so the two cancel to `level` plus the rounded-up width.
    let steps = u64::from(level)
        .saturating_add(units.saturating_add(2047) / 2048)
        .min((i32::MAX as u64) / STEP);
    (steps as i32) * STEP as i32
}

fn collision_safe_toc_page_token(
    source_xml: &[u8],
    generated: &[u8],
    placeholders: &[TocPagePlaceholder],
) -> String {
    let index = placeholders.len();
    for nonce in 0u64.. {
        let token = format!("__RDOCX_TOC_PAGE_{index}_{nonce}__");
        if find_bytes(source_xml, token.as_bytes()).is_none()
            && find_bytes(generated, token.as_bytes()).is_none()
            && placeholders
                .iter()
                .all(|placeholder| placeholder.token != token)
        {
            return token;
        }
    }
    unreachable!("u64 placeholder nonce space is finite but non-empty")
}

fn xml_escape_text(value: &str) -> String {
    quick_xml::escape::escape(value).into_owned()
}

fn xml_escape_attribute(value: &str) -> String {
    quick_xml::escape::escape(value).into_owned()
}

fn end_boundary_bookmark_starts(
    toc_index: usize,
    span: &DynamicTocSpan,
    spans: &[DynamicTocSpan],
    fields: &[Option<TocField>],
    bookmarks: &BTreeMap<usize, TocBookmark>,
    repairs: &[TocBookmarkRepair],
) -> Vec<u8> {
    let mut output = Vec::new();
    for repair in repairs
        .iter()
        .filter(|repair| repair.toc_index == toc_index && repair.replace_start)
    {
        output.extend_from_slice(
            format!(
                "<w:bookmarkStart w:id=\"{}\" w:name=\"{}\" xmlns:w=\"{W_NS}\"/>",
                repair.id,
                xml_escape_attribute(&repair.name)
            )
            .as_bytes(),
        );
    }
    let owns_last_end_boundary = fields.get(toc_index).is_some_and(Option::is_some)
        && spans
            .iter()
            .zip(fields)
            .filter(|(candidate, field)| {
                field.is_some() && candidate.end_paragraph == span.end_paragraph
            })
            .all(|(candidate, _)| candidate.end_run_end <= span.end_run_end);
    if owns_last_end_boundary
        && let Some(bookmark) = bookmarks
            .get(&span.end_paragraph)
            .filter(|bookmark| bookmark.insert)
    {
        output.extend_from_slice(
            format!(
                "<w:bookmarkStart w:id=\"{}\" w:name=\"{}\" xmlns:w=\"{W_NS}\"/>",
                bookmark.id,
                xml_escape_attribute(&bookmark.name)
            )
            .as_bytes(),
        );
    }
    output
}

fn dynamic_toc_replacement(
    xml: &[u8],
    span: &DynamicTocSpan,
    generated: &[u8],
    end_boundary_bookmark_starts: &[u8],
) -> Result<Vec<u8>> {
    if span.result_start > span.end_paragraph_start
        || span.end_paragraph_start > span.end_paragraph_content_start
        || span.end_paragraph_content_start > span.result_end
        || span.result_end > xml.len()
    {
        return Err(Error::Other(
            "table of contents result paragraph boundaries are invalid".to_owned(),
        ));
    }
    let mut prior_wrapper_end = span.end_paragraph_content_start;
    for &(start, end) in &span.end_wrapper_prefixes {
        if start < prior_wrapper_end || start >= end || end > span.result_end {
            return Err(Error::Other(
                "table of contents end-marker wrappers are invalid".to_owned(),
            ));
        }
        prior_wrapper_end = end;
    }
    let mut replacement = Vec::new();
    append_toc_wrapper_closures(&mut replacement, &span.separator_wrapper_names);
    replacement.extend_from_slice(b"</");
    replacement.extend_from_slice(span.start_paragraph_name.as_bytes());
    replacement.push(b'>');
    replacement.extend_from_slice(generated);
    replacement.extend_from_slice(&xml[span.end_paragraph_start..span.end_paragraph_content_start]);
    replacement.extend_from_slice(end_boundary_bookmark_starts);
    for &(start, end) in &span.end_wrapper_prefixes {
        replacement.extend_from_slice(&xml[start..end]);
    }
    Ok(replacement)
}

/// Reuse eligible entry paragraph roots while replacing only owned entry facts.
fn entry_boundary_toc_edits(
    xml: &[u8],
    span: &DynamicTocSpan,
    paragraphs: &[TocParagraphInsertion],
    generated: &[u8],
    entry_style_ids: &BTreeSet<String>,
    end_boundary_bookmark_starts: &[u8],
) -> Result<Option<Vec<FieldSourceEdit>>> {
    if !span.separator_wrapper_names.is_empty()
        || !span.end_wrapper_prefixes.is_empty()
        || !end_boundary_bookmark_starts.is_empty()
    {
        return Ok(None);
    }
    let begin = &paragraphs[span.begin_paragraph];
    let end = &paragraphs[span.end_paragraph];
    let single = span.begin_paragraph == span.end_paragraph;
    let begin_properties = &xml[begin.start_tag_end..begin.content_start];
    let end_properties = &xml[end.start_tag_end..end.content_start];
    let begin_scan = toc_fragment_scan(begin_properties, &begin.namespaces)?;
    let end_scan = toc_fragment_scan(end_properties, &end.namespaces)?;
    let begin_content = toc_fragment_scan(
        &xml[span.result_start..if single {
            span.result_end
        } else {
            begin.content_end
        }],
        &begin.namespaces,
    )?;
    let after_end = toc_fragment_scan(&xml[span.end_run_end..end.content_end], &end.namespaces)?;
    let is_entry = |scan: &TocFragmentScan, has_run: bool| {
        !scan.blocks_replacement
            && has_run
            && scan
                .style
                .as_ref()
                .is_some_and(|(id, _)| entry_style_ids.contains(id))
    };
    let merge_begin = begin.content_start == span.begin_run_start
        && (is_entry(&begin_scan, begin_content.has_run)
            || (single && !begin_content.has_run && !begin_scan.blocks_replacement));
    let merge_end = !after_end.has_run
        && is_entry(
            &end_scan,
            toc_fragment_scan(&xml[end.content_start..span.result_end], &end.namespaces)?.has_run,
        );
    let mut entries = Vec::new();
    let mut offset = 0;
    for close in byte_match_offsets(generated, b"</w:p>") {
        let next = close + b"</w:p>".len();
        entries.push(&generated[offset..next]);
        offset = next;
    }
    if offset != generated.len() {
        return Ok(None);
    }
    if single && (!merge_begin || after_end.has_run) {
        return Ok(None);
    }
    if !single && !merge_begin && !merge_end {
        return Ok(None);
    }
    if entries.is_empty() {
        let mut edits = Vec::new();
        if merge_begin {
            edits.push(FieldSourceEdit {
                start: begin.start_tag_end,
                end: begin.content_start,
                replacement: toc_overlay_entry_properties(begin_properties, &begin_scan, None)?,
            });
        }
        let replacement = if single {
            Vec::new()
        } else {
            // Zero entries retain the balanced two-root topology. Remove only
            // the entry style on an eligible owner, preserving its other facts.
            let mut replacement = format!("</{}>", begin.name).into_bytes();
            replacement.extend_from_slice(&xml[end.start..end.start_tag_end]);
            if merge_end {
                replacement.extend_from_slice(&toc_overlay_entry_properties(
                    end_properties,
                    &end_scan,
                    None,
                )?);
            } else {
                replacement.extend_from_slice(end_properties);
            }
            replacement
        };
        edits.push(FieldSourceEdit {
            start: span.result_start,
            end: span.result_end,
            replacement,
        });
        return Ok(Some(edits));
    }
    // Combining two roots cannot silently discard identity or foreign attributes
    // on the removed end root. Keep that root in the generic topology instead.
    let keep_end_root = entries.len() == 1
        && merge_begin
        && merge_end
        && !single
        && (end.has_non_namespace_attributes || end_scan.has_opaque_properties);
    let merge_end = merge_end && !keep_end_root;
    let generated_bindings = BTreeMap::from([("w".to_owned(), W_NS.to_owned())]);
    let mut edits = Vec::new();
    let mut replacement = Vec::new();
    if !merge_begin {
        replacement.extend_from_slice(format!("</{}>", begin.name).as_bytes());
    }
    for (index, entry) in entries.iter().enumerate() {
        let properties_end = find_bytes(entry, b"</w:pPr>").ok_or_else(|| {
            Error::Other("generated TOC entry has no paragraph properties".to_owned())
        })? + b"</w:pPr>".len();
        let generated_properties = &entry[b"<w:p>".len()..properties_end];
        let first_reused = index == 0 && merge_begin;
        let last_reused = !single && index + 1 == entries.len() && merge_end;
        if first_reused {
            edits.push(FieldSourceEdit {
                start: begin.start_tag_end,
                end: begin.content_start,
                replacement: toc_overlay_entry_properties(
                    begin_properties,
                    &begin_scan,
                    Some(generated_properties),
                )?,
            });
        } else if last_reused {
            replacement.extend_from_slice(&xml[end.start..end.start_tag_end]);
            replacement.extend_from_slice(&toc_overlay_entry_properties(
                end_properties,
                &end_scan,
                Some(generated_properties),
            )?);
        } else {
            replacement.extend_from_slice(format!("<w:p xmlns:w=\"{W_NS}\">").as_bytes());
            replacement.extend_from_slice(generated_properties);
        }
        replacement.extend_from_slice(&toc_close_fragment_namespaces(
            &entry[properties_end..entry.len() - b"</w:p>".len()],
            &generated_bindings,
        )?);
        if index + 1 < entries.len() || (!single && !last_reused) {
            replacement.extend_from_slice(
                if first_reused {
                    format!("</{}>", begin.name).into_bytes()
                } else {
                    b"</w:p>".to_vec()
                }
                .as_slice(),
            );
        }
    }
    if !single && !merge_end {
        replacement.extend_from_slice(&xml[end.start..end.content_start]);
    }
    edits.push(FieldSourceEdit {
        start: span.result_start,
        end: span.result_end,
        replacement,
    });
    if single && entries.len() > 1 {
        // The final generated entry no longer inherits the original paragraph
        // scope. Retained end content must carry that scope on every moved root.
        edits.push(FieldSourceEdit {
            start: span.result_end,
            end: end.content_end,
            replacement: toc_close_fragment_namespaces(
                &xml[span.result_end..end.content_end],
                &end.namespaces,
            )?,
        });
        edits.push(FieldSourceEdit {
            start: end.content_end,
            end: end.end,
            replacement: b"</w:p>".to_vec(),
        });
    }
    if !single && merge_begin && merge_end && entries.len() == 1 {
        // The end run and every trailing sibling move out of their original
        // namespace scope. Close each retained root, without rewriting tokens.
        edits.push(FieldSourceEdit {
            start: span.result_end,
            end: end.content_end,
            replacement: toc_close_fragment_namespaces(
                &xml[span.result_end..end.content_end],
                &end.namespaces,
            )?,
        });
        edits.push(FieldSourceEdit {
            start: end.content_end,
            end: end.end,
            replacement: format!("</{}>", begin.name).into_bytes(),
        });
    }
    edits.sort_by_key(|edit| edit.start);
    Ok(Some(edits))
}

/// Resolve a fragment in its actual paragraph scope, retaining exact byte ranges.
struct TocFragmentScan {
    namespaces: BTreeMap<String, String>,
    style: Option<(String, std::ops::Range<usize>)>,
    has_run: bool,
    blocks_replacement: bool,
    has_opaque_properties: bool,
    property_root: Option<(usize, usize, usize, usize, String)>,
    children: Vec<(Option<String>, std::ops::Range<usize>)>,
}

fn toc_fragment_scan(
    fragment: &[u8],
    bindings: &BTreeMap<String, String>,
) -> Result<TocFragmentScan> {
    let mut wrapper = b"<toc-fragment".to_vec();
    for (prefix, uri) in bindings {
        if prefix == "xml" {
            continue;
        }
        wrapper.extend_from_slice(
            if prefix.is_empty() {
                " xmlns".to_owned()
            } else {
                format!(" xmlns:{prefix}")
            }
            .as_bytes(),
        );
        wrapper.extend_from_slice(format!("=\"{}\"", xml_escape_attribute(uri)).as_bytes());
    }
    wrapper.push(b'>');
    let origin = wrapper.len();
    wrapper.extend_from_slice(fragment);
    wrapper.extend_from_slice(b"</toc-fragment>");
    let mut reader = NsReader::from_reader(wrapper.as_slice());
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut scan = TocFragmentScan {
        namespaces: bindings.clone(),
        style: None,
        has_run: false,
        blocks_replacement: false,
        has_opaque_properties: false,
        property_root: None,
        children: Vec::new(),
    };
    let mut child: Option<(Option<String>, usize)> = None;
    let mut owned = HashSet::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid TOC boundary XML: {error}")))?;
        let word = namespace_is_word(&namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let empty = matches!(event, Event::Empty(_));
                let name = element.name();
                let local = local_name(name.as_ref());
                if depth >= 1 && word && local == b"r" {
                    scan.has_run = true;
                }
                if depth == 1 && word && matches!(local, b"pPr" | b"tabs") {
                    if scan.property_root.is_some() {
                        return Err(Error::Other(
                            "duplicate owned TOC paragraph properties".to_owned(),
                        ));
                    }
                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(|error| {
                            Error::Other(format!("invalid TOC boundary namespace: {error}"))
                        })?;
                        let key = attribute.key.as_ref();
                        let prefix = if key == b"xmlns" {
                            Some(String::new())
                        } else {
                            key.strip_prefix(b"xmlns:")
                                .map(|prefix| String::from_utf8_lossy(prefix).into_owned())
                        };
                        if let Some(prefix) = prefix {
                            let value = attribute
                                .decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    element.decoder(),
                                )
                                .map_err(|error| {
                                    Error::Other(format!("invalid TOC boundary namespace: {error}"))
                                })?
                                .into_owned();
                            if value.is_empty() {
                                scan.namespaces.remove(&prefix);
                            } else {
                                scan.namespaces.insert(prefix, value);
                            }
                        }
                    }
                    let start = before - origin;
                    let opening = after - origin;
                    scan.property_root = Some((
                        start,
                        opening,
                        opening,
                        opening,
                        String::from_utf8_lossy(name.as_ref()).into_owned(),
                    ));
                    if empty {
                        scan.property_root.as_mut().unwrap().2 = start;
                    }
                }
                // A generated stop cannot erase opaque facts attached to an
                // existing stop. Leave this boundary in the generic topology.
                let inside_tab_stop = depth >= 3
                    && child
                        .as_ref()
                        .is_some_and(|(key, _)| key.as_deref() == Some("tabs"));
                if inside_tab_stop && (depth > 3 || !word || local != b"tab") {
                    scan.blocks_replacement = true;
                }
                let inside_style = child
                    .as_ref()
                    .is_some_and(|(key, _)| key.as_deref() == Some("pStyle"));
                if inside_style && depth >= 3 {
                    scan.blocks_replacement = true;
                }
                if scan.property_root.is_some() && depth >= 1 {
                    if depth >= 2 && !word {
                        scan.has_opaque_properties = true;
                    }
                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(|error| {
                            Error::Other(format!("invalid TOC property attribute: {error}"))
                        })?;
                        let key = attribute.key.as_ref();
                        let declaration = key == b"xmlns" || key.starts_with(b"xmlns:");
                        // Namespace closure on a generated Word fact is not an
                        // opaque producer declaration. Its exact binding is
                        // already required by the qualified owner itself.
                        if key == b"xmlns:w"
                            && word
                            && ((depth == 2 && name.as_ref() == b"w:pStyle")
                                || (inside_tab_stop && depth == 3 && name.as_ref() == b"w:tab"))
                            && attribute
                                .decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    element.decoder(),
                                )
                                .map_err(|error| {
                                    Error::Other(format!("invalid TOC property namespace: {error}"))
                                })?
                                == W_NS
                        {
                            continue;
                        }
                        let (attribute_namespace, attribute_local) =
                            reader.resolver().resolve_attribute(attribute.key);
                        // Only these attributes are generated entry facts. A
                        // recognized property name does not own its other data.
                        let owned_fact = !declaration
                            && word
                            && namespace_is_word(&attribute_namespace)
                            && ((depth == 2
                                && local == b"pStyle"
                                && attribute_local.as_ref() == b"val")
                                || (inside_tab_stop
                                    && depth == 3
                                    && local == b"tab"
                                    && matches!(
                                        attribute_local.as_ref(),
                                        b"val" | b"leader" | b"pos"
                                    )));
                        if !owned_fact {
                            scan.has_opaque_properties = true;
                            if inside_tab_stop || (depth == 2 && word && local == b"pStyle") {
                                // These owners are replaced or removed, including
                                // zero-entry style clearing. Keep opaque owners.
                                scan.blocks_replacement = true;
                            }
                        }
                    }
                }
                if depth == 2 && scan.property_root.is_some() {
                    let key = word.then(|| String::from_utf8_lossy(local).into_owned());
                    if word
                        && scan
                            .property_root
                            .as_ref()
                            .is_some_and(|root| !root.4.ends_with(":tabs") && root.4 != "tabs")
                        && !matches!(local, b"pStyle" | b"tabs")
                    {
                        scan.has_opaque_properties = true;
                    }
                    if let Some(ref key) = key {
                        if key != "tab" && !owned.insert(key.clone()) {
                            return Err(Error::Other(format!(
                                "duplicate owned TOC paragraph property {key}"
                            )));
                        }
                        if matches!(local, b"sectPr" | b"numPr" | b"pPrChange") {
                            scan.blocks_replacement = true;
                        }
                    }
                    let range = before - origin..after - origin;
                    if word && local == b"pStyle" {
                        let mut value = None;
                        for attribute in element.attributes() {
                            let attribute = attribute.map_err(|error| {
                                Error::Other(format!("invalid TOC property attribute: {error}"))
                            })?;
                            let (namespace, local) =
                                reader.resolver().resolve_attribute(attribute.key);
                            if namespace_is_word(&namespace) && local.as_ref() == b"val" {
                                if value.is_some() {
                                    return Err(Error::Other(
                                        "duplicate owned TOC style value".to_owned(),
                                    ));
                                }
                                value = Some(
                                    attribute
                                        .decoded_and_normalized_value(
                                            XmlVersion::Implicit1_0,
                                            element.decoder(),
                                        )
                                        .map_err(|error| {
                                            Error::Other(format!(
                                                "invalid TOC style value: {error}"
                                            ))
                                        })?
                                        .into_owned(),
                                );
                            }
                        }
                        let value = value.ok_or_else(|| {
                            Error::Other("TOC entry style has no owned value".to_owned())
                        })?;
                        scan.style = Some((value, range.clone()));
                    }
                    if empty {
                        scan.children.push((key, range));
                    } else {
                        child = Some((key, range.start));
                    }
                }
                if word && depth > 2 && matches!(local, b"ins" | b"del" | b"moveFrom" | b"moveTo") {
                    scan.blocks_replacement = true;
                }
                if !empty {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| Error::Other("unbalanced TOC boundary XML".to_owned()))?;
                if depth == 2
                    && let Some((key, start)) = child.take()
                {
                    let range = start..after - origin;
                    if key.as_deref() == Some("pStyle")
                        && let Some((_, old)) = scan.style.as_mut()
                    {
                        *old = range.clone();
                    }
                    scan.children.push((key, range));
                }
                if depth == 1
                    && let Some(root) = scan.property_root.as_mut()
                {
                    // Only the pPr root is active at depth two.
                    if root.3 == root.1 {
                        root.2 = before - origin;
                        root.3 = after - origin;
                    }
                }
            }
            Event::Comment(_)
            | Event::PI(_)
            | Event::CData(_)
            | Event::Text(_)
            | Event::GeneralRef(_) => {
                if depth >= 2
                    && scan
                        .property_root
                        .as_ref()
                        .is_some_and(|root| root.3 == root.1)
                {
                    // Ordinary XML indentation does not protect an otherwise
                    // mergeable root. All other body tokens are imported facts.
                    let opaque = match &event {
                        Event::Text(text) => !text
                            .iter()
                            .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n')),
                        _ => true,
                    };
                    scan.has_opaque_properties |= opaque;
                }
                if (depth >= 4
                    && child
                        .as_ref()
                        .is_some_and(|(key, _)| key.as_deref() == Some("tabs")))
                    || (depth >= 3
                        && child
                            .as_ref()
                            .is_some_and(|(key, _)| key.as_deref() == Some("pStyle")))
                {
                    // These tokens belong to the imported stop, not the generated
                    // entry facts. Replacing the stop would discard their bytes.
                    scan.blocks_replacement = true;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(scan)
}

/// Overlay generated style and tab facts. Retain all other source particles.
fn toc_overlay_entry_properties(
    original: &[u8],
    scan: &TocFragmentScan,
    generated: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let canonical = BTreeMap::from([("w".to_owned(), W_NS.to_owned())]);
    let mut replacements = Vec::new();
    if let Some(generated) = generated {
        let new_scan = toc_fragment_scan(generated, &canonical)?;
        for (name, range) in new_scan.children {
            let name = name.unwrap();
            let replacement = if name == "pStyle"
                && let Some((old_id, old_range)) = &scan.style
            {
                let generated_scan = toc_fragment_scan(generated, &canonical)?;
                let new_id = &generated_scan
                    .style
                    .as_ref()
                    .ok_or_else(|| Error::Other("generated TOC style is absent".to_owned()))?
                    .0;
                if old_id == new_id {
                    original[old_range.clone()].to_vec()
                } else {
                    toc_retarget_style(&original[old_range.clone()], new_id, &scan.namespaces)?
                }
            } else {
                toc_close_fragment_namespaces(&generated[range], &canonical)?
            };
            replacements.push((name, replacement));
        }
    }
    let Some((start, opening_end, closing_start, end, ref name)) = scan.property_root else {
        return generated.map_or_else(
            || Ok(original.to_vec()),
            |properties| toc_close_fragment_namespaces(properties, &canonical),
        );
    };
    let mut output = original[..start].to_vec();
    let empty = opening_end == end && closing_start == start;
    if empty {
        output.extend_from_slice(&original[start..opening_end - 2]);
        output.push(b'>');
    } else {
        output.extend_from_slice(&original[start..opening_end]);
    }
    // pStyle comes first in CT_PPr. Insert it before retaining source particles.
    if let Some((_, replacement)) = replacements.iter().find(|(key, _)| key == "pStyle") {
        output.extend_from_slice(replacement);
    }
    let mut cursor = opening_end;
    let mut tabs_written = false;
    for (key, range) in &scan.children {
        output.extend_from_slice(&original[cursor..range.start]);
        if key.as_deref() == Some("pStyle") {
            // Remove the complete old style, including a started element's end.
        } else if key.as_deref() == Some("tabs") {
            if let Some((_, replacement)) = replacements.iter().find(|(key, _)| key == "tabs") {
                output.extend_from_slice(&toc_overlay_tab_particles(
                    &original[range.clone()],
                    replacement,
                    &scan.namespaces,
                )?);
            } else {
                output.extend_from_slice(&original[range.clone()]);
            }
            tabs_written = true;
        } else {
            // Tabs precede spacing and later CT_PPr properties. Unknown particles
            // keep their original order and exact source bytes.
            if !tabs_written
                && matches!(
                    key.as_deref(),
                    Some(
                        "suppressAutoHyphens"
                            | "kinsoku"
                            | "wordWrap"
                            | "overflowPunct"
                            | "topLinePunct"
                            | "autoSpaceDE"
                            | "autoSpaceDN"
                            | "bidi"
                            | "adjustRightInd"
                            | "snapToGrid"
                            | "spacing"
                            | "ind"
                            | "contextualSpacing"
                            | "mirrorIndents"
                            | "suppressOverlap"
                            | "jc"
                            | "textDirection"
                            | "textAlignment"
                            | "textboxTightWrap"
                            | "outlineLvl"
                            | "divId"
                            | "cnfStyle"
                            | "rPr"
                            | "sectPr"
                            | "pPrChange"
                    )
                )
            {
                if let Some((_, replacement)) = replacements.iter().find(|(key, _)| key == "tabs") {
                    output.extend_from_slice(replacement);
                }
                tabs_written = true;
            }
            output.extend_from_slice(&original[range.clone()]);
        }
        cursor = range.end;
    }
    if !tabs_written
        && let Some((_, replacement)) = replacements.iter().find(|(key, _)| key == "tabs")
    {
        output.extend_from_slice(replacement);
    }
    if !empty {
        output.extend_from_slice(&original[cursor..closing_start]);
    }
    if empty {
        output.extend_from_slice(format!("</{name}>").as_bytes());
    } else {
        output.extend_from_slice(&original[closing_start..end]);
    }
    output.extend_from_slice(&original[end..]);
    Ok(output)
}

/// Change only the qualified style value, retaining opaque attributes and body.
fn toc_retarget_style(
    original: &[u8],
    value: &str,
    bindings: &BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let closed = xml_fragment_with_namespaces(original, bindings, "TOC style")?;
    let mut reader = NsReader::from_reader(closed.as_slice());
    let mut buffer = Vec::new();
    let (_, event) = reader
        .read_resolved_event_into(&mut buffer)
        .map_err(|error| Error::Other(format!("invalid TOC style: {error}")))?;
    let element = match event {
        Event::Start(element) | Event::Empty(element) => element,
        _ => return Err(Error::Other("TOC style has no root".to_owned())),
    };
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("invalid TOC style attribute: {error}")))?;
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if namespace_is_word(&namespace) && local.as_ref() == b"val" {
            let (_, start, end) = attribute_source_span(original, attribute.key.as_ref())
                .ok_or_else(|| Error::Other("TOC style source value is unavailable".to_owned()))?;
            let mut output = original[..start].to_vec();
            output.extend_from_slice(xml_escape_attribute(value).as_bytes());
            output.extend_from_slice(&original[end..]);
            return Ok(output);
        }
    }
    Err(Error::Other("TOC style has no qualified value".to_owned()))
}

/// Replace owned tab stops while keeping the source tabs root and opaque particles.
fn toc_overlay_tab_particles(
    original: &[u8],
    generated: &[u8],
    bindings: &BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let old = toc_fragment_scan(original, bindings)?;
    let canonical = BTreeMap::from([("w".to_owned(), W_NS.to_owned())]);
    let new = toc_fragment_scan(generated, &canonical)?;
    let (start, opening, closing, end, name) = old
        .property_root
        .as_ref()
        .ok_or_else(|| Error::Other("TOC tabs have no owned root".to_owned()))?;
    let (new_start, new_opening, new_closing, _, _) = new
        .property_root
        .as_ref()
        .ok_or_else(|| Error::Other("generated TOC tabs have no owned root".to_owned()))?;
    let _ = (start, new_start);
    let replacement =
        toc_close_fragment_namespaces(&generated[*new_opening..*new_closing], &canonical)?;
    let empty = *opening == *end && *closing == *start;
    let mut output = Vec::new();
    if empty {
        output.extend_from_slice(&original[..*opening - 2]);
        output.push(b'>');
    } else {
        output.extend_from_slice(&original[..*opening]);
    }
    let mut cursor = *opening;
    let mut written = false;
    for (key, range) in old.children {
        output.extend_from_slice(&original[cursor..range.start]);
        if key.as_deref() == Some("tab") {
            if !written {
                output.extend_from_slice(&replacement);
                written = true;
            }
        } else {
            output.extend_from_slice(&original[range.clone()]);
        }
        cursor = range.end;
    }
    if !written {
        output.extend_from_slice(&replacement);
    }
    if !empty {
        output.extend_from_slice(&original[cursor..*closing]);
        output.extend_from_slice(&original[*closing..*end]);
    } else {
        output.extend_from_slice(format!("</{name}>").as_bytes());
    }
    output.extend_from_slice(&original[*end..]);
    Ok(output)
}

/// Close every top-level fragment root using its original inherited namespace map.
fn toc_close_fragment_namespaces(
    fragment: &[u8],
    bindings: &BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let mut reader = quick_xml::Reader::from_reader(fragment);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut cursor = 0usize;
    let mut root = 0usize;
    let mut output = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid retained TOC fragment: {error}")))?
        {
            Event::Start(_) => {
                if depth == 0 {
                    root = before;
                }
                depth += 1;
            }
            Event::Empty(_) if depth == 0 => {
                let end = reader.buffer_position() as usize;
                output.extend_from_slice(&fragment[cursor..before]);
                output.extend_from_slice(&xml_fragment_with_namespaces(
                    &fragment[before..end],
                    bindings,
                    "TOC fragment",
                )?);
                cursor = end;
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| Error::Other("unbalanced retained TOC fragment".to_owned()))?;
                if depth == 0 {
                    let end = reader.buffer_position() as usize;
                    output.extend_from_slice(&fragment[cursor..root]);
                    output.extend_from_slice(&xml_fragment_with_namespaces(
                        &fragment[root..end],
                        bindings,
                        "TOC fragment",
                    )?);
                    cursor = end;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if depth != 0 {
        return Err(Error::Other("unclosed retained TOC fragment".to_owned()));
    }
    output.extend_from_slice(&fragment[cursor..]);
    Ok(output)
}

/// Every style identifier a TOC entry of any level may carry: the producer
/// styles named `toc 1` to `toc 9`, the canonical `TOC1` to `TOC9`, and the
/// styles this rebuild resolved.
fn toc_entry_style_ids(
    styles: &CT_Styles,
    resolved: &BTreeMap<u8, TocEntryStyle>,
) -> BTreeSet<String> {
    let mut ids = (1..=9)
        .map(|level| format!("TOC{level}"))
        .collect::<BTreeSet<_>>();
    for style in &styles.styles {
        let is_entry_name = style.name.as_deref().is_some_and(|name| {
            (1..=9).any(|level| name.trim().eq_ignore_ascii_case(&format!("toc {level}")))
        });
        if style.style_type == StyleType::Paragraph && !style.style_id.is_empty() && is_entry_name {
            ids.insert(style.style_id.clone());
        }
    }
    ids.extend(resolved.values().map(|style| style.style_id.clone()));
    ids
}

fn relocate_end_boundary_bookmark_starts(
    mut xml: Vec<u8>,
    spans: &[DynamicTocSpan],
    marker_starts: &[Vec<u8>],
) -> Result<Vec<u8>> {
    let mut edits = Vec::new();
    for (toc_index, span) in spans.iter().enumerate() {
        let markers = marker_starts.get(toc_index).ok_or_else(|| {
            Error::Other("table of contents end bookmark owner was not retained".to_owned())
        })?;
        if markers.is_empty() {
            continue;
        }
        let search_end = span
            .end_wrapper_prefixes
            .first()
            .map(|(start, _)| *start)
            .unwrap_or(span.result_end);
        if span.end_paragraph_content_start > search_end || search_end > span.end_run_end {
            return Err(Error::Other(
                "table of contents end bookmark boundaries are invalid".to_owned(),
            ));
        }
        let matches =
            byte_match_offsets(&xml[span.end_paragraph_content_start..search_end], markers);
        if matches.len() != 1 {
            return Err(Error::Other(
                "table of contents end bookmark start was not uniquely staged".to_owned(),
            ));
        }
        let start = span.end_paragraph_content_start + matches[0];
        let end = start + markers.len();
        if end > span.end_run_end {
            return Err(Error::Other(
                "table of contents end bookmark start crosses its field boundary".to_owned(),
            ));
        }
        let mut replacement = xml[end..span.end_run_end].to_vec();
        replacement.extend_from_slice(markers);
        edits.push(FieldSourceEdit {
            start,
            end: span.end_run_end,
            replacement,
        });
    }
    edits.sort_by_key(|edit| edit.start);
    if edits.windows(2).any(|pair| pair[0].end > pair[1].start) {
        return Err(Error::Other(
            "table of contents end bookmark repairs overlap".to_owned(),
        ));
    }
    for edit in edits.into_iter().rev() {
        xml.splice(edit.start..edit.end, edit.replacement);
    }
    Ok(xml)
}

fn append_toc_wrapper_closures(output: &mut Vec<u8>, wrappers: &[String]) {
    for name in wrappers.iter().rev() {
        output.extend_from_slice(b"</");
        output.extend_from_slice(name.as_bytes());
        output.push(b'>');
    }
}

fn reopen_staged_document(document: Document) -> Result<Document> {
    document.reopen_prepared_staged()
}

fn deterministic_toc_page_values(document: &Document) -> Result<HashMap<String, String>> {
    let layout = document.layout_deterministic()?;
    let section_formats = section_page_formats(&document.document);
    let input = document.build_layout_input()?;
    let snapshot = oxoffice_docx_layout::engine::evaluate_sequence_fields(&input)?;
    let mut output = HashMap::new();
    let mut invalid_target = None;
    for page in &layout.layout.pages {
        oxoffice_layout::walk(&page.elements, &mut |element, _| {
            if let oxoffice_layout::PositionedElement::Text(run) = element
                && let Some(oxoffice_layout::FieldKind::TargetPage(target)) = run.field_kind
                && let Some(name) = layout.page_reference_name(target)
            {
                let owner = layout.bookmark_page_section(name);
                let instruction = run
                    .field_source
                    .and_then(|source| {
                        layout
                            .source_node(source.node)
                            .and_then(|path| snapshot.source_id(path))
                            .map(|node| oxoffice_layout::FieldSource {
                                node,
                                index: source.index,
                            })
                    })
                    .and_then(|source| snapshot.source_field_context(source))
                    .map(|(raw, _, _)| Field::new(raw, "").effective_instruction())
                    .filter(|instruction| instruction.name == "PAGEREF");
                let valid_page = owner
                    .filter(|owner| (1..=layout.layout.pages.len()).contains(&owner.physical_page))
                    .zip(instruction.as_ref())
                    .is_some_and(|(owner, instruction)| {
                        oxoffice_docx_layout::engine::format_page_field_result(
                            instruction,
                            owner.displayed_page,
                            section_formats
                                .get(owner.section_index)
                                .and_then(Option::as_deref),
                        )
                        .is_ok_and(|expected| expected == run.text)
                    });
                if !valid_page
                    || output
                        .get(name)
                        .is_some_and(|existing| existing != &run.text)
                {
                    invalid_target.get_or_insert_with(|| name.to_owned());
                } else {
                    output
                        .entry(name.to_owned())
                        .or_insert_with(|| run.text.clone());
                }
            }
        });
    }
    if let Some(target) = invalid_target {
        return Err(Error::Other(format!(
            "table of contents page target {target} was not resolved"
        )));
    }
    Ok(output)
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

/// Preflight before copying a current Field. The boolean identifies unchanged
/// imported facts whose established raw preservation bypasses semantic editing.
pub(crate) fn preflight_current_field(field: &Field) -> Result<bool> {
    if let Err(error) = field.validate_for_attachment() {
        if field
            .source_replacement()?
            .is_some_and(|(original, current)| original == current)
        {
            return Ok(true);
        }
        return Err(error.into());
    }
    Ok(false)
}

/// Freeze one complete current Field tree for this read operation. Recursive
/// consumers borrow its already-normalized descendants instead of restaging each
/// subtree. Only required retained result projections get separate owned CT_Ps.
pub(crate) fn prepared_current_field(field: &Field) -> Result<Field> {
    fn normalize(field: &mut Field) -> Result<()> {
        for argument in field.instruction.arguments.iter_mut().chain(
            field
                .instruction
                .switches
                .iter_mut()
                .filter_map(|switch| switch.argument.as_mut()),
        ) {
            if let FieldArgument::Nested(child) = argument {
                normalize(child)?;
            }
        }
        let count = field.cached_fields_in_source_order().len();
        for index in 0..count {
            let child = field.cached_field_mut(index).ok_or_else(|| {
                Error::Other("current field preparation lost its actual cached slot".into())
            })?;
            normalize(child)?;
        }
        field.normalize_cached_result_owner()?;
        Ok(())
    }
    if preflight_current_field(field)? {
        return Ok(field.clone());
    }
    let mut current = field.clone();
    normalize(&mut current)?;
    Ok(current)
}

fn collect_ref_cached_note_references(
    field: &Field,
    output: &mut Vec<(StoryKind, i32)>,
) -> Result<()> {
    fn collect(field: &Field, output: &mut Vec<(StoryKind, i32)>) -> Result<()> {
        let prepared = field.cached_result_owner_projection()?;
        let owned_runs = prepared.as_ref().map(|paragraph| {
            paragraph
                .accepted_run_paths()
                .into_iter()
                .filter_map(|path| paragraph.accepted_run(&path))
                .collect::<Vec<_>>()
        });
        let runs = owned_runs
            .as_deref()
            .map(|runs| runs.to_vec())
            .or_else(|| field.cached_result_runs().map(|runs| runs.iter().collect()));
        if let Some(runs) = runs {
            for content in runs.into_iter().flat_map(|run| &run.content) {
                match content {
                    RunContent::FootnoteRef { id, .. } => output.push((StoryKind::Footnote, *id)),
                    RunContent::EndnoteRef { id, .. } => output.push((StoryKind::Endnote, *id)),
                    RunContent::Field(field) => collect(field, output)?,
                    _ => {}
                }
            }
        }
        Ok(())
    }
    let current = prepared_current_field(field)?;
    collect(&current, output)
}

fn ref_note_reference_counts(package: &OpcPackage) -> Result<HashMap<(StoryKind, i32), usize>> {
    let mut counts = HashMap::new();
    for (name, xml) in &package.parts {
        if !name.ends_with(".xml")
            || ![
                b"footnoteReference".as_slice(),
                b"endnoteReference".as_slice(),
            ]
            .iter()
            .any(|needle| xml.windows(needle.len()).any(|window| window == *needle))
        {
            continue;
        }
        let mut reader = NsReader::from_reader(xml.as_slice());
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer).map_err(|error| {
                Error::Other(format!(
                    "REF f source reference graph is malformed: {error}"
                ))
            })? {
                Event::Start(element) | Event::Empty(element) => {
                    let (namespace, local) = reader.resolver().resolve_element(element.name());
                    if namespace_is_word(&namespace) {
                        let kind = match local.as_ref() {
                            b"footnoteReference" => Some(StoryKind::Footnote),
                            b"endnoteReference" => Some(StoryKind::Endnote),
                            _ => None,
                        };
                        if let Some(kind) = kind {
                            let (_, id) = resolved_element_attribute(
                                &element,
                                reader.resolver(),
                                b"id",
                                AttributeNamespace::Word,
                            )?
                            .ok_or_else(|| {
                                Error::Other("REF f source reference has no ID".into())
                            })?;
                            let id = id.parse::<i32>().map_err(|_| {
                                Error::Other("REF f source reference ID is invalid".into())
                            })?;
                            *counts.entry((kind, id)).or_insert(0) += 1;
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buffer.clear();
        }
    }
    Ok(counts)
}

#[derive(Clone, Copy)]
enum RefCommentCompanionKind {
    Extended,
    Ids,
}

impl RefCommentCompanionKind {
    fn namespace(self) -> &'static str {
        match self {
            Self::Extended => oxoffice_docx_oxml::comments_extended::W15_NS,
            Self::Ids => "http://schemas.microsoft.com/office/word/2016/wordml/cid",
        }
    }

    fn root(self) -> &'static [u8] {
        match self {
            Self::Extended => b"commentsEx",
            Self::Ids => b"commentsIds",
        }
    }

    fn item(self) -> &'static [u8] {
        match self {
            Self::Extended => b"commentEx",
            Self::Ids => b"commentId",
        }
    }

    fn relationship(self) -> &'static str {
        match self {
            Self::Extended => crate::comments::COMMENTS_EXTENDED_REL_TYPE,
            Self::Ids => "http://schemas.microsoft.com/office/2016/09/relationships/commentsIds",
        }
    }
}

struct RefCommentCompanionEntry {
    para_id: String,
    parent: Option<String>,
    durable_id: Option<String>,
    span: std::ops::Range<usize>,
    xml: Vec<u8>,
}

fn ref_comment_companion_entries(
    xml: &[u8],
    kind: RefCommentCompanionKind,
) -> Result<(Vec<RefCommentCompanionEntry>, usize)> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut saw_root = false;
    let mut root_end = None;
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut durable_seen = HashSet::new();
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!(
                "REF f annotation companion XML is malformed: {error}"
            ))
        })?;
        let end = reader.buffer_position() as usize;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let empty = xml.get(end.saturating_sub(2)..end) == Some(b"/>");
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                let owned = namespace_matches(&namespace, kind.namespace());
                if depth == 0 {
                    if !owned || local.as_ref() != kind.root() || saw_root || empty {
                        return Err(Error::Other(
                            "REF f annotation companion has no unique qualified root".into(),
                        ));
                    }
                    saw_root = true;
                } else if depth == 1 && owned && local.as_ref() == kind.item() {
                    let attribute = |name| {
                        resolved_element_attribute(
                            &element,
                            reader.resolver(),
                            name,
                            AttributeNamespace::Bound(kind.namespace()),
                        )
                        .map(|value| value.map(|(_, value)| value))
                    };
                    let para_id = attribute(b"paraId")?.ok_or_else(|| {
                        Error::Other("REF f annotation companion entry has no paragraph ID".into())
                    })?;
                    if para_id.len() != 8
                        || u32::from_str_radix(&para_id, 16).is_err()
                        || !seen.insert(para_id.to_ascii_uppercase())
                    {
                        return Err(Error::Other(
                            "REF f annotation companion paragraph ID is invalid or ambiguous"
                                .into(),
                        ));
                    }
                    let parent = attribute(b"paraIdParent")?;
                    let durable_id = attribute(b"durableId")?;
                    if matches!(kind, RefCommentCompanionKind::Ids)
                        && !durable_id.as_ref().is_some_and(|id| {
                            id.len() == 8
                                && u32::from_str_radix(id, 16).is_ok()
                                && durable_seen.insert(id.to_ascii_uppercase())
                        })
                    {
                        return Err(Error::Other(
                            "REF f annotation companion durable ID is absent or invalid".into(),
                        ));
                    }
                    if !empty {
                        reader
                            .read_to_end_into(element.name(), &mut Vec::new())
                            .map_err(|error| {
                                Error::Other(format!(
                                    "REF f annotation companion entry is unclosed: {error}"
                                ))
                            })?;
                    }
                    let span = start..reader.buffer_position() as usize;
                    let scope = crate::document::story_namespace_scope_at(xml, start)?;
                    entries.push(RefCommentCompanionEntry {
                        para_id,
                        parent,
                        durable_id,
                        xml: crate::document::close_content_fragment_namespaces(
                            &xml[span.clone()],
                            &scope,
                        )?,
                        span,
                    });
                    buffer.clear();
                    continue;
                }
                if !empty {
                    depth += 1;
                }
            }
            Event::End(element) => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    Error::Other("REF f annotation companion XML is unbalanced".into())
                })?;
                if depth == 0 {
                    let (namespace, local) = reader.resolver().resolve_element(element.name());
                    if !namespace_matches(&namespace, kind.namespace())
                        || local.as_ref() != kind.root()
                        || root_end.replace(start).is_some()
                    {
                        return Err(Error::Other(
                            "REF f annotation companion root is ambiguous".into(),
                        ));
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if depth != 0 || !saw_root {
        return Err(Error::Other(
            "REF f annotation companion XML is unclosed".into(),
        ));
    }
    Ok((
        entries,
        root_end
            .ok_or_else(|| Error::Other("REF f annotation companion root end is missing".into()))?,
    ))
}

fn ref_comment_paragraph_ids(xml: &[u8]) -> Result<Vec<String>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    loop {
        match reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!("REF f annotation payload is malformed: {error}"))
        })? {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                if namespace_is_word(&namespace)
                    && matches!(
                        local.as_ref(),
                        b"commentReference" | b"commentRangeStart" | b"commentRangeEnd"
                    )
                {
                    return Err(Error::Other("REF f annotation payload contains an unqualified recursive annotation graph".into()));
                }
                if namespace_is_word(&namespace) && local.as_ref() == b"p" {
                    let (_, id) = resolved_element_attribute(&element, reader.resolver(), b"paraId", AttributeNamespace::Bound(W14_NS))?
                        .ok_or_else(|| Error::Other("REF f annotation paragraph lacks qualified identity for companion closure".into()))?;
                    if id.len() != 8
                        || u32::from_str_radix(&id, 16).is_err()
                        || !seen.insert(id.to_ascii_uppercase())
                    {
                        return Err(Error::Other(
                            "REF f annotation paragraph identity is invalid or duplicated".into(),
                        ));
                    }
                    ids.push(id);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(ids)
}

fn ref_comment_paragraph_identity_counts(xml: &[u8]) -> Result<HashMap<String, usize>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut counts = HashMap::new();
    loop {
        match reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!(
                "REF f annotation owner identity inventory is malformed: {error}"
            ))
        })? {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                if namespace_is_word(&namespace)
                    && local.as_ref() == b"p"
                    && let Some((_, id)) = resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"paraId",
                        AttributeNamespace::Bound(W14_NS),
                    )?
                {
                    *counts.entry(id.to_ascii_uppercase()).or_insert(0) += 1;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(counts)
}

fn ref_comment_companion_part(
    document: &Document,
    kind: RefCommentCompanionKind,
) -> Result<Option<String>> {
    let relationships = document.package.get_part_rels(&document.doc_part_name);
    let mut matches = relationships
        .into_iter()
        .flat_map(|relationships| &relationships.items)
        .filter(|relationship| relationship.rel_type == kind.relationship());
    let first = matches.next();
    if matches.next().is_some()
        || first
            .is_some_and(|relationship| !crate::document::relationship_is_internal(relationship))
    {
        return Err(Error::Other(
            "REF f annotation companion relationship owner is ambiguous or external".into(),
        ));
    }
    first
        .map(|relationship| {
            let part =
                OpcPackage::resolve_rel_target(&document.doc_part_name, &relationship.target);
            if document.package.get_part(&part).is_none() {
                return Err(Error::Other(
                    "REF f annotation companion relationship target is missing".into(),
                ));
            }
            Ok(part)
        })
        .transpose()
}

struct RefCommentCompanionCopy {
    kind: RefCommentCompanionKind,
    part: String,
    xml: Vec<u8>,
    replace_para_id: Option<String>,
}

struct RefCommentCopy {
    part: String,
    xml: Vec<u8>,
    replace_id: Option<i32>,
    companions: Vec<RefCommentCompanionCopy>,
}

fn ref_comment_occupied_identity_values(package: &OpcPackage) -> Result<HashSet<u32>> {
    let mut occupied = HashSet::new();
    for (part, xml) in &package.parts {
        if !part.ends_with(".xml") {
            continue;
        }
        let mut reader = NsReader::from_reader(xml.as_slice());
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer).map_err(|error| {
                Error::Other(format!(
                    "REF f annotation identity inventory is malformed: {error}"
                ))
            })? {
                Event::Start(element) | Event::Empty(element) => {
                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(|error| {
                            Error::Other(format!(
                                "REF f annotation identity attribute is malformed: {error}"
                            ))
                        })?;
                        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                        let namespace_owned = namespace_matches(&namespace, W14_NS)
                            || namespace_matches(
                                &namespace,
                                RefCommentCompanionKind::Extended.namespace(),
                            )
                            || namespace_matches(
                                &namespace,
                                RefCommentCompanionKind::Ids.namespace(),
                            );
                        if namespace_owned
                            && matches!(local.as_ref(), b"paraId" | b"paraIdParent" | b"durableId")
                        {
                            let value = attribute
                                .decoded_and_normalized_value(
                                    quick_xml::XmlVersion::Implicit1_0,
                                    element.decoder(),
                                )
                                .map_err(|error| {
                                    Error::Other(format!(
                                        "REF f annotation identity value is malformed: {error}"
                                    ))
                                })?;
                            if value.len() != 8 {
                                return Err(Error::Other("REF f annotation identity is not an eight-digit hexadecimal value".into()));
                            }
                            occupied.insert(u32::from_str_radix(&value, 16).map_err(|_| {
                                Error::Other("REF f annotation identity is invalid".into())
                            })?);
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buffer.clear();
        }
    }
    Ok(occupied)
}

pub(crate) fn patch_ref_comment_identity_attributes(
    xml: &[u8],
    comment_ids: &BTreeMap<String, String>,
    paragraph_ids: &BTreeMap<String, String>,
    durable_ids: &BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut edits = Vec::new();
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!(
                "REF f annotation identity patch XML is malformed: {error}"
            ))
        })?;
        let end = reader.buffer_position() as usize;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                if namespace_is_word(&namespace) && local.as_ref() == b"comment" {
                    add_identity_attribute_edit(
                        xml,
                        start,
                        end,
                        &element,
                        reader.resolver(),
                        b"id",
                        AttributeNamespace::Word,
                        comment_ids,
                        &mut edits,
                    )?;
                }
                if namespace_is_word(&namespace) && local.as_ref() == b"p" {
                    add_identity_attribute_edit(
                        xml,
                        start,
                        end,
                        &element,
                        reader.resolver(),
                        b"paraId",
                        AttributeNamespace::Bound(W14_NS),
                        paragraph_ids,
                        &mut edits,
                    )?;
                }
                for kind in [
                    RefCommentCompanionKind::Extended,
                    RefCommentCompanionKind::Ids,
                ] {
                    if namespace_matches(&namespace, kind.namespace())
                        && local.as_ref() == kind.item()
                    {
                        add_identity_attribute_edit(
                            xml,
                            start,
                            end,
                            &element,
                            reader.resolver(),
                            b"paraId",
                            AttributeNamespace::Bound(kind.namespace()),
                            paragraph_ids,
                            &mut edits,
                        )?;
                        if matches!(kind, RefCommentCompanionKind::Extended) {
                            add_identity_attribute_edit(
                                xml,
                                start,
                                end,
                                &element,
                                reader.resolver(),
                                b"paraIdParent",
                                AttributeNamespace::Bound(kind.namespace()),
                                paragraph_ids,
                                &mut edits,
                            )?;
                        }
                        add_identity_attribute_edit(
                            xml,
                            start,
                            end,
                            &element,
                            reader.resolver(),
                            b"durableId",
                            AttributeNamespace::Bound(kind.namespace()),
                            durable_ids,
                            &mut edits,
                        )?;
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let mut result = xml.to_vec();
    for edit in edits.into_iter().rev() {
        result.splice(edit.start..edit.end, edit.replacement);
    }
    Ok(result)
}

fn prepare_ref_comment_copy(
    document: &mut Document,
    original: &Document,
    source_id: i32,
    replace_id: Option<i32>,
    occupied_comments: &mut HashSet<i32>,
    occupied_identities: &mut HashSet<u32>,
) -> Result<(i32, RefCommentCopy)> {
    let (part, source) = original.fragment_comment_dependency(source_id)?;
    if document.comments_part_name.as_deref() != Some(&part)
        || !fragment_note_references(&source)?.is_empty()
    {
        return Err(Error::Other(
            "REF f annotation has an unqualified relationship owner or recursive note payload"
                .into(),
        ));
    }
    let paragraph_ids = ref_comment_paragraph_ids(&source)?;
    let paragraph_owners = ref_comment_paragraph_identity_counts(
        original
            .package
            .get_part(&part)
            .ok_or_else(|| Error::Other("REF f annotation owner part is absent".into()))?,
    )?;
    if paragraph_ids
        .iter()
        .any(|id| paragraph_owners.get(&id.to_ascii_uppercase()) != Some(&1))
    {
        return Err(Error::Other(
            "REF f annotation paragraph identity is shared with another physical owner".into(),
        ));
    }
    let source_last = paragraph_ids.last().ok_or_else(|| {
        Error::Other("REF f annotation source has no qualified last paragraph identity".into())
    })?;
    let old_last = replace_id
        .map(|id| {
            original
                .fragment_comment_dependency(id)
                .and_then(|(old_part, xml)| {
                    if old_part != part || id == source_id {
                        return Err(Error::Other(
                            "REF f cached annotation is borrowed or differently owned".into(),
                        ));
                    }
                    let old_paragraphs = ref_comment_paragraph_ids(&xml)?;
                    if old_paragraphs.iter().any(|id|paragraph_owners.get(&id.to_ascii_uppercase())!=Some(&1)) {
                        return Err(Error::Other("REF f cached annotation paragraph identity is shared with another physical owner".into()));
                    }
                    old_paragraphs
                        .last()
                        .cloned()
                        .ok_or_else(|| {
                            Error::Other(
                                "REF f cached annotation has no qualified last paragraph identity"
                                    .into(),
                            )
                        })
                })
        })
        .transpose()?;
    let mut para_map = BTreeMap::new();
    for old in &paragraph_ids {
        para_map.insert(
            old.clone(),
            crate::comments::allocate_para_id_from_occupied(occupied_identities)?,
        );
    }
    let id = if let Some(id) = replace_id {
        if !occupied_comments.contains(&id) {
            return Err(Error::Other(
                "REF f cached annotation owner is absent".into(),
            ));
        }
        id
    } else {
        let mut id = 0i32;
        while occupied_comments.contains(&id) {
            id = id
                .checked_add(1)
                .ok_or_else(|| Error::Other("REF f annotation IDs are exhausted".into()))?;
        }
        occupied_comments.insert(id);
        id
    };
    let mut companions = Vec::new();
    for kind in [
        RefCommentCompanionKind::Extended,
        RefCommentCompanionKind::Ids,
    ] {
        let Some(companion_part) = ref_comment_companion_part(original, kind)? else {
            continue;
        };
        let xml = original.package.get_part(&companion_part).unwrap();
        let (entries, _) = ref_comment_companion_entries(xml, kind)?;
        let selected = entries
            .iter()
            .filter(|entry| entry.para_id.eq_ignore_ascii_case(source_last))
            .collect::<Vec<_>>();
        if selected.len() != 1
            || selected[0].parent.is_some()
            || entries.iter().any(|entry| {
                entry
                    .parent
                    .as_ref()
                    .is_some_and(|parent| parent.eq_ignore_ascii_case(source_last))
            })
        {
            return Err(Error::Other("REF f annotation companion source is missing, ambiguous or part of an unqualified reply graph".into()));
        }
        let replace_para_id = if let Some(old_last) = &old_last {
            let old = entries
                .iter()
                .filter(|entry| entry.para_id.eq_ignore_ascii_case(old_last))
                .collect::<Vec<_>>();
            if old.len() != 1
                || old[0].parent.is_some()
                || entries.iter().any(|entry| {
                    entry
                        .parent
                        .as_ref()
                        .is_some_and(|parent| parent.eq_ignore_ascii_case(old_last))
                })
            {
                return Err(Error::Other(
                    "REF f cached annotation companion closure is ambiguous".into(),
                ));
            }
            Some(old[0].para_id.clone())
        } else {
            None
        };
        let mut durable_map = BTreeMap::new();
        if let Some(old) = &selected[0].durable_id {
            durable_map.insert(
                old.clone(),
                crate::comments::allocate_para_id_from_occupied(occupied_identities)?,
            );
        }
        let mut companion_para_map = para_map.clone();
        // Companion producers may use a different hexadecimal spelling of the same ID.
        companion_para_map.insert(selected[0].para_id.clone(), para_map[source_last].clone());
        companions.push(RefCommentCompanionCopy {
            kind,
            part: companion_part,
            xml: patch_ref_comment_identity_attributes(
                &selected[0].xml,
                &BTreeMap::new(),
                &companion_para_map,
                &durable_map,
            )?,
            replace_para_id,
        });
    }
    for relationship_id in relationship_ids_in_xml(&source)? {
        let relationship = original
            .package
            .get_part_rels(&part)
            .and_then(|relationships| relationships.get_by_id(&relationship_id))
            .ok_or_else(|| {
                Error::Other("REF f annotation payload relationship is absent".into())
            })?;
        if !crate::document::relationship_is_internal(relationship) {
            return Err(Error::Other(
                "REF f annotation external relationship payload lacks authenticated controls"
                    .into(),
            ));
        }
        discover_fragment_part_closure(
            &original.package,
            &OpcPackage::resolve_rel_target(&part, &relationship.target),
            &mut HashSet::new(),
        )?;
    }
    let mut xml = patch_ref_comment_identity_attributes(
        &source,
        &BTreeMap::from([(source_id.to_string(), id.to_string())]),
        &para_map,
        &BTreeMap::new(),
    )?;
    let wrapper = format!("<w:body xmlns:w=\"{W_NS}\">");
    let mut body = wrapper.as_bytes().to_vec();
    body.extend_from_slice(&xml);
    body.extend_from_slice(b"</w:body>");
    let mut remap = BodyIdentityRemap::default();
    for old in body_identity_values(&body)?.drawing_ids {
        if remap
            .drawing_ids
            .insert(old, document.identifiers.reserve_drawing_id()?.to_string())
            .is_some()
        {
            return Err(Error::Other(
                "REF f annotation source has duplicate drawing identities".into(),
            ));
        }
    }
    let mapped = patch_body_identity_attributes(&body, &remap)?;
    xml = mapped[wrapper.len()..mapped.len() - b"</w:body>".len()].to_vec();
    Ok((
        id,
        RefCommentCopy {
            part,
            xml,
            replace_id,
            companions,
        },
    ))
}

fn publish_ref_comment_copy(document: &mut Document, copy: RefCommentCopy) -> Result<()> {
    document.publish_fragment_comment_staged(&copy.part, copy.xml, copy.replace_id)?;
    for companion in copy.companions {
        let source = document.package.get_part(&companion.part).ok_or_else(|| {
            Error::Other("REF f annotation companion publication target is missing".into())
        })?;
        let (entries, end) = ref_comment_companion_entries(source, companion.kind)?;
        let mut updated = source.to_vec();
        if let Some(id) = companion.replace_para_id {
            let entries = entries
                .iter()
                .filter(|entry| entry.para_id == id)
                .collect::<Vec<_>>();
            if entries.len() != 1 {
                return Err(Error::Other(
                    "REF f cached annotation companion publication owner is ambiguous".into(),
                ));
            }
            updated.splice(entries[0].span.clone(), companion.xml);
        } else {
            updated.splice(end..end, companion.xml);
        }
        ref_comment_companion_entries(&updated, companion.kind)?;
        if matches!(companion.kind, RefCommentCompanionKind::Extended) {
            document.comments_extended =
                Some(oxoffice_docx_oxml::comments_extended::CT_CommentsEx::from_xml(&updated)?);
        }
        document.package.set_part(&companion.part, updated);
    }
    Ok(())
}

fn ref_comment_reference_ids(cache: &RefBookmarkCache) -> Result<Vec<i32>> {
    let ids = cache
        .runs
        .iter()
        .flat_map(|run| &run.content)
        .filter_map(|content| match content {
            RunContent::CommentReference { id, .. } => Some(*id),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut unique = HashSet::new();
    if cache.comment_ranges.len() != ids.len() * 2 {
        return Err(Error::Other(
            "REF f annotation target lacks closed owned range endpoints".into(),
        ));
    }
    for id in &ids {
        let starts = cache.comment_ranges.iter().filter(|marker| matches!(marker,CommentRangeMarker::Start{id:marker_id,..} if marker_id==id)).count();
        let ends = cache
            .comment_ranges
            .iter()
            .filter(
                |marker| matches!(marker,CommentRangeMarker::End{id:marker_id,..} if marker_id==id),
            )
            .count();
        if !unique.insert(*id) || (starts, ends) != (1, 1) {
            return Err(Error::Other(
                "REF f annotation target range ownership is ambiguous".into(),
            ));
        }
    }
    Ok(ids)
}

fn ref_comment_graph_counts(package: &OpcPackage) -> Result<HashMap<i32, [usize; 3]>> {
    let mut counts = HashMap::new();
    for (part, xml) in &package.parts {
        if !part.ends_with(".xml")
            || !xml
                .windows(b"comment".len())
                .any(|value| value == b"comment")
        {
            continue;
        }
        let mut reader = NsReader::from_reader(xml.as_slice());
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer).map_err(|error| {
                Error::Other(format!("REF f annotation graph is malformed: {error}"))
            })? {
                Event::Start(element) | Event::Empty(element) => {
                    let (namespace, local) = reader.resolver().resolve_element(element.name());
                    if namespace_is_word(&namespace) {
                        let index = match local.as_ref() {
                            b"commentRangeStart" => Some(0),
                            b"commentRangeEnd" => Some(1),
                            b"commentReference" => Some(2),
                            _ => None,
                        };
                        if let Some(index) = index {
                            let (_, value) = resolved_element_attribute(
                                &element,
                                reader.resolver(),
                                b"id",
                                AttributeNamespace::Word,
                            )?
                            .ok_or_else(|| {
                                Error::Other("REF f annotation graph edge has no ID".into())
                            })?;
                            let id = value.parse::<i32>().map_err(|_| {
                                Error::Other("REF f annotation graph edge has an invalid ID".into())
                            })?;
                            counts.entry(id).or_insert([0; 3])[index] += 1;
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buffer.clear();
        }
    }
    Ok(counts)
}

fn ref_comment_replacement_ids(
    document: &Document,
    field: &Field,
    source: &RefBookmarkCache,
) -> Result<Vec<Option<i32>>> {
    let mut old = RefBookmarkCache {
        runs: Vec::new(),
        comment_ranges: Vec::new(),
    };
    let mut run = CT_R::new("");
    run.content = vec![RunContent::Field(field.clone())];
    append_ref_cached_run(&run, None, false, &mut old)?;
    let source_ids = ref_comment_reference_ids(source)?;
    let old_ids = ref_comment_reference_ids(&old)?;
    if old_ids.is_empty() {
        return Ok(vec![None; source_ids.len()]);
    }
    if old_ids.len() != source_ids.len() {
        return Err(Error::Other(
            "REF f cached annotation reference inventory differs from its current source".into(),
        ));
    }
    let counts = ref_comment_graph_counts(&document.package)?;
    let mut unique = HashSet::new();
    let mut replacements = Vec::new();
    for (id, source_id) in old_ids.iter().zip(&source_ids) {
        let start = old.comment_ranges.iter().filter(|marker| matches!(marker, CommentRangeMarker::Start {id: marker_id,..} if marker_id==id)).count();
        let end = old.comment_ranges.iter().filter(|marker| matches!(marker, CommentRangeMarker::End {id: marker_id,..} if marker_id==id)).count();
        if id == source_id
            || !unique.insert(*id)
            || (start, end) != (1, 1)
            || counts.get(id) != Some(&[1, 1, 1])
        {
            return Err(Error::Other("REF f cached annotation owner is borrowed, shared or has ambiguous range endpoints".into()));
        }
        document.fragment_comment_dependency(*id)?;
        replacements.push(Some(*id));
    }
    Ok(replacements)
}

fn prepare_ref_note_copy(
    document: &mut Document,
    original: &Document,
    kind: StoryKind,
    old_id: i32,
    replace_id: Option<i32>,
    occupied: &mut HashMap<StoryKind, HashSet<i32>>,
) -> Result<(i32, FragmentNoteCopy)> {
    let (source_part, source) = original.fragment_note_dependency(kind, old_id)?;
    if !fragment_note_references(&source)?.is_empty() {
        return Err(Error::Other(
            "REF f note payload contains recursive note references".into(),
        ));
    }
    let part = document.ensure_fragment_note_part_staged(kind)?;
    if part != source_part {
        return Err(Error::Other(
            "REF f note copy has no same-owner relationship scope".into(),
        ));
    }
    let used = if let Some(used) = occupied.get_mut(&kind) {
        used
    } else {
        let notes = CT_Footnotes::from_xml(
            document
                .package
                .get_part(&part)
                .ok_or_else(|| Error::Other("REF f note part is missing".into()))?,
        )?;
        occupied
            .entry(kind)
            .or_insert_with(|| notes.footnotes.iter().map(|note| note.id).collect())
    };
    let id = if let Some(id) = replace_id {
        let (old_part, _) = original.fragment_note_dependency(kind, id)?;
        if old_part != part || !used.contains(&id) {
            return Err(Error::Other(
                "REF f cached note replacement is not a unique existing owner".into(),
            ));
        }
        id
    } else {
        let mut id = 1i32;
        while used.contains(&id) {
            id = id
                .checked_add(1)
                .ok_or_else(|| Error::Other("REF f note IDs are exhausted".into()))?;
        }
        used.insert(id);
        id
    };
    let mut xml = transform_ref_note_copy(original, &source_part, &source)?;
    for relationship_id in relationship_ids_in_xml(&xml)? {
        let relationships = original
            .package
            .get_part_rels(&source_part)
            .ok_or_else(|| Error::Other("REF f note relationship owner is missing".into()))?;
        let relationship = relationships.get_by_id(&relationship_id).ok_or_else(|| {
            Error::Other(format!(
                "REF f note relationship {relationship_id} is missing"
            ))
        })?;
        if !crate::document::relationship_is_internal(relationship) {
            return Err(Error::Other(
                "REF f retained note payload has an external relationship".into(),
            ));
        }
        let target = OpcPackage::resolve_rel_target(&source_part, &relationship.target);
        discover_fragment_part_closure(&original.package, &target, &mut HashSet::new())?;
    }
    let wrapper = format!("<w:body xmlns:w=\"{W_NS}\">").into_bytes();
    let mut body = wrapper.clone();
    body.extend_from_slice(&xml);
    body.extend_from_slice(b"</w:body>");
    let values = body_identity_values(&body)?;
    let mut remap = BodyIdentityRemap::default();
    for old in values.drawing_ids {
        if remap.drawing_ids.contains_key(&old) {
            return Err(Error::Other(
                "REF f source has duplicate drawing IDs".into(),
            ));
        }
        remap
            .drawing_ids
            .insert(old, document.identifiers.reserve_drawing_id()?.to_string());
    }
    let mapped = patch_body_identity_attributes(&body, &remap)?;
    xml = mapped[wrapper.len()..mapped.len() - b"</w:body>".len()].to_vec();
    let maps = HashMap::from([(kind, BTreeMap::from([(old_id.to_string(), id.to_string())]))]);
    xml = patch_fragment_note_ids(&xml, &maps)?;
    Ok((
        id,
        FragmentNoteCopy {
            kind,
            destination_part: part,
            xml,
            source: None,
            replace_id,
        },
    ))
}

fn transform_ref_note_copy(document: &Document, part: &str, source: &[u8]) -> Result<Vec<u8>> {
    let mut reader = NsReader::from_reader(source);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut stack = Vec::<bool>::new();
    let mut edits = Vec::new();
    loop {
        let start = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("REF f note XML is malformed: {error}")))?;
        let word = namespace_is_word(&namespace);
        let end = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let local = element.local_name();
                if word && matches!(local.as_ref(), b"bookmarkStart" | b"bookmarkEnd") {
                    reader
                        .read_to_end_into(element.name(), &mut Vec::new())
                        .map_err(|error| {
                            Error::Other(format!("REF f note bookmark is malformed: {error}"))
                        })?;
                    edits.push(start..reader.buffer_position() as usize);
                } else {
                    let flatten = if word && local.as_ref() == b"hyperlink" {
                        let relationship_id = resolved_element_attribute(&element, reader.resolver(), b"id", AttributeNamespace::Relationship)?.map(|(_, value)| value).ok_or_else(|| Error::Other("REF f internal note hyperlink is unsupported without native controls".into()))?;
                        let relationship = document
                            .package
                            .get_part_rels(part)
                            .and_then(|relationships| relationships.get_by_id(&relationship_id))
                            .ok_or_else(|| {
                                Error::Other("REF f note hyperlink relationship is missing".into())
                            })?;
                        if crate::document::relationship_is_internal(relationship) {
                            return Err(Error::Other("REF f internal note hyperlink is unsupported without native controls".into()));
                        }
                        edits.push(start..end);
                        true
                    } else {
                        false
                    };
                    stack.push(flatten);
                }
            }
            Event::Empty(element) => {
                if word
                    && matches!(
                        element.local_name().as_ref(),
                        b"bookmarkStart" | b"bookmarkEnd"
                    )
                {
                    edits.push(start..end);
                }
            }
            Event::End(_) => {
                if stack
                    .pop()
                    .ok_or_else(|| Error::Other("REF f note XML is unbalanced".into()))?
                {
                    edits.push(start..end);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !stack.is_empty() {
        return Err(Error::Other("REF f note XML is unclosed".into()));
    }
    let mut result = source.to_vec();
    for range in edits.into_iter().rev() {
        result.drain(range);
    }
    Ok(result)
}

struct RefCopyField {
    field: Field,
    locked: bool,
    generated: bool,
    suppress_copied_references: bool,
}

fn collect_ref_copy_fields(
    field: &Field,
    inherited_lock: bool,
    generated: bool,
    suppress_copied_references: bool,
    fields: &mut Vec<RefCopyField>,
) {
    let locked = inherited_lock || field.locked() == Some(true);
    fields.push(RefCopyField {
        field: field.clone(),
        locked,
        generated,
        suppress_copied_references,
    });
    for child in field.nested_fields_in_source_order() {
        collect_ref_copy_fields(child, locked, generated, suppress_copied_references, fields);
    }
    for child in field.cached_fields_in_source_order() {
        collect_ref_copy_fields(child, locked, true, suppress_copied_references, fields);
    }
}

fn append_ref_copy_paragraph(
    paragraph: &CT_P,
    suppress_copied_references: bool,
    fields: &mut Vec<RefCopyField>,
) {
    for content in accepted_toc_runs(paragraph)
        .into_iter()
        .flat_map(|run| &run.run.content)
    {
        if let RunContent::Field(field) = content {
            collect_ref_copy_fields(field, false, false, suppress_copied_references, fields);
        }
    }
}

struct RefBookmarkCache {
    runs: Vec<CT_R>,
    comment_ranges: Vec<CommentRangeMarker>,
}

fn ref_cache_marker_at(marker: &CommentRangeMarker, position: usize) -> Result<CommentRangeMarker> {
    match marker {
        CommentRangeMarker::Start {
            id,
            has_child_content: false,
            ..
        } => Ok(CommentRangeMarker::Start {
            id: *id,
            run_index: position,
            raw_before: 0,
            has_child_content: false,
        }),
        CommentRangeMarker::End {
            id,
            has_child_content: false,
            ..
        } => Ok(CommentRangeMarker::End {
            id: *id,
            run_index: position,
            raw_before: 0,
            has_child_content: false,
        }),
        _ => Err(Error::Other(
            "REF f annotation boundary has unmodeled child content".into(),
        )),
    }
}

fn append_ref_cached_run(
    run: &CT_R,
    inherited: Option<&oxoffice_docx_oxml::properties::CT_RPr>,
    already_prepared: bool,
    cache: &mut RefBookmarkCache,
) -> Result<()> {
    if let [RunContent::Field(field)] = run.content.as_slice() {
        // Normalize a private current owner before deriving both its runs and
        // annotation graph. Pending caller state remains unpublished.
        let current = if already_prepared {
            None
        } else {
            Some(prepared_current_field(field)?)
        };
        let field = current.as_ref().unwrap_or(field);
        let prepared = field.cached_result_owner_projection()?;
        let owned_runs = prepared.as_ref().map(|paragraph| {
            paragraph
                .accepted_run_paths()
                .into_iter()
                .filter_map(|path| paragraph.accepted_run(&path))
                .collect::<Vec<_>>()
        });
        let runs =
            owned_runs.or_else(|| field.cached_result_runs().map(|runs| runs.iter().collect()));
        if let Some(runs) = runs {
            let base = cache.runs.len();
            let mut boundaries = Vec::with_capacity(runs.len() + 1);
            for child in runs {
                boundaries.push(cache.runs.len());
                append_ref_cached_run(child, run.properties.as_ref().or(inherited), true, cache)?;
            }
            boundaries.push(cache.runs.len());
            for marker in field.cached_result_comment_ranges() {
                let position = *boundaries
                    .get(match marker {
                        CommentRangeMarker::Start { run_index, .. }
                        | CommentRangeMarker::End { run_index, .. } => *run_index,
                    })
                    .ok_or_else(|| {
                        Error::Other("REF f cached annotation boundary is stale".into())
                    })?;
                cache
                    .comment_ranges
                    .push(ref_cache_marker_at(marker, position)?);
            }
            if cache.runs.len() == base {
                cache.runs.push(CT_R::new(""));
            }
        } else {
            let segments = field.cached_display_segments();
            if segments.is_empty() {
                let mut stored = CT_R::new(&field.cached_result);
                stored.properties = run.properties.clone().or_else(|| inherited.cloned());
                cache.runs.push(stored);
            } else {
                for (text, properties) in segments {
                    let mut stored = CT_R::new(text);
                    stored.properties = properties
                        .cloned()
                        .or_else(|| run.properties.clone())
                        .or_else(|| inherited.cloned());
                    cache.runs.push(stored);
                }
            }
        }
    } else if run
        .content
        .iter()
        .any(|content| matches!(content, RunContent::Field(_)))
    {
        return Err(Error::Other(
            "REF f mixed field target lacks a source-qualified cache boundary".into(),
        ));
    } else {
        let mut run = run.clone();
        if run.properties.is_none() {
            run.properties = inherited.cloned();
        }
        cache.runs.push(run);
    }
    Ok(())
}

fn append_ref_target_paragraph(
    paragraph: &CT_P,
    start: usize,
    end: usize,
    cache: &mut RefBookmarkCache,
) -> Result<()> {
    let projected = paragraph.accepted_bookmark_runs();
    let selected = projected
        .get(start..end)
        .ok_or_else(|| Error::Other("REF f target run range is stale".into()))?;
    let mut boundaries = Vec::with_capacity(selected.len() + 1);
    for run in selected {
        boundaries.push(cache.runs.len());
        append_ref_cached_run(run, None, false, cache)?;
    }
    boundaries.push(cache.runs.len());
    if !paragraph.comment_ranges.is_empty() {
        if !paragraph.content_controls.is_empty()
            || !paragraph.revisions.is_empty()
            || projected.len() != paragraph.runs.len()
        {
            return Err(Error::Other(
                "REF f annotation target lacks an exact accepted boundary projection".into(),
            ));
        }
        let mut starts = BTreeMap::new();
        let mut pairs = Vec::new();
        for marker in &paragraph.comment_ranges {
            match marker {
                CommentRangeMarker::Start {
                    id,
                    run_index,
                    has_child_content: false,
                    ..
                } => {
                    if starts.insert(*id, *run_index).is_some() {
                        return Err(Error::Other(
                            "REF f annotation source range is ambiguous".into(),
                        ));
                    }
                }
                CommentRangeMarker::End {
                    id,
                    run_index,
                    has_child_content: false,
                    ..
                } => {
                    let first = starts.remove(id).ok_or_else(|| {
                        Error::Other("REF f annotation source range is unpaired".into())
                    })?;
                    if first > *run_index {
                        return Err(Error::Other(
                            "REF f annotation source range is reversed".into(),
                        ));
                    }
                    pairs.push((*id, first, *run_index));
                }
                _ => {
                    return Err(Error::Other(
                        "REF f annotation source range contains unmodeled payload".into(),
                    ));
                }
            }
        }
        if !starts.is_empty() {
            return Err(Error::Other(
                "REF f annotation source range crosses an unqualified target paragraph".into(),
            ));
        }
        for (id, first, last) in pairs {
            let clipped_start = first.max(start);
            let clipped_end = last.min(end);
            if clipped_start >= clipped_end {
                continue;
            }
            let first = boundaries[clipped_start - start];
            let last = boundaries[clipped_end - start];
            cache.comment_ranges.push(CommentRangeMarker::Start {
                id,
                run_index: first,
                raw_before: 0,
                has_child_content: false,
            });
            cache.comment_ranges.push(CommentRangeMarker::End {
                id,
                run_index: last,
                raw_before: 0,
                has_child_content: false,
            });
            if !cache.runs.iter().flat_map(|run| &run.content).any(|content| matches!(content,RunContent::CommentReference{id:reference,..} if *reference==id)) {
                let mut reference=CT_R::new("");reference.content=vec![RunContent::CommentReference{id,raw_before:0}];cache.runs.push(reference);
            }
        }
    }
    cache.comment_ranges.sort_by_key(|marker| match marker {
        CommentRangeMarker::Start { run_index, .. } | CommentRangeMarker::End { run_index, .. } => {
            *run_index
        }
    });
    Ok(())
}

fn ref_bookmark_runs(document: &Document, target: &str) -> Result<Option<RefBookmarkCache>> {
    let bookmarks = document.bookmarks();
    let matching = bookmarks
        .iter()
        .filter(|bookmark| bookmark.name() == Some(target))
        .collect::<Vec<_>>();
    if !matching.is_empty() {
        if matching.len() != 1 || matching[0].issue().is_some() {
            return Err(Error::Other(
                "REF f target bookmark is ambiguous or malformed".into(),
            ));
        }
        let range = matching[0]
            .range()
            .ok_or_else(|| Error::Other("REF f target lacks a range".into()))?;
        let mut paragraphs = Vec::new();
        collect_body_paragraphs(&document.document.body, &mut paragraphs, None);
        let mut cache = RefBookmarkCache {
            runs: Vec::new(),
            comment_ranges: Vec::new(),
        };
        for index in range.start.body_index..=range.end.body_index {
            let paragraph = paragraphs
                .get(index)
                .ok_or_else(|| Error::Other("REF f target paragraph is absent".into()))?;
            let projected = paragraph.accepted_bookmark_runs();
            let start = if index == range.start.body_index {
                range.start.run_index
            } else {
                0
            };
            let end = if index == range.end.body_index {
                range.end.run_index
            } else {
                projected.len()
            };
            if index > range.start.body_index {
                let mut line = CT_R::new("");
                line.content = vec![RunContent::Break(oxoffice_docx_oxml::text::BreakType::Line)];
                cache.runs.push(line);
            }
            append_ref_target_paragraph(paragraph, start, end, &mut cache)?;
        }
        return Ok(Some(cache));
    }
    let (ranges, across) = document.story_range_inventory()?;
    if across
        .iter()
        .any(|kind| matches!(kind, crate::StoryRangeKind::Bookmark { name, .. } if name == target))
    {
        return Err(Error::Other(
            "REF f target bookmark runs across story owners".into(),
        ));
    }
    let range = ranges.into_iter().find(|range| matches!(range.kind(), crate::StoryRangeKind::Bookmark { name, .. } if name == target));
    let Some(range) = range else {
        return Ok(None);
    };
    let crate::StoryRangeKind::Bookmark { id, .. } = range.kind() else {
        unreachable!()
    };
    let mut selected = false;
    let mut cache = RefBookmarkCache {
        runs: Vec::new(),
        comment_ranges: Vec::new(),
    };
    for (location, xml) in document.story_range_paragraphs()? {
        if location == range.range().start.location {
            selected = true;
        }
        if !selected {
            continue;
        }
        if location.story() != range.range().start.location.story() {
            return Err(Error::Other("REF f target crosses story owners".into()));
        }
        let paragraph = CT_P::from_xml_fragment(&xml)?;
        let runs = paragraph.accepted_bookmark_runs();
        let first = if location == range.range().start.location {
            paragraph
                .bookmark_markers
                .iter()
                .find(|marker| {
                    marker.is_start() && marker.id() == Some(*id) && marker.name() == Some(target)
                })
                .ok_or_else(|| {
                    Error::Other(
                        "REF f target start marker is absent from its physical projection".into(),
                    )
                })?
                .projected_run_index()
        } else {
            0
        };
        let last = if location == range.range().end.location {
            paragraph
                .bookmark_markers
                .iter()
                .find(|marker| !marker.is_start() && marker.id() == Some(*id))
                .ok_or_else(|| {
                    Error::Other(
                        "REF f target end marker is absent from its physical projection".into(),
                    )
                })?
                .projected_run_index()
        } else {
            runs.len()
        };
        if !cache.runs.is_empty() {
            let mut line = CT_R::new("");
            line.content = vec![RunContent::Break(oxoffice_docx_oxml::text::BreakType::Line)];
            cache.runs.push(line);
        }
        append_ref_target_paragraph(&paragraph, first, last, &mut cache)?;
        if location == range.range().end.location {
            return Ok(Some(cache));
        }
    }
    Err(Error::Other("REF f related target owner is absent".into()))
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

struct BookmarkValue {
    text: String,
    range: Option<crate::RunRange>,
}

fn field_needs_numbering_layout(field: &Field) -> bool {
    let instruction = field.effective_instruction();
    let own = unsupported_switch(&instruction).is_none()
        && validate_instruction_shape(&instruction).is_ok()
        && (instruction.name == "STYLEREF"
            || (instruction.name == "REF"
                && ["n", "r", "w"]
                    .iter()
                    .any(|name| has_switch(&instruction, name))));
    own || field
        .effective_nested_fields_in_source_order(&instruction)
        .into_iter()
        .any(field_needs_numbering_layout)
}

impl<'a> Evaluator<'a> {
    fn new(document: &'a Document, context: &'a FieldEvaluationContext) -> Self {
        let mut bookmarks: BTreeMap<String, BookmarkValue> = document
            .bookmarks()
            .into_iter()
            .filter(|bookmark| bookmark.issue().is_none())
            .filter_map(|bookmark| {
                Some((
                    bookmark.name()?.to_owned(),
                    BookmarkValue {
                        text: bookmark.text().to_owned(),
                        range: bookmark.range(),
                    },
                ))
            })
            .collect();
        // Paragraph inventory keeps the legacy body map and its name filter.
        // Pairing failure blocks only extra related admission.
        let mut cross_owner_bookmarks = HashSet::new();
        if let Ok((paired, name_counts)) = document.story_range_metadata() {
            if let Ok((ranges, across)) = paired {
                for kind in across {
                    if let crate::StoryRangeKind::Bookmark { name, .. } = kind
                        && !bookmarks.contains_key(&name)
                    {
                        cross_owner_bookmarks.insert(name);
                    }
                }
                for range in ranges {
                    let crate::StoryRangeKind::Bookmark { name, .. } = range.kind() else {
                        continue;
                    };
                    if bookmarks.contains_key(name)
                        || range.range().start.location.story().kind() == StoryKind::Body
                    {
                        continue;
                    }
                    if let Ok(Some(cache)) = ref_bookmark_runs(document, name) {
                        bookmarks.insert(
                            name.clone(),
                            BookmarkValue {
                                text: cache.runs.iter().map(CT_R::text).collect(),
                                range: None,
                            },
                        );
                    }
                }
            }
            bookmarks.retain(|name, _| name_counts.get(name).is_some_and(|count| *count == 1));
        }
        Self {
            document,
            context,
            bookmarks,
            cross_owner_bookmarks,
            numbering_layout: None,
            numbering_nodes: HashMap::new(),
            navigation_styles: HashMap::new(),
            navigation_style_pages: HashMap::new(),
            navigation_field_pages: HashMap::new(),
            results: Vec::new(),
            visible_results: Vec::new(),
            general_field_contexts: Vec::new(),
            general_context: true,
            mail_merge_stories: BTreeMap::new(),
            nested_outcomes: Vec::new(),
            missing_merge_fields_as_empty: false,
            current_field_source: None,
            sequence_snapshot: None,
            ref_note_counts: None,
            locked_fields: HashSet::new(),
            sequence_nodes: Vec::new(),
            field_sources: HashMap::new(),
        }
    }

    fn for_mail_merge(document: &'a Document, context: &'a FieldEvaluationContext) -> Self {
        let mut evaluator = Self::new(document, context);
        evaluator.missing_merge_fields_as_empty = true;
        evaluator
    }

    fn refresh_main_bookmark_sequence_text(&mut self, paragraphs: &[&CT_P]) -> Result<()> {
        let Some(snapshot) = &self.sequence_snapshot else {
            return Ok(());
        };
        if self.sequence_nodes.len() != paragraphs.len() {
            return Err(Error::Other(
                "main bookmark sequence source inventory is incomplete".into(),
            ));
        }
        let mut displays = Vec::with_capacity(paragraphs.len());
        for (paragraph, node) in paragraphs.iter().zip(&self.sequence_nodes) {
            let mut fields = Vec::new();
            for run in accepted_toc_runs(paragraph) {
                for content in &run.run.content {
                    if let RunContent::Field(field) = content {
                        collect_preorder_fields(field, false, &mut fields);
                    }
                }
            }
            let mut physical_fields = Vec::new();
            for run in paragraph.source_runs() {
                for content in &run.content {
                    if let RunContent::Field(field) = content {
                        collect_preorder_fields(field, false, &mut physical_fields);
                    }
                }
            }
            let physical_indices = physical_fields
                .iter()
                .enumerate()
                .map(|(index, (field, _))| (std::ptr::from_ref(*field), index as u32))
                .collect::<HashMap<_, _>>();
            let updates = fields
                .iter()
                .map(|(field, _)| {
                    match node.and_then(|node| {
                        snapshot.field_value(oxoffice_layout::FieldSource {
                            node,
                            index: *physical_indices.get(&std::ptr::from_ref(*field))?,
                        })
                    }) {
                        Some(Ok(value)) => Some(CachedFieldUpdate {
                            typed_runs: None,
                            comment_ranges: Vec::new(),
                            cached_result: value.to_owned(),
                            dirty: false,
                        }),
                        _ => None,
                    }
                })
                .collect::<Vec<_>>();
            let mut projection = (*paragraph).clone();
            let mut consumed = 0;
            apply_updates_to_paragraph(&mut projection, &updates, &mut consumed)?;
            if consumed != updates.len() {
                return Err(Error::Other(
                    "bookmark sequence field traversal is inconsistent".into(),
                ));
            }
            displays.push(
                projection
                    .accepted_bookmark_runs()
                    .into_iter()
                    .map(|run| {
                        let mut displayed = run.clone();
                        for content in &mut displayed.content {
                            if let RunContent::Field(field) = content
                                && field.effective_instruction().name == "SEQ"
                            {
                                *content = RunContent::Text(
                                    oxoffice_docx_oxml::text::CT_Text::new(&field.cached_result),
                                );
                            }
                        }
                        displayed.text()
                    })
                    .collect::<Vec<_>>(),
            );
        }
        for bookmark in self.bookmarks.values_mut() {
            let Some(range) = bookmark.range else {
                continue;
            };
            let mut parts = Vec::new();
            for index in range.start.body_index..=range.end.body_index {
                let Some(runs) = displays.get(index) else {
                    return Err(Error::Other(
                        "bookmark sequence range owner is missing".into(),
                    ));
                };
                let start = if index == range.start.body_index {
                    range.start.run_index
                } else {
                    0
                };
                let end = if index == range.end.body_index {
                    range.end.run_index
                } else {
                    runs.len()
                };
                let Some(slice) = runs.get(start..end) else {
                    return Err(Error::Other("bookmark sequence range is stale".into()));
                };
                parts.push(slice.concat());
            }
            bookmark.text = parts.join("\n");
        }
        Ok(())
    }

    fn evaluate_story(&mut self, story: &str, paragraphs: &[&CT_P]) -> Result<()> {
        if self.numbering_layout.is_none()
            && paragraphs.iter().any(|paragraph| {
                paragraph.runs().into_iter().any(|run| {
                    run.content.iter().any(|content| match content {
                        RunContent::Field(field) => field_needs_numbering_layout(field),
                        _ => false,
                    })
                })
            })
        {
            self.prepare_numbering_layout()?;
        }
        if self.sequence_snapshot.is_some() && self.sequence_nodes.len() != paragraphs.len() {
            return Err(Error::Other(format!(
                "source inventory identified {} of {} paragraphs in {story}",
                self.sequence_nodes.len(),
                paragraphs.len()
            )));
        }
        for (paragraph_index, paragraph) in paragraphs.iter().enumerate() {
            self.general_context = self
                .general_field_contexts
                .get(paragraph_index)
                .copied()
                .unwrap_or(true);
            self.register_paragraph_fields(
                paragraph,
                self.sequence_nodes.get(paragraph_index).copied().flatten(),
            );
            for run in accepted_toc_runs(paragraph) {
                for content in &run.run.content {
                    if let RunContent::Field(field) = content {
                        self.evaluate_field(field, story, paragraphs, paragraph_index, None);
                    }
                }
            }
        }
        Ok(())
    }

    fn register_paragraph_fields(
        &mut self,
        paragraph: &CT_P,
        node: Option<oxoffice_docx_layout::SourceNodeId>,
    ) {
        self.field_sources.clear();
        self.locked_fields.clear();
        let mut fields = Vec::new();
        for run in paragraph.source_runs() {
            for content in &run.content {
                if let RunContent::Field(field) = content {
                    collect_preorder_fields(field, false, &mut fields);
                }
            }
        }
        self.locked_fields.extend(
            fields
                .iter()
                .filter(|(_, locked)| *locked)
                .map(|(field, _)| std::ptr::from_ref(*field) as usize),
        );
        if let Some(node) = node {
            self.field_sources
                .extend(fields.into_iter().enumerate().map(|(index, (field, _))| {
                    (
                        std::ptr::from_ref(field) as usize,
                        oxoffice_layout::FieldSource {
                            node,
                            index: index as u32,
                        },
                    )
                }));
        }
    }

    fn prepare_numbering_layout(&mut self) -> Result<()> {
        let layout = self.document.layout_deterministic()?;
        self.bind_numbering_layout(layout);
        Ok(())
    }

    fn bind_numbering_layout(&mut self, layout: Arc<oxoffice_docx_layout::WordLayoutResult>) {
        self.numbering_nodes.clear();
        for index in 1.. {
            let Some(node) = oxoffice_docx_layout::SourceNodeId::new(index) else {
                break;
            };
            let Some(path) = layout.source_node(node) else {
                break;
            };
            self.numbering_nodes.insert(path.clone(), node);
        }
        self.navigation_styles.clear();
        self.navigation_style_pages.clear();
        self.navigation_field_pages.clear();
        let mut aliases: HashMap<&str, Vec<String>> = HashMap::new();
        for style in &self.document.styles.styles {
            let names = aliases.entry(&style.style_id).or_default();
            names.push(style.style_id.to_lowercase());
            if let Some(name) = &style.name {
                names.push(name.to_lowercase());
            }
            names.sort();
            names.dedup();
        }
        for (node, path, _, _, style, _, page) in layout.paragraph_navigation() {
            if path.story != oxoffice_docx_layout::WordStory::Document {
                continue;
            }
            for alias in style
                .and_then(|style| aliases.get(style))
                .into_iter()
                .flatten()
            {
                self.navigation_styles
                    .entry(alias.clone())
                    .or_default()
                    .push(node);
                if let Some(page) = page {
                    self.navigation_style_pages
                        .entry(alias.clone())
                        .or_default()
                        .entry(page)
                        .or_default()
                        .push(node);
                }
            }
        }
        for nodes in self.navigation_styles.values_mut() {
            nodes.sort_unstable_by_key(|node| node.get());
            nodes.dedup();
        }
        for placement in layout.field_placements() {
            self.navigation_field_pages
                .entry(placement.source)
                .or_default()
                .insert(placement.physical_page);
        }
        self.numbering_layout = Some(layout);
    }

    fn numbering_source(
        &self,
        node: oxoffice_docx_layout::SourceNodeId,
    ) -> Option<oxoffice_docx_layout::SourceNodeId> {
        let path = self.sequence_snapshot.as_ref()?.source_node(node)?;
        self.numbering_nodes.get(path).copied()
    }

    fn ensure_numbering_layout_for_field(&mut self, field: &Field) -> Result<()> {
        if self.numbering_layout.is_none() && field_needs_numbering_layout(field) {
            self.prepare_numbering_layout()?;
        }
        Ok(())
    }

    fn record_field_preparation_failure(&mut self, field: &Field, message: &str) -> FieldOutcome {
        let outcome = keep(message);
        self.visible_results
            .push(self.general_context || matches!(field.instruction.name.as_str(), "SEQ" | "REF"));
        self.results.push(FieldEvaluation {
            field_index: self.results.len(),
            instruction: field.instruction.raw.clone(),
            cached_result: field.cached_result.clone(),
            outcome: outcome.clone(),
        });
        outcome
    }

    fn evaluate_field(
        &mut self,
        field: &Field,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
        occurrence: Option<(Option<oxoffice_layout::FieldSource>, bool)>,
    ) -> FieldOutcome {
        let previous_source = self.current_field_source;
        let (source, locked) = occurrence.unwrap_or_else(|| {
            (
                self.field_sources
                    .get(&(std::ptr::from_ref(field) as usize))
                    .copied(),
                self.locked_fields
                    .contains(&(std::ptr::from_ref(field) as usize)),
            )
        });
        let prepared = if occurrence.is_some() {
            None
        } else {
            match prepared_current_field(field) {
                Ok(current) => Some(current),
                Err(error) => {
                    return self.record_field_preparation_failure(
                        field,
                        &format!("field preparation failed: {error}"),
                    );
                }
            }
        };
        let field = prepared.as_ref().unwrap_or(field);
        let instruction = field.effective_instruction();
        self.current_field_source = source;
        let result_index = self.results.len();
        let visible = self.general_context || matches!(instruction.name.as_str(), "SEQ" | "REF");
        self.visible_results.push(visible);
        self.results.push(FieldEvaluation {
            field_index: result_index,
            instruction: instruction.raw.clone(),
            cached_result: field.cached_result.clone(),
            outcome: keep("field evaluation did not complete"),
        });

        self.nested_outcomes.push(BTreeMap::new());
        let nested = self.evaluate_nested_fields(
            field,
            &instruction,
            story,
            paragraphs,
            paragraph_index,
            locked,
        );
        let mut outcome = if let Err(error) = nested {
            keep(&format!("field occurrence association failed: {error}"))
        } else if !visible {
            keep("ordinary field is outside its established evaluation context")
        } else if locked {
            keep("locked field retains its stored display")
        } else if instruction.name == "SEQ" && self.sequence_snapshot.is_some() {
            match self
                .current_field_source
                .and_then(|source| self.sequence_snapshot.as_ref()?.field_value(source))
            {
                Some(Ok(value)) => FieldOutcome::Resolved(value.to_owned()),
                Some(Err(message)) => keep(message),
                None => keep("SEQ lacks a unique physical accepted source"),
            }
        } else {
            self.evaluate_instruction(&instruction, story, paragraphs, paragraph_index)
        };
        if story == "main"
            && instruction.name == "REF"
            && has_switch(&instruction, "f")
            && matches!(outcome, FieldOutcome::Resolved(_))
        {
            let mut old_references = Vec::new();
            if let Err(error) = collect_ref_cached_note_references(field, &mut old_references) {
                outcome = keep(&error.to_string());
            } else if !old_references.is_empty() {
                let qualification = (|| -> Result<()> {
                    if self.ref_note_counts.is_none() {
                        self.ref_note_counts =
                            Some(ref_note_reference_counts(&self.document.package)?);
                    }
                    let target = text_argument(&instruction, 0)
                        .ok_or_else(|| Error::Other("REF f target is absent".into()))?;
                    let cache = ref_bookmark_runs(self.document, target)?
                        .ok_or_else(|| Error::Other("REF f target is missing".into()))?;
                    let new_references = cache
                        .runs
                        .iter()
                        .flat_map(|run| &run.content)
                        .filter_map(|content| match content {
                            RunContent::FootnoteRef { id, .. } => Some((StoryKind::Footnote, *id)),
                            RunContent::EndnoteRef { id, .. } => Some((StoryKind::Endnote, *id)),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    if old_references.len() != new_references.len() {
                        return Err(Error::Other(
                            "cached note graph differs from the current source reference inventory"
                                .into(),
                        ));
                    }
                    for ((kind, id), (source_kind, source_id)) in
                        old_references.iter().zip(&new_references)
                    {
                        if kind != source_kind
                            || id == source_id
                            || self
                                .ref_note_counts
                                .as_ref()
                                .and_then(|counts| counts.get(&(*kind, *id)))
                                != Some(&1)
                        {
                            return Err(Error::Other("cached note reference ownership is ambiguous, shared or borrowed from its source".into()));
                        }
                        self.document.fragment_note_dependency(*kind, *id)?;
                    }
                    Ok(())
                })();
                if let Err(error) = qualification {
                    outcome = keep(&format!(
                        "REF f repeated note copy retains its cache: {error}"
                    ));
                }
            }
        }
        if story == "main"
            && instruction.name == "REF"
            && has_switch(&instruction, "f")
            && matches!(outcome, FieldOutcome::Resolved(_))
        {
            let qualification = text_argument(&instruction, 0)
                .ok_or_else(|| Error::Other("REF f target is absent".into()))
                .and_then(|target| ref_bookmark_runs(self.document, target))
                .and_then(|cache| {
                    cache.ok_or_else(|| Error::Other("REF f target is absent".into()))
                })
                .and_then(|cache| {
                    let replacements = ref_comment_replacement_ids(self.document, field, &cache)?;
                    let ids = ref_comment_reference_ids(&cache)?;
                    if !ids.is_empty() {
                        let mut staged = self.document.clone_for_staging();
                        let mut occupied = self
                            .document
                            .comments
                            .as_ref()
                            .map(|comments| {
                                comments
                                    .comments
                                    .iter()
                                    .map(|comment| comment.id)
                                    .collect::<HashSet<_>>()
                            })
                            .unwrap_or_default();
                        let mut identities =
                            ref_comment_occupied_identity_values(&self.document.package)?;
                        for (id, replacement) in ids.into_iter().zip(replacements) {
                            prepare_ref_comment_copy(
                                &mut staged,
                                self.document,
                                id,
                                replacement,
                                &mut occupied,
                                &mut identities,
                            )?;
                        }
                    }
                    Ok(())
                });
            if let Err(error) = qualification {
                outcome = keep(&format!(
                    "REF f repeated annotation copy retains its cache: {error}"
                ));
            }
        }
        self.nested_outcomes.pop();
        self.results[result_index].outcome = outcome.clone();
        self.current_field_source = previous_source;
        outcome
    }

    fn evaluate_instruction(
        &mut self,
        instruction: &FieldInstruction,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
    ) -> FieldOutcome {
        if instruction.name.is_empty() {
            return keep("field instruction has no name");
        }
        if let Some(name) = unsupported_switch(instruction) {
            return keep(&format!(
                "field {} uses unsupported switch \\{name}",
                instruction.name
            ));
        }
        if let Err(diagnostic) = validate_instruction_shape(instruction) {
            return keep(&diagnostic);
        }

        let outcome = match instruction.name.as_str() {
            "PAGE" | "NUMPAGES" | "SECTION" | "SECTIONPAGES" => FieldOutcome::DeferredPagination,
            "PAGEREF" => self.evaluate_pageref(instruction),
            "REF" => self.evaluate_ref(instruction, story, paragraph_index),
            "IF" => self.evaluate_if(instruction, story, paragraphs, paragraph_index),
            "SEQ" => keep("SEQ requires accepted physical source context"),
            "DOCPROPERTY" => self.evaluate_docproperty(instruction),
            "DOCVARIABLE" => self.evaluate_docvariable(instruction),
            "STYLEREF" => self.evaluate_styleref(instruction, paragraphs, paragraph_index),
            "INCLUDETEXT" => self.evaluate_includetext(instruction),
            "DATE" | "TIME" => self.evaluate_date_time(instruction),
            "FILENAME" => self.evaluate_filename(instruction),
            "AUTHOR" => self.evaluate_author(),
            "MERGEFIELD" => self.evaluate_mergefield(instruction),
            "=" => self.evaluate_formula(instruction, story, paragraphs, paragraph_index),
            "TOC" => self.evaluate_toc(instruction, story, paragraphs, paragraph_index),
            "TC" => self.evaluate_tc(instruction, story, paragraphs, paragraph_index),
            "NEXT" | "NEXTIF" | "SKIPIF" | "MERGEREC" | "MERGESEQ" => {
                self.evaluate_mail_merge_control(instruction, story, paragraphs, paragraph_index)
            }
            "DISPLAYBARCODE" | "MERGEBARCODE" => {
                self.evaluate_barcode(instruction, story, paragraphs, paragraph_index)
            }
            name => keep(&format!("field {name} is unsupported")),
        };

        match outcome {
            FieldOutcome::Resolved(value) => {
                match apply_formats(instruction, &value, self.context.now) {
                    Ok(value) => FieldOutcome::Resolved(value),
                    Err(diagnostic) => keep(&diagnostic),
                }
            }
            other => other,
        }
    }

    fn evaluate_nested_fields(
        &mut self,
        field: &Field,
        instruction: &FieldInstruction,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
        inherited_locked: bool,
    ) -> Result<()> {
        // Canonical source indices are preorder occurrences under this actual
        // outer source. Resolve effective clones through argument/switch slots,
        // not by equal text, count-only zips or shared child addresses.
        let parent_source = self.current_field_source;
        let original_nested = field.nested_fields_in_source_order();
        let mut next = match parent_source {
            Some(source) => Some(
                source
                    .index
                    .checked_add(1)
                    .ok_or_else(|| Error::Other("field source index overflow".into()))?,
            ),
            None => None,
        };
        let mut instruction_sources = Vec::new();
        for original in &original_nested {
            instruction_sources.push(next);
            let mut subtree = Vec::new();
            collect_preorder_fields(original, false, &mut subtree);
            let count = u32::try_from(subtree.len())
                .map_err(|_| Error::Other("field source count overflow".into()))?;
            next = match next {
                Some(index) => Some(
                    index
                        .checked_add(count)
                        .ok_or_else(|| Error::Other("field source index overflow".into()))?,
                ),
                None => None,
            };
        }
        let mut selected = HashSet::new();
        for effective in field.effective_nested_fields_in_source_order(instruction) {
            let original = instruction.arguments.iter().enumerate().find_map(|(slot, argument)| {
                matches!(argument, FieldArgument::Nested(child) if std::ptr::eq(child.as_ref(), effective))
                    .then(|| field.instruction.arguments.get(slot)).flatten()
            }).or_else(|| instruction.switches.iter().enumerate().find_map(|(slot, switch)| {
                matches!(&switch.argument, Some(FieldArgument::Nested(child)) if std::ptr::eq(child.as_ref(), effective))
                    .then(|| field.instruction.switches.get(slot).and_then(|switch| switch.argument.as_ref())).flatten()
            }));
            let Some(FieldArgument::Nested(original)) = original else {
                return Err(Error::Other(
                    "effective nested field has no current instruction slot".into(),
                ));
            };
            let position = original_nested
                .iter()
                .position(|candidate| std::ptr::eq(*candidate, original.as_ref()))
                .ok_or_else(|| {
                    Error::Other("effective nested field has no active source occurrence".into())
                })?;
            if !selected.insert(position)
                || original.as_ref() != effective
                || original.form() != effective.form()
                || original.source_owner_id() != effective.source_owner_id()
            {
                return Err(Error::Other(
                    "effective instruction slot association is not injective".into(),
                ));
            }
            let source = parent_source
                .zip(instruction_sources[position])
                .map(|(parent, index)| oxoffice_layout::FieldSource {
                    node: parent.node,
                    index,
                });
            let outcome = self.evaluate_field(
                effective,
                story,
                paragraphs,
                paragraph_index,
                Some((source, inherited_locked || effective.locked() == Some(true))),
            );
            self.nested_outcomes
                .last_mut()
                .expect("field evaluation frame exists")
                .insert(std::ptr::from_ref(effective) as usize, outcome);
        }
        if selected.len() != original_nested.len() {
            return Err(Error::Other(
                "effective instruction source association is incomplete".into(),
            ));
        }
        for nested in field.cached_fields_in_source_order() {
            let source =
                parent_source
                    .zip(next)
                    .map(|(parent, index)| oxoffice_layout::FieldSource {
                        node: parent.node,
                        index,
                    });
            let outcome = self.evaluate_field(
                nested,
                story,
                paragraphs,
                paragraph_index,
                Some((source, inherited_locked || nested.locked() == Some(true))),
            );
            self.nested_outcomes
                .last_mut()
                .expect("field evaluation frame exists")
                .insert(std::ptr::from_ref(nested) as usize, outcome);
            let mut subtree = Vec::new();
            collect_preorder_fields(nested, false, &mut subtree);
            let count = u32::try_from(subtree.len())
                .map_err(|_| Error::Other("field source count overflow".into()))?;
            next = match next {
                Some(index) => Some(
                    index
                        .checked_add(count)
                        .ok_or_else(|| Error::Other("field source index overflow".into()))?,
                ),
                None => None,
            };
        }
        Ok(())
    }

    fn evaluate_ref(
        &self,
        instruction: &FieldInstruction,
        _story: &str,
        _paragraph_index: usize,
    ) -> FieldOutcome {
        let Some(target) = text_argument(instruction, 0) else {
            return keep("REF requires a bookmark name");
        };
        if has_switch(instruction, "f") {
            return match ref_bookmark_runs(self.document, target) {
                Ok(Some(cache)) => FieldOutcome::Resolved(
                    self.bookmarks
                        .get(target)
                        .map(|bookmark| bookmark.text.clone())
                        .unwrap_or_else(|| cache.runs.iter().map(CT_R::text).collect()),
                ),
                Ok(None) => keep(&format!("REF target {target} was not found")),
                Err(error) => keep(&format!("REF f source could not be resolved: {error}")),
            };
        }
        match self.bookmarks.get(target) {
            Some(bookmark) => {
                let position = if has_switch(instruction, "p") {
                    match self
                        .sequence_snapshot
                        .as_ref()
                        .zip(self.current_field_source)
                        .ok_or_else(|| {
                            "REF relative source lacks an accepted physical binding".to_owned()
                        })
                        .and_then(|(snapshot, source)| {
                            snapshot.bookmark_relative_position(target, source)
                        }) {
                        Ok(position) => Some(position.to_owned()),
                        Err(diagnostic) => return keep(&diagnostic),
                    }
                } else {
                    None
                };
                let numbering_switch = if has_switch(instruction, "w") {
                    Some("full")
                } else if has_switch(instruction, "r") {
                    Some("relative")
                } else if has_switch(instruction, "n") {
                    Some("level")
                } else {
                    None
                };
                let Some(_) = numbering_switch else {
                    if let Some(position) = position {
                        return FieldOutcome::Resolved(position);
                    }
                    return FieldOutcome::Resolved(bookmark.text.clone());
                };
                let Some(numbering) = self
                    .numbering_layout
                    .as_ref()
                    .and_then(|layout| layout.bookmark_numbering(target))
                else {
                    let mut value = if self
                        .numbering_layout
                        .as_ref()
                        .is_some_and(|layout| layout.bookmark_numbering_suppressed(target))
                    {
                        "0".into()
                    } else {
                        bookmark.text.clone()
                    };
                    if let Some(position) = position {
                        value.push(' ');
                        value.push_str(&position);
                    }
                    return FieldOutcome::Resolved(value);
                };
                let mut value = match numbering.numbered_reference_text(
                    instruction,
                    self.current_field_source
                        .and_then(|source| self.numbering_source(source.node))
                        .and_then(|node| self.numbering_layout.as_ref()?.paragraph_numbering(node)),
                ) {
                    Ok(value) => value,
                    Err(diagnostic) => return keep(&diagnostic),
                };
                if let Some(position) = position {
                    value.push(' ');
                    value.push_str(&position);
                }
                FieldOutcome::Resolved(value)
            }
            None if self.cross_owner_bookmarks.contains(target) => {
                keep(&format!("REF target {target} runs across story owners"))
            }
            None => keep(&format!("REF target {target} was not found")),
        }
    }

    fn evaluate_pageref(&self, instruction: &FieldInstruction) -> FieldOutcome {
        let Some(target) = text_argument(instruction, 0) else {
            return keep("PAGEREF requires a bookmark name");
        };
        if self.bookmarks.contains_key(target) {
            FieldOutcome::DeferredPagination
        } else {
            keep(&format!("PAGEREF target {target} was not found"))
        }
    }

    fn evaluate_if(
        &mut self,
        instruction: &FieldInstruction,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
    ) -> FieldOutcome {
        if instruction.arguments.len() != 5 {
            return keep("IF requires two operands, an operator, and two results");
        }
        let arguments = instruction
            .arguments
            .iter()
            .map(|argument| self.resolve_argument(argument, story, paragraphs, paragraph_index))
            .collect::<Vec<_>>();
        let left = match &arguments[0] {
            Ok(value) => value,
            Err(diagnostic) => return keep(diagnostic),
        };
        let operator = match &arguments[1] {
            Ok(value) => value,
            Err(diagnostic) => return keep(diagnostic),
        };
        let right = match &arguments[2] {
            Ok(value) => value,
            Err(diagnostic) => return keep(diagnostic),
        };
        let Some(condition) = compare_if(left, operator, right) else {
            return keep(&format!("IF operator {operator} is unsupported"));
        };
        let selected = if condition { 3 } else { 4 };
        match &arguments[selected] {
            Ok(value) => FieldOutcome::Resolved(value.clone()),
            Err(diagnostic) => keep(diagnostic),
        }
    }

    fn resolve_argument(
        &mut self,
        argument: &FieldArgument,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
    ) -> std::result::Result<String, String> {
        match argument {
            FieldArgument::Text(value) => Ok(value.clone()),
            FieldArgument::Nested(field) => {
                let key = std::ptr::from_ref(field.as_ref()) as usize;
                let outcome = if let Some(outcome) = self
                    .nested_outcomes
                    .last()
                    .and_then(|outcomes| outcomes.get(&key))
                {
                    outcome.clone()
                } else {
                    let outcome =
                        self.evaluate_field(field, story, paragraphs, paragraph_index, None);
                    self.nested_outcomes
                        .last_mut()
                        .expect("field evaluation frame exists")
                        .insert(key, outcome.clone());
                    outcome
                };
                match outcome {
                    FieldOutcome::Resolved(value) => Ok(value),
                    FieldOutcome::DeferredPagination => {
                        Err("nested field requires deferred pagination".to_owned())
                    }
                    FieldOutcome::TableOfContents(_)
                    | FieldOutcome::TableOfContentsEntry(_)
                    | FieldOutcome::MailMergeControl(_)
                    | FieldOutcome::Barcode(_) => {
                        Err("nested field produced a non-text result".to_owned())
                    }
                    FieldOutcome::KeepStored { diagnostic } => {
                        Err(format!("nested field was not resolved: {diagnostic}"))
                    }
                }
            }
        }
    }

    fn evaluate_docproperty(&self, instruction: &FieldInstruction) -> FieldOutcome {
        let Some(name) = text_argument(instruction, 0) else {
            return keep("DOCPROPERTY requires a property name");
        };
        if let Some(value) = self.core_property(name) {
            return FieldOutcome::Resolved(value.to_owned());
        }
        let matches = self
            .document
            .custom_properties
            .iter()
            .flat_map(|properties| &properties.properties)
            .filter(|property| {
                property
                    .name
                    .as_deref()
                    .is_some_and(|candidate| candidate.eq_ignore_ascii_case(name))
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return keep(&format!(
                "DOCPROPERTY target {name} was not found or is ambiguous"
            ));
        }
        match custom_property_text(&matches[0].value) {
            Some(value) => FieldOutcome::Resolved(value),
            None => keep(&format!(
                "DOCPROPERTY target {name} has an unsupported value"
            )),
        }
    }

    fn core_property(&self, name: &str) -> Option<&str> {
        let properties = self.document.core_properties.as_ref()?;
        match normalized_name(name).as_str() {
            "title" => properties.title.as_deref(),
            "subject" => properties.subject.as_deref(),
            "author" | "creator" => properties.creator.as_deref(),
            "comments" | "description" => properties.description.as_deref(),
            "keywords" => properties.keywords.as_deref(),
            "lastsavedby" | "lastmodifiedby" => properties.last_modified_by.as_deref(),
            "createtime" | "created" => properties.created.as_deref(),
            "savedate" | "modified" => properties.modified.as_deref(),
            _ => None,
        }
    }

    fn evaluate_docvariable(&self, instruction: &FieldInstruction) -> FieldOutcome {
        let Some(name) = text_argument(instruction, 0) else {
            return keep("DOCVARIABLE requires a variable name");
        };
        let matches = self
            .document
            .settings
            .iter()
            .flat_map(|settings| settings.document_variables())
            .filter(|variable| variable.name.eq_ignore_ascii_case(name))
            .collect::<Vec<_>>();
        if matches.len() == 1 {
            FieldOutcome::Resolved(matches[0].value.clone())
        } else {
            keep(&format!(
                "DOCVARIABLE target {name} was not found or is ambiguous"
            ))
        }
    }

    fn evaluate_styleref(
        &self,
        instruction: &FieldInstruction,
        _paragraphs: &[&CT_P],
        _paragraph_index: usize,
    ) -> FieldOutcome {
        let Some(target) = text_argument(instruction, 0) else {
            return keep("STYLEREF requires a style name");
        };
        let Some(layout) = &self.numbering_layout else {
            return keep("STYLEREF source layout is unavailable");
        };
        if let Some(failure) = layout.navigation_failure() {
            return keep(&format!(
                "STYLEREF navigation source discovery unavailable: {failure}"
            ));
        }
        let Some(field_source) = self.current_field_source else {
            return keep("STYLEREF field lacks an accepted physical binding");
        };
        let Some(caller) = self.numbering_source(field_source.node) else {
            return keep("STYLEREF field source is unavailable in this layout");
        };
        let caller_path = layout.source_node(caller).expect("registered caller");
        let key = target.to_lowercase();
        let candidates = self.navigation_styles.get(&key);
        let source = if caller_path.story == oxoffice_docx_layout::WordStory::Document
            && !has_switch(instruction, "l")
        {
            candidates.and_then(|nodes| {
                let boundary = nodes.partition_point(|node| node.get() < caller.get());
                boundary
                    .checked_sub(1)
                    .and_then(|i| nodes.get(i))
                    .or_else(|| nodes.iter().find(|node| **node != caller))
                    .copied()
            })
        } else {
            let source = oxoffice_layout::FieldSource {
                node: caller,
                index: field_source.index,
            };
            let Some(pages) = self
                .navigation_field_pages
                .get(&source)
                .filter(|pages| pages.len() == 1)
            else {
                return keep("STYLEREF page selection requires one actual placement");
            };
            let page = *pages.first().expect("one actual placement");
            self.navigation_style_pages.get(&key).and_then(|pages| {
                pages
                    .get(&page)
                    .and_then(|nodes| {
                        if has_switch(instruction, "l") {
                            nodes.iter().rev().find(|node| **node != caller)
                        } else {
                            nodes.iter().find(|node| **node != caller)
                        }
                    })
                    .copied()
                    .or_else(|| {
                        pages
                            .range(..page)
                            .next_back()
                            .and_then(|(_, nodes)| nodes.last())
                            .copied()
                    })
                    .or_else(|| {
                        pages
                            .range((std::ops::Bound::Excluded(page), std::ops::Bound::Unbounded))
                            .next()
                            .and_then(|(_, nodes)| nodes.first())
                            .copied()
                    })
            })
        };
        let Some(node) = source else {
            return keep(&format!("STYLEREF target {target} was not found"));
        };
        let Some((path, _, text, _, numbering, placed)) = layout.paragraph_navigation_source(node)
        else {
            return keep("STYLEREF selected source is unavailable");
        };
        if placed.is_none() {
            return keep("STYLEREF selected source is unplaced");
        }
        let numbered = ["n", "r", "w"]
            .iter()
            .any(|name| has_switch(instruction, name));
        let mut value = if numbered {
            match numbering {
                Some(numbering) => match numbering
                    .numbered_reference_text(instruction, layout.paragraph_numbering(caller))
                {
                    Ok(value) => value,
                    Err(diagnostic) => return keep(&diagnostic),
                },
                None if layout.paragraph_numbering_suppressed(node) => "0".into(),
                None => return keep("STYLEREF source numbering is unavailable"),
            }
        } else {
            text.to_owned()
        };
        if has_switch(instruction, "p") {
            if path.story != caller_path.story {
                return keep("STYLEREF relative position lacks a comparable physical story");
            }
            value.push_str(if path.children < caller_path.children {
                " above"
            } else {
                " below"
            });
        }
        FieldOutcome::Resolved(value)
    }

    fn evaluate_includetext(&self, instruction: &FieldInstruction) -> FieldOutcome {
        if has_switch(instruction, "c") {
            return keep("INCLUDETEXT converter selection is unsupported");
        }
        let Some(source) = text_argument(instruction, 0) else {
            return keep("INCLUDETEXT requires a source name");
        };
        let key = text_argument(instruction, 1)
            .map(|bookmark| format!("{source}#{bookmark}"))
            .unwrap_or_else(|| source.to_owned());
        match self.context.included_text.get(&key) {
            Some(value) => FieldOutcome::Resolved(value.clone()),
            None => keep(&format!("INCLUDETEXT input {key} was not supplied")),
        }
    }

    fn evaluate_date_time(&self, instruction: &FieldInstruction) -> FieldOutcome {
        let Some(now) = self.context.now else {
            return keep(&format!(
                "{} requires an explicit date and time",
                instruction.name
            ));
        };
        if !valid_date_time(now) {
            return keep("field date and time is invalid");
        }
        let default_picture = if instruction.name == "DATE" {
            "M/d/yyyy"
        } else {
            "h:mm:ss AM/PM"
        };
        let picture = switch_text(instruction, "@").unwrap_or(default_picture);
        match format_date_time(now, picture) {
            Ok(value) => FieldOutcome::Resolved(value),
            Err(diagnostic) => keep(&diagnostic),
        }
    }

    fn evaluate_filename(&self, instruction: &FieldInstruction) -> FieldOutcome {
        if has_switch(instruction, "p") {
            return self
                .context
                .file_path
                .clone()
                .map(FieldOutcome::Resolved)
                .unwrap_or_else(|| keep("FILENAME path was not supplied"));
        }
        self.context
            .file_name
            .clone()
            .or_else(|| {
                self.context
                    .file_path
                    .as_deref()
                    .and_then(lexical_file_name)
                    .map(str::to_owned)
            })
            .map(FieldOutcome::Resolved)
            .unwrap_or_else(|| keep("FILENAME name was not supplied"))
    }

    fn evaluate_author(&self) -> FieldOutcome {
        self.document
            .core_properties
            .as_ref()
            .and_then(|properties| properties.creator.clone())
            .map(FieldOutcome::Resolved)
            .unwrap_or_else(|| keep("AUTHOR metadata was not found"))
    }

    fn evaluate_mergefield(&self, instruction: &FieldInstruction) -> FieldOutcome {
        let value = text_argument(instruction, 0)
            .and_then(|name| self.context.merge_fields.get(name))
            .map(String::as_str);
        match resolve_mergefield_text(instruction, value, self.missing_merge_fields_as_empty) {
            Ok(value) => FieldOutcome::Resolved(value),
            Err(diagnostic) => keep(&diagnostic),
        }
    }

    fn evaluate_formula(
        &mut self,
        instruction: &FieldInstruction,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
    ) -> FieldOutcome {
        if instruction.arguments.is_empty() {
            return keep("formula requires an expression");
        }
        let mut expression = String::new();
        for argument in &instruction.arguments {
            let value = match self.resolve_argument(argument, story, paragraphs, paragraph_index) {
                Ok(value) => value,
                Err(diagnostic) => return keep(&diagnostic),
            };
            if !expression.is_empty() {
                expression.push(' ');
            }
            expression.push_str(&value);
            if expression.len() > MAX_FORMULA_BYTES {
                return keep("formula exceeds the 4096-byte limit");
            }
        }
        match FormulaParser::new(&expression).and_then(FormulaParser::parse) {
            Ok(value) => FieldOutcome::Resolved(format_formula_value(value)),
            Err(diagnostic) => keep(&diagnostic),
        }
    }

    fn evaluate_mail_merge_control(
        &mut self,
        instruction: &FieldInstruction,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
    ) -> FieldOutcome {
        match instruction.name.as_str() {
            "MERGESEQ" if self.context.merge_sequence_number == Some(0) => {
                return keep("MERGESEQ merge sequence number must be one-based");
            }
            "NEXT" | "NEXTIF" | "SKIPIF" | "MERGEREC"
                if self.context.merge_record_number == Some(0) =>
            {
                return keep(&format!(
                    "{} merge record number must be one-based",
                    instruction.name
                ));
            }
            _ => {}
        }
        let condition = match instruction.name.as_str() {
            "NEXTIF" | "SKIPIF" => {
                let values = instruction
                    .arguments
                    .iter()
                    .map(|argument| {
                        self.resolve_argument(argument, story, paragraphs, paragraph_index)
                    })
                    .collect::<std::result::Result<Vec<_>, _>>();
                let values = match values {
                    Ok(values) => values,
                    Err(diagnostic) => return keep(&diagnostic),
                };
                let Some(condition) = compare_if(&values[0], &values[1], &values[2]) else {
                    return keep(&format!(
                        "{} operator {} is unsupported",
                        instruction.name, values[1]
                    ));
                };
                condition
            }
            _ => false,
        };
        let state =
            self.mail_merge_stories
                .entry(story.to_owned())
                .or_insert(MailMergeStoryState {
                    record_number: self.context.merge_record_number,
                    sequence_number: self.context.merge_sequence_number,
                });
        match instruction.name.as_str() {
            "NEXT" => {
                let Some(current_record) = state.record_number else {
                    return keep("NEXT requires an explicit merge record number");
                };
                let Some(record_number) = current_record.checked_add(1) else {
                    return keep("NEXT record number overflowed");
                };
                state.record_number = Some(record_number);
                FieldOutcome::MailMergeControl(MailMergeControl::NextRecord { record_number })
            }
            "NEXTIF" => {
                let Some(mut record_number) = state.record_number else {
                    return keep("NEXTIF requires an explicit merge record number");
                };
                if condition {
                    let Some(next_record_number) = record_number.checked_add(1) else {
                        return keep("NEXTIF record number overflowed");
                    };
                    record_number = next_record_number;
                    state.record_number = Some(record_number);
                }
                FieldOutcome::MailMergeControl(MailMergeControl::NextRecordIf {
                    condition,
                    record_number,
                })
            }
            "SKIPIF" => match state.record_number {
                Some(record_number) => {
                    FieldOutcome::MailMergeControl(MailMergeControl::SkipRecordIf {
                        condition,
                        record_number,
                    })
                }
                None => keep("SKIPIF requires an explicit merge record number"),
            },
            "MERGEREC" => state
                .record_number
                .map(MailMergeControl::RecordNumber)
                .map(FieldOutcome::MailMergeControl)
                .unwrap_or_else(|| keep("MERGEREC requires an explicit merge record number")),
            "MERGESEQ" => state
                .sequence_number
                .map(MailMergeControl::SequenceNumber)
                .map(FieldOutcome::MailMergeControl)
                .unwrap_or_else(|| keep("MERGESEQ requires an explicit merge sequence number")),
            _ => unreachable!(),
        }
    }

    fn evaluate_barcode(
        &mut self,
        instruction: &FieldInstruction,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
    ) -> FieldOutcome {
        let Some(source) = instruction.arguments.first() else {
            return keep(&format!("{} requires a value", instruction.name));
        };
        let value = match source {
            FieldArgument::Text(source) if instruction.name == "MERGEBARCODE" => {
                match self.context.merge_fields.get(source) {
                    Some(value) => value.clone(),
                    None => return keep(&format!("MERGEBARCODE input {source} was not supplied")),
                }
            }
            _ => match self.resolve_argument(source, story, paragraphs, paragraph_index) {
                Ok(value) => value,
                Err(diagnostic) => return keep(&diagnostic),
            },
        };
        let Some(kind) = instruction.arguments.get(1) else {
            return keep(&format!("{} requires a barcode type", instruction.name));
        };
        let kind = match self.resolve_argument(kind, story, paragraphs, paragraph_index) {
            Ok(value) => value,
            Err(diagnostic) => return keep(&diagnostic),
        };
        let mut switches = Vec::with_capacity(instruction.switches.len());
        for field_switch in &instruction.switches {
            let argument = match &field_switch.argument {
                Some(argument) => {
                    match self.resolve_argument(argument, story, paragraphs, paragraph_index) {
                        Ok(value) => Some(value),
                        Err(diagnostic) => return keep(&diagnostic),
                    }
                }
                None => None,
            };
            switches.push((field_switch.name.clone(), argument));
        }
        match parse_barcode(&instruction.name, &switches, value, &kind) {
            Ok(barcode) => FieldOutcome::Barcode(barcode),
            Err(diagnostic) => keep(&diagnostic),
        }
    }

    fn evaluate_toc(
        &mut self,
        instruction: &FieldInstruction,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
    ) -> FieldOutcome {
        let mut toc = TocField {
            heading_levels: None,
            custom_styles: Vec::new(),
            entries: TocEntrySelection::None,
            sequence_identifier: None,
            bookmark: None,
            hyperlink: false,
            use_outline_levels: false,
            omit_page_number_levels: None,
            page_number_separator: None,
            entry_page_separator: None,
        };
        let mut has_explicit_source = false;
        for field_switch in &instruction.switches {
            let argument = match &field_switch.argument {
                Some(argument) => {
                    match self.resolve_argument(argument, story, paragraphs, paragraph_index) {
                        Ok(value) => Some(value),
                        Err(diagnostic) => return keep(&diagnostic),
                    }
                }
                None => None,
            };
            match field_switch.name.as_str() {
                "h" if argument.is_none() => toc.hyperlink = true,
                "z" if argument.is_none() => {}
                "u" if argument.is_none() => {
                    toc.use_outline_levels = true;
                    has_explicit_source = true;
                }
                "o" => {
                    has_explicit_source = true;
                    toc.heading_levels = match argument {
                        Some(value) => match parse_level_range(&value, "TOC heading") {
                            Ok(levels) => Some(levels),
                            Err(diagnostic) => return keep(&diagnostic),
                        },
                        None => Some((1, 9)),
                    };
                }
                "n" => {
                    toc.omit_page_number_levels = match argument {
                        Some(value) => match parse_level_range(&value, "TOC omitted page-number") {
                            Ok(levels) => Some(levels),
                            Err(diagnostic) => return keep(&diagnostic),
                        },
                        None => Some((1, 9)),
                    };
                }
                "t" => {
                    let Some(value) = argument else {
                        return keep("field TOC switch \\t requires a text argument");
                    };
                    has_explicit_source = true;
                    toc.custom_styles = match parse_custom_styles(&value) {
                        Ok(styles) => styles,
                        Err(diagnostic) => return keep(&diagnostic),
                    };
                }
                "f" => {
                    has_explicit_source = true;
                    toc.entries = argument.map_or(TocEntrySelection::All, |value| {
                        TocEntrySelection::Identifier(value)
                    });
                }
                "b" | "p" | "s" | "d" => {
                    let Some(value) = argument else {
                        return keep(&format!(
                            "field TOC switch \\{} requires a text argument",
                            field_switch.name
                        ));
                    };
                    match field_switch.name.as_str() {
                        "b" => toc.bookmark = Some(value),
                        "p" if value.chars().count() == 1 => {
                            toc.page_number_separator = Some(value)
                        }
                        "p" => {
                            return keep(
                                "TOC page-number separator must contain exactly one character",
                            );
                        }
                        "s" if !value.is_empty() => toc.sequence_identifier = Some(value),
                        "s" => return keep("TOC sequence identifier must not be empty"),
                        "d" => toc.entry_page_separator = Some(value),
                        _ => unreachable!(),
                    }
                }
                name if argument.is_some() => {
                    return keep(&format!(
                        "field TOC switch \\{name} does not take an argument"
                    ));
                }
                name => return keep(&format!("field TOC uses unsupported switch \\{name}")),
            }
        }
        if !has_explicit_source {
            toc.heading_levels = Some((1, 9));
        }
        FieldOutcome::TableOfContents(toc)
    }

    fn evaluate_tc(
        &mut self,
        instruction: &FieldInstruction,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
    ) -> FieldOutcome {
        let Some(entry) = instruction.arguments.first() else {
            return keep("TC requires entry text");
        };
        let entry = match self.resolve_argument(entry, story, paragraphs, paragraph_index) {
            Ok(entry) => entry,
            Err(diagnostic) => return keep(&diagnostic),
        };
        if entry.is_empty() {
            return keep("TC entry text must not be empty");
        }
        let mut tc = TcField {
            entry,
            level: 1,
            table_identifier: None,
            omit_page_number: false,
        };
        for field_switch in &instruction.switches {
            let argument = match &field_switch.argument {
                Some(argument) => {
                    match self.resolve_argument(argument, story, paragraphs, paragraph_index) {
                        Ok(value) => Some(value),
                        Err(diagnostic) => return keep(&diagnostic),
                    }
                }
                None => None,
            };
            match field_switch.name.as_str() {
                "n" if argument.is_none() => tc.omit_page_number = true,
                "f" => match argument {
                    Some(value) if !value.is_empty() => tc.table_identifier = Some(value),
                    Some(_) => return keep("TC table identifier must not be empty"),
                    None => return keep("field TC switch \\f requires a text argument"),
                },
                "l" => {
                    let Some(value) = argument else {
                        return keep("field TC switch \\l requires a text argument");
                    };
                    tc.level = match parse_toc_level(&value, "TC") {
                        Ok(level) => level,
                        Err(diagnostic) => return keep(&diagnostic),
                    };
                }
                name if argument.is_some() => {
                    return keep(&format!(
                        "field TC switch \\{name} does not take an argument"
                    ));
                }
                name => return keep(&format!("field TC uses unsupported switch \\{name}")),
            }
        }
        FieldOutcome::TableOfContentsEntry(tc)
    }
}

fn referenced_header_footer_parts(
    document: &Document,
    is_header: bool,
    include_control_sections: bool,
) -> Vec<(String, Vec<u8>)> {
    let Some(relationships) = document.package.get_part_rels(&document.doc_part_name) else {
        return Vec::new();
    };
    let relationship_type = if is_header {
        rel_types::HEADER
    } else {
        rel_types::FOOTER
    };
    // Pagination inventories modeled block-control section endings. General
    // field evaluation retains its existing direct-body story discovery.
    let sections = if include_control_sections {
        oxoffice_docx_layout::engine::document_sections(&document.document)
    } else {
        document
            .document
            .body
            .content
            .iter()
            .filter_map(|content| match content {
                BodyContent::Paragraph(paragraph) => paragraph
                    .properties
                    .as_ref()
                    .and_then(|properties| properties.sect_pr.as_ref()),
                _ => None,
            })
            .chain(document.document.body.sect_pr.as_ref())
            .collect()
    };
    let mut seen = HashSet::new();
    let mut parts = Vec::new();
    for section in sections {
        let references = section_header_footer_references(section, is_header);
        for reference in references {
            let Some(relationship) = relationships.get_by_id(&reference.rel_id) else {
                continue;
            };
            if relationship.rel_type != relationship_type
                || !crate::document::relationship_is_internal(relationship)
            {
                continue;
            }
            let part_name =
                OpcPackage::resolve_rel_target(&document.doc_part_name, &relationship.target);
            if seen.insert(part_name.clone())
                && let Some(xml) = document.package.get_part(&part_name)
            {
                parts.push((part_name, xml.to_vec()));
            }
        }
    }
    parts
}

fn section_header_footer_references(
    section: &CT_SectPr,
    is_header: bool,
) -> &[oxoffice_docx_oxml::header_footer::HdrFtrRef] {
    if is_header {
        &section.header_refs
    } else {
        &section.footer_refs
    }
}

fn relationship_parts(document: &Document, relationship_type: &str) -> Vec<(String, Vec<u8>)> {
    let Some(relationships) = document.package.get_part_rels(&document.doc_part_name) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    relationships
        .items
        .iter()
        .filter(|relationship| relationship.rel_type == relationship_type)
        .filter(|relationship| crate::document::relationship_is_internal(relationship))
        .filter_map(|relationship| {
            let part_name =
                OpcPackage::resolve_rel_target(&document.doc_part_name, &relationship.target);
            if !seen.insert(part_name.clone()) {
                return None;
            }
            Some((
                part_name.clone(),
                document.package.get_part(&part_name)?.to_vec(),
            ))
        })
        .collect()
}

/// All immutable field occurrences keyed by paragraph path and preorder index.
type PlacedPageFields = HashMap<
    (oxoffice_docx_layout::WordSourcePath, u32),
    Vec<oxoffice_docx_layout::WordFieldPlacement>,
>;

fn placed_page_fields(layout: &oxoffice_docx_layout::WordLayoutResult) -> PlacedPageFields {
    let mut placed = PlacedPageFields::new();
    for &placement in layout.field_placements() {
        if let Some(path) = layout.source_node(placement.source.node) {
            placed
                .entry((path.clone(), placement.source.index))
                .or_default()
                .push(placement);
        }
    }
    placed
}

fn collect_preorder_fields<'a>(
    field: &'a Field,
    locked: bool,
    fields: &mut Vec<(&'a Field, bool)>,
) {
    let locked = locked || field.locked() == Some(true);
    fields.push((field, locked));
    for nested in field.all_nested_fields_in_source_order() {
        collect_preorder_fields(nested, locked, fields);
    }
}

#[allow(clippy::too_many_arguments)]
fn push_page_field_updates(
    paragraphs: &[&CT_P],
    paths: &[Vec<oxoffice_docx_layout::WordSourcePath>],
    placed: &PlacedPageFields,
    layout: &oxoffice_docx_layout::WordLayoutResult,
    section_formats: &[Option<String>],
    updates: &mut Vec<Option<CachedFieldUpdate>>,
    report: &mut LayoutBackedFieldUpdateReport,
) {
    for (paragraph, paths) in paragraphs.iter().zip(paths) {
        let mut physical_fields = Vec::new();
        for run in paragraph.source_runs() {
            for content in &run.content {
                if let RunContent::Field(field) = content {
                    collect_preorder_fields(field, false, &mut physical_fields);
                }
            }
        }
        let physical_indices = physical_fields
            .iter()
            .enumerate()
            .map(|(index, (field, _))| (std::ptr::from_ref(*field), index as u32))
            .collect::<HashMap<_, _>>();
        let mut fields = Vec::new();
        for field in accepted_toc_runs(paragraph)
            .into_iter()
            .flat_map(|run| &run.run.content)
            .filter_map(|content| match content {
                RunContent::Field(field) => Some(field),
                _ => None,
            })
        {
            collect_preorder_fields(field, false, &mut fields);
        }
        for (field, locked) in fields {
            let Some(&index) = physical_indices.get(&std::ptr::from_ref(field)) else {
                report.diagnostics.push("field cache retained because its accepted owner has no physical source identity".into());
                updates.push(None);
                continue;
            };
            let instruction = field.effective_instruction();
            let name = instruction.name.as_str();
            if !matches!(
                name,
                "PAGE" | "NUMPAGES" | "SECTION" | "SECTIONPAGES" | "PAGEREF"
            ) {
                updates.push(None);
                continue;
            }
            let retained = |report: &mut LayoutBackedFieldUpdateReport, reason: &str| {
                report
                    .diagnostics
                    .push(format!("{name} field {index} retained cache: {reason}"))
            };
            if locked {
                retained(report, "locked owner");
                updates.push(None);
                continue;
            }
            let placement = paths
                .iter()
                .filter_map(|path| placed.get(&(path.clone(), index)))
                .flatten()
                .min_by_key(|placement| placement.physical_page);
            if placement.is_none() && name != "PAGEREF" {
                retained(report, "source did not reach pagination");
                updates.push(None);
                continue;
            }
            if matches!(name, "PAGE" | "NUMPAGES")
                && paths.iter().any(|path| {
                    matches!(
                        path.story,
                        oxoffice_docx_layout::WordStory::Header { .. }
                            | oxoffice_docx_layout::WordStory::Footer { .. }
                    )
                })
            {
                retained(
                    report,
                    "Word preserves dynamic header and footer caches on update and save",
                );
                updates.push(None);
                continue;
            }
            if let Some(switch) = unsupported_switch(&instruction) {
                retained(report, &format!("unsupported switch \\{switch}"));
                updates.push(None);
                continue;
            }
            if let Err(reason) = validate_instruction_shape(&instruction) {
                retained(report, &reason);
                updates.push(None);
                continue;
            }
            let value = match name {
                "PAGE" => placement.map(|placement| placement.displayed_page),
                "NUMPAGES" => Some(layout.layout.pages.len()),
                "SECTION" => placement.map(|placement| placement.section_index + 1),
                "SECTIONPAGES" => placement.map(|placement| {
                    layout
                        .page_sections()
                        .iter()
                        .filter(|record| record.section_index == placement.section_index)
                        .map(|record| record.physical_page)
                        .collect::<HashSet<_>>()
                        .len()
                }),
                "PAGEREF" => {
                    text_argument(&instruction, 0).and_then(|name| layout.bookmark_page(name))
                }
                _ => None,
            };
            let Some(value) = value else {
                retained(report, "bookmark target is missing, ambiguous or unplaced");
                updates.push(None);
                continue;
            };
            let cached_result = oxoffice_docx_layout::engine::format_page_field_result(
                &instruction,
                value,
                if name == "PAGEREF" {
                    text_argument(&instruction, 0)
                        .and_then(|target| layout.bookmark_page_section(target))
                        .and_then(|target| {
                            section_formats
                                .get(target.section_index)
                                .and_then(Option::as_deref)
                        })
                } else if name == "PAGE" {
                    placement.and_then(|placement| {
                        section_formats
                            .get(placement.section_index)
                            .and_then(Option::as_deref)
                    })
                } else {
                    Some("decimal")
                },
            );
            match cached_result {
                Ok(cached_result) => {
                    match name {
                        "PAGE" => report.page_fields += 1,
                        "NUMPAGES" => report.num_pages_fields += 1,
                        "SECTION" => report.section_fields += 1,
                        "SECTIONPAGES" => report.section_pages_fields += 1,
                        "PAGEREF" => report.page_reference_fields += 1,
                        _ => {}
                    }
                    updates.push(Some(CachedFieldUpdate {
                        typed_runs: None,
                        comment_ranges: Vec::new(),
                        cached_result,
                        dirty: false,
                    }));
                }
                Err(reason) => {
                    retained(report, &reason);
                    updates.push(None);
                }
            }
        }
    }
}

fn section_page_formats(document: &CT_Document) -> Vec<Option<String>> {
    let mut formats = oxoffice_docx_layout::engine::document_sections(document)
        .into_iter()
        .map(oxoffice_docx_layout::engine::section_page_number_format)
        .collect::<Vec<_>>();
    if document.body.sect_pr.is_none() {
        formats.push(Some("decimal".into()));
    }
    formats
}

#[derive(Clone, Copy)]
enum PackageStoryKind {
    Document,
    Header,
    Footer,
    Footnotes,
    Endnotes,
    Comments,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyBlockOwner {
    Root,
    Table,
    Row,
    Cell,
}

#[derive(Clone, Copy)]
enum LegacyStoryElementKind {
    DocumentRoot,
    BodyRoot,
    HeaderFooterRoot,
    FootnotesRoot,
    EndnotesRoot,
    CommentsRoot,
    NormalComment,
    NormalNote,
    Table,
    Row,
    Cell,
    ContentControl(LegacyBlockOwner, bool),
    ContentControlContent(LegacyBlockOwner),
    Other,
}

struct LegacyStoryParagraph {
    path: Vec<(&'static str, usize)>,
    start: usize,
    end: usize,
    general_context: bool,
    original: CT_P,
    paragraph: CT_P,
}

fn parsed_physical_field_paragraph(xml: &[u8]) -> Result<CT_P> {
    let mut body = CT_Body::default();
    body.content
        .push(BodyContent::Paragraph(Box::new(CT_P::from_xml_fragment(
            xml,
        )?)));
    prepare_physical_story_projection(&mut body, &mut [])?;
    let BodyContent::Paragraph(paragraph) = body.content.remove(0) else {
        unreachable!()
    };
    Ok(*paragraph)
}

fn legacy_story_paragraphs(
    xml: &[u8],
    story_kind: PackageStoryKind,
) -> Result<Vec<LegacyStoryParagraph>> {
    validate_story_document_declarations_and_doctype(xml)?;
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut stack = Vec::<LegacyStoryElementKind>::new();
    let mut bindings = BTreeMap::<String, Vec<u8>>::new();
    let mut binding_scopes = Vec::new();
    let mut paragraphs = Vec::new();
    let mut path = Vec::new();
    let mut path_lengths = Vec::new();
    let mut child_counts = Vec::<usize>::new();
    let mut root_seen = false;
    let mut root_closed = false;
    let general_context = |depth: usize| match story_kind {
        PackageStoryKind::Header | PackageStoryKind::Footer => depth == 1,
        PackageStoryKind::Footnotes | PackageStoryKind::Endnotes => depth == 2,
        PackageStoryKind::Comments | PackageStoryKind::Document => false,
    };
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid package story XML: {error}")))?;
        match event {
            Event::Start(element) => {
                let word = namespace_is_word(&namespace);
                let name = element.name();
                let local = local_name(name.as_ref());
                if stack.is_empty() {
                    if root_seen || !package_story_root_matches(story_kind, word, local) {
                        return Err(Error::Other(
                            "package story must contain exactly one relationship-appropriate root"
                                .to_owned(),
                        ));
                    }
                    root_seen = true;
                }
                let kind = legacy_story_element_kind(
                    story_kind,
                    &stack,
                    word,
                    local,
                    &element,
                    reader.resolver(),
                );
                if matches!(kind, Some(LegacyStoryElementKind::Other))
                    && word
                    && (local == b"p"
                        || (local == b"sdtContent"
                            && matches!(
                                stack.last(),
                                Some(LegacyStoryElementKind::ContentControl(_, true))
                            )))
                {
                    reader
                        .read_to_end_into(element.name(), &mut Vec::new())
                        .map_err(|error| {
                            Error::Other(format!("invalid package story paragraph: {error}"))
                        })?;
                } else if kind.is_none() && word && local == b"p" {
                    let start_after = reader.buffer_position() as usize;
                    reader
                        .read_to_end_into(element.name(), &mut Vec::new())
                        .map_err(|error| {
                            Error::Other(format!("invalid package story paragraph: {error}"))
                        })?;
                    let end = reader.buffer_position() as usize;
                    let fragment = paragraph_fragment_with_bindings(
                        &xml[before..end],
                        start_after - before,
                        &bindings,
                    )?;
                    let paragraph = parsed_physical_field_paragraph(&fragment)?;
                    let index = child_counts.last_mut().ok_or_else(|| {
                        Error::Other("field paragraph has no source owner".into())
                    })?;
                    let mut paragraph_path = path.clone();
                    paragraph_path.push(("p", *index));
                    *index = index
                        .checked_add(1)
                        .ok_or_else(|| Error::Other("field source path overflow".into()))?;
                    paragraphs.push(LegacyStoryParagraph {
                        path: paragraph_path,
                        start: before,
                        end,
                        general_context: general_context(stack.len()),
                        original: paragraph.clone(),
                        paragraph,
                    });
                } else {
                    if matches!(kind, Some(LegacyStoryElementKind::ContentControlContent(_)))
                        && let Some(LegacyStoryElementKind::ContentControl(_, seen)) =
                            stack.last_mut()
                    {
                        *seen = true;
                    }
                    let mut local_bindings = bindings.clone();
                    apply_namespace_declarations(&element, &mut local_bindings)?;
                    binding_scopes.push(std::mem::replace(&mut bindings, local_bindings));
                    let kind = kind.unwrap_or(LegacyStoryElementKind::Other);
                    path_lengths.push(path.len());
                    let token = match kind {
                        LegacyStoryElementKind::Table => Some("tbl"),
                        LegacyStoryElementKind::Row => Some("tr"),
                        LegacyStoryElementKind::Cell => Some("tc"),
                        LegacyStoryElementKind::ContentControl(_, _) => Some("sdt"),
                        _ => None,
                    };
                    if let Some(token) = token {
                        let index = child_counts.last_mut().ok_or_else(|| {
                            Error::Other("field source container has no parent".into())
                        })?;
                        path.push((token, *index));
                        *index = index
                            .checked_add(1)
                            .ok_or_else(|| Error::Other("field source path overflow".into()))?;
                    }
                    child_counts.push(0);
                    stack.push(kind);
                }
            }
            Event::End(_) => {
                if stack.pop().is_none() {
                    return Err(Error::Other(
                        "package story XML has an unmatched end element".to_owned(),
                    ));
                }
                bindings = binding_scopes.pop().unwrap_or_default();
                path.truncate(
                    path_lengths
                        .pop()
                        .ok_or_else(|| Error::Other("field source path stack underflow".into()))?,
                );
                child_counts.pop();
                if stack.is_empty() {
                    root_closed = true;
                }
            }
            // A self-closing paragraph is a story paragraph too, as the
            // layout source inventory counts it.
            Event::Empty(element) if !stack.is_empty() => {
                let word = namespace_is_word(&namespace);
                let name = element.name();
                let local = local_name(name.as_ref());
                if word
                    && local == b"p"
                    && legacy_story_element_kind(
                        story_kind,
                        &stack,
                        word,
                        local,
                        &element,
                        reader.resolver(),
                    )
                    .is_none()
                {
                    let end = reader.buffer_position() as usize;
                    let fragment = paragraph_fragment_with_bindings(
                        &xml[before..end],
                        end - before,
                        &bindings,
                    )?;
                    let paragraph = parsed_physical_field_paragraph(&fragment)?;
                    let index = child_counts.last_mut().ok_or_else(|| {
                        Error::Other("field paragraph has no source owner".into())
                    })?;
                    let mut paragraph_path = path.clone();
                    paragraph_path.push(("p", *index));
                    *index = index
                        .checked_add(1)
                        .ok_or_else(|| Error::Other("field source path overflow".into()))?;
                    paragraphs.push(LegacyStoryParagraph {
                        path: paragraph_path,
                        start: before,
                        end,
                        general_context: general_context(stack.len()),
                        original: paragraph.clone(),
                        paragraph,
                    });
                } else if matches!(
                    legacy_story_element_kind(
                        story_kind,
                        &stack,
                        word,
                        local,
                        &element,
                        reader.resolver()
                    ),
                    Some(
                        LegacyStoryElementKind::Table
                            | LegacyStoryElementKind::Row
                            | LegacyStoryElementKind::Cell
                            | LegacyStoryElementKind::ContentControl(_, _)
                    )
                ) {
                    let index = child_counts.last_mut().ok_or_else(|| {
                        Error::Other("field source empty container has no owner".into())
                    })?;
                    *index = index
                        .checked_add(1)
                        .ok_or_else(|| Error::Other("field source path overflow".into()))?;
                }
            }
            Event::Empty(element) if stack.is_empty() => {
                let word = namespace_is_word(&namespace);
                let name = element.name();
                let local = local_name(name.as_ref());
                if root_seen || !package_story_root_matches(story_kind, word, local) {
                    return Err(Error::Other(
                        "package story must contain exactly one relationship-appropriate root"
                            .to_owned(),
                    ));
                }
                root_seen = true;
                root_closed = true;
            }
            Event::Text(text)
                if stack.is_empty() && text.iter().any(|byte| !byte.is_ascii_whitespace()) =>
            {
                return Err(Error::Other(
                    "package story has non-whitespace text outside its root".to_owned(),
                ));
            }
            Event::CData(_) if stack.is_empty() => {
                return Err(Error::Other(
                    "package story has character data outside its root".to_owned(),
                ));
            }
            Event::GeneralRef(_) if stack.is_empty() => {
                return Err(Error::Other(
                    "package story has a character reference outside its root".to_owned(),
                ));
            }
            Event::Eof => {
                if !stack.is_empty() {
                    return Err(Error::Other(
                        "package story XML has an unclosed element".to_owned(),
                    ));
                }
                if !root_seen || !root_closed {
                    return Err(Error::Other(
                        "package story must contain exactly one relationship-appropriate root"
                            .to_owned(),
                    ));
                }
                return Ok(paragraphs);
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn validate_story_document_declarations_and_doctype(xml: &[u8]) -> Result<()> {
    validate_strict_xml_1_0(xml).map_err(package_story_lexical_error)?;
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut declaration_allowed = true;
    let mut declaration_seen = false;
    let mut depth = 0usize;
    loop {
        let (_, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid package story XML: {error}")))?;
        let event = event.into_owned();
        match event {
            Event::Decl(_) => {
                if !declaration_allowed || declaration_seen {
                    return Err(Error::Other(
                        "misplaced or duplicate package story XML declaration".to_owned(),
                    ));
                }
                declaration_seen = true;
                declaration_allowed = false;
            }
            Event::DocType(_) => {
                return Err(Error::Other(
                    "package story XML cannot contain a document type".to_owned(),
                ));
            }
            Event::Start(_) => {
                declaration_allowed = false;
                depth += 1;
            }
            Event::Empty(_) => {
                declaration_allowed = false;
            }
            Event::End(_) => {
                declaration_allowed = false;
                depth = depth.saturating_sub(1);
            }
            Event::GeneralRef(_) => {
                declaration_allowed = false;
            }
            Event::Eof => return Ok(()),
            _ => declaration_allowed = false,
        }
        buffer.clear();
    }
}

fn package_story_lexical_error(error: XmlLexicalError) -> Error {
    let message = match error {
        XmlLexicalError::InvalidUtf8 => "invalid package story XML: input is not UTF-8".to_owned(),
        XmlLexicalError::InvalidDeclaration(reason) if reason == "must begin with version" => {
            "package story XML declaration must begin with version".to_owned()
        }
        XmlLexicalError::InvalidDeclaration(reason)
            if reason == "attributes are invalid, duplicated, or out of order" =>
        {
            "invalid package story XML declaration".to_owned()
        }
        XmlLexicalError::InvalidDeclaration(reason) => {
            format!("invalid package story XML declaration: {reason}")
        }
        XmlLexicalError::ForbiddenLiteralCharacter => {
            "package story XML contains a forbidden literal XML 1.0 character".to_owned()
        }
        XmlLexicalError::InvalidName(reason) if reason.starts_with("qualified name ") => {
            format!("invalid package story XML {reason}")
        }
        XmlLexicalError::InvalidName(reason) if reason.starts_with("name ") => {
            format!("invalid package story XML {reason}")
        }
        XmlLexicalError::InvalidName(reason) => format!("invalid package story XML: {reason}"),
        XmlLexicalError::InvalidNamespace(reason)
            if reason == "element uses the reserved xmlns prefix" =>
        {
            "package story XML element uses the reserved xmlns prefix".to_owned()
        }
        XmlLexicalError::InvalidNamespace(reason) if reason == "invalid namespace declaration" => {
            "package story XML contains an invalid namespace declaration".to_owned()
        }
        XmlLexicalError::InvalidNamespace(_) => {
            "package story XML uses an unbound namespace prefix".to_owned()
        }
        XmlLexicalError::DuplicateExpandedAttribute => {
            "package story XML element has duplicate expanded-name attributes".to_owned()
        }
        XmlLexicalError::InvalidReference(reason)
            if reason == "attribute contains a literal less-than sign" =>
        {
            "package story XML attribute contains a literal less-than sign".to_owned()
        }
        XmlLexicalError::InvalidReference(reason)
            if reason == "character reference is not legal in XML 1.0" =>
        {
            "package story character reference is not legal in XML 1.0".to_owned()
        }
        XmlLexicalError::InvalidReference(reason)
            if reason.starts_with("undeclared entity reference ") =>
        {
            reason.replacen(
                "undeclared entity reference ",
                "undeclared package story XML entity reference ",
                1,
            )
        }
        XmlLexicalError::InvalidReference(reason) => {
            format!("invalid package story XML: {reason}")
        }
        XmlLexicalError::InvalidProcessingInstruction(reason)
            if reason == "reserved XML target" =>
        {
            "reserved package story XML processing instruction".to_owned()
        }
        XmlLexicalError::InvalidProcessingInstruction(reason) if reason.starts_with("name ") => {
            format!("invalid package story XML {reason}")
        }
        XmlLexicalError::InvalidProcessingInstruction(reason) => {
            format!("invalid package story XML processing instruction: {reason}")
        }
        XmlLexicalError::InvalidComment(reason) => format!("invalid package story XML: {reason}"),
    };
    Error::Other(message)
}

fn legacy_story_element_kind(
    story_kind: PackageStoryKind,
    stack: &[LegacyStoryElementKind],
    word: bool,
    local: &[u8],
    element: &BytesStart<'_>,
    resolver: &NamespaceResolver,
) -> Option<LegacyStoryElementKind> {
    if stack.is_empty() {
        return Some(match story_kind {
            PackageStoryKind::Document if word && local == b"document" => {
                LegacyStoryElementKind::DocumentRoot
            }
            PackageStoryKind::Header if word && local == b"hdr" => {
                LegacyStoryElementKind::HeaderFooterRoot
            }
            PackageStoryKind::Footer if word && local == b"ftr" => {
                LegacyStoryElementKind::HeaderFooterRoot
            }
            PackageStoryKind::Footnotes if word && local == b"footnotes" => {
                LegacyStoryElementKind::FootnotesRoot
            }
            PackageStoryKind::Endnotes if word && local == b"endnotes" => {
                LegacyStoryElementKind::EndnotesRoot
            }
            PackageStoryKind::Comments if word && local == b"comments" => {
                LegacyStoryElementKind::CommentsRoot
            }
            _ => LegacyStoryElementKind::Other,
        });
    }
    if word
        && matches!(
            (story_kind, stack.last()),
            (
                PackageStoryKind::Footnotes,
                Some(LegacyStoryElementKind::FootnotesRoot)
            ) | (
                PackageStoryKind::Endnotes,
                Some(LegacyStoryElementKind::EndnotesRoot)
            )
        )
        && matches!(local, b"footnote" | b"endnote")
        && note_is_normal(element, resolver)
    {
        return Some(LegacyStoryElementKind::NormalNote);
    }
    if matches!(story_kind, PackageStoryKind::Comments)
        && word
        && local == b"comment"
        && matches!(stack.last(), Some(LegacyStoryElementKind::CommentsRoot))
    {
        return Some(LegacyStoryElementKind::NormalComment);
    }
    if matches!(story_kind, PackageStoryKind::Document)
        && word
        && local == b"body"
        && matches!(stack.last(), Some(LegacyStoryElementKind::DocumentRoot))
    {
        return Some(LegacyStoryElementKind::BodyRoot);
    }
    if let Some(LegacyStoryElementKind::ContentControl(owner, false)) = stack.last()
        && word
        && local == b"sdtContent"
    {
        return Some(LegacyStoryElementKind::ContentControlContent(*owner));
    }
    let owner = match stack.last()? {
        LegacyStoryElementKind::BodyRoot
        | LegacyStoryElementKind::HeaderFooterRoot
        | LegacyStoryElementKind::NormalNote
        | LegacyStoryElementKind::NormalComment => LegacyBlockOwner::Root,
        LegacyStoryElementKind::Table => LegacyBlockOwner::Table,
        LegacyStoryElementKind::Row => LegacyBlockOwner::Row,
        LegacyStoryElementKind::Cell => LegacyBlockOwner::Cell,
        LegacyStoryElementKind::ContentControlContent(owner) => *owner,
        _ => return Some(LegacyStoryElementKind::Other),
    };
    if !word {
        return Some(LegacyStoryElementKind::Other);
    }
    match local {
        b"p" if matches!(owner, LegacyBlockOwner::Root | LegacyBlockOwner::Cell) => None,
        b"tbl" if matches!(owner, LegacyBlockOwner::Root | LegacyBlockOwner::Cell) => {
            Some(LegacyStoryElementKind::Table)
        }
        b"tr" if owner == LegacyBlockOwner::Table => Some(LegacyStoryElementKind::Row),
        b"tc" if owner == LegacyBlockOwner::Row => Some(LegacyStoryElementKind::Cell),
        b"sdt" => Some(LegacyStoryElementKind::ContentControl(owner, false)),
        _ => Some(LegacyStoryElementKind::Other),
    }
}

fn package_story_root_matches(story_kind: PackageStoryKind, word: bool, local: &[u8]) -> bool {
    word && match story_kind {
        PackageStoryKind::Document => local == b"document",
        PackageStoryKind::Header => local == b"hdr",
        PackageStoryKind::Footer => local == b"ftr",
        PackageStoryKind::Footnotes => local == b"footnotes",
        PackageStoryKind::Endnotes => local == b"endnotes",
        PackageStoryKind::Comments => local == b"comments",
    }
}

fn apply_namespace_declarations(
    element: &BytesStart<'_>,
    bindings: &mut BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("invalid namespace declaration: {error}")))?;
        let key = attribute.key.as_ref();
        if key == b"xmlns" || key.starts_with(b"xmlns:") {
            bindings.insert(
                String::from_utf8_lossy(key).into_owned(),
                attribute.value.as_ref().to_vec(),
            );
        }
    }
    Ok(())
}

fn paragraph_fragment_with_bindings(
    raw: &[u8],
    start_len: usize,
    bindings: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<u8>> {
    // Declarations go before the `>` of a start tag, or the `/>` of an
    // empty paragraph.
    let close = if raw[..start_len].ends_with(b"/>") {
        2
    } else {
        1
    };
    let insert_at = start_len
        .checked_sub(close)
        .ok_or_else(|| Error::Other("package story paragraph start tag is missing".to_owned()))?;
    let mut output = raw[..insert_at].to_vec();
    for (key, value) in bindings {
        if start_tag_has_raw_attribute(&raw[..start_len], key.as_bytes()) {
            continue;
        }
        output.push(b' ');
        output.extend_from_slice(key.as_bytes());
        output.extend_from_slice(b"=\"");
        output.extend_from_slice(value);
        output.push(b'"');
    }
    output.extend_from_slice(&raw[insert_at..]);
    Ok(output)
}

fn start_tag_has_raw_attribute(start: &[u8], name: &[u8]) -> bool {
    let mut reader = quick_xml::Reader::from_reader(start);
    let mut buffer = Vec::new();
    matches!(
        reader.read_event_into(&mut buffer),
        Ok(Event::Start(element) | Event::Empty(element))
            if element
                .attributes()
                .flatten()
                .any(|attribute| attribute.key.as_ref() == name)
    )
}

fn patch_legacy_story_field_sources(
    xml: &[u8],
    paragraphs: &[LegacyStoryParagraph],
    preserve_original_cache_sources: bool,
) -> Result<Vec<u8>> {
    let mut edits = Vec::new();
    for paragraph in paragraphs {
        let mut search_start = 0usize;
        let replacements = if preserve_original_cache_sources {
            paragraph_field_source_replacements(&paragraph.original, &paragraph.paragraph)?
        } else {
            staged_paragraph_field_source_replacements(&paragraph.paragraph)?
        };
        for (source, replacement) in replacements {
            let Some(start) = find_typed_field_source(
                xml,
                paragraph.start,
                paragraph.end,
                &source,
                search_start,
            )?
            else {
                return Err(Error::Other(
                    "package story field source was not found at its typed paragraph boundary"
                        .to_owned(),
                ));
            };
            let end = start + source.len();
            edits.push(FieldSourceEdit {
                start: paragraph.start + start,
                end: paragraph.start + end,
                replacement,
            });
            search_start = end;
        }
        let mut groups = Vec::new();
        append_nested_textbox_groups(&paragraph.paragraph, &mut groups)?;
        if groups.is_empty() {
            continue;
        }
        let window_start = paragraph.start;
        let window_end = paragraph.end;
        let spans = word_txbx_paragraph_spans(xml, &[])?
            .into_iter()
            .filter(|(start, _)| *start >= window_start && *start < window_end)
            .collect::<Vec<_>>();
        if spans.len() != groups.len() {
            return Err(Error::Other(
                "text box paragraph boundaries do not match the typed story owner".to_owned(),
            ));
        }
        for ((start, end), group) in spans.into_iter().zip(groups) {
            let mut box_search = 0usize;
            for (source, replacement) in group {
                let Some(relative) =
                    find_bound_field_source(xml, start, end, &source, box_search, &[])?
                else {
                    return Err(Error::Other(
                        "text box field source was not found at its text box paragraph boundary"
                            .to_owned(),
                    ));
                };
                if source != replacement {
                    edits.push(FieldSourceEdit {
                        start: start + relative,
                        end: start + relative + source.len(),
                        replacement,
                    });
                }
                box_search = relative + source.len();
            }
        }
    }
    let mut updated = xml.to_vec();
    for edit in edits.into_iter().rev() {
        updated.splice(edit.start..edit.end, edit.replacement);
    }
    Ok(updated)
}

struct FieldSourceEdit {
    start: usize,
    end: usize,
    replacement: Vec<u8>,
}

fn same_field_inventory(left: &CT_P, right: &CT_P) -> bool {
    fn fields(paragraph: &CT_P) -> Vec<(&Field, bool)> {
        let mut fields = Vec::new();
        for run in accepted_toc_runs(paragraph) {
            for content in &run.run.content {
                if let RunContent::Field(field) = content {
                    collect_preorder_fields(field, false, &mut fields);
                }
            }
        }
        fields
    }
    let left = fields(left);
    let right = fields(right);
    if left.len() != right.len() {
        return false;
    }
    fn argument(argument: &FieldArgument) -> Option<&str> {
        match argument {
            FieldArgument::Text(text) => Some(text.as_str()),
            FieldArgument::Nested(_) => None,
        }
    }
    for ((left, locked_left), (right, locked_right)) in left.iter().zip(&right) {
        if locked_left != locked_right || left.effective_display() != right.effective_display() {
            return false;
        }
        let a = left.effective_instruction();
        let b = right.effective_instruction();
        if a.name != b.name
            || a.arguments.len() != b.arguments.len()
            || a.switches.len() != b.switches.len()
        {
            return false;
        }
        if a.arguments
            .iter()
            .zip(&b.arguments)
            .any(|(a, b)| argument(a) != argument(b))
        {
            return false;
        }
        if a.switches.iter().zip(&b.switches).any(|(a, b)| {
            a.name != b.name
                || match (&a.argument, &b.argument) {
                    (None, None) => false,
                    (Some(a), Some(b)) => argument(a) != argument(b),
                    _ => true,
                }
        }) {
            return false;
        }
    }
    // Physical shared-owner equivalence is checked without treating fresh
    // source IDs as persistent identities across independent parsing.
    let groups = |fields: &[(&Field, bool)]| {
        let mut owners = HashMap::new();
        fields
            .iter()
            .enumerate()
            .map(|(index, (field, _))| {
                field
                    .source_owner_id()
                    .map_or(index, |owner| *owners.entry(owner).or_insert(index))
            })
            .collect::<Vec<_>>()
    };
    if groups(&left) != groups(&right) {
        return false;
    }

    true
}

fn paragraph_field_source_replacements(
    original: &CT_P,
    staged: &CT_P,
) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    fn instruction_context(paragraph: &CT_P) -> Vec<(String, bool)> {
        fn field_context(field: &Field, locked: bool, output: &mut Vec<(String, bool)>) {
            let locked = locked || field.locked() == Some(true);
            output.push((field.effective_instruction_text(), locked));
            for child in field.nested_fields_in_source_order() {
                field_context(child, locked, output);
            }
        }
        let mut output = Vec::new();
        for run in accepted_toc_runs(paragraph) {
            for content in &run.run.content {
                if let RunContent::Field(field) = content {
                    field_context(field, false, &mut output);
                }
            }
        }
        output
    }
    if instruction_context(original) != instruction_context(staged) {
        return Err(Error::Other(
            "field cache publication changed physical instruction or lock ownership".into(),
        ));
    }
    let original = staged_paragraph_field_source_replacements(original)?;
    let staged = staged_paragraph_field_source_replacements(staged)?;
    if original.len() != staged.len() {
        return Err(Error::Other(
            "field cache publication changed physical source span cardinality".into(),
        ));
    }
    Ok(original
        .into_iter()
        .zip(staged)
        .map(|((source, _), (_, replacement))| (source, replacement))
        .collect())
}

fn staged_paragraph_field_source_replacements(paragraph: &CT_P) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut replacements = Vec::new();
    let mut next_run = 0;
    for boundary in 0..=paragraph.runs.len() {
        for (_, owner) in accepted_paragraph_owners(paragraph, boundary) {
            match owner {
                AcceptedParagraphOwner::Control(control) => {
                    append_control_field_source_replacements(control, &mut replacements)?;
                }
                AcceptedParagraphOwner::Revision(revision) => {
                    if matches!(
                        revision.kind(),
                        RevisionKind::Insertion | RevisionKind::MoveTo
                    ) && let Some(content) = revision.content_paragraph()
                    {
                        replacements.extend(staged_paragraph_field_source_replacements(content)?);
                    }
                }
            }
        }
        if boundary < next_run || boundary == paragraph.runs.len() {
            continue;
        }
        next_run =
            boundary + paragraph.field_source_replacements_at(boundary, &mut replacements)?;
    }
    Ok(replacements)
}

fn append_control_field_source_replacements(
    control: &CT_Sdt,
    output: &mut Vec<(Vec<u8>, Vec<u8>)>,
) -> Result<()> {
    output.extend(
        control
            .inline_field_source_replacements()
            .into_iter()
            .map(|(source, replacement)| (source.to_vec(), replacement.to_vec())),
    );
    for boundary in 0..=control.content.len() {
        for (_, revision) in control.revisions().iter().filter(|(at, _)| *at == boundary) {
            if matches!(
                revision.kind(),
                RevisionKind::Insertion | RevisionKind::MoveTo
            ) && let Some(paragraph) = revision.content_paragraph()
            {
                output.extend(staged_paragraph_field_source_replacements(paragraph)?);
            }
        }
        let Some(content) = control.content.get(boundary) else {
            continue;
        };
        match content {
            SdtContent::ContentControl(control) => {
                append_control_field_source_replacements(control, output)?
            }
            SdtContent::Paragraph(paragraph) => {
                output.extend(staged_paragraph_field_source_replacements(paragraph)?);
            }
            SdtContent::Run(run) => {
                if let [RunContent::Field(field)] = run.content.as_slice()
                    && let Some((source, replacement)) = field.source_replacement()?
                {
                    output.push((source.to_vec(), replacement));
                }
            }
            SdtContent::Table(_)
            | SdtContent::Row(_)
            | SdtContent::Cell(_)
            | SdtContent::RawXml(_) => {}
        }
    }
    Ok(())
}

fn note_is_normal(element: &BytesStart<'_>, resolver: &NamespaceResolver) -> bool {
    let mut id = None;
    let mut note_type = None;
    for attribute in element.attributes() {
        let Ok(attribute) = attribute else {
            return false;
        };
        let (namespace, local) = resolver.resolve_attribute(attribute.key);
        if !namespace_is_word(&namespace) {
            continue;
        }
        if local.as_ref() == b"id" {
            let Ok(raw) = std::str::from_utf8(&attribute.value) else {
                return false;
            };
            let Ok(value) = quick_xml::escape::unescape(raw) else {
                return false;
            };
            let Ok(value) = value.parse::<i32>() else {
                return false;
            };
            if id.replace(value).is_some() {
                return false;
            }
        } else if local.as_ref() == b"type" {
            let Ok(raw) = std::str::from_utf8(&attribute.value) else {
                return false;
            };
            let Ok(value) = quick_xml::escape::unescape(raw) else {
                return false;
            };
            if note_type.replace(value.into_owned()).is_some() {
                return false;
            }
        }
    }
    match note_type.as_deref() {
        Some("separator" | "continuationSeparator" | "continuationNotice") => false,
        Some("normal") => id.is_some(),
        Some(_) => false,
        None => id.is_some_and(|id| id > 0),
    }
}

fn collect_legacy_form_anchors(
    output: &mut Vec<LegacyFormAnchor>,
    part: &str,
    xml: &[u8],
    kind: PackageStoryKind,
) -> Result<()> {
    let story = legacy_story_paragraphs(xml, kind)?;
    let mut ordinal = 0usize;
    for paragraph in &story {
        let sources = legacy_form_sources_in_paragraph(&paragraph.paragraph)?;
        place_legacy_sources(
            output,
            part,
            xml,
            paragraph.start,
            paragraph.end,
            &sources,
            &mut ordinal,
        )?;
    }
    for paragraph in &story {
        let boxes = textbox_paragraphs(&paragraph.paragraph);
        if boxes.is_empty() {
            continue;
        }
        let spans = word_txbx_paragraph_spans(xml, &[])?
            .into_iter()
            .filter(|(start, _)| *start >= paragraph.start && *start < paragraph.end)
            .collect::<Vec<_>>();
        if spans.len() != boxes.len() {
            return Err(Error::Other(
                "text box paragraph boundaries do not match the typed story owner".to_owned(),
            ));
        }
        for ((start, end), box_paragraph) in spans.into_iter().zip(boxes) {
            let sources = legacy_form_sources_in_paragraph(box_paragraph)?;
            place_legacy_sources(output, part, xml, start, end, &sources, &mut ordinal)?;
        }
    }
    Ok(())
}

fn legacy_form_sources_in_paragraph(paragraph: &CT_P) -> Result<Vec<Vec<u8>>> {
    let mut fields = Vec::new();
    let mut ordinal = 0usize;
    let mut sources = Vec::new();
    append_legacy_form_infos_in_paragraph(
        &mut fields,
        "",
        &mut ordinal,
        paragraph,
        true,
        &mut sources,
    )?;
    if sources.len() != fields.len() {
        return Err(Error::Other(
            "legacy form field has no parsed physical source".to_owned(),
        ));
    }
    Ok(sources)
}

fn place_legacy_sources(
    output: &mut Vec<LegacyFormAnchor>,
    part: &str,
    xml: &[u8],
    paragraph_start: usize,
    paragraph_end: usize,
    sources: &[Vec<u8>],
    ordinal: &mut usize,
) -> Result<()> {
    let mut search_start = 0usize;
    for source in sources {
        let Some(relative) =
            find_typed_field_source(xml, paragraph_start, paragraph_end, source, search_start)?
        else {
            return Err(Error::Other(
                "legacy form field source was not found in its accepted paragraph".to_owned(),
            ));
        };
        let start = paragraph_start + relative;
        output.push(LegacyFormAnchor {
            part: part.to_owned(),
            ordinal: *ordinal,
            start,
            end: start + source.len(),
        });
        *ordinal += 1;
        // The next accepted field may be nested inside this span. Advance one
        // byte so a later identical sibling is distinct and a nested field remains.
        search_start = relative.saturating_add(1);
    }
    Ok(())
}

fn textbox_paragraphs(paragraph: &CT_P) -> Vec<&CT_P> {
    let mut output = Vec::new();
    push_nested_textbox_paragraphs(paragraph, &mut output);
    output
}

fn push_nested_textbox_paragraphs<'a>(paragraph: &'a CT_P, output: &mut Vec<&'a CT_P>) {
    for path in paragraph.accepted_run_paths() {
        let Some(run) = paragraph.accepted_run(&path) else {
            continue;
        };
        for content in &run.content {
            if let RunContent::Drawing(drawing) = content {
                push_shape_paragraphs(drawing, output);
            }
        }
    }
}

fn push_shape_paragraphs<'a>(drawing: &'a CT_Drawing, output: &mut Vec<&'a CT_P>) {
    let Some(shape) = drawing
        .anchor
        .as_ref()
        .and_then(|anchor| anchor.shape.as_ref())
    else {
        return;
    };
    let mut paragraphs = Vec::new();
    if let Some(body) = &shape.text_body {
        collect_body_paragraphs(body, &mut paragraphs, None);
    } else {
        paragraphs.extend(shape.text.iter());
    }
    for paragraph in paragraphs {
        output.push(paragraph);
        push_nested_textbox_paragraphs(paragraph, output);
    }
}

pub(crate) fn find_typed_field_source(
    xml: &[u8],
    paragraph_start: usize,
    paragraph_end: usize,
    source: &[u8],
    search_start: usize,
) -> Result<Option<usize>> {
    let paragraph_xml = &xml[paragraph_start..paragraph_end];
    let mut candidate_start = search_start;
    while let Some(relative) = find_bytes(&paragraph_xml[candidate_start..], source) {
        let candidate = candidate_start + relative;
        if field_source_has_typed_ancestors(xml, paragraph_start + candidate, paragraph_start)? {
            return Ok(Some(candidate));
        }
        candidate_start = candidate + source.len().max(1);
    }
    Ok(None)
}

fn field_source_has_typed_ancestors(
    xml: &[u8],
    source_start: usize,
    paragraph_start: usize,
) -> Result<bool> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut ancestors = Vec::<(usize, bool, Vec<u8>)>::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid package paragraph XML: {error}")))?;
        match event {
            Event::Start(element) => {
                let name = element.name();
                let owned = name.as_ref().to_vec();
                let physical_word =
                    xml[before..].starts_with(b"<w:") || xml[before..].starts_with(b"</w:");
                let word = if matches!(namespace, ResolveResult::Bound(_)) {
                    namespace_is_word(&namespace)
                } else {
                    physical_word
                };
                if before == source_start {
                    let local = local_name(&owned);
                    return Ok(word
                        && matches!(local, b"fldSimple" | b"r")
                        && typed_field_ancestors(&ancestors, paragraph_start));
                }
                ancestors.push((before, word, local_name(&owned).to_vec()));
            }
            Event::Empty(element) => {
                let name = element.name();
                let owned = name.as_ref().to_vec();
                let physical_word =
                    xml[before..].starts_with(b"<w:") || xml[before..].starts_with(b"</w:");
                let word = if matches!(namespace, ResolveResult::Bound(_)) {
                    namespace_is_word(&namespace)
                } else {
                    physical_word
                };
                if before == source_start {
                    let local = local_name(&owned);
                    return Ok(word
                        && matches!(local, b"fldSimple" | b"r")
                        && typed_field_ancestors(&ancestors, paragraph_start));
                }
            }
            Event::End(_) => {
                ancestors.pop();
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

fn typed_field_ancestors(ancestors: &[(usize, bool, Vec<u8>)], paragraph_start: usize) -> bool {
    let Some(paragraph_index) = ancestors
        .iter()
        .position(|(start, _, _)| *start == paragraph_start)
    else {
        return false;
    };
    let (_, paragraph_word, paragraph) = &ancestors[paragraph_index];
    *paragraph_word
        && paragraph.as_slice() == b"p"
        && ancestors
            .iter()
            .skip(paragraph_index + 1)
            .all(|(_, word, local)| {
                *word
                    && matches!(
                        local.as_slice(),
                        b"hyperlink"
                            | b"sdt"
                            | b"sdtContent"
                            | b"ins"
                            | b"del"
                            | b"moveFrom"
                            | b"moveTo"
                    )
            })
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

fn apply_updates_to_paragraph(
    paragraph: &mut CT_P,
    updates: &[Option<CachedFieldUpdate>],
    update_index: &mut usize,
) -> Result<()> {
    for path in paragraph.accepted_run_paths() {
        let Some(source) = paragraph.accepted_run(&path) else {
            return Err(Error::Other(
                "accepted field update run path is stale".into(),
            ));
        };
        if !source
            .content
            .iter()
            .any(|content| matches!(content, RunContent::Field(_)))
        {
            continue;
        }
        let mut replacement = source.clone();
        apply_updates_to_run(&mut replacement, updates, update_index)?;
        if !paragraph.replace_accepted_run(&path, replacement)? {
            return Err(Error::Other(
                "accepted field update owner disappeared".into(),
            ));
        }
    }
    Ok(())
}

fn apply_updates_to_run(
    run: &mut oxoffice_docx_oxml::text::CT_R,
    updates: &[Option<CachedFieldUpdate>],
    update_index: &mut usize,
) -> Result<()> {
    for content in &mut run.content {
        if let RunContent::Field(field) = content {
            apply_updates_to_field(field, updates, update_index)?;
        }
    }
    Ok(())
}

fn apply_updates_to_field(
    field: &mut Field,
    updates: &[Option<CachedFieldUpdate>],
    update_index: &mut usize,
) -> Result<()> {
    let Some(update) = updates.get(*update_index) else {
        return Ok(());
    };
    if let Some(update) = update {
        field.cached_result.clone_from(&update.cached_result);
        field.dirty = Some(update.dirty);
    }
    *update_index += 1;

    let nested_pointers = field
        .nested_fields_in_source_order()
        .into_iter()
        .map(|nested| std::ptr::from_ref(nested) as usize)
        .collect::<Vec<_>>();
    for pointer in nested_pointers {
        if let Some(nested) = nested_field_mut(field, pointer) {
            apply_updates_to_field(nested, updates, update_index)?;
        }
    }
    for index in 0..field.cached_fields_in_source_order().len() {
        if let Some(nested) = field.cached_field_mut(index) {
            apply_updates_to_field(nested, updates, update_index)?;
        }
    }
    if let Some(runs) = update
        .as_ref()
        .and_then(|update| update.typed_runs.as_ref())
    {
        field.set_cached_runs_with_comment_ranges(
            runs.clone(),
            update
                .as_ref()
                .expect("typed cache update")
                .comment_ranges
                .clone(),
        )?;
    }
    if update.is_none() && !field.cached_fields_in_source_order().is_empty() {
        field.refresh_cached_field_projection();
    }
    field.normalize_cached_result_owner()?;
    Ok(())
}

fn nested_field_mut(field: &mut Field, pointer: usize) -> Option<&mut Field> {
    for argument in &mut field.instruction.arguments {
        if let FieldArgument::Nested(nested) = argument
            && std::ptr::from_ref(nested.as_ref()) as usize == pointer
        {
            return Some(nested.as_mut());
        }
    }
    for field_switch in &mut field.instruction.switches {
        if let Some(FieldArgument::Nested(nested)) = &mut field_switch.argument
            && std::ptr::from_ref(nested.as_ref()) as usize == pointer
        {
            return Some(nested.as_mut());
        }
    }
    None
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
