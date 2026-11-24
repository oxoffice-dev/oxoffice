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
use oxoffice_docx_oxml::table::{CT_Row, CT_Tbl, CT_Tc, CellContent, ST_VerticalJc};
use oxoffice_docx_oxml::text::{
    CT_P, CT_R, RangeAnchor, RunContent, SpecialCharacter, declare_w14_on_part_root,
    story_field_instruction_has_name, story_simple_field_is_typed,
};
use oxoffice_docx_oxml::units::Twips;
use oxoffice_docx_oxml::web_settings::CT_WebSettings;
use oxoffice_opc::content_types;
use oxoffice_opc::relationship::rel_types;
use oxoffice_opc::{OpcPackage, PackageReadLimits, SaveOptions, write_atomic_file};
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
use crate::table::{Table, TableRef, validate_table_topology};

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
    FILLABLE_FORMS_NEED_ACCEPTED_VIEW, FONT_SWITCHES_EXCLUSIVE, FontPolicy,
    SYSTEM_FONTS_UNAVAILABLE, font_policy,
};
pub(crate) use story_types::GeneratedStoryOwner;
pub use story_types::{
    BodyItemOwners, BodyItemRef, ElementExtent, HeaderFooterKind, OwnerElementXml, StoryError,
    StoryItemKind, StoryKind,
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
#[cfg(debug_assertions)]
pub(crate) use content::BodyBlockItem;
mod headers_footers;
mod media;
mod notes;
mod numbering;
mod paragraph_routes;
use paragraph_routes::{body_paragraph_routes, control_items};
mod replace;
mod sections;
mod styles;
mod tables;
mod template;

use headers_footers::{
    header_footer_story_has_expected_root, header_type_index, is_content_relationship,
    materialize_header_footer_inheritance,
};
pub use numbering::{ListItemKind, NumberingJournal};
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
    reserve_u32, reserve_u32_or_lowest_free, scan_identifier_definitions,
    split_relationship_ids_at_head,
};
pub(crate) use main_part::{CommentParagraphRead, CommentParagraphValue};
use main_part::{
    EmptyNamespaceChildProof, FrameIdentifierProof, FrameNamespacePolicyEmpty,
    IdentifierChildProof, NamespaceFrameSpans, NamespaceSpanProof, ParsedChild,
    ScopedChildCorrespondence, StandaloneParagraphValue, full_direct_body_item_location,
    holds_field_words, picture_free_span_kind, producer_scoped_normalization_seed, root_start_tag,
    scan_body_block, scan_body_children, scoped_children_correspond, scoped_normalization_context,
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
pub(crate) use xml_ids::{
    apply_template_source_edits, drawing_id_spans, remap_xml_relationship_ids,
    unique_drawing_id_edits, uniquify_drawing_ids_in_xml, xml_relationship_ids_in_order,
};
use xml_ids::{
    authored_doc_pr_edits, authored_relationship_ids_in_source_order, drawing_ids_in_xml,
    drawing_occurrences, relationship_ids_by_blocks, relationship_xml_from_replayed,
    rewrite_authored_doc_pr_ids, story_attribute_value_span, xml_attribute_values,
    xml_relationship_ids_in_collection_range, xml_relationship_ids_in_order_with_bindings,
};

mod edited_paragraph;
mod fragment_checks;
mod leading_bookmarks;
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
    WordXmlOwner, content_fragment_for_insertion, copy_shown_notes, fragment_content_boundary,
    fragment_insertion_boundary, fragment_uses_w14, freshen_content_fragment_identities,
    freshen_taken_content_fragment_identities, insert_story_fragment, physical_table_cell_owner,
    referenced_relationships, reject_section_owning_content_fragment, serialize_content_fragment,
    standalone_body_item_span, story_owner_content_end, story_owner_content_end_in,
    validate_element_owner_relationships, validate_element_xml_identity_change,
    validate_fragment_block_item, validate_fragment_relationships,
    validate_serialized_block_content_control, validated_content_boundary,
    validated_direct_content_item, validated_insertion_boundary, without_w14_identities,
    xml_1_0_whitespace,
};
pub(crate) use fragment_checks::{
    close_content_fragment_namespaces, package_authoritative_body_fragment,
    package_authoritative_body_source, validate_fragment_block_content,
};
pub(crate) use leading_bookmarks::is_block_bookmark_marker;
use leading_bookmarks::{OwnedMarkers, leading_bookmark_start_item, owned_bookmark_marker_items};
pub(crate) use numbering_types::effective_numbering_reference;
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
    append_story_fragments_to_root, append_story_sources_to_root, built_in_character_style_id,
    decoded_story_namespace_declaration, localized_run_style_ids, missing_note_specials,
    names_renamed_run_style, note_reference_style, owners_reference_styles, remove_note_references,
    rename_run_styles, serialized_footnote_fragment, serialized_note_fragment,
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
pub(crate) use story_xml::{SourceBlockMarker, source_block_bookmarks, source_bookmark_end_ids};
use story_xml::{
    StoryNamespace, StorySdtContext, changed_story_namespace_scope, comment_move_source_edits,
    comment_reference_run_span, field_instruction_has_name, modeled_story_child,
    scoped_proof_marker, scoped_proof_sentinel, story_element_end, story_element_sdt_context,
    story_frame_local_name, story_namespace, story_range_paragraph_spans,
    story_range_paragraph_spans_from_inventory, story_sdt_context, story_word_prefixes_at,
    word_element, xml_decodes,
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
    block_paragraphs, cell_paragraph_routes, close_typed_story_drawing_namespaces,
    collect_relationship_ids, insert_html_content_into_cell, modeled_main_cell_route,
    modeled_main_cell_routes, nth_cell_in_control, nth_cell_in_table, nth_paragraph_in_body,
    nth_table_in_body, nth_table_in_body_ref, physical_table_cells, push_control_text,
    push_paragraph_line, push_table_text, visit_authored_drawings_mut, visit_cell,
};

