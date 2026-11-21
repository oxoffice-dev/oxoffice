//! The main Document type — entry point for the oxoffice-docx API.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use oxoffice_chart::{CT_ChartSpace, ChartData, ChartKind};
use oxoffice_core::app_properties::AppProperties;
use oxoffice_docx_oxml::MathProperties;
use oxoffice_docx_oxml::content_control::{CT_Sdt, SdtContent, StorySdtOwner};
use oxoffice_docx_oxml::document::{
    BodyContent, CT_Body, CT_Column, CT_Columns, CT_DocGrid, CT_Document, CT_LineNumber,
    CT_NoteProperties, CT_PageBorders, CT_PaperSource, CT_SectPr,
};
use oxoffice_docx_oxml::drawing::{
    AnchorAlignH, AnchorAlignV, CT_Anchor, CT_Drawing, CT_Inline, ST_RelativeFromH,
    ST_RelativeFromV, SourceRect, WrapType, drawing_ns,
};
use oxoffice_docx_oxml::font_table::{EmbeddedFontReference, FontFaceKind, FontRecord, FontTable};
use oxoffice_docx_oxml::header_footer::{
    CT_HdrFtr, HdrFtrRef, HdrFtrType, VmlWatermark, replace_authored_watermark,
};
use oxoffice_docx_oxml::namespace::{M_NS, MC_NS, W_NS, matches_local_name};
use oxoffice_docx_oxml::numbering::{
    CT_AbstractNum, CT_Lvl, CT_Num, CT_NumLvl, CT_Numbering, ST_LvlSuffix, ST_NumberFormat,
};
use oxoffice_docx_oxml::properties::{CT_PPr, CT_RPr};
use oxoffice_docx_oxml::revision::CT_Revision;
use oxoffice_docx_oxml::settings::{
    CT_Settings, CharacterSpacingControl, CompatibilityOption, CompatibilitySetting,
    DocumentProofState, DocumentProtection, DocumentView, DocumentZoom, MailMerge,
    SettingsDiagnostic, ThemeFontLanguage,
};
use oxoffice_docx_oxml::shared::{ST_Jc, ST_PageOrientation, ST_SectionType};
use oxoffice_docx_oxml::styles::{CT_Styles, StyleType};
use oxoffice_docx_oxml::table::{CT_Row, CT_Tbl, CT_Tc, CellContent, ST_VerticalJc, VMerge};
use oxoffice_docx_oxml::text::{
    CT_P, CT_R, RangeAnchor, RunContent, SpecialCharacter, declare_w14_on_part_root,
    story_field_instruction_has_name, story_simple_field_is_typed,
};
use oxoffice_docx_oxml::units::Twips;
use oxoffice_docx_oxml::web_settings::CT_WebSettings;
use oxoffice_opc::content_types;
use oxoffice_opc::relationship::rel_types;
use oxoffice_opc::{OpcPackage, PackageReadLimits, SaveOptions, write_atomic_file};
use oxoffice_sml::Workbook;
use quick_xml::Writer;
use quick_xml::XmlVersion;
use quick_xml::escape::resolve_xml_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, PrefixDeclaration, ResolveResult};
use quick_xml::reader::NsReader;

use oxoffice_core::custom_properties::{CustomProperties, CustomProperty};
use oxoffice_docx_oxml::core_properties::CoreProperties;

use crate::Length;
use crate::comments::{StoryRunPosition, StoryRunRange};
use crate::content_control::ContentControlRef;
use crate::error::{Error, Result};
use crate::paragraph::{Paragraph, ParagraphRef};
use crate::revision::{RevisionRef, StoryRevision};
use crate::run::{FieldDisplaySegmentRef, RunRef};
use crate::style::{self, Style, StyleBuilder};
use crate::table::{Table, TableRef, row_cell_ranges, validate_table_topology};

mod checked_replacement;
mod story_paragraphs;
pub use checked_replacement::{
    ReplaceOptions, ReplacementRefusal, UncheckedReplacement, UnreachedReplacement,
};
pub use story_paragraphs::StoryParagraph;

mod drawing_options;
mod font_types;
mod fragment_types;
mod inspection;
mod note_policy;
mod numbering_types;
mod package_class;
mod render_options;
mod story_types;
mod text_report;
mod validation;

pub use drawing_options::{
    DrawingHorizontalAlignment, DrawingHorizontalRelativeFrom, DrawingVerticalAlignment,
    DrawingVerticalRelativeFrom, DrawingWrap, ImageWatermarkOptions, PictureAnchor, PictureCrop,
    PictureOptions, TextBoxDirection, TextBoxOptions, TextWatermarkOptions,
};
pub use font_types::{
    EmbeddedFont, EmbeddedFontKind, FontDefinition, FontEmbeddingLicense, derive_font_key,
};
pub use fragment_types::FragmentConflictPolicy;
pub(crate) use fragment_types::{
    ScopedDocumentRead, TemplateFragmentSelection, TemplateFragmentSources,
};
pub use inspection::{
    AccessibilityIssue, ImageInfo, IssueSeverity, LinkInfo, NumberedOutlineNode, OutlineNode,
};
pub use note_policy::{
    NoteFamily, NoteNumberFormat, NotePlacement, NotePolicy, NoteRestart, NoteSpecialRecord,
};
pub(crate) use numbering_types::list_number_format_from_st;
pub use numbering_types::{
    ListLevel, ListLevelRestart, ListLevelSuffix, ListNumberFormat, NumberingDefinition,
    NumberingDefinitionLevel, NumberingFormat, NumberingInstance, NumberingLevel,
    NumberingLevelOverride,
};
use numbering_types::{
    ct_level_from_list, list_level_alignment, list_level_from_ct, merge_list_level,
    numbering_level_has_unmodeled, numbering_override_for_update, numbering_override_from_public,
    numbering_override_has_unmodeled, restart_can_copy, validate_ct_numbering_level,
    validate_list_level, validate_numbering_definition_levels, validate_numbering_level_slice,
    xml_paragraph_numbering_properties, xml_references_numbering_instance,
};
pub use package_class::{WordCreationProfile, WordPackageClass};
pub use render_options::RenderOptions;
use render_options::{
    FILLABLE_FORMS_NEED_ACCEPTED_VIEW, FontPolicy, SYSTEM_FONTS_UNAVAILABLE, font_policy,
};
pub(crate) use story_types::GeneratedStoryOwner;
pub use story_types::{
    BodyItemOwners, BodyItemRef, HeaderFooterKind, StoryError, StoryItemKind, StoryKind,
};
use text_report::STORY_TEXT_RELATIONSHIPS;
pub use text_report::{StoryText, StoryTextItem, TextDiagnostic, TextReport};
use validation::document_style_references;
pub use validation::{
    ValidationFinding, ValidationReport, ValidationSeverity, validate_document_bytes,
    validate_document_file,
};

mod fonts;
mod merge;
mod properties;
mod render;
mod settings;
mod toc;

use fonts::valid_authored_sfnt;
pub use render::ContentMeasurement;

mod body;
mod content;
mod headers_footers;
mod media;
mod notes;
mod numbering;
mod replace;
mod sections;
mod styles;
mod tables;
mod template;

use headers_footers::{
    header_footer_story_has_expected_root, header_type_index, materialize_header_footer_inheritance,
};
use media::chart_with_workbook_relationship;
pub use replace::ReplacementCountMismatch;

mod drawing_xml;
mod identifiers;
mod main_part;
mod namespace_replay;
mod namespaces;
mod new_package;
mod paragraph_visibility;
mod section_handles;
mod template_sources;
mod unsupported_xml;
mod xml_ids;

use drawing_xml::{
    authored_text_box_alternate_content, drawingml_watermark_span, is_rgb_hex, picture_drawing,
    picture_source_extent, validate_picture_options, validate_text_box_options,
    validate_text_watermark_options,
};
pub(crate) use identifiers::part_name_identity;
use identifiers::{
    IdentifierDefinition, IdentifierXmlCategory, frame_drawings_are_absent,
    identifier_xml_is_well_formed, read_child_canonical_facts, relationship_owner_identity,
    reserve_lowest_i32, reserve_part_from_set, reserve_relationship_from_cursor, reserve_u32,
    scan_identifier_definitions, split_relationship_ids_at_head,
};
pub(crate) use main_part::{CommentParagraphRead, CommentParagraphValue};
use main_part::{
    EmptyNamespaceChildProof, FrameIdentifierProof, FrameNamespacePolicyEmpty,
    IdentifierChildProof, NamespaceFrameSpans, NamespaceSpanProof, ParsedChild,
    ScopedChildCorrespondence, StandaloneParagraphValue, block_proofs_enabled,
    full_direct_body_item_location, holds_field_words, offered_canonical_blocks,
    picture_free_span_kind, producer_scoped_normalization_seed, root_start_tag, scan_body_block,
    scan_body_children, scoped_children_correspond, scoped_normalization_context,
    scoped_normalization_reuse_allowed, single_frame_body, story_control_paragraphs,
    story_owner_item, story_owner_items,
};
use namespace_replay::{
    plain_local_edit_facts, replay_nested_namespace_declarations,
    replay_nested_namespace_declarations_in, replay_nested_namespace_declarations_with_spans,
    replay_unsavable_body_edit,
};
use namespaces::{
    candidate_namespace_owner_markers, document_namespace_scopes, logical_owner_snapshot,
    marker_multiset_matches, modeled_owner_piece_spans, modeled_owner_spans,
    namespace_declarations, namespace_owners_from_spans, namespace_prefix,
    nested_modeled_namespace_owners, retain_styles_root_attributes, root_namespace_declarations,
    shadowed_namespace_diagnostic, unsafe_nested_namespace_prefix,
    unsafe_serializer_namespace_prefix,
};
use new_package::{
    default_application_properties, is_app_version, new_word_compatible_package, new_word_package,
    repair_application_version,
};
use paragraph_visibility::{ParagraphVisibilityScope, paragraph_visibility_from_main_source};
pub use section_handles::BodyContentRef;
use section_handles::empty_section_properties;
pub(crate) use template_sources::TemplateMainWriter;
pub use unsupported_xml::UnsupportedXmlRef;
use unsupported_xml::{
    inherited_namespace, typed_numbering_leaf_has_unmodeled, xml_bytes_are_whitespace,
};
use xml_ids::{
    AuthoredDrawingSlots, AuthoredMainDrawing, apply_borrowed_template_source_edits,
    apply_shared_template_source_edits, authored_doc_pr_edits, authored_drawing_slots_by_blocks,
    authored_drawing_slots_in_source_order, authored_relationship_ids_in_source_order,
    drawing_ids_by_blocks, drawing_ids_in_xml, drawing_occurrences, relationship_ids_by_blocks,
    relationship_remap_edits, relationship_xml_from_replayed,
    remap_xml_relationship_ids_with_bindings, rewrite_authored_doc_pr_ids,
    select_authored_drawing_slots, story_attribute_value_span, xml_attribute_values,
    xml_relationship_ids_in_collection_range, xml_relationship_ids_in_order_with_bindings,
};
pub(crate) use xml_ids::{
    apply_template_source_edits, remap_xml_relationship_ids, unique_drawing_id_edits,
    uniquify_drawing_ids_in_xml, xml_relationship_ids_in_order,
};

mod edited_paragraph;
mod fragment_checks;
mod package_checks;
mod story_fields;
mod story_scan;
mod story_scopes;
mod story_source_edits;
mod story_text;
mod story_xml;
mod template_programs;
mod text_boxes;
mod walkers;

use edited_paragraph::{admit_edited_paragraph, edited_paragraph_ancestor_context};
use fragment_checks::{
    WordXmlOwner, content_fragment_for_insertion, fragment_content_boundary, fragment_uses_w14,
    freshen_content_fragment_identities, insert_story_fragment, physical_table_cell_owner,
    reject_section_owning_content_fragment, serialize_content_fragment, standalone_body_item_span,
    story_owner_content_end, story_owner_content_end_in, validate_element_owner_relationships,
    validate_element_xml_identity_change, validate_fragment_block_item,
    validate_fragment_relationships, validate_serialized_block_content_control,
    validated_content_boundary, validated_direct_content_item, xml_1_0_whitespace,
};
pub(crate) use fragment_checks::{
    close_content_fragment_namespaces, package_authoritative_body_fragment,
    package_authoritative_body_source, validate_fragment_block_content,
};
use package_checks::{validate_fresh_word_compatible_package, validated_word_package_class};
use story_fields::{admit_story_sdt_content_slot, complex_story_field_xml, modeled_story_event};
use story_scan::{
    ComplexFieldMarker, StoryFieldContext, StoryFieldTracker, StoryTextPiece,
    accepted_story_text_visible, bind_text_box_source_owners, complex_field_marker,
    complex_field_text_visible, editable_note_owner, opaque_story_event, paired_story_owners,
    prepare_semantic_paragraph_projection, scan_story_control_paragraphs,
    scan_story_control_paragraphs_with_scope, scan_story_item_links, scan_story_items,
    story_break_text, story_field_marker_tracked, structural_fingerprint,
    update_complex_field_results,
};
pub(crate) use story_scan::{
    admitted_story_paragraph_spans, prepare_physical_story_projection,
    prepare_semantic_body_projection, scan_story_owners, scan_story_owners_with_special,
};
use story_scopes::{
    content_fragment_root_is_section_properties, direct_story_content_items,
    direct_story_content_items_in, ordered_story_link_spans, scan_story_item_links_with_scope,
    scan_story_items_with_scope, scoped_story_fragment, story_item_text_with_scope,
    story_link_spans, story_namespace_scopes_at,
};
pub(crate) use story_scopes::{story_namespace_ancestors_at, story_namespace_scope_at};
pub(crate) use story_source_edits::set_story_source_xml;
use story_source_edits::{
    append_story_fragments_to_root, append_story_sources_to_root,
    decoded_story_namespace_declaration, missing_note_specials, owners_reference_hyperlink_style,
    remove_note_references, serialized_footnote_fragment, serialized_note_fragment,
    serialized_note_fragment_in, set_story_source_xml_in, story_word_prefix_for_scope,
    update_story_namespace_scope,
};
pub(crate) use story_text::paragraph_heading_level;
use story_text::{
    bookmark_marker_spans, external_story_hyperlink_start, remove_newly_empty_hyperlinks,
    replace_story_item_text, resize_story_pictures, story_hyperlink_attributes,
    story_hyperlink_tags, story_item_text, unwrap_story_hyperlink,
};
pub(crate) use story_xml::comment_namespace_source_boundaries;
use story_xml::{
    StoryNamespace, StorySdtContext, canonical_retained_main_part, changed_story_namespace_scope,
    comment_move_source_edits, comment_reference_run_span, field_instruction_has_name,
    modeled_story_child, scoped_proof_marker, scoped_proof_sentinel, story_element_end,
    story_element_sdt_context, story_frame_local_name, story_namespace,
    story_range_paragraph_spans, story_range_paragraph_spans_from_inventory, story_sdt_context,
    story_word_prefixes_at, word_element, xml_decodes,
};
use template_programs::{
    paired_scoped_edits, paired_scoped_ranges, prepare_template_source_programs,
    prepare_template_source_programs_in,
};
use text_boxes::{
    first_text_box_start, paired_text_box_projection, scoped_alternate_text_box_copies,
};
pub(crate) use walkers::{
    ExportItem, body_export_items, visit_accepted_drawings, visit_all_drawings,
    visit_all_drawings_mut, visit_body_paragraphs, visit_body_paragraphs_mut,
    visit_body_table_paragraphs_mut, visit_paragraph,
};
use walkers::{
    block_paragraphs, body_paragraph_routes, cell_paragraph_routes,
    close_typed_story_drawing_namespaces, collect_relationship_ids, insert_html_content_into_cell,
    modeled_main_cell_route, modeled_main_cell_routes, nth_cell_in_control, nth_cell_in_table,
    nth_paragraph_in_body, nth_table_in_body, nth_table_in_body_ref, physical_table_cells,
    push_control_text, push_paragraph_line, push_table_text, visit_authored_drawings,
    visit_authored_drawings_mut, visit_cell,
};

/// The effective header or footer story selected for one section variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionStory {
    story: StoryId,
    source_section: usize,
    inherited: bool,
}

/// Stable owner identity for one operation over an opened document.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StoryId {
    kind: StoryKind,
    part_name: String,
    owner_index: usize,
    fingerprint: u64,
}

/// A checked path to one item inside a story owner.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentLocation {
    story: StoryId,
    item_kind: StoryItemKind,
    index_path: Vec<usize>,
    is_end: bool,
}

/// Frozen facts for one admitted physical picture in a checked paragraph.
/// Relationship IDs identify media, not drawing occurrences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PictureOccurrence {
    pub location: ContentLocation,
    pub ordinal: usize,
    pub relationship_id: String,
    /// Internal media identity, independent of relationship spelling and bytes.
    pub media_part_name: Option<String>,
    /// External image source is metadata only and is never fetched.
    pub external_url: Option<String>,
    pub width: i64,
    pub height: i64,
    pub name: Option<String>,
    pub description: Option<String>,
    pub title: Option<String>,
    pub decorative: bool,
    pub anchored: bool,
    pub content_type: String,
    pub bytes: Vec<u8>,
    source: Vec<u8>,
}

/// Owned direct story content that can be inserted without a second document tree.
#[derive(Debug, Clone)]
pub struct ContentFragment {
    kind: StoryItemKind,
    xml: Vec<u8>,
    source_part_name: Option<String>,
    source_story_kind: Option<StoryKind>,
    source_owner_index: Option<usize>,
    namespace_scope: BTreeMap<String, String>,
}

/// An owned story range and the package dependencies it can reach.
#[derive(Debug, Clone)]
pub struct DocumentFragment {
    package: Vec<u8>,
    include_final_section_properties: bool,
}

/// A borrowed view of one supported story item.
///
/// The view owns its checked location. Body and comment items can use owned XML
/// serialized from their typed sources. Other package-backed items can borrow
/// exact subtree slices whose namespace bindings depend on retained ancestors.
/// A complex field uses an owned, namespace-complete paragraph projection
/// because its source can span several sibling runs.
pub struct StoryItemRef<'a> {
    document: &'a Document,
    location: ContentLocation,
}

/// One owned story-item projection materialized from a single story inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoryItemSnapshot {
    location: ContentLocation,
    direct_body_index: Option<usize>,
    direct_child: bool,
    text: Option<String>,
    xml: Vec<u8>,
}

/// What [`Document::body_item_owners`] reads besides the item, as
/// [`Document::body_item_context`] gives it.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyItemContext {
    part_name: String,
    root: [Vec<(String, String)>; 5],
    plain_local_edit_blocked: bool,
}

const WORD_NAMESPACE: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const W14_NAMESPACE: &str = "http://schemas.microsoft.com/office/word/2010/wordml";
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

#[derive(Default)]
struct DocumentNamespaceScopes {
    root_declarations: Vec<(String, String)>,
    body_declarations: Vec<(String, String)>,
    body_bindings: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
struct NestedNamespaceOwner {
    source: Range<usize>,
    local_name: String,
    declarations: Vec<(String, String)>,
    snapshot: LogicalOwnerSnapshot,
    markers: Vec<NamespaceMarker>,
    ambiguous_without_namespace: bool,
    same_marker_structural_alternate: bool,
    same_namespace_structural_alternate: bool,
}

#[derive(Clone, Debug)]
struct ModeledOwnerSpan {
    local_name: &'static str,
    start: usize,
    end: usize,
    declarations: Arc<Vec<(String, String)>>,
    bindings: Arc<Vec<(String, String)>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LogicalOwnerSnapshot {
    structure: Vec<String>,
    semantic: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NamespaceMarker {
    raw: Vec<u8>,
    namespace_facts: Vec<(String, Option<String>)>,
}

/// Whether `xml` holds an element whose local name is one of `names`.
fn xml_holds_element_named(xml: &[u8], names: &HashSet<&[u8]>) -> bool {
    let mut reader = quick_xml::Reader::from_reader(xml);
    loop {
        match reader.read_event() {
            Ok(
                quick_xml::events::Event::Start(element) | quick_xml::events::Event::Empty(element),
            ) if names.contains(element.local_name().as_ref()) => {
                return true;
            }
            Ok(quick_xml::events::Event::Eof) => return false,
            // An unreadable block is treated as one that may hold an owner.
            Err(_) => return true,
            Ok(_) => {}
        }
    }
}

/// Whether two tables hold the same rows, cells, cell content and unmodelled
/// children, whatever their table, row and cell properties. A cell must also
/// write a `w:tcPr` in both or in neither: it counts among the items of the
/// cell's story, so giving a cell its first property, or taking its last,
/// moves the paragraphs and tables of that story.
fn same_table_content(
    a: &oxoffice_docx_oxml::table::CT_Tbl,
    b: &oxoffice_docx_oxml::table::CT_Tbl,
) -> bool {
    a.extra_xml == b.extra_xml
        && a.content_controls == b.content_controls
        && a.rows.len() == b.rows.len()
        && a.rows.iter().zip(&b.rows).all(|(a, b)| {
            a.extra_xml == b.extra_xml
                && a.content_controls == b.content_controls
                && a.cells.len() == b.cells.len()
                && a.cells.iter().zip(&b.cells).all(|(a, b)| {
                    a.content == b.content
                        && a.extra_xml == b.extra_xml
                        && writes_cell_properties(a) == writes_cell_properties(b)
                })
        })
}

fn writes_cell_properties(cell: &oxoffice_docx_oxml::table::CT_Tc) -> bool {
    cell.properties
        .as_ref()
        .is_some_and(|properties| !properties.is_empty())
}

/// An immutable borrowed view of one section in document order.
#[derive(Clone, Copy)]
pub struct SectionRef<'a> {
    ordinal: usize,
    is_final: bool,
    inner: &'a CT_SectPr,
}

/// A mutable borrowed view of one section in document order.
pub struct Section<'a> {
    ordinal: usize,
    is_final: bool,
    inner: &'a mut CT_SectPr,
}

/// Package and WordprocessingML identity state owned by the document facade.
///
/// Each set represents one OOXML scope. References are deliberately excluded
/// from definition sets, so a bookmark end, comment anchor, or numbering use
/// does not collide with the definition it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DocumentIdentifiers {
    relationship_ids: HashMap<String, HashSet<String>>,
    bookmark_ids: HashSet<i32>,
    repeated_bookmark_ids: HashMap<i32, usize>,
    comment_ids: HashSet<i32>,
    drawing_ids: HashSet<u32>,
    /// Per part, the `wp:docPr/@id` values the opened XML repeats, with
    /// their occurrence count. Producers repeat them and consumers renumber
    /// or ignore them, so a scan accepts them. A staged edit keeps the counts
    /// of the opened document and may not exceed them.
    repeated_drawing_ids: HashMap<String, HashMap<u32, usize>>,
    abstract_numbering_ids: HashSet<u32>,
    numbering_instance_ids: HashSet<u32>,
    part_names: HashSet<String>,
    content_type_defaults: HashSet<String>,
    content_type_overrides: HashSet<String>,
    authored_content_type_defaults: HashSet<String>,
    preserved_relationship_ids: HashMap<String, HashSet<String>>,
    preserved_bookmark_ids: HashSet<i32>,
    preserved_comment_ids: HashSet<i32>,
    preserved_drawing_ids: HashSet<u32>,
    preserved_abstract_numbering_ids: HashSet<u32>,
    preserved_numbering_instance_ids: HashSet<u32>,
    preserved_part_names: HashSet<String>,
    authored_bookmark_ids: HashSet<i32>,
    authored_comment_ids: HashSet<i32>,
    authored_toc_bookmark_ids: HashSet<i32>,
    authored_bookmark_names: HashSet<String>,
    authored_story_parts: HashSet<String>,
    authored_story_relationship_ids: HashMap<String, HashSet<String>>,
    authored_nested_story_relationship_ids: HashMap<String, HashSet<String>>,
    authored_nested_story_drawing_ids: HashMap<String, HashSet<u32>>,
    authored_bundle_relationship_ids: HashMap<String, HashSet<String>>,
    provisional_relationship_ids: HashMap<String, HashSet<String>>,
    /// Main-part relationship ids named outside the body model: imported XML
    /// not inserted yet, or an id returned to a caller. Staging keeps each id
    /// until the main story references it or the id leaves the registry.
    pending_relationship_ids: HashMap<String, HashSet<String>>,
    /// Definitions read from each identifier part. A later scan replays a
    /// part only when its bytes and categories still match. This is the
    /// current scan, not a history of earlier ones.
    scanned_parts: Arc<HashMap<String, ScannedIdentifierPart>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScannedIdentifierPart {
    bytes: Arc<Vec<u8>>,
    categories: HashSet<IdentifierXmlCategory>,
    /// Definitions before a read error, or `None` when the part was not XML.
    definitions: Option<Vec<IdentifierDefinition>>,
}

fn hdr_ftr_type_order(value: HdrFtrType) -> u8 {
    match value {
        HdrFtrType::Default => 0,
        HdrFtrType::First => 1,
        HdrFtrType::Even => 2,
    }
}

/// Where a header or footer reference of `hdr_type` goes in `references`:
/// the place of the first one of its type, otherwise its place in the
/// default, first, even order among the others, which keep theirs.
fn story_reference_slot(references: &[HdrFtrRef], hdr_type: HdrFtrType) -> usize {
    references
        .iter()
        .position(|reference| reference.hdr_ftr_type == hdr_type)
        .or_else(|| {
            references.iter().position(|reference| {
                hdr_ftr_type_order(reference.hdr_ftr_type) > hdr_ftr_type_order(hdr_type)
            })
        })
        .unwrap_or(references.len())
}

/// Puts `reference` in `references`, in place of those of its type.
fn place_story_reference(references: &mut Vec<HdrFtrRef>, reference: HdrFtrRef) {
    let slot = story_reference_slot(references, reference.hdr_ftr_type);
    references.retain(|existing| existing.hdr_ftr_type != reference.hdr_ftr_type);
    references.insert(slot, reference);
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DrawingOccurrence {
    doc_pr: Option<(u32, Range<usize>)>,
    embeds: Vec<String>,
}

/// Restores the previous switch on return and on unwind.
#[cfg(debug_assertions)]
struct CanonicalBlockProofOff {
    previous: bool,
}

enum DrawingMut<'a> {
    Inline(&'a mut CT_Inline),
    Anchor(&'a mut CT_Anchor),
}

impl DrawingMut<'_> {
    fn remap_relationship(&mut self, remap: &HashMap<String, String>) {
        let (embed_id, chart_rel_id) = match self {
            Self::Inline(drawing) => (&mut drawing.embed_id, &mut drawing.chart_rel_id),
            Self::Anchor(drawing) => (&mut drawing.embed_id, &mut drawing.chart_rel_id),
        };
        if let Some(updated) = remap.get(embed_id) {
            embed_id.clone_from(updated);
        }
        if let Some(id) = chart_rel_id
            && let Some(updated) = remap.get(id)
        {
            id.clone_from(updated);
        }
    }
}

#[derive(Clone)]
struct StylesPartCache {
    bytes: Arc<Vec<u8>>,
    parsed: Arc<CT_Styles>,
    // The exact immutable model this writer successfully emitted. It is not
    // the parsed read model and must not replace that model on reopen.
    emitted_model: Option<Arc<CT_Styles>>,
    proof: StylesSerializationProof,
}

#[derive(Clone, Copy)]
enum StylesSerializationProof {
    Serializable,
    Unserializable,
}

/// Proof of the exact already-classified owner-free main-part allocation.
#[derive(Clone)]
struct LocalCellSource {
    key: String,
    main_part: String,
    bytes: Arc<Vec<u8>>,
    root: Vec<(String, String)>,
    body: Vec<(String, String)>,
}

impl LocalCellSource {
    fn classified(
        package: &OpcPackage,
        main_part: &str,
        root: &[(String, String)],
        body: &[(String, String)],
        owner_free: bool,
    ) -> Option<Self> {
        if !owner_free {
            return None;
        }
        let selected = package.get_part(main_part)?;
        let (key, bytes) = package
            .parts
            .iter()
            .find(|(_, bytes)| std::ptr::eq(bytes.as_slice(), selected))?;
        Some(Self {
            key: key.clone(),
            main_part: main_part.to_owned(),
            bytes: Arc::clone(bytes),
            root: root.to_vec(),
            body: body.to_vec(),
        })
    }

    fn applies(&self, document: &Document) -> bool {
        self.main_part == document.doc_part_name
            && self.root == document.root_namespace_declarations
            && self.body == document.body_namespace_declarations
            && document
                .package
                .parts
                .get(&self.key)
                .is_some_and(|bytes| Arc::ptr_eq(bytes, &self.bytes))
    }
}

/// Owns model value and invalidates clone equality before mutable access.
#[derive(Clone)]
pub(crate) struct DocumentModel {
    value: CT_Document,
    opening_clone: Option<Arc<()>>,
}

impl DocumentModel {
    pub(crate) fn unproved(value: CT_Document) -> Self {
        Self {
            value,
            opening_clone: None,
        }
    }

    fn opening_pair(value: CT_Document) -> (Self, Self) {
        let current = Self {
            value,
            opening_clone: Some(Arc::new(())),
        };
        let opened = current.clone();
        (current, opened)
    }

    fn same_opening_clone(&self, other: &Self) -> bool {
        self.opening_clone
            .as_ref()
            .zip(other.opening_clone.as_ref())
            .is_some_and(|(left, right)| Arc::ptr_eq(left, right))
    }

    pub(crate) fn into_inner(self) -> CT_Document {
        self.value
    }
}

impl std::ops::Deref for DocumentModel {
    type Target = CT_Document;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl std::ops::DerefMut for DocumentModel {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.opening_clone = None;
        &mut self.value
    }
}

impl PartialEq for DocumentModel {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl std::fmt::Debug for DocumentModel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.value.fmt(formatter)
    }
}

impl Default for DocumentModel {
    fn default() -> Self {
        Self::unproved(CT_Document::default())
    }
}

/// A Word document (.docx file).
///
/// This is the main entry point for reading, creating, and modifying
/// DOCX documents.
pub struct Document {
    pub(crate) package: OpcPackage,
    pub(crate) document: DocumentModel,
    /// Namespace declarations on the original document root.
    root_namespace_declarations: Vec<(String, String)>,
    /// Namespace declarations local to the original document body.
    body_namespace_declarations: Vec<(String, String)>,
    /// Namespace declarations in scope on the original document body.
    body_namespace_bindings: Vec<(String, String)>,
    /// The opened or saved package retains a shadowed serializer prefix, so a
    /// plain child edit must use the staged transaction instead of a local write.
    plain_local_edit_blocked: bool,
    local_cell_source: Option<LocalCellSource>,
    /// Save failure a checked body edit must return before it changes the model.
    /// Cleared when the typed body or the saved main part changes.
    unsavable_body_edit_cache: OnceLock<Option<String>>,
    /// The package and main-part reads from the last open. A staged clone
    /// shares this record. Reopening builds another one.
    parsed: Option<Arc<Parsed>>,
    /// Scans of the current main-part body children. A staged clone shares
    /// this record. A frame change replaces it. It is not a chain of older
    /// documents.
    main_body_scans: Arc<Mutex<MainBodyScans>>,
    /// The current main-part serialization record. A staged clone shares this
    /// value. Reopen keeps it only when the reopened part bytes match. It is
    /// not a chain of older documents.
    main_part_blocks: Option<MainPartBlocks>,
    pub(crate) styles: Arc<CT_Styles>,
    styles_part_cache: Option<StylesPartCache>,
    pub(crate) numbering: Option<CT_Numbering>,
    published_abstract_by_live: HashMap<u32, u32>,
    published_instance_by_live: HashMap<u32, u32>,
    /// Boxed, because debug builds keep many copies of `Document` on deep
    /// stacks, and the fifteen-field model would otherwise grow every one.
    pub(crate) core_properties: Option<Box<CoreProperties>>,
    pub(crate) application_properties: Option<AppProperties>,
    pub(crate) custom_properties: Option<CustomProperties>,
    /// Package part containing the core properties, resolved from `_rels/.rels`.
    core_properties_part_name: Option<String>,
    application_properties_part_name: Option<String>,
    custom_properties_part_name: Option<String>,
    custom_properties_owned: bool,
    /// Part name for the main document
    pub(crate) doc_part_name: String,
    /// Part name the styles were loaded from, and where they are written back.
    /// Resolved through the relationship rather than assumed, so a document
    /// that keeps its styles somewhere other than `/word/styles.xml` is
    /// updated in place instead of gaining an orphaned second part.
    styles_part_name: Option<String>,
    /// Part name for numbering definitions, resolved the same way.
    numbering_part_name: Option<String>,
    /// Typed document settings loaded through the main document relationship.
    pub(crate) settings: Option<CT_Settings>,
    /// Existing settings relationship target. No conventional target is assumed.
    settings_part_name: Option<String>,
    /// Whether this facade instance created an optional settings graph.
    settings_owned: bool,
    /// Typed web settings loaded through the main document relationship.
    web_settings: Option<CT_WebSettings>,
    /// Existing web settings relationship target. No conventional target is
    /// assumed.
    web_settings_part_name: Option<String>,
    /// Whether this facade instance created an optional web settings graph.
    web_settings_owned: bool,
    /// Typed shared DrawingML theme loaded through the main-document relationship.
    theme: Option<oxoffice_drawing::theme::CT_OfficeStyleSheet>,
    /// Existing theme relationship target.
    theme_part_name: Option<String>,
    /// Whether the theme model must be serialized into the package.
    theme_dirty: bool,
    /// Typed font table loaded through the main-document relationship.
    font_table: Option<FontTable>,
    /// Existing font-table relationship target.
    font_table_part_name: Option<String>,
    /// Whether the font-table model must be serialized into the package.
    font_table_dirty: bool,
    /// Shared package and WordprocessingML identifier allocation state.
    pub(crate) identifiers: DocumentIdentifiers,
    /// Typed footnotes loaded through the main document relationship.
    pub(crate) footnotes: oxoffice_docx_oxml::footnotes::CT_Footnotes,
    /// Existing footnotes relationship target. No conventional target is assumed on read.
    pub(crate) footnotes_part_name: Option<String>,
    /// Producer root namespaces inherited by retained footnote raw XML.
    footnotes_root_namespace_declarations: Vec<(String, String)>,
    /// Whether a facade mutation requires complete typed footnote serialization.
    pub(crate) footnotes_dirty: bool,
    /// Typed comments loaded through the main document relationship.
    pub(crate) comments: Option<oxoffice_docx_oxml::comments::CT_Comments>,
    /// Existing comments relationship target. No target is invented on read.
    pub(crate) comments_part_name: Option<String>,
    /// Whether a typed comment mutation must be published into the package.
    pub(crate) comments_dirty: bool,
    /// Typed reply linkage and resolved state for comments.
    pub(crate) comments_extended: Option<oxoffice_docx_oxml::comments_extended::CT_CommentsEx>,
    /// Existing comments-extended relationship target.
    pub(crate) comments_extended_part_name: Option<String>,
    /// Whether this facade created the comments part and may remove it when empty.
    pub(crate) comments_owned: bool,
    /// Whether this facade created the comments-extended part and may remove it when empty.
    pub(crate) comments_extended_owned: bool,
    /// Embedded identities whose retained signature evidence is known invalid.
    pub(crate) embedded_invalidated_signatures: HashSet<(String, String)>,
    /// Whether retained package signature evidence is known invalid.
    pub(crate) package_signatures_invalidated: bool,
    /// The file this document was opened from, canonical, which `save`
    /// refuses to replace. Every staged clone and reopened candidate carries it.
    source_path: Option<PathBuf>,
    /// Relationship-resolved glossary and building-block entries.
    pub(crate) glossary: Option<oxoffice_docx_oxml::glossary::CT_GlossaryDocument>,
    /// Existing glossary relationship target.
    pub(crate) glossary_part_name: Option<String>,
    /// Whether a bounded facade replacement changed the glossary model.
    pub(crate) glossary_dirty: bool,
    /// Normal layout, including system font discovery, computed on first use.
    layout_cache: Mutex<Option<Arc<oxoffice_docx_layout::WordLayoutResult>>>,
    /// Reusable normal-font engine retained across document edits.
    normal_layout_engine: Mutex<Option<oxoffice_docx_layout::engine::Engine>>,
    /// Bundled-font-only layout used by deterministic rendering.
    deterministic_layout_cache: Mutex<Option<Arc<oxoffice_docx_layout::WordLayoutResult>>>,
    /// Reusable bundled-font engine with caller faces at highest priority.
    bundled_fallback_layout_engine: Mutex<Option<oxoffice_docx_layout::engine::Engine>>,
}

/// The actual source authority of a consumed staged document. No model copy.
struct StagedCommitOrigin {
    package: OpcPackage,
    signature_invalidated: bool,
    normal_engine: Mutex<Option<oxoffice_docx_layout::engine::Engine>>,
    bundled_engine: Mutex<Option<oxoffice_docx_layout::engine::Engine>>,
}

/// Package and main-part reads kept for the document opened from them.
/// The record is the current opening only.
struct Parsed {
    package: OpcPackage,
    /// `OpcPackage::reread` applies only to a package read from bytes.
    read_from_bytes: bool,
    main_part: String,
    root_namespace_declarations: Vec<(String, String)>,
    body_namespace_declarations: Vec<(String, String)>,
    body_namespace_bindings: Vec<(String, String)>,
    /// `document.to_xml()` at open. Later reuse must serialize to these bytes.
    serialization_proof: Option<Vec<u8>>,
    main_document: OnceLock<Option<DocumentModel>>,
    initial_writer_blocks: Option<std::sync::Weak<Vec<Arc<BodyBlockScan>>>>,
    nested_namespace_owners: OnceLock<Option<Vec<NestedNamespaceOwner>>>,
    /// Actual source reads for this opening, independently of writer reuse.
    source_blocks: OnceLock<Option<MainPartBlocks>>,
    unchanged_main_part: OnceLock<Option<UnchangedMainPart>>,
}

/// Main-part reads taken while canonicalizing a part with nothing to renumber.
#[derive(Clone, PartialEq)]
struct UnchangedMainPart {
    xml: Vec<u8>,
    body_namespace_bindings: Vec<(String, String)>,
    drawing_ids: Option<HashSet<u32>>,
}

impl Parsed {
    fn of(
        package: &OpcPackage,
        main_part: &str,
        read_from_bytes: bool,
        root_namespace_declarations: Vec<(String, String)>,
        body_namespace_declarations: Vec<(String, String)>,
        body_namespace_bindings: Vec<(String, String)>,
        serialization_proof: Option<Vec<u8>>,
    ) -> Self {
        Self {
            package: package.clone(),
            read_from_bytes,
            main_part: main_part.to_owned(),
            root_namespace_declarations,
            body_namespace_declarations,
            body_namespace_bindings,
            serialization_proof,
            main_document: OnceLock::new(),
            initial_writer_blocks: None,
            nested_namespace_owners: OnceLock::new(),
            source_blocks: OnceLock::new(),
            unchanged_main_part: OnceLock::new(),
        }
    }

    fn main_part_xml(&self) -> Option<&[u8]> {
        self.package.get_part(&self.main_part)
    }

    fn main_document(&self) -> Option<&CT_Document> {
        self.main_document
            .get_or_init(|| {
                let xml = self.main_part_xml()?;
                CT_Document::from_xml(xml).ok().map(DocumentModel::unproved)
            })
            .as_ref()
            .map(|model| &**model)
    }

    fn nested_namespace_owners(&self) -> Option<&[NestedNamespaceOwner]> {
        self.nested_namespace_owners
            .get_or_init(|| {
                let xml = self.main_part_xml()?;
                nested_modeled_namespace_owners(xml).ok()
            })
            .as_deref()
    }
}

/// Whether every header and footer reference of a section keeps a relationship
/// read with the document.
fn section_references_were_read(section: &CT_SectPr, preserved: &HashSet<String>) -> bool {
    section
        .header_refs
        .iter()
        .chain(&section.footer_refs)
        .all(|reference| preserved.contains(&reference.rel_id))
}

enum ChartPackageSource<'a> {
    #[allow(dead_code)]
    Typed {
        chart: &'a CT_ChartSpace,
        workbook: &'a Workbook,
    },
    Authored {
        kind: ChartKind,
        data: &'a ChartData,
    },
}

/// Fallback part names used when a document does not already declare one.
const DEFAULT_STYLES_PART: &str = "/word/styles.xml";
const DEFAULT_NUMBERING_PART: &str = "/word/numbering.xml";
const DEFAULT_CORE_PROPERTIES_PART: &str = "/docProps/core.xml";
const DEFAULT_APP_PROPERTIES_PART: &str = "/docProps/app.xml";
const DEFAULT_FONT_TABLE_PART: &str = "/word/fontTable.xml";
const DEFAULT_THEME_PART: &str = "/word/theme/theme1.xml";
const STYLES_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml";
const NUMBERING_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml";
const CORE_PROPERTIES_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
const SETTINGS_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml";
const DEFAULT_SETTINGS_PART: &str = "/word/settings.xml";
const WEB_SETTINGS_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.webSettings+xml";
const DEFAULT_WEB_SETTINGS_PART: &str = "/word/webSettings.xml";
const FONT_TABLE_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.fontTable+xml";
const EMBEDDED_FONT_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.obfuscatedFont";
/// Plain sfnt faces, as Google Docs and LibreOffice store them. These parts
/// are copied when the body already has an sfnt header.
const PLAIN_EMBEDDED_FONT_CONTENT_TYPES: [&str; 9] = [
    "application/x-font-ttf",
    "application/x-font-otf",
    "application/x-font-truetype",
    "application/x-font-opentype",
    "application/font-sfnt",
    "application/vnd.ms-opentype",
    "font/ttf",
    "font/otf",
    "font/sfnt",
];
const DEFAULT_FONT_TABLE_XML: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    r#"<w:fonts xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
    r#"<w:font w:name="Calibri"/><w:font w:name="Times New Roman"/>"#,
    r#"</w:fonts>"#,
);

pub(crate) fn relationship_is_internal(
    relationship: &oxoffice_opc::relationship::Relationship,
) -> bool {
    matches!(relationship.target_mode.as_deref(), None | Some("Internal"))
}

fn bundle_relationship_order(rel_type: &str) -> Option<u8> {
    match rel_type {
        rel_types::STYLES => Some(0),
        rel_types::SETTINGS => Some(1),
        rel_types::NUMBERING => Some(2),
        rel_types::FOOTNOTES => Some(3),
        rel_types::COMMENTS => Some(4),
        crate::comments::COMMENTS_EXTENDED_REL_TYPE => Some(5),
        _ => None,
    }
}

fn bundle_relationship_precedes_story(rel_type: &str) -> bool {
    matches!(rel_type, rel_types::STYLES | rel_types::SETTINGS)
}

#[cfg(debug_assertions)]
fn assert_same_reopen(reopened: &Result<Document>, through_zip: &Result<Document>) {
    match (reopened, through_zip) {
        (Ok(reopened), Ok(expected)) => {
            assert_eq!(reopened.document, expected.document);
            assert_eq!(reopened.identifiers, expected.identifiers);
            assert!(reopened.package.same_package_as(&expected.package));
            let saved = |document: &Document| {
                let mut copy = document.clone_for_debug_reference();
                copy.to_bytes().map_err(|error| error.to_string())
            };
            assert_eq!(saved(reopened), saved(expected));
        }
        (Err(reopened), Err(expected)) => {
            assert_eq!(reopened.to_string(), expected.to_string());
        }
        (reopened, expected) => panic!(
            "in-memory reopen {} zip {}",
            reopened.as_ref().map(|_| "ok").unwrap_or("err"),
            expected
                .as_ref()
                .err()
                .map(|error| error.to_string())
                .unwrap_or_else(|| "ok".to_string())
        ),
    }
}

/// Compare an already initialized retained read without warming its shared cache.
#[cfg(debug_assertions)]
fn assert_kept_main_document_without_initializing(
    kept: Option<&Parsed>,
    source: Option<&[u8]>,
    current: &CT_Document,
) {
    let fresh = source.and_then(|xml| CT_Document::from_xml(xml).ok());
    let fresh_matches = fresh.as_ref() == Some(current);
    if let Some(cached) = kept.and_then(|kept| kept.main_document.get()) {
        assert_eq!(
            cached.as_ref().map(|model| &**model) == Some(current),
            fresh_matches,
            "the kept main part parse disagrees with a fresh parse"
        );
    }
    assert!(
        fresh_matches,
        "opened-main serialization proof disagrees with a parse"
    );
}

#[cfg(debug_assertions)]
fn assert_same_canonicalization(
    document: &Document,
    result: &Result<()>,
    full: &Document,
    full_result: &Result<()>,
) {
    match (result, full_result) {
        (Ok(()), Ok(())) => {}
        (Err(error), Err(full_error)) => assert_eq!(error.to_string(), full_error.to_string()),
        (result, full_result) => panic!(
            "canonicalizing beside the unchanged main part gives {result:?} where the whole \
             canonicalization gives {full_result:?}"
        ),
    }
    assert!(
        document.document == full.document
            && document.document.to_xml().ok() == full.document.to_xml().ok(),
        "canonicalizing beside the unchanged main part changes the model otherwise"
    );
    assert!(
        document.package.same_package_as(&full.package),
        "canonicalizing beside the unchanged main part changes the package otherwise"
    );
    assert!(
        document.identifiers == full.identifiers,
        "canonicalizing beside the unchanged main part changes the identifiers otherwise"
    );
}

fn direct_body_paragraph_slot(location: &ContentLocation) -> Option<usize> {
    if location.story().kind() != StoryKind::Body || location.index_path().len() != 1 {
        return None;
    }
    location.index_path().first().copied()
}

fn paragraph_is_plain(paragraph: &CT_P) -> bool {
    paragraph.comment_ranges.is_empty()
        && paragraph.content_controls.is_empty()
        && paragraph.hyperlinks.is_empty()
        && paragraph.revisions.is_empty()
        && paragraph.equations.is_empty()
        && paragraph.rubies.is_empty()
        && paragraph.extra_xml.is_empty()
        && paragraph.bookmark_markers.is_empty()
        && paragraph.runs.iter().all(|run| {
            run.extra_xml.is_empty()
                && run.alt_drawings.is_empty()
                && run.content.iter().all(|content| {
                    matches!(
                        content,
                        RunContent::Text(_)
                            | RunContent::Tab
                            | RunContent::Break(_)
                            | RunContent::Symbol { .. }
                            | RunContent::SpecialCharacter(_)
                    )
                })
        })
}

// The finite ordinary value language excludes private read identities.
fn paragraph_has_finite_plain_properties<'a>(
    paragraph: &'a CT_P,
    qualified_properties: &mut Option<&'a oxoffice_docx_oxml::properties::CT_RPr>,
) -> bool {
    if paragraph.properties.is_some() || !paragraph_is_plain(paragraph) {
        return false;
    }
    for run in &paragraph.runs {
        if !run.extra_xml_positions.is_empty()
                        || !run.content.iter().all(|content| matches!(content,
                            RunContent::Text(text) if text.text.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b' ')))
                    {
                        return false;
                    }
        if let Some(properties) = &run.properties {
            if *qualified_properties == Some(properties) {
                continue;
            }
            let shading = properties.shading.as_ref().map(|value| {
                Box::new(oxoffice_docx_oxml::properties::CT_Shd {
                    val: value.val.clone(),
                    color: value.color.clone(),
                    fill: value.fill.clone(),
                    theme_color: value.theme_color.clone(),
                    theme_tint: value.theme_tint,
                    theme_shade: value.theme_shade,
                    theme_fill: value.theme_fill.clone(),
                    theme_fill_tint: value.theme_fill_tint,
                    theme_fill_shade: value.theme_fill_shade,
                    extra_attributes: Vec::new(),
                })
            });
            let allowed = oxoffice_docx_oxml::properties::CT_RPr {
                font_ascii: properties.font_ascii.clone(),
                font_hansi: properties.font_hansi.clone(),
                font_east_asia: properties.font_east_asia.clone(),
                font_cs: properties.font_cs.clone(),
                bold: properties.bold,
                italic: properties.italic,
                small_caps: properties.small_caps,
                strike: properties.strike,
                color: properties.color.clone(),
                sz: properties.sz,
                sz_cs: properties.sz_cs,
                underline: properties.underline,
                shading,
                vert_align: properties.vert_align.clone(),
                rtl: properties.rtl,
                ..Default::default()
            };
            if properties != &allowed {
                return false;
            }
            *qualified_properties = Some(properties);
        }
    }
    true
}

fn direct_body_paragraph_context(_document: &Document) -> EditedParagraphContext {
    let mut context = EditedParagraphContext::default();
    // `CT_P::to_xml` writes these prefixes, and `CT_Document::to_xml` declares
    // them on `w:document`. Stored `xmlns` records are attribute names, not
    // prefixes, so they are not copied into this map.
    for (prefix, uri) in [
        ("w", W_NS),
        ("r", oxoffice_docx_oxml::namespace::R_NS),
        ("mc", MC_NS),
        ("wp", drawing_ns::WP),
    ] {
        context.namespaces.insert(prefix.to_owned(), uri.to_owned());
    }
    context
}

struct StorySource<'a> {
    root_kind: StoryKind,
    part_name: String,
    xml: Cow<'a, [u8]>,
}

pub(crate) struct TextBoxCacheParagraph {
    pub(crate) location: ContentLocation,
    pub(crate) xml: Vec<u8>,
    pub(crate) owner_index: usize,
    pub(crate) paragraph_index: usize,
}

#[derive(Clone)]
pub(crate) struct StoryOwnerSpan {
    pub(crate) kind: StoryKind,
    owner_index: usize,
    pub(crate) full: Range<usize>,
    fingerprint: u64,
}

#[derive(Clone)]
struct StoryItemSpan {
    kind: StoryItemKind,
    full: Range<usize>,
    scan: Range<usize>,
    direct_owner_child: bool,
    complex_field: bool,
    complex_ancestors: Vec<usize>,
    sdt_context: Option<StorySdtContext>,
}

impl StoryItemSpan {
    fn moved(&self, to: impl Fn(usize, bool) -> usize) -> Self {
        Self {
            full: to(self.full.start, false)..to(self.full.end, true),
            scan: to(self.scan.start, false)..to(self.scan.end, true),
            complex_ancestors: self
                .complex_ancestors
                .iter()
                .map(|at| to(*at, false))
                .collect(),
            ..self.clone()
        }
    }
}

/// Scans of the current main-part body children.
///
/// Each record is the exact child bytes plus the head and tail passed to
/// `scan_body_block`. Those slices are the scanner input. A field, tab, or
/// bookmark changes the child bytes, so the next lookup cannot treat the old
/// record as the same input. A namespace or body-frame change changes the
/// head or tail and drops the record. Reopen copies this record only when
/// the reopened part serializes that same head and tail. Each subsequent
/// lookup aligns exact current child bytes before using any carried fact.
/// It does not copy model field caches or relationship checks.
#[derive(Clone, Default)]
struct MainBodyScans {
    head: Vec<u8>,
    tail: Vec<u8>,
    frame: Option<Arc<BodyBlockScan>>,
    blocks: Vec<Arc<BodyBlockScan>>,
    fingerprint: Option<u64>,
    /// Current writer-produced pieces. This is not a flushed package part.
    /// It keeps no assembled part bytes and never supplies unread owner facts.
    serialized: Option<MainPartBlocks>,
}

#[derive(Clone)]
struct BodyBlockScan {
    bytes: Vec<u8>,
    owners: Vec<StoryOwnerSpan>,
    items: Option<Vec<StoryItemSpan>>,
    /// The body child that an actual writer proved serializes to these bytes.
    /// Layout and source-only read scans leave this empty. Equality with the
    /// live child is not a parsed model.
    staged: Option<Arc<BodyContent>>,
    /// Fixed standalone value keys for one root, not Part-child or Source proof.
    standalone_value: OnceLock<Arc<Mutex<Option<StandaloneParagraphValue>>>>,
    /// One parse of `bytes` under the root and body bindings recorded with it.
    /// Empty until a read. `None` inside means that read could not prove one child.
    proved: OnceLock<ParsedChild>,
    /// `xmlns` occurs in `bytes`. This is a byte fact for these exact bytes.
    declares_namespace: bool,
    /// Exact lexical field-token result for these immutable bytes, not model facts.
    holds_field_words: bool,
    /// Definitions in `bytes` under one root and body scope. `None` is a failed
    /// scan and is not an empty success.
    identifier_proof: OnceLock<Option<IdentifierChildProof>>,
    /// Successful relationship and drawing facts for `bytes` under one scope.
    /// `None` is a failed scan, not an empty success.
    canonical_read: OnceLock<Option<CanonicalChildFacts>>,
    /// Successful empty nested-owner proof for these exact bytes under one
    /// root, body, head and tail. `None` is nonempty, failed, or unproved.
    /// It is not an empty owner list. Q22 already covers this namespace layer.
    empty_namespace_owners: OnceLock<Option<EmptyNamespaceChildProof>>,
    /// Successful modeled-owner spans for these exact bytes under one root,
    /// body, head and tail. Offsets are relative to `bytes`. `None` is an
    /// ineligible or failed parse, not an empty span list.
    namespace_spans: OnceLock<Option<NamespaceSpanProof>>,
    decodes: OnceLock<bool>,
    scoped_correspondence: OnceLock<ScopedChildCorrespondence>,
}

#[derive(Clone)]
/// Head and tail relationship ids under one closed frame.
///
/// `bound` uses the body bindings. `plain` uses none. Empty body bindings
/// store the same ids in both. A drawing, an invalid shell, or a head scan
/// that is not a prefix of the shell is not stored.
struct CanonicalFrameProof {
    root_declarations: Vec<(String, String)>,
    body_declarations: Vec<(String, String)>,
    body_bindings: Vec<(String, String)>,
    head: Arc<Vec<u8>>,
    tail: Arc<Vec<u8>>,
    head_bound_relationship_ids: Vec<String>,
    tail_bound_relationship_ids: Vec<String>,
    head_plain_relationship_ids: Vec<String>,
    tail_plain_relationship_ids: Vec<String>,
}

#[derive(Clone)]
struct CanonicalChildFacts {
    root_declarations: Vec<(String, String)>,
    body_declarations: Vec<(String, String)>,
    body_bindings: Vec<(String, String)>,
    head: Arc<Vec<u8>>,
    tail: Arc<Vec<u8>>,
    bound_relationship_ids: Vec<String>,
    plain_relationship_ids: Vec<String>,
    /// `doc_pr` spans are relative to this child's bytes.
    drawings: Vec<DrawingOccurrence>,
    drawing_ids: HashSet<u32>,
}

/// Canonical facts tied to one immutable document phase, never retained.
struct PreparedMainRead<'a> {
    document: &'a Document,
    xml: Vec<u8>,
    writer: Option<MainPartBlocks>,
    content: Range<usize>,
    children: Vec<Range<usize>>,
}

/// One current main-part byte partition, from a writer or a checked source.
/// `frame` is this head and tail actually parsed as an empty-body shell. A
/// producer whitespace frame must normalize to the proved canonical shell.
/// This is not a second whole-body AST. A serialized child is not that shell,
/// and a source-only parsed child does not supply writer reuse.
struct MainPartBlocks {
    head: Vec<u8>,
    root: std::ops::Range<usize>,
    tail: Vec<u8>,
    blocks: Arc<Vec<Arc<BodyBlockScan>>>,
    part: Option<Arc<Vec<u8>>>,
    /// Head and tail parsed with an empty body, when that shell proves.
    frame: std::sync::OnceLock<Option<CT_Document>>,
    /// Nested owners for `part`, when a scan or an empty proved carry filled them.
    nested_namespace_owners: std::sync::OnceLock<Option<Vec<NestedNamespaceOwner>>>,
    /// Identifier definitions in the empty-body head and tail. `None` is an
    /// unproved shell, not an empty definition list.
    identifier_frame: std::sync::OnceLock<Option<FrameIdentifierProof>>,
    /// Relationship ids contributed by this head and tail. Drawings stay absent.
    canonical_frame: std::sync::OnceLock<Option<CanonicalFrameProof>>,
    /// Empty nested-owner policy for this head and tail. `None` is an unproved
    /// shell, not an empty owner list. Q22 already covers this namespace layer.
    /// The added state is this fact plus `BodyBlockScan::empty_namespace_owners`.
    namespace_frame: std::sync::OnceLock<Option<FrameNamespacePolicyEmpty>>,
    /// Modeled owner spans of this head and tail. `None` is an unproved shell,
    /// not an empty span list. Q22 already covers this namespace layer.
    namespace_frame_spans: std::sync::OnceLock<Option<NamespaceFrameSpans>>,
}

// Operation-local output handles. They retain the actual emitted sources
// while the legacy block record owns its separately accounted byte copy.
// Immutable output records which already exist at this operation's ingress.
// Sharing their handles copies metadata only. Selection still uses the exact
// record chosen by the existing writer and pays every actual later byte copy.
#[derive(Default)]
pub(crate) struct TemplateWordRecordInputs {
    blocks: BTreeMap<usize, Arc<BodyBlockScan>>,
    openings: BTreeMap<(usize, usize), Arc<Parsed>>,
}

pub(crate) struct TemplateMainSources {
    pub(crate) source: Option<Arc<oxoffice_docx_oxml::document::ScopedWordSource>>,
    reader_source: Option<Arc<oxoffice_docx_oxml::document::ScopedWordSource>>,
    comments_reader_source: Option<Arc<oxoffice_docx_oxml::document::ScopedWordSource>>,
    published_main: Option<(
        Arc<Vec<u8>>,
        Arc<oxoffice_docx_oxml::document::ScopedWordSource>,
    )>,
    read_note_count: usize,
    notes_model_read: bool,
    related: BTreeMap<
        String,
        (
            Arc<Vec<u8>>,
            Arc<oxoffice_docx_oxml::document::ScopedWordSource>,
        ),
    >,
    items: Vec<Option<oxoffice_docx_oxml::document::ScopedWordSource>>,
    blocks: Vec<std::sync::Weak<BodyBlockScan>>,
    model_lineage_valid: bool,
    first_visit: bool,
    original_part: Option<(String, usize)>,
}

/// The root start tag of emitted item bytes binds `w` to a namespace other
/// than Word. Only the root start tag is scanned, so a matching byte sequence
/// inside a text run or opaque payload cannot disable the span check.
#[cfg(debug_assertions)]
fn root_declares_foreign_word(bytes: &[u8]) -> bool {
    let Some(end) = bytes.iter().position(|byte| *byte == b'>') else {
        return false;
    };
    let root = &bytes[..end];
    const NEEDLE: &[u8] = b"xmlns:w=\"";
    let mut cursor = 0usize;
    while let Some(offset) = root[cursor..]
        .windows(NEEDLE.len())
        .position(|window| window == NEEDLE)
    {
        let value_start = cursor + offset + NEEDLE.len();
        let Some(length) = root[value_start..].iter().position(|byte| *byte == b'"') else {
            return true;
        };
        let value = &root[value_start..value_start + length];
        if value != WORD_NAMESPACE.as_bytes() {
            return true;
        }
        cursor = value_start + length + 1;
    }
    false
}

struct MainStoryLayout {
    content: Range<usize>,
    frame: Arc<BodyBlockScan>,
    blocks: Vec<(usize, Arc<BodyBlockScan>)>,
    fingerprint: Option<u64>,
}

#[cfg(debug_assertions)]
fn assert_main_story_layout(xml: &[u8], layout: &MainStoryLayout, items: bool) {
    let whole = scan_story_owners(xml, StoryKind::Body).expect("the main part scans whole");
    let owners = layout
        .owners(xml)
        .expect("the main story layout gives its owners");
    let key = |owner: &StoryOwnerSpan| {
        (
            owner.kind,
            owner.owner_index,
            owner.full.clone(),
            owner.fingerprint,
        )
    };
    assert_eq!(
        owners.iter().map(key).collect::<Vec<_>>(),
        whole.iter().map(key).collect::<Vec<_>>()
    );
    if !items {
        return;
    }
    let body = whole
        .iter()
        .find(|owner| owner.kind == StoryKind::Body)
        .expect("body");
    let whole_items = scan_story_items(xml, body, None).expect("body items");
    let item_key = |item: &StoryItemSpan| (item.kind, item.full.clone(), item.direct_owner_child);
    assert_eq!(
        layout.body_items().iter().map(item_key).collect::<Vec<_>>(),
        whole_items.iter().map(item_key).collect::<Vec<_>>()
    );
}

struct StoryLinkSpan {
    full: Range<usize>,
    rel_id: Option<String>,
    anchor: Option<String>,
}

struct XmlElementFrame {
    namespace: StoryNamespace,
    local_name: Cow<'static, [u8]>,
    is_word: bool,
    full_start: usize,
    owner_kind: Option<StoryKind>,
    item_kind: Option<StoryItemKind>,
    direct_owner_child: bool,
    sdt_context: Option<StorySdtContext>,
    word_prefixes: Arc<Vec<String>>,
    opaque: bool,
    typed_sdt_content_seen: bool,
}

#[derive(Clone, Default)]
struct EditedParagraphContext {
    namespaces: BTreeMap<String, String>,
    ignorable: HashSet<String>,
}

/// Hand vertical merges started by `row_index` to the row below before removal.
fn restart_merges_continued_below(table: &mut CT_Tbl, row_index: usize) -> Result<()> {
    let grid_columns = table
        .grid
        .as_ref()
        .map(|grid| grid.columns.len())
        .ok_or_else(|| Error::Other("table has no active grid".to_owned()))?;
    let Some([row, below]) = table.rows.get_mut(row_index..row_index + 2) else {
        return Ok(());
    };
    let restarted = row_cell_ranges(row, grid_columns)?
        .into_iter()
        .zip(&row.cells)
        .filter_map(|(range, cell)| {
            (cell
                .properties
                .as_ref()
                .and_then(|properties| properties.v_merge.as_ref())
                == Some(&VMerge::Restart))
            .then_some(range)
        })
        .collect::<HashSet<_>>();
    for (range, cell) in row_cell_ranges(below, grid_columns)?
        .into_iter()
        .zip(&mut below.cells)
    {
        if restarted.contains(&range)
            && let Some(properties) = cell.properties.as_mut()
            && properties.v_merge == Some(VMerge::Continue)
        {
            properties.v_merge = Some(VMerge::Restart);
        }
    }
    Ok(())
}

struct PairedTextBoxCopy {
    alternate: usize,
    branch: usize,
    range: Range<usize>,
}

struct PairedTextEdit {
    range: Range<usize>,
    raw: Vec<u8>,
}

// Bounded physical correspondence inside one admitted AlternateContent.
struct PairedTextBoxProjection {
    signature: Vec<String>,
    slots: Vec<(Vec<usize>, Range<usize>)>,
}

struct PairedReplacement<'a> {
    span: &'a Range<usize>,
    paragraph: Option<usize>,
    placeholder: &'a str,
    replacement: &'a str,
    literal_input: &'a Cell<bool>,
    count: usize,
}

/// One admitted bookmark marker and its physical source range.
struct BookmarkMarkerSpan {
    start: bool,
    id: String,
    span: Range<usize>,
}

impl Document {
    /// Create a new Word-compatible DOCX document with default page setup and styles.
    pub fn new() -> Self {
        Self::new_with_profile(WordCreationProfile::WordCompatible(
            WordPackageClass::Document,
        ))
    }

    /// Create a new document with explicit package completeness and class.
    pub fn new_with_profile(profile: WordCreationProfile) -> Self {
        let document = CT_Document::new();
        let (class, compatible) = match profile {
            WordCreationProfile::Minimal(class) => (class, false),
            WordCreationProfile::WordCompatible(class) => (class, true),
        };
        let mut styles = CT_Styles::new_default();
        if compatible {
            styles.styles.extend(style::common_styles());
        }
        // Word writes compatibility mode 15 into every document it creates.
        // A document that names no mode opens in Compatibility Mode and lays
        // out under the legacy rules (see `build_layout_input`).
        let settings = compatible.then(|| {
            let mut settings = CT_Settings::new();
            settings
                .set_compatibility_setting(CompatibilitySetting {
                    name: "compatibilityMode".to_owned(),
                    uri: "http://schemas.microsoft.com/office/word".to_owned(),
                    value: "15".to_owned(),
                })
                .expect("fresh Word settings accept a compatibility setting");
            settings
        });
        let core_properties = compatible.then(Box::<CoreProperties>::default);
        let application_properties = compatible.then(default_application_properties);
        let mut package = if compatible {
            new_word_compatible_package(
                class,
                &document,
                &styles,
                settings.as_ref().expect("compatible settings exist"),
                core_properties
                    .as_deref()
                    .expect("compatible core properties exist"),
                application_properties
                    .as_ref()
                    .expect("compatible application properties exist"),
            )
        } else {
            new_word_package(class)
        };
        if compatible {
            validate_fresh_word_compatible_package(&package, class)
                .expect("fresh Word-compatible package graph must validate");
        }

        if !compatible {
            package
                .get_or_create_part_rels("/word/document.xml")
                .add(rel_types::STYLES, "styles.xml");
        }
        let identifiers = DocumentIdentifiers::scan(&package)
            .expect("a freshly constructed package has valid identifiers");

        let styles = Arc::new(styles);
        // Preserve the authored writer model while recording a genuine read
        // model for a later same-package reopen.
        let styles_part_cache = package.get_part_shared(DEFAULT_STYLES_PART).map(|bytes| {
            let parsed =
                Arc::new(CT_Styles::from_xml(bytes).expect("freshly emitted styles must parse"));
            let proof = if parsed.to_xml().is_ok() {
                StylesSerializationProof::Serializable
            } else {
                StylesSerializationProof::Unserializable
            };
            StylesPartCache {
                bytes: Arc::clone(bytes),
                parsed,
                emitted_model: Some(Arc::clone(&styles)),
                proof,
            }
        });
        let local_cell_source = package.get_part("/word/document.xml").and_then(|xml| {
            let (blocked, _) = plain_local_edit_facts(&[], &[], xml, &OnceLock::new());
            LocalCellSource::classified(&package, "/word/document.xml", &[], &[], !blocked)
        });
        Document {
            package,
            document: crate::document::DocumentModel::unproved(document),
            root_namespace_declarations: Vec::new(),
            body_namespace_declarations: Vec::new(),
            body_namespace_bindings: Vec::new(),
            plain_local_edit_blocked: false,
            local_cell_source,
            unsavable_body_edit_cache: OnceLock::from(None),
            styles,
            styles_part_cache,
            numbering: None,
            published_abstract_by_live: HashMap::new(),
            published_instance_by_live: HashMap::new(),
            core_properties,
            application_properties,
            custom_properties: None,
            core_properties_part_name: compatible.then(|| DEFAULT_CORE_PROPERTIES_PART.to_owned()),
            application_properties_part_name: compatible
                .then(|| DEFAULT_APP_PROPERTIES_PART.to_owned()),
            custom_properties_part_name: None,
            custom_properties_owned: false,
            doc_part_name: "/word/document.xml".to_string(),
            styles_part_name: Some(DEFAULT_STYLES_PART.to_owned()),
            numbering_part_name: None,
            settings,
            settings_part_name: compatible.then(|| DEFAULT_SETTINGS_PART.to_owned()),
            settings_owned: false,
            // A fresh package gains no web settings part. An ordinary save
            // never creates a part the document did not already have.
            web_settings: None,
            web_settings_part_name: None,
            web_settings_owned: false,
            theme: compatible.then(oxoffice_drawing::theme::CT_OfficeStyleSheet::office_default),
            theme_part_name: compatible.then(|| DEFAULT_THEME_PART.to_owned()),
            theme_dirty: false,
            font_table: compatible.then(|| {
                FontTable::from_xml(DEFAULT_FONT_TABLE_XML.as_bytes())
                    .expect("fresh Word font table must parse")
            }),
            font_table_part_name: compatible.then(|| DEFAULT_FONT_TABLE_PART.to_owned()),
            font_table_dirty: false,
            identifiers,
            footnotes: oxoffice_docx_oxml::footnotes::CT_Footnotes::new(),
            footnotes_part_name: None,
            footnotes_root_namespace_declarations: Vec::new(),
            footnotes_dirty: false,
            comments: None,
            comments_part_name: None,
            comments_dirty: false,
            comments_extended: None,
            comments_extended_part_name: None,
            comments_owned: false,
            comments_extended_owned: false,
            embedded_invalidated_signatures: HashSet::new(),
            package_signatures_invalidated: false,
            source_path: None,
            glossary: None,
            glossary_part_name: None,
            glossary_dirty: false,
            layout_cache: Mutex::new(None),
            normal_layout_engine: Mutex::new(None),
            deterministic_layout_cache: Mutex::new(None),
            bundled_fallback_layout_engine: Mutex::new(None),
            parsed: None,
            main_body_scans: Arc::new(Mutex::new(MainBodyScans::default())),
            main_part_blocks: None,
        }
    }

    /// Admit each loaded recursive owner before staging clones it.
    pub(crate) fn validate_model_resource_limits(&self) -> Result<()> {
        self.document.validate_resource_limits()?;
        for note in &self.footnotes.footnotes {
            for paragraph in &note.paragraphs {
                paragraph.validate_resource_limits()?;
            }
        }
        if let Some(comments) = &self.comments {
            for comment in &comments.comments {
                for paragraph in &comment.paragraphs {
                    paragraph.validate_resource_limits()?;
                }
            }
        }
        if let Some(glossary) = &self.glossary {
            glossary.validate_resource_limits()?;
        }
        let mut field_error = None;
        let mut preflight = |paragraph: &CT_P| {
            if field_error.is_some() {
                return;
            }
            for run in paragraph.source_runs() {
                for content in &run.content {
                    if let oxoffice_docx_oxml::text::RunContent::Field(field) = content
                        && let Err(error) = crate::field::preflight_current_field(field)
                    {
                        field_error = Some(error);
                        return;
                    }
                }
            }
        };
        visit_body_paragraphs(&self.document.body.content, false, &mut preflight);
        for note in &self.footnotes.footnotes {
            for paragraph in &note.paragraphs {
                preflight(paragraph);
            }
        }
        if let Some(comments) = &self.comments {
            for comment in &comments.comments {
                for paragraph in &comment.paragraphs {
                    preflight(paragraph);
                }
            }
        }
        if let Some(glossary) = &self.glossary {
            for part in &glossary.doc_parts {
                visit_body_paragraphs(&part.body.content, false, &mut preflight);
            }
        }
        if let Some(error) = field_error {
            return Err(error);
        }
        // Header/footer and endnote parts are source-owned OPC bytes here,
        // not recursive cached models. Their independent parsers and writers
        // admit owners when those parts are projected or edited.
        Ok(())
    }

    /// Clone all package and typed state while discarding derived layout caches.
    pub fn clone_for_staging(&self) -> Self {
        Self {
            package: self.package.clone(),
            document: self.document.clone(),
            root_namespace_declarations: self.root_namespace_declarations.clone(),
            body_namespace_declarations: self.body_namespace_declarations.clone(),
            body_namespace_bindings: self.body_namespace_bindings.clone(),
            plain_local_edit_blocked: self.plain_local_edit_blocked,
            local_cell_source: self.local_cell_source.clone(),
            parsed: self.parsed.clone(),
            main_body_scans: Arc::clone(&self.main_body_scans),
            main_part_blocks: self.main_part_blocks.clone(),
            unsavable_body_edit_cache: self
                .unsavable_body_edit_cache
                .get()
                .cloned()
                .map(OnceLock::from)
                .unwrap_or_default(),
            styles: Arc::clone(&self.styles),
            styles_part_cache: self.styles_part_cache.clone(),
            numbering: self.numbering.clone(),
            published_abstract_by_live: self.published_abstract_by_live.clone(),
            published_instance_by_live: self.published_instance_by_live.clone(),
            core_properties: self.core_properties.clone(),
            application_properties: self.application_properties.clone(),
            custom_properties: self.custom_properties.clone(),
            core_properties_part_name: self.core_properties_part_name.clone(),
            application_properties_part_name: self.application_properties_part_name.clone(),
            custom_properties_part_name: self.custom_properties_part_name.clone(),
            custom_properties_owned: self.custom_properties_owned,
            doc_part_name: self.doc_part_name.clone(),
            styles_part_name: self.styles_part_name.clone(),
            numbering_part_name: self.numbering_part_name.clone(),
            settings: self.settings.clone(),
            settings_part_name: self.settings_part_name.clone(),
            settings_owned: self.settings_owned,
            web_settings: self.web_settings.clone(),
            web_settings_part_name: self.web_settings_part_name.clone(),
            web_settings_owned: self.web_settings_owned,
            theme: self.theme.clone(),
            theme_part_name: self.theme_part_name.clone(),
            theme_dirty: self.theme_dirty,
            font_table: self.font_table.clone(),
            font_table_part_name: self.font_table_part_name.clone(),
            font_table_dirty: self.font_table_dirty,
            identifiers: self.identifiers.clone(),
            footnotes: self.footnotes.clone(),
            footnotes_part_name: self.footnotes_part_name.clone(),
            footnotes_root_namespace_declarations: self
                .footnotes_root_namespace_declarations
                .clone(),
            footnotes_dirty: self.footnotes_dirty,
            comments: self.comments.clone(),
            comments_part_name: self.comments_part_name.clone(),
            comments_dirty: self.comments_dirty,
            comments_extended: self.comments_extended.clone(),
            comments_extended_part_name: self.comments_extended_part_name.clone(),
            comments_owned: self.comments_owned,
            comments_extended_owned: self.comments_extended_owned,
            embedded_invalidated_signatures: self.embedded_invalidated_signatures.clone(),
            package_signatures_invalidated: self.package_signatures_invalidated,
            source_path: self.source_path.clone(),
            glossary: self.glossary.clone(),
            glossary_part_name: self.glossary_part_name.clone(),
            glossary_dirty: self.glossary_dirty,
            layout_cache: Mutex::new(None),
            normal_layout_engine: Mutex::new(None),
            deterministic_layout_cache: Mutex::new(None),
            bundled_fallback_layout_engine: Mutex::new(None),
        }
    }

    /// Independent cold reference state for debug agreement oracles only.
    #[cfg(debug_assertions)]
    fn clone_for_debug_reference(&self) -> Self {
        let mut copy = self.clone_for_staging();
        copy.parsed = None;
        copy.main_part_blocks = None;
        copy.main_body_scans = Arc::new(Mutex::new(MainBodyScans::default()));
        copy
    }

    fn canonicalize_typed_identifiers(
        &mut self,
        template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        self.canonicalize_bookmark_ids(template)?;
        Ok(())
    }

    fn canonicalize_bookmark_ids(
        &mut self,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        let mut semantic = Vec::new();
        visit_body_paragraphs(&self.document.body.content, false, &mut |paragraph| {
            for marker in &paragraph.bookmark_markers {
                if marker.is_start()
                    && let Some(id) = marker.id()
                    && self.identifiers.authored_bookmark_ids.contains(&id)
                    && !semantic.contains(&id)
                {
                    semantic.push(id);
                }
            }
        });
        let mut occupied = self.identifiers.preserved_bookmark_ids.clone();
        let mut remap = HashMap::new();
        for old in semantic {
            remap.insert(old, reserve_lowest_i32(&mut occupied, 0, "bookmark")?);
        }
        if !remap.is_empty() {
            let mut field_owned = false;
            visit_body_paragraphs(&self.document.body.content, false, &mut |paragraph| {
                if paragraph.bookmark_markers.iter().any(|marker| {
                    marker
                        .id()
                        .is_some_and(|id| remap.get(&id).is_some_and(|new| *new != id))
                }) && paragraph.runs().iter().any(|run| {
                    run.content.iter().any(|content| {
                        matches!(content, RunContent::Field(field) if field.legacy_form.is_some())
                    })
                }) {
                    field_owned = true;
                }
            });
            if field_owned {
                // An external fragment can make a native form bookmark authored.
                // Its start is inside a retained field source, so patch both
                // physical markers together through the existing whole path.
                let remap = remap
                    .iter()
                    .map(|(old, new)| (old.to_string(), new.to_string()))
                    .collect();
                if let Some(template) = template.as_deref_mut() {
                    let source = template.document_source(&self.document)?;
                    let source =
                        crate::field::patch_bookmark_source(template.operation, source, &remap)?;
                    self.document = crate::document::DocumentModel::unproved(
                        template
                            .sources
                            .read_document(template.operation, Arc::new(source))?,
                    );
                } else {
                    let updated =
                        crate::field::patch_bookmark_ids(&self.document.to_xml()?, &remap)?;
                    self.document =
                        crate::document::DocumentModel::unproved(CT_Document::from_xml(&updated)?);
                }
            } else {
                visit_body_paragraphs_mut(&mut self.document.body.content, &mut |paragraph| {
                    let _ = paragraph.remap_authored_bookmark_ids(&remap);
                });
            }
            for old in remap.keys() {
                self.identifiers.authored_bookmark_ids.remove(old);
            }
            self.identifiers
                .authored_bookmark_ids
                .extend(remap.values());
        }
        self.canonicalize_toc_bookmark_ids(template)?;
        Ok(())
    }

    fn canonicalize_toc_bookmark_ids(
        &mut self,
        template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        let mut semantic = Vec::new();
        visit_body_paragraphs(&self.document.body.content, false, &mut |paragraph| {
            for marker in &paragraph.bookmark_markers {
                if marker.is_start()
                    && let Some(id) = marker.id()
                    && self.identifiers.authored_toc_bookmark_ids.contains(&id)
                    && !semantic.contains(&id)
                {
                    semantic.push(id);
                }
            }
        });
        if semantic.is_empty() {
            return Ok(());
        }
        let mut occupied = self.identifiers.bookmark_ids.clone();
        for id in &self.identifiers.authored_toc_bookmark_ids {
            occupied.remove(id);
        }
        let mut next = occupied.iter().copied().max().unwrap_or(0).checked_add(1);
        let mut remap = BTreeMap::new();
        for old in semantic {
            let new = next.ok_or_else(|| {
                Error::Other("table of contents exhausted the bookmark ID range".to_owned())
            })?;
            occupied.insert(new);
            next = new.checked_add(1);
            if old != new {
                remap.insert(old.to_string(), new.to_string());
            }
        }
        if remap.is_empty() {
            return Ok(());
        }
        if let Some(template) = template {
            let source = template.document_source(&self.document)?;
            let source = crate::field::patch_bookmark_source(template.operation, source, &remap)?;
            self.document = crate::document::DocumentModel::unproved(
                template
                    .sources
                    .read_document(template.operation, Arc::new(source))?,
            );
        } else {
            let xml = self.document.to_xml()?;
            let updated = crate::field::patch_bookmark_ids(&xml, &remap)?;
            self.document =
                crate::document::DocumentModel::unproved(CT_Document::from_xml(&updated)?);
        }
        for (old, new) in remap {
            let old = old.parse::<i32>().expect("bookmark remap key is numeric");
            let new = new.parse::<i32>().expect("bookmark remap value is numeric");
            self.identifiers.bookmark_ids.remove(&old);
            self.identifiers.bookmark_ids.insert(new);
            self.identifiers.preserved_bookmark_ids.remove(&old);
            self.identifiers.preserved_bookmark_ids.insert(new);
            self.identifiers.authored_toc_bookmark_ids.remove(&old);
            self.identifiers.authored_toc_bookmark_ids.insert(new);
        }
        Ok(())
    }

    fn canonicalize_numbering_ids(&mut self) -> Result<(HashMap<u32, u32>, HashMap<u32, u32>)> {
        let Some(numbering) = self.numbering.as_ref() else {
            return Ok((HashMap::new(), HashMap::new()));
        };
        let mut semantic_nums = Vec::new();
        visit_body_paragraphs(&self.document.body.content, false, &mut |paragraph| {
            if let Some(id) = paragraph
                .properties
                .as_ref()
                .and_then(|properties| properties.num_id)
                && !self
                    .identifiers
                    .preserved_numbering_instance_ids
                    .contains(&id)
                && self.identifiers.numbering_instance_ids.contains(&id)
                && !semantic_nums.contains(&id)
            {
                semantic_nums.push(id);
            }
        });
        for instance in &numbering.nums {
            if !self
                .identifiers
                .preserved_numbering_instance_ids
                .contains(&instance.num_id)
                && self
                    .identifiers
                    .numbering_instance_ids
                    .contains(&instance.num_id)
                && !semantic_nums.contains(&instance.num_id)
            {
                semantic_nums.push(instance.num_id);
            }
        }
        let abstract_by_num = numbering
            .nums
            .iter()
            .map(|instance| (instance.num_id, instance.abstract_num_id))
            .collect::<HashMap<_, _>>();
        let mut occupied_nums = self.identifiers.preserved_numbering_instance_ids.clone();
        let mut occupied_abstract = self.identifiers.preserved_abstract_numbering_ids.clone();
        let referenced_abstract = abstract_by_num.values().copied().collect::<HashSet<_>>();
        occupied_abstract.extend(
            numbering
                .abstract_nums
                .iter()
                .map(|definition| definition.abstract_num_id)
                .filter(|id| !referenced_abstract.contains(id)),
        );
        let mut num_remap = HashMap::new();
        let mut abstract_remap = HashMap::new();
        for old_num in semantic_nums {
            num_remap.insert(
                old_num,
                reserve_u32(&mut occupied_nums, 1, "numbering instance")?,
            );
            if let Some(old_abstract) = abstract_by_num.get(&old_num)
                && !self
                    .identifiers
                    .preserved_abstract_numbering_ids
                    .contains(old_abstract)
                && !abstract_remap.contains_key(old_abstract)
            {
                abstract_remap.insert(
                    *old_abstract,
                    reserve_u32(&mut occupied_abstract, 0, "abstract numbering")?,
                );
            }
        }
        visit_body_paragraphs_mut(&mut self.document.body.content, &mut |paragraph| {
            if let Some(id) = paragraph
                .properties
                .as_mut()
                .and_then(|properties| properties.num_id.as_mut())
                && let Some(updated) = num_remap.get(id)
            {
                *id = *updated;
            }
        });
        let numbering = self.numbering.as_mut().expect("checked above");
        for definition in &mut numbering.abstract_nums {
            if let Some(updated) = abstract_remap.get(&definition.abstract_num_id) {
                definition.abstract_num_id = *updated;
            }
        }
        for instance in &mut numbering.nums {
            if let Some(updated) = num_remap.get(&instance.num_id) {
                instance.num_id = *updated;
            }
            if let Some(updated) = abstract_remap.get(&instance.abstract_num_id) {
                instance.abstract_num_id = *updated;
            }
        }
        if self.identifiers.preserved_abstract_numbering_ids.is_empty()
            && self.identifiers.preserved_numbering_instance_ids.is_empty()
        {
            numbering
                .abstract_nums
                .sort_by_key(|definition| definition.abstract_num_id);
            numbering.nums.sort_by_key(|instance| instance.num_id);
        }
        Ok((abstract_remap, num_remap))
    }

    fn opaque_relationship_ids_from_serialized_story(
        &self,
        modeled_relationships: &[String],
    ) -> Result<Vec<String>> {
        // Without modeled candidates there is nothing to mask. An unchanged
        // typed story can inventory its exact retained source, including opaque
        // property-root attributes that canonical serialization cannot replay.
        // Dirty stories and modeled relationships still need the checked mask.
        // The serialization shortcut still requires the opened model. Namespace
        // owners follow the opened part bytes after that model changes.
        let source_kept = self.kept_main_part();
        let model_kept = source_kept
            .clone()
            .filter(|_| self.opened_main_still_serializes());
        if modeled_relationships.is_empty()
            && let Some(source) = self.package.get_part(&self.doc_part_name)
            && {
                let matches = if model_kept.is_some() {
                    true
                } else if let Some(parsed) = source_kept.as_deref().and_then(Parsed::main_document)
                {
                    // The live model may differ, but these retained source
                    // bytes still have the exact parse from their opening.
                    parsed == &*self.document
                } else if let Some(proved) = self
                    .main_part_blocks
                    .as_ref()
                    .and_then(|blocks| blocks.proved_document_matches(source, &self.document))
                {
                    #[cfg(debug_assertions)]
                    {
                        let fresh = CT_Document::from_xml(source).ok();
                        assert_eq!(
                            fresh.as_ref() == Some(&*self.document),
                            proved,
                            "the proved main part disagrees with a fresh parse"
                        );
                    }
                    proved
                } else {
                    CT_Document::from_xml(source).ok().as_ref() == Some(&*self.document)
                };
                #[cfg(debug_assertions)]
                if let Some(kept) = model_kept.as_deref() {
                    {
                        assert_kept_main_document_without_initializing(
                            Some(kept),
                            Some(source),
                            &self.document,
                        );
                    }
                }
                matches
            }
        {
            return relationship_ids_by_blocks(source, self.main_part_blocks.as_ref(), &[]);
        }
        let scanned;
        let nested_namespace_owners = match source_kept
            .as_deref()
            .and_then(Parsed::nested_namespace_owners)
        {
            Some(owners) => {
                #[cfg(debug_assertions)]
                {
                    let source = self.package.get_part(&self.doc_part_name);
                    {
                        assert_eq!(
                            format!("{owners:?}"),
                            format!(
                                "{:?}",
                                source
                                    .map(nested_modeled_namespace_owners)
                                    .transpose()
                                    .ok()
                                    .flatten()
                                    .unwrap_or_default()
                            ),
                            "the kept nested namespace owners differ from a scan"
                        );
                    }
                }
                owners
            }
            None => {
                let nested_source = self.package.get_part(&self.doc_part_name);
                let proved_owners = nested_source.and_then(|xml| {
                    self.main_part_blocks
                        .as_ref()
                        .and_then(|blocks| blocks.retained_nested_owners(xml))
                });
                if let Some(owners) = proved_owners {
                    #[cfg(debug_assertions)]
                    {
                        let fresh = nested_source
                            .map(nested_modeled_namespace_owners)
                            .transpose()
                            .ok()
                            .flatten()
                            .unwrap_or_default();
                        assert_eq!(
                            format!("{owners:?}"),
                            format!("{fresh:?}"),
                            "the proved nested owners differ from a scan"
                        );
                    }
                    scanned = owners;
                    &scanned
                } else {
                    scanned = nested_source
                        .map(nested_modeled_namespace_owners)
                        .transpose()?
                        .unwrap_or_default();
                    &scanned
                }
            }
        };
        let serialize = |document: &CT_Document,
                         blocked: Result<(Vec<u8>, Option<MainPartBlocks>)>,
                         query_source: Option<&MainPartBlocks>|
         -> Result<(Vec<u8>, Option<MainPartBlocks>)> {
            match blocked {
                Ok((xml, record)) => {
                    let spans = record.as_ref().and_then(|record| {
                        query_source
                            .and_then(|source| record.masked_query_namespace_spans(&xml, source))
                            .or_else(|| record.proved_namespace_spans(&xml))
                    });
                    let replayed = replay_nested_namespace_declarations_with_spans(
                        &xml,
                        nested_namespace_owners,
                        spans.as_deref(),
                    )?;
                    Ok(relationship_xml_from_replayed(replayed, Ok((xml, record))))
                }
                Err(_) => {
                    // A failed optional writer proof does not replace the
                    // authoritative full writer or its original error.
                    let xml = document.to_xml()?;
                    let spans = self
                        .main_part_blocks
                        .as_ref()
                        .and_then(|blocks| blocks.proved_namespace_spans(&xml));
                    Ok((
                        replay_nested_namespace_declarations_with_spans(
                            &xml,
                            nested_namespace_owners,
                            spans.as_deref(),
                        )?,
                        None,
                    ))
                }
            }
        };
        let owners_empty = nested_namespace_owners.is_empty();
        if modeled_relationships.is_empty() {
            if owners_empty {
                let (current_xml, current_blocks) = self.canonical_source()?;
                return relationship_ids_by_blocks(&current_xml, current_blocks.as_ref(), &[]);
            }
            let (current_xml, current_blocks) =
                serialize(&self.document, self.canonical_source(), None)?;
            return relationship_ids_by_blocks(&current_xml, current_blocks.as_ref(), &[]);
        }

        let (current_xml, current_blocks) = if owners_empty {
            self.canonical_source()?
        } else {
            serialize(&self.document, self.canonical_source(), None)?
        };
        let current_relationships =
            relationship_ids_by_blocks(&current_xml, current_blocks.as_ref(), &[])?;
        let mut masked = self.document.clone();
        let mut masked_values = HashSet::new();
        let mut remap = HashMap::new();
        for (index, relationship_id) in modeled_relationships.iter().enumerate() {
            let mut ordinal = index;
            let sentinel = loop {
                let candidate = format!("rdocx-modeled-relationship-{ordinal}");
                if !current_relationships.contains(&candidate)
                    && masked_values.insert(candidate.clone())
                {
                    break candidate;
                }
                ordinal = ordinal
                    .checked_add(modeled_relationships.len().max(1))
                    .ok_or_else(|| {
                        Error::Other("modeled relationship mask range is exhausted".to_owned())
                    })?;
            };
            remap.insert(relationship_id.clone(), sentinel);
        }

        visit_authored_drawings_mut(&mut masked.body.content, &mut |drawing| {
            if let Some(mut drawing) = drawing
                .inline
                .as_mut()
                .map(DrawingMut::Inline)
                .or_else(|| drawing.anchor.as_mut().map(DrawingMut::Anchor))
            {
                drawing.remap_relationship(&remap);
            }
        });
        visit_body_paragraphs_mut(&mut masked.body.content, &mut |paragraph| {
            for hyperlink in &mut paragraph.hyperlinks {
                if let Some(id) = &mut hyperlink.rel_id
                    && let Some(updated) = remap.get(id)
                {
                    id.clone_from(updated);
                }
            }
            if let Some(section) = paragraph
                .properties
                .as_mut()
                .and_then(|properties| properties.sect_pr.as_mut())
            {
                for reference in section
                    .header_refs
                    .iter_mut()
                    .chain(&mut section.footer_refs)
                {
                    if let Some(updated) = remap.get(&reference.rel_id) {
                        reference.rel_id.clone_from(updated);
                    }
                }
            }
        });
        if let Some(section) = &mut masked.body.sect_pr {
            for reference in section
                .header_refs
                .iter_mut()
                .chain(&mut section.footer_refs)
            {
                if let Some(updated) = remap.get(&reference.rel_id) {
                    reference.rel_id.clone_from(updated);
                }
            }
        }

        let (masked_xml, masked_blocks) = if owners_empty {
            Self::canonical_source_from(&masked, current_blocks.as_ref())?
        } else {
            serialize(
                &masked,
                Self::canonical_source_from(&masked, current_blocks.as_ref()),
                current_blocks.as_ref(),
            )?
        };
        let masked_relationships = masked_blocks.as_ref().and_then(|masked| {
            current_blocks
                .as_ref()
                .and_then(|current| masked.masked_query_relationship_ids(&masked_xml, current))
        });
        let masked_relationships = match masked_relationships {
            Some(ids) => ids,
            None => relationship_ids_by_blocks(&masked_xml, masked_blocks.as_ref(), &[])?,
        };
        Ok(masked_relationships
            .into_iter()
            .filter(|id| !masked_values.contains(id) && current_relationships.contains(id))
            .collect())
    }

    /// Keep a main-part relationship id from staging renumbering until the
    /// main story references it or the id leaves the relationship registry.
    pub(crate) fn register_pending_relationship(&mut self, owner: &str, id: &str) {
        if owner == self.doc_part_name {
            self.identifiers
                .pending_relationship_ids
                .entry(relationship_owner_identity(owner))
                .or_default()
                .insert(id.to_owned());
        }
    }

    fn canonicalize_drawing_ids(
        &mut self,
        template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        let Some(kept) = self.unchanged_main_part() else {
            return self.canonicalize_all_drawing_ids(template);
        };
        let unchanged = kept
            .unchanged_main_part
            .get()
            .and_then(Option::as_ref)
            .expect("checked unchanged main part");
        #[cfg(debug_assertions)]
        let mut full = self.clone_for_debug_reference();
        let result = self.canonicalize_beside_unchanged_main_part(unchanged, template);
        #[cfg(debug_assertions)]
        {
            let _proof_off = CanonicalBlockProofOff::enter();
            let full_result = full.canonicalize_all_drawing_ids(None);
            assert_same_canonicalization(self, &result, &full, &full_result);
        }
        result
    }

    fn opened_main_still_serializes(&self) -> bool {
        let Some(kept) = self.kept_main_part() else {
            return false;
        };
        let Some(expected) = kept.serialization_proof.as_deref() else {
            return false;
        };
        if kept.root_namespace_declarations != self.root_namespace_declarations
            || kept.body_namespace_declarations != self.body_namespace_declarations
            || kept.body_namespace_bindings != self.body_namespace_bindings
            || kept.main_document() != Some(&*self.document)
        {
            return false;
        }
        self.serialized_main_part_matches(expected).unwrap_or(false)
    }

    fn unchanged_main_part(&self) -> Option<Arc<Parsed>> {
        if !self.opened_main_still_serializes() {
            return None;
        }
        let kept = self.kept_main_part()?;
        let owner_identity = relationship_owner_identity(&self.doc_part_name);
        if self
            .identifiers
            .pending_relationship_ids
            .get(&owner_identity)
            .is_some_and(|ids| !ids.is_empty())
        {
            return None;
        }
        if self
            .identifiers
            .authored_nested_story_relationship_ids
            .get(&owner_identity)
            .is_some_and(|ids| !ids.is_empty())
        {
            return None;
        }
        let preserved = self
            .identifiers
            .preserved_relationship_ids
            .get(&owner_identity)
            .cloned()
            .unwrap_or_default();
        if self
            .package
            .get_part_rels(&self.doc_part_name)
            .is_some_and(|relationships| {
                !relationships
                    .items
                    .iter()
                    .all(|relationship| preserved.contains(&relationship.id))
            })
        {
            return None;
        }
        if let Some(section) = &self.document.body.sect_pr
            && !section_references_were_read(section, &preserved)
        {
            return None;
        }
        let mut paragraph_sections_ok = true;
        visit_body_paragraphs(&self.document.body.content, false, &mut |paragraph| {
            if let Some(section) = paragraph
                .properties
                .as_ref()
                .and_then(|properties| properties.sect_pr.as_ref())
                && !section_references_were_read(section, &preserved)
            {
                paragraph_sections_ok = false;
            }
        });
        if !paragraph_sections_ok {
            return None;
        }
        let mut authored_drawing = false;
        visit_authored_drawings(&self.document.body.content, &mut |_| {
            authored_drawing = true;
        });
        if authored_drawing {
            return None;
        }
        let unchanged = kept
            .unchanged_main_part
            .get_or_init(|| self.unchanged_main_part_checks().ok())
            .as_ref()?;
        if unchanged.body_namespace_bindings != self.body_namespace_bindings
            || self.serialized_main_part_matches(&unchanged.xml).ok() != Some(true)
        {
            return None;
        }
        Some(kept)
    }

    fn unchanged_main_part_checks(&self) -> Result<UnchangedMainPart> {
        let mut modeled_story_relationships = Vec::new();
        collect_relationship_ids(
            &self.document.body.content,
            &mut modeled_story_relationships,
        );
        if let Some(section) = &self.document.body.sect_pr {
            for reference in section.header_refs.iter().chain(&section.footer_refs) {
                if !modeled_story_relationships.contains(&reference.rel_id) {
                    modeled_story_relationships.push(reference.rel_id.clone());
                }
            }
        }
        visit_body_paragraphs(&self.document.body.content, false, &mut |paragraph| {
            if let Some(section) = paragraph
                .properties
                .as_ref()
                .and_then(|properties| properties.sect_pr.as_ref())
            {
                for reference in section.header_refs.iter().chain(&section.footer_refs) {
                    if !modeled_story_relationships.contains(&reference.rel_id) {
                        modeled_story_relationships.push(reference.rel_id.clone());
                    }
                }
            }
        });
        self.opaque_relationship_ids_from_serialized_story(&modeled_story_relationships)?;
        let (xml, blocks) = self.canonical_source()?;
        relationship_ids_by_blocks(&xml, blocks.as_ref(), &self.body_namespace_bindings)?;
        authored_drawing_slots_by_blocks(&xml, blocks.as_ref(), &[], &HashSet::new())?;
        let drawing_ids = drawing_ids_by_blocks(&xml, blocks.as_ref());
        relationship_ids_by_blocks(&xml, blocks.as_ref(), &[])?;
        Ok(UnchangedMainPart {
            xml,
            body_namespace_bindings: self.body_namespace_bindings.clone(),
            drawing_ids,
        })
    }

    fn canonicalize_beside_unchanged_main_part(
        &mut self,
        unchanged: &UnchangedMainPart,
        template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        let owner_identity = relationship_owner_identity(&self.doc_part_name);
        self.identifiers
            .authored_nested_story_relationship_ids
            .remove(&owner_identity);
        let related_parts = self.related_story_parts();
        let mut occupied_drawings = self.identifiers.preserved_drawing_ids.clone();
        if let Some(live_ids) = &unchanged.drawing_ids {
            occupied_drawings.extend(live_ids);
        }
        for part in &related_parts {
            if !self.identifiers.authored_story_parts.contains(part)
                && let Some(xml) = self.package.get_part(part)
                && let Ok(live_ids) = drawing_ids_in_xml(xml)
            {
                occupied_drawings.extend(live_ids);
            }
        }
        self.identifiers
            .authored_nested_story_drawing_ids
            .remove(&owner_identity);
        self.canonicalize_related_story_relationships(
            &related_parts,
            &mut occupied_drawings,
            template,
        )?;
        self.identifiers.drawing_ids = occupied_drawings;
        self.canonicalize_image_parts(true)
    }

    fn canonicalize_all_drawing_ids(
        &mut self,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        let owner = self.doc_part_name.clone();
        let owner_identity = relationship_owner_identity(&owner);
        // Story picture insertion keeps current producer XML. Patch its
        // canonical relationship and drawing IDs into that retained source.
        let retained_main_xml = self
            .package
            .get_part_shared(&owner)
            .filter(|xml| {
                let authored = self
                    .identifiers
                    .authored_nested_story_relationship_ids
                    .get(&owner_identity)
                    .is_some_and(|ids| !ids.is_empty());
                if !authored {
                    return false;
                }
                let kept = self.kept_main_part();
                let matches = if let Some(parsed) = self
                    .main_part_blocks
                    .as_ref()
                    .and_then(|blocks| blocks.proved_document_matches(xml, &self.document))
                {
                    parsed
                } else if let Some(parsed) = kept.as_deref().and_then(Parsed::main_document) {
                    parsed == &*self.document
                } else {
                    CT_Document::from_xml(xml).ok().as_ref() == Some(&*self.document)
                };
                matches
                    && self
                        .serialized_main_part()
                        .ok()
                        .as_ref()
                        .map(|(xml, _)| xml.as_slice())
                        != Some(xml.as_slice())
            })
            .map(Arc::clone);
        let retained_main_source =
            if let (Some(bytes), Some(writer)) = (&retained_main_xml, template.as_deref_mut()) {
                if let Some((stored, source)) = &writer.sources.published_main
                    && Arc::ptr_eq(stored, bytes)
                {
                    Some(Arc::clone(source))
                } else {
                    writer
                        .lineage
                        .original_part_source(writer.operation, &owner, bytes)?
                        .map(Arc::new)
                }
            } else {
                None
            };
        let preserved_relationships = self
            .identifiers
            .preserved_relationship_ids
            .get(&owner_identity)
            .cloned()
            .unwrap_or_default();
        let authored_bundle_relationships = self
            .identifiers
            .authored_bundle_relationship_ids
            .get(&owner_identity)
            .cloned()
            .unwrap_or_default();
        let mut modeled_story_relationships = Vec::new();
        collect_relationship_ids(
            &self.document.body.content,
            &mut modeled_story_relationships,
        );
        if let Some(section) = &self.document.body.sect_pr {
            for reference in section.header_refs.iter().chain(&section.footer_refs) {
                if !modeled_story_relationships.contains(&reference.rel_id) {
                    modeled_story_relationships.push(reference.rel_id.clone());
                }
            }
        }
        let nested_relationships = self
            .identifiers
            .authored_nested_story_relationship_ids
            .get(&owner_identity)
            .cloned()
            .unwrap_or_default();
        let mut opaque_relationships =
            self.opaque_relationship_ids_from_serialized_story(&modeled_story_relationships)?;
        opaque_relationships.retain(|id| !nested_relationships.contains(id));
        let mut story_relationships = self.main_story_relationship_ids();
        let relationships = self
            .package
            .get_part_rels(&owner)
            .map(|relationships| relationships.items.clone())
            .unwrap_or_default();
        // Pending XML or a caller still names these ids. Renumbering them
        // would point that name at another relationship. A main-story
        // reference, or disappearance from the registry, ends the hold.
        // Owner-part loss alone does not.
        if let Some(pending) = self
            .identifiers
            .pending_relationship_ids
            .get_mut(&owner_identity)
        {
            pending.retain(|id| {
                !modeled_story_relationships.contains(id)
                    && !opaque_relationships.contains(id)
                    && relationships
                        .iter()
                        .any(|relationship| relationship.id == *id)
            });
            opaque_relationships.extend(pending.iter().cloned());
        }
        story_relationships.retain(|id| {
            !preserved_relationships.contains(id)
                && relationships
                    .iter()
                    .any(|relationship| relationship.id == *id)
        });
        let mut bundle_relationships = relationships
            .iter()
            .filter_map(|relationship| {
                (!preserved_relationships.contains(&relationship.id)
                    && authored_bundle_relationships.contains(&relationship.id)
                    && relationship_is_internal(relationship))
                .then(|| {
                    bundle_relationship_order(&relationship.rel_type).map(|order| {
                        (
                            order,
                            relationship.target.as_str(),
                            relationship.id.as_str(),
                            bundle_relationship_precedes_story(&relationship.rel_type),
                        )
                    })
                })
                .flatten()
            })
            .collect::<Vec<_>>();
        bundle_relationships.sort_unstable();
        let mut semantic_relationships = bundle_relationships
            .iter()
            .filter(|(_, _, _, precedes_story)| *precedes_story)
            .map(|(_, _, id, _)| (*id).to_owned())
            .collect::<Vec<_>>();
        semantic_relationships.extend(story_relationships);
        if let Some(section) = &self.document.body.sect_pr {
            for reference in section.header_refs.iter().chain(&section.footer_refs) {
                if !preserved_relationships.contains(&reference.rel_id)
                    && !semantic_relationships.contains(&reference.rel_id)
                {
                    semantic_relationships.push(reference.rel_id.clone());
                }
            }
        }
        let mut unreferenced_relationships = relationships
            .iter()
            .filter(|relationship| {
                !preserved_relationships.contains(&relationship.id)
                    && !semantic_relationships.contains(&relationship.id)
                    && !authored_bundle_relationships.contains(&relationship.id)
                    && !opaque_relationships.contains(&relationship.id)
            })
            .map(|relationship| {
                let internal = relationship_is_internal(relationship);
                let target = if internal {
                    part_name_identity(&OpcPackage::resolve_rel_target(
                        &owner,
                        &relationship.target,
                    ))
                } else {
                    relationship.target.clone()
                };
                (
                    !internal,
                    relationship.rel_type.as_str(),
                    target,
                    relationship.target_mode.as_deref().unwrap_or(""),
                    relationship.id.as_str(),
                )
            })
            .collect::<Vec<_>>();
        unreferenced_relationships.sort_unstable();
        semantic_relationships.extend(
            unreferenced_relationships
                .into_iter()
                .map(|(_, _, _, _, id)| id.to_owned()),
        );
        semantic_relationships.extend(
            bundle_relationships
                .into_iter()
                .filter(|(_, _, _, precedes_story)| !precedes_story)
                .map(|(_, _, id, _)| id.to_owned()),
        );

        let mut occupied_relationships = preserved_relationships.clone();
        occupied_relationships.extend(opaque_relationships);
        occupied_relationships.extend(
            relationships
                .iter()
                .filter(|relationship| !semantic_relationships.contains(&relationship.id))
                .map(|relationship| relationship.id.clone()),
        );
        let mut relationship_cursor = if semantic_relationships.is_empty() {
            0
        } else {
            preserved_relationships
                .iter()
                .filter_map(|id| id.strip_prefix("rId")?.parse::<u32>().ok())
                .max()
                .map_or(Ok(0), |maximum| {
                    maximum.checked_add(1).ok_or_else(|| {
                        Error::Other("relationship id range is exhausted".to_owned())
                    })
                })?
        };
        let mut relationship_remap = HashMap::new();
        for old in &semantic_relationships {
            let updated = reserve_relationship_from_cursor(
                &mut occupied_relationships,
                &mut relationship_cursor,
            )?;
            relationship_remap.insert(old.clone(), updated);
        }
        if let Some(relationships) = self.package.get_part_rels_mut(&owner) {
            for relationship in &mut relationships.items {
                if let Some(updated) = relationship_remap.get(&relationship.id) {
                    relationship.id.clone_from(updated);
                }
            }
            let mut authored = Vec::new();
            relationships.items.retain(|relationship| {
                if preserved_relationships.contains(&relationship.id) {
                    true
                } else {
                    authored.push(relationship.clone());
                    false
                }
            });
            authored.sort_by_key(|relationship| {
                relationship
                    .id
                    .strip_prefix("rId")
                    .and_then(|value| value.parse::<u32>().ok())
                    .unwrap_or(u32::MAX)
            });
            relationships.items.extend(authored);
        }
        if !relationship_remap.is_empty() {
            if let Some(occupied) = self.identifiers.relationship_ids.get_mut(&owner_identity) {
                for old in relationship_remap.keys() {
                    occupied.remove(old);
                }
                occupied.extend(relationship_remap.values().cloned());
            }
            if let Some(authored) = self
                .identifiers
                .authored_story_relationship_ids
                .get_mut(&owner_identity)
            {
                let remapped = authored
                    .iter()
                    .map(|id| relationship_remap.get(id).unwrap_or(id).clone())
                    .collect();
                *authored = remapped;
            }
            if let Some(authored) = self
                .identifiers
                .authored_bundle_relationship_ids
                .get_mut(&owner_identity)
            {
                let remapped = authored
                    .iter()
                    .map(|id| relationship_remap.get(id).unwrap_or(id).clone())
                    .collect();
                *authored = remapped;
            }
            if let Some(provisional) = self
                .identifiers
                .provisional_relationship_ids
                .get_mut(&owner_identity)
            {
                provisional.retain(|id| !relationship_remap.contains_key(id));
                if provisional.is_empty() {
                    self.identifiers
                        .provisional_relationship_ids
                        .remove(&owner_identity);
                }
            }
        }
        let registered_nested_drawing_ids = self
            .identifiers
            .authored_nested_story_drawing_ids
            .get(&owner_identity)
            .cloned()
            .unwrap_or_default();
        let mut modeled_drawing_ids = Vec::new();
        visit_authored_drawings(&self.document.body.content, &mut |drawing| {
            if let Some(inline) = &drawing.inline
                && !registered_nested_drawing_ids.contains(&inline.doc_pr_id)
            {
                modeled_drawing_ids.push(inline.doc_pr_id);
            }
            if let Some(anchor) = &drawing.anchor
                && !registered_nested_drawing_ids.contains(&anchor.doc_pr_id)
            {
                modeled_drawing_ids.push(anchor.doc_pr_id);
            }
        });
        let has_live_nested_relationships = self.remap_authored_nested_story_relationships(
            &owner_identity,
            &nested_relationships,
            &relationship_remap,
            template.as_deref_mut(),
        )?;
        if has_live_nested_relationships && let Some(writer) = template.as_deref_mut() {
            writer.sources.model_lineage_valid = false;
        }
        if !has_live_nested_relationships {
            visit_authored_drawings_mut(&mut self.document.body.content, &mut |drawing| {
                if let Some(mut drawing) = drawing
                    .inline
                    .as_mut()
                    .map(DrawingMut::Inline)
                    .or_else(|| drawing.anchor.as_mut().map(DrawingMut::Anchor))
                {
                    drawing.remap_relationship(&relationship_remap);
                }
            });
            visit_body_paragraphs_mut(&mut self.document.body.content, &mut |paragraph| {
                for hyperlink in &mut paragraph.hyperlinks {
                    if let Some(id) = &mut hyperlink.rel_id
                        && let Some(updated) = relationship_remap.get(id)
                    {
                        id.clone_from(updated);
                    }
                }
                if let Some(section) = paragraph
                    .properties
                    .as_mut()
                    .and_then(|properties| properties.sect_pr.as_mut())
                {
                    for reference in section
                        .header_refs
                        .iter_mut()
                        .chain(&mut section.footer_refs)
                    {
                        if let Some(updated) = relationship_remap.get(&reference.rel_id) {
                            reference.rel_id.clone_from(updated);
                        }
                    }
                }
            });
            if let Some(section) = &mut self.document.body.sect_pr {
                for reference in section
                    .header_refs
                    .iter_mut()
                    .chain(&mut section.footer_refs)
                {
                    if let Some(updated) = relationship_remap.get(&reference.rel_id) {
                        reference.rel_id.clone_from(updated);
                    }
                }
            }
        }
        let related_parts = self.related_story_parts();
        let live_nested_relationships = self
            .identifiers
            .authored_nested_story_relationship_ids
            .get(&owner_identity)
            .cloned()
            .unwrap_or_default();
        let nested_image_relationship_ids = self
            .package
            .get_part_rels(&owner)
            .into_iter()
            .flat_map(|relationships| &relationships.items)
            .filter(|relationship| {
                relationship.rel_type == rel_types::IMAGE
                    && live_nested_relationships.contains(&relationship.id)
            })
            .map(|relationship| relationship.id.clone())
            .collect::<HashSet<_>>();
        let (current_document_xml, blocks) =
            if modeled_drawing_ids.is_empty() && nested_image_relationship_ids.is_empty() {
                self.canonical_source()?
            } else {
                self.canonical_source_in(template.as_deref_mut())?
            };
        let authored_drawing_slots = authored_drawing_slots_by_blocks(
            &current_document_xml,
            blocks.as_ref(),
            &modeled_drawing_ids,
            &nested_image_relationship_ids,
        )?;
        let mut occupied_drawings = self.identifiers.preserved_drawing_ids.clone();
        if let Some(live_ids) = drawing_ids_by_blocks(&current_document_xml, blocks.as_ref()) {
            occupied_drawings.extend(live_ids);
        }
        for (_, _, id) in &authored_drawing_slots {
            occupied_drawings.remove(id);
        }
        for part in &related_parts {
            if !self.identifiers.authored_story_parts.contains(part)
                && let Some(xml) = self.package.get_part(part)
                && let Ok(live_ids) = drawing_ids_in_xml(xml)
            {
                occupied_drawings.extend(live_ids);
            }
        }
        let mut drawing_edits = Vec::with_capacity(authored_drawing_slots.len());
        let mut canonical_drawing_ids = Vec::with_capacity(authored_drawing_slots.len());
        let mut canonical_nested_drawing_ids = HashSet::new();
        for (slot, span, old_id) in authored_drawing_slots {
            let id = reserve_u32(&mut occupied_drawings, 1, "drawing")?;
            if matches!(slot, AuthoredMainDrawing::Nested) {
                canonical_nested_drawing_ids.insert(id);
            }
            canonical_drawing_ids.push((old_id, id));
            drawing_edits.push((span.start, span.end, id.to_string().into_bytes()));
        }
        let drawing_bytes_unchanged = drawing_edits.iter().all(|(start, end, value)| {
            current_document_xml.get(*start..*end) == Some(value.as_slice())
        });
        let drawing_noop = !drawing_edits.is_empty()
            && drawing_bytes_unchanged
            && offered_canonical_blocks(blocks.as_ref()).is_some_and(|record| {
                record.current_source_model_matches(&current_document_xml, &self.document)
            });
        if !drawing_edits.is_empty() && !drawing_noop {
            if let Some(writer) = template.as_deref_mut() {
                let source = writer.sources.source.take().ok_or_else(|| {
                    Error::Other("main drawing canonicalization lost its produced source".into())
                })?;
                let source =
                    apply_borrowed_template_source_edits(writer.operation, &source, drawing_edits)?;
                self.document = crate::document::DocumentModel::unproved(
                    writer
                        .sources
                        .read_document(writer.operation, Arc::new(source))?,
                );
                self.main_part_blocks = None;
                self.main_body_scans
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .serialized = None;
            } else {
                let mut updated = current_document_xml;
                drawing_edits.sort_by_key(|(start, _, _)| *start);
                for (start, end, replacement) in drawing_edits.into_iter().rev() {
                    updated.splice(start..end, replacement);
                }
                let updated = Arc::new(updated);
                if let Some((parsed, mut record)) = offered_canonical_blocks(blocks.as_ref())
                    .and_then(|blocks| {
                        self.source_blocks_in(blocks.part.as_deref()?, blocks.clone(), &updated)
                    })
                {
                    self.document = crate::document::DocumentModel::unproved(parsed);
                    if let Some(writer) = template.as_deref_mut() {
                        writer.sources.model_lineage_valid = false;
                    }
                    // This is a current writer memo, not a published package part.
                    record.part = None;
                    self.main_body_scans
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .serialized = Some(record);
                } else {
                    self.document =
                        crate::document::DocumentModel::unproved(CT_Document::from_xml(&updated)?);
                    if let Some(writer) = template.as_deref_mut() {
                        writer.sources.model_lineage_valid = false;
                    }
                }
            }
        }
        if let Some(retained) = retained_main_xml {
            if let Some(writer) = template.as_deref_mut() {
                let source = retained_main_source.ok_or_else(|| {
                    Error::Other("retained main publication has no actual source handoff".into())
                })?;
                let edits = relationship_remap_edits(source.as_bytes(), &relationship_remap, &[])?;
                let source =
                    apply_borrowed_template_source_edits(writer.operation, &source, edits)?;
                let slots = authored_drawing_slots_in_source_order(
                    source.as_bytes(),
                    &modeled_drawing_ids,
                    &nested_image_relationship_ids,
                )?;
                if slots.len() == canonical_drawing_ids.len()
                    && slots
                        .iter()
                        .zip(&canonical_drawing_ids)
                        .all(|((_, _, old), (expected, _))| old == expected)
                {
                    let edits = slots
                        .into_iter()
                        .zip(&canonical_drawing_ids)
                        .map(|((_, span, _), (_, id))| {
                            (span.start, span.end, id.to_string().into_bytes())
                        })
                        .collect();
                    let source = apply_template_source_edits(writer.operation, source, edits)?;
                    let retained = writer.operation.repeat_source(&source)?;
                    let bytes = writer.operation.finish_source(retained)?;
                    self.document = crate::document::DocumentModel::unproved(
                        writer
                            .sources
                            .read_document(writer.operation, Arc::new(source))?,
                    );
                    self.package.set_part(&owner, bytes);
                    let bytes = self
                        .package
                        .get_part_shared(&owner)
                        .expect("actual retained main publication");
                    writer.sources.published_main = Some((
                        Arc::clone(bytes),
                        Arc::clone(
                            writer
                                .sources
                                .reader_source
                                .as_ref()
                                .expect("actual retained main reader"),
                        ),
                    ));
                    self.main_part_blocks = None;
                }
            } else {
                if let Some(writer) = template.as_deref_mut() {
                    writer.sources.model_lineage_valid = false;
                }
                let fast = self.package_main_part_blocks().and_then(|record| {
                    record.canonical_retained(
                        &relationship_remap,
                        &modeled_drawing_ids,
                        &nested_image_relationship_ids,
                        &canonical_drawing_ids,
                        &mut self.document,
                    )
                });
                match fast {
                    Some(Some((xml, record))) => {
                        self.package.set_part(&owner, xml);
                        self.main_part_blocks = Some(record);
                    }
                    Some(None) => {}
                    None => {
                        if let Some((xml, document)) = canonical_retained_main_part(
                            &retained,
                            &relationship_remap,
                            &modeled_drawing_ids,
                            &nested_image_relationship_ids,
                            &canonical_drawing_ids,
                        )? {
                            self.document = crate::document::DocumentModel::unproved(document);
                            self.package.set_part(&owner, xml);
                            self.main_part_blocks = None;
                        }
                    }
                }
            }
        }
        if canonical_nested_drawing_ids.is_empty() {
            self.identifiers
                .authored_nested_story_drawing_ids
                .remove(&owner_identity);
        } else {
            self.identifiers
                .authored_nested_story_drawing_ids
                .insert(owner_identity.clone(), canonical_nested_drawing_ids);
        }
        self.canonicalize_related_story_relationships(
            &related_parts,
            &mut occupied_drawings,
            template,
        )?;
        self.identifiers.drawing_ids = occupied_drawings;
        self.canonicalize_image_parts(false)?;
        Ok(())
    }

    fn active_header_footer_parts(&self) -> Vec<String> {
        let mut parts = Vec::new();
        for (relationship_id, is_header) in self.header_footer_rel_ids() {
            if let Some(part) = self.header_footer_part_name(&relationship_id, is_header)
                && !parts.contains(&part)
            {
                parts.push(part);
            }
        }
        parts
    }

    fn remap_authored_nested_story_relationships(
        &mut self,
        physical_owner: &str,
        registered: &HashSet<String>,
        relationship_remap: &HashMap<String, String>,
        template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<bool> {
        let (xml, blocks) = self.canonical_source()?;
        let occurrences =
            relationship_ids_by_blocks(&xml, blocks.as_ref(), &self.body_namespace_bindings)?;
        let live = registered
            .iter()
            .filter(|id| occurrences.contains(id))
            .cloned()
            .collect::<HashSet<_>>();
        if !relationship_remap.is_empty() && !live.is_empty() {
            if let Some(writer) = template {
                let source = writer.document_source(&self.document)?;
                let edits = relationship_remap_edits(
                    source.as_bytes(),
                    relationship_remap,
                    &self.body_namespace_bindings,
                )?;
                let source = apply_template_source_edits(writer.operation, source, edits)?;
                self.document = crate::document::DocumentModel::unproved(
                    writer
                        .sources
                        .read_document(writer.operation, Arc::new(source))?,
                );
                self.main_part_blocks = None;
                self.main_body_scans
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .serialized = None;
            } else {
                let fast = offered_canonical_blocks(blocks.as_ref()).and_then(|record| {
                    let edits = record.relationship_remap_edits(
                        relationship_remap,
                        &self.body_namespace_bindings,
                    )?;
                    let mut edited = record.edited(&edits)?;
                    let qualified_xml = edited.qualify_document_children()?;
                    let reconciled = edits.is_empty()
                        && edited.reconcile_readback(&qualified_xml, &mut self.document);
                    if reconciled {
                        return Some((None, edited));
                    }
                    // Original phase2, exactly once after original mandatory phase1.
                    let document = edited.proved_document(&qualified_xml)?;
                    document.validate_resource_limits().ok()?;
                    Some((Some(document), edited))
                });
                if let Some((document, mut record)) = fast {
                    if let Some(document) = document {
                        self.document = crate::document::DocumentModel::unproved(document);
                    }
                    record.part = None;
                    self.main_body_scans
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .serialized = Some(record);
                } else {
                    let updated = remap_xml_relationship_ids_with_bindings(
                        &xml,
                        relationship_remap,
                        &self.body_namespace_bindings,
                    )?;
                    self.document =
                        crate::document::DocumentModel::unproved(CT_Document::from_xml(&updated)?);
                }
            }
        }
        if live.is_empty() {
            self.identifiers
                .authored_nested_story_relationship_ids
                .remove(physical_owner);
        } else {
            let remapped = live
                .iter()
                .map(|id| relationship_remap.get(id).unwrap_or(id).clone())
                .collect();
            self.identifiers
                .authored_nested_story_relationship_ids
                .insert(physical_owner.to_owned(), remapped);
        }
        Ok(!live.is_empty())
    }

    fn related_story_parts(&self) -> Vec<String> {
        let mut parts = self.active_header_footer_parts();
        if let Some(relationships) = self.package.get_part_rels(&self.doc_part_name) {
            for relationship in &relationships.items {
                if matches!(
                    relationship.rel_type.as_str(),
                    rel_types::FOOTNOTES | rel_types::ENDNOTES | rel_types::COMMENTS
                ) && relationship_is_internal(relationship)
                {
                    let part =
                        OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target);
                    if self.package.contains_part(&part) && !parts.contains(&part) {
                        parts.push(part);
                    }
                }
            }
        }
        parts
    }

    fn main_story_relationship_ids(&self) -> Vec<String> {
        let mut modeled = Vec::new();
        collect_relationship_ids(&self.document.body.content, &mut modeled);
        if let Some(section) = &self.document.body.sect_pr {
            for reference in section.header_refs.iter().chain(&section.footer_refs) {
                if !modeled.contains(&reference.rel_id) {
                    modeled.push(reference.rel_id.clone());
                }
            }
        }
        let owner = relationship_owner_identity(&self.doc_part_name);
        let nested = self
            .identifiers
            .authored_nested_story_relationship_ids
            .get(&owner)
            .cloned()
            .unwrap_or_default();
        let semantic = modeled
            .iter()
            .cloned()
            .chain(nested.iter().cloned())
            .collect::<HashSet<_>>();
        if let Ok((xml, blocks)) = self.canonical_source()
            && let Some(ordered) = {
                if let Some(all) = offered_canonical_blocks(blocks.as_ref())
                    .and_then(|blocks| blocks.proved_relationship_ids(&xml, &[]))
                {
                    let mut seen = HashSet::new();
                    let ordered = all
                        .into_iter()
                        .filter(|id| semantic.contains(id) && seen.insert(id.clone()))
                        .collect::<Vec<_>>();
                    (ordered.len() == semantic.len()).then_some(ordered)
                } else {
                    authored_relationship_ids_in_source_order(&xml, &semantic)
                }
            }
        {
            return ordered;
        }
        let mut nested = nested.into_iter().collect::<Vec<_>>();
        nested.sort();
        for id in nested {
            if !modeled.contains(&id) {
                modeled.push(id);
            }
        }
        modeled
    }

    fn canonicalize_related_story_relationships(
        &mut self,
        parts: &[String],
        occupied_drawings: &mut HashSet<u32>,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        let mut partially_authored_drawing_slots = HashMap::new();
        for owner in parts {
            if self.identifiers.authored_story_parts.contains(owner) {
                continue;
            }
            let owner_identity = relationship_owner_identity(owner);
            let partially_authored = self
                .identifiers
                .authored_story_relationship_ids
                .get(&owner_identity)
                .cloned()
                .unwrap_or_default();
            if partially_authored.is_empty() {
                continue;
            }
            let xml = self
                .package
                .get_part(owner)
                .ok_or_else(|| Error::Other(format!("story part {owner} is missing")))?;
            let authored_image_relationship_ids = self
                .package
                .get_part_rels(owner)
                .into_iter()
                .flat_map(|relationships| &relationships.items)
                .filter(|relationship| {
                    relationship.rel_type == rel_types::IMAGE
                        && partially_authored.contains(&relationship.id)
                })
                .map(|relationship| relationship.id.clone())
                .collect::<HashSet<_>>();
            let drawing_slots =
                authored_drawing_slots_in_source_order(xml, &[], &authored_image_relationship_ids)?
                    .into_iter()
                    .map(|(_, span, id)| (span, id))
                    .collect::<Vec<_>>();
            for (_, id) in &drawing_slots {
                occupied_drawings.remove(id);
            }
            partially_authored_drawing_slots.insert(owner.clone(), drawing_slots);
        }

        for owner in parts {
            let owner_identity = relationship_owner_identity(owner);
            let fully_authored = self.identifiers.authored_story_parts.contains(owner);
            let partially_authored = self
                .identifiers
                .authored_story_relationship_ids
                .get(&owner_identity)
                .cloned()
                .unwrap_or_default();
            if !fully_authored && partially_authored.is_empty() {
                continue;
            }
            let xml = self
                .package
                .get_part_shared(owner)
                .cloned()
                .ok_or_else(|| Error::Other(format!("story part {owner} is missing")))?;
            let referenced = xml_relationship_ids_in_order(&xml)?;
            let preserved = self
                .identifiers
                .preserved_relationship_ids
                .get(&owner_identity)
                .cloned()
                .unwrap_or_default();
            let relationships = self
                .package
                .get_part_rels(owner)
                .map(|relationships| relationships.items.clone())
                .unwrap_or_default();
            let semantic = referenced
                .into_iter()
                .filter(|id| {
                    (fully_authored && !preserved.contains(id) || partially_authored.contains(id))
                        && relationships
                            .iter()
                            .any(|relationship| relationship.id == *id)
                })
                .collect::<Vec<_>>();
            let mut occupied = preserved.clone();
            occupied.extend(
                relationships
                    .iter()
                    .filter(|relationship| !semantic.contains(&relationship.id))
                    .map(|relationship| relationship.id.clone()),
            );
            let mut cursor = if semantic.is_empty() {
                0
            } else {
                preserved
                    .iter()
                    .filter_map(|id| id.strip_prefix("rId")?.parse::<u32>().ok())
                    .max()
                    .map_or(Ok(0), |maximum| {
                        maximum.checked_add(1).ok_or_else(|| {
                            Error::Other("relationship id range is exhausted".to_owned())
                        })
                    })?
            };
            let mut remap = HashMap::new();
            for old in semantic {
                remap.insert(
                    old,
                    reserve_relationship_from_cursor(&mut occupied, &mut cursor)?,
                );
            }
            if let Some(owner_relationships) = self.package.get_part_rels_mut(owner) {
                for relationship in &mut owner_relationships.items {
                    if let Some(updated) = remap.get(&relationship.id) {
                        relationship.id.clone_from(updated);
                    }
                }
                let canonicalized_relationship_ids = if fully_authored {
                    owner_relationships
                        .items
                        .iter()
                        .filter(|relationship| !preserved.contains(&relationship.id))
                        .map(|relationship| relationship.id.clone())
                        .collect::<HashSet<_>>()
                } else {
                    remap.values().cloned().collect::<HashSet<_>>()
                };
                if !canonicalized_relationship_ids.is_empty() {
                    let mut authored = owner_relationships
                        .items
                        .iter()
                        .filter(|relationship| {
                            canonicalized_relationship_ids.contains(&relationship.id)
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    authored.sort_by_key(|relationship| {
                        relationship
                            .id
                            .strip_prefix("rId")
                            .and_then(|value| value.parse::<u32>().ok())
                            .unwrap_or(u32::MAX)
                    });
                    let mut authored = authored.into_iter();
                    for relationship in &mut owner_relationships.items {
                        if canonicalized_relationship_ids.contains(&relationship.id) {
                            *relationship = authored
                                .next()
                                .expect("each authored relationship slot has a value");
                        }
                    }
                }
            }
            let mut drawing_edits = Vec::new();
            if !fully_authored {
                for (span, _) in partially_authored_drawing_slots
                    .remove(owner)
                    .unwrap_or_default()
                {
                    let id = reserve_u32(occupied_drawings, 1, "drawing")?;
                    drawing_edits.push((span.start, span.end, id.to_string().into_bytes()));
                }
            }
            let original_storage = Arc::clone(&xml);
            let mut part_changed = false;
            let mut produced_source = None;
            let xml = if let Some(template) = template.as_deref_mut() {
                let source = template.take_related_source(owner, &xml)?.ok_or_else(|| {
                    Error::Other(format!("related canonicalization lost its source: {owner}"))
                })?;
                part_changed |= drawing_edits.iter().any(|(start, end, value)| {
                    source.as_bytes().get(*start..*end) != Some(value.as_slice())
                });
                let source =
                    apply_shared_template_source_edits(template.operation, source, drawing_edits)?;
                let edits = relationship_remap_edits(source.as_bytes(), &remap, &[])?;
                part_changed |= edits.iter().any(|(start, end, value)| {
                    source.as_bytes().get(*start..*end) != Some(value.as_slice())
                });
                let source = apply_shared_template_source_edits(template.operation, source, edits)?;
                let source = if fully_authored {
                    let edits = authored_doc_pr_edits(source.as_bytes(), occupied_drawings)?;
                    part_changed |= edits.iter().any(|(start, end, value)| {
                        source.as_bytes().get(*start..*end) != Some(value.as_slice())
                    });
                    apply_shared_template_source_edits(template.operation, source, edits)?
                } else {
                    source
                };
                let xml = if part_changed {
                    let retained = template.operation.repeat_source(&source)?;
                    template.operation.finish_source(retained)?
                } else {
                    Vec::new()
                };
                produced_source = Some(source);
                xml
            } else {
                let mut xml = xml.as_ref().clone();
                drawing_edits.sort_by_key(|(start, _, _)| *start);
                for (start, end, replacement) in drawing_edits.into_iter().rev() {
                    part_changed |= xml.get(start..end) != Some(replacement.as_slice());
                    xml.splice(start..end, replacement);
                }
                let edits = relationship_remap_edits(&xml, &remap, &[])?;
                for (start, end, replacement) in edits.into_iter().rev() {
                    part_changed |= xml.get(start..end) != Some(replacement.as_slice());
                    xml.splice(start..end, replacement);
                }
                xml
            };
            if let Some(ids) = self
                .identifiers
                .authored_story_relationship_ids
                .get_mut(&owner_identity)
            {
                *ids = ids
                    .drain()
                    .map(|id| remap.get(&id).cloned().unwrap_or(id))
                    .collect();
            }
            let xml = if fully_authored && produced_source.is_none() {
                let edits = authored_doc_pr_edits(&xml, occupied_drawings)?;
                let mut xml = xml;
                for (start, end, replacement) in edits.into_iter().rev() {
                    part_changed |= xml.get(start..end) != Some(replacement.as_slice());
                    xml.splice(start..end, replacement);
                }
                xml
            } else {
                xml
            };
            if part_changed {
                self.package.set_part(owner, xml);
            } else {
                // The actual edit plans made no byte change. Keep the existing
                // immutable producer storage, not a new buffer matched by value.
                drop(xml);
                debug_assert!(
                    self.package
                        .get_part_shared(owner)
                        .is_some_and(|stored| Arc::ptr_eq(stored, &original_storage))
                );
            }
            if let (Some(template), Some(source)) = (template.as_deref_mut(), produced_source) {
                let bytes = self
                    .package
                    .get_part_shared(owner)
                    .expect("actual published related part");
                template.sources.put_related_source(owner, bytes, source);
            }
        }
        Ok(())
    }

    fn canonicalize_image_parts(&mut self, main_checked: bool) -> Result<()> {
        let mut ordered = Vec::new();
        let main_moves_nothing = main_checked
            && self
                .package
                .get_part_rels(&self.doc_part_name)
                .is_none_or(|relationships| {
                    relationships
                        .items
                        .iter()
                        .filter(|relationship| relationship.rel_type == rel_types::IMAGE)
                        .all(|relationship| {
                            self.identifiers
                                .part_is_preserved(&OpcPackage::resolve_rel_target(
                                    &self.doc_part_name,
                                    &relationship.target,
                                ))
                        })
                });
        let main_ids = if main_moves_nothing {
            Vec::new()
        } else {
            {
                let (xml, blocks) = self.canonical_source()?;
                relationship_ids_by_blocks(&xml, blocks.as_ref(), &[])?
            }
        };
        let mut owners = vec![(self.doc_part_name.clone(), main_ids)];
        for owner in self.related_story_parts() {
            let owner_identity = relationship_owner_identity(&owner);
            if self.identifiers.authored_story_parts.contains(&owner)
                || self
                    .identifiers
                    .authored_story_relationship_ids
                    .contains_key(&owner_identity)
            {
                let ids = self
                    .package
                    .get_part(&owner)
                    .map(xml_relationship_ids_in_order)
                    .transpose()?
                    .unwrap_or_default();
                owners.push((owner, ids));
            }
        }
        for (owner, ids) in &owners {
            let Some(relationships) = self.package.get_part_rels(owner) else {
                continue;
            };
            for id in ids {
                if let Some(relationship) = relationships.get_by_id(id)
                    && relationship.rel_type == rel_types::IMAGE
                {
                    let part = OpcPackage::resolve_rel_target(owner, &relationship.target);
                    if !self.identifiers.part_is_preserved(&part) && !ordered.contains(&part) {
                        ordered.push(part);
                    }
                }
            }
        }
        let mut occupied = self.identifiers.preserved_part_names.clone();
        occupied.extend(
            self.package
                .parts
                .keys()
                .filter(|part| !ordered.contains(part))
                .map(|part| part_name_identity(part)),
        );
        let mut remap = HashMap::new();
        for old in ordered {
            let extension = old
                .rsplit_once('.')
                .map(|(_, extension)| extension)
                .unwrap_or("bin")
                .to_owned();
            remap.insert(
                old,
                reserve_part_from_set(&mut occupied, "/word/media", "image", &extension)?,
            );
        }
        for (owner, _) in &owners {
            if let Some(relationships) = self.package.get_part_rels_mut(owner) {
                for relationship in &mut relationships.items {
                    if relationship.rel_type == rel_types::IMAGE {
                        let old = OpcPackage::resolve_rel_target(owner, &relationship.target);
                        if let Some(updated) = remap.get(&old) {
                            relationship.target = relative_descendant_target(owner, updated);
                        }
                    }
                }
            }
        }
        let mut moved = Vec::new();
        for (old, updated) in remap {
            if old != updated
                && let Some(bytes) = self.package.remove_part(&old)
            {
                moved.push((updated.clone(), bytes));
                if let Some(content_type) = self.package.content_types.remove_override(&old) {
                    self.package
                        .content_types
                        .add_override(&updated, &content_type);
                }
            }
        }
        for (part_name, bytes) in moved {
            self.package.set_part(&part_name, bytes);
        }
        Ok(())
    }

    /// Commit staged package state without discarding reusable layout work.
    fn take_staged_commit_origin(&mut self) -> StagedCommitOrigin {
        let package = self.package.clone();
        StagedCommitOrigin {
            package,
            signature_invalidated: self.package_signatures_invalidated,
            normal_engine: std::mem::replace(&mut self.normal_layout_engine, Mutex::new(None)),
            bundled_engine: std::mem::replace(
                &mut self.bundled_fallback_layout_engine,
                Mutex::new(None),
            ),
        }
    }

    fn finish_staged_candidate(
        source_package: &OpcPackage,
        source_invalidated: bool,
        normal_engine: &mut Mutex<Option<oxoffice_docx_layout::engine::Engine>>,
        bundled_engine: &mut Mutex<Option<oxoffice_docx_layout::engine::Engine>>,
        mut candidate: Self,
    ) -> Self {
        candidate.package_signatures_invalidated |= candidate
            .package
            .package_rels
            .items
            .iter()
            .any(|relationship| relationship.rel_type == rel_types::DIGITAL_SIGNATURE_ORIGIN)
            && (source_invalidated
                || crate::embedded::synchronized_package_mutation_invalidates_signature(
                    source_package,
                    &candidate.package,
                ));
        std::mem::swap(normal_engine, &mut candidate.normal_layout_engine);
        std::mem::swap(
            bundled_engine,
            &mut candidate.bundled_fallback_layout_engine,
        );
        candidate
    }

    /// Apply `edit`, a properties edit, without staging the whole document.
    ///
    /// `edit` first reserves the properties part in the package graph and the
    /// identifier registry, which can fail, then writes the properties model
    /// and its part name, which cannot. The package graph, whose part bytes
    /// are shared, and the registry are saved first and put back on refusal,
    /// so a refused edit leaves no reserved name or relationship id. A
    /// success leaves what [`Self::commit_staged_mutation`] leaves for a
    /// staged clone: the derived layout is dropped and a signed package
    /// records a package mutation. Debug builds run the staged edit on a
    /// clone and compare. `edit` is `Fn` for that second run.
    fn edit_properties(&mut self, edit: impl Fn(&mut Self) -> Result<()>) -> Result<()> {
        #[cfg(debug_assertions)]
        let staged = self.clone_for_staging();
        let package = self.package.clone();
        let identifiers = self.identifiers.clone();
        let result = edit(self);
        match &result {
            Ok(()) => {
                self.package_signatures_invalidated |=
                    self.package.package_rels.items.iter().any(|relationship| {
                        relationship.rel_type == rel_types::DIGITAL_SIGNATURE_ORIGIN
                    }) && (self.package_signatures_invalidated
                        || crate::embedded::synchronized_package_mutation_invalidates_signature(
                            &package,
                            &self.package,
                        ));
                self.layout_cache
                    .get_mut()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                self.deterministic_layout_cache
                    .get_mut()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
            }
            Err(_) => {
                self.package = package;
                self.identifiers = identifiers;
            }
        }
        #[cfg(debug_assertions)]
        self.assert_properties_edit_matches_staged(staged, &edit, &result);
        result
    }

    /// Debug proof that a direct properties edit left the package graph, the
    /// properties models and their part names of the staged edit, or
    /// refused with its error.
    #[cfg(debug_assertions)]
    fn assert_properties_edit_matches_staged(
        &self,
        mut staged: Self,
        edit: &impl Fn(&mut Self) -> Result<()>,
        result: &Result<()>,
    ) {
        match (result, edit(&mut staged)) {
            (Ok(()), Ok(())) => {
                assert!(
                    staged.package.same_package_as(&self.package),
                    "direct properties edit changed another package graph"
                );
                assert_eq!(
                    staged.identifiers, self.identifiers,
                    "direct properties edit left another identifier registry"
                );
                assert_eq!(staged.core_properties, self.core_properties);
                assert_eq!(
                    format!("{:?}", staged.application_properties),
                    format!("{:?}", self.application_properties)
                );
                assert_eq!(staged.custom_properties, self.custom_properties);
                assert_eq!(
                    (
                        &staged.core_properties_part_name,
                        &staged.application_properties_part_name,
                        &staged.custom_properties_part_name,
                        staged.custom_properties_owned,
                    ),
                    (
                        &self.core_properties_part_name,
                        &self.application_properties_part_name,
                        &self.custom_properties_part_name,
                        self.custom_properties_owned,
                    ),
                    "direct properties edit named another part"
                );
            }
            (Err(direct), Err(staged)) => assert_eq!(direct.to_string(), staged.to_string()),
            (Ok(()), Err(staged)) => panic!("direct properties edit hid the staged error {staged}"),
            (Err(direct), Ok(())) => panic!("direct properties edit refused with {direct}"),
        }
    }

    pub(crate) fn commit_staged_mutation(&mut self, candidate: Self) {
        let source_path = self.source_path.take();
        *self = Self::finish_staged_candidate(
            &self.package,
            self.package_signatures_invalidated,
            &mut self.normal_layout_engine,
            &mut self.bundled_fallback_layout_engine,
            candidate,
        );
        self.source_path = source_path;
    }

    /// Open a document from a file path.
    ///
    /// The document remembers the file, and [`Document::save`] refuses to
    /// replace it unless asked, see [`Document::save_with_options`].
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let package = OpcPackage::open(path.as_ref())?;
        let mut document = Self::from_opened_package(package, true, None)?;
        document.remember_source(path.as_ref());
        Ok(document)
    }

    /// Open a password-protected agile-encrypted document from a file path.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn open_encrypted<P: AsRef<Path>>(path: P, password: &str) -> Result<Self> {
        let file = std::fs::File::open(path.as_ref())?;
        let package = OpcPackage::from_encrypted_reader(file, password)?;
        let mut document = Self::from_package(package)?;
        document.remember_source(path.as_ref());
        Ok(document)
    }

    /// Record `path` as the file this document was opened from. A path that
    /// cannot be resolved records nothing.
    pub(crate) fn remember_source(&mut self, path: &Path) {
        self.source_path = std::fs::canonicalize(path).ok();
    }

    /// Open a document from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::from_bytes_with_limits(bytes, PackageReadLimits::UNBOUNDED)
    }

    /// Open a password-protected agile-encrypted document from bytes.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn from_encrypted_bytes(bytes: &[u8], password: &str) -> Result<Self> {
        Self::from_encrypted_bytes_with_limits(bytes, password, PackageReadLimits::UNBOUNDED)
    }

    /// Open an agile-encrypted document while bounding OPC archive expansion.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn from_encrypted_bytes_with_limits(
        bytes: &[u8],
        password: &str,
        limits: PackageReadLimits,
    ) -> Result<Self> {
        let cursor = std::io::Cursor::new(bytes);
        let package = OpcPackage::from_encrypted_reader_with_limits(cursor, password, limits)?;
        Self::from_opened_package(package, true, None)
    }

    /// Open a document from bytes while bounding OPC archive expansion.
    pub fn from_bytes_with_limits(bytes: &[u8], limits: PackageReadLimits) -> Result<Self> {
        let cursor = std::io::Cursor::new(bytes);
        let package = OpcPackage::from_reader_with_limits(cursor, limits)?;
        Self::from_opened_package(package, true, None)
    }

    /// Return the exact Word package class declared for the main document part.
    pub fn package_class(&self) -> Result<WordPackageClass> {
        validated_word_package_class(&self.package).map(|(_, class)| class)
    }

    /// Serialize a staged copy as the requested Word package class.
    ///
    /// This changes only the main part's content-type override. Macro projects
    /// and all other package payloads remain present.
    pub fn to_bytes_as(&self, class: WordPackageClass) -> Result<Vec<u8>> {
        self.validate_model_resource_limits()?;
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_output()?;
        candidate
            .package
            .content_types
            .add_override(&candidate.doc_part_name, class.content_type());
        candidate.package_signatures_invalidated |=
            crate::embedded::synchronized_package_mutation_invalidates_signature(
                &self.package,
                &candidate.package,
            );
        crate::embedded::persist_invalidated_package_signature(
            &mut candidate.package,
            candidate.package_signatures_invalidated,
        )?;
        let mut output = std::io::Cursor::new(Vec::new());
        candidate.package.write_to(&mut output)?;
        let bytes = output.into_inner();
        Self::from_bytes(&bytes)?;
        Ok(bytes)
    }

    /// Save a staged copy as the requested Word package class.
    pub fn save_as_package_class<P: AsRef<Path>>(
        &self,
        path: P,
        class: WordPackageClass,
    ) -> Result<()> {
        let bytes = self.to_bytes_as(class)?;
        write_atomic_file(
            path.as_ref(),
            &bytes,
            "oxoffice",
            "invalid Word package file name",
            "could not allocate Word package save staging file",
        )?;
        Ok(())
    }

    /// Verify package signatures without asserting certificate-chain trust.
    #[cfg(feature = "digital-signatures")]
    pub fn verify_signatures(
        &self,
    ) -> std::result::Result<Vec<oxoffice_opc::SignatureReport>, oxoffice_opc::OpcError> {
        self.package.verify_signatures()
    }

    /// Sign the current typed document with the strict RSA-SHA256 profile.
    ///
    /// The private key must be PKCS#8 DER or one unencrypted PRIVATE KEY PEM
    /// block. The certificate must be X.509 DER or one CERTIFICATE PEM block. Typed state is flushed and signed on a staged clone, so any
    /// serialization, key, certificate, or signature failure leaves this
    /// document unchanged. Certificate-chain trust remains caller policy.
    #[cfg(all(feature = "digital-signatures", not(target_arch = "wasm32")))]
    pub fn sign(
        &mut self,
        private_key_pkcs8_der: &[u8],
        certificate_der: &[u8],
    ) -> Result<oxoffice_opc::SignatureReport> {
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_output()?;
        let report = candidate
            .package
            .sign(private_key_pkcs8_der, certificate_der)?;
        self.commit_staged_mutation(candidate);
        Ok(report)
    }

    pub(crate) fn from_package(package: OpcPackage) -> Result<Self> {
        Self::from_opened_package(package, false, None)
    }

    pub(crate) fn from_opened_package(
        package: OpcPackage,
        read_from_bytes: bool,
        scoped_source: Option<ScopedDocumentRead<'_, '_, '_>>,
    ) -> Result<Self> {
        Self::from_package_reusing(
            package,
            read_from_bytes,
            None,
            None,
            None,
            None,
            scoped_source,
        )
    }

    fn from_package_reusing(
        package: OpcPackage,
        read_from_bytes: bool,
        staged: Option<(&DocumentIdentifiers, &Parsed)>,
        blocks: Option<&MainPartBlocks>,
        writer_blocks: Option<&MainPartBlocks>,
        styles_read: Option<&StylesPartCache>,
        mut scoped_source: Option<ScopedDocumentRead<'_, '_, '_>>,
    ) -> Result<Self> {
        let (doc_part_name, _) = validated_word_package_class(&package)?;

        let doc_xml = package
            .get_part(&doc_part_name)
            .ok_or(Error::NoDocumentPart)?;
        let namespace_scopes = document_namespace_scopes(doc_xml)?;
        let kept = staged.map(|(_, parsed)| parsed).filter(|parsed| {
            parsed.main_part == doc_part_name
                && parsed.main_part_xml().is_some_and(|kept| kept == doc_xml)
                && parsed.root_namespace_declarations == namespace_scopes.root_declarations
                && parsed.body_namespace_declarations == namespace_scopes.body_declarations
                && parsed.body_namespace_bindings == namespace_scopes.body_bindings
        });
        // A pre-operation child cache has no operation-bound reader facts.
        // Use the existing authoritative factory below rather than reparsing
        // solely to certify the cached model. Qualified kept-model reuse stays.
        let hinted = scoped_source
            .is_none()
            .then(|| blocks.and_then(|blocks| blocks.proved_document(doc_xml)))
            .flatten();
        let nested_namespace_owners = OnceLock::new();
        if let Some(kept) = kept
            && let Some(owners) = kept.nested_namespace_owners.get()
        {
            let _ = nested_namespace_owners.set((*owners).clone());
        }
        if nested_namespace_owners.get().is_none()
            && let Some(owners) = blocks.and_then(|blocks| blocks.retained_nested_owners(doc_xml))
        {
            let _ = nested_namespace_owners.set(Some(owners));
        }
        let (plain_local_edit_blocked, unsavable_body_edit_cache) = plain_local_edit_facts(
            &namespace_scopes.root_declarations,
            &namespace_scopes.body_declarations,
            doc_xml,
            &nested_namespace_owners,
        );
        if let Some(input) = scoped_source.as_ref()
            && input.source.as_bytes() != doc_xml
        {
            // Check the known producer binding, never discover provenance or
            // grant input roles merely because two buffers have equal bytes.
            return Err(Error::Other(
                "scoped document source does not match its produced main part".into(),
            ));
        }
        let document = if let Some(document) = hinted {
            document
        } else {
            match kept.and_then(|kept| kept.main_document().cloned()) {
                Some(document) => document,
                None => match scoped_source.as_mut() {
                    Some(input) => input.operation.source_document(input.source)?,
                    None => CT_Document::from_xml(doc_xml)?,
                },
            }
        };

        // Resolve the part a relationship of the given type points at.
        let resolve_part = |rel_type: &str| -> Option<String> {
            let rels = package.get_part_rels(&doc_part_name)?;
            let rel = rels.items.iter().find(|relationship| {
                relationship.rel_type == rel_type && relationship_is_internal(relationship)
            })?;
            Some(OpcPackage::resolve_rel_target(&doc_part_name, &rel.target))
        };

        // Try to load styles, remembering where they came from.
        let styles_part_name = resolve_part(rel_types::STYLES);
        let (styles, styles_part_cache) = match styles_part_name
            .as_deref()
            .and_then(|part| package.get_part_shared(part))
        {
            Some(bytes) => {
                if let Some(kept) = styles_read.filter(|kept| Arc::ptr_eq(bytes, &kept.bytes)) {
                    (Arc::clone(&kept.parsed), Some(kept.clone()))
                } else {
                    let parsed = Arc::new(CT_Styles::from_xml(bytes)?);
                    let proof = if parsed.to_xml().is_ok() {
                        StylesSerializationProof::Serializable
                    } else {
                        StylesSerializationProof::Unserializable
                    };
                    let cache = StylesPartCache {
                        bytes: Arc::clone(bytes),
                        parsed: Arc::clone(&parsed),
                        emitted_model: None,
                        proof,
                    };
                    (parsed, Some(cache))
                }
            }
            None => (Arc::new(CT_Styles::new_default()), None),
        };

        // Try to load numbering definitions
        let numbering_part_name = resolve_part(rel_types::NUMBERING);
        let numbering = match numbering_part_name
            .as_deref()
            .and_then(|p| package.get_part(p))
        {
            Some(num_xml) => Some(CT_Numbering::from_xml(num_xml)?),
            None => None,
        };

        let settings_part_name = resolve_part(rel_types::SETTINGS);
        let settings = match settings_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
        {
            Some(xml) => Some(CT_Settings::from_xml(xml)?),
            None => None,
        };

        let web_settings_part_name = resolve_part(rel_types::WEB_SETTINGS);
        let web_settings = match web_settings_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
        {
            Some(xml) => Some(CT_WebSettings::from_xml(xml)?),
            None => None,
        };

        let theme_part_name = resolve_part(rel_types::THEME);
        let theme = match theme_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
        {
            Some(xml) => oxoffice_drawing::theme::CT_OfficeStyleSheet::from_xml(xml).ok(),
            None => None,
        };

        let font_table_part_name = resolve_part(rel_types::FONT_TABLE);
        let font_table = match font_table_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
        {
            Some(xml) => FontTable::from_xml(xml).ok(),
            None => None,
        };

        // Core properties are a package-level relationship, not a document part.
        let core_properties_part_name = package
            .package_rels
            .items
            .iter()
            .find(|relationship| {
                relationship.rel_type == CORE_PROPERTIES_REL_TYPE
                    && relationship_is_internal(relationship)
            })
            .map(|rel| OpcPackage::resolve_rel_target("/", &rel.target));
        let core_properties = core_properties_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
            .and_then(|xml| CoreProperties::from_xml(xml).ok())
            .map(Box::new);

        let application_properties_part_name = package
            .package_rels
            .items
            .iter()
            .find(|relationship| {
                relationship.rel_type == rel_types::EXTENDED_PROPERTIES
                    && relationship_is_internal(relationship)
            })
            .map(|rel| OpcPackage::resolve_rel_target("/", &rel.target));
        let application_properties = application_properties_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
            .and_then(|xml| AppProperties::from_xml(xml).ok())
            .map(repair_application_version);

        let custom_properties_part_name = package
            .package_rels
            .items
            .iter()
            .find(|relationship| {
                relationship.rel_type == rel_types::CUSTOM_PROPERTIES
                    && relationship_is_internal(relationship)
            })
            .map(|rel| OpcPackage::resolve_rel_target("/", &rel.target));
        let custom_properties = custom_properties_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
            .and_then(|xml| CustomProperties::from_xml(xml).ok());

        let footnotes_part_name = resolve_part(rel_types::FOOTNOTES);
        let footnotes_root_namespace_declarations = footnotes_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
            .map(root_namespace_declarations)
            .transpose()?
            .unwrap_or_default();
        let footnotes = match footnotes_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
        {
            Some(xml) => {
                let paired = footnotes_part_name.as_ref().and_then(|part| {
                    scoped_source
                        .as_ref()
                        .and_then(|input| input.related.and_then(|related| related.get(part)))
                });
                if let (Some(input), Some(source)) = (scoped_source.as_mut(), paired.cloned()) {
                    match input.operation.source_notes(&source) {
                        Ok(notes) => notes,
                        Err(_) => {
                            input.operation.reserve_template_text_copy(0)?;
                            Default::default()
                        }
                    }
                } else {
                    oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(xml).unwrap_or_default()
                }
            }
            None => Default::default(),
        };

        let comments_part_name = resolve_part(rel_types::COMMENTS);
        let comments = match comments_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
        {
            Some(xml) => {
                let paired = comments_part_name
                    .as_ref()
                    .and_then(|part| {
                        scoped_source
                            .as_ref()
                            .and_then(|input| input.related.and_then(|related| related.get(part)))
                    })
                    .cloned();
                Some(
                    if let (Some(input), Some(source)) = (scoped_source.as_mut(), paired) {
                        input.operation.source_comments(&source)?
                    } else {
                        oxoffice_docx_oxml::comments::CT_Comments::from_xml(xml)?
                    },
                )
            }
            None => None,
        };
        let comments_extended_part_name = resolve_part(crate::comments::COMMENTS_EXTENDED_REL_TYPE);
        let comments_extended = match comments_extended_part_name
            .as_deref()
            .and_then(|part| package.get_part(part))
        {
            Some(xml) => Some(oxoffice_docx_oxml::comments_extended::CT_CommentsEx::from_xml(xml)?),
            None => None,
        };
        let glossary = crate::building_block::load_glossary(&package, &doc_part_name)?;
        let (glossary_part_name, glossary) = match glossary {
            Some((part_name, glossary)) => (Some(part_name), Some(glossary)),
            None => (None, None),
        };

        let package_signatures_invalidated =
            crate::embedded::known_invalid_package_signature_on_open(&package);
        let identifier_hint =
            blocks.and_then(|blocks| blocks.identifier_hint(doc_xml, &namespace_scopes));
        let identifiers = DocumentIdentifiers::scan_reusing(
            &package,
            staged.map(|(identifiers, _)| identifiers),
            identifier_hint.as_ref(),
        )?;
        let serialized = document.validate_resource_limits().ok().and_then(|()| {
            MainPartBlocks::serialize(
                &document,
                &document.body.content,
                // A producer source record proves reads, while the retained
                // writer record independently proves normalized child bytes.
                // serialize rechecks model alignment, fields and the frame.
                offered_canonical_blocks(writer_blocks.or(blocks)),
            )
            .ok()
        });
        let (serialization_proof, serialized_blocks) = match serialized {
            Some((xml, record)) => {
                let retained = block_proofs_enabled() && !record.head.is_empty();
                (Some(xml), retained.then_some(record))
            }
            None => (None, None),
        };
        let mut parsed_record = Parsed::of(
            &package,
            &doc_part_name,
            read_from_bytes,
            namespace_scopes.root_declarations.clone(),
            namespace_scopes.body_declarations.clone(),
            namespace_scopes.body_bindings.clone(),
            serialization_proof,
        );
        parsed_record.nested_namespace_owners = nested_namespace_owners;
        if let Some(kept) = kept {
            let relationships = package
                .get_part_rels(&doc_part_name)
                .map(|relationships| &relationships.items);
            let kept_relationships = kept
                .package
                .get_part_rels(&kept.main_part)
                .map(|relationships| &relationships.items);
            if relationships == kept_relationships
                && let Some(unchanged) = kept.unchanged_main_part.get()
            {
                parsed_record.unchanged_main_part = OnceLock::from((*unchanged).clone());
            }
        }
        // Every branch above obtained an actual parse of these exact source
        // bytes, either fresh or from a qualified source proof. Keep it at
        // this opening instead of parsing a fresh source a second time later.
        parsed_record.initial_writer_blocks = serialized_blocks
            .as_ref()
            .map(|record| Arc::downgrade(&record.blocks));
        let (document, opening_document) = DocumentModel::opening_pair(document);
        parsed_record.main_document = OnceLock::from(Some(opening_document));
        let parsed = Some(Arc::new(parsed_record));
        let local_cell_source = LocalCellSource::classified(
            &package,
            &doc_part_name,
            &namespace_scopes.root_declarations,
            &namespace_scopes.body_declarations,
            !plain_local_edit_blocked,
        );
        Ok(Document {
            package,
            document,
            root_namespace_declarations: namespace_scopes.root_declarations,
            body_namespace_declarations: namespace_scopes.body_declarations,
            body_namespace_bindings: namespace_scopes.body_bindings,
            plain_local_edit_blocked,
            local_cell_source,
            unsavable_body_edit_cache,
            styles,
            styles_part_cache,
            numbering,
            published_abstract_by_live: HashMap::new(),
            published_instance_by_live: HashMap::new(),
            core_properties,
            application_properties,
            custom_properties,
            core_properties_part_name,
            application_properties_part_name,
            custom_properties_part_name,
            custom_properties_owned: false,
            doc_part_name,
            styles_part_name,
            numbering_part_name,
            settings,
            settings_part_name,
            settings_owned: false,
            web_settings,
            web_settings_part_name,
            web_settings_owned: false,
            theme,
            theme_part_name,
            theme_dirty: false,
            font_table,
            font_table_part_name,
            font_table_dirty: false,
            identifiers,
            footnotes,
            footnotes_part_name,
            footnotes_root_namespace_declarations,
            footnotes_dirty: false,
            comments,
            comments_part_name,
            comments_dirty: false,
            comments_extended,
            comments_extended_part_name,
            comments_owned: false,
            comments_extended_owned: false,
            embedded_invalidated_signatures: HashSet::new(),
            package_signatures_invalidated,
            source_path: None,
            glossary,
            glossary_part_name,
            glossary_dirty: false,
            layout_cache: Mutex::new(None),
            normal_layout_engine: Mutex::new(None),
            deterministic_layout_cache: Mutex::new(None),
            bundled_fallback_layout_engine: Mutex::new(None),
            parsed,
            main_body_scans: Arc::new(Mutex::new(MainBodyScans {
                serialized: serialized_blocks,
                ..MainBodyScans::default()
            })),
            main_part_blocks: None,
        })
    }

    /// Save the document to a file path.
    ///
    /// The package is staged in a synced sibling file and renamed over `path`
    /// through [`oxoffice_opc::write_atomic_file`], so a failed save leaves an
    /// existing file as it was. A symbolic link at `path` is kept and the file
    /// it names is replaced, and on Unix that file keeps its permission bits.
    ///
    /// A `.docx`, `.docm`, `.dotx`, or `.dotm` extension selects the main
    /// part content type, so a template saved as `.docx` declares a document.
    /// Any other extension keeps the opened class, and `save_encrypted` and
    /// the Flat OPC saves ignore the extension. When the class changes, a
    /// main part that carries a VBA project cannot be saved under a
    /// macro-free extension, because the project would remain in a file that
    /// claims to carry none. [`Document::save_as_package_class`] performs
    /// that conversion explicitly and keeps the VBA part.
    ///
    /// A document opened from a file refuses to be saved over that file,
    /// whatever the spelling of `path` (a symbolic link, a relative path, a
    /// hard link), with [`Error::OverwritesSource`]. This differs from
    /// python-docx. [`Document::save_with_options`] with
    /// [`SaveOptions::overwrite_source`] replaces it.
    pub fn save<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        self.save_with_options(path, SaveOptions::default())
    }

    /// Save the document to a file path, as [`Document::save`] does, with
    /// `options`.
    pub fn save_with_options<P: AsRef<Path>>(
        &mut self,
        path: P,
        options: SaveOptions,
    ) -> Result<()> {
        let path = path.as_ref();
        if !options.overwrite_source
            && let Some(source) = &self.source_path
            && oxoffice_opc::is_same_file(source, path)
        {
            return Err(Error::OverwritesSource {
                path: path.to_path_buf(),
            });
        }
        self.validate_model_resource_limits()?;
        if let Some(class) = self.package_class_for_path(path)? {
            return self.save_as_package_class(path, class);
        }
        let (mut candidate, normalized) = self.staged_current_output()?;
        if normalized {
            candidate = candidate.reopen_prepared_staged()?;
        }
        candidate.package.save(path)?;
        if normalized {
            *self = candidate;
        }
        Ok(())
    }

    /// Whether this document and `other` save the same bytes.
    ///
    /// This save is prepared first, then `other`. The first error is returned.
    /// Packages opened from bytes are compared by [`OpcPackage::reread`].
    /// A new, in-memory, or Flat OPC package is written and compared as a ZIP.
    #[doc(hidden)]
    pub fn saves_as(&self, other: &Document) -> Result<bool> {
        let same: Result<bool> = (|| match (self.read_package(), other.read_package()) {
            (Some(read), Some(other_read)) => Ok(self
                .saved_package()?
                .reread(read)?
                .same_package_as(&other.saved_package()?.reread(other_read)?)),
            _ => {
                let mut bytes = std::io::Cursor::new(Vec::new());
                self.saved_package()?.write_to(&mut bytes)?;
                let mut other_bytes = std::io::Cursor::new(Vec::new());
                other.saved_package()?.write_to(&mut other_bytes)?;
                Ok(bytes.into_inner() == other_bytes.into_inner())
            }
        })();
        #[cfg(debug_assertions)]
        {
            let forgetting = |document: &Document| -> Result<Document> {
                document.validate_model_resource_limits()?;
                Ok(document.clone_for_debug_reference())
            };
            let bytes = (|| -> Result<bool> {
                Ok(forgetting(self)?.to_bytes()? == forgetting(other)?.to_bytes()?)
            })();
            match (&same, bytes) {
                (Ok(same), Ok(bytes)) => assert_eq!(*same, bytes, "saves_as differs from to_bytes"),
                (Err(same), Err(bytes)) => assert_eq!(same.to_string(), bytes.to_string()),
                (same, bytes) => panic!("saves_as gives {same:?} where to_bytes gives {bytes:?}"),
            }
        }
        same
    }

    fn read_package(&self) -> Option<&OpcPackage> {
        self.parsed
            .as_deref()
            .filter(|parsed| parsed.read_from_bytes)
            .map(|parsed| &parsed.package)
    }

    fn prepare_current_field_models<'a>(
        &mut self,
        mut template: Option<(
            &mut oxoffice_docx_oxml::document::ScopedWordOperation<'a>,
            &mut crate::template::TemplateWordLineage<'a>,
            Option<&oxoffice_docx_oxml::document::ScopedWordSource>,
            Option<&oxoffice_docx_oxml::document::ScopedWordSource>,
        )>,
    ) -> Result<bool> {
        let mut paragraph_index = 0usize;
        let mut changed = false;
        let mut error = None;
        let mut normalize = |paragraph: &mut CT_P, main_body: bool, comment_body: bool| {
            let current_paragraph = paragraph_index;
            paragraph_index += 1;
            if error.is_some() {
                return;
            }
            let result = (|| -> Result<()> {
                for path in paragraph.accepted_run_paths() {
                    let run = paragraph.accepted_run(&path).ok_or_else(|| {
                        Error::Other("current Field publication path is stale".into())
                    })?;
                    let mut fields = Vec::new();
                    for (slot, content) in run.content.iter().enumerate() {
                        if let oxoffice_docx_oxml::text::RunContent::Field(field) = content {
                            let current =
                                if let Some((operation, lineage, reader, comments_reader)) =
                                    template.as_mut()
                                {
                                    let bypass = crate::field::preflight_current_field(field)?;
                                    if let Some(source) = if main_body {
                                        reader.as_ref()
                                    } else if comment_body {
                                        comments_reader.as_ref()
                                    } else {
                                        None
                                    } {
                                        operation.prepare_template_field_from_read(
                                            field, source, !bypass,
                                        )?
                                    } else if let Some(frozen) =
                                        lineage.initial_field(current_paragraph, &path, slot)
                                    {
                                        operation.prepare_template_field(field, frozen, !bypass)?
                                    } else {
                                        crate::field::prepared_current_field(field)?
                                    }
                                } else {
                                    crate::field::prepared_current_field(field)?
                                };
                            if field != &current {
                                fields.push((slot, current));
                            }
                        }
                    }
                    if fields.is_empty() {
                        continue;
                    }
                    let installed_slots: Vec<_> = fields.iter().map(|(slot, _)| *slot).collect();
                    let installed = if let Some((operation, lineage, reader, comments_reader)) =
                        template.as_mut()
                    {
                        let frozen = if (main_body && reader.is_some())
                            || (comment_body && comments_reader.is_some())
                        {
                            None
                        } else {
                            lineage.initial_paragraph(current_paragraph)
                        };
                        operation
                            .install_template_prepared_fields(paragraph, &path, fields, frozen)?
                    } else {
                        let mut replacement = run.clone();
                        for (slot, field) in fields {
                            replacement.content[slot] =
                                oxoffice_docx_oxml::text::RunContent::Field(field);
                        }
                        paragraph.replace_accepted_run(&path, replacement)?
                    };
                    if !installed {
                        return Err(Error::Other(
                            "current Field publication owner disappeared".into(),
                        ));
                    }
                    if let Some((_, lineage, _, _)) = template.as_mut() {
                        for slot in installed_slots {
                            lineage.installed_field(current_paragraph, &path, slot);
                        }
                    }
                    changed = true;
                }
                Ok(())
            })();
            if let Err(failure) = result {
                error = Some(failure);
            }
        };
        visit_body_paragraphs_mut(&mut self.document.body.content, &mut |paragraph| {
            normalize(paragraph, true, false)
        });
        for note in &mut self.footnotes.footnotes {
            for paragraph in &mut note.paragraphs {
                normalize(paragraph, false, false);
            }
        }
        if let Some(comments) = &mut self.comments {
            for comment in &mut comments.comments {
                for paragraph in &mut comment.paragraphs {
                    normalize(paragraph, false, true);
                }
            }
        }
        if let Some(glossary) = &mut self.glossary {
            for part in &mut glossary.doc_parts {
                visit_body_paragraphs_mut(&mut part.body.content, &mut |paragraph| {
                    normalize(paragraph, false, false)
                });
            }
        }
        if let Some(error) = error {
            return Err(error);
        }
        Ok(changed)
    }

    fn staged_current_output(&self) -> Result<(Self, bool)> {
        self.validate_model_resource_limits()?;
        let mut candidate = self.clone_for_staging();
        let normalized = candidate.prepare_staged_output()?;
        crate::embedded::persist_invalidated_package_signature(
            &mut candidate.package,
            candidate.package_signatures_invalidated,
        )?;
        Ok((candidate, normalized))
    }

    fn saved_package(&self) -> Result<OpcPackage> {
        Ok(self.staged_current_output()?.0.package)
    }

    /// Save the document to a byte vector. Successful checked publication also
    /// installs normalized current owners for subsequent borrowed readers.
    pub fn to_bytes(&mut self) -> Result<Vec<u8>> {
        let (mut candidate, normalized) = self.staged_current_output()?;
        if normalized {
            candidate = candidate.reopen_prepared_staged()?;
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        candidate.package.write_to(&mut buf)?;
        let bytes = buf.into_inner();
        if normalized {
            *self = candidate;
        }
        Ok(bytes)
    }

    /// Serialize the bytes that [`Document::save`] writes to `path`.
    pub fn to_bytes_for_path<P: AsRef<Path>>(&mut self, path: P) -> Result<Vec<u8>> {
        match self.package_class_for_path(path.as_ref())? {
            Some(class) => self.to_bytes_as(class),
            None => self.to_bytes(),
        }
    }

    /// The class a save to `path` converts to, or `None` to keep the opened class.
    fn package_class_for_path(&self, path: &Path) -> Result<Option<WordPackageClass>> {
        let Some(target) = WordPackageClass::from_path(path) else {
            return Ok(None);
        };
        // Read the override itself: a new compact document has no main part
        // bytes until staging, so `package_class` cannot validate it yet.
        let current = self
            .package
            .content_types
            .override_for(&self.doc_part_name)
            .and_then(WordPackageClass::from_content_type);
        if current == Some(target) {
            return Ok(None);
        }
        let carries_vba =
            self.package
                .get_part_rels(&self.doc_part_name)
                .is_some_and(|relationships| {
                    relationships.get_by_type(rel_types::VBA_PROJECT).is_some()
                });
        if carries_vba && !target.is_macro_enabled() {
            return Err(Error::Other(format!(
                "cannot save {}: the document carries a VBA project and the extension names a macro-free class, save it as .docm or .dotm",
                path.display()
            )));
        }
        Ok(Some(target))
    }

    /// Save a password-protected document using the fixed Agile write profile.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn save_encrypted<P: AsRef<Path>>(&self, path: P, password: &str) -> Result<()> {
        let bytes = self.to_encrypted_bytes(password)?;
        write_atomic_file(
            path.as_ref(),
            &bytes,
            "oxoffice",
            "invalid file name",
            "could not allocate encrypted-save staging file",
        )?;
        Ok(())
    }

    /// Save a password-protected document to a byte vector.
    #[cfg(all(feature = "agile-encryption", not(target_arch = "wasm32")))]
    pub fn to_encrypted_bytes(&self, password: &str) -> Result<Vec<u8>> {
        self.validate_model_resource_limits()?;
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_output()?;
        crate::embedded::persist_invalidated_package_signature(
            &mut candidate.package,
            candidate.package_signatures_invalidated,
        )?;
        let mut encrypted = Vec::new();
        candidate
            .package
            .write_encrypted_to(&mut encrypted, password)?;
        Ok(encrypted)
    }

    fn ensure_referenced_hyperlink_style(&mut self) -> Result<()> {
        if self.styles.get_by_id("Hyperlink").is_some() {
            return Ok(());
        }
        let body = self
            .document
            .body
            .content
            .iter()
            .filter_map(|item| match item {
                BodyContent::Paragraph(paragraph) => Some(WordXmlOwner::Paragraph(paragraph)),
                BodyContent::Table(table) => Some(WordXmlOwner::Table(table)),
                BodyContent::ContentControl(control) => Some(WordXmlOwner::Control(control)),
                BodyContent::RawXml(_) => None,
            })
            .collect();
        let mut referenced = owners_reference_hyperlink_style(body);
        for (rel_id, is_header) in self.header_footer_rel_ids() {
            if !referenced && let Some(story) = self.load_header_footer(&rel_id, is_header) {
                referenced = owners_reference_hyperlink_style(
                    story
                        .paragraphs
                        .iter()
                        .map(WordXmlOwner::Paragraph)
                        .collect(),
                );
            }
        }
        if !referenced {
            referenced = owners_reference_hyperlink_style(
                self.footnotes
                    .footnotes
                    .iter()
                    .flat_map(|note| &note.paragraphs)
                    .map(WordXmlOwner::Paragraph)
                    .collect(),
            );
        }
        if !referenced
            && let Ok(Some(part)) = self.note_part_name(StoryKind::Endnote)
            && let Some(source) = self.package.get_part(&part)
            && let Ok(notes) = oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(source)
        {
            referenced = owners_reference_hyperlink_style(
                notes
                    .footnotes
                    .iter()
                    .flat_map(|note| &note.paragraphs)
                    .map(WordXmlOwner::Paragraph)
                    .collect(),
            );
        }
        if referenced {
            self.add_style(
                StyleBuilder::character("Hyperlink", "Hyperlink").run_properties(CT_RPr {
                    color: Some("0563C1".into()),
                    underline: Some(oxoffice_docx_oxml::shared::ST_Underline::Single),
                    ..Default::default()
                }),
            )?;
        }
        Ok(())
    }

    pub(crate) fn prepare_staged_package(&mut self) -> Result<bool> {
        self.prepare_staged_package_in(None)
    }

    pub(crate) fn prepare_staged_template_package<'a>(
        &mut self,
        operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'a>,
        lineage: &mut crate::template::TemplateWordLineage<'a>,
        sources: &mut TemplateMainSources,
    ) -> Result<bool> {
        let result = self.prepare_staged_package_in(Some((operation, lineage, sources)));
        lineage.finish_preparation();
        result
    }

    fn prepare_staged_package_in<'a>(
        &mut self,
        mut template: Option<(
            &mut oxoffice_docx_oxml::document::ScopedWordOperation<'a>,
            &mut crate::template::TemplateWordLineage<'a>,
            &mut TemplateMainSources,
        )>,
    ) -> Result<bool> {
        let normalized = self.prepare_current_field_models(template.as_mut().map(
            |(operation, lineage, sources)| {
                (
                    &mut **operation,
                    &mut **lineage,
                    sources.reader_source.as_deref(),
                    sources.comments_reader_source.as_deref(),
                )
            },
        ))?;
        if let Some((_, lineage, _)) = &mut template {
            lineage.finish_preparation();
        }
        self.ensure_referenced_hyperlink_style()?;
        self.preflight_flush_required_bundles()?;
        if let Some((operation, lineage, sources)) = template.as_mut() {
            let mut writer = TemplateMainWriter {
                operation,
                lineage,
                sources,
            };
            self.canonicalize_typed_identifiers(Some(&mut writer))?;
        } else {
            self.canonicalize_typed_identifiers(None)?;
        }
        let retained_signature_invalidated =
            self.retained_package_signature_would_be_invalidated()?;
        if let Some((operation, lineage, sources)) = template.as_mut() {
            let mut writer = TemplateMainWriter {
                operation,
                lineage,
                sources,
            };
            self.flush_dirty_related_story_models_in(Some(&mut writer))?;
        } else {
            self.flush_dirty_related_story_models()?;
        }
        if let Some((operation, lineage, sources)) = template.as_mut() {
            let mut writer = TemplateMainWriter {
                operation,
                lineage,
                sources,
            };
            self.canonicalize_drawing_ids(Some(&mut writer))?;
        } else {
            self.canonicalize_drawing_ids(None)?;
        }
        if let Some((operation, lineage, sources)) = template.as_mut() {
            let mut writer = TemplateMainWriter {
                operation,
                lineage,
                sources,
            };
            self.refresh_related_story_caches_in(Some(&mut writer))?;
        } else {
            self.refresh_related_story_caches()?;
        }
        self.package_signatures_invalidated |= retained_signature_invalidated;
        let result = if let Some((operation, lineage, sources)) = template {
            let mut writer = TemplateMainWriter {
                operation,
                lineage,
                sources,
            };
            self.flush_to_package_in(Some(&mut writer))
        } else {
            self.flush_to_package()
        };
        result.map(|()| normalized)
    }

    /// Stage every public output. A comments model that no longer matches its
    /// part is written back even when no mutation marked it dirty. An
    /// unchanged comments part keeps its producer bytes, as styles and the
    /// other modelled parts do, so a save without a comment edit neither
    /// rewrites it nor invalidates a package signature over it.
    pub(crate) fn prepare_staged_output(&mut self) -> Result<bool> {
        if let (Some(comments), Some(part_name)) = (&self.comments, &self.comments_part_name) {
            self.comments_dirty |= self
                .package
                .get_part(part_name)
                .and_then(|xml| oxoffice_docx_oxml::comments::CT_Comments::from_xml(xml).ok())
                .as_ref()
                != Some(comments);
        }
        self.prepare_staged_package()
    }

    pub(crate) fn prepare_and_reopen_staged(mut self) -> Result<Self> {
        self.prepare_staged_package()?;
        self.reopen_prepared_staged()
    }

    pub(crate) fn reopen_prepared_staged(self) -> Result<Self> {
        self.reopen_prepared_staged_with_limits(PackageReadLimits::UNBOUNDED)
    }

    pub(crate) fn reopen_prepared_staged_with_limits(
        self,
        limits: PackageReadLimits,
    ) -> Result<Self> {
        let in_memory = limits == PackageReadLimits::UNBOUNDED && self.read_package().is_some();
        #[cfg(debug_assertions)]
        let through_zip = in_memory.then(|| self.clone_for_debug_reference());
        let reopened = self.reopen_staged(in_memory, limits);
        #[cfg(debug_assertions)]
        if let Some(copy) = through_zip {
            let expected = copy.reopen_staged(false, limits);
            assert_same_reopen(&reopened, &expected);
        }
        reopened
    }

    fn install_carried_body_scans(&self, mut carried: MainBodyScans) {
        // Writer eligibility is checked against the current model and frame
        // when offered. It is independent of the stricter address-scan carry.
        // A changed owner span must not discard that current writer memo.
        {
            let mut scans = self
                .main_body_scans
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // The opening may already have qualified a newer writer record.
            if scans.serialized.is_none() {
                scans.serialized = carried.serialized.clone();
            }
        }
        if carried.frame.is_none() {
            return;
        }
        let Ok((xml, content, children)) = self.main_part_with_body_spans() else {
            return;
        };
        if carried.head.as_slice() != &xml[..content.start]
            || carried.tail.as_slice() != &xml[content.end..]
        {
            return;
        }
        if carried.blocks.len() != children.len()
            || carried
                .blocks
                .iter()
                .zip(&children)
                .any(|(scan, range)| scan.bytes.as_slice() != &xml[range.clone()])
        {
            carried.fingerprint = None;
        }
        // main_story_layout checks the current frame and each aligned child's
        // exact bytes and item coverage before it consumes these read facts.
        // Keep the newly qualified writer memo independently of those facts.
        let mut scans = self
            .main_body_scans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        carried.serialized = scans.serialized.take();
        *scans = carried;
    }

    fn reopen_staged(self, in_memory: bool, limits: PackageReadLimits) -> Result<Self> {
        let carried_scans = self
            .main_body_scans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let carried_blocks = self.main_part_blocks.clone();
        let provenance = self.identifiers.clone();
        let custom_properties_owned = self.custom_properties_owned;
        let settings_owned = self.settings_owned;
        let owned_settings = settings_owned.then(|| self.settings.clone());
        let web_settings_owned = self.web_settings_owned;
        let owned_web_settings = web_settings_owned.then(|| self.web_settings.clone());
        let comments_owned = self.comments_owned;
        let comments_extended_owned = self.comments_extended_owned;
        let embedded_invalidated_signatures = self.embedded_invalidated_signatures.clone();
        let package_signatures_invalidated = self.package_signatures_invalidated;
        let source_path = self.source_path.clone();
        let opened = self.parsed.as_deref().filter(|_| in_memory);
        let mut reopened = match opened {
            Some(opened) => Self::from_package_reusing(
                self.package.reread(&opened.package)?,
                true,
                Some((&provenance, opened)),
                carried_blocks.as_ref(),
                carried_scans.serialized.as_ref(),
                self.styles_part_cache.as_ref(),
                None,
            )?,
            None => {
                let mut output = std::io::Cursor::new(Vec::new());
                self.package.write_to(&mut output)?;
                Self::from_bytes_with_limits(output.get_ref(), limits)?
            }
        };
        reopened
            .identifiers
            .reject_new_repeated_drawing_ids(&provenance)?;
        reopened
            .identifiers
            .reject_new_repeated_bookmark_ids(&provenance)?;
        reopened.identifiers.reconcile_provenance(&provenance);
        reopened.restore_live_numbering(
            &self.published_abstract_by_live,
            &self.published_instance_by_live,
            &provenance,
        )?;
        reopened.custom_properties_owned = custom_properties_owned;
        reopened.settings_owned = settings_owned;
        if let Some(settings) = owned_settings {
            reopened.settings = settings;
        }
        reopened.web_settings_owned = web_settings_owned;
        if let Some(web_settings) = owned_web_settings {
            reopened.web_settings = web_settings;
        }
        reopened.comments_owned = comments_owned;
        reopened.comments_extended_owned = comments_extended_owned;
        reopened.embedded_invalidated_signatures = embedded_invalidated_signatures;
        reopened.package_signatures_invalidated = package_signatures_invalidated;
        reopened.source_path = source_path;
        reopened.install_carried_body_scans(carried_scans);
        reopened.install_carried_blocks(carried_blocks);
        Ok(reopened)
    }

    fn restore_live_numbering(
        &mut self,
        abstract_map: &HashMap<u32, u32>,
        instance_map: &HashMap<u32, u32>,
        provenance: &DocumentIdentifiers,
    ) -> Result<()> {
        if abstract_map.is_empty() && instance_map.is_empty() {
            return Ok(());
        }
        let numbering = self
            .numbering
            .as_mut()
            .ok_or_else(|| Error::Other("published numbering model is missing".into()))?;
        for published in abstract_map.values() {
            if !numbering
                .abstract_nums
                .iter()
                .any(|definition| definition.abstract_num_id == *published)
            {
                return Err(Error::Other(
                    "published abstract numbering ID is missing".into(),
                ));
            }
        }
        for published in instance_map.values() {
            if !numbering
                .nums
                .iter()
                .any(|instance| instance.num_id == *published)
            {
                return Err(Error::Other(
                    "published numbering instance ID is missing".into(),
                ));
            }
        }
        let abstract_inverse = abstract_map
            .iter()
            .map(|(live, published)| (*published, *live))
            .collect::<HashMap<_, _>>();
        let instance_inverse = instance_map
            .iter()
            .map(|(live, published)| (*published, *live))
            .collect::<HashMap<_, _>>();
        let published_ids = numbering
            .nums
            .iter()
            .map(|instance| instance.num_id)
            .collect::<HashSet<_>>();
        let mut missing_body_id = false;
        visit_body_paragraphs_mut(&mut self.document.body.content, &mut |paragraph| {
            if let Some(id) = paragraph
                .properties
                .as_mut()
                .and_then(|properties| properties.num_id.as_mut())
                && let Some(live) = instance_inverse.get(id)
            {
                missing_body_id |= !published_ids.contains(id);
                *id = *live;
            }
        });
        if missing_body_id {
            return Err(Error::Other(
                "published body numbering ID is missing".into(),
            ));
        }
        for definition in &mut numbering.abstract_nums {
            if let Some(live) = abstract_inverse.get(&definition.abstract_num_id) {
                definition.abstract_num_id = *live;
            }
        }
        for instance in &mut numbering.nums {
            if let Some(live) = instance_inverse.get(&instance.num_id) {
                instance.num_id = *live;
            }
            if let Some(live) = abstract_inverse.get(&instance.abstract_num_id) {
                instance.abstract_num_id = *live;
            }
        }
        self.identifiers.abstract_numbering_ids = numbering
            .abstract_nums
            .iter()
            .map(|definition| definition.abstract_num_id)
            .collect();
        self.identifiers.numbering_instance_ids = numbering
            .nums
            .iter()
            .map(|instance| instance.num_id)
            .collect();
        self.identifiers.preserved_abstract_numbering_ids =
            provenance.preserved_abstract_numbering_ids.clone();
        self.identifiers.preserved_numbering_instance_ids =
            provenance.preserved_numbering_instance_ids.clone();
        self.published_abstract_by_live = abstract_map.clone();
        self.published_instance_by_live = instance_map.clone();
        Ok(())
    }

    /// Compare current writer bytes without constructing an owned result when
    /// the existing writer memo proves every current child and the full frame.
    fn serialized_main_part_matches(&self, expected: &[u8]) -> Result<bool> {
        if block_proofs_enabled() {
            let scans = self
                .main_body_scans
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(matched) = scans
                .serialized
                .as_ref()
                .and_then(|record| record.writer_bytes_match(&self.document, expected))
            {
                return Ok(matched);
            }
        }
        self.serialized_main_part().map(|(xml, _)| xml == expected)
    }

    /// Current writer bytes and pieces, without claiming a flushed part.
    fn serialized_main_part(&self) -> Result<(Vec<u8>, Option<MainPartBlocks>)> {
        self.serialized_main_part_in(None)
    }

    fn serialized_main_part_in(
        &self,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<(Vec<u8>, Option<MainPartBlocks>)> {
        self.document.validate_resource_limits()?;
        let mut scans = self
            .main_body_scans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(record) = offered_canonical_blocks(scans.serialized.as_ref())
            && let Some(kept) = self.kept_main_part()
            && let Some(expected) = kept.serialization_proof.as_deref()
            && record.writer_bytes_match(&self.document, expected) == Some(true)
        {
            // These are an opening's normalized writer bytes, not its raw
            // source. Requalify the complete current writer before copying.
            let xml = if let Some(template) = &mut template
                && let Some(records) = template.lineage.records
                && let Some(source) = records.source_for_part(template.operation, expected)?
            {
                let copied = template.operation.repeat_source(&source)?;
                let bytes = template.operation.finish_source(copied)?;
                template.sources.source = Some(Arc::new(source));
                bytes
            } else {
                if let Some(template) = &mut template {
                    template.sources.source = None;
                }
                expected.to_vec()
            };
            let mut record = record.clone();
            record.part = None;
            drop(scans);
            return Ok((xml, Some(record)));
        }
        let (xml, record) = MainPartBlocks::serialize_in(
            &self.document,
            &self.document.body.content,
            offered_canonical_blocks(scans.serialized.as_ref().or(self.main_part_blocks.as_ref())),
            template,
        )?;
        if offered_canonical_blocks(Some(&record)).is_some() {
            scans.serialized = (!record.head.is_empty()).then(|| record.clone());
        }
        drop(scans);
        Ok((xml, (!record.head.is_empty()).then_some(record)))
    }

    /// Temporary main-part bytes for canonicalization. The returned record
    /// proves these bytes, without replacing the package or flushed record.
    fn canonical_source(&self) -> Result<(Vec<u8>, Option<MainPartBlocks>)> {
        self.canonical_source_in(None)
    }

    fn canonical_source_in(
        &self,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<(Vec<u8>, Option<MainPartBlocks>)> {
        let (xml, mut record) = self.serialized_main_part_in(template.as_deref_mut())?;
        if let Some(record) = &mut record {
            let part = if let Some(template) = &mut template
                && let Some(source) = &template.sources.source
            {
                let copied = template.operation.repeat_source(source)?;
                template.operation.finish_source(copied)?
            } else {
                xml.clone()
            };
            record.part = Some(Arc::new(part));
        }
        Ok((xml, record))
    }

    fn prepared_main_read(&self) -> Result<PreparedMainRead<'_>> {
        let (xml, writer) = self.canonical_source()?;
        let (xml, content, children) = match writer.as_ref() {
            Some(record) => {
                let (content, children) = record.body_spans()?;
                (xml, content, children)
            }
            None => {
                let (xml, content, children) = self.document.to_xml_with_body_spans()?;
                (xml, content, children)
            }
        };
        Ok(PreparedMainRead {
            document: self,
            xml,
            writer,
            content,
            children,
        })
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn main_part_with_body_spans(
        &self,
    ) -> Result<(Vec<u8>, Range<usize>, Vec<Range<usize>>)> {
        let (xml, record) = self.serialized_main_part()?;
        if let Some(record) = record {
            let (content, children) = record.body_spans()?;
            let start = content.start;
            let end = content.end;
            #[cfg(debug_assertions)]
            {
                // The canonical spans must point at the exact canonical
                // blocks. This holds for every child, including a direct
                // paragraph whose root keeps a foreign `w` binding.
                for (range, block) in children.iter().zip(record.blocks.iter()) {
                    assert_eq!(
                        xml.get(range.clone()),
                        Some(block.bytes.as_slice()),
                        "serialized pieces changed the body spans"
                    );
                }
                // `to_xml_with_body_spans` is a public call and stays
                // Publication. The canonical Source writer emits a direct
                // paragraph with a root-local foreign `w` binding
                // differently, so compare the public bytes of every other
                // child against its canonical block.
                if let Ok((public, _, public_children)) = self.document.to_xml_with_body_spans() {
                    assert_eq!(
                        public_children.len(),
                        children.len(),
                        "serialized pieces changed the body spans"
                    );
                    for (public_range, block) in public_children.iter().zip(record.blocks.iter()) {
                        if !root_declares_foreign_word(&block.bytes) {
                            assert_eq!(
                                public.get(public_range.clone()),
                                Some(block.bytes.as_slice()),
                                "serialized pieces changed the body spans"
                            );
                        }
                    }
                }
            }
            return Ok((xml, start..end, children));
        }
        self.document.to_xml_with_body_spans().map_err(Error::from)
    }

    /// Reconstruct a source edit from actual parsed child facts. A changed
    /// frame, an unclosed element or an unproved child declines and
    /// leaves the whole parser authoritative.
    fn source_blocks(&self, xml: &Arc<Vec<u8>>) -> Option<(CT_Document, MainPartBlocks)> {
        if !block_proofs_enabled() {
            return None;
        }
        if let Some(record) = &self.main_part_blocks
            && let Some(current) = &record.part
            && self.package.get_part(&self.doc_part_name) == Some(current.as_slice())
            && let Some(found) = self.source_blocks_in(current, record.clone(), xml)
        {
            return Some(found);
        }
        let (mut current, record) = self.canonical_source().ok()?;
        let record = record?;
        if let Some(found) = self.source_blocks_in(&current, record.clone(), xml) {
            return Some(found);
        }
        if let Some(kept) = self.kept_main_part() {
            let source = kept.main_part_xml()?;
            if let Some(producer) = kept
                .source_blocks
                .get_or_init(|| self.producer_source_blocks(source, &record))
                && let Some(found) = self.source_blocks_in(source, producer.clone(), xml)
            {
                return Some(found);
            }
        }
        // Bootstrap a producer's body-leading whitespace under the exact
        // canonical root and body start. Root aliases and other frame changes
        // still require the whole reader. The empty frame is actually parsed
        // and its normalized output must match the proved canonical shell.
        if !xml.starts_with(&record.head) {
            return None;
        }
        let after_head = xml.get(record.head.len()..)?;
        let whitespace = after_head
            .iter()
            .take_while(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
            .count();
        if whitespace == 0 {
            return None;
        }
        let gap = after_head.get(..whitespace)?;
        let mut head = record.head.clone();
        head.extend_from_slice(gap);
        let shell =
            CT_Document::from_xml(&[head.as_slice(), record.tail.as_slice()].concat()).ok()?;
        let canonical_shell = record.frame.get()?.as_ref()?;
        if !shell.body.content.is_empty()
            || shell.to_xml().ok()? != canonical_shell.to_xml().ok()?
        {
            return None;
        }
        current.splice(record.head.len()..record.head.len(), gap.iter().copied());
        let retained = MainPartBlocks {
            head,
            root: record.root,
            tail: record.tail,
            blocks: Arc::new(
                record
                    .blocks
                    .iter()
                    .map(BodyBlockScan::fresh_context_piece)
                    .collect(),
            ),
            part: Some(Arc::new(current.clone())),
            frame: OnceLock::from(Some(shell)),
            nested_namespace_owners: OnceLock::new(),
            identifier_frame: OnceLock::new(),
            canonical_frame: OnceLock::new(),
            namespace_frame: OnceLock::new(),
            namespace_frame_spans: OnceLock::new(),
        };
        self.source_blocks_in(&current, retained, xml)
    }

    /// Partition a producer frame using physical owner ranges and actual
    /// one-child reads. These facts do not certify canonical writer bytes.
    fn producer_source_blocks(
        &self,
        source: &[u8],
        normal: &MainPartBlocks,
    ) -> Option<MainPartBlocks> {
        let owners = scan_story_owners(source, StoryKind::Body).ok()?;
        let body = single_frame_body(&owners, None).ok()?;
        let body_start = source.get(body.full.start..)?;
        if !body_start.starts_with(b"<w:body>") && !body_start.starts_with(b"<w:body ") {
            return None;
        }
        let items = direct_story_content_items(source, body).ok()?;
        let first = items.first()?.full.start;
        let end = items.last()?.full.end;
        let head = source.get(..first)?.to_vec();
        let tail = source.get(end..)?.to_vec();
        let mut root = root_start_tag(&head)?;
        root.start += head
            .get(root.clone())?
            .iter()
            .take_while(|b| b.is_ascii_whitespace())
            .count();
        let tag = head.get(root.clone())?;
        if !tag.starts_with(b"<w:document ") {
            return None;
        }
        let frame = CT_Document::from_xml(&[head.as_slice(), tail.as_slice()].concat()).ok()?;
        if !frame.body.content.is_empty()
            || frame.to_xml().ok()? != normal.frame.get()?.as_ref()?.to_xml().ok()?
        {
            return None;
        }
        let scopes = document_namespace_scopes(source).ok()?;
        if !scopes.body_declarations.is_empty()
            || scopes
                .root_declarations
                .iter()
                .any(|(name, _)| name == "xmlns")
        {
            return None;
        }
        let mut blocks = Vec::with_capacity(items.len());
        let mut at = first;
        for item in items {
            if !matches!(
                item.kind,
                StoryItemKind::Paragraph | StoryItemKind::Table | StoryItemKind::ContentControl
            ) || item.full.start < at
                || !source
                    .get(at..item.full.start)?
                    .iter()
                    .all(u8::is_ascii_whitespace)
            {
                return None;
            }
            let mut block = BodyBlockScan::from_source(source.get(at..item.full.end)?.to_vec());
            let child = block.read_source_child(&head, &tail, &scopes)?;
            if block.holds_field_words || !child.owner_facts_reusable_by_value() {
                return None;
            }
            // The proof above is an actual read under the producer scope.
            // Canonical serialization remains a separate authority.
            block.standalone_value = OnceLock::new();
            block.staged = None;
            blocks.push(Arc::new(block));
            at = item.full.end;
        }
        let record = MainPartBlocks {
            head,
            root,
            tail,
            blocks: Arc::new(blocks),
            part: Some(Arc::new(source.to_vec())),
            frame: OnceLock::from(Some(frame)),
            nested_namespace_owners: OnceLock::new(),
            identifier_frame: OnceLock::new(),
            canonical_frame: OnceLock::new(),
            namespace_frame: OnceLock::new(),
            namespace_frame_spans: OnceLock::new(),
        };
        // Complete actual source equality is read authority only.
        let source_matches = record.proved_document_matches(source, &self.document)?;
        if record.blocks.len() != normal.blocks.len() {
            return None;
        }
        // Preserve the original fallible frame/resource preflight before either route.
        let context = scoped_normalization_context(&self.document, &record, normal)?;
        if source_matches {
            let lexical_and_private = record
                .blocks
                .iter()
                .zip(normal.blocks.iter())
                .zip(&self.document.body.content)
                .all(|((left, right), item)| {
                    scoped_normalization_reuse_allowed(&record, normal, left, right, item)
                });
            if lexical_and_private && let Some(actual_normal_part) = &normal.part {
                let writer_matches = normal.writer_bytes_match(&self.document, actual_normal_part);
                if writer_matches == Some(true) {
                    // No scoped correspondence memo is created by this different proof.
                    return Some(record);
                }
            }
        }
        for children in record.blocks.iter().zip(normal.blocks.iter()) {
            let seed = producer_scoped_normalization_seed(
                &record, normal, children.0, children.1, &scopes,
            );
            if !scoped_children_correspond(
                &self.document,
                &record,
                normal,
                children,
                &context,
                None,
                seed,
            ) {
                return None;
            }
        }
        Some(record)
    }

    /// Rebuild one exact source partition. Kept children supply actual parse
    /// facts, and replacing children are read under this same source frame.
    fn source_blocks_in(
        &self,
        current: &[u8],
        mut record: MainPartBlocks,
        xml: &Arc<Vec<u8>>,
    ) -> Option<(CT_Document, MainPartBlocks)> {
        let mut parsed = record.proved_document(current)?;
        let head = record.head.len();
        let content_end = current.len().checked_sub(record.tail.len())?;
        let new_content_end = xml.len().checked_sub(record.tail.len())?;
        let same = current
            .iter()
            .zip(xml.iter())
            .take_while(|(a, b)| a == b)
            .count()
            .min(content_end)
            .min(new_content_end);
        if same < head {
            return None;
        }
        let suffix = current
            .get(same..)?
            .iter()
            .rev()
            .zip(xml.get(same..)?.iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        if suffix < record.tail.len() {
            return None;
        }
        let mut boundaries = Vec::with_capacity(record.blocks.len() + 1);
        boundaries.push(head);
        for block in record.blocks.iter() {
            boundaries.push(boundaries.last()?.checked_add(block.bytes.len())?);
        }
        if boundaries.last().copied()? != content_end {
            return None;
        }
        let first = boundaries[1..].partition_point(|end| *end <= same);
        let changed_end = current.len().checked_sub(suffix)?;
        let last = boundaries[..record.blocks.len()]
            .partition_point(|start| *start < changed_end)
            .max(first);
        let start = *boundaries.get(first)?;
        let end = *boundaries.get(last)?;
        let new_end = xml.len().checked_sub(current.len().checked_sub(end)?)?;
        let replacing = xml.get(start..new_end)?;
        let scopes = document_namespace_scopes(current).ok()?;
        let mut reader = quick_xml::Reader::from_reader(replacing);
        let mut buffer = Vec::new();
        let mut depth = 0usize;
        let mut element = 0usize;
        let mut ranges = Vec::new();
        loop {
            let at = reader.buffer_position() as usize;
            let event = reader.read_event_into(&mut buffer).ok()?;
            let after = reader.buffer_position() as usize;
            match event {
                Event::Eof if depth == 0 => break,
                Event::Eof => return None,
                Event::Start(_) => {
                    if depth == 0 {
                        element = at;
                    }
                    depth = depth.checked_add(1)?;
                }
                Event::End(_) => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        ranges.push(element..after);
                    }
                }
                Event::Empty(_) if depth == 0 => ranges.push(at..after),
                _ if depth == 0 => return None,
                _ => {}
            }
            buffer.clear();
        }
        let mut items = Vec::with_capacity(ranges.len());
        let mut replacement_blocks = Vec::with_capacity(ranges.len());
        for range in ranges {
            let bytes = replacing.get(range)?.to_vec();
            let mut block = BodyBlockScan::from_source(bytes);
            let item = block.read_source_child(&record.head, &record.tail, &scopes)?;
            items.push(item);
            replacement_blocks.push(Arc::new(block));
        }
        parsed.body.content.splice(first..last, items);
        parsed.validate_resource_limits().ok()?;
        Arc::make_mut(&mut record.blocks).splice(first..last, replacement_blocks);
        record.part = Some(Arc::clone(xml));
        record.nested_namespace_owners = OnceLock::new();
        if !record.part_partition_matches(xml) {
            return None;
        }
        Some((parsed, record))
    }

    fn package_main_part_blocks(&self) -> Option<MainPartBlocks> {
        if !block_proofs_enabled() {
            return None;
        }
        let source = self.package.get_part(&self.doc_part_name)?;
        if let Some(record) = &self.main_part_blocks
            && record
                .part
                .as_deref()
                .is_some_and(|bytes| bytes.as_slice() == source)
        {
            return Some(record.clone());
        }
        self.source_blocks(&Arc::new(source.to_vec()))
            .map(|(_, record)| record)
    }

    fn main_part_parses_to_model(&self, kept: Option<&Parsed>) -> bool {
        let Some(source) = self.package.get_part(&self.doc_part_name) else {
            return false;
        };
        if let Some(parsed) = offered_canonical_blocks(self.main_part_blocks.as_ref())
            .and_then(|record| record.proved_document_matches(source, &self.document))
        {
            return parsed;
        }
        if let Some(parsed) = kept
            .filter(|kept| {
                kept.main_part == self.doc_part_name && kept.main_part_xml() == Some(source)
            })
            .and_then(Parsed::main_document)
        {
            return parsed == &*self.document;
        }
        CT_Document::from_xml(source).ok().as_ref() == Some(&*self.document)
    }

    /// Same temporary record for a document that is not `self.document`.
    /// `kept` supplies neighbor bytes only. It is not stored as history.
    fn canonical_source_from(
        document: &CT_Document,
        kept: Option<&MainPartBlocks>,
    ) -> Result<(Vec<u8>, Option<MainPartBlocks>)> {
        document.validate_resource_limits()?;
        let (xml, mut record) = MainPartBlocks::serialize(
            document,
            &document.body.content,
            offered_canonical_blocks(kept),
        )?;
        record.part = Some(Arc::new(xml.clone()));
        Ok((xml, (!record.head.is_empty()).then_some(record)))
    }

    /// The main part bytes from the current block record, or the whole writer
    /// when the frame or a child cannot be proved. A writer error is returned.
    /// It is not an empty part.
    fn main_part_xml(&mut self) -> Result<Vec<u8>> {
        self.main_part_xml_in(None)
    }

    fn main_part_xml_in(
        &mut self,
        template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<Vec<u8>> {
        let (xml, record) = self.canonical_source_in(template)?;
        if let Some(record) = &record {
            let _ = record.proved_document_matches(&xml, &self.document);
        }
        self.main_part_blocks = record;
        Ok(xml)
    }

    fn install_carried_blocks(&mut self, carried: Option<MainPartBlocks>) {
        let Some(carried) = carried else {
            return;
        };
        let Some(part) = carried.part.as_deref() else {
            return;
        };
        let Some(current) = self.package.get_part(&self.doc_part_name) else {
            return;
        };
        if current != part.as_slice() {
            return;
        }
        if self.document.validate_resource_limits().is_ok()
            && self
                .document
                .body
                .content
                .iter()
                .all(BodyContent::owner_facts_reusable_by_value)
            && carried.blocks.iter().all(|block| !block.reads_anew())
            && carried.proved_document_matches(current, &self.document) == Some(true)
            && self.opened_main_still_serializes()
        {
            self.main_part_blocks = Some(carried);
            return;
        }
        let Ok(xml) = self.document.to_xml() else {
            return;
        };
        // A retained source child may normalize on write. Its actual parse
        // must normalize to this reopened model before its read facts carry.
        let Some(parsed) = carried.proved_document(current) else {
            return;
        };
        if parsed.to_xml().ok().as_ref() != Some(&xml) {
            return;
        }
        self.main_part_blocks = Some(carried);
    }

    fn preflight_flush_required_bundles(&mut self) -> Result<()> {
        self.reserve_styles_bundle()?;
        if self.numbering.is_some() {
            self.reserve_numbering_bundle()?;
        }
        if self.footnotes_dirty && !self.footnotes.footnotes.is_empty() {
            self.reserve_footnotes_bundle()?;
        }
        if let Some(part_name) = self.comments_part_name.clone()
            && self.comments.is_some()
        {
            self.ensure_part_relationship_checked(
                &part_name,
                rel_types::COMMENTS,
                crate::comments::COMMENTS_CONTENT_TYPE,
            )
            .map_err(|error| {
                Error::Other(format!("comments relationship allocation failed: {error}"))
            })?;
        }
        if let Some(part_name) = self.comments_extended_part_name.clone()
            && self.comments_extended.is_some()
        {
            self.ensure_part_relationship_checked(
                &part_name,
                crate::comments::COMMENTS_EXTENDED_REL_TYPE,
                crate::comments::COMMENTS_EXTENDED_CONTENT_TYPE,
            )
            .map_err(|error| {
                Error::Other(format!(
                    "comments-extended relationship allocation failed: {error}"
                ))
            })?;
        }
        if self.core_properties.is_some() {
            self.reserve_core_properties_bundle()?;
        }
        if self.application_properties.is_some() {
            self.reserve_application_properties_bundle()?;
        }
        if self.custom_properties.is_some() {
            self.reserve_custom_properties_bundle()?;
        }
        Ok(())
    }

    fn styles_differ_from_part(&mut self, part: &str) -> Result<bool> {
        let Some(bytes) = self.package.get_part_shared(part).cloned() else {
            return Ok(true);
        };
        if let Some(cache) = self
            .styles_part_cache
            .as_ref()
            .filter(|cache| Arc::ptr_eq(&bytes, &cache.bytes))
        {
            if cache.emitted_model.as_ref().is_some_and(|emitted| {
                Arc::ptr_eq(&self.styles, emitted) || self.styles == *emitted
            }) {
                // The exact writer already succeeded for this model and these
                // bytes. Keep its lexical form rather than adopting a reparse.
                return Ok(false);
            }
            if Arc::ptr_eq(&self.styles, &cache.parsed) || self.styles == cache.parsed {
                if matches!(cache.proof, StylesSerializationProof::Unserializable) {
                    self.styles.to_xml()?;
                }
                self.styles = Arc::clone(&cache.parsed);
                return Ok(false);
            }
            return Ok(true);
        }
        let parsed = Arc::new(CT_Styles::from_xml(&bytes)?);
        let proof = if parsed.to_xml().is_ok() {
            StylesSerializationProof::Serializable
        } else {
            StylesSerializationProof::Unserializable
        };
        if self.styles != parsed {
            return Ok(true);
        }
        if matches!(proof, StylesSerializationProof::Unserializable) {
            self.styles.to_xml()?;
        }
        self.styles = Arc::clone(&parsed);
        self.styles_part_cache = Some(StylesPartCache {
            bytes,
            parsed,
            emitted_model: None,
            proof,
        });
        Ok(false)
    }

    /// Restore live numbering immediately after writing its package projection.
    fn flush_numbering_emission(
        &mut self,
        template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<(HashMap<u32, u32>, HashMap<u32, u32>)> {
        let numbering = self.numbering.clone();
        let abstract_ids = self.identifiers.abstract_numbering_ids.clone();
        let instance_ids = self.identifiers.numbering_instance_ids.clone();
        let mut body_ids = Vec::new();
        visit_body_paragraphs(&self.document.body.content, false, &mut |paragraph| {
            body_ids.push(
                paragraph
                    .properties
                    .as_ref()
                    .and_then(|properties| properties.num_id),
            );
        });
        // Scoped template packages are private live-model staging inputs.
        // Their main Source and numbering definitions must share that space.
        // Final public publication still takes the ordinary canonical path.
        let (abstract_map, instance_map) = if template.is_some() {
            (HashMap::new(), HashMap::new())
        } else {
            self.canonicalize_numbering_ids()?
        };
        let result = (|| {
            self.flush_document_to_package_in(template)?;
            // Serialize numbering definitions if we have any
            if let Some(numbering_xml) = self
                .numbering
                .as_ref()
                .map(CT_Numbering::to_xml)
                .transpose()?
            {
                let numbering_part_name = self.numbering_part_name.clone();
                let numbering_part = self
                    .reserve_document_part_bundle(
                        numbering_part_name.as_deref(),
                        DEFAULT_NUMBERING_PART,
                        rel_types::NUMBERING,
                        NUMBERING_CONTENT_TYPE,
                    )
                    .map_err(|error| {
                        Error::Other(format!("numbering relationship allocation failed: {error}"))
                    })?;
                self.numbering_part_name = Some(numbering_part.clone());
                let numbering_changed = self
                    .package
                    .get_part(&numbering_part)
                    .and_then(|xml| CT_Numbering::from_xml(xml).ok())
                    .as_ref()
                    != self.numbering.as_ref();
                if numbering_changed {
                    self.package.set_part(&numbering_part, numbering_xml);
                }
            }

            Ok(())
        })();
        self.numbering = numbering;
        self.identifiers.abstract_numbering_ids = abstract_ids;
        self.identifiers.numbering_instance_ids = instance_ids;
        let mut body_ids = body_ids.into_iter();
        visit_body_paragraphs_mut(&mut self.document.body.content, &mut |paragraph| {
            let original = body_ids
                .next()
                .expect("publication preserves paragraph inventory");
            if let Some(properties) = &mut paragraph.properties {
                properties.num_id = original;
            }
        });
        result.map(|()| (abstract_map, instance_map))
    }

    /// Write the in-memory document/styles back into the OPC package parts.
    pub(crate) fn flush_to_package(&mut self) -> Result<()> {
        self.flush_to_package_in(None)
    }

    fn flush_to_package_in(
        &mut self,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        let (abstract_map, instance_map) =
            self.flush_numbering_emission(template.as_deref_mut())?;

        // Unchanged proved styles share their model and source bytes through staging.
        let styles_part_name = self.styles_part_name.clone();
        let styles_part = self
            .reserve_document_part_bundle(
                styles_part_name.as_deref(),
                DEFAULT_STYLES_PART,
                rel_types::STYLES,
                STYLES_CONTENT_TYPE,
            )
            .map_err(|error| {
                Error::Other(format!("styles relationship allocation failed: {error}"))
            })?;
        self.styles_part_name = Some(styles_part.clone());
        if self.styles_differ_from_part(&styles_part)? {
            let styles_xml = self.styles.to_xml()?;
            let styles_xml = if let Some(source) = self.package.get_part(&styles_part) {
                retain_styles_root_attributes(source, styles_xml)?
            } else {
                styles_xml
            };
            self.package.set_part(&styles_part, styles_xml);
            let bytes = self
                .package
                .get_part_shared(&styles_part)
                .expect("just installed styles bytes");
            // The cache promises the model an actual reopening would parse.
            // Authored properties can acquire retained namespace context on
            // parse, so the pre-publication model is not that proof.
            let parsed = Arc::new(CT_Styles::from_xml(bytes)?);
            let proof = if parsed.to_xml().is_ok() {
                StylesSerializationProof::Serializable
            } else {
                StylesSerializationProof::Unserializable
            };
            self.styles_part_cache = Some(StylesPartCache {
                bytes: Arc::clone(bytes),
                parsed,
                emitted_model: Some(Arc::clone(&self.styles)),
                proof,
            });
        }

        // Settings are exposed as a read-only projection. Parsed settings
        // retain their complete producer bytes and are written back only to
        // the relationship-resolved part they came from.
        if let (Some(settings), Some(part_name)) = (&self.settings, &self.settings_part_name) {
            let changed = self
                .package
                .get_part(part_name)
                .and_then(|xml| CT_Settings::from_xml(xml).ok())
                .as_ref()
                != Some(settings);
            if changed {
                self.package.set_part(part_name, settings.to_xml()?);
            }
        }

        if let (Some(web_settings), Some(part_name)) =
            (&self.web_settings, &self.web_settings_part_name)
        {
            let changed = self
                .package
                .get_part(part_name)
                .and_then(|xml| CT_WebSettings::from_xml(xml).ok())
                .as_ref()
                != Some(web_settings);
            if changed {
                self.package.set_part(part_name, web_settings.to_xml()?);
            }
        }

        if self.theme_dirty {
            let theme = self
                .theme
                .as_ref()
                .ok_or_else(|| Error::Other("dirty theme model is missing".to_owned()))?;
            let part_name = self
                .theme_part_name
                .as_ref()
                .ok_or_else(|| Error::Other("dirty theme part name is missing".to_owned()))?;
            self.package.set_part(
                part_name,
                theme.to_xml().map_err(|error| {
                    Error::Other(format!("theme serialization failed: {error}"))
                })?,
            );
        }

        if self.font_table_dirty {
            let table = self
                .font_table
                .as_ref()
                .ok_or_else(|| Error::Other("dirty font-table model is missing".to_owned()))?;
            let part_name = self
                .font_table_part_name
                .as_ref()
                .ok_or_else(|| Error::Other("dirty font-table part name is missing".to_owned()))?;
            self.package.set_part(part_name, table.to_xml()?);
        }

        self.flush_dirty_related_story_models_in(template)?;

        if let (Some(comments), Some(part_name)) = (
            &self.comments_extended,
            self.comments_extended_part_name.clone(),
        ) {
            let changed = self
                .package
                .get_part(&part_name)
                .and_then(|xml| {
                    oxoffice_docx_oxml::comments_extended::CT_CommentsEx::from_xml(xml).ok()
                })
                .as_ref()
                != Some(comments);
            if changed {
                self.package.set_part(&part_name, comments.to_xml()?);
            }
            self.ensure_part_relationship_checked(
                &part_name,
                crate::comments::COMMENTS_EXTENDED_REL_TYPE,
                crate::comments::COMMENTS_EXTENDED_CONTENT_TYPE,
            )
            .map_err(|error| {
                Error::Other(format!(
                    "comments-extended relationship allocation failed: {error}"
                ))
            })?;
        }

        if self.glossary_dirty {
            let glossary = self
                .glossary
                .as_ref()
                .ok_or_else(|| Error::Other("dirty glossary model is missing".to_owned()))?;
            let part_name = self
                .glossary_part_name
                .as_ref()
                .ok_or_else(|| Error::Other("dirty glossary part name is missing".to_owned()))?;
            self.package.set_part(part_name, glossary.to_xml()?);
        }

        // Serialize core properties to the package relationship's target.
        if let Some(core_xml) = self
            .core_properties
            .as_deref()
            .map(CoreProperties::to_xml)
            .transpose()?
        {
            let core_part = self.reserve_core_properties_bundle()?;
            let changed = self
                .package
                .get_part(&core_part)
                .and_then(|xml| CoreProperties::from_xml(xml).ok())
                .as_ref()
                != self.core_properties.as_deref();
            if changed {
                self.package.set_part(&core_part, core_xml);
            }
        }
        if let Some(application_xml) = self
            .application_properties
            .as_ref()
            .map(AppProperties::to_xml)
            .transpose()?
        {
            let application_part = self.reserve_application_properties_bundle()?;
            let changed = self
                .package
                .get_part(&application_part)
                .and_then(|xml| AppProperties::from_xml(xml).ok())
                .as_ref()
                != self.application_properties.as_ref();
            if changed {
                self.package.set_part(&application_part, application_xml);
            }
        }
        if let Some(custom_xml) = self
            .custom_properties
            .as_ref()
            .map(CustomProperties::to_xml)
            .transpose()?
        {
            let custom_part = self.reserve_custom_properties_bundle()?;
            let changed = self
                .package
                .get_part(&custom_part)
                .and_then(|xml| CustomProperties::from_xml(xml).ok())
                .as_ref()
                != self.custom_properties.as_ref();
            if changed {
                self.package.set_part(&custom_part, custom_xml);
            }
        }

        self.published_abstract_by_live = abstract_map;
        self.published_instance_by_live = instance_map;
        Ok(())
    }

    pub(crate) fn flush_dirty_related_story_models(&mut self) -> Result<()> {
        self.flush_dirty_related_story_models_in(None)
    }

    fn flush_dirty_related_story_models_in(
        &mut self,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        // Existing footnote XML remains the authority. Typed additions are
        // inserted into that root once so producer namespace declarations and
        // unmodelled root children never pass through the lossy typed model.
        if self.footnotes_dirty && !self.footnotes.footnotes.is_empty() {
            let footnotes_part_name = self.footnotes_part_name.clone();
            let footnotes_part = self
                .reserve_document_part_bundle(
                    footnotes_part_name.as_deref(),
                    "/word/footnotes.xml",
                    rel_types::FOOTNOTES,
                    "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml",
                )
                .map_err(|error| {
                    Error::Other(format!("footnotes relationship allocation failed: {error}"))
                })?;
            self.footnotes_part_name = Some(footnotes_part.clone());
            let mut emitted_part = None;
            let xml = if let Some(existing_xml) = self.package.get_part_shared(&footnotes_part) {
                let mut parent = if let Some(template) = template.as_deref_mut() {
                    template.take_related_source(&footnotes_part, existing_xml)?
                } else {
                    None
                };
                let existing = if let (Some(template), Some(source)) =
                    (template.as_deref_mut(), parent.as_ref())
                {
                    template.operation.source_notes(source)?
                } else {
                    oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(existing_xml)?
                };
                let mut additions = Vec::new();
                let mut addition_sources = Vec::new();
                let mut all_sources = parent.is_some();
                for (index, note) in self.footnotes.footnotes.iter().enumerate() {
                    if existing.footnotes.iter().any(|candidate| {
                        candidate.id == note.id && candidate.note_type == note.note_type
                    }) {
                        continue;
                    }
                    let source = if let Some(template) = template.as_deref_mut() {
                        let read = (index < template.sources.read_note_count)
                            .then_some(parent.as_deref())
                            .flatten();
                        serialized_note_fragment_in(
                            note,
                            StoryKind::Footnote,
                            index,
                            read,
                            template,
                        )?
                    } else {
                        None
                    };
                    if let (Some(template), Some(source)) = (template.as_deref_mut(), source) {
                        let copied = template.operation.repeat_source(&source)?;
                        additions.push(template.operation.finish_source(copied)?);
                        addition_sources.push(Arc::new(source));
                    } else {
                        all_sources = false;
                        additions.push(serialized_footnote_fragment(note)?);
                    }
                }
                if existing.footnotes.len() + additions.len() != self.footnotes.footnotes.len() {
                    return Err(Error::Other(
                        "typed footnote mutation is not an additive package update".to_owned(),
                    ));
                }
                if all_sources {
                    let template = template.as_deref_mut().expect("scoped additive notes");
                    let source = append_story_sources_to_root(
                        template.operation,
                        parent.take().expect("actual existing note Source"),
                        &addition_sources,
                        b"footnotes",
                        b"footnote",
                    )?;
                    let copied = template.operation.repeat_source(&source)?;
                    let bytes = template.operation.finish_source(copied)?;
                    emitted_part = Some(source);
                    bytes
                } else {
                    append_story_fragments_to_root(
                        existing_xml,
                        b"footnotes",
                        b"footnote",
                        &additions,
                    )?
                }
            } else if let Some(template) = template.as_deref_mut() {
                let mut texts = Vec::new();
                let mut fields = Vec::new();
                let mut raw = Vec::new();
                if !template.sources.notes_model_read {
                    for (index, note) in self.footnotes.footnotes.iter().enumerate() {
                        if let Some(inputs) = template.lineage.note_writer_inputs(index, note) {
                            texts.extend(inputs.texts);
                            fields.extend(inputs.fields);
                            raw.extend(inputs.raw);
                        }
                    }
                }
                let source = template.operation.source_model_notes(
                    &self.footnotes,
                    false,
                    &texts,
                    &fields,
                    &raw,
                    true,
                )?;
                let copied = template.operation.repeat_source(&source)?;
                let bytes = template.operation.finish_source(copied)?;
                emitted_part = Some(Arc::from(source));
                bytes
            } else {
                let mut footnotes = self.footnotes.clone();
                for footnote in &mut footnotes.footnotes {
                    for paragraph in &mut footnote.paragraphs {
                        close_typed_story_drawing_namespaces(paragraph)?;
                    }
                }
                footnotes.to_xml_footnotes()?
            };
            self.package.set_part(&footnotes_part, xml);
            if let (Some(template), Some(source)) = (template.as_deref_mut(), emitted_part) {
                template.sources.put_related_source(
                    &footnotes_part,
                    self.package
                        .get_part_shared(&footnotes_part)
                        .expect("installed note part"),
                    source,
                );
            }
            self.footnotes_dirty = false;
        }

        // An existing comments part is modelled and flushed to its resolved
        // relationship target. An absent part remains absent until the later
        // comment API creates one deliberately.
        if self.comments_dirty
            && let (Some(comments), Some(part_name)) =
                (&self.comments, self.comments_part_name.clone())
        {
            let emitted = if let Some(writer) = template.as_deref_mut() {
                let source = if let Some(source) = writer.sources.comments_reader_source.as_ref() {
                    writer
                        .operation
                        .source_model_comments_from_read(comments, source)?
                } else {
                    let inputs = writer
                        .lineage
                        .comments_writer_inputs(comments)
                        .unwrap_or_default();
                    writer.operation.source_model_comments(
                        comments,
                        &inputs.texts,
                        &inputs.fields,
                        &inputs.raw,
                    )?
                };
                let retained = writer.operation.repeat_source(&source)?;
                Some((writer.operation.finish_source(retained)?, source))
            } else {
                None
            };
            let (xml, source) = if let Some((bytes, source)) = emitted {
                (bytes, Some(source))
            } else {
                let mut comments = comments.clone();
                for comment in &mut comments.comments {
                    for paragraph in &mut comment.paragraphs {
                        close_typed_story_drawing_namespaces(paragraph)?;
                    }
                }
                (comments.to_xml()?, None)
            };
            self.package.set_part(&part_name, xml);
            if let (Some(writer), Some(source)) = (template, source) {
                writer.sources.comments_reader_source = None;
                writer.sources.put_related_source(
                    &part_name,
                    self.package
                        .get_part_shared(&part_name)
                        .expect("actual comments publication"),
                    source,
                );
            }
            self.comments_dirty = false;
            self.ensure_part_relationship_checked(
                &part_name,
                rel_types::COMMENTS,
                crate::comments::COMMENTS_CONTENT_TYPE,
            )
            .map_err(|error| {
                Error::Other(format!("comments relationship allocation failed: {error}"))
            })?;
        }
        Ok(())
    }

    fn flush_document_to_package(&mut self) -> Result<()> {
        self.flush_document_to_package_in(None)
    }

    fn flush_document_to_package_in(
        &mut self,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        // A read-only save with unsafe retained namespace shadows keeps the
        // producer's complete scopes and every retained raw subtree byte for
        // byte. A modified document fails closed when canonical serialization
        // could change the meaning or bytes of retained content.
        // A main part that still holds its opened bytes is reused only when
        // the namespace context and the current serialization match that open.
        let kept = self.kept_main_part();
        let existing_document_xml = self.package.get_part(&self.doc_part_name);
        let typed_document_is_unchanged = if kept.is_some() && self.opened_main_still_serializes() {
            #[cfg(debug_assertions)]
            {
                assert_kept_main_document_without_initializing(
                    kept.as_deref(),
                    existing_document_xml,
                    &self.document,
                );
            }
            true
        } else if let Some(parsed) = kept.as_deref().and_then(Parsed::main_document) {
            #[cfg(debug_assertions)]
            {
                let fresh = existing_document_xml.and_then(|xml| CT_Document::from_xml(xml).ok());
                assert_eq!(
                    fresh.as_ref(),
                    Some(parsed),
                    "the retained source parse disagrees with a fresh parse"
                );
            }
            parsed == &*self.document
        } else if let Some(proved) = existing_document_xml.and_then(|xml| {
            self.main_part_blocks
                .as_ref()
                .and_then(|blocks| blocks.proved_document_matches(xml, &self.document))
        }) {
            #[cfg(debug_assertions)]
            {
                let fresh = existing_document_xml.and_then(|xml| CT_Document::from_xml(xml).ok());
                assert_eq!(
                    fresh.as_ref() == Some(&*self.document),
                    proved,
                    "the proved existing part disagrees with a fresh parse"
                );
            }
            proved
        } else {
            let parsed = existing_document_xml.and_then(|xml| CT_Document::from_xml(xml).ok());
            let unchanged = parsed.as_ref() == Some(&*self.document);
            drop(parsed);
            unchanged
        };
        if typed_document_is_unchanged
            && let Some(template) = &mut template
            && template.sources.source.is_none()
            && let Some((part, length)) = template.sources.original_part.take()
        {
            // This branch actually keeps the first staging clone's ingress
            // part. Later generated or refreshed parts cannot enter it.
            template.sources.source = Some(Arc::new(template.operation.source_part(
                &part,
                0..length,
                &BTreeMap::new(),
            )?));
        }
        let scanned;
        let nested_namespace_owners =
            match kept.as_deref().and_then(Parsed::nested_namespace_owners) {
                Some(owners) => {
                    #[cfg(debug_assertions)]
                    {
                        assert_eq!(
                            format!("{owners:?}"),
                            format!(
                                "{:?}",
                                existing_document_xml
                                    .map(nested_modeled_namespace_owners)
                                    .transpose()
                                    .ok()
                                    .flatten()
                                    .unwrap_or_default()
                            ),
                            "the kept nested namespace owners differ from a scan"
                        );
                    }
                    owners
                }
                None => {
                    let proved_owners = existing_document_xml.and_then(|xml| {
                        self.main_part_blocks
                            .as_ref()
                            .and_then(|blocks| blocks.retained_nested_owners(xml))
                    });
                    if let Some(owners) = proved_owners {
                        #[cfg(debug_assertions)]
                        {
                            let fresh = existing_document_xml
                                .map(nested_modeled_namespace_owners)
                                .transpose()
                                .ok()
                                .flatten()
                                .unwrap_or_default();
                            assert_eq!(
                                format!("{owners:?}"),
                                format!("{fresh:?}"),
                                "the proved nested owners differ from a scan"
                            );
                        }
                        scanned = owners;
                        &scanned
                    } else {
                        // `nested_modeled_namespace_owners` notes this scan itself.
                        scanned = existing_document_xml
                            .map(nested_modeled_namespace_owners)
                            .transpose()?
                            .unwrap_or_default();
                        &scanned
                    }
                }
            };
        let unsafe_prefix = unsafe_serializer_namespace_prefix(
            &self.root_namespace_declarations,
            &self.body_namespace_declarations,
            existing_document_xml,
        )
        .or_else(|| unsafe_nested_namespace_prefix(nested_namespace_owners));
        let kept_xml = existing_document_xml.map(<[u8]>::to_vec);
        let mut replayed_output_has_retained_namespace_owner = false;
        let doc_xml = if typed_document_is_unchanged {
            None
        } else {
            if let Some(prefix) = unsafe_prefix {
                return Err(Error::Other(format!(
                    "cannot serialize a modified document with a shadowed `{prefix}` namespace"
                )));
            }
            let serialized = match template.as_deref_mut() {
                Some(template) => self.main_part_xml_in(Some(template))?,
                None => self.main_part_xml()?,
            };
            let spans = self
                .main_part_blocks
                .as_ref()
                .and_then(|blocks| blocks.proved_namespace_spans(&serialized));
            let (replayed, has_retained_owner) = replay_nested_namespace_declarations_in(
                &serialized,
                nested_namespace_owners,
                spans.as_deref(),
                template.as_deref_mut(),
            )?;
            replayed_output_has_retained_namespace_owner = has_retained_owner;
            Some(replayed)
        };
        let namespace_scopes = document_namespace_scopes(
            doc_xml
                .as_deref()
                .or(kept_xml.as_deref())
                .expect("compared existing document XML"),
        )?;
        // The replay classification above belongs to the old source. A newly
        // serialized owner can introduce its own namespace scope. Certify the
        // exact output, reusing only facts qualified for those output bytes.
        let local_cell_owner_free = if replayed_output_has_retained_namespace_owner {
            // A successful actual selected output owner is enough to block
            // direct edits. Unknown output still needs its existing scan.
            false
        } else if let Some(xml) = &doc_xml {
            let owners = OnceLock::new();
            if let Some(classified) = self
                .main_part_blocks
                .as_ref()
                .and_then(|blocks| blocks.retained_nested_owners(xml))
            {
                let _ = owners.set(Some(classified));
            }
            let (blocked, _) = plain_local_edit_facts(
                &namespace_scopes.root_declarations,
                &namespace_scopes.body_declarations,
                xml,
                &owners,
            );
            !blocked
        } else {
            unsafe_prefix.is_none() && nested_namespace_owners.is_empty()
        };
        if let Some(doc_xml) = doc_xml {
            self.package.set_part(&self.doc_part_name, doc_xml);
            if let Some(template) = template {
                let source =
                    template.sources.source.as_ref().ok_or_else(|| {
                        Error::Other("published main writer lost its source".into())
                    })?;
                let bytes = self
                    .package
                    .get_part_shared(&self.doc_part_name)
                    .expect("actual main publication");
                template.sources.published_main = Some((Arc::clone(bytes), Arc::clone(source)));
            }
        }
        self.root_namespace_declarations = namespace_scopes.root_declarations;
        self.body_namespace_declarations = namespace_scopes.body_declarations;
        self.body_namespace_bindings = namespace_scopes.body_bindings;
        self.plain_local_edit_blocked = typed_document_is_unchanged && unsafe_prefix.is_some();
        self.local_cell_source = LocalCellSource::classified(
            &self.package,
            &self.doc_part_name,
            &self.root_namespace_declarations,
            &self.body_namespace_declarations,
            local_cell_owner_free,
        );
        if !typed_document_is_unchanged {
            self.unsavable_body_edit_cache = OnceLock::new();
        }
        Ok(())
    }

    /// The last opening, while the main part still holds those bytes.
    fn kept_main_part(&self) -> Option<Arc<Parsed>> {
        let parsed = self.parsed.as_ref()?;
        let kept = parsed.main_part_xml()?;
        let current = self.package.get_part(&self.doc_part_name)?;
        (parsed.main_part == self.doc_part_name && (std::ptr::eq(kept, current) || kept == current))
            .then(|| Arc::clone(parsed))
    }

    fn refresh_related_story_caches(&mut self) -> Result<()> {
        self.refresh_related_story_caches_in(None)
    }

    fn refresh_related_story_caches_in(
        &mut self,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<()> {
        if let Some(part_name) = self.footnotes_part_name.as_deref()
            && let Some(xml) = self.package.get_part_shared(part_name)
        {
            if let Some(template) = template.as_deref_mut() {
                template.sources.notes_model_read = true;
                if let Some(source) = template.take_related_source(part_name, xml)? {
                    self.footnotes = template.operation.source_notes(&source)?;
                    template.sources.read_note_count = self.footnotes.footnotes.len();
                    template.sources.put_related_source(part_name, xml, source);
                } else {
                    self.footnotes = oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(xml)?;
                }
            } else {
                self.footnotes = oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(xml)?;
            }
            self.footnotes_dirty = false;
        }
        if let Some(part_name) = self.comments_part_name.as_deref()
            && let Some(xml) = self.package.get_part_shared(part_name)
        {
            self.comments = Some(if let Some(writer) = template {
                let source = writer.take_related_source(part_name, xml)?.ok_or_else(|| {
                    Error::Other("comments refresh lost its actual source".into())
                })?;
                let comments = writer.operation.source_comments(&source)?;
                writer.sources.comments_reader_source = Some(Arc::clone(&source));
                writer.sources.put_related_source(part_name, xml, source);
                comments
            } else {
                oxoffice_docx_oxml::comments::CT_Comments::from_xml(xml)?
            });
            self.comments_dirty = false;
        }
        Ok(())
    }

    /// Make sure `part_name` is reachable from the main document: it needs a
    /// relationship of `rel_type` and a content-type override.
    pub(crate) fn ensure_part_relationship_checked(
        &mut self,
        part_name: &str,
        rel_type: &str,
        content_type: &str,
    ) -> Result<()> {
        let doc_part_name = self.doc_part_name.clone();
        let already_linked =
            self.package
                .get_part_rels(&doc_part_name)
                .is_some_and(|relationships| {
                    relationships.items.iter().any(|relationship| {
                        relationship.rel_type == rel_type
                            && relationship_is_internal(relationship)
                            && OpcPackage::resolve_rel_target(&doc_part_name, &relationship.target)
                                == part_name
                    })
                });
        let pending_id = if already_linked {
            None
        } else {
            Some(
                self.identifiers
                    .reserve_relationship_id_checked(&doc_part_name)?,
            )
        };
        self.package
            .content_types
            .add_override(part_name, content_type);
        self.identifiers.register_content_type_override(part_name);
        if let Some(id) = pending_id {
            // Relationship targets are relative to the source part's directory.
            let target = relative_target(&doc_part_name, part_name);
            self.package
                .get_or_create_part_rels(&doc_part_name)
                .add_with_id(&id, rel_type, &target);
            if bundle_relationship_order(rel_type).is_some() {
                self.identifiers
                    .authored_bundle_relationship_ids
                    .entry(relationship_owner_identity(&doc_part_name))
                    .or_default()
                    .insert(id);
            }
        }
        Ok(())
    }

    pub(crate) fn reserve_document_part_bundle(
        &mut self,
        existing: Option<&str>,
        preferred: &str,
        rel_type: &str,
        content_type: &str,
    ) -> Result<String> {
        self.identifiers.observe_package_graph(&self.package)?;
        let part_name = match existing {
            Some(part_name) => part_name.to_owned(),
            None => self.identifiers.reserve_preferred_part_name(preferred)?,
        };
        let owner = self.doc_part_name.clone();
        let already_linked = self
            .package
            .get_part_rels(&owner)
            .is_some_and(|relationships| {
                relationships.items.iter().any(|relationship| {
                    relationship.rel_type == rel_type
                        && relationship_is_internal(relationship)
                        && OpcPackage::resolve_rel_target(&owner, &relationship.target) == part_name
                })
            });
        let pending_id = if already_linked {
            None
        } else {
            Some(
                self.identifiers
                    .reserve_bundle_relationship_id_checked(&owner, rel_type)?,
            )
        };
        self.package
            .content_types
            .add_override(&part_name, content_type);
        self.identifiers.register_content_type_override(&part_name);
        if let Some(id) = pending_id {
            let target = relative_target(&owner, &part_name);
            self.package
                .get_or_create_part_rels(&owner)
                .add_with_id(&id, rel_type, &target);
        }
        Ok(part_name)
    }

    fn reserve_core_properties_bundle(&mut self) -> Result<String> {
        let existing = self.core_properties_part_name.clone();
        let part_name = self.reserve_package_properties_bundle(
            existing.as_deref(),
            DEFAULT_CORE_PROPERTIES_PART,
            rel_types::CORE_PROPERTIES,
            content_types::CORE_PROPERTIES,
        )?;
        self.core_properties_part_name = Some(part_name.clone());
        Ok(part_name)
    }

    fn reserve_application_properties_bundle(&mut self) -> Result<String> {
        let existing = self.application_properties_part_name.clone();
        let part_name = self.reserve_package_properties_bundle(
            existing.as_deref(),
            DEFAULT_APP_PROPERTIES_PART,
            rel_types::EXTENDED_PROPERTIES,
            content_types::EXTENDED_PROPERTIES,
        )?;
        self.application_properties_part_name = Some(part_name.clone());
        Ok(part_name)
    }

    fn reserve_custom_properties_bundle(&mut self) -> Result<String> {
        let existing = self.custom_properties_part_name.clone();
        let part_name = self.reserve_package_properties_bundle(
            existing.as_deref(),
            "/docProps/custom.xml",
            rel_types::CUSTOM_PROPERTIES,
            content_types::CUSTOM_PROPERTIES,
        )?;
        self.custom_properties_part_name = Some(part_name.clone());
        Ok(part_name)
    }

    fn reserve_package_properties_bundle(
        &mut self,
        existing: Option<&str>,
        preferred: &str,
        rel_type: &str,
        content_type: &str,
    ) -> Result<String> {
        self.identifiers.observe_package_graph(&self.package)?;
        let part_name = match existing {
            Some(part_name) => part_name.to_owned(),
            None => self.identifiers.reserve_preferred_part_name(preferred)?,
        };
        let already_linked = self.package.package_rels.items.iter().any(|relationship| {
            relationship.rel_type == rel_type
                && relationship_is_internal(relationship)
                && OpcPackage::resolve_rel_target("/", &relationship.target) == part_name
        });
        let pending_id = if already_linked {
            None
        } else {
            Some(self.identifiers.reserve_relationship_id_checked("/")?)
        };
        self.package
            .content_types
            .add_override(&part_name, content_type);
        self.identifiers.register_content_type_override(&part_name);
        if let Some(id) = pending_id {
            let target = part_name.strip_prefix('/').unwrap_or(&part_name);
            self.package.package_rels.add_with_id(&id, rel_type, target);
        }
        Ok(part_name)
    }

    pub(crate) fn add_internal_relationship_checked(
        &mut self,
        owner: &str,
        rel_type: &str,
        target: &str,
    ) -> Result<String> {
        let id = self.identifiers.reserve_relationship_id_checked(owner)?;
        self.package
            .get_or_create_part_rels(owner)
            .add_with_id(&id, rel_type, target);
        Ok(id)
    }

    pub(crate) fn add_relative_internal_relationship_checked(
        &mut self,
        owner: &str,
        rel_type: &str,
        target_part: &str,
    ) -> Result<String> {
        let id = self.add_internal_relationship_checked(
            owner,
            rel_type,
            &relative_target(owner, target_part),
        )?;
        if owner != self.doc_part_name {
            self.identifiers
                .authored_story_relationship_ids
                .entry(relationship_owner_identity(owner))
                .or_default()
                .insert(id.clone());
        }
        Ok(id)
    }

    fn add_image_relationship_checked(
        &mut self,
        owner: &str,
        image_data: &[u8],
        filename: &str,
    ) -> Result<String> {
        let (part_name, format) = self.reserve_image_part(image_data, filename)?;
        self.install_reserved_image_part(&part_name, image_data, format);
        self.add_relative_internal_relationship_checked(owner, rel_types::IMAGE, &part_name)
    }

    pub(crate) fn add_external_relationship_checked(
        &mut self,
        owner: &str,
        rel_type: &str,
        target: &str,
    ) -> Result<String> {
        let id = self.identifiers.reserve_relationship_id_checked(owner)?;
        self.package.get_or_create_part_rels(owner).items.push(
            oxoffice_opc::relationship::Relationship {
                id: id.clone(),
                rel_type: rel_type.to_owned(),
                target: target.to_owned(),
                target_mode: Some("External".to_owned()),
            },
        );
        if owner != self.doc_part_name {
            self.identifiers
                .authored_story_relationship_ids
                .entry(relationship_owner_identity(owner))
                .or_default()
                .insert(id.clone());
        }
        Ok(id)
    }

    pub(crate) fn add_html_image_relationship_to_story(
        &mut self,
        story: &StoryId,
        image_data: &[u8],
        filename: &str,
    ) -> Result<String> {
        self.story_source_and_owner(story)?;
        let owner = story.part_name.clone();
        let relationship_id = self.add_image_relationship_checked(&owner, image_data, filename)?;
        if owner == self.doc_part_name && story.kind != StoryKind::Body {
            self.identifiers
                .register_nested_story_relationship(story, relationship_id.clone());
        }
        Ok(relationship_id)
    }

    pub(crate) fn add_html_hyperlink_relationship_to_story(
        &mut self,
        story: &StoryId,
        url: &str,
    ) -> Result<String> {
        self.story_source_and_owner(story)?;
        let owner = story.part_name.clone();
        let relationship_id =
            self.add_external_relationship_checked(&owner, rel_types::HYPERLINK, url)?;
        if owner == self.doc_part_name && story.kind != StoryKind::Body {
            self.identifiers
                .register_nested_story_relationship(story, relationship_id.clone());
        }
        Ok(relationship_id)
    }

    pub(crate) fn reserve_html_drawing_id(&mut self, story: &StoryId) -> Result<u32> {
        self.story_source_and_owner(story)?;
        let drawing_id = self.identifiers.reserve_drawing_id()?;
        if story.part_name == self.doc_part_name && story.kind != StoryKind::Body {
            self.identifiers
                .register_nested_story_drawing(story, drawing_id);
        }
        Ok(drawing_id)
    }

    /// Stage a chart, its editable workbook, and the Word drawing that reaches them.
    fn add_chart_package(
        &mut self,
        source: ChartPackageSource<'_>,
        width: Length,
        height: Length,
    ) -> Result<()> {
        if width.to_emu() <= 0 || height.to_emu() <= 0 {
            return Err(Error::Other(
                "chart width and height must be positive".to_owned(),
            ));
        }

        let mut candidate = self.clone_for_staging();
        candidate
            .identifiers
            .observe_package_graph(&candidate.package)?;
        if matches!(&source, ChartPackageSource::Authored { .. }) {
            candidate.ensure_authored_chart_theme()?;
        }
        let chart_part = candidate
            .identifiers
            .reserve_part_name("/word/charts", "chart", "xml")?;
        let workbook_part =
            candidate
                .identifiers
                .reserve_part_name("/word/embeddings", "Workbook", "xlsx")?;
        let document_part = candidate.doc_part_name.clone();
        let document_relationship_id = candidate
            .add_relative_internal_relationship_checked(
                &document_part,
                rel_types::CHART,
                &chart_part,
            )
            .map_err(|error| {
                Error::Other(format!("chart relationship allocation failed: {error}"))
            })?;
        let workbook_relationship_id = candidate
            .identifiers
            .reserve_relationship_id_checked(&chart_part)
            .map_err(|error| {
                Error::Other(format!(
                    "chart workbook relationship allocation failed: {error}"
                ))
            })?;
        candidate
            .package
            .get_or_create_part_rels(&chart_part)
            .add_with_id(
                &workbook_relationship_id,
                rel_types::PACKAGE,
                &relative_target(&chart_part, &workbook_part),
            );
        let (chart_xml, workbook_bytes) = match source {
            ChartPackageSource::Typed { chart, workbook } => (
                chart_with_workbook_relationship(chart, &workbook_relationship_id)?,
                workbook
                    .to_xlsx_bytes()
                    .map_err(|error| Error::Other(format!("invalid chart workbook: {error}")))?,
            ),
            ChartPackageSource::Authored { kind, data } => {
                oxoffice_chart::authored_chart_parts(kind, data, &workbook_relationship_id)
                    .map_err(|error| Error::Other(format!("invalid chart data: {error}")))?
            }
        };

        let mut inline =
            CT_Inline::new_chart(&document_relationship_id, width.to_emu(), height.to_emu());
        inline.doc_pr_id = candidate.identifiers.reserve_drawing_id()?;
        let drawing = CT_Drawing::inline(inline);
        let run = CT_R {
            alt_drawings: Vec::new(),
            properties: None,
            content: vec![RunContent::Drawing(drawing)],
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
        };
        let mut paragraph = CT_P::new();
        paragraph.runs.push(run);
        candidate
            .document
            .body
            .content
            .push(BodyContent::Paragraph(Box::new(paragraph)));
        candidate.document.to_xml()?;

        candidate.package.set_part(&chart_part, chart_xml);
        candidate.package.set_part(&workbook_part, workbook_bytes);
        candidate
            .package
            .content_types
            .add_override(&chart_part, content_types::CHART);
        candidate
            .identifiers
            .register_content_type_override(&chart_part);
        candidate
            .package
            .content_types
            .add_override(&workbook_part, content_types::EMBEDDED_WORKBOOK);
        candidate
            .identifiers
            .register_content_type_override(&workbook_part);

        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    fn ensure_authored_chart_theme(&mut self) -> Result<()> {
        let document_part = self.doc_part_name.clone();
        let reusable = self
            .package
            .get_part_rels(&document_part)
            .and_then(|relationships| {
                relationships.items.iter().find(|relationship| {
                    relationship.rel_type == rel_types::THEME
                        && relationship_is_internal(relationship)
                })
            })
            .and_then(|relationship| {
                let part_name =
                    OpcPackage::resolve_rel_target(&document_part, &relationship.target);
                if self.package.content_types.content_type_for(&part_name)
                    != Some(content_types::THEME)
                {
                    return None;
                }
                let bytes = if self.theme_dirty
                    && self.theme_part_name.as_deref() == Some(part_name.as_str())
                {
                    self.theme.as_ref()?.to_xml().ok()?
                } else {
                    self.package.get_part(&part_name)?.to_vec()
                };
                let theme = oxoffice_drawing::theme::CT_OfficeStyleSheet::from_xml(&bytes).ok()?;
                Some((part_name, theme, bytes))
            });
        if let Some((part_name, theme, bytes)) = reusable {
            if self.theme_dirty && self.theme_part_name.as_deref() == Some(part_name.as_str()) {
                self.package.set_part(&part_name, bytes);
            }
            self.theme = Some(theme);
            self.theme_part_name = Some(part_name);
            return Ok(());
        }

        let theme = oxoffice_drawing::theme::CT_OfficeStyleSheet::office_default();
        let theme_xml = theme
            .to_xml()
            .map_err(|error| Error::Other(format!("invalid default Office theme: {error}")))?;
        self.identifiers.observe_package_graph(&self.package)?;
        let theme_part = self
            .identifiers
            .reserve_preferred_part_name(DEFAULT_THEME_PART)?;
        self.package.set_part(&theme_part, theme_xml);
        self.package
            .content_types
            .add_override(&theme_part, content_types::THEME);
        self.identifiers.register_content_type_override(&theme_part);

        let target = relative_target(&document_part, &theme_part);
        let relationships = self.package.get_or_create_part_rels(&document_part);
        if let Some(relationship) = relationships
            .items
            .iter_mut()
            .find(|relationship| relationship.rel_type == rel_types::THEME)
        {
            relationship.target = target;
            relationship.target_mode = None;
        } else {
            let id = self
                .identifiers
                .reserve_relationship_id_checked(&document_part)?;
            relationships.add_with_id(&id, rel_types::THEME, &target);
        }
        self.theme = Some(theme);
        self.theme_part_name = Some(theme_part);
        self.theme_dirty = false;
        Ok(())
    }

    pub(crate) fn generated_table_paragraph_positions(
        &self,
    ) -> Result<Vec<(ContentLocation, std::ops::Range<usize>)>> {
        let mut result = Vec::new();
        for story in self.stories()? {
            if story.kind == StoryKind::Comment {
                continue;
            }
            let (source, _) = self.story_source_and_owner(&story)?;
            let xml = self
                .package
                .get_part(&source.part_name)
                .unwrap_or(source.xml.as_ref());
            let owner = scan_story_owners(xml, source.root_kind)?
                .into_iter()
                .find(|owner| owner.kind == story.kind && owner.owner_index == story.owner_index)
                .ok_or_else(|| {
                    Error::Other(
                        "generated paragraph owner disappeared from retained source".into(),
                    )
                })?;
            for (index, item) in scan_story_items(xml, &owner, None)?.iter().enumerate() {
                let spans = match item.kind {
                    StoryItemKind::Paragraph => vec![(vec![index], item.full.clone())],
                    StoryItemKind::ContentControl => scan_story_control_paragraphs(xml, item)?
                        .into_iter()
                        .enumerate()
                        .map(|(paragraph, span)| (vec![index, paragraph], span))
                        .collect(),
                    _ => continue,
                };
                for (path, range) in spans {
                    let location =
                        ContentLocation::new(story.clone(), StoryItemKind::Paragraph, path);
                    self.story_range_paragraph_source(&location)?;
                    result.push((location, range));
                }
            }
        }
        result.sort_by_key(|(location, range)| {
            (location.story().part_name().to_owned(), range.start)
        });
        Ok(result)
    }

    /// Exact physical story-owner content, from the existing checked inventory.
    pub(crate) fn generated_table_story_sources(&self) -> Result<Vec<GeneratedStoryOwner>> {
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for source in self.story_sources()? {
            let xml = if source.part_name == self.doc_part_name {
                self.package
                    .get_part(&source.part_name)
                    .unwrap_or(source.xml.as_ref())
            } else {
                source.xml.as_ref()
            };
            for owner in scan_story_owners(xml, source.root_kind)? {
                if matches!(owner.kind, StoryKind::Comment | StoryKind::TableCell)
                    || !seen.insert((owner.kind, source.part_name.clone(), owner.owner_index))
                {
                    continue;
                }
                let mut reader = quick_xml::Reader::from_reader(&xml[owner.full.clone()]);
                let mut buffer = Vec::new();
                let start = match reader.read_event_into(&mut buffer).map_err(|error| {
                    Error::Other(format!("story owner start scan failed: {error}"))
                })? {
                    Event::Start(_) => owner.full.start + reader.buffer_position() as usize,
                    Event::Empty(_) => continue,
                    _ => return Err(Error::Other("story owner has no start element".into())),
                };
                let end = story_owner_content_end(xml, &owner)?;
                let scope = story_namespace_scope_at(xml, start)?;
                result.push(GeneratedStoryOwner {
                    story: StoryId {
                        kind: owner.kind,
                        part_name: source.part_name.clone(),
                        owner_index: owner.owner_index,
                        fingerprint: owner.fingerprint,
                    },
                    range: start..end,
                    xml: xml.to_vec(),
                    namespaces: scope,
                });
            }
        }
        Ok(result)
    }

    fn story_sources(&self) -> Result<Vec<StorySource<'_>>> {
        Ok(self.spanned_story_sources()?.0)
    }

    #[allow(clippy::type_complexity)]
    fn spanned_story_sources(
        &self,
    ) -> Result<(Vec<StorySource<'_>>, Range<usize>, Vec<Range<usize>>)> {
        let (main, content, children) = self.main_part_with_body_spans()?;
        let mut sources = vec![StorySource {
            root_kind: StoryKind::Body,
            part_name: self.doc_part_name.clone(),
            xml: Cow::Owned(main),
        }];
        sources.extend(self.related_story_sources()?);
        Ok((sources, content, children))
    }

    fn story_sources_and_main_layout(
        &self,
        items: bool,
    ) -> Result<(Vec<StorySource<'_>>, Option<MainStoryLayout>)> {
        let (sources, content, children) = self.spanned_story_sources()?;
        let layout = self
            .main_story_layout(&sources[0].xml, &content, &children, items)
            .ok();
        Ok((sources, layout))
    }

    /// The story sources after the main part, in [`Self::story_sources`]
    /// order, read from the package without serializing the main part.
    fn related_story_sources(&self) -> Result<Vec<StorySource<'_>>> {
        let mut sources = Vec::new();
        let mut seen = HashSet::new();
        seen.insert((StoryKind::Body, self.doc_part_name.clone()));

        for (relationship_id, is_header) in self.header_footer_rel_ids() {
            let kind = if is_header {
                StoryKind::Header
            } else {
                StoryKind::Footer
            };
            let part_name = self
                .header_footer_part_name(&relationship_id, is_header)
                .ok_or_else(|| {
                    Error::Other(format!(
                        "{kind:?} reference {relationship_id} has no valid internal relationship"
                    ))
                })?;
            if seen.insert((kind, part_name.clone())) {
                let xml = self.package.get_part(&part_name).ok_or_else(|| {
                    Error::Other(format!("{kind:?} story targets missing part {part_name}"))
                })?;
                sources.push(StorySource {
                    root_kind: kind,
                    part_name,
                    xml: Cow::Borrowed(xml),
                });
            }
        }

        if let Some(relationships) = self.package.get_part_rels(&self.doc_part_name) {
            for relationship in &relationships.items {
                let kind = match relationship.rel_type.as_str() {
                    rel_types::FOOTNOTES => StoryKind::Footnote,
                    rel_types::ENDNOTES => StoryKind::Endnote,
                    rel_types::COMMENTS => StoryKind::Comment,
                    _ => continue,
                };
                if !relationship_is_internal(relationship) {
                    continue;
                }
                let part_name =
                    OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target);
                if !seen.insert((kind, part_name.clone())) {
                    return Err(Error::Other(format!(
                        "{kind:?} story part {part_name} is referenced more than once"
                    )));
                }
                let part_xml = self.package.get_part(&part_name).ok_or_else(|| {
                    Error::Other(format!("{kind:?} story targets missing part {part_name}"))
                })?;
                let xml = if kind == StoryKind::Comment {
                    Cow::Owned(part_xml.to_vec())
                } else {
                    Cow::Borrowed(part_xml)
                };
                sources.push(StorySource {
                    root_kind: kind,
                    part_name,
                    xml,
                });
            }
        }
        Ok(sources)
    }

    /// Discover every supported Word story owner in stable document and
    /// relationship order.
    pub fn stories(&self) -> Result<Vec<StoryId>> {
        let (sources, main) = self.story_sources_and_main_layout(false)?;
        Self::stories_in_sources(sources, main)
    }

    fn stories_in_sources(
        sources: Vec<StorySource<'_>>,
        main: Option<MainStoryLayout>,
    ) -> Result<Vec<StoryId>> {
        let mut stories = Vec::new();
        let mut seen = HashSet::new();
        for (index, source) in sources.into_iter().enumerate() {
            let owners = match main.as_ref().filter(|_| index == 0) {
                Some(layout) => layout.owners(&source.xml)?,
                None => scan_story_owners(&source.xml, source.root_kind)?,
            };
            for owner in owners {
                let identity = (owner.kind, source.part_name.clone(), owner.owner_index);
                if seen.insert(identity) {
                    stories.push(StoryId {
                        kind: owner.kind,
                        part_name: source.part_name.clone(),
                        owner_index: owner.owner_index,
                        fingerprint: owner.fingerprint,
                    });
                }
            }
        }
        Ok(stories)
    }

    fn story_source_and_owner<'a>(
        &'a self,
        story: &StoryId,
    ) -> Result<(StorySource<'a>, StoryOwnerSpan)> {
        let (source, owner, _) = self.story_source_owner_and_layout(story, false)?;
        Ok((source, owner))
    }

    fn story_source_owner_and_layout<'a>(
        &'a self,
        story: &StoryId,
        items: bool,
    ) -> Result<(StorySource<'a>, StoryOwnerSpan, Option<MainStoryLayout>)> {
        let (sources, content, children) = self.spanned_story_sources()?;
        self.story_source_owner_and_layout_in(story, items, sources, &content, &children)
    }

    fn story_source_owner_and_layout_in<'a>(
        &self,
        story: &StoryId,
        items: bool,
        mut sources: Vec<StorySource<'a>>,
        content: &Range<usize>,
        children: &[Range<usize>],
    ) -> Result<(StorySource<'a>, StoryOwnerSpan, Option<MainStoryLayout>)> {
        let source = sources.iter().position(|source| {
            source.part_name == story.part_name
                && (source.root_kind == story.kind
                    || matches!(story.kind, StoryKind::TableCell | StoryKind::TextBox))
        });
        let (source, layout) = match source {
            Some(index) => {
                let source = sources.swap_remove(index);
                let layout = if index == 0 {
                    self.main_story_layout(&source.xml, content, children, items)
                        .ok()
                } else {
                    None
                };
                (source, layout)
            }
            None => {
                if let Some(actual) = sources
                    .iter()
                    .find(|source| source.part_name == story.part_name)
                    .map(|source| source.root_kind)
                {
                    return Err(StoryError::WrongOwner {
                        expected: story.kind,
                        actual,
                    }
                    .into());
                }
                return Err(StoryError::OwnerNotFound {
                    story: story.clone(),
                }
                .into());
            }
        };
        let owner = match &layout {
            Some(layout) => layout.owner(&source.xml, story.kind, story.owner_index)?,
            None => scan_story_owners(&source.xml, source.root_kind)?
                .into_iter()
                .find(|owner| owner.kind == story.kind && owner.owner_index == story.owner_index),
        }
        .ok_or_else(|| StoryError::OwnerNotFound {
            story: story.clone(),
        })?;
        if story.fingerprint != owner.fingerprint {
            return Err(StoryError::Stale {
                story: story.clone(),
            }
            .into());
        }
        Ok((source, owner, layout))
    }

    fn main_story_layout(
        &self,
        xml: &[u8],
        content: &Range<usize>,
        children: &[Range<usize>],
        items: bool,
    ) -> Result<MainStoryLayout> {
        let (head, tail) = (&xml[..content.start], &xml[content.end..]);
        let mut scans = self
            .main_body_scans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let frame = match &scans.frame {
            Some(frame) if scans.head == head && scans.tail == tail => Arc::clone(frame),
            _ => {
                let serialized = scans.serialized.take();
                *scans = MainBodyScans::default();
                let frame = Arc::new(scan_body_block(head, None, tail, true)?);
                *scans = MainBodyScans {
                    head: head.to_vec(),
                    tail: tail.to_vec(),
                    frame: Some(Arc::clone(&frame)),
                    blocks: Vec::new(),
                    fingerprint: None,
                    serialized,
                };
                frame
            }
        };
        let old = std::mem::take(&mut scans.blocks);
        // Owners-only scans have no items. An item lookup must read those
        // children again. An owners-only lookup keeps a scan that already
        // holds items.
        let same = |scan: &Arc<BodyBlockScan>, range: &Range<usize>| {
            scan.bytes == xml[range.clone()] && (!items || scan.items.is_some())
        };
        let prefix = old
            .iter()
            .zip(children)
            .take_while(|(scan, range)| same(scan, range))
            .count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(children[prefix..].iter().rev())
            .take_while(|(scan, range)| same(scan, range))
            .count();
        let mut blocks = children
            .iter()
            .enumerate()
            .map(|(index, _range)| {
                if index < prefix {
                    Some(Arc::clone(&old[index]))
                } else if suffix > 0 && index >= children.len() - suffix {
                    Some(Arc::clone(&old[old.len() - (children.len() - index)]))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let todo = blocks
            .iter()
            .enumerate()
            .filter(|(_, scan)| scan.is_none())
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let alone = todo
            .iter()
            .map(|index| head.len() + children[*index].len() + tail.len())
            .sum::<usize>();
        if prefix != children.len() || prefix != old.len() {
            scans.fingerprint = None;
        }
        let whole_walk = alone >= xml.len();
        let scanned = if !whole_walk {
            let mut scanned = Vec::with_capacity(todo.len());
            for index in &todo {
                let bytes = &xml[children[*index].clone()];
                match scan_body_block(head, Some(bytes), tail, items) {
                    Ok(scan) => scanned.push(scan),
                    Err(error) => {
                        scans.blocks = old;
                        return Err(error);
                    }
                }
            }
            scanned
        } else {
            match scan_body_children(xml, children, &todo, items) {
                Ok((scanned, fingerprint)) => {
                    scans.fingerprint = Some(fingerprint);
                    scanned
                }
                Err(error) => {
                    scans.blocks = old;
                    return Err(error);
                }
            }
        };
        for (index, scan) in todo.into_iter().zip(scanned) {
            blocks[index] = Some(Arc::new(scan));
        }
        let blocks = blocks.into_iter().flatten().collect::<Vec<_>>();
        scans.blocks = blocks.clone();
        let fingerprint = scans.fingerprint;
        drop(scans);
        let layout = MainStoryLayout {
            content: content.clone(),
            frame,
            blocks: children
                .iter()
                .map(|range| range.start)
                .zip(blocks)
                .collect(),
            fingerprint,
        };
        #[cfg(debug_assertions)]
        {
            assert_main_story_layout(xml, &layout, items);
        }
        Ok(layout)
    }

    /// Bind an existing control to a snapshot-checked placeholder entry.
    pub fn bind_building_block_placeholder(
        &mut self,
        control: &ContentLocation,
        entry: &crate::BuildingBlockInfo,
    ) -> Result<()> {
        self.checked_building_block(entry)?;
        if entry.block.gallery.as_deref() != Some("placeholder") {
            return Err(Error::Other(
                "placeholder binding requires the placeholder gallery".to_owned(),
            ));
        }
        if control.item_kind != StoryItemKind::ContentControl
            || control.index_path.len() != 1
            || control.is_end
        {
            return Err(Error::Other(
                "placeholder binding requires an existing content control location".to_owned(),
            ));
        }
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_package()?;
        let (source, owner, _) =
            candidate.fragment_story_source_owner_and_owners(&control.story)?;
        let part = source.part_name.clone();
        let mut xml = source.xml.into_owned();
        let item = scan_story_items(&xml, &owner, None)?
            .get(control.index_path[0])
            .cloned()
            .ok_or_else(|| Error::Other("stale content control index".to_owned()))?;
        if item.kind != StoryItemKind::ContentControl {
            return Err(Error::Other(
                "content control location changed kind".to_owned(),
            ));
        }
        let scope = story_namespace_scope_at(&xml, item.full.start)?;
        let raw = close_content_fragment_namespaces(&xml[item.full.clone()], &scope)?;
        let updated = crate::building_block::bind_placeholder_xml(&raw, entry)?;
        xml.splice(item.full, updated);
        set_story_source_xml(&mut candidate, &part, xml)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    pub(crate) fn preserve_glossary_drawing_ids_staged(&mut self) -> Result<()> {
        if let Some(glossary) = &self.glossary {
            let ids = drawing_ids_in_xml(&glossary.to_xml()?)?;
            self.identifiers.drawing_ids.extend(ids.iter().copied());
            self.identifiers.preserved_drawing_ids.extend(ids);
        }
        Ok(())
    }

    fn fragment_projection_relationships(
        &mut self,
        source_part: &str,
        selected: &[u8],
    ) -> Result<BTreeMap<String, String>> {
        if source_part == self.doc_part_name {
            return Ok(BTreeMap::new());
        }
        let source_rels = self
            .package
            .get_part_rels(source_part)
            .cloned()
            .unwrap_or_default();
        let mut relationship_map = BTreeMap::new();
        for relationship_id in crate::field::relationship_ids_in_xml(selected)? {
            let relationship = source_rels.get_by_id(&relationship_id).ok_or_else(|| {
                Error::Other(format!(
                    "fragment source relationship {relationship_id} is missing from {source_part}"
                ))
            })?;
            let destination_part = self.doc_part_name.clone();
            let new_id = if relationship_is_internal(relationship) {
                let target_part = OpcPackage::resolve_rel_target(source_part, &relationship.target);
                self.add_internal_relationship_checked(
                    &destination_part,
                    &relationship.rel_type,
                    &relative_target(&destination_part, &target_part),
                )?
            } else {
                self.add_external_relationship_checked(
                    &destination_part,
                    &relationship.rel_type,
                    &relationship.target,
                )?
            };
            relationship_map.insert(relationship_id, new_id);
        }
        Ok(relationship_map)
    }

    pub(crate) fn project_fragment_part_content(
        &mut self,
        source_part: &str,
        mut selected: Vec<u8>,
    ) -> Result<()> {
        if source_part != self.doc_part_name {
            let relationships = self.fragment_projection_relationships(source_part, &selected)?;
            selected = crate::field::patch_relationship_ids(&selected, &relationships)?;
        }
        let main_xml = self
            .package
            .get_part(&self.doc_part_name)
            .ok_or_else(|| Error::Other("document fragment main part is missing".to_owned()))?
            .to_vec();
        let main_owner = scan_story_owners(&main_xml, StoryKind::Body)?
            .into_iter()
            .find(|owner| owner.kind == StoryKind::Body)
            .ok_or_else(|| Error::Other("document fragment main body is missing".to_owned()))?;
        let main_items = direct_story_content_items(&main_xml, &main_owner)?;
        let main_end = story_owner_content_end(&main_xml, &main_owner)?;
        let main_start = main_items.first().map_or(main_end, |item| item.full.start);
        let mut updated = main_xml;
        updated.splice(main_start..main_end, selected);
        self.package.set_part(&self.doc_part_name, updated);
        Ok(())
    }

    pub(crate) fn project_fragment_part_source(
        &mut self,
        source_part: &str,
        selected: oxoffice_docx_oxml::document::ScopedWordSource,
        main_source: &oxoffice_docx_oxml::document::ScopedWordSource,
        operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
    ) -> Result<oxoffice_docx_oxml::document::ScopedWordSource> {
        let relationships =
            self.fragment_projection_relationships(source_part, selected.as_bytes())?;
        let selected =
            crate::field::patch_relationship_source(operation, selected, &relationships)?;
        let main_owner = scan_story_owners(main_source.as_bytes(), StoryKind::Body)?
            .into_iter()
            .find(|owner| owner.kind == StoryKind::Body)
            .ok_or_else(|| Error::Other("document fragment main body is missing".into()))?;
        let main_items = direct_story_content_items(main_source.as_bytes(), &main_owner)?;
        let main_end = story_owner_content_end(main_source.as_bytes(), &main_owner)?;
        let main_start = main_items.first().map_or(main_end, |item| item.full.start);
        let source = operation.compose_sources(&[
            (Some(main_source), 0..main_start, &[]),
            (Some(&selected), 0..selected.as_bytes().len(), &[]),
            (
                Some(main_source),
                main_end..main_source.as_bytes().len(),
                &[],
            ),
        ])?;
        let retained = operation.repeat_source(&source)?;
        self.package
            .set_part(&self.doc_part_name, operation.finish_source(retained)?);
        Ok(source)
    }

    fn fragment_story_source_owner_and_owners<'a>(
        &'a self,
        story: &StoryId,
    ) -> Result<(StorySource<'a>, StoryOwnerSpan, Vec<StoryOwnerSpan>)> {
        let (canonical, _) = self.story_source_and_owner(story)?;
        self.fragment_story_source_from_canonical(story, &canonical)
    }

    // Reuse a canonical source only within the same immutable candidate phase.
    // The retained physical owner is still independently discovered below.
    fn fragment_story_source_from_canonical<'a>(
        &'a self,
        story: &StoryId,
        canonical: &StorySource<'_>,
    ) -> Result<(StorySource<'a>, StoryOwnerSpan, Vec<StoryOwnerSpan>)> {
        let xml = self.package.get_part(&canonical.part_name).ok_or_else(|| {
            Error::Other(format!(
                "document fragment story part {} is missing",
                canonical.part_name
            ))
        })?;
        let owners = scan_story_owners(xml, canonical.root_kind)?;
        let owner = owners
            .iter()
            .find(|owner| owner.kind == story.kind && owner.owner_index == story.owner_index)
            .cloned()
            .ok_or_else(|| StoryError::OwnerNotFound {
                story: story.clone(),
            })?;
        Ok((
            StorySource {
                root_kind: canonical.root_kind,
                part_name: canonical.part_name.clone(),
                xml: Cow::Borrowed(xml),
            },
            owner,
            owners,
        ))
    }

    pub(crate) fn story_paragraph_mut(&mut self, location: &ContentLocation) -> Result<&mut CT_P> {
        let (part_name, paragraph_slot, ordinal, cell_route) = {
            let (source, owner) = self.story_source_and_owner(&location.story)?;
            let part_name = source.part_name.clone();
            let source_xml = source.xml.into_owned();
            // A paragraph inside a block content control of the body is the
            // control item followed by the paragraph's position among the
            // paragraphs of that control.
            let (item_index, ordinal) = match location.index_path.as_slice() {
                [item_index] => (*item_index, None),
                [item_index, ordinal] if location.story.kind == StoryKind::Body => {
                    (*item_index, Some(*ordinal))
                }
                _ => {
                    return Err(StoryError::InvalidPath {
                        path: location.index_path.clone(),
                    }
                    .into());
                }
            };
            let items = scan_story_items(&source_xml, &owner, None)?;
            let item = items.get(item_index).ok_or(StoryError::OutOfBounds {
                index: item_index,
                len: items.len(),
            })?;
            if location.item_kind != StoryItemKind::Paragraph {
                return Err(Error::Other(
                    "comment positions must identify paragraphs".to_owned(),
                ));
            }
            if ordinal.is_some()
                && (item.kind != StoryItemKind::ContentControl || !item.direct_owner_child)
            {
                return Err(Error::Other(
                    "a two-segment comment position must start with a block content control"
                        .to_owned(),
                ));
            }
            if ordinal.is_none() && item.kind != location.item_kind {
                return Err(StoryError::KindMismatch {
                    expected: location.item_kind,
                    actual: item.kind,
                }
                .into());
            }
            // A paragraph item is a direct child of its owner. In the body it
            // resolves to its direct body slot, the count of direct items
            // before it, and in a cell to its position among the cell's
            // direct paragraphs. Neither counts the paragraphs inside a block
            // content control. The body section properties are serialized
            // last, so they never precede a paragraph.
            let preceding = items[..item_index].iter();
            let paragraph_slot = if location.story.kind == StoryKind::Body {
                preceding.filter(|item| item.direct_owner_child).count()
            } else {
                preceding
                    .filter(|item| item.kind == StoryItemKind::Paragraph)
                    .count()
            };
            let cell_route = (location.story.kind == StoryKind::TableCell
                && part_name == self.doc_part_name)
                .then(|| modeled_main_cell_route(&source_xml, &owner))
                .transpose()?;
            (part_name, paragraph_slot, ordinal, cell_route)
        };
        if part_name != self.doc_part_name {
            return Err(Error::Other(
                "path-aware comments currently support the main document story".to_owned(),
            ));
        }
        match location.story.kind {
            StoryKind::Body => {
                match (self.document.body.content.get_mut(paragraph_slot), ordinal) {
                    (Some(BodyContent::Paragraph(paragraph)), None) => Ok(paragraph),
                    (Some(BodyContent::ContentControl(control)), Some(ordinal)) => control
                        .content
                        .iter_mut()
                        .filter_map(|child| match child {
                            SdtContent::Paragraph(paragraph) => Some(paragraph),
                            _ => None,
                        })
                        .nth(ordinal)
                        .ok_or_else(|| {
                            Error::Other(format!(
                                "comment content control has no paragraph {ordinal}"
                            ))
                        }),
                    _ => Err(Error::Other("comment body paragraph is missing".to_owned())),
                }
            }
            StoryKind::TableCell => {
                let (content_index, mut cell_index) = cell_route.ok_or_else(|| {
                    Error::Other("comment position has no modeled table-cell route".to_owned())
                })?;
                let content = self
                    .document
                    .body
                    .content
                    .get_mut(content_index)
                    .ok_or_else(|| Error::Other("comment table container is missing".to_owned()))?;
                let cell = match content {
                    BodyContent::Table(table) => nth_cell_in_table(table, &mut cell_index),
                    BodyContent::ContentControl(control) => {
                        nth_cell_in_control(control, &mut cell_index)
                    }
                    BodyContent::Paragraph(_) | BodyContent::RawXml(_) => None,
                }
                .ok_or_else(|| Error::Other("comment table cell is missing".to_owned()))?;
                cell.content
                    .iter_mut()
                    .filter_map(|child| match child {
                        CellContent::Paragraph(paragraph) => Some(paragraph),
                        CellContent::Table(_) | CellContent::ContentControl(_) => None,
                    })
                    .nth(paragraph_slot)
                    .ok_or_else(|| Error::Other("comment cell paragraph is missing".to_owned()))
            }
            _ => Err(Error::Other(
                "path-aware comments support body and table-cell paragraphs".to_owned(),
            )),
        }
    }

    /// Insert a checked run through its physical story paragraph owner.
    pub(crate) fn insert_story_run_staged(
        &mut self,
        position: &crate::StoryRunPosition,
        run: CT_R,
    ) -> Result<()> {
        let (source, item) = self.story_range_paragraph_source(&position.location)?;
        if item.kind != StoryItemKind::Paragraph {
            return Err(Error::Other(
                "field insertion needs a paragraph location".into(),
            ));
        }
        let part_name = source.part_name.clone();
        let mut xml = source.xml.into_owned();
        let scope = story_namespace_scope_at(&xml, item.full.start)?;
        let closed = close_content_fragment_namespaces(&xml[item.full.clone()], &scope)?;
        let mut paragraph = CT_P::from_xml_fragment(&closed)?;
        paragraph
            .insert_accepted_run(position.run_index, run)
            .map_err(|error| Error::Other(format!("field insertion boundary: {error}")))?;
        let replacement = serialize_content_fragment(BodyContent::Paragraph(Box::new(paragraph)))?;
        let replacement = close_content_fragment_namespaces(&replacement, &scope)?;
        xml.splice(item.full, replacement);
        set_story_source_xml(self, &part_name, xml)
    }

    pub(crate) fn anchor_story_range(
        &mut self,
        range: &crate::comments::StoryRunRange,
        anchor: RangeAnchor<'_>,
        label: &str,
        endpoint_probe: bool,
    ) -> Result<()> {
        let start = &range.start.location;
        let end = &range.end.location;
        let start_run = range.start.run_index;
        let end_run = range.end.run_index;
        if start.story != end.story {
            return Err(Error::Other(format!(
                "{label} range endpoints must be ordered in one story owner"
            )));
        }
        let nested_body_control = [start, end].iter().any(|location| {
            location.index_path.len() == 2
                && self
                    .story_item_source(&ContentLocation::new(
                        location.story.clone(),
                        StoryItemKind::ContentControl,
                        vec![location.index_path[0]],
                    ))
                    .is_ok_and(|(_, control)| !control.direct_owner_child)
        });
        // Multiple paragraphs must be qualified before either endpoint changes
        // the owner's fingerprint. The raw path below publishes both spans once.
        if start == end
            && start.story.kind() == StoryKind::Body
            && (start.index_path.len() == 2 || end.index_path.len() == 2)
            && !nested_body_control
        {
            if start.index_path > end.index_path {
                return Err(Error::Other(format!(
                    "{label} range endpoints must be ordered in one story owner"
                )));
            }
            let mut first = self.story_paragraph_mut(start)?.clone();
            let mut last = if start == end {
                first.clone()
            } else {
                self.story_paragraph_mut(end)?.clone()
            };
            if start == end {
                first
                    .anchor_accepted_range(Some(start_run), Some(end_run), anchor)
                    .map_err(|error| {
                        Error::Other(format!("{label} range cannot be anchored: {error}"))
                    })?;
            } else {
                first
                    .anchor_accepted_range(Some(start_run), None, anchor)
                    .map_err(|error| {
                        Error::Other(format!("{label} range cannot be anchored: {error}"))
                    })?;
                last.anchor_accepted_range(None, Some(end_run), anchor)
                    .map_err(|error| {
                        Error::Other(format!("{label} range cannot be anchored: {error}"))
                    })?;
            }
            *self.story_paragraph_mut(start)? = first;
            if start != end {
                *self.story_paragraph_mut(end)? = last;
            }
            return Ok(());
        }
        let (source, start_item) = self.story_range_paragraph_source(start)?;
        let part_name = source.part_name.clone();
        let source_xml = source.xml.into_owned();
        let same_endpoint = start == end;
        let end_item = if same_endpoint {
            start_item.clone()
        } else {
            self.story_range_paragraph_source(end)?.1
        };
        if start_item.full.start > end_item.full.start {
            return Err(Error::Other(format!(
                "{label} range endpoints must be ordered in one story owner"
            )));
        }
        let read_paragraph = |item: &StoryItemSpan| -> Result<CT_P> {
            if item.kind != StoryItemKind::Paragraph {
                return Err(Error::Other(
                    "comment positions must identify paragraphs".to_owned(),
                ));
            }
            let scope = story_namespace_scope_at(&source_xml, item.full.start)?;
            let closed = close_content_fragment_namespaces(&source_xml[item.full.clone()], &scope)?;
            Ok(CT_P::from_xml_fragment(&closed)?)
        };
        let mut first = read_paragraph(&start_item)?;
        let mut last = if start_item.full == end_item.full {
            None
        } else {
            Some(read_paragraph(&end_item)?)
        };
        for (boundary, index, paragraph) in [
            ("start", start_run, &first),
            ("end", end_run, last.as_ref().unwrap_or(&first)),
        ] {
            let count = paragraph.accepted_run_paths().len();
            if index > count {
                return Err(Error::Other(format!(
                    "{label} range {boundary} run index {index} exceeds paragraph run count {count}"
                )));
            }
        }
        if start_item.full == end_item.full {
            if start_run > end_run {
                return Err(Error::Other(format!(
                    "{label} range start must not follow its end"
                )));
            }
            first
                .anchor_accepted_range(Some(start_run), Some(end_run), anchor)
                .map_err(|error| {
                    Error::Other(format!("{label} range cannot be anchored: {error}"))
                })?;
        } else {
            first
                .anchor_accepted_range(Some(start_run), None, anchor)
                .map_err(|error| {
                    Error::Other(format!("{label} range cannot be anchored: {error}"))
                })?;
            last.as_mut()
                .expect("distinct endpoint was parsed")
                .anchor_accepted_range(None, Some(end_run), anchor)
                .map_err(|error| {
                    Error::Other(format!("{label} range cannot be anchored: {error}"))
                })?;
        }
        let mut edits = vec![(start_item.full, first)];
        if let Some(last) = last {
            edits.push((end_item.full, last));
        }
        edits.sort_by_key(|item| std::cmp::Reverse(item.0.start));
        let mut updated = source_xml;
        for (span, paragraph) in edits {
            let scope = story_namespace_scope_at(&updated, span.start)?;
            let xml = serialize_content_fragment(BodyContent::Paragraph(Box::new(paragraph)))?;
            let xml = close_content_fragment_namespaces(&xml, &scope)?;
            updated.splice(span, xml);
        }
        let updated = match anchor {
            RangeAnchor::Comment(id) if !endpoint_probe => {
                self.comment_move_source_with_namespaces(&part_name, updated, id, false)?
            }
            _ => updated,
        };
        set_story_source_xml(self, &part_name, updated)
    }

    /// Stage a comment move using physical source edits, then bind the surviving
    /// destination paragraphs and accepted run boundaries to their new owners.
    pub(crate) fn move_comment_story_range(
        &mut self,
        entry: &crate::comments::StoryRangeRef,
        target: &StoryRunRange,
        id: i32,
    ) -> Result<()> {
        // Validate both endpoints before any source is removed. The probe never
        // publishes its synthesized reference or changes the requested axis.
        let mut probe = self.clone_for_staging();
        probe.anchor_story_range(target, RangeAnchor::Comment(id), "comment", true)?;
        let endpoints = [&target.start, &target.end]
            .map(|position| {
                let (source, paragraph) = self.story_range_paragraph_source(&position.location)?;
                Ok((source.part_name.clone(), paragraph.full.start))
            })
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        let (part_name, xml, selected) = self.story_range_marker_spans(entry, true)?;
        let (edits, reference, reference_span) = comment_move_source_edits(&xml, &selected)?;
        let mut reference_axis = None;
        let (_, paragraphs) = self.story_range_paragraph_sources()?;
        for (_, _, span, location, raw) in paragraphs {
            if location.story != entry.range().start.location.story {
                continue;
            }
            // This inventory already owns the checked physical span and
            // closed XML. Resolving every location again repeats owner scans.
            if !span.contains(&reference_span.start) {
                continue;
            }
            let paragraph_model = CT_P::from_xml_fragment(&raw)?;
            let indices = paragraph_model
                .accepted_run_paths()
                .iter()
                .enumerate()
                .filter_map(|(index, path)| {
                    paragraph_model
                        .accepted_run(path)
                        .filter(|run| {
                            run.content.iter().any(|content| {
                                matches!(content,
                        RunContent::CommentReference { id: found, .. } if *found == id)
                            })
                        })
                        .map(|_| index)
                })
                .collect::<Vec<_>>();
            if indices.len() != 1 || reference_axis.is_some() {
                return Err(Error::Other(
                    "comment reference has no unique accepted run boundary".into(),
                ));
            }
            reference_axis = Some((span.start, indices[0]));
        }
        let (reference_paragraph, reference_index) = reference_axis.ok_or_else(|| {
            Error::Other("comment reference is outside the supported accepted run axis".into())
        })?;
        let map_offset = |offset: usize| -> Result<usize> {
            if edits.iter().any(|span| span.contains(&offset)) {
                return Err(Error::Other(
                    "comment move destination is inside a removed marker-only owner".into(),
                ));
            }
            Ok(offset
                - edits
                    .iter()
                    .filter(|span| span.end <= offset)
                    .map(|span| span.len())
                    .sum::<usize>())
        };
        let mut updated = xml;
        for span in edits.iter().rev() {
            updated.drain(span.clone());
        }
        let updated = self.comment_move_source_with_namespaces(&part_name, updated, id, true)?;
        set_story_source_xml(self, &part_name, updated)?;
        let stories = self.stories()?;
        let rebase = |position: &StoryRunPosition,
                      endpoint: &(String, usize)|
         -> Result<StoryRunPosition> {
            let original = position.location.story();
            let story = stories
                .iter()
                .find(|story| {
                    story.kind == original.kind
                        && story.part_name == original.part_name
                        && story.owner_index == original.owner_index
                })
                .ok_or_else(|| Error::Other("comment move destination story disappeared".into()))?;
            let expected = if endpoint.0 == part_name {
                map_offset(endpoint.1)?
            } else {
                endpoint.1
            };
            let (source, owner) = self.story_source_and_owner(story)?;
            let qualified =
                story_range_paragraph_spans(source.xml.as_ref(), &[(story.clone(), owner.clone())])
                    .ok()
                    .filter(|spans| spans.len() == 1)
                    .and_then(|mut spans| spans.pop());
            let mut found = None;
            if let Some(paragraphs) = qualified {
                for (location, span) in paragraphs {
                    if span.start == expected {
                        if found.is_some() {
                            return Err(Error::Other("ambiguous comment move destination".into()));
                        }
                        found = Some(location.index_path);
                    }
                }
            } else {
                // Preserve the original lookup and its early error ordering.
                for (index, item) in scan_story_items(source.xml.as_ref(), &owner, None)?
                    .iter()
                    .enumerate()
                {
                    let paths = match item.kind {
                        StoryItemKind::Paragraph => vec![(vec![index], item.full.start)],
                        StoryItemKind::ContentControl => {
                            scan_story_control_paragraphs(source.xml.as_ref(), item)?
                                .iter()
                                .enumerate()
                                .map(|(child, span)| (vec![index, child], span.start))
                                .collect()
                        }
                        _ => Vec::new(),
                    };
                    for (path, offset) in paths {
                        if offset == expected {
                            if found.is_some() {
                                return Err(Error::Other(
                                    "ambiguous comment move destination".into(),
                                ));
                            }
                            found = Some(path);
                        }
                    }
                }
            }
            let path = found.ok_or_else(|| {
                Error::Other("comment move destination paragraph disappeared".into())
            })?;
            let removed_before_boundary = endpoint.0 == part_name
                && endpoint.1 == reference_paragraph
                && position.run_index > reference_index
                && edits.iter().any(|span| {
                    span.start <= reference_span.start && reference_span.end <= span.end
                });
            Ok(StoryRunPosition {
                location: ContentLocation::new(story.clone(), StoryItemKind::Paragraph, path),
                run_index: position.run_index - usize::from(removed_before_boundary),
            })
        };
        let rebound = StoryRunRange {
            start: rebase(&target.start, &endpoints[0])?,
            end: rebase(&target.end, &endpoints[1])?,
        };
        self.anchor_story_range(&rebound, RangeAnchor::Comment(id), "comment", false)?;
        // Replace only the uniquely synthesized selected reference run. The
        // original run carries its namespace closure, formatting and raw attrs.
        self.restore_comment_reference_run(id, reference)
    }

    pub(crate) fn remove_comment_source_for_move(
        &mut self,
        entry: &crate::comments::StoryRangeRef,
    ) -> Result<Vec<u8>> {
        let (part, xml, selected) = self.story_range_marker_spans(entry, true)?;
        let (edits, reference, _) = comment_move_source_edits(&xml, &selected)?;
        let mut updated = xml;
        for span in edits.iter().rev() {
            updated.drain(span.clone());
        }
        let updated = self.comment_move_source_with_namespaces(
            &part,
            updated,
            match entry.kind() {
                crate::comments::StoryRangeKind::Comment { id } => *id,
                _ => return Err(Error::Other("expected comment move source".into())),
            },
            true,
        )?;
        set_story_source_xml(self, &part, updated)?;
        Ok(reference)
    }

    pub(crate) fn restore_comment_reference_run(
        &mut self,
        id: i32,
        reference: Vec<u8>,
    ) -> Result<()> {
        let selected = self.checked_movable_comment(id)?;
        self.splice_comment_reference_run(selected.range().end.location.story(), id, reference)
    }

    /// Replace the reference run that anchoring synthesized for comment `id`
    /// in `story` with the original one. A reply moving with its thread names
    /// the story its reply authoring wrote, without another selection.
    pub(crate) fn splice_comment_reference_run(
        &mut self,
        story: &crate::StoryId,
        id: i32,
        reference: Vec<u8>,
    ) -> Result<()> {
        let (source, owner) = self.story_source_and_owner(story)?;
        let part = source.part_name.clone();
        let span = comment_reference_run_span(source.xml.as_ref(), &owner.full, id)?;
        let mut xml = source.xml.into_owned();
        xml.splice(span, reference);
        let xml = self.comment_move_source_with_namespaces(&part, xml, id, false)?;
        set_story_source_xml(self, &part, xml)
    }

    // Source discovery retains the established canonical fingerprint axis.
    // Before a move replaces that source, replay its original qualified nested
    // declarations so the replacement does not erase preserved raw aliases.
    fn comment_move_source_with_namespaces(
        &self,
        part: &str,
        xml: Vec<u8>,
        id: i32,
        source_removal: bool,
    ) -> Result<Vec<u8>> {
        if part != self.doc_part_name {
            return Ok(xml);
        }
        let original = self
            .package
            .get_part(part)
            .ok_or_else(|| Error::Other("comment move main source is missing".into()))?;
        // Apply the same proved source edits to the original namespace authority.
        // Whole transported runs disappear, while mixed source runs keep their
        // declarations and every non-reference child in their original order.
        let mut remaining_source = original.to_vec();
        if source_removal {
            let selected = crate::comments::comment_source_markers(original)?
                .into_iter()
                .filter(|marker| marker.id == id)
                .map(|marker| marker.span)
                .collect::<Vec<_>>();
            if selected.len() != 3 {
                return Err(Error::Other(
                    "comment move original source lacks one qualified pair and reference".into(),
                ));
            }
            let (edits, _, _) = comment_move_source_edits(original, &selected)?;
            for span in edits.into_iter().rev() {
                remaining_source.drain(span);
            }
        }
        let original = remaining_source.as_slice();
        let mut owners = nested_modeled_namespace_owners(original)?;
        let original_spans = modeled_owner_spans(original)?;
        let spans = modeled_owner_spans(&xml)?;
        for owner in &mut owners {
            let has_pair = |source: &[u8], span: &ModeledOwnerSpan| -> Result<bool> {
                let mut scope = span
                    .bindings
                    .iter()
                    .map(|(name, uri)| (namespace_prefix(name), uri.clone()))
                    .collect::<BTreeMap<_, _>>();
                for (name, uri) in &owner.declarations {
                    let prefix = namespace_prefix(name);
                    scope.insert(prefix, uri.clone());
                }
                let closed =
                    close_content_fragment_namespaces(&source[span.start..span.end], &scope)?;
                Ok(CT_P::comment_range_text_from_source(&closed, id)?.is_some())
            };
            let mut original_selected = false;
            for span in original_spans
                .iter()
                .filter(|span| span.local_name == owner.local_name)
            {
                let markers = candidate_namespace_owner_markers(
                    &original[span.start..span.end],
                    &owner.declarations,
                    &span.bindings,
                    &owner.markers,
                )?;
                if marker_multiset_matches(&owner.markers, &markers, true)
                    && has_pair(original, span)?
                {
                    original_selected = true;
                }
            }
            let mut matching = Vec::new();
            for span in spans
                .iter()
                .filter(|span| span.local_name == owner.local_name)
            {
                let markers = candidate_namespace_owner_markers(
                    &xml[span.start..span.end],
                    &owner.declarations,
                    &span.bindings,
                    &owner.markers,
                )?;
                if marker_multiset_matches(&owner.markers, &markers, true) {
                    matching.push(span);
                }
            }
            let selected = matching
                .iter()
                .map(|span| has_pair(&xml, span))
                .collect::<Result<Vec<_>>>()?;
            if original_selected || selected.iter().any(|selected| *selected) {
                let [span] = matching.as_slice() else {
                    return Err(Error::Other(
                        "comment move has ambiguous retained namespace owner".into(),
                    ));
                };
                owner.snapshot = logical_owner_snapshot(
                    &xml[span.start..span.end],
                    &span.bindings,
                    &owner.declarations,
                )?;
            }
        }
        replay_nested_namespace_declarations(&xml, &owners)
    }

    fn story_range_marker_spans(
        &self,
        entry: &crate::comments::StoryRangeRef,
        move_comment_reference: bool,
    ) -> Result<(String, Vec<u8>, Vec<Range<usize>>)> {
        use crate::comments::StoryRangeKind;
        let (source, owner) = self.story_source_and_owner(entry.range().start.location.story())?;
        let part_name = source.part_name.clone();
        let xml = source.xml.as_ref();
        let qualified = story_range_paragraph_spans(
            xml,
            &[(entry.range().start.location.story().clone(), owner.clone())],
        )
        .ok()
        .filter(|spans| spans.len() == 1)
        .and_then(|mut spans| spans.pop());
        let paragraphs = if let Some(paragraphs) = qualified {
            paragraphs
                .into_iter()
                .map(|(_, span)| span)
                .collect::<Vec<_>>()
        } else {
            let mut paragraphs = Vec::new();
            for item in scan_story_items(xml, &owner, None)? {
                match item.kind {
                    StoryItemKind::Paragraph => paragraphs.push(item.full),
                    StoryItemKind::ContentControl => {
                        paragraphs.extend(scan_story_control_paragraphs(xml, &item)?);
                    }
                    _ => {}
                }
            }
            paragraphs
        };
        let (start_tag, end_tag, identity) = match entry.kind() {
            StoryRangeKind::Bookmark { id, .. } => (
                b"bookmarkStart".as_slice(),
                b"bookmarkEnd".as_slice(),
                id.to_string(),
            ),
            StoryRangeKind::Comment { id } => (
                b"commentRangeStart".as_slice(),
                b"commentRangeEnd".as_slice(),
                id.to_string(),
            ),
            StoryRangeKind::Permission { id, .. } => (
                b"permStart".as_slice(),
                b"permEnd".as_slice(),
                id.to_string(),
            ),
            StoryRangeKind::Proofing { kind } => {
                (b"proofErr".as_slice(), b"proofErr".as_slice(), kind.clone())
            }
        };
        // Source-order disjoint ranges admit one forward membership cursor.
        // Unqualified inventories keep the original containment predicate.
        let ordered = paragraphs
            .iter()
            .all(|span| span.start <= span.end && span.end <= xml.len())
            && paragraphs
                .windows(2)
                .all(|pair| pair[0].end <= pair[1].start);
        let mut paragraph_cursor = 0usize;
        let mut paragraph_contains = |offset: usize| {
            if !ordered {
                return paragraphs
                    .iter()
                    .any(|paragraph| paragraph.contains(&offset));
            }
            while let Some(paragraph) = paragraphs.get(paragraph_cursor) {
                if offset < paragraph.end {
                    return paragraph.start <= offset;
                }
                paragraph_cursor += 1;
            }
            false
        };
        let mut reader = NsReader::from_reader(xml);
        reader.config_mut().trim_text(false);
        let mut ranges = Vec::new();
        let mut ordinal = 0usize;
        let mut stack = Vec::<(bool, Vec<u8>, usize)>::new();
        let mut buffer = Vec::new();
        loop {
            let before = reader.buffer_position() as usize;
            let (namespace, event) = match reader.read_resolved_event_into(&mut buffer) {
                Ok(event) => event,
                Err(error) => {
                    return Err(Error::Other(format!("range removal scan failed: {error}")));
                }
            };
            let is_word = word_element(&namespace);
            drop(namespace);
            let after = reader.buffer_position() as usize;
            let hidden = stack.iter().any(|(word, name, start)| {
                *word
                    && matches!(
                        name.as_slice(),
                        b"del" | b"moveFrom" | b"smartTag" | b"customXml" | b"fldSimple"
                    )
                    || *word && name == b"txbxContent" && *start > owner.full.start
            });
            if let Event::End(_) = &event {
                stack.pop();
            }
            if let Event::Start(element) = &event {
                stack.push((is_word, element.local_name().as_ref().to_vec(), before));
            }
            let element = match &event {
                Event::Start(element) | Event::Empty(element)
                    if !hidden
                        && is_word
                        && matches!(
                            element.local_name().as_ref(),
                            b"bookmarkStart"
                                | b"bookmarkEnd"
                                | b"commentRangeStart"
                                | b"commentRangeEnd"
                                | b"permStart"
                                | b"permEnd"
                                | b"proofErr"
                                | b"commentReference"
                        )
                        && paragraph_contains(before) =>
                {
                    element
                }
                Event::Eof => break,
                _ => {
                    buffer.clear();
                    continue;
                }
            };
            let mut id_value = None;
            let mut type_value = None;
            for attribute in element.attributes() {
                let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
                let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                if word_element(&namespace) && matches!(local.as_ref(), b"id" | b"type") {
                    let value = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                        .map_err(|error| Error::Other(error.to_string()))?;
                    if local.as_ref() == b"id" {
                        id_value = Some(value.into_owned());
                    } else {
                        type_value = Some(value.into_owned());
                    }
                }
            }
            let name = element.local_name();
            let name = name.as_ref();
            let paired = matches!(
                name,
                b"bookmarkStart"
                    | b"bookmarkEnd"
                    | b"commentRangeStart"
                    | b"commentRangeEnd"
                    | b"permStart"
                    | b"permEnd"
            ) || name == b"proofErr"
                && matches!(
                    type_value.as_deref(),
                    Some("spellStart" | "spellEnd" | "gramStart" | "gramEnd")
                );
            let this_ordinal = paired.then(|| {
                let current = ordinal;
                ordinal += 1;
                current
            });
            let matched = if name == b"commentReference" {
                move_comment_reference
                    && matches!(entry.kind(), StoryRangeKind::Comment { .. })
                    && id_value.as_deref() == Some(identity.as_str())
            } else {
                (name == start_tag || name == end_tag)
                    && this_ordinal.is_some_and(|index| {
                        index == entry.start_ordinal || index == entry.end_ordinal
                    })
                    && if matches!(entry.kind(), StoryRangeKind::Proofing { .. }) {
                        type_value.as_deref() == Some(format!("{identity}Start").as_str())
                            || type_value.as_deref() == Some(format!("{identity}End").as_str())
                    } else {
                        id_value.as_deref() == Some(identity.as_str())
                    }
            };
            if matched {
                let end = if matches!(event, Event::Empty(_)) {
                    after
                } else {
                    story_element_end(xml, before)?
                };
                ranges.push(before..end);
            }
            buffer.clear();
        }
        let expected =
            if move_comment_reference && matches!(entry.kind(), StoryRangeKind::Comment { .. }) {
                3
            } else {
                2
            };
        if ranges.len() != expected {
            return Err(Error::Other(format!(
                "range marker identity {} has {} selected elements, expected {expected}",
                identity,
                ranges.len()
            )));
        }
        Ok((part_name, xml.to_vec(), ranges))
    }

    pub(crate) fn remove_story_range_markers(
        &mut self,
        entry: &crate::comments::StoryRangeRef,
        move_comment_reference: bool,
    ) -> Result<()> {
        let (part_name, mut updated, ranges) =
            self.story_range_marker_spans(entry, move_comment_reference)?;
        for range in ranges.into_iter().rev() {
            updated.splice(range, []);
        }
        set_story_source_xml(self, &part_name, updated)
    }

    fn story_item_source<'a>(
        &'a self,
        location: &ContentLocation,
    ) -> Result<(StorySource<'a>, StoryItemSpan)> {
        let (source, owner, layout) = self.story_source_owner_and_layout(&location.story, true)?;
        let item = Self::story_item_span(source.xml.as_ref(), &owner, layout.as_ref(), location)?;
        Ok((source, item))
    }

    fn story_item_span(
        xml: &[u8],
        owner: &StoryOwnerSpan,
        layout: Option<&MainStoryLayout>,
        location: &ContentLocation,
    ) -> Result<StoryItemSpan> {
        if location.index_path.len() != 1 {
            return Err(StoryError::InvalidPath {
                path: location.index_path.clone(),
            }
            .into());
        }
        let index = location.index_path[0];
        let (item, len) = story_owner_item(xml, owner, layout, index)?;
        let item = item.ok_or(StoryError::OutOfBounds { index, len })?;
        if item.kind != location.item_kind {
            return Err(StoryError::KindMismatch {
                expected: location.item_kind,
                actual: item.kind,
            }
            .into());
        }
        Ok(item)
    }

    pub(crate) fn location_is_end(location: &ContentLocation) -> bool {
        location.is_end
    }

    pub(crate) fn owned_story_parts(&self) -> Result<Vec<Vec<u8>>> {
        Ok(self
            .story_sources()?
            .into_iter()
            .map(|source| source.xml.into_owned())
            .collect())
    }

    pub(crate) fn copy_indexed_story_item(
        &self,
        location: &ContentLocation,
        index_path: &[usize],
    ) -> Result<(String, Vec<u8>, std::ops::Range<usize>, bool)> {
        let mut located = location.clone();
        located.index_path = index_path.to_vec();
        located.is_end = false;
        let (source, item) = if located.item_kind == StoryItemKind::Paragraph {
            self.story_range_paragraph_source(&located)?
        } else {
            self.story_item_source(&located)?
        };
        Ok((
            source.part_name.clone(),
            source.xml.into_owned(),
            item.full,
            item.direct_owner_child,
        ))
    }

    pub(crate) fn write_story_part(&mut self, part_name: &str, xml: Vec<u8>) -> Result<()> {
        set_story_source_xml(self, part_name, xml)
    }

    pub(crate) fn story_owner_span(
        &self,
        story: &StoryId,
    ) -> Result<(Vec<u8>, std::ops::Range<usize>)> {
        let (source, owner) = self.story_source_and_owner(story)?;
        Ok((source.xml.into_owned(), owner.full))
    }

    /// Refresh an owner by its complete kind/part/index selector after a
    /// mutation, discovering every story before qualifying the selected span.
    pub(crate) fn refreshed_story_owner_span(
        &self,
        previous: &StoryId,
    ) -> Result<Option<(Vec<u8>, Range<usize>)>> {
        let (sources, content, children) = self.spanned_story_sources()?;
        let main = self
            .main_story_layout(&sources[0].xml, &content, &children, false)
            .ok();
        let borrowed = sources
            .iter()
            .map(|source| StorySource {
                root_kind: source.root_kind,
                part_name: source.part_name.clone(),
                xml: Cow::Borrowed(source.xml.as_ref()),
            })
            .collect();
        let Some(refreshed) =
            Self::stories_in_sources(borrowed, main)?
                .into_iter()
                .find(|candidate| {
                    candidate.kind == previous.kind
                        && candidate.part_name == previous.part_name
                        && candidate.owner_index == previous.owner_index
                })
        else {
            return Ok(None);
        };
        let (source, owner, _) =
            self.story_source_owner_and_layout_in(&refreshed, false, sources, &content, &children)?;
        Ok(Some((source.xml.into_owned(), owner.full)))
    }

    /// Accepted content controls, non-preserved carriers, and text-box owners.
    ///
    /// Ranges come from the story-item scan. Controls are already deduplicated
    /// by part and byte span. Text-box ranges identify legacy fields that
    /// belong after the non-textbox inventory.
    #[allow(clippy::type_complexity)]
    pub(crate) fn accepted_form_regions(
        &self,
    ) -> Result<(
        Vec<(String, std::ops::Range<usize>)>,
        Vec<(String, std::ops::Range<usize>)>,
        Vec<(String, std::ops::Range<usize>)>,
    )> {
        let mut controls = Vec::new();
        let mut carriers = Vec::new();
        let mut text_boxes = Vec::new();
        let mut seen_controls = HashSet::new();
        // Story discovery still names the accepted parts. Offsets come from the
        // package bytes those parts will be patched in. The body model serializes
        // separately and its offsets are not the physical file.
        for source in self.story_sources()? {
            let Some(xml) = self.package.get_part(&source.part_name) else {
                continue;
            };
            let part = source.part_name.clone();
            for owner in scan_story_owners(xml, source.root_kind)? {
                if owner.kind == StoryKind::TextBox {
                    text_boxes.push((part.clone(), owner.full.clone()));
                }
                let items = scan_story_items(xml, &owner, None)?;
                for item in items {
                    if item.kind == StoryItemKind::ContentControl
                        && seen_controls.insert((part.clone(), item.full.start, item.full.end))
                    {
                        controls.push((part.clone(), item.full.clone()));
                    }
                    if owner.kind != StoryKind::TextBox && item.kind != StoryItemKind::PreservedNode
                    {
                        carriers.push((part.clone(), item.full.clone()));
                    }
                }
            }
        }
        Ok((controls, carriers, text_boxes))
    }

    fn story_range_paragraph_source<'a>(
        &'a self,
        location: &ContentLocation,
    ) -> Result<(StorySource<'a>, StoryItemSpan)> {
        if location.index_path.len() == 1 {
            return self.story_item_source(location);
        }
        if location.index_path.len() != 2 || location.item_kind != StoryItemKind::Paragraph {
            return Err(StoryError::InvalidPath {
                path: location.index_path.clone(),
            }
            .into());
        }
        let (source, owner, layout) = self.story_source_owner_and_layout(&location.story, true)?;
        let items = story_owner_items(source.xml.as_ref(), &owner, layout.as_ref())?;
        let control_index = location.index_path[0];
        let control = items.get(control_index).ok_or(StoryError::OutOfBounds {
            index: control_index,
            len: items.len(),
        })?;
        if control.kind != StoryItemKind::ContentControl {
            return Err(StoryError::KindMismatch {
                expected: StoryItemKind::ContentControl,
                actual: control.kind,
            }
            .into());
        }
        let paragraphs = story_control_paragraphs(source.xml.as_ref(), control, layout.as_ref())?;
        let paragraph_index = location.index_path[1];
        let full = paragraphs
            .get(paragraph_index)
            .cloned()
            .ok_or(StoryError::OutOfBounds {
                index: paragraph_index,
                len: paragraphs.len(),
            })?;
        Ok((
            source,
            StoryItemSpan {
                kind: StoryItemKind::Paragraph,
                scan: full.clone(),
                full,
                direct_owner_child: false,
                complex_field: false,
                complex_ancestors: Vec::new(),
                sdt_context: None,
            },
        ))
    }

    /// Traverse one story's supported content without constructing a second
    /// document tree.
    pub fn story_items<'a>(&'a self, story: &StoryId) -> Result<Vec<StoryItemRef<'a>>> {
        let (source, owner, layout) = self.story_source_owner_and_layout(story, true)?;
        let spans = story_owner_items(source.xml.as_ref(), &owner, layout.as_ref())?;
        Ok(spans
            .into_iter()
            .enumerate()
            .map(|(index, item)| StoryItemRef {
                document: self,
                location: ContentLocation {
                    story: story.clone(),
                    item_kind: item.kind,
                    index_path: vec![index],
                    is_end: false,
                },
            })
            .collect())
    }

    pub(crate) fn patch_story_paragraph_field_sources(
        &mut self,
        location: &ContentLocation,
        replacements: &[(Vec<u8>, Vec<u8>)],
    ) -> Result<()> {
        // Earlier writes may have changed the owning story's fingerprint.
        // Refresh only that fingerprint, keeping its physical part and ordinal.
        let mut location = location.clone();
        location.story = self
            .stories()?
            .into_iter()
            .find(|story| {
                story.kind == location.story.kind
                    && story.part_name == location.story.part_name
                    && story.owner_index == location.story.owner_index
            })
            .ok_or_else(|| {
                Error::Other("field cache owner disappeared during staging".to_owned())
            })?;
        let (source, paragraph) = self.story_range_paragraph_source(&location)?;
        let part_name = source.part_name.clone();
        let mut xml = source.xml.into_owned();
        let mut edits = Vec::new();
        let mut cursor = 0;
        for (original, replacement) in replacements {
            if original.is_empty() {
                return Err(Error::Other("empty field source identity".to_owned()));
            }
            let offset = crate::field::find_typed_field_source(
                &xml,
                paragraph.full.start,
                paragraph.full.end,
                original,
                cursor,
            )?
            .ok_or_else(|| {
                Error::Other("field source no longer belongs to its checked paragraph".to_owned())
            })?;
            let start = paragraph.full.start + offset;
            cursor = offset + original.len();
            edits.push((start..(start + original.len()), replacement));
        }
        for (range, replacement) in edits.into_iter().rev() {
            xml.splice(range, replacement.iter().copied());
        }
        set_story_source_xml(self, &part_name, xml)?;
        self.invalidate_layout();
        Ok(())
    }

    pub(crate) fn text_box_cache_paragraphs(&self) -> Result<Vec<TextBoxCacheParagraph>> {
        let (sources, paragraphs) = self.story_range_paragraph_sources()?;
        let text_boxes = sources
            .iter()
            .map(|source| {
                Ok(scan_story_owners(source.xml.as_ref(), source.root_kind)?
                    .into_iter()
                    .filter(|owner| owner.kind == StoryKind::TextBox)
                    .collect::<Vec<_>>())
            })
            .collect::<Result<Vec<_>>>()?;
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for (source_index, _, paragraph, location, xml) in paragraphs {
            if !matches!(
                location.story.kind,
                StoryKind::TextBox | StoryKind::TableCell
            ) {
                continue;
            }
            let source = &sources[source_index];
            let text_box = text_boxes[source_index]
                .iter()
                .filter(|owner| {
                    owner.full.start <= paragraph.start && owner.full.end >= paragraph.end
                })
                .min_by_key(|owner| owner.full.len());
            if let Some(text_box) = text_box
                && seen.insert((source.part_name.clone(), paragraph.start, paragraph.end))
            {
                result.push(TextBoxCacheParagraph {
                    location,
                    xml,
                    owner_index: text_box.owner_index,
                    paragraph_index: paragraph.start,
                });
            }
        }
        result.sort_by(|left, right| {
            (
                left.location.story.part_name.as_str(),
                left.owner_index,
                left.paragraph_index,
            )
                .cmp(&(
                    right.location.story.part_name.as_str(),
                    right.owner_index,
                    right.paragraph_index,
                ))
        });
        let mut ordinals = HashMap::<(String, usize), usize>::new();
        for paragraph in &mut result {
            let ordinal = ordinals
                .entry((
                    paragraph.location.story.part_name.clone(),
                    paragraph.owner_index,
                ))
                .or_default();
            paragraph.paragraph_index = *ordinal;
            *ordinal += 1;
        }
        Ok(result)
    }

    /// Snapshot one checked range paragraph, including a paragraph within a block control.
    /// This does not expand the ordinary story-item inventory.
    #[doc(hidden)]
    pub fn story_range_paragraph_snapshot(
        &self,
        location: &ContentLocation,
    ) -> Result<StoryItemSnapshot> {
        let (source, item) = self.story_range_paragraph_source(location)?;
        let scope = story_namespace_scope_at(source.xml.as_ref(), item.full.start)?;
        let xml = close_content_fragment_namespaces(&source.xml.as_ref()[item.full], &scope)?;
        let text = CT_P::from_xml_fragment(&xml)?.accepted_text();
        let containing = if let [control, _] = location.index_path() {
            ContentLocation::new(
                location.story().clone(),
                StoryItemKind::ContentControl,
                vec![*control],
            )
        } else {
            location.clone()
        };
        let direct_body_index = StoryItemRef {
            document: self,
            location: containing,
        }
        .direct_body_index()?;
        Ok(StoryItemSnapshot {
            location: location.clone(),
            direct_body_index,
            direct_child: location.index_path().len() == 1,
            text: Some(text),
            xml,
        })
    }

    // Guard the unique newly placed pair against the actual main source after
    // existing namespace replay. Only this unpublished candidate's document part
    // is flushed, leaving pending comment models and identifier state untouched.
    pub(crate) fn comment_literal_range_text(&mut self, id: i32) -> Result<Option<String>> {
        let xml = self.comment_move_source_with_namespaces(
            &self.doc_part_name,
            self.document.to_xml()?,
            id,
            false,
        )?;
        let part = self.doc_part_name.clone();
        set_story_source_xml(self, &part, xml)?;
        self.flush_document_to_package()?;
        let source = self.package.get_part(&self.doc_part_name).ok_or_else(|| {
            Error::Other("comment literal guard has no main document source".into())
        })?;
        Ok(CT_P::comment_range_text_from_source(source, id)?)
    }

    // Keep canonical locations and fingerprints while restoring only the proved
    // namespace context used by rich comment projection. Physical event offsets
    // qualify every owner before a restored declaration can reinterpret a name.
    pub(crate) fn namespace_restored_main_source(&self) -> Result<Vec<u8>> {
        let canonical = self.main_part_with_body_spans()?.0;
        let original = self
            .package
            .get_part(&self.doc_part_name)
            .ok_or_else(|| Error::Other("rich comment projection has no main source".into()))?;
        let mut namespace_owners = nested_modeled_namespace_owners(original)?;
        if namespace_owners.is_empty() {
            return Ok(canonical);
        }
        let canonical_spans = modeled_owner_spans(&canonical)?;
        let mut needed = Vec::new();
        for owner in namespace_owners.drain(..) {
            let original_scope = story_namespace_scope_at(original, owner.source.start)?;
            let original_owner = close_content_fragment_namespaces(
                &original[owner.source.clone()],
                &original_scope,
            )?;
            let mut redundant = 0;
            for target in canonical_spans.iter().filter(|span| {
                span.local_name == owner.local_name
                    && owner
                        .declarations
                        .iter()
                        .all(|declaration| span.declarations.contains(declaration))
            }) {
                let scope = story_namespace_scope_at(&canonical, target.start)?;
                let target_owner = close_content_fragment_namespaces(
                    &canonical[target.start..target.end],
                    &scope,
                )?;
                // Full owner payload, names and attributes prove the unique
                // source counterpart. Only redundant namespace declarations may
                // differ, with every resolved namespace fact still identical.
                if let Some((_, changed)) =
                    comment_namespace_source_boundaries(&original_owner, &target_owner)?
                    && changed.is_empty()
                {
                    redundant += 1;
                }
            }
            if redundant != 1 {
                needed.push(owner);
            }
        }
        let restored = replay_nested_namespace_declarations(&canonical, &needed)?;
        if restored == canonical {
            return Ok(canonical);
        }
        let (boundaries, _) = comment_namespace_source_boundaries(&canonical, &restored)?
            .ok_or_else(|| {
                Error::Other("rich comment namespace replay changed physical source".into())
            })?;
        let owners = scan_story_owners(&canonical, StoryKind::Body)?;
        let restored_owners = scan_story_owners(&restored, StoryKind::Body)?;
        if owners.len() != restored_owners.len()
            || owners.iter().any(|owner| {
                !restored_owners.iter().any(|other| {
                    other.kind == owner.kind
                        && other.owner_index == owner.owner_index
                        && boundaries.get(&owner.full.start) == Some(&other.full.start)
                        && boundaries.get(&owner.full.end) == Some(&other.full.end)
                })
            })
        {
            return Err(Error::Other(
                "rich comment namespace replay changes story ownership".into(),
            ));
        }
        Ok(restored)
    }

    /// Snapshot advisory paragraph values once after checked source traversal.
    pub(crate) fn comment_paragraph_read(&self) -> CommentParagraphRead<'_> {
        let scans = self
            .main_body_scans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let blocks =
            offered_canonical_blocks(scans.serialized.as_ref().or(self.main_part_blocks.as_ref()))
                .map_or(&[][..], |record| record.blocks.as_slice());
        CommentParagraphRead::from_blocks(&self.doc_part_name, blocks)
    }

    /// The checked range paragraphs in story order, each with its flow, as
    /// the source index and flow that `story_range_paragraph_sources` gives,
    /// and its offset in that source, which orders the paragraphs of a flow.
    #[allow(clippy::type_complexity)] // Located paragraphs with their flow and source order.
    pub(crate) fn comment_story_range_paragraphs(
        &self,
    ) -> Result<Vec<((usize, usize), usize, ContentLocation, Vec<u8>)>> {
        let (_, paragraphs) = self.story_range_paragraph_sources()?;
        let mut paragraphs = paragraphs
            .into_iter()
            .map(|(source, flow, span, location, xml)| ((source, flow), span.start, location, xml))
            .collect::<Vec<_>>();
        let canonical = self.document.to_xml()?;
        let restored = self.namespace_restored_main_source()?;
        if restored == canonical {
            return Ok(paragraphs);
        }
        let (boundaries, _) = comment_namespace_source_boundaries(&canonical, &restored)?
            .ok_or_else(|| {
                Error::Other("rich comment namespace replay changed physical source".into())
            })?;
        let owners = scan_story_owners(&canonical, StoryKind::Body)?
            .into_iter()
            .map(|owner| {
                let story = StoryId {
                    kind: owner.kind,
                    part_name: self.doc_part_name.clone(),
                    owner_index: owner.owner_index,
                    fingerprint: owner.fingerprint,
                };
                (story, owner)
            })
            .collect::<Vec<_>>();
        let mut paragraph_spans = HashMap::new();
        for (location, span) in story_range_paragraph_spans(&canonical, &owners)?
            .into_iter()
            .flatten()
        {
            if paragraph_spans.insert(location, span).is_some() {
                return Err(Error::Other(
                    "rich comment has duplicate physical paragraph path".into(),
                ));
            }
        }
        let scopes = story_namespace_scopes_at(
            &restored,
            paragraph_spans
                .values()
                .filter_map(|span| boundaries.get(&span.start).copied()),
        )?;
        for (_, _, location, xml) in &mut paragraphs {
            if location.story.part_name != self.doc_part_name {
                continue;
            }
            let span = paragraph_spans.get(location).ok_or_else(|| {
                Error::Other("rich comment paragraph has no checked physical path".into())
            })?;
            let start = *boundaries.get(&span.start).ok_or_else(|| {
                Error::Other("rich comment paragraph has no physical namespace boundary".into())
            })?;
            let end = *boundaries.get(&span.end).ok_or_else(|| {
                Error::Other("rich comment paragraph has no physical namespace boundary".into())
            })?;
            let scope = scopes.get(&start).ok_or_else(|| {
                Error::Other("rich comment paragraph namespace scope was not inventoried".into())
            })?;
            *xml = close_content_fragment_namespaces(&restored[start..end], scope)?;
        }
        Ok(paragraphs)
    }

    #[doc(hidden)]
    pub fn story_range_paragraphs(&self) -> Result<Vec<(ContentLocation, Vec<u8>)>> {
        Ok(self
            .story_range_located_paragraphs()?
            .into_iter()
            .map(|(_, _, _, location, xml)| (location, xml))
            .collect())
    }

    /// Owned paragraph projection from one source inventory.
    ///
    /// The private source list stays inside this module. Callers receive the
    /// source index, flow, raw span, location and namespace-closed paragraph.
    #[allow(clippy::type_complexity)]
    pub(crate) fn story_range_located_paragraphs(
        &self,
    ) -> Result<Vec<(usize, usize, Range<usize>, ContentLocation, Vec<u8>)>> {
        let (_sources, paragraphs) = self.story_range_paragraph_sources()?;
        Ok(paragraphs)
    }

    /// List every checked range paragraph in story order, with the index of
    /// its source, its flow and its span there, after building each package
    /// source once.
    ///
    /// The flow is the start offset of the paragraph's owner, or for a table
    /// cell that of the innermost other owner holding it, so the paragraphs of
    /// one flow are one story with its table cells, in source order.
    ///
    /// Stories keep the identities and order of [`Self::stories`]. Each source
    /// is scanned for its owners once and its paragraph scopes are inventoried
    /// in one pass.
    #[allow(clippy::type_complexity)] // Sources and their located paragraph spans form one inventory.
    fn story_range_paragraph_sources(
        &self,
    ) -> Result<(
        Vec<StorySource<'_>>,
        Vec<(usize, usize, Range<usize>, ContentLocation, Vec<u8>)>,
    )> {
        let sources = self.story_sources()?;
        let mut paragraphs = Vec::new();
        let mut seen = HashSet::new();
        let mut story_index = 0usize;
        for (source_index, source) in sources.iter().enumerate() {
            let source_xml = source.xml.as_ref();
            let mut owners = Vec::new();
            let mut flows = Vec::new();
            // Owners come in start order, so the open non-cell owners nest.
            let mut holders = Vec::<Range<usize>>::new();
            for owner in scan_story_owners(source_xml, source.root_kind)? {
                while holders
                    .last()
                    .is_some_and(|holder| holder.end <= owner.full.start)
                {
                    holders.pop();
                }
                let flow = match holders.last() {
                    Some(holder) if owner.kind == StoryKind::TableCell => holder.start,
                    _ => owner.full.start,
                };
                if owner.kind != StoryKind::TableCell {
                    holders.push(owner.full.clone());
                }
                let identity = (owner.kind, source.part_name.clone(), owner.owner_index);
                if !seen.insert(identity) {
                    continue;
                }
                let story = StoryId {
                    kind: owner.kind,
                    part_name: source.part_name.clone(),
                    owner_index: owner.owner_index,
                    fingerprint: owner.fingerprint,
                };
                owners.push((story, owner));
                flows.push(flow);
            }
            let spans = story_range_paragraph_spans(source_xml, &owners)?;
            let scopes = story_namespace_scopes_at(
                source_xml,
                spans.iter().flatten().map(|(_, span)| span.start),
            )?;
            for (story_spans, flow) in spans.into_iter().zip(flows) {
                for (location, span) in story_spans {
                    let scope = scopes.get(&span.start).ok_or_else(|| {
                        Error::Other("story paragraph namespace scope was not inventoried".into())
                    })?;
                    let xml = close_content_fragment_namespaces(&source_xml[span.clone()], scope)?;
                    paragraphs.push((story_index, source_index, flow, span, location, xml));
                }
                story_index += 1;
            }
        }
        paragraphs.sort_by_key(|(story_index, _, _, span, _, _)| (*story_index, span.start));
        Ok((
            sources,
            paragraphs
                .into_iter()
                .map(|(_, source_index, flow, span, location, xml)| {
                    (source_index, flow, span, location, xml)
                })
                .collect(),
        ))
    }

    /// The body story location of the direct body child at `slot`.
    /// `Ok(None)` is absence. A scan that cannot be proved uses the whole
    /// story snapshot lookup. The error is that lookup's error.
    #[doc(hidden)]
    pub fn direct_body_item_location(&self, slot: usize) -> Result<Option<ContentLocation>> {
        let (xml, content, children) = self.main_part_with_body_spans()?;
        self.direct_body_item_location_in(&xml, &content, &children, slot)
    }

    fn direct_body_item_location_in(
        &self,
        xml: &[u8],
        content: &Range<usize>,
        children: &[Range<usize>],
        slot: usize,
    ) -> Result<Option<ContentLocation>> {
        let Ok(layout) = self.main_story_layout(xml, content, children, true) else {
            return full_direct_body_item_location(xml, &self.doc_part_name, slot);
        };
        let owner = layout
            .owner(xml, StoryKind::Body, 0)?
            .ok_or_else(|| Error::Other("main document body owner was not found".to_owned()))?;
        let story = StoryId {
            kind: StoryKind::Body,
            part_name: self.doc_part_name.clone(),
            owner_index: 0,
            fingerprint: owner.fingerprint,
        };
        let located = layout.direct_body_item(xml, slot, true).map(|item| {
            item.map(|(index, item)| ContentLocation::new(story, item.kind, vec![index]))
        });
        #[cfg(debug_assertions)]
        {
            let whole = full_direct_body_item_location(xml, &self.doc_part_name, slot);
            match (&located, &whole) {
                (Ok(actual), Ok(expected)) => {
                    assert_eq!(actual, expected, "direct body item location {slot}")
                }
                (Err(actual), Err(expected)) => assert_eq!(
                    actual.to_string(),
                    expected.to_string(),
                    "direct body item location {slot}"
                ),
                (Ok(actual), Err(expected)) => panic!(
                    "direct body item location {slot}: fast Ok {actual:?} hid full error {expected}"
                ),
                (Err(actual), Ok(expected)) => panic!(
                    "direct body item location {slot}: fast error {actual} but full Ok {expected:?}"
                ),
            }
        }
        located
    }

    /// Resolve a physical direct-body cell from the actual current main output.
    /// Current writer proof shares that output. Other cases keep the full lookup.
    #[doc(hidden)]
    pub fn direct_body_cell_story(
        &self,
        slot: usize,
        row: usize,
        cell: usize,
    ) -> Result<Option<StoryId>> {
        let (xml, content, children) = self.main_part_with_body_spans()?;
        let Some(location) = self.direct_body_item_location_in(&xml, &content, &children, slot)?
        else {
            return Ok(None);
        };
        // This proof describes the actual current writer bytes above. It is
        // not an input/read receipt and never grants scoped Source credit.
        let same_current_writer = {
            let scans = self
                .main_body_scans
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            offered_canonical_blocks(scans.serialized.as_ref())
                .is_some_and(|record| record.writer_bytes_match(&self.document, &xml) == Some(true))
        };
        if !same_current_writer || location.item_kind != StoryItemKind::Table || location.is_end {
            return self.story_table_cell(&location, row, cell).map(Some);
        }
        // The old second source preparation also collects related sources.
        // Preserve those relationship/part errors before owner/item lookup.
        let mut sources = vec![StorySource {
            root_kind: StoryKind::Body,
            part_name: self.doc_part_name.clone(),
            xml: Cow::Borrowed(xml.as_slice()),
        }];
        sources.extend(self.related_story_sources()?);
        let (source, owner, layout) = self.story_source_owner_and_layout_in(
            &location.story,
            true,
            sources,
            &content,
            &children,
        )?;
        let item = Self::story_item_span(source.xml.as_ref(), &owner, layout.as_ref(), &location)?;
        let owner =
            physical_table_cell_owner(source.xml.as_ref(), source.root_kind, &item, row, cell)?;
        Ok(Some(StoryId {
            kind: owner.kind,
            part_name: source.part_name,
            owner_index: owner.owner_index,
            fingerprint: owner.fingerprint,
        }))
    }

    /// Materialize every story item after building each package source once.
    ///
    /// Unlike repeated calls through [`StoryItemRef`], this owned projection
    /// serializes the main document once and scans each story owner once for
    /// the complete snapshot.
    pub fn story_item_snapshots(&self) -> Result<Vec<StoryItemSnapshot>> {
        self.collect_story_item_snapshots(false)
    }

    /// [`Document::story_item_snapshots`] with each item's XML standalone.
    ///
    /// Every item declares the namespaces in scope at its start, as
    /// [`Document::story_range_paragraph_snapshot`] does, so it parses alone.
    /// The exact-slice API stays [`Document::story_item_snapshots`].
    #[doc(hidden)]
    pub fn standalone_story_item_snapshots(&self) -> Result<Vec<StoryItemSnapshot>> {
        self.collect_story_item_snapshots(true)
    }

    fn collect_story_item_snapshots(&self, standalone: bool) -> Result<Vec<StoryItemSnapshot>> {
        let mut snapshots = Vec::new();
        let mut seen = HashSet::new();
        for source in self.story_sources()? {
            let owners = scan_story_owners(source.xml.as_ref(), source.root_kind)?;
            let owner_scopes = story_namespace_scopes_at(
                source.xml.as_ref(),
                owners.iter().map(|owner| owner.full.start),
            )?;
            let mut inventories = Vec::new();
            for owner in owners {
                let identity = (owner.kind, source.part_name.clone(), owner.owner_index);
                if !seen.insert(identity) {
                    continue;
                }
                let story = StoryId {
                    kind: owner.kind,
                    part_name: source.part_name.clone(),
                    owner_index: owner.owner_index,
                    fingerprint: owner.fingerprint,
                };
                let scope = owner_scopes.get(&owner.full.start).ok_or_else(|| {
                    Error::Other("story owner namespace scope was not inventoried".to_owned())
                })?;
                let items = scan_story_items_with_scope(source.xml.as_ref(), &owner, scope, None)?;
                let direct_items = if owner.kind == StoryKind::Body {
                    let mut direct = Vec::new();
                    for item in items.iter().filter(|item| item.direct_owner_child) {
                        if item.kind != StoryItemKind::PreservedNode
                            || !content_fragment_root_is_section_properties(
                                source.xml.as_ref(),
                                item,
                            )?
                        {
                            direct.push(item.clone());
                        }
                    }
                    direct
                } else {
                    Vec::new()
                };
                inventories.push((story, items, direct_items));
            }
            let item_scopes = story_namespace_scopes_at(
                source.xml.as_ref(),
                inventories.iter().flat_map(|(_, items, _)| {
                    items.iter().flat_map(|item| {
                        [item.scan.start]
                            .into_iter()
                            .chain((standalone && !item.complex_field).then_some(item.full.start))
                    })
                }),
            )?;
            for (story, items, direct_items) in inventories {
                let mut direct_cursor = 0usize;
                for (index, item) in items.iter().enumerate() {
                    while direct_items
                        .get(direct_cursor)
                        .is_some_and(|direct| direct.full.end <= item.full.start)
                    {
                        direct_cursor += 1;
                    }
                    let direct_body_index = direct_items.get(direct_cursor).and_then(|direct| {
                        (direct.full.start <= item.full.start && item.full.end <= direct.full.end)
                            .then_some(direct_cursor)
                    });
                    let scope = item_scopes.get(&item.scan.start).ok_or_else(|| {
                        Error::Other("story item namespace scope was not inventoried".to_owned())
                    })?;
                    let text =
                        story_item_text_with_scope(source.xml.as_ref(), item, scope, &items)?;
                    let xml = if item.complex_field {
                        complex_story_field_xml(source.xml.as_ref(), item)?
                    } else if standalone {
                        let scope = item_scopes.get(&item.full.start).ok_or_else(|| {
                            Error::Other(
                                "story item namespace scope was not inventoried".to_owned(),
                            )
                        })?;
                        close_content_fragment_namespaces(&source.xml[item.full.clone()], scope)?
                    } else {
                        source.xml[item.full.clone()].to_vec()
                    };
                    snapshots.push(StoryItemSnapshot {
                        location: ContentLocation {
                            story: story.clone(),
                            item_kind: item.kind,
                            index_path: vec![index],
                            is_end: false,
                        },
                        direct_body_index,
                        direct_child: item.direct_owner_child,
                        text,
                        xml,
                    });
                }
            }
        }
        Ok(snapshots)
    }

    fn story_link_info(
        &self,
        story: &StoryId,
        xml: &[u8],
        link: StoryLinkSpan,
        scope: &BTreeMap<String, String>,
    ) -> Result<LinkInfo> {
        let text_item = StoryItemSpan {
            kind: StoryItemKind::Paragraph,
            full: link.full.clone(),
            scan: link.full,
            direct_owner_child: false,
            complex_field: false,
            complex_ancestors: Vec::new(),
            sdt_context: None,
        };
        let text = story_item_text_with_scope(xml, &text_item, scope, &[])?.unwrap_or_default();
        let url = link
            .rel_id
            .as_deref()
            .map(|relationship_id| self.hyperlink_url_for_validated_story(story, relationship_id))
            .transpose()?;
        Ok(LinkInfo {
            text,
            url,
            anchor: link.anchor,
            rel_id: link.rel_id,
        })
    }

    /// Return one story's modeled hyperlinks in physical source order together
    /// with the checked location of the item that owns each hyperlink.
    pub fn story_links(&self, story: &StoryId) -> Result<Vec<(ContentLocation, LinkInfo)>> {
        let (source, owner) = self.story_source_and_owner(story)?;
        let links = story_link_spans(source.xml.as_ref(), &owner, story)?;
        let link_scopes = story_namespace_scopes_at(
            source.xml.as_ref(),
            links.iter().map(|(_, link)| link.full.start),
        )?;
        links
            .into_iter()
            .map(|(location, link)| {
                let scope = link_scopes.get(&link.full.start).ok_or_else(|| {
                    Error::Other("story hyperlink namespace scope was not inventoried".to_owned())
                })?;
                let info = self.story_link_info(story, source.xml.as_ref(), link, scope)?;
                Ok((location, info))
            })
            .collect()
    }

    /// Materialize every modeled story hyperlink from one package inventory.
    pub fn story_link_snapshots(&self) -> Result<Vec<(ContentLocation, LinkInfo)>> {
        let mut snapshots = Vec::new();
        let mut seen = HashSet::new();
        for source in self.story_sources()? {
            let owners = scan_story_owners(source.xml.as_ref(), source.root_kind)?;
            let owner_scopes = story_namespace_scopes_at(
                source.xml.as_ref(),
                owners.iter().map(|owner| owner.full.start),
            )?;
            let mut inventories = Vec::new();
            for owner in owners {
                let identity = (owner.kind, source.part_name.clone(), owner.owner_index);
                if !seen.insert(identity) {
                    continue;
                }
                let story = StoryId {
                    kind: owner.kind,
                    part_name: source.part_name.clone(),
                    owner_index: owner.owner_index,
                    fingerprint: owner.fingerprint,
                };
                let scope = owner_scopes.get(&owner.full.start).ok_or_else(|| {
                    Error::Other("story owner namespace scope was not inventoried".to_owned())
                })?;
                let items = scan_story_items_with_scope(source.xml.as_ref(), &owner, scope, None)?;
                inventories.push((story, items));
            }
            let item_scopes = story_namespace_scopes_at(
                source.xml.as_ref(),
                inventories
                    .iter()
                    .flat_map(|(_, items)| items.iter().map(|item| item.scan.start)),
            )?;
            let mut link_inventories = Vec::new();
            for (story, items) in inventories {
                let links =
                    ordered_story_link_spans(source.xml.as_ref(), &story, items, &item_scopes)?;
                link_inventories.push((story, links));
            }
            let link_scopes = story_namespace_scopes_at(
                source.xml.as_ref(),
                link_inventories
                    .iter()
                    .flat_map(|(_, links)| links.iter().map(|(_, link)| link.full.start)),
            )?;
            for (story, links) in link_inventories {
                for (location, link) in links {
                    let scope = link_scopes.get(&link.full.start).ok_or_else(|| {
                        Error::Other(
                            "story hyperlink namespace scope was not inventoried".to_owned(),
                        )
                    })?;
                    let info = self.story_link_info(&story, source.xml.as_ref(), link, scope)?;
                    snapshots.push((location, info));
                }
            }
        }
        Ok(snapshots)
    }

    fn append_fragment_to_story(
        &mut self,
        story: &StoryId,
        mut fragment: ContentFragment,
    ) -> Result<()> {
        let (source, owner) = self.story_source_and_owner(story)?;
        let part_name = source.part_name.clone();
        let source_xml = source.xml.into_owned();
        let destination = ContentLocation::end(story.clone());
        let (boundary, _, _) = validated_content_boundary(&source_xml, &owner, &destination)?;
        fragment.source_part_name = Some(part_name.clone());
        fragment.source_story_kind = Some(story.kind);
        fragment.source_owner_index = Some(story.owner_index);
        let fragment_xml = content_fragment_for_insertion(self, story, &fragment)?;
        let mut updated = source_xml;
        insert_story_fragment(&mut updated, &owner, boundary, fragment_xml)?;
        set_story_source_xml(self, &part_name, updated)
    }

    /// Append an inline picture paragraph to one checked story owner.
    pub fn add_picture_to_story(
        &mut self,
        story: &StoryId,
        image_data: &[u8],
        image_filename: &str,
        width: Length,
        height: Length,
    ) -> Result<()> {
        self.insert_picture_to_story(
            story,
            None,
            image_data,
            image_filename,
            Some(width),
            Some(height),
        )?;
        Ok(())
    }

    /// Insert an inline picture at one checked physical run boundary.
    /// The media, owning relationship and drawing are published together.
    pub fn insert_picture_at(
        &mut self,
        position: &StoryRunPosition,
        image_data: &[u8],
        image_filename: &str,
        width: Option<Length>,
        height: Option<Length>,
    ) -> Result<()> {
        self.insert_picture_with_options_at(
            position,
            image_data,
            image_filename,
            width,
            height,
            None,
        )
        .map(|_| ())
    }

    /// Import picture metadata and placement through the same checked owner transaction.
    pub fn insert_picture_with_options_at(
        &mut self,
        position: &StoryRunPosition,
        image_data: &[u8],
        image_filename: &str,
        width: Option<Length>,
        height: Option<Length>,
        options: Option<PictureOptions>,
    ) -> Result<PictureOccurrence> {
        let format = oxoffice_media::ImageFormat::sniff(image_data)
            .ok_or_else(|| Error::Other("picture image bytes are unsupported".to_owned()))?;
        if matches!(
            format,
            oxoffice_media::ImageFormat::Png
                | oxoffice_media::ImageFormat::Jpeg
                | oxoffice_media::ImageFormat::Gif
                | oxoffice_media::ImageFormat::Bmp
                | oxoffice_media::ImageFormat::Webp
        ) && oxoffice_media::probe(image_data).is_none()
        {
            return Err(Error::Other(
                "picture image bytes are unsupported".to_owned(),
            ));
        }
        let (width, height) = match (width, height) {
            (Some(width), Some(height)) if width.to_emu() > 0 && height.to_emu() > 0 => {
                (width, height)
            }
            (None, None) => {
                let size = oxoffice_media::probe(image_data)
                    .and_then(|info| info.native_size(72.0))
                    .ok_or_else(|| Error::UnavailableImageDimensions {
                        filename: image_filename.to_owned(),
                    })?;
                (Length::emu(size.width_emu), Length::emu(size.height_emu))
            }
            _ => {
                return Err(Error::Other(
                    "picture dimensions require two positive lengths or neither".to_owned(),
                ));
            }
        };
        let mut options = options.unwrap_or(PictureOptions {
            width,
            height,
            crop: None,
            anchor: None,
            name: None,
            description: None,
            title: None,
            decorative: false,
        });
        options.width = width;
        options.height = height;
        validate_picture_options(&options)?;
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let snapshot = candidate.story_range_paragraph_snapshot(&position.location)?;
        let original_paragraph = CT_P::from_xml_fragment(snapshot.xml())?;
        let original = original_paragraph.accepted_view();
        if position.run_index > original.runs.len() {
            return Err(Error::Other("picture run boundary is out of range".into()));
        }
        let ordinal = original
            .runs
            .iter()
            .take(position.run_index)
            .flat_map(|run| {
                run.content
                    .iter()
                    .filter_map(|content| match content {
                        RunContent::Drawing(drawing) => Some(drawing),
                        _ => None,
                    })
                    .chain(run.alt_drawings.iter())
            })
            .filter(|drawing| {
                drawing
                    .inline
                    .as_ref()
                    .is_some_and(|i| !i.embed_id.is_empty())
                    || drawing
                        .anchor
                        .as_ref()
                        .is_some_and(|a| !a.embed_id.is_empty())
            })
            .count();
        let relationship = candidate.add_image_relationship_checked(
            position.location.story.part_name(),
            image_data,
            image_filename,
        )?;
        let drawing = picture_drawing(
            &relationship,
            candidate.identifiers.reserve_drawing_id()?,
            &options,
        );
        let mut paragraph = CT_P::new();
        paragraph.runs.push(CT_R {
            alt_drawings: Vec::new(),
            properties: None,
            content: vec![RunContent::Drawing(drawing)],
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
        });
        close_typed_story_drawing_namespaces(&mut paragraph)?;
        candidate.insert_story_run_staged(position, paragraph.runs.remove(0))?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        let picture = reopened
            .pictures()?
            .into_iter()
            .find(|picture| {
                picture.location.story.kind == position.location.story.kind
                    && picture.location.story.part_name == position.location.story.part_name
                    && picture.location.story.owner_index == position.location.story.owner_index
                    && picture.location.index_path == position.location.index_path
                    && picture.ordinal == ordinal
            })
            .ok_or_else(|| Error::Other("inserted picture occurrence was not projected".into()))?;
        self.commit_staged_mutation(reopened);
        Ok(picture)
    }

    /// Insert an inline picture paragraph after checked story content.
    ///
    /// Omitting `after` appends to the story. Width and height must either both
    /// be present or both be omitted. Omitted dimensions use the image's native
    /// size at 72 DPI.
    pub fn insert_picture_to_story(
        &mut self,
        story: &StoryId,
        after: Option<&ContentLocation>,
        image_data: &[u8],
        image_filename: &str,
        width: Option<Length>,
        height: Option<Length>,
    ) -> Result<ContentLocation> {
        self.insert_picture_to_story_with_options(
            story,
            after,
            image_data,
            image_filename,
            width,
            height,
            None,
        )
    }

    /// Import picture metadata and placement through the same checked owner transaction.
    pub fn insert_picture_to_story_with_options(
        &mut self,
        story: &StoryId,
        after: Option<&ContentLocation>,
        image_data: &[u8],
        image_filename: &str,
        width: Option<Length>,
        height: Option<Length>,
        options: Option<PictureOptions>,
    ) -> Result<ContentLocation> {
        let options = Self::checked_insertion_picture_options(
            image_data,
            image_filename,
            width,
            height,
            options,
        )?;
        let candidate = self.clone_for_staging();
        let (updated, location) = Self::insert_picture_candidate(
            candidate,
            story,
            after,
            image_data,
            image_filename,
            options,
            None,
        )?;
        self.commit_staged_mutation(updated);
        Ok(location)
    }

    /// Consume a private candidate using the same checked picture transaction.
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn insert_picture_to_story_with_options_owned(
        mut self,
        story: &StoryId,
        after: Option<&ContentLocation>,
        image_data: &[u8],
        image_filename: &str,
        width: Option<Length>,
        height: Option<Length>,
        options: Option<PictureOptions>,
    ) -> Result<(Self, ContentLocation)> {
        let options = Self::checked_insertion_picture_options(
            image_data,
            image_filename,
            width,
            height,
            options,
        )?;
        let mut origin = self.take_staged_commit_origin();
        let (updated, location) = Self::insert_picture_candidate(
            self,
            story,
            after,
            image_data,
            image_filename,
            options,
            None,
        )?;
        Ok((
            Self::finish_staged_candidate(
                &origin.package,
                origin.signature_invalidated,
                &mut origin.normal_engine,
                &mut origin.bundled_engine,
                updated,
            ),
            location,
        ))
    }

    /// Consume a private candidate and return the complete checked picture inventory.
    /// Existing insertion-only methods do not perform this inventory.
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn insert_picture_to_story_with_options_and_inventory_owned(
        mut self,
        story: &StoryId,
        after: Option<&ContentLocation>,
        image_data: &[u8],
        image_filename: &str,
        width: Option<Length>,
        height: Option<Length>,
        options: Option<PictureOptions>,
    ) -> Result<(Self, ContentLocation, Vec<PictureOccurrence>)> {
        let options = Self::checked_insertion_picture_options(
            image_data,
            image_filename,
            width,
            height,
            options,
        )?;
        let mut origin = self.take_staged_commit_origin();
        let mut inventory = None;
        let (updated, location) = Self::insert_picture_candidate(
            self,
            story,
            after,
            image_data,
            image_filename,
            options,
            Some(&mut inventory),
        )?;
        let updated = Self::finish_staged_candidate(
            &origin.package,
            origin.signature_invalidated,
            &mut origin.normal_engine,
            &mut origin.bundled_engine,
            updated,
        );
        // None is only an explicit shared-preparation decline. Collector errors
        // propagate, never become a second attempted inventory.
        let inventory = match inventory {
            Some(inventory) => inventory,
            None => updated.pictures()?,
        };
        Ok((updated, location, inventory))
    }

    fn checked_insertion_picture_options(
        image_data: &[u8],
        image_filename: &str,
        width: Option<Length>,
        height: Option<Length>,
        options: Option<PictureOptions>,
    ) -> Result<PictureOptions> {
        let format = oxoffice_media::ImageFormat::sniff(image_data)
            .ok_or_else(|| Error::Other("picture image bytes are unsupported".to_owned()))?;
        if matches!(
            format,
            oxoffice_media::ImageFormat::Png
                | oxoffice_media::ImageFormat::Jpeg
                | oxoffice_media::ImageFormat::Gif
                | oxoffice_media::ImageFormat::Bmp
                | oxoffice_media::ImageFormat::Webp
        ) && oxoffice_media::probe(image_data).is_none()
        {
            return Err(Error::Other(
                "picture image bytes are unsupported".to_owned(),
            ));
        }
        let (width, height) = match (width, height) {
            (Some(width), Some(height)) => (width, height),
            (None, None) => {
                let native_size = oxoffice_media::probe(image_data)
                    .and_then(|info| info.native_size(72.0))
                    .ok_or_else(|| Error::UnavailableImageDimensions {
                        filename: image_filename.to_owned(),
                    })?;
                (
                    Length::emu(native_size.width_emu),
                    Length::emu(native_size.height_emu),
                )
            }
            _ => {
                return Err(Error::Other(
                    "picture width and height must both be provided or both be omitted".to_owned(),
                ));
            }
        };
        let mut options = options.unwrap_or(PictureOptions {
            width,
            height,
            crop: None,
            anchor: None,
            name: None,
            description: None,
            title: None,
            decorative: false,
        });
        options.width = width;
        options.height = height;
        validate_picture_options(&options)?;
        Ok(options)
    }

    fn insert_picture_candidate(
        mut candidate: Self,
        story: &StoryId,
        after: Option<&ContentLocation>,
        image_data: &[u8],
        image_filename: &str,
        options: PictureOptions,
        inventory: Option<&mut Option<Vec<PictureOccurrence>>>,
    ) -> Result<(Self, ContentLocation)> {
        candidate.flush_to_package()?;
        let (source, mut story_owner) = candidate.story_source_and_owner(story)?;
        let part_name = source.part_name.clone();
        let mut source_xml = source.xml.into_owned();
        // The ordinary story projection is typed XML. For picture insertion,
        // use the retained main part when it still describes the current model.
        if part_name == candidate.doc_part_name
            && let Some(retained) = candidate.package.get_part(&part_name)
            && retained != source_xml
            && candidate.main_part_parses_to_model(candidate.kept_main_part().as_deref())
        {
            source_xml = retained.to_vec();
            story_owner = scan_story_owners(&source_xml, StoryKind::Body)?
                .into_iter()
                .find(|owner| owner.kind == story.kind && owner.owner_index == story.owner_index)
                .ok_or_else(|| Error::Other("picture story source disappeared".to_owned()))?;
        }
        // Typed projection can allocate private field identities. Field-bearing
        // sources keep the original scan schedule before and after publication.
        let shared_items = if !holds_field_words(&source_xml) {
            Some(scan_story_items(&source_xml, &story_owner, None)?)
        } else {
            None
        };
        let direct_items = match &shared_items {
            Some(items) => direct_story_content_items_in(&source_xml, &story_owner, items)?,
            None => direct_story_content_items(&source_xml, &story_owner)?,
        };
        let (boundary, inserted_direct_index) = match after {
            Some(after) => {
                if after.story != *story {
                    return Err(Error::Other(
                        "picture placement must belong to the target story".to_owned(),
                    ));
                }
                let item = validated_direct_content_item(&source_xml, &story_owner, after)?;
                let index = direct_items
                    .iter()
                    .position(|candidate| candidate.full == item.full)
                    .ok_or_else(|| Error::Other("picture placement was not found".to_owned()))?;
                (item.full.end, index + 1)
            }
            None => (
                match &shared_items {
                    Some(items) => story_owner_content_end_in(&source_xml, &story_owner, items)?,
                    None => story_owner_content_end(&source_xml, &story_owner)?,
                },
                direct_items.len(),
            ),
        };
        let relationship_id =
            candidate.add_image_relationship_checked(&part_name, image_data, image_filename)?;
        let drawing_id = candidate.identifiers.reserve_drawing_id()?;

        let drawing = picture_drawing(&relationship_id, drawing_id, &options);
        let mut paragraph = CT_P::new();
        paragraph.runs.push(CT_R {
            alt_drawings: Vec::new(),
            properties: None,
            content: vec![RunContent::Drawing(drawing)],
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
        });
        close_typed_story_drawing_namespaces(&mut paragraph)?;
        let mut fragment = ContentFragment::paragraph(paragraph)?;
        fragment.source_part_name = Some(part_name.clone());
        fragment.source_story_kind = Some(story.kind);
        fragment.source_owner_index = Some(story.owner_index);
        let fragment_xml = content_fragment_for_insertion(&candidate, story, &fragment)?;
        let mut updated = source_xml;
        insert_story_fragment(&mut updated, &story_owner, boundary, fragment_xml)?;
        set_story_source_xml(&mut candidate, &part_name, updated)?;
        if part_name == candidate.doc_part_name {
            candidate
                .identifiers
                .register_nested_story_relationship(story, relationship_id);
            candidate
                .identifiers
                .register_nested_story_drawing(story, drawing_id);
        }
        let reopened = candidate.prepare_and_reopen_staged()?;
        let location = {
            let read = reopened.prepared_main_read()?;
            let related = reopened.related_story_sources()?;
            let refreshed_story = read
                .stories(&related)?
                .into_iter()
                .find(|candidate| {
                    candidate.kind == story.kind
                        && candidate.part_name == story.part_name
                        && candidate.owner_index == story.owner_index
                })
                .ok_or_else(|| Error::Other("inserted picture story was not found".to_owned()))?;
            let (source, owner, _) = read.owner_and_layout(&refreshed_story, false, &related)?;
            let shared_projected = if !holds_field_words(source.xml.as_ref()) {
                Some(scan_story_items(source.xml.as_ref(), &owner, None)?)
            } else {
                None
            };
            let direct = match &shared_projected {
                Some(items) => direct_story_content_items_in(source.xml.as_ref(), &owner, items)?,
                None => direct_story_content_items(source.xml.as_ref(), &owner)?,
            };
            let inserted = direct.get(inserted_direct_index).ok_or_else(|| {
                Error::Other("inserted picture paragraph was not found".to_owned())
            })?;
            let projected = match shared_projected {
                Some(items) => items,
                None => scan_story_items(source.xml.as_ref(), &owner, None)?,
            };
            let index = projected
                .iter()
                .position(|item| item.full == inserted.full)
                .ok_or_else(|| {
                    Error::Other("inserted picture paragraph was not projected".to_owned())
                })?;
            if let Some(inventory) = inventory
                && !holds_field_words(&read.xml)
                && related.iter().all(|source| !holds_field_words(&source.xml))
            {
                *inventory = Some(reopened.pictures_in_prepared(&read, &related)?);
            }
            ContentLocation::new(refreshed_story, StoryItemKind::Paragraph, vec![index])
        };
        Ok((reopened, location))
    }

    /// Append one configured picture paragraph to a checked story owner.
    pub fn add_picture_with_options(
        &mut self,
        story: &StoryId,
        image_data: &[u8],
        image_filename: &str,
        options: PictureOptions,
    ) -> Result<()> {
        validate_picture_options(&options)?;
        if oxoffice_media::probe(image_data).is_none() {
            return Err(Error::Other(format!(
                "picture `{image_filename}` is unsupported or malformed"
            )));
        }

        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        candidate.story_source_and_owner(story)?;
        let owner = story.part_name.clone();
        let relationship_id =
            candidate.add_image_relationship_checked(&owner, image_data, image_filename)?;
        let drawing_id = candidate.identifiers.reserve_drawing_id()?;
        let drawing = picture_drawing(&relationship_id, drawing_id, &options);
        let mut paragraph = CT_P::new();
        paragraph.runs.push(CT_R {
            alt_drawings: Vec::new(),
            properties: None,
            content: vec![RunContent::Drawing(drawing)],
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
        });
        candidate.append_authored_paragraph_to_story(story, paragraph)?;
        if owner == candidate.doc_part_name && story.kind != StoryKind::Body {
            candidate
                .identifiers
                .register_nested_story_relationship(story, relationship_id);
            candidate
                .identifiers
                .register_nested_story_drawing(story, drawing_id);
        }
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    fn append_internal_hyperlink_staged(
        &mut self,
        location: &ContentLocation,
        text: &str,
        bookmark: &str,
        tooltip: Option<&str>,
    ) -> Result<StoryRunPosition> {
        for value in [Some(text), Some(bookmark), tooltip].into_iter().flatten() {
            oxoffice_core::xml::reject_non_xml_characters("internal hyperlink", value)?;
        }
        crate::comments::validate_bookmark_name(bookmark)?;
        let (ranges, across) = self.story_range_inventory()?;
        let matching = ranges
            .iter()
            .map(crate::StoryRangeRef::kind)
            .chain(&across)
            .filter(|kind| {
                matches!(kind, crate::StoryRangeKind::Bookmark { name, .. } if name == bookmark)
            })
            .count();
        if matching != 1 {
            return Err(Error::Other(
                "internal hyperlink bookmark is missing or ambiguous".into(),
            ));
        }
        let story = self
            .stories()?
            .into_iter()
            .find(|story| {
                story.kind == location.story.kind
                    && story.part_name == location.story.part_name
                    && story.owner_index == location.story.owner_index
            })
            .ok_or_else(|| Error::Other("hyperlink story disappeared".into()))?;
        let refreshed =
            ContentLocation::new(story, location.item_kind, location.index_path.clone());
        let run_index = self.edit_story_paragraph(&refreshed, |paragraph| {
            let index = paragraph.run_count();
            paragraph.add_internal_hyperlink(text, bookmark, tooltip);
            Ok(index)
        })?;
        let story = self
            .stories()?
            .into_iter()
            .find(|story| {
                story.kind == location.story.kind
                    && story.part_name == location.story.part_name
                    && story.owner_index == location.story.owner_index
            })
            .ok_or_else(|| Error::Other("hyperlink story disappeared".into()))?;
        Ok(StoryRunPosition {
            location: ContentLocation::new(story, location.item_kind, location.index_path.clone()),
            run_index,
        })
    }

    /// Add a checked internal link without allocating an external relationship.
    pub fn add_internal_hyperlink_at(
        &mut self,
        location: &ContentLocation,
        text: &str,
        bookmark: &str,
        tooltip: Option<&str>,
    ) -> Result<StoryRunPosition> {
        self.story_range_paragraph_snapshot(location)?;
        let mut candidate = self.clone_for_staging();
        let result =
            candidate.append_internal_hyperlink_staged(location, text, bookmark, tooltip)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        let story = reopened
            .stories()?
            .into_iter()
            .find(|s| {
                s.kind == result.location.story.kind
                    && s.part_name == result.location.story.part_name
                    && s.owner_index == result.location.story.owner_index
            })
            .ok_or_else(|| Error::Other("hyperlink story disappeared".into()))?;
        self.commit_staged_mutation(reopened);
        Ok(StoryRunPosition {
            location: ContentLocation::new(
                story,
                result.location.item_kind,
                result.location.index_path,
            ),
            run_index: result.run_index,
        })
    }

    /// Stage a heading bookmark and source link together, with unique IDs and
    /// one caller publication. Existing complete heading bookmarks are reused.
    pub fn add_heading_hyperlink_at(
        &mut self,
        source: &ContentLocation,
        heading: &ContentLocation,
        text: &str,
        tooltip: Option<&str>,
    ) -> Result<StoryRunPosition> {
        self.story_range_paragraph_snapshot(source)?;
        let item = self.story_range_paragraph_snapshot(heading)?;
        let paragraph = CT_P::from_xml_fragment(&item.xml)?;
        let mut level = paragraph.properties.as_ref().and_then(|p| p.outline_lvl);
        let mut style = paragraph
            .properties
            .as_ref()
            .and_then(|p| p.style_id.as_deref());
        let mut seen = HashSet::new();
        while level.is_none() {
            let Some(id) = style else {
                break;
            };
            if !seen.insert(id) {
                return Err(Error::Other("heading style ancestry is cyclic".into()));
            }
            let Some(value) = self.styles.get_by_id(id) else {
                break;
            };
            level = value.ppr.as_ref().and_then(|p| p.outline_lvl);
            style = value.based_on.as_deref();
        }
        if !level.is_some_and(|level| level < 9)
            && self.detect_heading_level_for_toc(&paragraph).is_none()
        {
            return Err(Error::Other(
                "heading target is not an outline heading".into(),
            ));
        }
        let run_count = paragraph.accepted_run_paths().len();
        let (ranges, across) = self.story_range_inventory()?;
        let existing = ranges.iter().find_map(|entry| {
            let crate::StoryRangeKind::Bookmark { name, .. } = entry.kind() else {
                return None;
            };
            let range = entry.range();
            (crate::comments::validate_bookmark_name(name).is_ok()
                && range.start.location == *heading
                && range.end.location == *heading
                && range.start.run_index == 0
                && range.end.run_index == run_count)
                .then(|| name.clone())
        });
        let mut candidate = self.clone_for_staging();
        let name = match existing {
            Some(name) => name,
            None => {
                let names = ranges
                    .iter()
                    .map(crate::StoryRangeRef::kind)
                    .chain(&across)
                    .filter_map(|kind| match kind {
                        crate::StoryRangeKind::Bookmark { name, .. } => Some(name.as_str()),
                        _ => None,
                    })
                    .collect::<HashSet<_>>();
                let mut suffix = 1u32;
                let name = loop {
                    let name = format!("Heading_{suffix}");
                    if !names.contains(name.as_str()) {
                        break name;
                    }
                    suffix = suffix
                        .checked_add(1)
                        .ok_or_else(|| Error::Other("heading bookmark names exhausted".into()))?;
                };
                candidate.add_story_bookmark(
                    &name,
                    crate::StoryRunRange {
                        start: crate::StoryRunPosition {
                            location: heading.clone(),
                            run_index: 0,
                        },
                        end: crate::StoryRunPosition {
                            location: heading.clone(),
                            run_index: run_count,
                        },
                    },
                )?;
                name
            }
        };
        let result = candidate.append_internal_hyperlink_staged(source, text, &name, tooltip)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        let story = reopened
            .stories()?
            .into_iter()
            .find(|s| {
                s.kind == source.story.kind
                    && s.part_name == source.story.part_name
                    && s.owner_index == source.story.owner_index
            })
            .ok_or_else(|| Error::Other("heading hyperlink source disappeared".into()))?;
        self.commit_staged_mutation(reopened);
        Ok(StoryRunPosition {
            location: ContentLocation::new(story, source.item_kind, source.index_path.clone()),
            run_index: result.run_index,
        })
    }

    /// Append one modeled DrawingML text box with a VML compatibility fallback.
    pub fn add_text_box_to_story(
        &mut self,
        story: &StoryId,
        text: &str,
        options: TextBoxOptions,
    ) -> Result<()> {
        validate_text_box_options(&options)?;
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        candidate.story_source_and_owner(story)?;
        let drawing_id = candidate.identifiers.reserve_drawing_id()?;
        let alternate = authored_text_box_alternate_content(drawing_id, text, &options)?;
        let drawing = oxoffice_docx_oxml::drawing::parse_alternate_content(&alternate, &[])
            .ok_or_else(|| {
                Error::Other("authored text box did not reopen as DrawingML".to_owned())
            })?;
        let mut paragraph = CT_P::new();
        paragraph.runs.push(CT_R {
            properties: None,
            content: Vec::new(),
            extra_xml: vec![alternate],
            extra_xml_positions: vec![0],
            alt_drawings: vec![drawing],
        });
        candidate.append_authored_paragraph_to_story(story, paragraph)?;
        if story.part_name == candidate.doc_part_name && story.kind != StoryKind::Body {
            candidate
                .identifiers
                .register_nested_story_drawing(story, drawing_id);
        }
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    fn append_authored_paragraph_to_story(
        &mut self,
        story: &StoryId,
        mut paragraph: CT_P,
    ) -> Result<()> {
        self.story_source_and_owner(story)?;
        if story.kind == StoryKind::Body && story.part_name == self.doc_part_name {
            self.document
                .body
                .content
                .push(BodyContent::Paragraph(Box::new(paragraph)));
        } else {
            close_typed_story_drawing_namespaces(&mut paragraph)?;
            self.append_fragment_to_story(story, ContentFragment::paragraph(paragraph)?)?;
        }
        Ok(())
    }

    /// Append one external hyperlink paragraph to a checked story owner.
    pub fn add_hyperlink_to_story(&mut self, story: &StoryId, text: &str, url: &str) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        candidate.story_source_and_owner(story)?;
        let owner = story.part_name.clone();
        let relationship_id =
            candidate.add_external_relationship_checked(&owner, rel_types::HYPERLINK, url)?;
        let mut paragraph = CT_P::new();
        crate::Paragraph {
            inner: &mut paragraph,
        }
        .add_hyperlink(text, &relationship_id);
        paragraph
            .hyperlinks
            .last_mut()
            .expect("the authored hyperlink was appended")
            .extra_attributes
            .push(("xmlns:r".to_owned(), drawing_ns::R.to_owned()));
        let preserve_typed_body = story.kind == StoryKind::Body && owner == candidate.doc_part_name;
        if preserve_typed_body {
            candidate
                .document
                .body
                .content
                .push(BodyContent::Paragraph(Box::new(paragraph)));
        } else {
            let fragment = ContentFragment::paragraph(paragraph)?;
            candidate.append_fragment_to_story(story, fragment)?;
        }
        if owner == candidate.doc_part_name {
            candidate
                .identifiers
                .register_nested_story_relationship(story, relationship_id);
        }
        candidate.prepare_staged_package()?;
        let reopened = candidate.clone_for_staging().reopen_prepared_staged()?;
        if candidate.doc_part_name == reopened.doc_part_name
            && candidate.package.get_part(&candidate.doc_part_name)
                == reopened.package.get_part(&reopened.doc_part_name)
            && candidate.root_namespace_declarations == reopened.root_namespace_declarations
            && candidate.body_namespace_declarations == reopened.body_namespace_declarations
            && candidate.body_namespace_bindings == reopened.body_namespace_bindings
        {
            // Keep the authored typed body, but retain the actual validated
            // opening and its package provenance instead of discarding them.
            candidate.package = reopened.package;
            candidate.parsed = reopened.parsed;
            candidate.identifiers.scanned_parts = reopened.identifiers.scanned_parts;
            candidate.install_carried_blocks(reopened.main_part_blocks);
        }
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Add an external hyperlink relationship to a checked story owner.
    pub fn add_hyperlink_relationship_to_story(
        &mut self,
        story: &StoryId,
        url: &str,
    ) -> Result<String> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        candidate.story_source_and_owner(story)?;
        let owner = story.part_name.clone();
        let matching_before = candidate
            .package
            .get_part_rels(&owner)
            .into_iter()
            .flat_map(|relationships| &relationships.items)
            .filter(|relationship| {
                relationship.rel_type == rel_types::HYPERLINK
                    && relationship.target == url
                    && relationship.target_mode.as_deref() == Some("External")
            })
            .count();
        let id = candidate.add_external_relationship_checked(&owner, rel_types::HYPERLINK, url)?;
        candidate.register_pending_relationship(&owner, &id);
        let reopened = candidate.prepare_and_reopen_staged()?;
        let relationship_id = reopened
            .package
            .get_part_rels(&owner)
            .into_iter()
            .flat_map(|relationships| &relationships.items)
            .filter(|relationship| {
                relationship.rel_type == rel_types::HYPERLINK
                    && relationship.target == url
                    && relationship.target_mode.as_deref() == Some("External")
            })
            .nth(matching_before)
            .map(|relationship| relationship.id.clone())
            .ok_or_else(|| Error::Other("authored hyperlink relationship was lost".to_owned()))?;
        self.commit_staged_mutation(reopened);
        Ok(relationship_id)
    }

    /// Resolve and validate one internal relationship in a checked story scope.
    pub fn validate_internal_relationship_for_story(
        &self,
        story: &StoryId,
        relationship_id: &str,
        expected_type: &str,
    ) -> Result<String> {
        self.story_source_and_owner(story)?;
        self.validate_internal_relationship_in_checked_story(story, relationship_id, expected_type)
    }

    // The caller must have checked this owner against the current immutable
    // source. Relationship policy stays shared with the public checked entry.
    fn validate_internal_relationship_in_checked_story(
        &self,
        story: &StoryId,
        relationship_id: &str,
        expected_type: &str,
    ) -> Result<String> {
        let relationships = self
            .package
            .get_part_rels(&story.part_name)
            .ok_or_else(|| {
                Error::Other(format!(
                    "story owner {} has no relationship scope",
                    story.part_name
                ))
            })?;
        let relationship = relationships.get_by_id(relationship_id).ok_or_else(|| {
            Error::Other(format!(
                "story owner {} has no relationship {relationship_id}",
                story.part_name
            ))
        })?;
        if relationship.rel_type != expected_type {
            return Err(Error::Other(format!(
                "story relationship {relationship_id} has type {}, expected {expected_type}",
                relationship.rel_type
            )));
        }
        if !relationship_is_internal(relationship) {
            return Err(Error::Other(format!(
                "story relationship {relationship_id} must be internal"
            )));
        }
        let target = OpcPackage::resolve_rel_target(&story.part_name, &relationship.target);
        if !self.package.contains_part(&target) {
            return Err(Error::Other(format!(
                "story relationship {relationship_id} targets missing part {target}"
            )));
        }
        Ok(target)
    }

    /// Inventory supported pictures once per package source, in story and
    /// physical source order. Only the existing accepted paragraph projection
    /// admits a drawing. Unknown XML and unselected alternatives stay opaque.
    pub fn pictures(&self) -> Result<Vec<PictureOccurrence>> {
        let prepared = self.prepared_main_read()?;
        let related = self.related_story_sources()?;
        self.pictures_in_prepared(&prepared, &related)
    }

    /// The same complete collector over one actual immutable document phase.
    /// Callers must prepare from this document and keep it unchanged throughout.
    fn pictures_in_prepared(
        &self,
        prepared: &PreparedMainRead<'_>,
        related: &[StorySource<'_>],
    ) -> Result<Vec<PictureOccurrence>> {
        debug_assert!(std::ptr::eq(self, prepared.document));
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        let sources = prepared.sources(related);
        // Optional, read-only qualification. No child reader or memo filling.
        let plain_proofs = prepared
            .writer
            .as_ref()
            .and_then(|record| record.picture_free_block_ranges(&prepared.xml));
        for source in sources {
            let owners = scan_story_owners(&source.xml, source.root_kind)?
                .into_iter()
                .map(|owner| {
                    let story = StoryId {
                        kind: owner.kind,
                        part_name: source.part_name.clone(),
                        owner_index: owner.owner_index,
                        fingerprint: owner.fingerprint,
                    };
                    (story, owner)
                })
                .collect::<Vec<_>>();
            // This is optional preparation. A later owner's malformed source
            // must not replace an earlier paragraph's authoritative error.
            let batch = (|| -> Result<_> {
                let spans = story_range_paragraph_spans(&source.xml, &owners)?;
                let scopes = story_namespace_scopes_at(
                    &source.xml,
                    spans.iter().flatten().map(|(_, span)| span.start),
                )?;
                Ok((spans, scopes))
            })()
            .ok();
            for (index, (story, owner)) in owners.iter().enumerate() {
                let spans = match &batch {
                    Some((spans, _)) => std::borrow::Cow::Borrowed(spans[index].as_slice()),
                    None => std::borrow::Cow::Owned(
                        story_range_paragraph_spans(
                            &source.xml,
                            &[(story.clone(), owner.clone())],
                        )?
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>(),
                    ),
                };
                for (location, span) in spans.as_ref() {
                    let scope = match batch
                        .as_ref()
                        .and_then(|(_, scopes)| scopes.get(&span.start))
                    {
                        Some(inherited) => {
                            let mut reader =
                                quick_xml::Reader::from_reader(&source.xml[span.clone()]);
                            match reader.read_event().map_err(|error| {
                                Error::Other(format!("story namespace scope scan failed: {error}"))
                            })? {
                                Event::Start(element) | Event::Empty(element) => {
                                    changed_story_namespace_scope(inherited, &element)?
                                        .unwrap_or_else(|| inherited.clone())
                                }
                                _ => story_namespace_scope_at(&source.xml, span.start)?,
                            }
                        }
                        None => story_namespace_scope_at(&source.xml, span.start)?,
                    };
                    let closed =
                        close_content_fragment_namespaces(&source.xml[span.clone()], &scope)?;
                    let proof = (source.part_name == self.doc_part_name
                        && source.root_kind == StoryKind::Body)
                        .then(|| {
                            plain_proofs
                                .as_ref()
                                .and_then(|ranges| picture_free_span_kind(ranges, owner, span))
                        })
                        .flatten();
                    if proof.is_some() && block_proofs_enabled() {
                        continue;
                    }
                    let paragraph = CT_P::from_xml_fragment(&closed)?;
                    let accepted = paragraph.accepted_view();
                    let drawings = accepted.runs.iter().flat_map(|run| {
                        run.content
                            .iter()
                            .filter_map(|content| match content {
                                RunContent::Drawing(drawing) => Some(drawing),
                                _ => None,
                            })
                            .chain(run.alt_drawings.iter())
                    });
                    let mut occurrences = Vec::new();
                    let mut claimed = HashSet::new();
                    for drawing in drawings {
                        let (
                            raw,
                            embed,
                            link,
                            chart,
                            _projected_width,
                            _projected_height,
                            name,
                            description,
                            title,
                            decorative,
                            anchored,
                        ) = if let Some(inline) = &drawing.inline {
                            (
                                &inline.raw_xml,
                                &inline.embed_id,
                                &inline.link_id,
                                &inline.chart_rel_id,
                                inline.extent_cx.0,
                                inline.extent_cy.0,
                                &inline.name,
                                &inline.description,
                                &inline.title,
                                inline.decorative,
                                false,
                            )
                        } else if let Some(anchor) = &drawing.anchor {
                            (
                                &anchor.raw_xml,
                                &anchor.embed_id,
                                &anchor.link_id,
                                &anchor.chart_rel_id,
                                anchor.extent_cx.0,
                                anchor.extent_cy.0,
                                &anchor.name,
                                &anchor.description,
                                &anchor.title,
                                anchor.decorative,
                                true,
                            )
                        } else {
                            continue;
                        };
                        if chart.is_some() || (embed.is_empty() && link.is_none()) {
                            continue;
                        }
                        let raw = raw.as_ref().ok_or_else(|| {
                            Error::Other("picture has no physical source identity".into())
                        })?;
                        if raw.is_empty() {
                            return Err(Error::Other("picture source is empty".into()));
                        }
                        let Some((width, height)) = picture_source_extent(raw, &scope, anchored)?
                        else {
                            continue;
                        };
                        let matches = source.xml[span.clone()]
                            .windows(raw.len())
                            .enumerate()
                            .filter(|(_, bytes)| *bytes == raw.as_slice())
                            .map(|(offset, _)| span.start + offset)
                            .collect::<Vec<_>>();
                        // Physical copies that cannot be distinguished by the
                        // admitted projection are not silently reinterpreted.
                        let count = accepted
                            .runs
                            .iter()
                            .flat_map(|run| {
                                run.content
                                    .iter()
                                    .filter_map(|c| match c {
                                        RunContent::Drawing(d) => Some(d),
                                        _ => None,
                                    })
                                    .chain(run.alt_drawings.iter())
                            })
                            .filter(|d| {
                                d.inline.as_ref().and_then(|i| i.raw_xml.as_ref()) == Some(raw)
                                    || d.anchor.as_ref().and_then(|a| a.raw_xml.as_ref())
                                        == Some(raw)
                            })
                            .count();
                        if matches.len() != count {
                            return Err(Error::Other(
                                "picture has ambiguous alternate physical copies".into(),
                            ));
                        }
                        let start = matches
                            .into_iter()
                            .find(|start| !claimed.contains(start))
                            .ok_or_else(|| {
                                Error::Other("picture source correspondence is missing".into())
                            })?;
                        claimed.insert(start);
                        if width <= 0 || height <= 0 {
                            return Err(Error::Other("picture extent must be positive".into()));
                        }
                        let relationship_id = link.as_deref().unwrap_or(embed);
                        let (media_part_name, external_url, bytes, content_type) = if link.is_some()
                        {
                            let relationship = self
                                .package
                                .get_part_rels(&story.part_name)
                                .and_then(|rels| rels.get_by_id(relationship_id))
                                .ok_or_else(|| {
                                    Error::Other("linked picture relationship is missing".into())
                                })?;
                            if relationship.rel_type != rel_types::IMAGE
                                || relationship_is_internal(relationship)
                            {
                                return Err(Error::Other(
                                    "linked picture must have an external image relationship"
                                        .into(),
                                ));
                            }
                            (
                                None,
                                Some(relationship.target.clone()),
                                Vec::new(),
                                String::new(),
                            )
                        } else {
                            // This owner came from the current source above.
                            // The immutable inventory cannot stale it between
                            // drawings, so do not rebuild that source per image.
                            let target = self.validate_internal_relationship_in_checked_story(
                                story,
                                embed,
                                rel_types::IMAGE,
                            )?;
                            let target = self
                                .package
                                .parts
                                .keys()
                                .filter(|name| {
                                    OpcPackage::resolve_rel_target("/", name)
                                        .eq_ignore_ascii_case(&target)
                                })
                                .min()
                                .cloned()
                                .ok_or_else(|| {
                                    Error::Other("picture media identity is missing".into())
                                })?;
                            let bytes = self
                                .package
                                .get_part(&target)
                                .ok_or_else(|| Error::Other("picture media is missing".into()))?
                                .to_vec();
                            let content_type = self
                                .package
                                .content_types
                                .content_type_for(&target)
                                .filter(|kind| kind.starts_with("image/"))
                                .ok_or_else(|| {
                                    Error::Other("picture media content type is unsupported".into())
                                })?
                                .to_owned();
                            (Some(target), None, bytes, content_type)
                        };
                        occurrences.push((
                            start,
                            PictureOccurrence {
                                location: location.clone(),
                                ordinal: 0,
                                relationship_id: relationship_id.to_owned(),
                                media_part_name,
                                external_url,
                                width,
                                height,
                                name: name.clone(),
                                description: description.clone(),
                                title: title.clone(),
                                decorative,
                                anchored,
                                content_type,
                                bytes,
                                source: raw.clone(),
                            },
                        ));
                    }
                    occurrences.sort_by_key(|(start, _)| *start);
                    for (ordinal, (start, mut picture)) in occurrences.into_iter().enumerate() {
                        picture.ordinal = ordinal;
                        if seen.insert((source.part_name.clone(), start)) {
                            result.push(picture);
                        }
                    }
                }
            }
        }
        Ok(result)
    }

    /// Resize one frozen occurrence, preserving shared media and every other
    /// physical drawing. Changed source or unsupported copies refuse atomically.
    pub fn set_picture_size_at(
        &mut self,
        picture: &PictureOccurrence,
        width: Length,
        height: Length,
    ) -> Result<()> {
        if picture.external_url.is_some() {
            return Err(Error::Other(
                "linked picture resizing is unsupported".into(),
            ));
        }
        let (width, height) = (width.to_emu(), height.to_emu());
        if width <= 0 || height <= 0 {
            return Err(Error::Other("picture dimensions must be positive".into()));
        }
        let current = self
            .pictures()?
            .into_iter()
            .find(|current| {
                current.location == picture.location && current.ordinal == picture.ordinal
            })
            .ok_or_else(|| Error::Other("picture occurrence is stale or foreign".into()))?;
        if current != *picture {
            return Err(Error::Other("picture occurrence source changed".into()));
        }
        if (width, height) == (picture.width, picture.height) {
            return Ok(());
        }
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, owner) = candidate.story_source_and_owner(&picture.location.story)?;
        let part_name = source.part_name.clone();
        let spans = story_range_paragraph_spans(
            &source.xml,
            &[(picture.location.story.clone(), owner.clone())],
        )?
        .into_iter()
        .flatten();
        let span = spans
            .into_iter()
            .find(|(location, _)| location == &picture.location)
            .map(|(_, span)| span)
            .ok_or_else(|| Error::Other("picture paragraph disappeared".into()))?;
        let scope = story_namespace_scope_at(&source.xml, span.start)?;
        let paragraph = &source.xml[span.clone()];
        let matches = paragraph
            .windows(picture.source.len())
            .enumerate()
            .filter(|(_, bytes)| *bytes == picture.source.as_slice())
            .map(|(offset, _)| offset)
            .collect::<Vec<_>>();
        let equal_before = candidate
            .pictures()?
            .into_iter()
            .filter(|other| {
                other.location == picture.location
                    && other.ordinal < picture.ordinal
                    && other.source == picture.source
            })
            .count();
        let offset = *matches
            .get(equal_before)
            .ok_or_else(|| Error::Other("picture physical source disappeared".into()))?;
        let closed = close_content_fragment_namespaces(&picture.source, &scope)?;
        let (resized, count) =
            resize_story_pictures(&closed, &picture.relationship_id, width, height)?;
        if count != 1 {
            return Err(Error::Other(
                "picture occurrence has unsupported nested representation".into(),
            ));
        }
        // Namespace closure is projection-only. Numeric edits are transported
        // back onto the exact physical owner without rewriting its bindings.
        let (boundaries, _) = comment_namespace_source_boundaries(&picture.source, &closed)?
            .ok_or_else(|| Error::Other("picture namespace closure changed source".into()))?;
        let mut edits = Vec::new();
        let mut before_reader = NsReader::from_reader(closed.as_slice());
        let mut after_reader = NsReader::from_reader(resized.as_slice());
        loop {
            let start = before_reader.buffer_position() as usize;
            let before = before_reader
                .read_event()
                .map_err(|e| Error::Other(e.to_string()))?;
            let after = after_reader
                .read_event()
                .map_err(|e| Error::Other(e.to_string()))?;
            if before != after {
                let physical_start = boundaries
                    .iter()
                    .find_map(|(raw, closed)| (*closed == start).then_some(*raw))
                    .ok_or_else(|| Error::Other("picture edit has no physical boundary".into()))?;
                let end = before_reader.buffer_position() as usize;
                let physical_end = boundaries
                    .iter()
                    .find_map(|(raw, closed)| (*closed == end).then_some(*raw))
                    .ok_or_else(|| Error::Other("picture edit has no physical boundary".into()))?;
                // Only numeric values change, the original element spelling
                // and all other attributes remain byte-exact.
                let Event::Empty(ref before_tag) = before else {
                    return Err(Error::Other("picture extent source is unsupported".into()));
                };
                let Event::Empty(ref after_tag) = after else {
                    return Err(Error::Other("picture extent source is unsupported".into()));
                };
                let mut tag = picture.source[physical_start..physical_end].to_vec();
                let mut values = Vec::new();
                for (a, b) in before_tag.attributes().zip(after_tag.attributes()) {
                    let (a, b) = (
                        a.map_err(|e| Error::Other(e.to_string()))?,
                        b.map_err(|e| Error::Other(e.to_string()))?,
                    );
                    if a.key != b.key {
                        return Err(Error::Other(
                            "picture resize changed attribute ownership".into(),
                        ));
                    }
                    if a.value != b.value {
                        let (a_start, a_end) = story_attribute_value_span(&tag, a.key.as_ref())
                            .ok_or_else(|| {
                                Error::Other("picture extent attribute missing".into())
                            })?;
                        values.push((a_start..a_end, b.value.to_vec()));
                    }
                }
                values.sort_by_key(|(range, _)| range.start);
                for (range, value) in values.into_iter().rev() {
                    tag.splice(range, value);
                }
                edits.push((physical_start..physical_end, tag));
            }
            if matches!(before, Event::Eof) {
                break;
            }
        }
        let mut raw = picture.source.clone();
        for (range, value) in edits.into_iter().rev() {
            raw.splice(range, value);
        }
        let mut updated = source.xml.into_owned();
        updated.splice(
            span.start + offset..span.start + offset + picture.source.len(),
            raw,
        );
        set_story_source_xml(&mut candidate, &part_name, updated)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Fetch image bytes through a checked story-local relationship.
    pub fn image_data_for_story(&self, story: &StoryId, relationship_id: &str) -> Result<Vec<u8>> {
        let target = self.validate_internal_relationship_for_story(
            story,
            relationship_id,
            rel_types::IMAGE,
        )?;
        self.package
            .get_part(&target)
            .map(ToOwned::to_owned)
            .ok_or_else(|| Error::Other(format!("image relationship target {target} is missing")))
    }

    /// Replace a picture through a checked story-local relationship.
    pub fn replace_image_for_story(
        &mut self,
        story: &StoryId,
        relationship_id: &str,
        image_data: &[u8],
    ) -> Result<()> {
        self.validate_internal_relationship_for_story(story, relationship_id, rel_types::IMAGE)?;
        self.replace_image_relationship(&story.part_name, relationship_id, image_data)
    }

    fn replace_image_relationship(
        &mut self,
        owner: &str,
        relationship_id: &str,
        image_data: &[u8],
    ) -> Result<()> {
        let format = oxoffice_media::ImageFormat::sniff(image_data).ok_or_else(|| {
            Error::Other("replacement bytes are not a supported image".to_owned())
        })?;
        let old_target = self
            .package
            .get_part_rels(owner)
            .and_then(|relationships| relationships.get_by_id(relationship_id))
            .filter(|relationship| {
                relationship.rel_type == rel_types::IMAGE
                    && relationship_is_internal(relationship)
            })
            .map(|relationship| OpcPackage::resolve_rel_target(owner, &relationship.target))
            .ok_or_else(|| {
                Error::Other(format!(
                    "relationship owner {owner} has no internal image relationship {relationship_id}"
                ))
            })?;
        if !self.package.contains_part(&old_target) {
            return Err(Error::Other(format!(
                "image relationship target {old_target} is missing"
            )));
        }

        let mut candidate = self.clone_for_staging();
        let reference_count = std::iter::once(("/", &candidate.package.package_rels))
            .chain(
                candidate
                    .package
                    .part_rels
                    .iter()
                    .map(|(source, relationships)| (source.as_str(), relationships)),
            )
            .flat_map(|(source, relationships)| {
                relationships
                    .items
                    .iter()
                    .map(move |relationship| (source, relationship))
            })
            .filter(|(source, relationship)| {
                relationship_is_internal(relationship)
                    && OpcPackage::resolve_rel_target(source, &relationship.target) == old_target
            })
            .count();
        let compatible_extension = old_target
            .rsplit_once('.')
            .and_then(|(_, extension)| oxoffice_media::ImageFormat::from_extension(extension))
            == Some(format);
        let new_target = if reference_count == 1 && compatible_extension {
            old_target.clone()
        } else {
            candidate
                .identifiers
                .reserve_part_name("/word/media", "image", format.extension())?
        };

        if new_target != old_target {
            let relationship = candidate
                .package
                .get_part_rels_mut(owner)
                .and_then(|relationships| {
                    relationships
                        .items
                        .iter_mut()
                        .find(|relationship| relationship.id == relationship_id)
                })
                .ok_or_else(|| {
                    Error::Other(format!(
                        "image relationship {relationship_id} disappeared during replacement"
                    ))
                })?;
            relationship.target = relative_target(owner, &new_target);
        }
        candidate.install_reserved_image_part(&new_target, image_data, format);

        if new_target != old_target {
            let old_target_is_referenced = std::iter::once(("/", &candidate.package.package_rels))
                .chain(
                    candidate
                        .package
                        .part_rels
                        .iter()
                        .map(|(source, relationships)| (source.as_str(), relationships)),
                )
                .any(|(source, relationships)| {
                    relationships.items.iter().any(|relationship| {
                        relationship_is_internal(relationship)
                            && OpcPackage::resolve_rel_target(source, &relationship.target)
                                == old_target
                    })
                });
            if !old_target_is_referenced {
                candidate.package.remove_part(&old_target);
                candidate.package.remove_part_rels(&old_target);
                candidate.package.content_types.remove_override(&old_target);
                candidate.identifiers.retire_authored_part(&old_target);
                if let Some((_, extension)) = old_target.rsplit_once('.') {
                    let extension = extension.to_ascii_lowercase();
                    let default_is_used = candidate.package.parts.keys().any(|part_name| {
                        !candidate.package.content_types.contains_override(part_name)
                            && part_name.rsplit_once('.').is_some_and(|(_, candidate)| {
                                candidate.eq_ignore_ascii_case(&extension)
                            })
                    });
                    if !default_is_used
                        && candidate
                            .identifiers
                            .authored_content_type_defaults
                            .remove(&extension)
                    {
                        candidate.package.content_types.remove_default(&extension);
                        candidate
                            .identifiers
                            .content_type_defaults
                            .remove(&extension);
                    }
                }
            }
        }
        candidate.invalidate_layout();
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Resolve an external hyperlink through a checked story-local relationship.
    pub fn hyperlink_url_for_story(
        &self,
        story: &StoryId,
        relationship_id: &str,
    ) -> Result<String> {
        self.story_source_and_owner(story)?;
        self.hyperlink_url_for_validated_story(story, relationship_id)
    }

    fn hyperlink_url_for_validated_story(
        &self,
        story: &StoryId,
        relationship_id: &str,
    ) -> Result<String> {
        let relationships = self
            .package
            .get_part_rels(&story.part_name)
            .ok_or_else(|| {
                Error::Other(format!(
                    "story owner {} has no relationship scope",
                    story.part_name
                ))
            })?;
        let relationship = relationships.get_by_id(relationship_id).ok_or_else(|| {
            Error::Other(format!(
                "story owner {} has no relationship {relationship_id}",
                story.part_name
            ))
        })?;
        if relationship.rel_type != rel_types::HYPERLINK
            || relationship.target_mode.as_deref() != Some("External")
        {
            return Err(Error::Other(format!(
                "story relationship {relationship_id} is not an external hyperlink"
            )));
        }
        Ok(relationship.target.clone())
    }

    /// Replace the visible text of one checked story item atomically.
    /// Replace one checked story item in its actual package owner.
    ///
    /// The item kind, physical story and surrounding bytes remain unchanged.
    /// Relationship-bearing content must already belong to this story part.
    pub fn replace_story_content(
        &mut self,
        location: &ContentLocation,
        fragment: ContentFragment,
    ) -> Result<()> {
        if location.is_end
            || location.item_kind != fragment.kind
            || location.item_kind == StoryItemKind::PreservedNode
        {
            return Err(Error::Other(
                "story replacement requires the same modeled item kind".to_owned(),
            ));
        }
        reject_section_owning_content_fragment(&fragment)?;
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let owner = if location.item_kind == StoryItemKind::Paragraph {
            let (_, owner) = candidate.story_source_and_owner(&location.story)?;
            Some(owner.clone())
        } else {
            None
        };
        let (source, item) = if location.item_kind == StoryItemKind::Paragraph {
            candidate.story_range_paragraph_source(location)?
        } else {
            candidate.story_item_source(location)?
        };
        if item.kind != location.item_kind {
            return Err(StoryError::KindMismatch {
                expected: location.item_kind,
                actual: item.kind,
            }
            .into());
        }
        let part_name = source.part_name.clone();
        let item_range = item.full.clone();
        let source_xml = source.xml.clone().into_owned();
        drop(source);
        let paired = match &owner {
            Some(owner)
                if source_xml
                    .windows(b"AlternateContent".len())
                    .any(|window| window == b"AlternateContent") =>
            {
                paired_scoped_ranges(&source_xml, owner, &item_range, None)?
            }
            _ => None,
        };
        let replacement = content_fragment_for_insertion(&candidate, &location.story, &fragment)?;
        let mut xml = source_xml;
        if let Some(ranges) = paired {
            let mut edits = Vec::with_capacity(ranges.len());
            for range in ranges {
                let scope = story_namespace_scope_at(&xml, range.start)?;
                edits.push((
                    range,
                    close_content_fragment_namespaces(&replacement, &scope)?,
                ));
            }
            for (range, bytes) in edits.into_iter().rev() {
                xml.splice(range, bytes);
            }
        } else {
            let scope = story_namespace_scope_at(&xml, item_range.start)?;
            let replacement = close_content_fragment_namespaces(&replacement, &scope)?;
            xml.splice(item_range, replacement);
        }
        set_story_source_xml(&mut candidate, &part_name, xml)?;
        candidate.reconcile_comment_removal(self)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Apply checked element content only when paragraph-owned section profiles
    /// match the selected original. Generic content insertion keeps its stricter
    /// section-owner refusal contract.
    fn replace_checked_element_content(
        &mut self,
        location: &ContentLocation,
        fragment: ContentFragment,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();

        candidate.flush_to_package()?;

        let (source, item) = if location.item_kind == StoryItemKind::Paragraph {
            candidate.story_range_paragraph_source(location)?
        } else {
            candidate.story_item_source(location)?
        };
        if location.is_end || item.kind != fragment.kind || item.kind != location.item_kind {
            return Err(Error::Other(
                "element XML requires the same existing physical owner".into(),
            ));
        }
        let part_name = source.part_name.clone();
        let scope = story_namespace_scope_at(&source.xml, item.full.start)?;
        let original = close_content_fragment_namespaces(&source.xml[item.full.clone()], &scope)?;
        let incoming = close_content_fragment_namespaces(&fragment.xml, &fragment.namespace_scope)?;
        let profiles = |xml: &[u8]| -> Result<Vec<CT_SectPr>> {
            let mut wrapped =
                format!(r#"<w:document xmlns:w="{WORD_NAMESPACE}"><w:body>"#).into_bytes();
            wrapped.extend_from_slice(xml);
            wrapped.extend_from_slice(b"</w:body></w:document>");
            // Checked paragraph XML writes canonical `w14:paraId` without a
            // local declaration. The wrapper root has to bind that prefix
            // before the section-profile parse, or the attribute is unbound.
            declare_w14_on_part_root(&mut wrapped)?;
            let document = CT_Document::from_xml(&wrapped)?;
            let mut profiles = Vec::new();
            visit_body_paragraphs(&document.body.content, false, &mut |paragraph| {
                if let Some(section) = paragraph
                    .properties
                    .as_ref()
                    .and_then(|properties| properties.sect_pr.as_ref())
                {
                    profiles.push(section.clone());
                }
            });
            Ok(profiles)
        };
        if profiles(&original)? != profiles(&incoming)? {
            return Err(Error::Other("element XML changed or introduced paragraph-owned section properties are unsupported, use the checked section owner".into()));
        }
        let mut xml = source.xml.into_owned();
        let replacement = content_fragment_for_insertion(&candidate, &location.story, &fragment)?;

        let replacement = close_content_fragment_namespaces(&replacement, &scope)?;

        xml.splice(item.full, replacement);
        set_story_source_xml(&mut candidate, &part_name, xml)?;

        candidate.reconcile_comment_removal(self)?;

        let reopened = candidate.prepare_and_reopen_staged()?;

        self.commit_staged_mutation(reopened);

        Ok(())
    }

    /// Namespace-complete XML for the held physical paragraph owner.
    #[doc(hidden)]
    pub fn paragraph_element_xml(&self, location: &ContentLocation) -> Result<Vec<u8>> {
        Ok(self
            .story_range_paragraph_snapshot(location)?
            .xml()
            .to_vec())
    }

    /// Namespace-complete XML for an accepted run in a held paragraph owner.
    #[doc(hidden)]
    pub fn run_element_xml(
        &self,
        location: &ContentLocation,
        path: &oxoffice_docx_oxml::text::AcceptedRunPath,
    ) -> Result<Vec<u8>> {
        let snapshot = self.story_range_paragraph_snapshot(location)?;
        let paragraph = CT_P::from_xml_fragment(snapshot.xml())?;
        let run = paragraph
            .accepted_run(path)
            .ok_or_else(|| Error::Other("element XML run owner is absent".into()))?;
        let mut bytes = Vec::new();
        run.to_xml(&mut Writer::new(&mut bytes))?;
        let scope = BTreeMap::from([
            ("w".to_owned(), WORD_NAMESPACE.to_owned()),
            (
                "r".to_owned(),
                oxoffice_docx_oxml::namespace::R_NS.to_owned(),
            ),
        ]);
        close_content_fragment_namespaces(&bytes, &scope)
    }

    /// The main part bytes for `item` alone in this body, and the item's
    /// namespace-complete XML there, closed as [`Self::close_direct_fragment`]
    /// closes it.
    fn standalone_body_item(&self, item: &BodyContent) -> Result<(Vec<u8>, Vec<u8>)> {
        let part = self.document.standalone_body_item_xml(item)?;
        let (start, end) = standalone_body_item_span(&part)?;
        let xml = self.close_direct_fragment(&part, start..end)?;
        Ok((part, xml))
    }

    /// Close the fragment at `span` of a standalone main part over the
    /// declarations in scope there, as the whole part would close it, except
    /// for `w14`.
    ///
    /// The whole part declares `w14` on its root when any of its bytes use it,
    /// so a whole-part read carries that declaration whenever any body item
    /// uses it. A direct read declares it when the original root does or the
    /// fragment itself uses it, by name or in a markup compatibility list.
    fn close_direct_fragment(&self, part: &[u8], span: Range<usize>) -> Result<Vec<u8>> {
        let mut scope = story_namespace_scope_at(part, span.start)?;
        let fragment = &part[span];
        let root_declares = self
            .document
            .extra_namespaces
            .iter()
            .any(|(name, _)| name == "xmlns:w14");
        if !root_declares {
            if fragment_uses_w14(fragment)? {
                scope
                    .entry("w14".to_owned())
                    .or_insert_with(|| W14_NAMESPACE.to_owned());
            } else {
                scope.remove("w14");
            }
        }
        close_content_fragment_namespaces(fragment, &scope)
    }

    /// Namespace-complete XML for the direct body paragraph at `slot`, read
    /// without serializing, scanning or admitting the rest of the body.
    ///
    /// The XML is [`Self::paragraph_element_xml`] for that paragraph up to
    /// the `w14` declaration, which [`Self::close_direct_fragment`] decides
    /// from the root and the paragraph alone. `None` when the child is not a
    /// paragraph.
    #[doc(hidden)]
    pub fn direct_body_paragraph_xml(&self, slot: usize) -> Result<Option<Vec<u8>>> {
        let Some(item @ BodyContent::Paragraph(_)) = self.document.body.content.get(slot) else {
            return Ok(None);
        };
        let (_, xml) = self.standalone_body_item(item)?;
        CT_P::from_xml_fragment(&xml)?;
        Ok(Some(xml))
    }

    /// Namespace-complete XML for an accepted run of the direct body
    /// paragraph at `slot`, the bytes [`Self::run_element_xml`] returns for
    /// it, read without serializing or scanning the rest of the body.
    #[doc(hidden)]
    pub fn direct_body_run_xml(
        &self,
        slot: usize,
        path: &oxoffice_docx_oxml::text::AcceptedRunPath,
    ) -> Result<Option<Vec<u8>>> {
        let Some(item @ BodyContent::Paragraph(_)) = self.document.body.content.get(slot) else {
            return Ok(None);
        };
        // The run closes over a fixed scope, so the paragraph's own closure
        // only has to parse.
        let (_, xml) = self.standalone_body_item(item)?;
        let paragraph = CT_P::from_xml_fragment(&xml)?;
        let run = paragraph
            .accepted_run(path)
            .ok_or_else(|| Error::Other("element XML run owner is absent".into()))?;
        let mut bytes = Vec::new();
        run.to_xml(&mut Writer::new(&mut bytes))?;
        let scope = BTreeMap::from([
            ("w".to_owned(), WORD_NAMESPACE.to_owned()),
            (
                "r".to_owned(),
                oxoffice_docx_oxml::namespace::R_NS.to_owned(),
            ),
        ]);
        let closed = close_content_fragment_namespaces(&bytes, &scope)?;
        Ok(Some(closed))
    }

    /// Namespace-complete XML for the direct body table at `slot`, read as
    /// [`Self::direct_body_paragraph_xml`] reads a paragraph, the XML of
    /// [`Self::table_element_xml`] up to the `w14` declaration.
    #[doc(hidden)]
    pub fn direct_body_table_xml(&self, slot: usize) -> Result<Option<Vec<u8>>> {
        let Some(item @ BodyContent::Table(_)) = self.document.body.content.get(slot) else {
            return Ok(None);
        };
        Ok(Some(self.standalone_body_item(item)?.1))
    }

    /// Namespace-complete XML for a physical cell of the direct body table at
    /// `slot`, read as [`Self::direct_body_table_xml`] reads its table, the
    /// XML of [`Self::cell_element_xml`] up to the `w14` declaration, which
    /// the root and the cell alone decide.
    #[doc(hidden)]
    pub fn direct_body_cell_xml(
        &self,
        slot: usize,
        row: usize,
        cell: usize,
    ) -> Result<Option<Vec<u8>>> {
        let Some(item @ BodyContent::Table(_)) = self.document.body.content.get(slot) else {
            return Ok(None);
        };
        let (part, _) = self.standalone_body_item(item)?;
        let body = scan_story_owners(&part, StoryKind::Body)?
            .into_iter()
            .find(|owner| owner.kind == StoryKind::Body)
            .ok_or_else(|| Error::Other("standalone body item has no body owner".into()))?;
        let table = scan_story_items(&part, &body, None)?
            .into_iter()
            .find(|item| item.direct_owner_child)
            .ok_or_else(|| Error::Other("standalone body table has no item".into()))?;
        let owner = physical_table_cell_owner(&part, StoryKind::Body, &table, row, cell)?;
        let xml = self.close_direct_fragment(&part, owner.full)?;
        Ok(Some(xml))
    }

    /// The paragraph [`Self::insert_paragraph`] and [`Self::add_paragraph`]
    /// place for `text`, built before it is placed.
    #[doc(hidden)]
    pub fn detached_text_paragraph(text: &str) -> CT_P {
        let mut p = CT_P::new();
        if !text.is_empty() {
            p.add_run(text);
        }
        p
    }

    /// The accepted text and run paths that a checked owner inventory reads
    /// for `paragraph` placed as a direct body paragraph of this document.
    ///
    /// `None` unless the paragraph is plain and owns no section, so that it
    /// is one body story item and owns no other story or route.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // The two read facts of one paragraph owner.
    pub fn direct_body_paragraph_facts(
        &self,
        paragraph: &CT_P,
    ) -> Result<Option<(String, Vec<oxoffice_docx_oxml::text::AcceptedRunPath>)>> {
        if !paragraph_is_plain(paragraph)
            || paragraph
                .properties
                .as_ref()
                .is_some_and(|properties| properties.sect_pr.is_some())
        {
            return Ok(None);
        }
        paragraph.validate_resource_limits()?;
        // The undecided `w14` declaration names no prefix the paragraph uses.
        let (_, xml) =
            self.standalone_body_item(&BodyContent::Paragraph(Box::new(paragraph.clone())))?;
        let parsed = CT_P::from_xml_fragment(&xml)?;
        Ok(Some((parsed.accepted_text(), parsed.accepted_run_paths())))
    }

    /// Read plain opened body paragraphs under one exact opening proof.
    ///
    /// This finite reader-invariance proof returns facts only, never Source
    /// input or retained authority. Every miss leaves the full writer available.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // Existing paragraph text and run-path facts, in physical slot order.
    pub fn opened_plain_body_paragraph_facts(
        &self,
    ) -> Result<
        Option<
            Vec<(
                Option<Arc<BodyContent>>,
                String,
                Vec<oxoffice_docx_oxml::text::AcceptedRunPath>,
            )>,
        >,
    > {
        let Some(kept) = self.kept_main_part() else {
            return Ok(None);
        };
        if self.plain_local_edit_blocked
            || kept.serialization_proof.is_none()
            || kept.root_namespace_declarations != self.root_namespace_declarations
            || kept.body_namespace_declarations != self.body_namespace_declarations
            || kept.body_namespace_bindings != self.body_namespace_bindings
        {
            return Ok(None);
        }
        let Some(opened) = kept.main_document() else {
            return Ok(None);
        };
        let CT_Document {
            body:
                oxoffice_docx_oxml::document::CT_Body {
                    content: current,
                    sect_pr: current_section,
                },
            extra_namespaces: current_namespaces,
            background_xml: current_background,
            background_extra_xml: current_background_extra,
            root_attributes: current_attributes,
        } = &*self.document;
        let CT_Document {
            body:
                oxoffice_docx_oxml::document::CT_Body {
                    content: opened_content,
                    sect_pr: opened_section,
                },
            extra_namespaces: opened_namespaces,
            background_xml: opened_background,
            background_extra_xml: opened_background_extra,
            root_attributes: opened_attributes,
        } = opened;
        if (
            current_section,
            current_namespaces,
            current_background,
            current_background_extra,
            current_attributes,
        ) != (
            opened_section,
            opened_namespaces,
            opened_background,
            opened_background_extra,
            opened_attributes,
        ) || current.len() != opened_content.len()
        {
            return Ok(None);
        }
        let same_opening = kept
            .main_document
            .get()
            .and_then(Option::as_ref)
            .is_some_and(|opened| self.document.same_opening_clone(opened));
        // Compare and qualify each physical item while its values are in use.
        // Every opening item must match before any finite result or fallback.
        let facts = {
            let scans = self
                .main_body_scans
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut models = offered_canonical_blocks(
                scans.serialized.as_ref().or(self.main_part_blocks.as_ref()),
            )
            .filter(|record| record.blocks.len() == current.len());
            let staged_origin = same_opening
                && models.is_some_and(|record| {
                    kept.initial_writer_blocks
                        .as_ref()
                        .and_then(std::sync::Weak::upgrade)
                        .is_some_and(|opening| Arc::ptr_eq(&opening, &record.blocks))
                });
            let mut qualified_properties: Option<&oxoffice_docx_oxml::properties::CT_RPr> = None;
            let mut facts = Some(Vec::with_capacity(current.len()));
            for (slot, (item, opened_item)) in current.iter().zip(opened_content).enumerate() {
                let mut qualify = || {
                    let BodyContent::Paragraph(paragraph) = item else {
                        return None;
                    };
                    if !paragraph_has_finite_plain_properties(paragraph, &mut qualified_properties)
                    {
                        return None;
                    }
                    // These are the same finite writer-invariant facts.
                    let length = paragraph
                        .runs
                        .iter()
                        .flat_map(|run| &run.content)
                        .try_fold(0usize, |length, content| match content {
                            RunContent::Text(text) => length.checked_add(text.text.len()),
                            _ => None,
                        })
                        .filter(|length| *length <= isize::MAX as usize);
                    let text = length
                        .map(|length| {
                            let mut text = String::with_capacity(length);
                            for run in &paragraph.runs {
                                for content in &run.content {
                                    let RunContent::Text(content) = content else {
                                        return paragraph.accepted_text();
                                    };
                                    text.push_str(&content.text);
                                }
                            }
                            text
                        })
                        .unwrap_or_else(|| paragraph.accepted_text());
                    Some((text, paragraph.accepted_run_paths()))
                };
                // Only proved plain values may omit equality. Unsupported
                // equality can itself read XML and retains its original order.
                let early = (same_opening && facts.is_some()).then(&mut qualify);
                let finite_origin = early.as_ref().is_some_and(Option::is_some);
                if !finite_origin && item != opened_item {
                    return Ok(None);
                }
                if let Some(record) = models
                    && !(finite_origin && staged_origin)
                    && record.blocks[slot].staged.as_deref() != Some(item)
                {
                    models = None;
                }
                let Some(values) = facts.as_mut() else {
                    continue;
                };
                let finite = early.unwrap_or_else(qualify);
                match finite {
                    Some((text, runs)) => values.push((None, text, runs)),
                    None => facts = None,
                }
            }
            // Only a complete record alignment may lend any model value.
            if let (Some(values), Some(record)) = (facts.as_mut(), models) {
                for ((model, _, _), block) in values.iter_mut().zip(record.blocks.iter()) {
                    *model = block.staged.clone();
                }
            }
            facts
        };
        // The finite graph has exact resource maxima (0, 0). Unsupported
        // payloads retain the full guard, after the complete opening check
        // and after releasing the temporary writer-record lock.
        if facts.is_none() {
            self.document.validate_resource_limits()?;
        }
        Ok(facts)
    }

    /// The number of body story items that the direct body child at `slot`
    /// contributes, itself included, without scanning the rest of the body.
    #[doc(hidden)]
    pub fn direct_body_story_item_count(&self, slot: usize) -> Result<Option<usize>> {
        let Some(item) = self.document.body.content.get(slot) else {
            return Ok(None);
        };
        let (part, _) = self.standalone_body_item(item)?;
        let owner = scan_story_owners(&part, StoryKind::Body)?
            .into_iter()
            .find(|owner| owner.kind == StoryKind::Body)
            .ok_or_else(|| Error::Other("standalone body item has no body owner".into()))?;
        Ok(Some(scan_story_items(&part, &owner, None)?.len()))
    }

    /// The number of paragraphs [`Self::paragraphs`] counts before body child
    /// `slot`, without reading the body's XML.
    #[doc(hidden)]
    pub fn paragraph_count_before_content(&self, slot: usize) -> usize {
        self.document.body.paragraph_count_before(slot)
    }

    /// Whether removal takes the direct body child at `slot` alone.
    ///
    /// A plain paragraph with no section qualifies. Retained `w:sdt`
    /// namespace owners whose current replay still saves keep that removal
    /// local. Any other unsavable body stays on the checked staged path.
    #[doc(hidden)]
    pub fn plain_removable_paragraph(&self, slot: usize) -> bool {
        let plain = matches!(
            self.document.body.content.get(slot),
            Some(BodyContent::Paragraph(paragraph))
                if paragraph_is_plain(paragraph)
                    && paragraph
                        .properties
                        .as_ref()
                        .is_none_or(|properties| properties.sect_pr.is_none())
        );
        if !plain || self.unsavable_body_edit().is_some() {
            return false;
        }
        if !self.plain_local_edit_blocked {
            return true;
        }
        let Some(xml) = self.package.get_part(&self.doc_part_name) else {
            return false;
        };
        self.kept_main_part()
            .and_then(|kept| kept.nested_namespace_owners().map(<[_]>::to_vec))
            .or_else(|| self.main_part_blocks.as_ref()?.retained_nested_owners(xml))
            .or_else(|| nested_modeled_namespace_owners(xml).ok())
            .is_some_and(|owners| {
                !owners.is_empty() && owners.iter().all(|owner| owner.local_name == "sdt")
            })
    }

    /// Namespace-complete XML for an existing physical table owner.
    #[doc(hidden)]
    pub fn table_element_xml(&self, location: &ContentLocation) -> Result<Vec<u8>> {
        let (source, item) = self.story_item_source(location)?;
        if item.kind != StoryItemKind::Table || location.is_end {
            return Err(Error::Other(
                "element XML needs an existing physical table owner".into(),
            ));
        }
        let scope = story_namespace_scope_at(&source.xml, item.full.start)?;
        close_content_fragment_namespaces(&source.xml[item.full], &scope)
    }

    /// Namespace-complete XML for a physical cell, including an empty cell.
    #[doc(hidden)]
    pub fn cell_element_xml(
        &self,
        table: &ContentLocation,
        row: usize,
        cell: usize,
    ) -> Result<Vec<u8>> {
        let owner = self.story_table_cell(table, row, cell)?;
        let (source, span) = self.story_source_and_owner(&owner)?;
        let scope = story_namespace_scope_at(&source.xml, span.full.start)?;
        close_content_fragment_namespaces(&source.xml[span.full], &scope)
    }

    /// Namespace-complete XML for an existing section properties owner.
    #[doc(hidden)]
    pub fn section_properties_xml(&self, index: usize) -> Result<Vec<u8>> {
        let section = self
            .section(index)
            .ok_or_else(|| Error::Other("element XML section owner is absent".into()))?;
        let mut bytes = Vec::new();
        section.properties().to_xml(&mut Writer::new(&mut bytes))?;
        let scope = BTreeMap::from([
            ("w".to_owned(), WORD_NAMESPACE.to_owned()),
            (
                "r".to_owned(),
                oxoffice_docx_oxml::namespace::R_NS.to_owned(),
            ),
        ]);
        close_content_fragment_namespaces(&bytes, &scope)
    }

    /// Replace the checked existing paragraph owner atomically.
    #[doc(hidden)]
    pub fn replace_paragraph_xml(&mut self, location: &ContentLocation, xml: &str) -> Result<()> {
        self.validate_model_resource_limits()?;
        if let Some(slot) = direct_body_paragraph_slot(location) {
            match self.try_replace_plain_body_paragraph(slot, xml)? {
                None => {}
                Some(_) => return Ok(()),
            }
        }
        let selected = self.story_range_paragraph_snapshot(location)?;
        if selected.xml() == xml.as_bytes() {
            return Ok(());
        }
        let paragraph = CT_P::from_xml_fragment_checked(xml.as_bytes())?;
        let (source, item) = self.story_range_paragraph_source(location)?;
        let context = edited_paragraph_ancestor_context(&source.xml, item.full.start)?;
        admit_edited_paragraph(selected.xml(), xml.as_bytes(), &context)?;
        validate_element_owner_relationships(
            self,
            &location.story,
            WordXmlOwner::Paragraph(&paragraph),
        )?;
        validate_element_xml_identity_change(
            self,
            &location.story,
            selected.xml(),
            xml.as_bytes(),
        )?;
        let fragment = ContentFragment {
            kind: StoryItemKind::Paragraph,
            xml: xml.as_bytes().to_vec(),
            namespace_scope: BTreeMap::new(),
            source_part_name: Some(location.story.part_name.clone()),
            source_story_kind: Some(location.story.kind),
            source_owner_index: Some(location.story.owner_index),
        };
        self.replace_checked_element_content(location, fragment)
    }

    #[doc(hidden)]
    /// Replace one existing accepted regular run through its physical
    /// paragraph owner, never by inserting a run at an old numeric slot.
    pub fn replace_run_xml(&mut self, position: &StoryRunPosition, xml: &str) -> Result<()> {
        self.validate_model_resource_limits()?;
        let snapshot = self.story_range_paragraph_snapshot(&position.location)?;
        let mut paragraph = CT_P::from_xml_fragment(snapshot.xml())?;
        let path = paragraph
            .accepted_run_paths()
            .get(position.run_index)
            .cloned()
            .ok_or_else(|| Error::Other("element XML run index is out of range".into()))?;
        let current = paragraph
            .accepted_run(&path)
            .ok_or_else(|| Error::Other("element XML run owner is absent".into()))?;
        if current
            .content
            .iter()
            .any(|content| matches!(content, RunContent::Field(_)))
        {
            return Err(Error::Other(
                "element XML regular run replacement cannot replace a field owner".into(),
            ));
        }
        let mut current_xml = Vec::new();
        current.to_xml(&mut Writer::new(&mut current_xml))?;
        let bindings = BTreeMap::from([
            ("w".to_owned(), WORD_NAMESPACE.to_owned()),
            (
                "r".to_owned(),
                oxoffice_docx_oxml::namespace::R_NS.to_owned(),
            ),
        ]);
        let current_xml = close_content_fragment_namespaces(&current_xml, &bindings)?;
        if current_xml == xml.as_bytes() {
            return Ok(());
        }
        let replacement = CT_R::from_xml_fragment_checked(xml.as_bytes())?;
        validate_element_owner_relationships(
            self,
            &position.location.story,
            WordXmlOwner::Run(&replacement),
        )?;
        validate_element_xml_identity_change(
            self,
            &position.location.story,
            &current_xml,
            xml.as_bytes(),
        )?;
        if !paragraph.replace_accepted_run(&path, replacement)? {
            return Err(Error::Other("element XML run owner disappeared".into()));
        }
        let mut fragment = ContentFragment::paragraph(paragraph)?;
        fragment.source_part_name = Some(position.location.story.part_name.clone());
        fragment.source_story_kind = Some(position.location.story.kind);
        fragment.source_owner_index = Some(position.location.story.owner_index);
        self.replace_checked_element_content(&position.location, fragment)
    }

    /// Replace one checked physical table atomically.
    #[doc(hidden)]
    pub fn replace_table_xml(&mut self, location: &ContentLocation, xml: &str) -> Result<()> {
        self.validate_model_resource_limits()?;

        let (source, item) = self.story_item_source(location)?;

        if item.kind != StoryItemKind::Table || location.is_end {
            return Err(Error::Other(
                "element XML needs an existing physical table owner".into(),
            ));
        }
        let scope = story_namespace_scope_at(&source.xml, item.full.start)?;
        let current = close_content_fragment_namespaces(&source.xml[item.full], &scope)?;
        if current == xml.as_bytes() {
            return Ok(());
        }
        let table = CT_Tbl::from_xml_fragment_checked(xml.as_bytes())?;

        validate_element_owner_relationships(self, &location.story, WordXmlOwner::Table(&table))?;

        validate_element_xml_identity_change(self, &location.story, &current, xml.as_bytes())?;

        let fragment = ContentFragment {
            kind: StoryItemKind::Table,
            xml: xml.as_bytes().to_vec(),
            namespace_scope: BTreeMap::new(),
            source_part_name: Some(location.story.part_name.clone()),
            source_story_kind: Some(location.story.kind),
            source_owner_index: Some(location.story.owner_index),
        };
        self.replace_checked_element_content(location, fragment)
    }

    /// Replace exactly the source-qualified cell resolved by the held table.
    #[doc(hidden)]
    pub fn replace_cell_xml(
        &mut self,
        table: &ContentLocation,
        row: usize,
        cell: usize,
        xml: &str,
    ) -> Result<()> {
        self.validate_model_resource_limits()?;
        let owner = self.story_table_cell(table, row, cell)?;
        let (source, span) = self.story_source_and_owner(&owner)?;
        let scope = story_namespace_scope_at(&source.xml, span.full.start)?;
        let current = close_content_fragment_namespaces(&source.xml[span.full.clone()], &scope)?;
        if current == xml.as_bytes() {
            return Ok(());
        }
        let replacement = CT_Tc::from_xml_fragment_checked(xml.as_bytes())?;
        if replacement.content.is_empty() {
            return Err(Error::Other(
                "element XML cell requires modeled block content".into(),
            ));
        }
        validate_element_owner_relationships(self, &owner, WordXmlOwner::Cell(&replacement))?;
        validate_element_xml_identity_change(self, &owner, &current, xml.as_bytes())?;
        let original_table = self.story_table_model(table)?;
        let original_cell = original_table
            .rows
            .get(row)
            .and_then(|row| row.cells.get(cell))
            .ok_or_else(|| Error::Other("element XML original physical cell disappeared".into()))?;
        let profiles = |cell: &CT_Tc| {
            let mut profiles = Vec::new();
            visit_cell(cell, false, &mut |paragraph| {
                if let Some(section) = paragraph
                    .properties
                    .as_ref()
                    .and_then(|properties| properties.sect_pr.as_ref())
                {
                    profiles.push(section.clone());
                }
            });
            profiles
        };
        if profiles(original_cell) != profiles(&replacement) {
            return Err(Error::Other("element XML changed or introduced paragraph-owned section properties are unsupported, use the checked section owner".into()));
        }
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, span) = candidate.story_source_and_owner(&owner)?;
        let scope = story_namespace_scope_at(&source.xml, span.full.start)?;
        let part = source.part_name.clone();
        let mut bytes = source.xml.into_owned();
        bytes.splice(
            span.full,
            close_content_fragment_namespaces(xml.as_bytes(), &scope)?,
        );
        set_story_source_xml(&mut candidate, &part, bytes)?;
        // Only the closed candidate changed: replacing the qualified cell
        // cannot move its enclosing table or preceding owner boundary.
        // Rebind that candidate's story fingerprint before its topology check.
        let mut enclosing = table.clone();
        enclosing.story = candidate
            .stories()?
            .into_iter()
            .find(|story| {
                story.kind == table.story.kind
                    && story.part_name == table.story.part_name
                    && story.owner_index == table.story.owner_index
            })
            .ok_or_else(|| Error::Other("element XML enclosing table story disappeared".into()))?;
        validate_table_topology(&candidate.story_table_model(&enclosing)?)?;
        candidate.reconcile_comment_removal(self)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Replace an existing physical section owner through a closed candidate.
    #[doc(hidden)]
    pub fn replace_section_properties_xml(&mut self, index: usize, xml: &str) -> Result<()> {
        self.validate_model_resource_limits()?;
        let current = self
            .section(index)
            .ok_or_else(|| Error::Other("element XML section owner is absent".into()))?;
        let replacement = CT_SectPr::from_xml_fragment_checked(xml.as_bytes())?;
        for (references, kind) in [
            (&replacement.header_refs, HeaderFooterKind::Header),
            (&replacement.footer_refs, HeaderFooterKind::Footer),
        ] {
            let mut types = Vec::new();
            for reference in references {
                if types.contains(&reference.hdr_ftr_type) {
                    return Err(Error::Other(
                        "element XML section has duplicate header/footer types".into(),
                    ));
                }
                types.push(reference.hdr_ftr_type);
                let part = self
                    .header_footer_part_name(&reference.rel_id, kind == HeaderFooterKind::Header)
                    .ok_or_else(|| {
                        Error::Other(
                            "element XML section relationship kind or mode is invalid".into(),
                        )
                    })?;
                if self.package.content_types.content_type_for(&part) != Some(kind.content_type())
                    || self
                        .load_header_footer(&reference.rel_id, kind == HeaderFooterKind::Header)
                        .is_none()
                {
                    return Err(Error::Other(
                        "element XML section relationship target is invalid".into(),
                    ));
                }
            }
        }
        let mut bytes = Vec::new();
        current.properties().to_xml(&mut Writer::new(&mut bytes))?;
        let scope = BTreeMap::from([
            ("w".to_owned(), WORD_NAMESPACE.to_owned()),
            (
                "r".to_owned(),
                oxoffice_docx_oxml::namespace::R_NS.to_owned(),
            ),
        ]);
        if close_content_fragment_namespaces(&bytes, &scope)? == xml.as_bytes() {
            return Ok(());
        }
        let mut candidate = self.clone_for_staging();
        *candidate
            .section_mut(index)
            .ok_or_else(|| Error::Other("element XML section owner disappeared".into()))?
            .inner = replacement;
        if candidate
            .validate_sections()
            .findings
            .iter()
            .any(|finding| {
                finding.severity == ValidationSeverity::Error
                    && finding.message == format!("section {index} has nonpositive page size")
            })
        {
            return Err(Error::Other(
                "element XML section geometry has nonpositive page size".into(),
            ));
        }
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    fn story_table_model(&self, location: &ContentLocation) -> Result<CT_Tbl> {
        if location.item_kind != StoryItemKind::Table || location.is_end {
            return Err(Error::Other(
                "rich table needs an existing table location".to_owned(),
            ));
        }
        let (source, item) = self.story_item_source(location)?;
        Self::story_table_from_source(&source.xml, &item)
    }

    fn story_table_from_source(xml: &[u8], item: &StoryItemSpan) -> Result<CT_Tbl> {
        let scope = story_namespace_scope_at(xml, item.full.start)?;
        let closed = close_content_fragment_namespaces(&xml[item.full.clone()], &scope)?;
        let mut wrapped =
            format!(r#"<w:document xmlns:w="{WORD_NAMESPACE}"><w:body>"#).into_bytes();
        wrapped.extend_from_slice(&closed);
        wrapped.extend_from_slice(b"</w:body></w:document>");
        let mut parsed = CT_Document::from_xml(&wrapped)?;
        if parsed.body.content.len() != 1 {
            return Err(Error::Other(
                "rich table has ambiguous physical content".to_owned(),
            ));
        }
        match parsed.body.content.remove(0) {
            BodyContent::Table(table) => Ok(*table),
            _ => Err(Error::Other(
                "rich table source is not a qualified table".to_owned(),
            )),
        }
    }

    /// Read owned facts from one source-qualified table.
    #[doc(hidden)]
    pub fn read_story_table<T>(
        &self,
        location: &ContentLocation,
        read: impl FnOnce(TableRef<'_>) -> T,
    ) -> Result<T> {
        let table = self.story_table_model(location)?;
        Ok(read(TableRef { inner: &table }))
    }

    /// Edit one source-qualified table and publish it atomically.
    #[doc(hidden)]
    pub fn edit_story_table<T>(
        &mut self,
        location: &ContentLocation,
        edit: impl FnOnce(&mut Table<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut table = self.story_table_model(location)?;
        let result = edit(&mut Table { inner: &mut table })?;
        self.replace_story_content(location, ContentFragment::table(table)?)?;
        Ok(result)
    }

    /// Snapshot all checked Word owners within one immutable source inventory.
    ///
    /// The four families have the same order, payload and admission as story
    /// items, physical table cells, range paragraphs and hyperlinks respectively.
    /// Source qualification is shared only within this call, never across edits.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // One cross-crate projection carries four existing owned families.
    pub fn story_owner_snapshots(
        &self,
    ) -> Result<(
        Vec<StoryItemSnapshot>,
        Vec<(ContentLocation, usize, usize, StoryId)>,
        Vec<(ContentLocation, Vec<u8>)>,
        Vec<(ContentLocation, LinkInfo)>,
    )> {
        self.owner_snapshots_of(self.story_sources()?, false)
    }

    /// [`Self::story_owner_snapshots`] of a main part whose body holds only
    /// `table`, placed as a direct body table of this document, with the
    /// table's namespace-complete XML. Body routes are those of that body,
    /// and cell story owners count from zero.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // The table XML and the four owned families.
    pub fn detached_table_owner_snapshots(
        &self,
        table: &CT_Tbl,
    ) -> Result<(
        Vec<u8>,
        (
            Vec<StoryItemSnapshot>,
            Vec<(ContentLocation, usize, usize, StoryId)>,
            Vec<(ContentLocation, Vec<u8>)>,
            Vec<(ContentLocation, LinkInfo)>,
        ),
    )> {
        let (part, xml) =
            self.standalone_body_item(&BodyContent::Table(Box::new(table.clone())))?;
        let snapshots = self.owner_snapshots_of(
            vec![StorySource {
                root_kind: StoryKind::Body,
                part_name: self.doc_part_name.clone(),
                xml: Cow::Owned(part),
            }],
            false,
        )?;
        Ok((xml, snapshots))
    }

    /// The checked Word owners of a main part whose body holds only `item`,
    /// placed as a direct body child of this document.
    ///
    /// Body story items count from the item, and table cell and text box
    /// stories from its first of each, and each snapshot's XML is
    /// namespace-complete. Equal items
    /// give equal owners while [`Self::body_item_context`] is equal, up to
    /// the hyperlink relationships, which they do not check again.
    #[doc(hidden)]
    pub fn body_item_owners(&self, item: &BodyContent) -> Result<BodyItemOwners> {
        // The whole body admits each of its children alone.
        let mut alone = CT_Body {
            content: vec![item.clone()],
            sect_pr: None,
        };
        alone.validate_resource_limits()?;
        let part = self.document.standalone_body_item_xml(item)?;
        let (snapshots, cells, paragraphs, links) = self.owner_snapshots_of(
            vec![StorySource {
                root_kind: StoryKind::Body,
                part_name: self.doc_part_name.clone(),
                xml: Cow::Borrowed(&part),
            }],
            true,
        )?;
        let owners = scan_story_owners(&part, StoryKind::Body)?;
        let body = owners
            .iter()
            .find(|owner| owner.kind == StoryKind::Body)
            .ok_or_else(|| Error::Other("standalone body item has no body owner".into()))?;
        let items = scan_story_items(&part, body, None)?;
        let count = |kind| owners.iter().filter(|owner| owner.kind == kind).count();
        Ok(BodyItemOwners {
            snapshots,
            cells,
            paragraphs,
            links,
            cell_stories: count(StoryKind::TableCell),
            text_boxes: count(StoryKind::TextBox),
            paragraph_routes: body_paragraph_routes(&alone, &items),
            tables: alone.tables().count(),
            cell_paragraphs: cell_paragraph_routes(&mut alone, &part, &owners, &self.doc_part_name)
                .ok(),
            holds_field: holds_field_words(&part),
        })
    }

    /// What [`Self::body_item_owners`] reads besides the item: the main
    /// part's name and root, and the namespace facts of the opened package.
    #[doc(hidden)]
    pub fn body_item_context(&self) -> BodyItemContext {
        BodyItemContext {
            part_name: self.doc_part_name.clone(),
            root: [
                self.document.extra_namespaces.clone(),
                self.document.root_attributes.clone(),
                self.root_namespace_declarations.clone(),
                self.body_namespace_declarations.clone(),
                self.body_namespace_bindings.clone(),
            ],
            plain_local_edit_blocked: self.plain_local_edit_blocked,
        }
    }

    /// The direct body children, as [`Self::body_item_owners`] takes them.
    #[doc(hidden)]
    pub fn body_children(&self) -> &[BodyContent] {
        &self.document.body.content
    }

    /// The story parts after the main part, in the order of
    /// [`Self::story_owner_snapshots`]: the root story kind, the part name
    /// and the bytes of each, read without serializing the main part.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // The kind, name and bytes of each part.
    pub fn story_part_sources(&self) -> Result<Vec<(StoryKind, String, Cow<'_, [u8]>)>> {
        Ok(self
            .related_story_sources()?
            .into_iter()
            .map(|source| (source.root_kind, source.part_name, source.xml))
            .collect())
    }

    /// [`Self::story_owner_snapshots`] of one part that
    /// [`Self::story_part_sources`] lists, each snapshot's XML
    /// namespace-complete.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // The four owned families of one part.
    pub fn story_part_owner_snapshots(
        &self,
        kind: StoryKind,
        part_name: &str,
        xml: &[u8],
    ) -> Result<(
        Vec<StoryItemSnapshot>,
        Vec<(ContentLocation, usize, usize, StoryId)>,
        Vec<(ContentLocation, Vec<u8>)>,
        Vec<(ContentLocation, LinkInfo)>,
    )> {
        self.owner_snapshots_of(
            vec![StorySource {
                root_kind: kind,
                part_name: part_name.to_owned(),
                xml: Cow::Borrowed(xml),
            }],
            true,
        )
    }

    /// The relationship IDs of `part_name` that a hyperlink of a story in
    /// that part may name: the first relationship of each ID, when it is an
    /// external hyperlink, as the owner snapshots check a link's
    /// relationship.
    #[doc(hidden)]
    pub fn story_hyperlink_relationships(&self, part_name: &str) -> HashSet<&str> {
        let mut seen = HashSet::new();
        let mut valid = HashSet::new();
        for relationship in self
            .package
            .get_part_rels(part_name)
            .map_or(&[][..], |relationships| relationships.items.as_slice())
        {
            if seen.insert(relationship.id.as_str())
                && relationship.rel_type == rel_types::HYPERLINK
                && relationship.target_mode.as_deref() == Some("External")
            {
                valid.insert(relationship.id.as_str());
            }
        }
        valid
    }

    #[allow(clippy::type_complexity)] // One cross-crate projection carries four existing owned families.
    fn owner_snapshots_of(
        &self,
        sources: Vec<StorySource<'_>>,
        standalone: bool,
    ) -> Result<(
        Vec<StoryItemSnapshot>,
        Vec<(ContentLocation, usize, usize, StoryId)>,
        Vec<(ContentLocation, Vec<u8>)>,
        Vec<(ContentLocation, LinkInfo)>,
    )> {
        let mut snapshots = Vec::new();
        let mut cells = Vec::new();
        let mut paragraphs = Vec::new();
        let mut links = Vec::new();
        let mut seen = HashSet::new();
        let mut emitted_cells = HashSet::new();
        for source in sources {
            let physical_owners = scan_story_owners(&source.xml, source.root_kind)?;
            let owner_scopes = story_namespace_scopes_at(
                &source.xml,
                physical_owners.iter().map(|owner| owner.full.start),
            )?;
            let mut owners = Vec::with_capacity(physical_owners.len());
            let mut inventories = Vec::with_capacity(physical_owners.len());
            let mut admitted = Vec::with_capacity(physical_owners.len());
            for owner in &physical_owners {
                let scope = owner_scopes.get(&owner.full.start).ok_or_else(|| {
                    Error::Other("story owner namespace scope was not inventoried".into())
                })?;
                inventories.push(scan_story_items_with_scope(
                    &source.xml,
                    owner,
                    scope,
                    None,
                )?);
                let story = StoryId {
                    kind: owner.kind,
                    part_name: source.part_name.clone(),
                    owner_index: owner.owner_index,
                    fingerprint: owner.fingerprint,
                };
                admitted.push(seen.insert((
                    owner.kind,
                    source.part_name.clone(),
                    owner.owner_index,
                )));
                owners.push((story, owner.clone()));
            }
            let scopes = story_namespace_scopes_at(
                &source.xml,
                inventories
                    .iter()
                    .flatten()
                    .flat_map(|item| [item.scan.start, item.full.start]),
            )?;
            cells.extend(Self::story_table_cells_from_inventory(
                &source,
                &physical_owners,
                &inventories,
                &scopes,
                &mut emitted_cells,
            )?);
            let paragraph_spans = story_range_paragraph_spans_from_inventory(
                &source.xml,
                &owners,
                &inventories,
                &scopes,
            )?;
            let mut link_spans = Vec::with_capacity(owners.len());
            for ((story, _), items) in owners.iter().zip(&inventories) {
                link_spans.push(ordered_story_link_spans(
                    &source.xml,
                    story,
                    items.clone(),
                    &scopes,
                )?);
            }
            let leaf_scopes = story_namespace_scopes_at(
                &source.xml,
                paragraph_spans
                    .iter()
                    .flatten()
                    .map(|(_, span)| span.start)
                    .chain(link_spans.iter().flatten().map(|(_, link)| link.full.start)),
            )?;
            for (owner_index, ((story, _), items)) in owners.iter().zip(&inventories).enumerate() {
                if !admitted[owner_index] {
                    continue;
                }
                let mut direct_items = Vec::new();
                if story.kind == StoryKind::Body {
                    for item in items.iter().filter(|item| item.direct_owner_child) {
                        if item.kind != StoryItemKind::PreservedNode
                            || !content_fragment_root_is_section_properties(&source.xml, item)?
                        {
                            direct_items.push(item);
                        }
                    }
                }
                let mut direct_cursor = 0;
                for (index, item) in items.iter().enumerate() {
                    while direct_items
                        .get(direct_cursor)
                        .is_some_and(|direct| direct.full.end <= item.full.start)
                    {
                        direct_cursor += 1;
                    }
                    let direct_body_index = direct_items.get(direct_cursor).and_then(|direct| {
                        (direct.full.start <= item.full.start && item.full.end <= direct.full.end)
                            .then_some(direct_cursor)
                    });
                    let scope = scopes.get(&item.scan.start).ok_or_else(|| {
                        Error::Other("story item namespace scope was not inventoried".into())
                    })?;
                    let text = story_item_text_with_scope(&source.xml, item, scope, items)?;
                    let xml = if item.complex_field {
                        complex_story_field_xml(&source.xml, item)?
                    } else if standalone {
                        let scope = scopes.get(&item.full.start).ok_or_else(|| {
                            Error::Other("story item namespace scope was not inventoried".into())
                        })?;
                        close_content_fragment_namespaces(&source.xml[item.full.clone()], scope)?
                    } else {
                        source.xml[item.full.clone()].to_vec()
                    };
                    snapshots.push(StoryItemSnapshot {
                        location: ContentLocation::new(story.clone(), item.kind, vec![index]),
                        direct_body_index,
                        direct_child: item.direct_owner_child,
                        text,
                        xml,
                    });
                }
            }
            for (index, mut spans) in paragraph_spans.into_iter().enumerate() {
                if !admitted[index] {
                    continue;
                }
                spans.sort_by_key(|(_, span)| span.start);
                for (location, span) in spans {
                    let scope = leaf_scopes.get(&span.start).ok_or_else(|| {
                        Error::Other("story paragraph namespace scope was not inventoried".into())
                    })?;
                    paragraphs.push((
                        location,
                        close_content_fragment_namespaces(&source.xml[span], scope)?,
                    ));
                }
            }
            for (index, spans) in link_spans.into_iter().enumerate() {
                if !admitted[index] {
                    continue;
                }
                for (location, span) in spans {
                    let scope = leaf_scopes.get(&span.full.start).ok_or_else(|| {
                        Error::Other("story hyperlink namespace scope was not inventoried".into())
                    })?;
                    let info = self.story_link_info(location.story(), &source.xml, span, scope)?;
                    links.push((location, info));
                }
            }
        }
        Ok((snapshots, cells, paragraphs, links))
    }

    #[allow(clippy::type_complexity)]
    fn story_table_cells_from_inventory(
        source: &StorySource<'_>,
        owners: &[StoryOwnerSpan],
        inventories: &[Vec<StoryItemSpan>],
        scopes: &HashMap<usize, BTreeMap<String, String>>,
        emitted: &mut HashSet<(ContentLocation, usize, usize)>,
    ) -> Result<Vec<(ContentLocation, usize, usize, StoryId)>> {
        let mut result = Vec::new();
        let mut tables = Vec::new();
        for (owner, items) in owners.iter().zip(inventories) {
            let story = StoryId {
                kind: owner.kind,
                part_name: source.part_name.clone(),
                owner_index: owner.owner_index,
                fingerprint: owner.fingerprint,
            };
            for (index, item) in items.iter().enumerate() {
                if item.kind == StoryItemKind::Table {
                    tables.push((
                        item.full.clone(),
                        ContentLocation::new(story.clone(), item.kind, vec![index]),
                    ));
                }
            }
        }
        // Recover physical order with one lexical pass instead of sorting
        // a growing table inventory. Qualification came from the owner scan.
        let mut by_start = tables
            .into_iter()
            .map(|item| (item.0.start, item))
            .collect::<HashMap<_, _>>();
        let mut tables = Vec::with_capacity(by_start.len());
        let mut reader = quick_xml::Reader::from_reader(source.xml.as_ref());
        let mut buffer = Vec::new();
        loop {
            let before = reader.buffer_position() as usize;
            match reader
                .read_event_into(&mut buffer)
                .map_err(|error| Error::Other(format!("table source order scan failed: {error}")))?
            {
                Event::Start(_) | Event::Empty(_) => {
                    if let Some(item) = by_start.remove(&before) {
                        tables.push(item);
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buffer.clear();
        }
        if !by_start.is_empty() {
            return Err(Error::Other(
                "table physical source order is incomplete".into(),
            ));
        }
        let cells = owners
            .iter()
            .filter(|owner| owner.kind == StoryKind::TableCell)
            .collect::<Vec<_>>();
        let mut next = 0;
        let mut cell_cursor = 0;
        while next < tables.len() {
            let root = next;
            let span = tables[root].0.clone();
            next += 1;
            while next < tables.len() && tables[next].0.end < span.end {
                next += 1;
            }
            let scope = scopes
                .get(&span.start)
                .ok_or_else(|| Error::Other("table item scope missing".into()))?;
            let closed = close_content_fragment_namespaces(&source.xml[span.clone()], scope)?;
            let mut wrapped =
                format!(r#"<w:document xmlns:w="{WORD_NAMESPACE}"><w:body>"#).into_bytes();
            wrapped.extend(closed);
            wrapped.extend_from_slice(b"</w:body></w:document>");
            let parsed = CT_Document::from_xml(&wrapped)?;
            let [BodyContent::Table(table)] = parsed.body.content.as_slice() else {
                return Err(Error::Other(
                    "rich table has ambiguous physical content".into(),
                ));
            };
            let (count, modeled) = physical_table_cells(table);
            while cells
                .get(cell_cursor)
                .is_some_and(|owner| owner.full.start < span.start)
            {
                cell_cursor += 1;
            }
            let first = cell_cursor;
            while cells
                .get(cell_cursor)
                .is_some_and(|owner| owner.full.start < span.end)
            {
                cell_cursor += 1;
            }
            let physical = &cells[first..cell_cursor];
            if modeled.len() != physical.len() {
                return Err(Error::Other(
                    "rich table cell source correspondence is incomplete".into(),
                ));
            }
            let mut table_routes = vec![None; count];
            let mut containing = Vec::<usize>::new();
            let mut cursor = root;
            for ((table, coordinate), owner) in modeled.into_iter().zip(physical) {
                while cursor < next && tables[cursor].0.start < owner.full.start {
                    while containing
                        .last()
                        .is_some_and(|&index| tables[index].0.end < tables[cursor].0.end)
                    {
                        containing.pop();
                    }
                    containing.push(cursor);
                    cursor += 1;
                }
                while containing
                    .last()
                    .is_some_and(|&index| tables[index].0.end <= owner.full.start)
                {
                    containing.pop();
                }
                let index = *containing
                    .last()
                    .ok_or_else(|| Error::Other("cell has no physical table".into()))?;
                if owner.full.end >= tables[index].0.end
                    || table_routes[table].is_some_and(|before| before != index)
                {
                    return Err(Error::Other(
                        "rich table cell has ambiguous physical owner".into(),
                    ));
                }
                table_routes[table] = Some(index);
                if let Some((row, cell)) = coordinate
                    && emitted.insert((tables[index].1.clone(), row, cell))
                {
                    result.push((
                        tables[index].1.clone(),
                        row,
                        cell,
                        StoryId {
                            kind: owner.kind,
                            part_name: source.part_name.clone(),
                            owner_index: owner.owner_index,
                            fingerprint: owner.fingerprint,
                        },
                    ));
                }
            }
        }
        Ok(result)
    }

    /// Resolve one related table cell through its actual enclosing physical table.
    #[doc(hidden)]
    pub fn story_table_cell(
        &self,
        location: &ContentLocation,
        row: usize,
        cell: usize,
    ) -> Result<StoryId> {
        if location.item_kind != StoryItemKind::Table || location.is_end {
            return Err(Error::Other(
                "rich table needs an existing table location".to_owned(),
            ));
        }
        let (source, item) = self.story_item_source(location)?;
        let owner = physical_table_cell_owner(&source.xml, source.root_kind, &item, row, cell)?;
        let story = StoryId {
            kind: owner.kind,
            part_name: source.part_name.clone(),
            owner_index: owner.owner_index,
            fingerprint: owner.fingerprint,
        };
        Ok(story)
    }

    /// Resolve one paragraph through its actual checked related table cell owner.
    #[doc(hidden)]
    pub fn story_table_cell_paragraph_location(
        &self,
        location: &ContentLocation,
        row: usize,
        cell: usize,
        paragraph: usize,
    ) -> Result<ContentLocation> {
        let story = self.story_table_cell(location, row, cell)?;
        let (source, owner) = self.story_source_and_owner(&story)?;
        let spans = story_range_paragraph_spans(&source.xml, &[(story.clone(), owner.clone())])?
            .into_iter()
            .flatten();
        let (location, _) = spans.into_iter().nth(paragraph).ok_or_else(|| {
            Error::Other("rich table cell paragraph index is out of range".to_owned())
        })?;
        self.story_range_paragraph_snapshot(&location)?;
        Ok(location)
    }

    /// Read owned facts from one source-qualified rich paragraph.
    #[doc(hidden)]
    pub fn read_story_paragraph<T>(
        &self,
        location: &ContentLocation,
        read: impl FnOnce(ParagraphRef<'_>) -> T,
    ) -> Result<T> {
        let snapshot = self.story_range_paragraph_snapshot(location)?;
        let paragraph = CT_P::from_xml_fragment(snapshot.xml())?;
        Ok(read(ParagraphRef { inner: &paragraph }))
    }

    /// Edit a direct cell paragraph without a whole-document transaction when local admission is sufficient.
    /// Controlled paragraphs and candidates needing package reconciliation keep the checked story route.
    #[doc(hidden)]
    pub fn edit_cell_paragraph<T>(
        &mut self,
        table: usize,
        row: usize,
        cell: usize,
        paragraph: usize,
        edit: impl FnOnce(&mut Paragraph<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut table_index = table;
        let selected = nth_table_in_body_ref(&self.document.body.content, &mut table_index)
            .and_then(|table| table.rows.get(row))
            .and_then(|row| row.cells.get(cell))
            .ok_or_else(|| Error::Other("cell index is out of range".to_owned()))?;
        let readers = selected.paragraphs();
        let direct_count = selected
            .content
            .iter()
            .filter(|child| matches!(child, CellContent::Paragraph(_)))
            .count();
        if readers.len() != direct_count || paragraph >= direct_count {
            let location = self.cell_paragraph_story_location(table, row, cell, paragraph)?;
            return self.edit_story_paragraph(&location, edit);
        }
        let original = readers[paragraph].clone();
        let mut candidate = original.clone();
        let result = edit(&mut Paragraph {
            inner: &mut candidate,
        })?;
        let mut fragment = ContentFragment::paragraph(candidate.clone())?;
        reject_section_owning_content_fragment(&fragment)?;
        let qualified = (|| -> Option<bool> {
            if self.plain_local_edit_blocked
                || oxoffice_core::xml::validate_strict_xml_1_0(&fragment.xml).is_err()
            {
                return Some(false);
            }
            let inherited = fragment
                .namespace_scope
                .iter()
                .map(|(prefix, namespace)| {
                    (
                        if prefix.is_empty() {
                            "xmlns".to_owned()
                        } else {
                            format!("xmlns:{prefix}")
                        },
                        namespace.clone(),
                    )
                })
                .collect::<Vec<_>>();
            if !xml_relationship_ids_in_order_with_bindings(&fragment.xml, &inherited)
                .ok()?
                .is_empty()
            {
                return Some(false);
            }
            let before = ContentFragment::paragraph(original.clone()).ok()?;
            let count = |xml: &[u8]| -> Option<BTreeMap<(i32, usize), usize>> {
                let mut counts = BTreeMap::new();
                for marker in crate::comments::comment_source_markers(xml).ok()? {
                    let entry = counts.entry((marker.id, marker.family)).or_insert(0usize);
                    *entry = entry.checked_add(1)?;
                }
                Some(counts)
            };
            let before = count(&before.xml)?;
            let after = count(&fragment.xml)?;
            Some(
                before
                    .iter()
                    .all(|(marker, before)| after.get(marker).copied().unwrap_or(0) >= *before),
            )
        })()
        .unwrap_or(false);
        if !qualified {
            let location = self.cell_paragraph_story_location(table, row, cell, paragraph)?;
            fragment.source_part_name = Some(location.story.part_name.clone());
            fragment.source_story_kind = Some(location.story.kind);
            fragment.source_owner_index = Some(location.story.owner_index);
            self.replace_story_content(&location, fragment)?;
            return Ok(result);
        }
        let mut table_index = table;
        let selected = nth_table_in_body(&mut self.document.body.content, &mut table_index)
            .and_then(|table| table.rows.get_mut(row))
            .and_then(|row| row.cells.get_mut(cell))
            .ok_or_else(|| Error::Other("cell index is out of range".to_owned()))?;
        if selected
            .content
            .iter()
            .filter(|child| matches!(child, CellContent::Paragraph(_)))
            .count()
            != direct_count
        {
            return Err(Error::Other(
                "cell paragraph changed during local edit".to_owned(),
            ));
        }
        let selected = selected
            .paragraphs_mut()
            .into_iter()
            .nth(paragraph)
            .ok_or_else(|| Error::Other("cell paragraph index is out of range".to_owned()))?;
        if *selected != original {
            return Err(Error::Other(
                "cell paragraph changed during local edit".to_owned(),
            ));
        }
        *selected = candidate;
        self.invalidate_layout();
        Ok(result)
    }

    /// Edit one source-qualified rich paragraph and publish it atomically.
    #[doc(hidden)]
    pub fn edit_story_paragraph<T>(
        &mut self,
        location: &ContentLocation,
        edit: impl FnOnce(&mut Paragraph<'_>) -> Result<T>,
    ) -> Result<T> {
        let snapshot = self.story_range_paragraph_snapshot(location)?;
        let mut paragraph = CT_P::from_xml_fragment(snapshot.xml())?;
        let result = edit(&mut Paragraph {
            inner: &mut paragraph,
        })?;
        let mut fragment = ContentFragment::paragraph(paragraph)?;
        // This fragment is rebuilt from the checked paragraph of this same owner.
        // Keep relationship admission scoped to that owner, including newly
        // allocated relationships already validated in its package scope.
        fragment.source_part_name = Some(location.story.part_name.clone());
        fragment.source_story_kind = Some(location.story.kind);
        fragment.source_owner_index = Some(location.story.owner_index);
        self.replace_story_content(location, fragment)?;
        Ok(result)
    }

    pub fn set_story_text(&mut self, location: &ContentLocation, value: &str) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, owner) = candidate.story_source_and_owner(&location.story)?;
        if location.index_path.len() != 1 {
            return Err(StoryError::InvalidPath {
                path: location.index_path.clone(),
            }
            .into());
        }
        let items = scan_story_items(&source.xml, &owner, None)?;
        let index = location.index_path[0];
        let item = items.get(index).ok_or(StoryError::OutOfBounds {
            index,
            len: items.len(),
        })?;
        if item.kind != location.item_kind {
            return Err(StoryError::KindMismatch {
                expected: location.item_kind,
                actual: item.kind,
            }
            .into());
        }
        if item.kind == StoryItemKind::PreservedNode {
            return Err(StoryError::NotTextBearing { kind: item.kind }.into());
        }
        let original_len = source.xml.len();
        let item_start = item.full.start;
        let item_end = item.full.end;
        let updated = replace_story_item_text(&source.xml, item, value)?;
        let new_item_end = if updated.len() >= original_len {
            item_end + (updated.len() - original_len)
        } else {
            item_end
                .checked_sub(original_len - updated.len())
                .ok_or_else(|| {
                    Error::Other("story text replacement underflowed its item range".to_owned())
                })?
        };
        let updated = remove_newly_empty_hyperlinks(
            &source.xml,
            item_start..item_end,
            &updated,
            item_start..new_item_end,
        )?;
        if source.part_name == candidate.doc_part_name {
            candidate.document =
                crate::document::DocumentModel::unproved(CT_Document::from_xml(&updated)?);
            candidate.flush_to_package()?;
        } else {
            candidate.package.set_part(&source.part_name, updated);
        }
        let reopened = candidate.reopen_prepared_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Point one story hyperlink at an external URL.
    ///
    /// `link_index` indexes [`Self::story_links`] for `story`. An anchor
    /// hyperlink becomes external and loses its anchor. The link's
    /// relationship is retargeted in place when nothing else references it.
    /// Otherwise the link gets a new relationship, so another link that shared
    /// the old one keeps its target. A relationship left unreferenced is removed.
    ///
    /// Only links that [`Self::story_links`] lists can be addressed. An empty
    /// `w:hyperlink`, a link inside `w:fldSimple`, a link in a text box inside
    /// `mc:AlternateContent`, and a HYPERLINK field are out of reach.
    pub fn set_hyperlink_url(
        &mut self,
        story: &StoryId,
        link_index: usize,
        url: &str,
    ) -> Result<()> {
        self.edit_story_hyperlink(story, link_index, Some(url))
    }

    /// Remove one story hyperlink and keep its content in place.
    ///
    /// `link_index` indexes [`Self::story_links`] for `story`. The runs keep
    /// their formatting, except that the built-in Hyperlink and
    /// FollowedHyperlink character styles are cleared, as Word's Remove
    /// Hyperlink does. A relationship left unreferenced is removed. The links
    /// out of reach are those [`Self::set_hyperlink_url`] names.
    pub fn remove_hyperlink(&mut self, story: &StoryId, link_index: usize) -> Result<()> {
        self.edit_story_hyperlink(story, link_index, None)
    }

    fn edit_story_hyperlink(
        &mut self,
        story: &StoryId,
        link_index: usize,
        url: Option<&str>,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, owner) = candidate.story_source_and_owner(story)?;
        let part_name = source.part_name.clone();
        let xml = source.xml.into_owned();
        let mut links = story_link_spans(&xml, &owner, story)?;
        let len = links.len();
        if link_index >= len {
            return Err(StoryError::OutOfBounds {
                index: link_index,
                len,
            }
            .into());
        }
        let (_, link) = links.swap_remove(link_index);
        let scope = story_namespace_scope_at(&xml, link.full.start)?;
        let updated = match url {
            Some(url) => {
                let (start, _) = story_hyperlink_tags(&xml, &link.full)?;
                let with_relationship = |relationship_id: &str| -> Result<Vec<u8>> {
                    let tag = external_story_hyperlink_start(
                        &xml[start.clone()],
                        &scope,
                        relationship_id,
                    )?;
                    let mut updated = xml.clone();
                    updated.splice(start.clone(), tag);
                    Ok(updated)
                };
                let shared = match link.rel_id.as_deref() {
                    Some(old) => xml_relationship_ids_in_order(&with_relationship("")?)?
                        .iter()
                        .any(|id| id == old),
                    None => false,
                };
                let in_place = link.rel_id.as_deref().filter(|_| !shared).and_then(|old| {
                    candidate
                        .package
                        .get_part_rels_mut(&part_name)?
                        .items
                        .iter_mut()
                        .find(|relationship| {
                            relationship.id == old
                                && relationship.rel_type == rel_types::HYPERLINK
                                && relationship.target_mode.as_deref() == Some("External")
                        })
                });
                match in_place {
                    Some(relationship) => {
                        relationship.target = url.to_owned();
                        let relationship_id = relationship.id.clone();
                        if link.anchor.is_some() {
                            with_relationship(&relationship_id)?
                        } else {
                            xml.clone()
                        }
                    }
                    None => {
                        let relationship_id = candidate.add_external_relationship_checked(
                            &part_name,
                            rel_types::HYPERLINK,
                            url,
                        )?;
                        if part_name == candidate.doc_part_name {
                            candidate
                                .identifiers
                                .register_nested_story_relationship(story, relationship_id.clone());
                        }
                        with_relationship(&relationship_id)?
                    }
                }
            }
            None => {
                let styles = candidate.hyperlink_character_style_ids();
                unwrap_story_hyperlink(&xml, &link.full, &scope, &styles)?
            }
        };
        if let Some(old) = link.rel_id.as_deref()
            && !xml_relationship_ids_in_order(&updated)?
                .iter()
                .any(|id| id == old)
            && let Some(relationships) = candidate.package.get_part_rels_mut(&part_name)
        {
            let before = relationships.items.len();
            relationships.items.retain(|relationship| {
                relationship.id != old || relationship.rel_type != rel_types::HYPERLINK
            });
            if relationships.items.len() != before {
                if relationships.items.is_empty() {
                    candidate.package.remove_part_rels(&part_name);
                }
                candidate
                    .identifiers
                    .retire_authored_story_relationships(&part_name, [old.to_owned()]);
            }
        }
        set_story_source_xml(&mut candidate, &part_name, updated)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// The character style IDs that mark hyperlink text: the built-in
    /// Hyperlink and FollowedHyperlink styles, by ID and by name.
    fn hyperlink_character_style_ids(&self) -> HashSet<String> {
        const NAMES: [&str; 2] = ["Hyperlink", "FollowedHyperlink"];
        let mut ids = NAMES.map(str::to_owned).into_iter().collect::<HashSet<_>>();
        ids.extend(
            self.styles
                .styles
                .iter()
                .filter(|style| {
                    style.style_type == StyleType::Character
                        && style.name.as_deref().is_some_and(|name| {
                            NAMES.iter().any(|known| known.eq_ignore_ascii_case(name))
                        })
                })
                .map(|style| style.style_id.clone()),
        );
        ids
    }

    /// Insert one owned fragment at a checked direct-child boundary.
    pub fn insert_content(
        &mut self,
        destination: &ContentLocation,
        fragment: ContentFragment,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, owner) = candidate.story_source_and_owner(&destination.story)?;
        let part_name = source.part_name.clone();
        let source_xml = source.xml.into_owned();
        let (boundary, _, _) = validated_content_boundary(&source_xml, &owner, destination)?;
        reject_section_owning_content_fragment(&fragment)?;
        let fragment_xml =
            content_fragment_for_insertion(&candidate, &destination.story, &fragment)?;
        let mut updated = source_xml;
        insert_story_fragment(&mut updated, &owner, boundary, fragment_xml)?;
        set_story_source_xml(&mut candidate, &part_name, updated)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Insert one bounded HTML fragment at a checked direct-child boundary.
    ///
    /// Images are resolved only from data URIs or the explicit resource slice.
    /// The document remains unchanged if parsing, projection, package
    /// reconciliation, insertion, or reopen validation fails.
    pub fn insert_html_fragment(
        &mut self,
        destination: &ContentLocation,
        html: &str,
        images: &[crate::HtmlImageResource<'_>],
    ) -> Result<crate::HtmlFragmentInsertResult> {
        if !matches!(
            destination.story.kind,
            StoryKind::Body | StoryKind::TableCell | StoryKind::Header | StoryKind::Footer
        ) {
            return Err(Error::Other(
                "HTML fragments support body, table-cell, header, and footer stories".to_owned(),
            ));
        }
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, owner) = candidate.story_source_and_owner(&destination.story)?;
        let part_name = source.part_name.clone();
        let mut source_xml = source.xml.into_owned();
        let (boundary, direct_index, _) =
            validated_content_boundary(&source_xml, &owner, destination)?;
        let modeled_cell_route = (destination.story.kind == StoryKind::TableCell
            && part_name == candidate.doc_part_name)
            .then(|| modeled_main_cell_route(&source_xml, &owner))
            .transpose()?;
        let projected =
            crate::html::project_html_fragment(&mut candidate, &destination.story, html, images)?;
        let inserted_count = projected.content.len();
        if destination.story.kind == StoryKind::Body && part_name == candidate.doc_part_name {
            candidate
                .document
                .body
                .content
                .splice(direct_index..direct_index, projected.content);
        } else if destination.story.kind == StoryKind::TableCell
            && part_name == candidate.doc_part_name
        {
            let (content_index, mut cell_index) = modeled_cell_route.ok_or_else(|| {
                Error::Other("modeled table-cell route disappeared before insertion".to_owned())
            })?;
            let content = candidate
                .document
                .body
                .content
                .get_mut(content_index)
                .ok_or_else(|| StoryError::OwnerNotFound {
                    story: destination.story.clone(),
                })?;
            let cell = match content {
                BodyContent::Table(table) => nth_cell_in_table(table, &mut cell_index),
                BodyContent::ContentControl(control) => {
                    nth_cell_in_control(control, &mut cell_index)
                }
                BodyContent::Paragraph(_) | BodyContent::RawXml(_) => None,
            }
            .ok_or_else(|| StoryError::OwnerNotFound {
                story: destination.story.clone(),
            })?;
            insert_html_content_into_cell(cell, direct_index, projected.content)?;
        } else {
            let mut fragment_xml = Vec::new();
            for content in projected.content {
                fragment_xml.extend_from_slice(&serialize_content_fragment(content)?);
            }
            insert_story_fragment(&mut source_xml, &owner, boundary, fragment_xml)?;
            set_story_source_xml(&mut candidate, &part_name, source_xml)?;
        }
        let reopened = candidate.prepare_and_reopen_staged()?;
        let result_story = reopened
            .stories()?
            .into_iter()
            .find(|story| {
                story.kind == destination.story.kind
                    && story.part_name == destination.story.part_name
                    && story.owner_index == destination.story.owner_index
            })
            .ok_or_else(|| StoryError::OwnerNotFound {
                story: destination.story.clone(),
            })?;
        let range_end = direct_index
            .checked_add(inserted_count)
            .ok_or_else(|| Error::Other("HTML fragment range overflowed".to_owned()))?;
        self.commit_staged_mutation(reopened);
        Ok(crate::HtmlFragmentInsertResult {
            story: result_story,
            direct_range: direct_index..range_end,
            diagnostics: projected.diagnostics,
        })
    }

    /// Import an owned cross-document fragment at a checked story boundary.
    ///
    /// The complete supported dependency closure is allocated on a staged
    /// candidate. Unreconcilable dependencies return an error before the
    /// destination changes.
    pub fn import_fragment(
        &mut self,
        destination: &ContentLocation,
        fragment: &DocumentFragment,
        policy: FragmentConflictPolicy,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, owner, _) =
            candidate.fragment_story_source_owner_and_owners(&destination.story)?;
        let part_name = source.part_name.clone();
        let mut source_xml = source.xml.into_owned();
        let (mut owner, mut boundary, _, _) =
            fragment_content_boundary(&source_xml, &owner, destination)?;
        if fragment.include_final_section_properties
            && (destination.story.kind != StoryKind::Body
                || (!destination.is_end && destination.index_path.len() != 1))
        {
            return Err(Error::Other(
                "section-inclusive fragments require the main body destination".to_owned(),
            ));
        }
        let imported_xml = fragment.import_content_staged(
            &mut candidate,
            &part_name,
            policy,
            false,
            &mut Vec::new(),
        )?;
        // Companion closure may append notes or comment threads to the same
        // physical part that owns the destination. Refresh that part before
        // inserting, so publishing the selected owner cannot discard them.
        candidate.prepare_staged_package()?;
        let companion_owner = candidate.comments_part_name.as_deref() == Some(&part_name)
            || candidate.note_part_name(StoryKind::Footnote)?.as_deref() == Some(&part_name)
            || candidate.note_part_name(StoryKind::Endnote)?.as_deref() == Some(&part_name);
        if companion_owner {
            let (updated_source, updated_owner, _) =
                candidate.fragment_story_source_owner_and_owners(&destination.story)?;
            source_xml = updated_source.xml.into_owned();
            let (updated_owner, updated_boundary, _, _) =
                fragment_content_boundary(&source_xml, &updated_owner, destination)?;
            owner = updated_owner;
            boundary = updated_boundary;
        }
        insert_story_fragment(&mut source_xml, &owner, boundary, imported_xml)?;
        set_story_source_xml(&mut candidate, &part_name, source_xml)?;
        let reopened = candidate.reopen_prepared_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Remove one checked direct child and return it as an owned fragment.
    pub fn remove_content_at(&mut self, location: &ContentLocation) -> Result<ContentFragment> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, owner) = candidate.story_source_and_owner(&location.story)?;
        let part_name = source.part_name.clone();
        let source_xml = source.xml.into_owned();
        let item = validated_direct_content_item(&source_xml, &owner, location)?;
        let namespace_scope = story_namespace_scope_at(&source_xml, item.full.start)?;
        let closed =
            close_content_fragment_namespaces(&source_xml[item.full.clone()], &namespace_scope)?;
        Self::refuse_commented_fragment(&closed)?;
        if owner.kind == StoryKind::TableCell
            && item.kind == StoryItemKind::Paragraph
            && direct_story_content_items(&source_xml, &owner)?
                .iter()
                .filter(|candidate| candidate.kind == StoryItemKind::Paragraph)
                .count()
                == 1
        {
            return Err(Error::Other(
                "cannot remove the final required table-cell paragraph".to_owned(),
            ));
        }
        let fragment = ContentFragment {
            kind: item.kind,
            xml: source_xml[item.full.clone()].to_vec(),
            source_part_name: Some(part_name.clone()),
            source_story_kind: Some(location.story.kind),
            source_owner_index: Some(location.story.owner_index),
            namespace_scope,
        };
        reject_section_owning_content_fragment(&fragment)?;
        validate_fragment_relationships(&candidate, &location.story, &fragment)?;
        let mut updated = source_xml;
        updated.drain(item.full);
        set_story_source_xml(&mut candidate, &part_name, updated)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(fragment)
    }

    /// Clone one checked direct child into a checked insertion boundary.
    ///
    /// Document-wide identities of the copy are made unique, and its
    /// paragraphs and table rows drop `w14:paraId` and `w14:textId`.
    pub fn clone_content(
        &mut self,
        source_location: &ContentLocation,
        destination: &ContentLocation,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, source_owner) = candidate.story_source_and_owner(&source_location.story)?;
        let source_xml = source.xml.into_owned();
        let source_item =
            validated_direct_content_item(&source_xml, &source_owner, source_location)?;
        let mut fragment = ContentFragment {
            kind: source_item.kind,
            xml: source_xml[source_item.full.clone()].to_vec(),
            source_part_name: Some(source_location.story.part_name.clone()),
            source_story_kind: Some(source_location.story.kind),
            source_owner_index: Some(source_location.story.owner_index),
            namespace_scope: story_namespace_scope_at(&source_xml, source_item.full.start)?,
        };
        reject_section_owning_content_fragment(&fragment)?;

        let (destination_source, destination_owner) =
            candidate.story_source_and_owner(&destination.story)?;
        let destination_part_name = destination_source.part_name.clone();
        let destination_xml = destination_source.xml.into_owned();
        let (boundary, _, _) =
            validated_content_boundary(&destination_xml, &destination_owner, destination)?;
        fragment.xml = freshen_content_fragment_identities(&mut candidate, &fragment)?;
        let fragment_xml =
            content_fragment_for_insertion(&candidate, &destination.story, &fragment)?;
        let mut updated = destination_xml;
        insert_story_fragment(&mut updated, &destination_owner, boundary, fragment_xml)?;
        set_story_source_xml(&mut candidate, &destination_part_name, updated)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// Move one checked direct child within its current story owner.
    pub fn move_content(
        &mut self,
        source_location: &ContentLocation,
        destination: &ContentLocation,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (source, owner) = candidate.story_source_and_owner(&source_location.story)?;
        let part_name = source.part_name.clone();
        let source_xml = source.xml.into_owned();
        let source_item = validated_direct_content_item(&source_xml, &owner, source_location)?;

        let (destination_source, destination_owner) =
            candidate.story_source_and_owner(&destination.story)?;
        let destination_xml = destination_source.xml.into_owned();
        let (boundary, destination_index, _) =
            validated_content_boundary(&destination_xml, &destination_owner, destination)?;
        if source_location.story != destination.story {
            return Err(Error::Other(
                "content can move only within one unchanged story owner".to_owned(),
            ));
        }
        let direct_items = direct_story_content_items(&source_xml, &owner)?;
        let source_index = direct_items
            .iter()
            .position(|item| item.full == source_item.full)
            .ok_or_else(|| Error::Other("direct content source was not found".to_owned()))?;
        let fragment = ContentFragment {
            kind: source_item.kind,
            xml: source_xml[source_item.full.clone()].to_vec(),
            source_part_name: Some(part_name.clone()),
            source_story_kind: Some(source_location.story.kind),
            source_owner_index: Some(source_location.story.owner_index),
            namespace_scope: story_namespace_scope_at(&source_xml, source_item.full.start)?,
        };
        reject_section_owning_content_fragment(&fragment)?;
        validate_fragment_relationships(&candidate, &destination.story, &fragment)?;
        if destination_index == source_index || destination_index == source_index + 1 {
            return Ok(());
        }
        let removed_len = source_item.full.len();
        let mut updated = source_xml;
        updated.drain(source_item.full);
        let adjusted_boundary = if source_index < destination_index {
            boundary.checked_sub(removed_len).ok_or_else(|| {
                Error::Other("content move destination underflowed after removal".to_owned())
            })?
        } else {
            boundary
        };
        updated.splice(adjusted_boundary..adjusted_boundary, fragment.xml);
        set_story_source_xml(&mut candidate, &part_name, updated)?;
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    /// The package a save would write now, staged on a copy.
    fn staged_output_package(&self) -> Result<OpcPackage> {
        self.validate_model_resource_limits()?;
        let mut candidate = self.clone_for_staging();
        candidate.prepare_staged_output()?;
        crate::embedded::persist_invalidated_package_signature(
            &mut candidate.package,
            candidate.package_signatures_invalidated,
        )?;
        Ok(candidate.package)
    }

    /// Package part names [`Self::to_bytes`] would write now, each with a
    /// leading slash, in lexicographic order. They are its ZIP entries, so
    /// they include `/[Content_Types].xml`, `/_rels/.rels`, every
    /// relationship part such as `/word/_rels/document.xml.rels`, and parts
    /// with no relationship. When the package would not save, this returns
    /// the save error, such as a graph validation failure, a duplicate part
    /// identity or a character XML cannot carry.
    pub fn part_names(&self) -> Result<Vec<String>> {
        let package = self.staged_output_package()?;
        let mut names = package
            .zip_entries()?
            .into_iter()
            .map(|(entry, _)| format!("/{entry}"))
            .collect::<Vec<_>>();
        names.sort();
        Ok(names)
    }

    /// UTF-8 XML of one package part as [`Self::to_bytes`] would write it now.
    ///
    /// The part is the ZIP entry of [`Self::to_bytes`] with that name, a
    /// relationship part and `/[Content_Types].xml` included. The name may
    /// omit its leading slash and matches without regard to case. A missing,
    /// binary, or non-UTF-8 part returns an error that names the part and its
    /// content type. A UTF-8 BOM is stripped. UTF-16 is not decoded. When the
    /// package would not save, this returns the save error, as
    /// [`Self::part_names`] does.
    pub fn part_xml(&self, part_name: &str) -> Result<String> {
        let requested = part_name.trim();
        let requested = requested.strip_prefix('/').unwrap_or(requested);
        let package = self.staged_output_package()?;
        let entries = package.zip_entries()?;
        let Some((entry, data)) = entries
            .iter()
            .find(|(entry, _)| entry.eq_ignore_ascii_case(requested))
        else {
            let requested = format!("/{requested}");
            let content_type = package
                .content_types
                .content_type_for(&requested)
                .unwrap_or("none");
            return Err(Error::Other(format!(
                "part {requested} is missing (content type {content_type})"
            )));
        };
        let stored = format!("/{entry}");
        let content_type = package
            .content_types
            .content_type_for(&stored)
            .unwrap_or("none");
        // A relationship part and [Content_Types].xml are XML whatever the
        // content types declare. Every other entry is a stored part.
        let xml_type =
            !package.contains_part(&stored) || content_type.to_ascii_lowercase().contains("xml");
        if !xml_type {
            return Err(Error::Other(format!(
                "part {stored} has content type {content_type} and is not UTF-8 XML"
            )));
        }
        let body = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(data.as_ref());
        // UTF-16 text without a BOM is ASCII bytes interleaved with NUL.
        // Those bytes are valid UTF-8, and XML 1.0 forbids NUL, so a NUL is
        // the non-UTF-8 refusal for an XML content type.
        let Ok(xml) = String::from_utf8(body.to_vec()) else {
            return Err(Error::Other(format!(
                "part {stored} has content type {content_type} and is not UTF-8 XML"
            )));
        };
        if xml.contains('\0') {
            return Err(Error::Other(format!(
                "part {stored} has content type {content_type} and is not UTF-8 XML"
            )));
        }
        Ok(xml)
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

/// Express `target_part` relative to the directory holding `source_part`.
///
/// Falls back to the absolute part name when the two live in different
/// directories, which OPC also permits.
fn relative_target(source_part: &str, target_part: &str) -> String {
    let dir = match source_part.rfind('/') {
        Some(pos) => &source_part[..=pos],
        None => "/",
    };
    match target_part.strip_prefix(dir) {
        Some(rest) if !rest.contains('/') => rest.to_string(),
        _ => target_part.to_string(),
    }
}

fn relative_descendant_target(source_part: &str, target_part: &str) -> String {
    let directory = source_part
        .rfind('/')
        .map_or("/", |position| &source_part[..=position]);
    target_part
        .strip_prefix(directory)
        .unwrap_or(target_part)
        .to_owned()
}

fn section_has_layout(properties: &CT_SectPr) -> bool {
    properties.page_width.is_some()
        || properties.page_height.is_some()
        || properties.orientation.is_some()
        || properties.margin_top.is_some()
        || properties.margin_right.is_some()
        || properties.margin_bottom.is_some()
        || properties.margin_left.is_some()
        || properties.gutter.is_some()
        || properties.header_distance.is_some()
        || properties.footer_distance.is_some()
        || properties.section_type.is_some()
        || properties.columns.is_some()
        || properties.page_borders.is_some()
        || properties.line_numbers.is_some()
        || properties.vertical_alignment.is_some()
        || properties.title_pg.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    const W15: &str = "http://schemas.microsoft.com/office/word/2012/wordml";

    #[test]
    fn a_table_edit_that_is_refused_declined_or_panics_keeps_the_cached_answers() {
        let mut doc = read_with_body(
            "<w:tbl><w:tblGrid><w:gridCol w:w=\"2000\"/></w:tblGrid><w:tr><w:tc><w:p/></w:tc></w:tr>\
             </w:tbl><w:p/>",
        );
        assert_eq!(doc.unsavable_body_edit(), None);
        let refused = doc.edit_table_properties_in_place(0, |table| {
            table.set_alignment(crate::paragraph::Alignment::Right);
            Err::<(), _>(Error::Other("refused".to_owned()))
        });
        assert!(refused.is_err());
        let declined = doc.edit_table_properties_in_place(0, |table| table.add_row().map(|_| ()));
        assert!(matches!(declined, Ok(None)));
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            doc.edit_table_properties_in_place(0, |_| -> Result<()> { panic!("half way") })
        }));
        assert!(panicked.is_err());
        assert!(
            doc.unsavable_body_edit_cache.get().is_some(),
            "the answer is kept"
        );
        let applied = doc.edit_table_properties_in_place(0, |table| {
            table.set_alignment(crate::paragraph::Alignment::Right);
            Ok(())
        });
        assert!(matches!(applied, Ok(Some(()))));
        assert!(
            doc.unsavable_body_edit_cache.get().is_none(),
            "an applied edit drops it"
        );
    }

    #[test]
    fn a_cell_that_gains_its_first_property_has_other_table_content() {
        let mut doc = read_with_body(
            "<w:tbl><w:tblGrid><w:gridCol w:w=\"2000\"/></w:tblGrid><w:tr><w:tc><w:p/></w:tc></w:tr>\
             </w:tbl><w:p/>",
        );
        let bare = doc.table_mut(0).expect("the table").inner.clone();
        assert!(
            !writes_cell_properties(&bare.rows[0].cells[0]),
            "the cell has no w:tcPr"
        );
        let mut empty = bare.clone();
        empty.rows[0].cells[0].properties = Some(Default::default());
        assert!(
            same_table_content(&bare, &empty),
            "an empty w:tcPr is not written"
        );
        doc.table_mut(0)
            .and_then(|mut table| {
                table
                    .cell(0, 0)
                    .map(|mut cell| cell.set_vertical_alignment(crate::VerticalAlignment::Center))
            })
            .expect("the cell");
        let aligned = doc.table_mut(0).expect("the table").inner.clone();
        assert!(!same_table_content(&bare, &aligned));
        assert!(!same_table_content(&aligned, &bare));
    }

    #[test]
    fn a_change_of_row_cell_controls_is_a_change_of_table_content() {
        let mut doc = read_with_body(
            "<w:tbl><w:tblGrid><w:gridCol w:w=\"2000\"/><w:gridCol w:w=\"2000\"/></w:tblGrid><w:tr>\
             <w:tc><w:p/></w:tc><w:sdt><w:sdtPr><w:tag w:val=\"cell\"/></w:sdtPr><w:sdtContent>\
             <w:tc><w:p/></w:tc></w:sdtContent></w:sdt></w:tr></w:tbl><w:p/>",
        );
        let table = doc.table_mut(0).expect("the table").inner.clone();
        assert_eq!(
            table.rows[0].content_controls.len(),
            1,
            "the row holds a cell control"
        );
        let mut properties = table.clone();
        properties.properties = Some(Default::default());
        properties.rows[0].properties = Some(Default::default());
        assert!(same_table_content(&table, &properties));
        let mut controls = table.clone();
        controls.rows[0].content_controls.clear();
        assert!(!same_table_content(&table, &controls));
    }

    /// A document read from bytes whose body is `body`, as a file is read.
    fn read_with_body(body: &str) -> Document {
        let bytes = Document::new().to_bytes().expect("a new document saves");
        let mut package =
            OpcPackage::from_reader(std::io::Cursor::new(bytes)).expect("the package opens");
        let part = package.main_document_part().expect("a main document part");
        let xml = String::from_utf8(package.get_part(&part).expect("main part").to_vec())
            .expect("the main part is UTF-8");
        let start = xml.find("<w:body>").expect("a body") + "<w:body>".len();
        let end = xml.find("<w:sectPr").expect("section properties");
        package.set_part(
            &part,
            format!("{}{body}{}", &xml[..start], &xml[end..]).into_bytes(),
        );
        let mut out = std::io::Cursor::new(Vec::new());
        package.write_to(&mut out).expect("the package writes");
        Document::from_bytes(&out.into_inner()).expect("the document opens")
    }

    /// The local names of the nested namespace owners the body keeps.
    fn owner_names(doc: &Document) -> Vec<String> {
        doc.kept_main_part()
            .and_then(|kept| {
                kept.nested_namespace_owners().map(|owners| {
                    owners
                        .iter()
                        .map(|owner| owner.local_name.clone())
                        .collect()
                })
            })
            .unwrap_or_default()
    }

    /// A content control that declares `w15` and uses it: a nested namespace
    /// owner named `sdt`.
    fn control_owner() -> String {
        format!(
            r#"<w:sdt xmlns:w15="{W15}"><w:sdtPr><w15:repeatingSection/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>row</w:t></w:r></w:p></w:sdtContent></w:sdt>"#
        )
    }

    /// A paragraph that declares `w15` and uses it: a nested namespace owner
    /// named `p`.
    fn paragraph_owner() -> String {
        format!(r#"<w:p xmlns:w15="{W15}"><w:r><w15:mark/><w:t>owner</w:t></w:r></w:p>"#)
    }

    /// The body of `body` after a typed paragraph appended to it, and whether
    /// `keep_body_savable_after_typed_insert` then kept the savable answer.
    fn kept_after_typed_paragraph(body: &str, savable_before: bool) -> (Document, bool) {
        let mut doc = read_with_body(body);
        assert_eq!(doc.unsavable_body_edit(), None, "the body is savable");
        let slot = doc.content_count();
        doc.add_paragraph("typed");
        assert!(
            doc.unsavable_body_edit_cache.get().is_none(),
            "the insert drops the answer"
        );
        doc.keep_body_savable_after_typed_insert(slot, savable_before);
        let kept = doc.unsavable_body_edit_cache.get() == Some(&None);
        (doc, kept)
    }

    #[test]
    fn a_typed_paragraph_beside_a_content_control_owner_keeps_the_savable_answer() {
        assert_eq!(owner_names(&read_with_body(&control_owner())), ["sdt"]);
        let (doc, kept) = kept_after_typed_paragraph(&control_owner(), true);
        assert!(kept);
        assert_eq!(doc.unsavable_body_edit(), None);
    }

    #[test]
    fn a_typed_paragraph_beside_a_paragraph_owner_is_left_to_the_full_computation() {
        assert_eq!(owner_names(&read_with_body(&paragraph_owner())), ["p"]);
        let (doc, kept) = kept_after_typed_paragraph(&paragraph_owner(), true);
        assert!(!kept);
        assert_eq!(doc.unsavable_body_edit(), None);
    }

    #[test]
    fn keeping_after_an_insert_into_a_body_that_was_not_savable_marks_nothing() {
        let (_, kept) = kept_after_typed_paragraph(&control_owner(), false);
        assert!(!kept);
    }

    #[test]
    fn an_element_is_found_by_its_local_name_whatever_its_prefix() {
        let names = HashSet::from([b"hyperlink".as_slice(), b"graphic".as_slice()]);
        let found = |xml: &str| xml_holds_element_named(xml.as_bytes(), &names);
        assert!(found(
            r#"<w:p><w:hyperlink r:id="rId1"><w:r/></w:hyperlink></w:p>"#
        ));
        assert!(found(
            r#"<w:p><w:r><w:drawing><a:graphic/></w:drawing></w:r></w:p>"#
        ));
        assert!(found(r#"<p><hyperlink/></p>"#));
        assert!(!found(
            r#"<w:p><w:r><w:t>hyperlink graphic</w:t></w:r></w:p>"#
        ));
        assert!(!found(r#"<w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>"#));
        assert!(
            found("<w:p><unclosed"),
            "an unreadable block counts as holding one"
        );
    }
}