mod canonicalize;
#[cfg(debug_assertions)]
use canonicalize::IdentifierSummary;
mod element_routes;
mod flush;
mod fragment_sources;
mod main_source;
mod open_save;
mod owner_snapshots;
pub use owner_snapshots::resolved_xml_events;
mod edit_in_place;
mod relationships;
mod staged_output;
mod staging;
use edit_in_place::InPlaceState;
pub use edit_in_place::{BodyBlocksChange, EditReport, EditUndo, EditUnit};
mod story_edits;
mod story_items;
mod story_pictures;
mod story_ranges;
mod story_sources;

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
    /// The relationships of its source part that the fragment references, as
    /// they were where it was taken (empty when it was built, not taken), so
    /// that an insertion where those ids name something else is refused.
    relationships: Vec<(oxoffice_opc::relationship::Relationship, Option<u64>)>,
    /// The bookmark starts written just before the item and the bookmark
    /// ends written just after it that belong to it (a heading's bookmark as
    /// Google Docs writes it, a bookmark around a whole table), each one
    /// element in the item's namespace scope. An insertion writes them
    /// around the item.
    markers: FragmentMarkers,
}

/// The bookmark markers a [`ContentFragment`] carries around its item.
#[derive(Debug, Clone, Default)]
pub(crate) struct FragmentMarkers {
    pub(crate) before: Vec<Vec<u8>>,
    pub(crate) after: Vec<Vec<u8>>,
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
        && a.rows
            .iter()
            .zip(&b.rows)
            .all(|(a, b)| same_row_content(a, b))
}

/// The part of [`same_table_content`] one row holds.
fn same_row_content(a: &CT_Row, b: &CT_Row) -> bool {
    a.extra_xml == b.extra_xml
        && a.content_controls == b.content_controls
        && a.cells.len() == b.cells.len()
        && a.cells
            .iter()
            .zip(&b.cells)
            .all(|(a, b)| same_cell_content(a, b))
}

/// The part of [`same_table_content`] one cell holds.
fn same_cell_content(a: &CT_Tc, b: &CT_Tc) -> bool {
    a.content == b.content
        && a.extra_xml == b.extra_xml
        && writes_cell_properties(a) == writes_cell_properties(b)
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
    preserved_comment_ids: HashSet<i32>,
    preserved_drawing_ids: HashSet<u32>,
    preserved_abstract_numbering_ids: HashSet<u32>,
    preserved_numbering_instance_ids: HashSet<u32>,
    preserved_part_names: HashSet<String>,
    authored_comment_ids: HashSet<i32>,
    authored_toc_bookmark_ids: HashSet<i32>,
    authored_bookmark_names: HashSet<String>,
    authored_story_parts: HashSet<String>,
    authored_story_relationship_ids: HashMap<String, HashSet<String>>,
    authored_nested_story_relationship_ids: HashMap<String, HashSet<String>>,
    /// Definitions read from each identifier part. A later scan replays a
    /// part only when its bytes and categories still match. This is the
    /// current scan, not a history of earlier ones.
    scanned_parts: Arc<HashMap<String, ScannedIdentifierPart>>,
    /// What the in-place edit under way has reserved, if one journals.
    journal: identifiers::JournalSlot,
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

/// Puts `reference` in `references`, in place of those of its type, at the
/// place among the section's references of the one it replaces.
fn place_story_reference(references: &mut Vec<HdrFtrRef>, mut reference: HdrFtrRef) {
    if let Some(replaced) = references
        .iter()
        .find(|existing| existing.hdr_ftr_type == reference.hdr_ftr_type)
    {
        reference.take_place_of(replaced);
    }
    let slot = story_reference_slot(references, reference.hdr_ftr_type);
    references.retain(|existing| existing.hdr_ftr_type != reference.hdr_ftr_type);
    references.insert(slot, reference);
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DrawingOccurrence {
    doc_pr: Option<(u32, Range<usize>)>,
    embeds: Vec<String>,
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
    /// Content relationships (owner part, id) a fragment `remove_content_at`
    /// took names. They stay until the document is written, so a removed
    /// fragment can be inserted again; the output drops those no element
    /// names then, with their parts when nothing else reaches them.
    pub(crate) freed_relationships: Vec<(String, String)>,
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
    /// Layout that may use system fonts (the default fallback, or system
    /// fonts first), tagged with that tier, computed on first use.
    layout_cache: Mutex<
        Option<(
            oxoffice_layout::SystemFonts,
            Arc<oxoffice_docx_layout::WordLayoutResult>,
        )>,
    >,
    /// Reusable engine of `layout_cache`'s tier, retained across edits.
    normal_layout_engine: Mutex<Option<oxoffice_docx_layout::engine::Engine>>,
    /// Bundled-font-only layout used by deterministic rendering.
    deterministic_layout_cache: Mutex<Option<Arc<oxoffice_docx_layout::WordLayoutResult>>>,
    /// Reusable bundled-font engine with caller faces at highest priority.
    bundled_fallback_layout_engine: Mutex<Option<oxoffice_docx_layout::engine::Engine>>,
    /// The in-place edit open on this document, and the undo a caller holds.
    in_place: InPlaceState,
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
                            | RunContent::Break(..)
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
            // The main part is there from the start, as in the Word-compatible
            // profile, so the package answers for its class before a save.
            let mut package = new_word_package(class);
            package.set_part(
                "/word/document.xml",
                document
                    .to_xml()
                    .expect("fresh Word document model must serialize"),
            );
            package
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
            freed_relationships: Vec::new(),
            package_signatures_invalidated: false,
            source_path: None,
            glossary: None,
            glossary_part_name: None,
            glossary_dirty: false,
            layout_cache: Mutex::new(None),
            normal_layout_engine: Mutex::new(None),
            deterministic_layout_cache: Mutex::new(None),
            bundled_fallback_layout_engine: Mutex::new(None),
            in_place: InPlaceState::default(),
            parsed: None,
            main_body_scans: Arc::new(Mutex::new(MainBodyScans::default())),
            main_part_blocks: None,
        }
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
pub(crate) fn relative_target(source_part: &str, target_part: &str) -> String {
    let dir = match source_part.rfind('/') {
        Some(pos) => &source_part[..=pos],
        None => "/",
    };
    match target_part.strip_prefix(dir) {
        Some(rest) if !rest.contains('/') => rest.to_string(),
        _ => target_part.to_string(),
    }
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

    #[test]
    fn a_font_embed_the_settings_refuse_leaves_no_reservation() {
        // Two `w:embedTrueTypeFonts`: the embed reserves the face's part name
        // and relationship id, then the settings refuse it.
        let bytes = Document::new().to_bytes().expect("a new document saves");
        let mut package =
            OpcPackage::from_reader(std::io::Cursor::new(bytes)).expect("the package opens");
        package.set_part(
            "/word/settings.xml",
            format!(
                "<w:settings xmlns:w=\"{}\"><w:embedTrueTypeFonts/><w:embedTrueTypeFonts/>\
                 </w:settings>",
                WORD_NAMESPACE
            )
            .into_bytes(),
        );
        let mut out = std::io::Cursor::new(Vec::new());
        package.write_to(&mut out).expect("the package writes");
        let bytes = out.into_inner();
        let fresh = Document::from_bytes(&bytes).expect("the document opens");
        let mut doc = Document::from_bytes(&bytes).expect("the document opens");
        let face = EmbeddedFont::new(
            EmbeddedFontKind::Regular,
            include_bytes!("../../oxoffice-layout/fonts/NotoSansSymbols2-bullets-subset.ttf")
                .to_vec(),
            "{00112233-4455-6677-8899-AABBCCDDEEFF}",
            FontEmbeddingLicense::new(true, "SIL Open Font License 1.1"),
        );
        let error = doc.embed_font("Probe", face).unwrap_err();
        assert!(
            error.to_string().contains("ambiguous font setting"),
            "{error}"
        );
        assert_eq!(
            doc.identifiers, fresh.identifiers,
            "the reservations are undone"
        );
        assert!(doc.package.same_package_as(&fresh.package));
    }
}
